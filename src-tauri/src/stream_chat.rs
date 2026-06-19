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
    FetchWebPageArgs, FetchWebPageTool, ReadNoteArgs, ReadNoteTool, SearchNotesArgs,
    SearchNotesTool, WriteNoteArgs, WriteNoteTool,
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
    preamble: &str,
    prompt: &str,
    request_id: &str,
) -> Result<()> {
    let url = format!("{}/v1/chat/completions", config.base_url());
    let model = config.model_name();
    let temperature = config.temperature;
    let max_turns = (config.max_turns.max(1)) as usize;
    let tools = crate::agent::tool_specs();
    let note_id = state.current_note_id();

    let mut messages: Vec<Value> = vec![
        json!({ "role": "system", "content": preamble }),
        json!({ "role": "user", "content": prompt }),
    ];

    let client = reqwest::Client::new();

    // Backstop state: the user asked to modify the note, so if the turns finish
    // without a write landing we harvest the content as plain text and save it
    // (see the harvest step after the loop). Weak models reliably *generate*
    // prose but fail to emit a large string argument as a parseable tool call.
    let want_write = state.latest_chat_wants_note_write();
    let mut wrote_note = false;

    for _turn in 0..max_turns {
        let body = json!({
            "model": model,
            "messages": messages,
            "tools": tools,
            "temperature": temperature,
            "stream": true,
        });

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
                        let _ = state.handle.emit(
                            "ai://chat_chunk",
                            json!({ "requestId": request_id, "delta": t }),
                        );
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

        // No tool calls → the streamed text is the final answer for this turn.
        // The harvest backstop after the loop handles a stranded note-write.
        let real_calls: Vec<&ToolAccum> =
            tool_calls.iter().filter(|t| !t.name.is_empty()).collect();
        if real_calls.is_empty() {
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
            if t.name == "write_note" && result.starts_with("Note successfully updated") {
                wrote_note = true;
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
    }

    // Harvest backstop: the user asked to write/edit the note but no write landed
    // (the model answered in chat, or emitted a large tool call llama.cpp couldn't
    // parse). Generate the note body as plain text — which weak models do reliably
    // — and save it deterministically.
    if want_write && !wrote_note && !state.latest_chat_wants_clear() {
        log::info!("[stream_chat] write intent but no write landed; harvesting plain-text content");
        if let Some(content) = harvest_note_content(&client, &url, &model, temperature, prompt).await
        {
            if !content.trim().is_empty() {
                let args = json!({ "content": content, "mode": "replace" }).to_string();
                let result = execute_tool(state, "write_note", &args).await;
                log::info!("[stream_chat] harvest write -> {}", {
                    let p: String = result.chars().take(80).collect();
                    p
                });
                if result.starts_with("Note successfully updated") {
                    let _ = state.handle.emit(
                        "ai://chat_chunk",
                        json!({ "requestId": request_id, "delta": "Done — I've written it to the note." }),
                    );
                }
            }
        }
    }

    Ok(())
}

/// Generate the note body as plain text (no tools) and return it cleaned. Weak
/// models reliably *generate* prose but fail to emit a large string argument as a
/// parseable tool call, so for a stranded note-write we harvest the text here.
async fn harvest_note_content(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    temperature: f32,
    user_prompt: &str,
) -> Option<String> {
    let sys = "You produce note content. Output ONLY the final note body in Markdown — the exact text that should go in the note. Follow the user's formatting request exactly: use Markdown headings (## Heading) and bullet lists (- item) when they ask for headings/bullets, and meet any length they ask for. No preamble, no \"here is\", no commentary or explanation, no surrounding code fences, and do not repeat or describe the request. Just the note content itself.";
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": sys },
            { "role": "user", "content": user_prompt },
        ],
        "temperature": temperature,
        "stream": true,
    });
    let resp = client.post(url).json(&body).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut text = String::new();
    let mut done = false;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.ok()?;
        buf.extend_from_slice(&bytes);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let data = match line.trim().strip_prefix("data:") {
                Some(d) => d.trim().to_string(),
                None => continue,
            };
            if data == "[DONE]" {
                done = true;
                break;
            }
            if let Ok(j) = serde_json::from_str::<Value>(&data) {
                if let Some(t) = j["choices"][0]["delta"]["content"].as_str() {
                    text.push_str(t);
                }
            }
        }
        if done {
            break;
        }
    }
    Some(crate::agent::clean_note_text(&text))
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
