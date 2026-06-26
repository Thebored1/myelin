//! Direct OpenAI-compatible streaming chat loop for the note assistant.
//!
//! Why this exists instead of `rig`'s agent stream: `rig` accumulates a tool
//! call's `arguments` internally and only hands back the *fully assembled*
//! arguments once the call is complete. That makes real token-by-token
//! streaming of a note impossible — the whole note arrives at once and the UI
//! has to fake a typewriter. Here we talk to `llama-server` directly, read the
//! SSE deltas ourselves, and surface the `write_note` `content` field to the UI
//! as it is generated (`ai://note_delta`). We still reuse the existing rig
//! `Tool` implementations to *execute* tools, so all the guard rails, approval
//! gating, and `note_written` save logic stay in one place.

use crate::agent::{
    EditNotebookArgs, EditNotebookTool, FetchWebPageArgs, FetchWebPageTool, FindInNoteArgs,
    FindInNoteTool, FormatNoteArgs, FormatNoteTool, ReadNoteArgs, ReadNoteTool, SearchDocumentsArgs,
    SearchDocumentsTool, SearchNotesArgs, SearchNotesTool, WebSearchArgs, WebSearchTool,
    WriteNoteArgs, WriteNoteTool,
};
use crate::llama_server::ResolvedLlamaConfig;
use crate::state::AppState;
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use rig_core::tool::Tool;
use serde_json::{json, Value};
use tauri::Emitter;

#[derive(Default)]
struct ToolAccum {
    id: String,
    name: String,
    args: String,
}

