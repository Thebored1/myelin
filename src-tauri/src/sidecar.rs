//! openharn-myelin sidecar integration.
//!
//! Myelin spawns the `openharn-myelin` binary (the agent harness from
//! `src-tauri/openharn-myelin`) as a long-lived sidecar and drives it over HTTP.
//! Each chat turn we POST the conversation + Myelin's tool schemas to
//! `/v1/chat/stream`; the sidecar runs the openharn reliability loop against
//! `llama-server` directly and streams SSE events back. On a `tool` event we run
//! the REAL Myelin tool (note store / RAG / web) against our own `AppState` and
//! POST the result to `/v1/tool-result`, which unblocks the harness. This keeps
//! the tools where `AppState` lives while openharn owns the agent loop.
//!
//! The sidecar binary is bundled under `<resource_dir>/bin` (Tauri externalBin
//! naming: `openharn-myelin-<target-triple>`). An explicit `OPENHARN_MYELIN_BIN`
//! env var overrides resolution (handy for local dev / testing).

use crate::llama_server::ResolvedLlamaConfig;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{Emitter, Manager};

const SIDECAR_NAME: &str = "openharn-myelin";
const DEFAULT_PORT: u16 = 8091;

/// Reuse loopback connections for health checks and sidecar requests. The
/// sidecar is long-lived, so constructing a new client for every chat turn
/// needlessly throws away the connection pool.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// A running sidecar process plus the HTTP base we talk to it on.
pub struct ManagedSidecar {
    pub base: String,
    _child: Child,
}

impl Drop for ManagedSidecar {
    fn drop(&mut self) {
        let _ = self._child.kill();
    }
}

/// Full target triple for the current platform, used to locate the Tauri
/// externalBin build (`<name>-<triple>`).
fn target_triple() -> &'static str {
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "windows"),
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "macos")
    )))]
    {
        "unknown"
    }
}

