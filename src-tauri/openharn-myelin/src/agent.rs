//! The openharn harness loop, made async and streaming for the Myelin sidecar.
//!
//! Same reliability behaviour as `openharn/src/agent.rs::run` — tool-call text
//! recovery, context-fit, per-turn + total call limits, exact-repeat circuit
//! breaker, optional strict grammar / prompt-tools — but instead of executing
//! tools locally it emits a `Tool` event and BLOCKS until Myelin posts the real
//! result back (Myelin runs the actual note-store / RAG / web tools against its
//! own AppState). Assistant text and the live `write_note` body are streamed out
//! as they are generated.

use crate::harness;
use crate::policy;
use crate::protocol::Out;
use crate::server::Pending;
use crate::upstream::emit_prompt as emit_model_prompt;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) fn normalize_lfm_tool_arguments(messages: &mut [Value], model: &str) {
    if !model.to_ascii_lowercase().contains("lfm2") {
        return;
    }
    for message in messages {
        let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for call in calls {
            let Some(arguments) = call
                .get_mut("function")
                .and_then(|function| function.get_mut("arguments"))
            else {
                continue;
            };
            let Some(raw) = arguments.as_str() else {
                continue;
            };
            if let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(raw) {
                *arguments = parsed;
            }
        }
    }
}

/// A successful mutating tool call is terminal for this request. Small models
/// often treat the tool result as a prompt to rewrite the same note again; that
/// is both unnecessary and dangerous because the host clears the open-note
/// context as soon as the request completes.
const INTENT_ROUTING_RULES: &str = "Classify the latest user message by calling the classify_intent tool exactly once. TOOL means the user asks for any operation to be performed. Every instruction, command, or action request is TOOL; unless another target is named, it is work on the open note. Operations include creating, writing, adding, editing, revising, formatting, deleting, reading, finding, searching, fetching, browsing, looking up, and researching. CHAT means the user only wants a direct answer, explanation, capability description, greeting, thanks, small talk, opinion, or general knowledge without an operation. Questions are CHAT unless they explicitly ask you to perform an operation. Examples: \"write this on the note\", \"add a poem\", and \"search my notes for Rust\" are TOOL; \"hey\", \"what can you do?\", \"what does this poem mean?\", and \"what is Rust?\" are CHAT.";

/// Ask the same model/session to classify the turn via a virtual tool call. This
/// deliberately uses the live system/note history rather than a standalone
/// completion, preserving the main conversation's KV-cache prefix in the single
/// llama-server slot. The virtual call is consumed locally and never reaches the
/// host or user-visible tool list.
/// Whole-note replacement can safely preview from an empty buffer. Append and
/// insertion requests cannot: until the model finishes declaring their mode, a
/// replacement preview would temporarily hide the existing note.
pub(crate) fn should_stream_note_preview(user_text: &str, selection_scoped: bool) -> bool {
    policy::should_stream_note_preview(user_text, selection_scoped)
}