/// Run a multi-turn streaming chat. Emits `ai://chat_chunk` for assistant text,
/// `ai://note_delta` (+ start/cancel) for live note content, and relies on the
/// reused tools to emit `ai://chat_tool` / `ai://note_written`.
pub async fn run_chat(
    state: &AppState,
    config: &ResolvedLlamaConfig,
    mut messages: Vec<Value>,
    mut tools: Vec<Value>,
    request_id: &str,
) -> Result<Vec<Value>> {
    let url = format!("{}/v1/chat/completions", config.base_url());
    let model = config.model_name();
    let temperature = config.temperature;
    let max_turns = (config.max_turns.max(1)) as usize;
    let note_id = state.current_note_id();

    let client = reqwest::Client::new();
    // The most recent tool executed. If the model ends its final turn with no
    // closing text (some models return an empty turn after a successful
    // tool call), we use this to emit a short confirmation instead of a silent,
    // empty bubble.
    let mut last_tool: Option<String> = None;
    // Once a note-mutating tool (write_note/format_note) runs this exchange, stop
    // echoing the model's prose to chat: models — especially small ones — tend to
    // ALSO paste the note's content (or a play-by-play of it) into the reply,
    // duplicating what's already in the editor. Model-agnostic: the note + tool
    // chip are the record, and a concise confirmation replaces the prose.
    let mut suppress_prose = false;
    // A model will otherwise fetch a full page for every search result,
    // burying itself in tens of thousands of characters and never reaching the
    // write. Cap fetches, then stop offering the tool.
    let mut fetch_count = 0usize;
    const MAX_WEB_FETCHES: usize = 2;

    for _turn in 0..max_turns {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "stream": true,
        });
        // Per-message tool gating: only offer the tools this message warrants
        // (computed by agent::select_tools). With none, the model just replies.
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            body["tool_choice"] = json!("auto");
        }
        // DEBUG (temporary): dump each turn's exact request so the failing flow
        // can be replayed verbatim as a test fixture.
        let _ = std::fs::write(
            std::env::temp_dir().join(format!("myelin-req-turn{_turn}.json")),
            serde_json::to_vec_pretty(&body).unwrap_or_default(),
        );

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("chat request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("llama-server returned {status}: {text}"));
        }

        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();

        let mut assistant_text = String::new();
        let mut tool_calls: Vec<ToolAccum> = Vec::new();
        // Live note-streaming state (replace mode only).
        let mut note_streaming = false;
        let mut note_emitted = String::new();
        let mut note_cancelled = false;
        let mut done = false;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| anyhow!("chat stream error: {e}"))?;
            buf.extend_from_slice(&bytes);

            // Process every complete SSE line (newline-delimited) we have so far.
            while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buf.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let line = line.trim();
                let data = match line.strip_prefix("data:") {
                    Some(d) => d.trim(),
                    None => continue,
                };
                if data == "[DONE]" {
                    done = true;
                    break;
                }
                let chunk_json: Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let choice = match chunk_json["choices"].get(0) {
                    Some(c) => c,
                    None => continue,
                };
                let delta = &choice["delta"];

                if let Some(t) = delta["content"].as_str() {
                    if !t.is_empty() {
                        assistant_text.push_str(t);
                        if !suppress_prose {
                            let _ = state.handle.emit(
                                "ai://chat_chunk",
                                json!({ "requestId": request_id, "delta": t }),
                            );
                        }
                    }
                }

                if let Some(tcs) = delta["tool_calls"].as_array() {
                    for tc in tcs {
                        let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                        while tool_calls.len() <= idx {
                            tool_calls.push(ToolAccum::default());
                        }
                        let slot = &mut tool_calls[idx];
                        if let Some(id) = tc["id"].as_str() {
                            if !id.is_empty() {
                                slot.id = id.to_string();
                            }
                        }
                        if let Some(name) = tc["function"]["name"].as_str() {
                            if !name.is_empty() {
                                slot.name = name.to_string();
                                // A note edit is incoming — stop echoing the model's
                                // prose for the rest of this exchange (it duplicates
                                // the note). See `suppress_prose`.
                                if name == "write_note"
                                    || name == "format_note"
                                    || name == "edit_notebook"
                                {
                                    suppress_prose = true;
                                }
                            }
                        }
                        if let Some(args) = tc["function"]["arguments"].as_str() {
                            slot.args.push_str(args);
                        }

                        // Live-stream write_note's content into the editor — but
                        // only for a whole-body replace (the dominant case). For
                        // append/edit the live preview would be misleading, so we
                        // cancel and let the final note_written reconcile.
                        if slot.name == "write_note" && !note_cancelled {
                            // Mirror the planner's intent logic: a whole-body
                            // write (what we live-stream) is anything that is NOT
                            // an append and NOT a targeted `find` snippet. An
                            // explicit mode:"replace" is a whole write even if a
                            // stray `find` is present; mode:"edit" with no find is
                            // also a whole write.
                            let mode = partial_field(&slot.args, "mode");
                            let find = partial_field(&slot.args, "find");
                            let m = mode.as_deref().unwrap_or("");
                            let is_append = m == "append";
                            let explicit_replace = m == "replace";
                            let has_find =
                                find.map(|f| !f.trim().is_empty()).unwrap_or(false);
                            let snippet = has_find && !explicit_replace && !is_append;
                            let is_replace = !is_append && !snippet;
                            if !is_replace {
                                if note_streaming {
                                    if let Some(nid) = &note_id {
                                        let _ = state.handle.emit(
                                            "ai://note_stream_cancel",
                                            json!({ "noteId": nid }),
                                        );
                                    }
                                    note_streaming = false;
                                }
                                note_cancelled = true;
                            } else if let (Some(nid), Some(content)) =
                                (&note_id, extract_partial_content(&slot.args))
                            {
                                if !note_streaming {
                                    let _ = state.handle.emit(
                                        "ai://note_stream_start",
                                        json!({ "noteId": nid }),
                                    );
                                    note_streaming = true;
                                }
                                if content.len() > note_emitted.len()
                                    && content.starts_with(&note_emitted)
                                {
                                    let new_part = content[note_emitted.len()..].to_string();
                                    let _ = state.handle.emit(
                                        "ai://note_delta",
                                        json!({ "noteId": nid, "delta": new_part }),
                                    );
                                    note_emitted = content;
                                }
                            }
                        }
                    }
                }
            }
            if done {
                break;
            }
        }

        // Diagnostics: surface exactly what the model emitted this turn so we can
        // tell a missing tool call (steering) from a refused one (tool result),
        // and catch a tool call that leaked through as plain text.
        let tc_summary: Vec<String> = tool_calls
            .iter()
            .map(|t| format!("{}({}b)", t.name, t.args.len()))
            .collect();
        log::info!(
            "[stream_chat] turn {_turn}: text={}c tool_calls=[{}]",
            assistant_text.chars().count(),
            tc_summary.join(", ")
        );
        if !assistant_text.trim().is_empty() {
            let preview: String = assistant_text.chars().take(400).collect();
            log::info!("[stream_chat] assistant text preview: {preview}");
        }

        // No tool calls → the streamed text is the final answer.
        let real_calls: Vec<&ToolAccum> =
            tool_calls.iter().filter(|t| !t.name.is_empty()).collect();
        if real_calls.is_empty() {
            // Surface a short confirmation so the turn never ends on a silent,
            // empty bubble. Fires when the model returned no text OR when we
            // suppressed its note-duplicating prose this exchange (suppress_prose) —
            // in both cases the visible bubble would otherwise be empty.
            if assistant_text.trim().is_empty() || suppress_prose {
                let msg = if suppress_prose
                    || matches!(
                        last_tool.as_deref(),
                        Some("write_note") | Some("format_note") | Some("edit_notebook")
                    )
                {
                    "Done — I've updated your note."
                } else if last_tool.is_some() {
                    "Done."
                } else {
                    ""
                };
                if !msg.is_empty() {
                    let _ = state.handle.emit(
                        "ai://chat_chunk",
                        json!({ "requestId": request_id, "delta": msg }),
                    );
                }
            }
            break;
        }

        // Record the assistant's tool-call turn, then execute each tool and feed
        // the results back so the model can continue (or produce its final text).
        let tc_json: Vec<Value> = real_calls
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "type": "function",
                    "function": { "name": t.name, "arguments": t.args }
                })
            })
            .collect();
        messages.push(json!({
            "role": "assistant",
            "content": if assistant_text.is_empty() { Value::Null } else { Value::String(assistant_text.clone()) },
            "tool_calls": tc_json,
        }));

        for t in &real_calls {
            let result = execute_tool(state, &t.name, &t.args).await;
            last_tool = Some(t.name.clone());
            if t.name == "fetch_web_page" {
                fetch_count += 1;
            }
            let args_preview: String = t.args.chars().take(300).collect();
            let result_preview: String = result.chars().take(200).collect();
            log::info!(
                "[stream_chat] exec {} args={args_preview} -> {result_preview}",
                t.name
            );
            messages.push(json!({
                "role": "tool",
                "tool_call_id": t.id,
                "content": result,
            }));
        }

        // Once the fetch budget is spent, stop offering fetch_web_page so the
        // model synthesises from the search snippets + pages it already pulled
        // instead of looping on fetches forever (models drown otherwise).
        if fetch_count >= MAX_WEB_FETCHES {
            tools.retain(|t| t["function"]["name"].as_str() != Some("fetch_web_page"));
        }
    }

    Ok(messages)
}