/// Resolve the sidecar binary path. Precedence:
///   1. `OPENHARN_MYELIN_BIN` env var (absolute path).
///   2. `<resource_dir>/bin/<name>-<target-triple>` (Tauri externalBin).
///   3. `<resource_dir>/bin/<name>` (already-suffixed / dev bundle).
fn resolve_sidecar_bin(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("OPENHARN_MYELIN_BIN") {
        if !explicit.trim().is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    let dir = resource_dir?;
    let triple = target_triple();
    let with_ext = if cfg!(target_os = "windows") {
        format!("{SIDECAR_NAME}.exe")
    } else {
        SIDECAR_NAME.to_string()
    };
    // Tauri maps `resources/bin` -> `<resource_dir>/bin` at build time, but in
    // `tauri dev` the source tree (`<resource_dir>/resources/bin`) is used, so
    // probe both locations (mirrors how llama_server resolves its binaries).
    let mut roots: Vec<PathBuf> = vec![dir.join("bin"), dir.join("resources").join("bin")];
    roots.retain(|r| r.is_dir());
    let mut candidates: Vec<PathBuf> = Vec::new();
    if triple != "unknown" {
        let suffixed = if cfg!(target_os = "windows") {
            format!("{SIDECAR_NAME}-{triple}.exe")
        } else {
            format!("{SIDECAR_NAME}-{triple}")
        };
        for r in &roots {
            candidates.push(r.join(&suffixed));
        }
    }
    for r in &roots {
        candidates.push(r.join(&with_ext));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Spawn the sidecar if it isn't already running, wait for it to report healthy,
/// and return its HTTP base URL. Idempotent: re-checks the running process and
/// relaunches if it died.
pub async fn ensure_sidecar(state: &AppState) -> Result<String> {
    let mut guard = state.inner.sidecar.lock().await;

    if let Some(sc) = guard.as_ref() {
        if health_ok(&sc.base).await {
            return Ok(sc.base.clone());
        }
        // Process died — drop the stale handle (kill is best-effort).
        *guard = None;
    }

    let oh = state.openharn_settings();
    let resource_dir = state.handle.path().resource_dir().ok();

    // Binary resolution: explicit path in settings > OPENHARN_MYELIN_BIN env >
    // bundled/resource resolution.
    let bin = oh
        .bin_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| {
            std::env::var("OPENHARN_MYELIN_BIN")
                .ok()
                .map(|p| PathBuf::from(p))
                .filter(|p| p.exists())
        })
        .or_else(|| resolve_sidecar_bin(resource_dir.as_deref()))
        .ok_or_else(|| {
            anyhow!(
                "openharn-myelin sidecar binary not found. Build it (npm run build:sidecar) and \
             place it under the app's bin dir, or set its path in Settings > Agent (openharn)."
            )
        })?;

    let port = oh
        .port
        .filter(|&p| p != 0)
        .or_else(|| {
            std::env::var("OPENHARN_MYELIN_PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok())
        })
        .unwrap_or(DEFAULT_PORT);
    let base = format!("http://127.0.0.1:{port}");

    log::info!("[sidecar] launching {bin:?} on {base}");
    let mut command = Command::new(&bin);
    command.arg("--port").arg(port.to_string());
    command.stdout(Stdio::null()).stderr(Stdio::null());
    // Kill the sidecar if the app dies (mirrors llama-server's PR_SET_PDEATHSIG).
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to start sidecar at {bin:?}"))?;

    let managed = ManagedSidecar {
        base: base.clone(),
        _child: child,
    };

    // Wait for readiness (the sidecar prints a readiness line, but we just poll
    // /health so we don't depend on stdout capture).
    let mut ready = false;
    for _ in 0..50 {
        if health_ok(&base).await {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        return Err(anyhow!(
            "openharn-myelin sidecar at {base} did not become ready"
        ));
    }

    *guard = Some(managed);
    Ok(base)
}

async fn health_ok(base: &str) -> bool {
    http_client()
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(1))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Run a streaming chat turn through the sidecar. Maps the sidecar's SSE events
/// onto the same Tauri emissions the in-process loop used (`ai://chat_chunk`,
/// `ai://note_stream_start`/`note_delta`/`note_stream_cancel`, and the tool
/// emits that fire from inside `execute_tool`), and returns the final message
/// array so `ask_ai_stream` can persist the conversation exactly as before.
pub async fn run_chat(
    state: &AppState,
    config: &ResolvedLlamaConfig,
    messages: Vec<Value>,
    tools: Vec<Value>,
    request_id: &str,
    note_id: &str,
) -> Result<Vec<Value>> {
    let base = ensure_sidecar(state).await?;

    let oh = state.openharn_settings();
    // The sidecar reaches llama-server at this base URL. A Settings override
    // wins (handy if the resolved config's host/port isn't what's listening);
    // otherwise derive it from the llama config (config.base_url() + "/v1").
    let llama_base = oh
        .base_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| format!("{}/v1", config.base_url()));

    // If the model profile says it prefers prompt-tools (e.g. LFM2 at low quants),
    // force that mode regardless of user settings — native tool-calling doesn't work.
    let force_prompt_tools = config.prefers_prompt_tools.unwrap_or(false);

    let mut options = json!({
        "strict": force_prompt_tools || oh.strict,
        "prompt_tools": force_prompt_tools || oh.prompt_tools,
        "no_think": oh.no_think,
        "narrow": oh.narrow,
        "slm": oh.slm,
    });
    if let Some(mc) = oh.max_calls {
        options["max_calls"] = json!(mc);
    }
    if let Some(tm) = oh.total_max {
        options["total_max"] = json!(tm);
    }
    if let Some(tt) = oh.tool_timeout_secs {
        options["tool_timeout_secs"] = json!(tt);
    }
    if let Some(subset) = oh.tool_subset.as_ref() {
        let tools: Vec<String> = subset
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !tools.is_empty() {
            options["tool_subset"] = json!(tools);
        }
    }
    // Model-profile defaults (from bundled model-profiles.json), overridable
    // by the user's explicit OpenharnSettings. The config provides per-model
    // baked-in values; the user's oh.* settings win when non-empty.
    let tool_choice = oh
        .tool_choice
        .as_ref()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| config.tool_choice.as_ref())
        .map(|v| v.trim().to_string());
    if let Some(tc) = tool_choice {
        options["tool_choice"] = json!(tc);
    }
    let template_kwargs = oh
        .template_kwargs
        .as_ref()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| config.template_kwargs.as_ref())
        .map(|v| v.trim().to_string());
    if let Some(kw) = template_kwargs {
        options["template_kwargs"] = json!(kw);
    }

    let body = json!({
        "request_id": request_id,
        "base_url": llama_base,
        "model": config.model_name(),
        "temperature": config.temperature,
        "max_turns": config.max_turns.max(1) as usize,
        "messages": messages,
        "tools": tools,
        "options": options,
    });

    let client = http_client();
    let resp = client
        .post(format!("{base}/v1/chat/stream"))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("sidecar request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("sidecar returned {status}: {text}"));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut event_name: Option<String> = None;
    let mut event_data: Option<String> = None;
    let mut emitted_text = false;
    let mut last_tool: Option<String> = None;
    let mut final_messages: Vec<Value> = Vec::new();
    let handle = &state.handle;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| anyhow!("sidecar stream error: {e}"))?;
        buf.extend_from_slice(&bytes);

        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=nl).collect();
            let mut line = String::from_utf8_lossy(&line_bytes).to_string();
            if line.ends_with('\r') {
                line.pop();
            }
            let line = line.trim();

            if line.is_empty() {
                // End of an SSE event — dispatch it.
                if let (Some(name), Some(data)) = (event_name.take(), event_data.take()) {
                    match name.as_str() {
                        "chat_chunk" => {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                if let Some(delta) = v["delta"].as_str() {
                                    if !delta.is_empty() {
                                        emitted_text = true;
                                        let _ = handle.emit(
                                            "ai://chat_chunk",
                                            json!({ "requestId": request_id, "delta": delta }),
                                        );
                                    }
                                }
                            }
                        }
                        "note_start" => {
                            let _ =
                                handle.emit("ai://note_stream_start", json!({ "noteId": note_id }));
                        }
                        "note_delta" => {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                if let Some(delta) = v["delta"].as_str() {
                                    let _ = handle.emit(
                                        "ai://note_delta",
                                        json!({ "noteId": note_id, "delta": delta }),
                                    );
                                }
                            }
                        }
                        "note_cancel" => {
                            let _ = handle
                                .emit("ai://note_stream_cancel", json!({ "noteId": note_id }));
                        }
                        "tool" => {
                            let v: Value = match serde_json::from_str(&data) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let id = v["id"].as_str().unwrap_or("").to_string();
                            let name = v["name"].as_str().unwrap_or("").to_string();
                            let args = v["arguments"].as_str().unwrap_or("{}").to_string();
                            if name.is_empty() {
                                continue;
                            }
                            // Run the REAL Myelin tool (emits ai://chat_tool and,
                            // for write_note, ai://note_written on its own).
                            let result =
                                crate::stream_chat::execute_tool(state, &name, &args).await;
                            // Unblock the harness.
                            let post = client
                                .post(format!("{base}/v1/tool-result"))
                                .json(&json!({
                                    "request_id": request_id,
                                    "tool_call_id": id,
                                    "result": result,
                                }))
                                .send()
                                .await;
                            if post.is_err() {
                                log::warn!("[sidecar] failed to deliver tool result for {id}");
                            }
                        }
                        "done" => {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                if let Some(msgs) = v["messages"].as_array() {
                                    final_messages = msgs.clone();
                                }
                                last_tool = v["last_tool"].as_str().map(|s| s.to_string());
                            }
                        }
                        "usage" => {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                let _ = handle.emit(
                                    "ai://chat_usage",
                                    json!({
                                        "requestId": request_id,
                                        "promptTokens": v["prompt_tokens"].as_u64().unwrap_or(0),
                                        "completionTokens": v["completion_tokens"].as_u64().unwrap_or(0),
                                        "totalTokens": v["total_tokens"].as_u64().unwrap_or(0),
                                    }),
                                );
                            }
                        }
                        "error" => {
                            let msg = serde_json::from_str::<Value>(&data)
                                .ok()
                                .and_then(|v| v["message"].as_str().map(|s| s.to_string()))
                                .unwrap_or_else(|| "sidecar error".to_string());
                            return Err(anyhow!("{msg}"));
                        }
                        other => log::debug!("[sidecar] unhandled event: {other}"),
                    }
                }
                continue;
            }

            if let Some(rest) = line.strip_prefix("event:") {
                event_name = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                event_data = Some(rest.trim().to_string());
            }
        }
    }

    // Mirror the in-process loop's no-empty-bubble confirmation: if the model
    // produced no visible text and the final action was a note mutation, surface
    // a short confirmation instead of a silent turn.
    if !emitted_text
        && matches!(
            last_tool.as_deref(),
            Some("write_note") | Some("format_note") | Some("edit_notebook")
        )
    {
        let _ = handle.emit(
            "ai://chat_chunk",
            json!({ "requestId": request_id, "delta": "Done — I've updated your note." }),
        );
    }

    Ok(final_messages)
}