pub(crate) async fn detect_intent_in_session(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    model: &str,
    history: &[Value],
    tx: &mpsc::Sender<Out>,
    cancel: &watch::Receiver<bool>,
) -> bool {
    let route_schemas = json!([{
        "type": "function",
        "function": {
            "name": "classify_intent",
            "description": "Classify the latest user message using the routing rules in the system prompt.",
            "parameters": {
                "type": "object",
                "properties": {
                    "intent": {
                        "type": "string",
                        "enum": ["CHAT", "TOOL"],
                        "description": "CHAT for a direct answer; TOOL for an operation."
                    }
                },
                "required": ["intent"]
            }
        }
    }]);

    let mut route_history = history.to_vec();
    if let Some(system) = route_history
        .iter_mut()
        .find(|message| message["role"].as_str() == Some("system"))
    {
        let base = system["content"].as_str().unwrap_or("");
        system["content"] = json!(format!("{base}\n\n{INTENT_ROUTING_RULES}"));
    } else {
        route_history.insert(
            0,
            json!({ "role": "system", "content": INTENT_ROUTING_RULES }),
        );
    }
    let wire = harness::flatten_for_prompt_tools(&route_history, &route_schemas);
    let body = json!({
        "model": model,
        "messages": wire,
        "temperature": 0.0,
        "max_tokens": 32,
        "stream": true,
        "stream_options": { "include_usage": true },
        "cache_prompt": true,
        "id_slot": 0,
        "grammar": harness::tool_grammar(&route_schemas, "call"),
    });
    let _ = tx
        .send(Out::Debug {
            kind: "intent_prompt".into(),
            message: format!("In-session classify_intent tool\n{INTENT_ROUTING_RULES}"),
        })
        .await;
    emit_model_prompt(tx, "INTENT virtual-tool request messages:", &body).await;

    let mut request = client.post(url).json(&body);
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let (content, mut calls, _) = match request.send().await {
        Ok(response) if response.status().is_success() => {
            match stream_upstream(
                response,
                tx,
                false,
                true,
                false,
                &route_schemas,
                cancel,
                120,
            )
            .await
            {
                Ok(result) => result,
                Err(_) => (String::new(), Vec::new(), false),
            }
        }
        _ => (String::new(), Vec::new(), false),
    };
    if calls.is_empty() {
        calls = harness::parse_text_tool_calls(&content, &route_schemas).unwrap_or_default();
    }
    let intent = calls
        .first()
        .and_then(|call| call["function"]["arguments"].as_str())
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .and_then(|arguments| arguments["intent"].as_str().map(str::to_owned))
        .map(|intent| intent.to_ascii_uppercase());
    // Default to TOOL on ambiguity: losing a genuine note-write is worse than
    // a greeting occasionally entering the tool loop.
    let is_tool = intent.as_deref() != Some("CHAT");
    let raw = intent.unwrap_or_else(|| "(no valid tool call; defaulting to TOOL)".into());
    let _ = tx
        .send(Out::Debug {
            kind: "intent_result".into(),
            message: format!(
                "virtual tool result: {raw}; decision: {}",
                if is_tool { "TOOL" } else { "CHAT" }
            ),
        })
        .await;
    is_tool
}

/// Drive one user request to completion, streaming events on `tx` and requesting
/// tool execution from Myelin via the `pending` registry. `cancel` is the
/// request's cancellation flag: the host posts to `/v1/cancel` to flip it, which
/// aborts the upstream stream and any pending tool wait. On cancel the loop
/// emits a final `done` carrying the partial history so already-executed tool
/// calls are not lost from the conversation.

/// Ask Myelin to run a tool: emit a `Tool` event, register a oneshot keyed by
/// request+call id, and await the result Myelin posts to `/v1/tool-result`.
/// Aborts the wait immediately when the request is cancelled.
pub(crate) async fn dispatch_tool(
    tx: &mpsc::Sender<Out>,
    pending: &Pending,
    request_id: &str,
    call_id: &str,
    name: &str,
    args_raw: &str,
    timeout_secs: u64,
    cancel: &watch::Receiver<bool>,
) -> String {
    let key = format!("{request_id}:{call_id}");
    let (otx, orx) = oneshot::channel::<String>();
    pending.lock().await.insert(key.clone(), otx);

    let _ = tx
        .send(Out::Tool {
            id: call_id.to_string(),
            name: name.to_string(),
            arguments: args_raw.to_string(),
        })
        .await;

    let mut cancel = cancel.clone();
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(timeout_secs), orx) => match result {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                pending.lock().await.remove(&key);
                format!("Tool '{name}' failed: the host closed the result channel.")
            }
            Err(_) => {
                pending.lock().await.remove(&key);
                format!("Tool '{name}' timed out after {timeout_secs}s.")
            }
        },
        _ = cancel.changed() => {
            pending.lock().await.remove(&key);
            format!("Tool '{name}' was cancelled before it completed.")
        }
    }
}