/// Deserialize the arguments for a named tool and run the matching rig `Tool`,
/// reusing all of its guard rails / save logic. Returns the tool's result text
/// (tools return `Ok(message)` even for refusals).
async fn execute_tool(state: &AppState, name: &str, args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
    match name {
        "write_note" => match serde_json::from_value::<WriteNoteArgs>(v) {
            Ok(a) => WriteNoteTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid write_note arguments: {e}"),
        },
        "read_note" => match serde_json::from_value::<ReadNoteArgs>(v) {
            Ok(a) => ReadNoteTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid read_note arguments: {e}"),
        },
        "search_notes" => match serde_json::from_value::<SearchNotesArgs>(v) {
            Ok(a) => SearchNotesTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid search_notes arguments: {e}"),
        },
        "fetch_web_page" => match serde_json::from_value::<FetchWebPageArgs>(v) {
            Ok(a) => FetchWebPageTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid fetch_web_page arguments: {e}"),
        },
        "web_search" => match serde_json::from_value::<WebSearchArgs>(v) {
            Ok(a) => WebSearchTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid web_search arguments: {e}"),
        },
        "search_documents" => match serde_json::from_value::<SearchDocumentsArgs>(v) {
            Ok(a) => SearchDocumentsTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid search_documents arguments: {e}"),
        },
        "find_in_note" => match serde_json::from_value::<FindInNoteArgs>(v) {
            Ok(a) => FindInNoteTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid find_in_note arguments: {e}"),
        },
        "format_note" => match serde_json::from_value::<FormatNoteArgs>(v) {
            Ok(a) => FormatNoteTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid format_note arguments: {e}"),
        },
        "edit_notebook" => match serde_json::from_value::<EditNotebookArgs>(v) {
            Ok(a) => EditNotebookTool { state: state.clone() }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("Invalid edit_notebook arguments: {e}"),
        },
        other => format!("Unknown tool: {other}"),
    }
}

