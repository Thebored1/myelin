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
const SIDECAR_PROTOCOL_VERSION: u64 = 3;

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
        if compatible_health(&sc.base).await {
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
        if compatible_health(&base).await {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        return Err(anyhow!(
            "openharn-myelin sidecar at {base} is unavailable or incompatible \
             (required protocol {SIDECAR_PROTOCOL_VERSION}). Rebuild it with \
             `npm run build:sidecar:debug`."
        ));
    }

    *guard = Some(managed);
    Ok(base)
}

async fn compatible_health(base: &str) -> bool {
    let Ok(response) = http_client()
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(1))
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<Value>()
        .await
        .ok()
        .and_then(|health| health["protocol_version"].as_u64())
        == Some(SIDECAR_PROTOCOL_VERSION)
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
    // Host-computed deterministic TOOL/CHAT intent:
    //   Some(true)  → enter the tool loop (user wants an operation)
    //   Some(false) → skip the tool loop and answer directly (chat mode)
    //   None        → let the sidecar's per-request policy decide
    intent_is_tool: Option<bool>,
    chat_mode: bool,
    // Operation mode is tool-only: the editor and tool indicators communicate
    // progress/results, while model prose must not become a chat reply.
    suppress_chat_output: bool,
    selection_scoped: bool,
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

    // Tool format is user-controlled. Auto leaves the Openharn per-request
    // policy in charge; Native always uses the model's native function-call
    // format; Prompt tools uses text-form calls. Model profiles never force a
    // format because a setting that helps one model can hurt another.
    let tool_mode = match oh.tool_mode.trim().to_ascii_lowercase().as_str() {
        "native" => "native",
        "prompt" | "prompt_tools" => "prompt",
        _ => "auto",
    };
    let explicit_prompt = tool_mode == "prompt";
    // Auto is authoritative: ignore legacy manual booleans that may have been
    // persisted by older builds. Openharn decides native vs prompt-tools per
    // request and enables prompt-tools only as a recovery path when needed.
    // Native mode is shared across Chat and Operation so switching modes keeps
    // the same rendered prompt prefix. Strict prompt-tools remains available as
    // an explicit setting or as the sidecar's recovery path after native fails.
    let use_prompt_tools = explicit_prompt;
    let use_strict = explicit_prompt && (oh.strict || suppress_chat_output);
    // Operation is an explicit user instruction to act, not a routing hint.
    // In native function-calling mode `call_only` becomes `tool_choice: required`
    // below; in prompt-tool mode it selects the call-only grammar.
    let call_only = suppress_chat_output
        || (explicit_prompt && oh.call_only && intent_is_tool == Some(true));

    let mut options = json!({
        "strict": use_strict,
        "prompt_tools": use_prompt_tools,
        "prefers_prompt_tools": false,
        "call_only": call_only,
        // Model-based intent classification is skipped when the host provides
        // a deterministic intent_is_tool below. When intent_is_tool is None,
        // the sidecar defaults to friendly_results=false and enters the tool
        // loop normally — the per-request policy handles abstention via
        // harness_decompose (plan_len==0 → NO_TOOL).
        // An explicit host intent must use the friendly CHAT/TOOL branches,
        // but it must not run the model classifier: intent_is_tool below is
        // authoritative and already computed by Myelin.
        "friendly_results": intent_is_tool.is_some(),

        // Ordinary chat should never surface model reasoning as visible
        // `<think>` blocks. Operation mode may retain the user's configured
        // reasoning setting because it can help tool selection/edit quality.
        "no_think": oh.no_think || chat_mode,
        "narrow": false,
        "slm": false,
        "chat_mode": chat_mode,
        "selection_scoped": selection_scoped,
        "native_first": !explicit_prompt,
    });
    // Host-computed deterministic intent overrides model-based classification.
    // The sidecar uses this value directly and skips the separate model
    // inference that used to cost ~8s per turn.
    if let Some(is_tool) = intent_is_tool {
        options["intent_is_tool"] = json!(is_tool);
    }
    // Operation mode executes one action per model turn. This gives the model the
    // real result before it chooses the next action or emits its own completion,
    // preventing several competing write_note calls from one generation.
    // Auto mode keeps the existing configured/default limit.
    options["max_calls"] = json!(if intent_is_tool == Some(true) {
        1
    } else {
        oh.max_calls
            .unwrap_or(if tool_mode == "prompt" { 3 } else { 1 })
    });
    if let Some(tm) = oh.total_max {
        options["total_max"] = json!(tm);
    }
    if let Some(tt) = oh.tool_timeout_secs {
        options["tool_timeout_secs"] = json!(tt);
    }
    if let Some(gt) = oh.generation_timeout_secs {
        options["generation_timeout_secs"] = json!(gt);
    }

    // These are explicit user settings. Do not inherit tool_choice or template
    // kwargs from a model profile; those options can be harmful on larger models.
    if let Some(tc) = oh.tool_choice.as_ref().filter(|v| !v.trim().is_empty()) {
        options["tool_choice"] = json!(tc.trim());
    }
    if let Some(kw) = oh.template_kwargs.as_ref().filter(|v| !v.trim().is_empty()) {
        options["template_kwargs"] = json!(kw);
    }

    // The epoch identifies this exact note/config/prompt revision. It remains
    // stable for all passes in this request; any changed input naturally yields
    // a different epoch and prevents a stale slot from being treated as valid.
    let mut epoch_hash = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    note_id.hash(&mut epoch_hash);
    config.model_path.hash(&mut epoch_hash);
    serde_json::to_string(&messages).unwrap_or_default().hash(&mut epoch_hash);
    serde_json::to_string(&tools).unwrap_or_default().hash(&mut epoch_hash);
    let epoch = epoch_hash.finish();
    let pass_kind = if intent_is_tool == Some(true) {
        "tool selection"
    } else {
        "direct answer"
    };
    let submitted_messages = messages.clone();
    let body = json!({
        "request_id": request_id,
        "base_url": llama_base,
        "model": config.model_name(),
        "temperature": config.temperature,
        "max_tokens": 4096,
        "max_turns": config.max_turns.max(1) as usize,
        "messages": messages,
        "tools": tools,
        "options": options,
        "session": { "slot_id": 0, "epoch": epoch, "pass_kind": pass_kind },
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
    let mut emitted_generation_start = false;
    let mut last_tool: Option<String> = None;
    let mut final_messages: Vec<Value> = Vec::new();
    let mut new_messages: Vec<Value> = Vec::new();
    let mut has_new_messages = false;
    let handle = &state.handle;

    // Emit a debug event so the frontend can show what the model is doing.
    let emit_debug = |kind: &str, msg: &str| {
        let _ = handle.emit(
            "ai://debug_event",
            json!({
                "kind": kind,
                "msg": msg,
                "requestId": request_id,
            }),
        );
    };

    emit_debug(
        "config",
        &format!(
            "options: mode={}, strict={}, prompt_tools={}, call_only={}, intent_is_tool={}, max_calls={}",
            tool_mode,
            options["strict"],
            options["prompt_tools"],
            options["call_only"],
            options
                .get("intent_is_tool")
                .map_or("null".to_string(), |v| v.to_string()),
            options
                .get("max_calls")
                .map_or(1, |v| v.as_u64().unwrap_or(1))
        ),
    );

    let tool_names: Vec<String> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(String::from))
        .collect();
    if !tool_names.is_empty() {
        emit_debug("tools", &format!("offered: {}", tool_names.join(", ")));
    }

    loop {
        // Wait directly for either upstream data or the cancellation signal.
        // The old 250 ms timeout loop could leave cancellation latent and added
        // needless polling around the first visible model delta.
        if state.ai_cancel_requested() {
            emit_debug("cancel", "generation stopped by user");
            let _ = handle.emit(
                "ai://note_stream_cancel",
                json!({ "noteId": note_id, "requestId": request_id }),
            );
            break;
        }
        let chunk = tokio::select! {
            chunk = stream.next() => chunk,
            _ = state.wait_for_ai_cancel() => {
                emit_debug("cancel", "generation stopped by user");
                let _ = handle.emit(
                    "ai://note_stream_cancel",
                    json!({ "noteId": note_id, "requestId": request_id }),
                );
                break;
            }
        };
        let Some(chunk) = chunk else { break };
        if state.ai_cancel_requested() {
            emit_debug("cancel", "generation stopped by user");
            let _ = handle.emit(
                "ai://note_stream_cancel",
                json!({ "noteId": note_id, "requestId": request_id }),
            );
            break;
        }
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
                                        if !emitted_generation_start {
                                            emitted_generation_start = true;
                                            emit_debug("gen", "first model delta received");
                                        }
                                        if !suppress_chat_output {
                                            let _ = handle.emit(
                                                "ai://chat_chunk",
                                                json!({ "requestId": request_id, "delta": delta }),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        "note_start" => {
                            if !emitted_generation_start {
                                emitted_generation_start = true;
                                emit_debug("gen", "first model note delta received");
                            }
                            let _ = handle.emit(
                                "ai://note_stream_start",
                                json!({ "noteId": note_id, "requestId": request_id }),
                            );
                        }
                        "note_delta" => {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                if let Some(delta) = v["delta"].as_str() {
                                    let _ = handle.emit(
                                        "ai://note_delta",
                                        json!({
                                            "noteId": note_id,
                                            "requestId": request_id,
                                            "delta": delta
                                        }),
                                    );
                                }
                            }
                        }
                        "note_cancel" => {
                            let _ = handle.emit(
                                "ai://note_stream_cancel",
                                json!({ "noteId": note_id, "requestId": request_id }),
                            );
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
                            let args_preview: String = args.chars().take(80).collect();
                            emit_debug("tool", &format!("executing {name}(…{args_preview}…)"));
                            // Run the REAL Myelin tool (emits ai://chat_tool and,
                            // for write_note, ai://note_written on its own).
                            let result =
                                crate::stream_chat::execute_tool(state, &name, &args).await;
                            let result_preview: String = result.chars().take(40).collect();
                            emit_debug("tool_result", &format!("{name} -> {result_preview}"));
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
                                if let Some(msgs) = v["new_messages"].as_array() {
                                    new_messages = msgs.clone();
                                    has_new_messages = true;
                                }
                                last_tool = v["last_tool"].as_str().map(|s| s.to_string());
                                if let Some(ref lt) = last_tool {
                                    emit_debug("done", &format!("turn complete (last tool: {lt})"));
                                } else {
                                    emit_debug("done", "turn complete (chat)");
                                }
                                // `done` is terminal for this request. Do not
                                // keep consuming a malformed/late stream that
                                // could dispatch another tool after completion.
                                return Ok(if has_new_messages {
                                    new_messages.clone()
                                } else {
                                    // Protocol v1 returned only the complete
                                    // conversation. Never append that whole
                                    // array as a delta: doing so recursively
                                    // duplicates every prior turn and destroys
                                    // prompt-prefix cache reuse.
                                    conversation_delta(
                                        &submitted_messages,
                                        &final_messages,
                                    )?
                                });
                            }
                        }
                        "debug" => {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                let kind = v["kind"].as_str().unwrap_or("sidecar");
                                let message = v["message"].as_str().unwrap_or("");
                                emit_debug(kind, message);
                            }
                        }
                        "usage" => {
                            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                let pt = v["prompt_tokens"].as_u64().unwrap_or(0);
                                let ct = v["completion_tokens"].as_u64().unwrap_or(0);
                                let cached = v["cached_tokens"].as_u64().unwrap_or(0);
                                let evaluated = v["evaluated_tokens"]
                                    .as_u64()
                                    .unwrap_or_else(|| pt.saturating_sub(cached));
                                let ratio = v["cache_reuse_ratio"].as_f64().unwrap_or_else(|| {
                                    if pt == 0 { 0.0 } else { cached as f64 / pt as f64 }
                                });
                                let _ = handle.emit(
                                    "ai://chat_usage",
                                    json!({
                                        "requestId": request_id,
                                        "promptTokens": pt,
                                        "completionTokens": ct,
                                        "totalTokens": v["total_tokens"].as_u64().unwrap_or(0),
                                        "cachedTokens": cached,
                                        "evaluatedTokens": evaluated,
                                        "cacheReuseRatio": ratio,
                                    }),
                                );
                                emit_debug(
                                    "usage",
                                    &format!(
                                        "prompt={pt}, cached={cached}, evaluated={evaluated}, reuse={:.1}%, completion={ct}",
                                        ratio * 100.0
                                    ),
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

    // Keep operation turns tool-only. In other modes, retain the concise
    // confirmation when a note mutation completed without model prose.
    if !suppress_chat_output
        && !emitted_text
        && matches!(
            last_tool.as_deref(),
            Some("write_note")
                | Some("append_note")
                | Some("prepend_note")
                | Some("replace_in_note")
                | Some("insert_after_line")
                | Some("delete_in_note")
                | Some("format_note")
                | Some("edit_notebook")
        )
    {
        let _ = handle.emit(
            "ai://chat_chunk",
            json!({ "requestId": request_id, "delta": "Done — I've updated your note." }),
        );
    }

    Ok(final_messages)
}

fn conversation_delta(submitted: &[Value], returned: &[Value]) -> Result<Vec<Value>> {
    if returned.len() < submitted.len() || returned[..submitted.len()] != *submitted {
        return Err(anyhow!(
            "sidecar returned neither new_messages nor a compatible conversation suffix"
        ));
    }
    Ok(returned[submitted.len()..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::conversation_delta;
    use serde_json::json;

    #[test]
    fn legacy_full_conversation_is_reduced_to_delta() {
        let submitted = vec![
            json!({"role": "system", "content": "policy"}),
            json!({"role": "user", "content": "hello"}),
        ];
        let mut returned = submitted.clone();
        returned.push(json!({"role": "assistant", "content": "hi"}));
        let delta = conversation_delta(&submitted, &returned).unwrap();
        assert_eq!(delta, vec![json!({"role": "assistant", "content": "hi"})]);
    }

    #[test]
    fn incompatible_full_conversation_is_rejected() {
        let submitted = vec![json!({"role": "user", "content": "hello"})];
        let returned = vec![json!({"role": "system", "content": "wrong"})];
        assert!(conversation_delta(&submitted, &returned).is_err());
    }
}
