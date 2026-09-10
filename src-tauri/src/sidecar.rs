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
use crate::ai_turn::ToolTurnContext;
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::Emitter;

mod transport;
use transport::http_client;

mod lifecycle;
mod recovery;
mod request_policy;
pub use lifecycle::{ensure_sidecar, ManagedSidecar, CANCEL_DRAIN_SECS, SIDECAR_PROTOCOL_VERSION};
use recovery::{conversation_delta, recoverable_assistant_text};
use request_policy::build as build_request_policy;

/// Run a streaming chat turn through the sidecar. Maps the sidecar's SSE events
/// onto the same Tauri emissions the in-process loop used (`ai://chat_chunk`,
/// `ai://note_stream_start`/`note_delta`/`note_stream_cancel`, and the tool
/// emits that fire from inside `execute_tool`), and returns the final message
/// array so `ask_ai_stream` can persist the conversation exactly as before.
pub async fn run_chat(
    turn: &ToolTurnContext,
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
    targeted_write: bool,
) -> Result<Vec<Value>> {
    let state = &turn.state;
    let base = ensure_sidecar(state).await?;
    let token = state
        .inner.ai
        .sidecar
        .lock()
        .await
        .as_ref()
        .map(|sidecar| sidecar.token.clone())
        .ok_or_else(|| anyhow!("sidecar disappeared after startup"))?;

    let oh = state.openharn_settings();
    let policy = build_request_policy(
        &oh,
        config,
        intent_is_tool,
        chat_mode,
        suppress_chat_output,
        selection_scoped,
        targeted_write,
    );
    let llama_base = policy.llama_base.clone();
    let model = policy.model.clone();
    let api_key = policy.api_key.clone();

    let tool_mode = policy.tool_mode;
    // The policy preserves the existing native/prompt-tools selection and
    // applies the same mutation, chat, and external-endpoint rules for every turn.

    let options = policy.options;
    // The epoch identifies this exact note/config/prompt revision. It remains
    // stable for all passes in this request; any changed input naturally yields
    // a different epoch and prevents a stale slot from being treated as valid.
    let mut epoch_hash = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    note_id.hash(&mut epoch_hash);
    config.model_path.hash(&mut epoch_hash);
    serde_json::to_string(&messages)
        .unwrap_or_default()
        .hash(&mut epoch_hash);
    serde_json::to_string(&tools)
        .unwrap_or_default()
        .hash(&mut epoch_hash);
    let epoch = epoch_hash.finish();
    let pass_kind = policy.pass_kind;
    let submitted_messages = messages.clone();
    // Keep ordinary direct answers bounded. Raising this to the full context
    // made a short question capable of producing thousands of tokens, which
    // looked like an endless/hallucinating generation. The previous 768-token
    // ceiling is intentional; tool turns remain separately bounded because
    // their output is structured arguments/results rather than prose.
    let max_output_tokens = policy.max_output_tokens;
    let body = json!({
        "request_id": request_id,
        "base_url": llama_base,
        "model": model,
        "api_key": api_key,
        "temperature": config.temperature,
        "max_tokens": max_output_tokens,
        "max_turns": config.max_turns.max(1) as usize,
        "messages": messages,
        "tools": tools,
        "options": options,
        "session": { "slot_id": 0, "epoch": epoch, "pass_kind": pass_kind },
    });

    let client = http_client();
    let resp = client
        .post(format!("{base}/v1/chat/stream"))
        .bearer_auth(&token)
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
            "options: mode={}, strict={}, prompt_tools={}, call_only={}, intent_is_tool={}, max_calls={}, max_output_tokens={}",
            tool_mode,
            options["strict"],
            options["prompt_tools"],
            options["call_only"],
            options
                .get("intent_is_tool")
                .map_or("null".to_string(), |v| v.to_string()),
            options
                .get("max_calls")
                .map_or(1, |v| v.as_u64().unwrap_or(1)),
            max_output_tokens,
        ),
    );

    let tool_names: Vec<String> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(String::from))
        .collect();
    if !tool_names.is_empty() {
        emit_debug("tools", &format!("offered: {}", tool_names.join(", ")));
    }

    let mut cancel_sent = false;
    // Once a cancel has been posted, the sidecar should emit its final `done`
    // within this window; if it is stuck (e.g. old binary), give up waiting.
    let cancel_drain = tokio::time::sleep(Duration::from_secs(CANCEL_DRAIN_SECS));
    tokio::pin!(cancel_drain);

    loop {
        if !cancel_sent && turn.cancellation.is_cancelled() {
            cancel_sent = true;
            emit_debug("cancel", "cancelling sidecar turn");
            let _ = handle.emit(
                "ai://note_stream_cancel",
                json!({ "noteId": note_id, "requestId": request_id }),
            );
            // Tell the sidecar to abort now instead of running to completion
            // (a stuck tool wait or a long generation would otherwise occupy
            // the single llama slot and drop the partial turn).
            let post = client
                .post(format!("{base}/v1/cancel"))
                .bearer_auth(&token)
                .json(&json!({ "request_id": request_id }))
                .send()
                .await;
            if let Err(e) = post {
                log::warn!("[sidecar] cancel request failed: {e}");
            }
            cancel_drain
                .as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_secs(CANCEL_DRAIN_SECS));
        }
        let chunk = tokio::select! {
            chunk = stream.next() => chunk,
            _ = turn.cancellation.cancelled() => continue,
            _ = &mut cancel_drain, if cancel_sent => break,
        };
        let Some(chunk) = chunk else { break };
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
                                crate::stream_chat::execute_tool(turn, &name, &args).await;
                            if turn.cancellation.is_cancelled() {
                                // The turn was cancelled while the tool ran (e.g.
                                // awaiting approval). Do not unblock the harness —
                                // cancel it so the partial turn closes promptly.
                                cancel_sent = true;
                                emit_debug("cancel", "dropping tool result (turn cancelled)");
                                let _ = handle.emit(
                                    "ai://note_stream_cancel",
                                    json!({ "noteId": note_id, "requestId": request_id }),
                                );
                                let _ = client
                                    .post(format!("{base}/v1/cancel"))
                                    .bearer_auth(&token)
                                    .json(&json!({ "request_id": request_id }))
                                    .send()
                                    .await;
                                cancel_drain.as_mut().reset(
                                    tokio::time::Instant::now()
                                        + Duration::from_secs(CANCEL_DRAIN_SECS),
                                );
                                continue;
                            }
                            let result_preview: String = result.chars().take(40).collect();
                            emit_debug("tool_result", &format!("{name} -> {result_preview}"));
                            // Unblock the harness.
                            let post = client
                                .post(format!("{base}/v1/tool-result"))
                                .bearer_auth(&token)
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
                                // A tool loop can complete with the final
                                // assistant message present in `done` but no
                                // corresponding streamed chat_chunk (for
                                // example when the upstream server batches the
                                // final SSE content). Do not leave the frontend
                                // with an empty assistant bubble: recover the
                                // last ordinary assistant message once, while
                                // still preferring normal streaming whenever it
                                // happened.
                                if !suppress_chat_output && !emitted_text {
                                    // Only recover text produced by this turn.
                                    // The previous implementation searched the
                                    // complete returned conversation after an
                                    // empty generation, so it replayed the last
                                    // successful answer as if it belonged to
                                    // the current prompt.
                                    let recovery_messages = if has_new_messages {
                                        new_messages.clone()
                                    } else {
                                        conversation_delta(&submitted_messages, &final_messages)
                                            .unwrap_or_default()
                                    };
                                    let recovered = recoverable_assistant_text(&recovery_messages);
                                    if let Some(content) = recovered {
                                        emit_debug(
                                            "gen",
                                            "recovered final assistant text from completed turn",
                                        );
                                        let _ = handle.emit(
                                            "ai://chat_chunk",
                                            json!({ "requestId": request_id, "delta": content }),
                                        );
                                    }
                                }
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
                                    conversation_delta(&submitted_messages, &final_messages)?
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
                                    if pt == 0 {
                                        0.0
                                    } else {
                                        cached as f64 / pt as f64
                                    }
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

#[cfg(test)]
mod tests;