/// Extract a complete simple string value for `key` from a partial JSON object
/// (e.g. `mode`). Returns `None` if the key isn't present yet or its value isn't
/// closed yet. Good enough for short, escape-free fields like `mode`.
fn partial_field(raw: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let kpos = raw.find(&pat)?;
    let after = raw[kpos + pat.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let after = after.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = after.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some(n) => out.push(n),
                None => return None,
            },
            _ => out.push(c),
        }
    }
    None
}

/// Best-effort decode of the `content` string value from partial JSON like
/// `{"content":"hello wo`. Conservatively stops before any incomplete escape so
/// we never emit a half-decoded character; the next fragment completes it.
fn extract_partial_content(raw: &str) -> Option<String> {
    let pat = "\"content\"";
    let kpos = raw.find(pat)?;
    let after = raw[kpos + pat.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let body = after.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break, // value complete
            '\\' => match chars.next() {
                None => break, // incomplete escape; stop here
                Some(e) => match e {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000C}'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'u' => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            match chars.next() {
                                Some(h) => hex.push(h),
                                None => return Some(out), // incomplete \u escape
                            }
                        }
                        if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(cp) {
                                out.push(ch);
                            }
                        }
                    }
                    other => out.push(other),
                },
            },
            _ => out.push(c),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_complete_content_after_other_keys() {
        // The live probe's exact shape: mode first, then content.
        let raw = r#"{"mode":"edit","content":"hello world"}"#;
        assert_eq!(extract_partial_content(raw).as_deref(), Some("hello world"));
    }

    #[test]
    fn extracts_partial_content_mid_string() {
        let raw = r#"{"content":"hello wo"#;
        assert_eq!(extract_partial_content(raw).as_deref(), Some("hello wo"));
    }

    #[test]
    fn decodes_escapes() {
        let raw = r#"{"content":"line1\nline2\t!"}"#;
        assert_eq!(extract_partial_content(raw).as_deref(), Some("line1\nline2\t!"));
    }

    #[test]
    fn stops_before_incomplete_escape() {
        // A lone trailing backslash is an unfinished escape — don't emit it.
        let raw = "{\"content\":\"ab\\";
        assert_eq!(extract_partial_content(raw).as_deref(), Some("ab"));
    }

    #[test]
    fn no_content_key_yet() {
        assert_eq!(extract_partial_content(r#"{"mode":"replace""#), None);
    }

    #[test]
    fn partial_field_reads_closed_value_only() {
        assert_eq!(partial_field(r#"{"mode":"append"}"#, "mode").as_deref(), Some("append"));
        assert_eq!(partial_field(r#"{"mode":"app"#, "mode"), None); // not closed yet
        assert_eq!(partial_field(r#"{"content":"x"}"#, "mode"), None); // absent
    }
}