/// Read the upstream SSE stream: forward assistant text as `ChatChunk`, stream
/// native function-call `write_note` arguments or prompt-tools content as
/// `NoteStart`/`NoteDelta` when they arrive incrementally, and assemble the
/// tool-call deltas. No text is fabricated if the upstream server batches SSE
/// chunks.
pub(crate) async fn stream_upstream_with_timeout(
    resp: reqwest::Response,
    tx: &mpsc::Sender<Out>,
    no_think: bool,
    suppress_text_call: bool,
    stream_note_preview: bool,
    schemas: &Value,
    cancel: &watch::Receiver<bool>,
    timeout_secs: u64,
) -> Result<(String, Vec<Value>, bool), String> {
    stream_upstream(
        resp,
        tx,
        no_think,
        suppress_text_call,
        stream_note_preview,
        schemas,
        cancel,
        timeout_secs,
    )
    .await
}

pub(crate) async fn stream_upstream(
    resp: reqwest::Response,
    tx: &mpsc::Sender<Out>,
    no_think: bool,
    suppress_text_call: bool,
    stream_note_preview: bool,
    schemas: &Value,
    cancel: &watch::Receiver<bool>,
    idle_timeout_secs: u64,
) -> Result<(String, Vec<Value>, bool), String> {
    let mut content = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    // Live note-streaming state.
    let mut note_streaming = false;
    let mut note_emitted = String::new();
    let mut note_cancelled = false;
    // Once a note-mutating tool starts, stop echoing prose to chat (it duplicates
    // the note in the editor).
    let mut suppress_prose = false;

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let stream_started = Instant::now();
    let mut first_delta = false;
    let mut cancel = cancel.clone();

    loop {
        // Cancellation aborts mid-generation: drop the upstream connection so
        // the llama slot frees immediately.
        if *cancel.borrow() {
            return Err("cancelled by user".to_string());
        }
        let chunk = tokio::time::timeout(
            Duration::from_secs(idle_timeout_secs.max(1)),
            async {
                tokio::select! {
                    chunk = stream.next() => Ok(chunk),
                    _ = cancel.changed() => Err("cancelled by user"),
                }
            },
        )
        .await
        .map_err(|_| {
            format!(
                "upstream stream idle for {idle_timeout_secs}s while waiting for llama-server output"
            )
        })??;
        let Some(chunk) = chunk else { break };
        let bytes = chunk.map_err(|e| {
            if e.is_timeout() {
                "upstream stream stalled while waiting for llama-server output".to_string()
            } else {
                format!("upstream stream error: {e}")
            }
        })?;
        buf.extend_from_slice(&bytes);

        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim();
            let data = match line.strip_prefix("data:") {
                Some(d) => d.trim(),
                None => continue,
            };
            if data == "[DONE]" {
                buf.clear();
                return finish(content, tool_calls, no_think, note_streaming);
            }
            let chunk_json: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Detect error events in the streaming response. llama-server may
            // emit an error mid-stream when it fails to decode a tool call
            // (e.g. "could not decode tool: unexpected end of JSON input").
            // Previously these were silently skipped (no "choices" key),
            // causing the model to appear to return nothing and triggering
            // the native-empty fallback — which adds a wasted round-trip.
            // Surface the error so the caller can retry with prompt-tools.
            if let Some(err) = chunk_json.get("error") {
                let msg = err.as_str().unwrap_or("unknown error");
                return Err(format!("completion parsing error: {msg}"));
            }
            // llama-server emits usage on the final chunk when include_usage is set.
            // Always forward the event even when values are zero so the frontend can
            // distinguish "no usage data yet" from "usage is zero".
            if let Some(usage) = chunk_json.get("usage") {
                let pt = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                let ct = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
                let tt = usage["total_tokens"].as_u64().unwrap_or(0) as u32;
                let cached = usage["prompt_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .or_else(|| chunk_json["timings"]["cache_n"].as_u64())
                    .unwrap_or(0) as u32;
                let evaluated = pt.saturating_sub(cached);
                let ratio = if pt == 0 {
                    0.0
                } else {
                    cached as f64 / pt as f64
                };
                let _ = tx
                    .send(Out::Usage {
                        prompt_tokens: pt,
                        completion_tokens: ct,
                        total_tokens: tt,
                        cached_tokens: cached,
                        evaluated_tokens: evaluated,
                        cache_reuse_ratio: ratio,
                    })
                    .await;
            }
            let choice = match chunk_json["choices"].get(0) {
                Some(c) => c,
                None => continue,
            };
            let delta = &choice["delta"];

            if let Some(t) = delta["content"].as_str() {
                if !t.is_empty() {
                    if !first_delta {
                        first_delta = true;
                        let _ = tx
                            .send(Out::Debug {
                                kind: "first_model_delta".into(),
                                message: format!(
                                    "elapsed_ms={}",
                                    stream_started.elapsed().as_millis()
                                ),
                            })
                            .await;
                    }
                    content.push_str(t);
                    if !suppress_prose && !suppress_text_call {
                        let _ = tx.send(Out::ChatChunk(t.to_string())).await;
                    }

                    // Prompt-tools calls arrive as ordinary content rather than
                    // OpenAI `tool_calls` deltas. Extract the decoded content
                    // string from each upstream delta and stream only that real
                    // generated text into the editor. If the server batches its
                    // SSE output, the UI can only display that batch when it
                    // arrives; it must not invent text ahead of the model.
                    if stream_note_preview
                        && suppress_text_call
                        && content.contains("write_note")
                        && !note_cancelled
                    {
                        // Do not classify a partial `find` field as an edit:
                        // weak models can emit transient/incomplete fields while
                        // the replacement body is still streaming. Only an
                        // explicit append or a fully valid JSON object can cancel
                        // the replace preview.
                        let partial_mode = harness::partial_field(&content, "mode");
                        let parsed = serde_json::from_str::<Value>(&content).ok();
                        let is_append = partial_mode.as_deref() == Some("append");
                        let complete_find = parsed
                            .as_ref()
                            .and_then(|v| v.get("find"))
                            .and_then(Value::as_str)
                            .map(|f| !f.trim().is_empty())
                            .unwrap_or(false);
                        let is_replace = !is_append && !complete_find;
                        if !is_replace {
                            if note_streaming {
                                let _ = tx.send(Out::NoteCancel).await;
                                note_streaming = false;
                            }
                            note_cancelled = true;
                        } else if let Some(c) = harness::extract_partial_content(&content) {
                            if !c.is_empty() && !note_streaming {
                                let _ = tx.send(Out::NoteStart).await;
                                note_streaming = true;
                            }
                            if c.len() > note_emitted.len() && c.starts_with(&note_emitted) {
                                let new_part = c[note_emitted.len()..].to_string();
                                if !new_part.is_empty() {
                                    let _ = tx.send(Out::NoteDelta(new_part)).await;
                                }
                                note_emitted = c;
                            }
                        }
                    }

                    // Some chat templates append a closing tool wrapper but never
                    // emit the upstream [DONE] event. Once that wrapper arrives,
                    // the call is terminal: execute it immediately if it parses,
                    // otherwise fail now so the speculative note is reverted
                    // instead of leaving the UI timer running until timeout.
                    if suppress_text_call
                        && [
                            "</tool_call>",
                            "<|tool_call_end|>",
                            "<|eot_id|>",
                            "<|end_of_text|>",
                        ]
                        .iter()
                        .any(|marker| content.contains(marker))
                    {
                        if harness::parse_text_tool_calls(&content, schemas).is_some() {
                            return finish(content, tool_calls, no_think, note_streaming);
                        }
                        if note_streaming {
                            let _ = tx.send(Out::NoteCancel).await;
                        }
                        return Err(
                            "Generation ended with an incomplete write_note call. Live preview reverted; no changes were saved."
                                .to_string(),
                        );
                    }
                }
            }

            if let Some(tcs) = delta["tool_calls"].as_array() {
                if !first_delta && !tcs.is_empty() {
                    first_delta = true;
                    let _ = tx
                        .send(Out::Debug {
                            kind: "first_model_delta".into(),
                            message: format!("elapsed_ms={}", stream_started.elapsed().as_millis()),
                        })
                        .await;
                }
                for tc in tcs {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    while tool_calls.len() <= idx {
                        tool_calls.push(json!({"id":"","type":"function","function":{"name":"","arguments":""}}));
                    }
                    let slot = &mut tool_calls[idx];
                    if let Some(id) = tc["id"].as_str() {
                        if !id.is_empty() {
                            slot["id"] = json!(id);
                        }
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        if !name.is_empty() {
                            slot["function"]["name"] = json!(name);
                            if matches!(
                                name,
                                "write_note"
                                    | "append_note"
                                    | "prepend_note"
                                    | "replace_in_note"
                                    | "insert_after_line"
                                    | "delete_in_note"
                                    | "format_note"
                                    | "edit_notebook"
                            ) {
                                suppress_prose = true;
                            }
                        }
                    }
                    if let Some(a) = tc["function"]["arguments"].as_str() {
                        let prev = slot["function"]["arguments"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        slot["function"]["arguments"] = json!(prev + a);
                    }

                    // Live-stream write_note's whole-body content into the editor.
                    let slot_name = slot["function"]["name"].as_str().unwrap_or("").to_string();
                    let slot_args = slot["function"]["arguments"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    if stream_note_preview && slot_name == "write_note" && !note_cancelled {
                        // Keep streaming until an explicit append or a complete
                        // JSON object confirms a targeted edit. Partial `find`
                        // fields are not reliable enough to cancel the preview.
                        let partial_mode = harness::partial_field(&slot_args, "mode");
                        let parsed = serde_json::from_str::<Value>(&slot_args).ok();
                        let is_append = partial_mode.as_deref() == Some("append");
                        let complete_find = parsed
                            .as_ref()
                            .and_then(|v| v.get("find"))
                            .and_then(Value::as_str)
                            .map(|f| !f.trim().is_empty())
                            .unwrap_or(false);
                        let is_replace = !is_append && !complete_find;
                        if !is_replace {
                            if note_streaming {
                                let _ = tx.send(Out::NoteCancel).await;
                                note_streaming = false;
                            }
                            note_cancelled = true;
                        } else if let Some(c) = harness::extract_partial_content(&slot_args) {
                            if !note_streaming {
                                let _ = tx.send(Out::NoteStart).await;
                                note_streaming = true;
                            }
                            if c.len() > note_emitted.len() && c.starts_with(&note_emitted) {
                                let new_part = c[note_emitted.len()..].to_string();
                                let _ = tx.send(Out::NoteDelta(new_part)).await;
                                note_emitted = c;
                            }
                        }
                    }
                }
            }
        }
    }
    finish(content, tool_calls, no_think, note_streaming)
}

pub(crate) fn finish(
    content: String,
    mut tool_calls: Vec<Value>,
    no_think: bool,
    note_streaming: bool,
) -> Result<(String, Vec<Value>, bool), String> {
    tool_calls.retain(|t| !t["function"]["name"].as_str().unwrap_or("").is_empty());
    validate_generated_tool_calls(&tool_calls)?;
    let content = if no_think {
        harness::strip_think(&content)
    } else {
        content
    };
    Ok((content, tool_calls, note_streaming))
}

pub(crate) fn validate_generated_tool_calls(tool_calls: &[Value]) -> Result<(), String> {
    for call in tool_calls {
        if call["function"]["name"].as_str() != Some("write_note") {
            continue;
        }
        let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
        let content = serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|value| value["content"].as_str().map(str::to_owned));
        if content
            .as_deref()
            .is_some_and(harness::note_content_has_protocol_residue)
        {
            return Err(
                "Generation mixed tool protocol text into the note. Live preview reverted; no changes were saved."
                    .to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
