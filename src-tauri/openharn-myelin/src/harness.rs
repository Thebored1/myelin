//! openharn's reliability primitives, ported verbatim in spirit from
//! `openharn/src/agent.rs` (MIT). These are the whole point of using openharn:
//! they make a *small* local model drive tools reliably. Pure functions only —
//! no I/O — so they are reused unchanged inside the async sidecar loop.
//!
//!   - `cap_result_with`        cap a single tool result so it can't blow context
//!   - `fit_context`            trim history to a char budget, keep system + whole turns
//!   - `parse_text_tool_calls`  recover a tool call the server left as plain text
//!   - `flatten_for_prompt_tools` render history for a server with no tool API
//!   - `tool_grammar`           GBNF that forces a schema-valid call (or text)
//!   - `strip_think`            drop leaked <think>…</think> in reasoning-off mode
//!   - `extract_partial_content` / `partial_field` — stream write_note's body live

use serde_json::{json, Value};

mod context;
mod decompose;
pub use context::*;
pub use decompose::harness_decompose;

/// The first required parameter of a tool (used to map a positional text call
/// like `web_search(rust release)` onto `{"query": "..."}`). Schema-driven so it
/// works for ANY tool set (Myelin's, not just openharn's coding tools).
fn first_required_param(schemas: &Value, name: &str) -> Option<String> {
    let arr = schemas.as_array()?;
    for t in arr {
        if t["function"]["name"].as_str() == Some(name) {
            if let Some(req) = t["function"]["parameters"]["required"].as_array() {
                if let Some(first) = req.first().and_then(|v| v.as_str()) {
                    return Some(first.to_string());
                }
            }
            // no required list: fall back to the first declared property
            if let Some(props) = t["function"]["parameters"]["properties"].as_object() {
                if let Some(k) = props.keys().next() {
                    return Some(k.clone());
                }
            }
        }
    }
    None
}

/// Recover a structured tool call the server left as plain text. Handles the
/// Granite family shape — an optional `<tool_call>` / `<|tool_call|>` marker
/// followed by a JSON list `[{"name":…,"arguments":{…}}]` — and a bare
/// `name({...})` fallback. Returns OpenAI-format tool_calls (arguments as a JSON
/// string). Returns None when `content` isn't a recognizable call, so a normal
/// answer is never misread as one.
pub fn parse_text_tool_calls(content: &str, schemas: &Value) -> Option<Vec<Value>> {
    let mut s = content.trim();
    for marker in ["<|tool_call|>", "<tool_call>", "```json", "```"] {
        if let Some(rest) = s.strip_prefix(marker) {
            s = rest;
            break;
        }
    }
    let s = s.trim();
    let mut calls = Vec::new();

    // JSON tool-call format (Granite / llama-server style).
    if let Some(open) = s.find(['[', '{']) {
        let is_arr = s.as_bytes()[open] == b'[';
        let close = if is_arr { s.rfind(']') } else { s.rfind('}') };
        if let Some(close) = close {
            if close > open {
                if let Ok(val) = serde_json::from_str::<Value>(&s[open..=close]) {
                    let items: Vec<Value> = match val {
                        Value::Array(a) => a,
                        obj => vec![obj],
                    };
                    for (i, item) in items.iter().enumerate() {
                        let f = item.get("function").unwrap_or(item);
                        if let Some(name) = f.get("name").and_then(|v| v.as_str()) {
                            let args = f.get("arguments").or_else(|| f.get("parameters"));
                            let args_str = match args {
                                Some(a) if a.is_string() => a.as_str().unwrap_or("{}").to_string(),
                                Some(a) if !a.is_null() => a.to_string(),
                                _ => "{}".to_string(),
                            };
                            calls.push(json!({
                                "id": format!("call_{i}"),
                                "type": "function",
                                "function": { "name": name, "arguments": args_str }
                            }));
                        }
                    }
                    if !calls.is_empty() {
                        return Some(calls);
                    }
                }
            }
        }
        // Incomplete array: no closing bracket found — repair by appending `]`
        if is_arr && close.is_none() {
            let tail = &s[open..];
            let repaired = format!("{tail}]");
            if let Ok(val) = serde_json::from_str::<Value>(&repaired) {
                if let Value::Array(a) = val {
                    for (i, item) in a.iter().enumerate() {
                        let f = item.get("function").unwrap_or(item);
                        if let Some(name) = f.get("name").and_then(|v| v.as_str()) {
                            let args_str = f
                                .get("arguments")
                                .or_else(|| f.get("parameters"))
                                .map(|a| {
                                    if a.is_string() {
                                        a.as_str().unwrap_or("{}").to_string()
                                    } else if !a.is_null() {
                                        a.to_string()
                                    } else {
                                        "{}".to_string()
                                    }
                                })
                                .unwrap_or_else(|| "{}".to_string());
                            calls.push(json!({
                                "id": format!("call_{i}"),
                                "type": "function",
                                "function": { "name": name, "arguments": args_str }
                            }));
                        }
                    }
                    if !calls.is_empty() {
                        return Some(calls);
                    }
                }
            }
        }
    }

    // Liquid LFM native text fallback. Parse the quoted content with the same
    // escape-aware decoder used by live preview rather than the generic regex,
    // which cannot safely handle parentheses inside generated Markdown.
    if s.contains("<|tool_call_start|>")
        && schemas.as_array().is_some_and(|items| {
            items
                .iter()
                .any(|tool| tool["function"]["name"].as_str() == Some("write_note"))
        })
    {
        if let Some((generated, true, quote_end)) = extract_lfm_content_value_with_end(s) {
            let tail = &s[quote_end..];
            let call_closed = tail.contains(')');
            let native_frame_closed =
                !s.contains("<|tool_call_start|>") || tail.contains("<|tool_call_end|>");
            if call_closed && native_frame_closed && !note_content_has_protocol_residue(&generated)
            {
                return Some(vec![json!({
                    "id": "call_lfm_text_0",
                    "type": "function",
                    "function": {
                        "name": "write_note",
                        "arguments": json!({ "content": generated }).to_string()
                    }
                })]);
            }
        }
    }

    // Some LFM2 Q2 generations emit the tool wrapper and a valid Markdown
    // body, but leave the JSON string closing quote out and place the template
    // marker </content> before the wrapper closes. Recover only this narrow,
    // schema-checked shape; ordinary malformed JSON must still be rejected.
    if s.contains("</content>")
        || s.contains("}</tool_call>")
            && s.contains("write_note")
            && schemas.as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|tool| tool["function"]["name"].as_str() == Some("write_note"))
            })
    {
        if let Some(content_key) = s.find("\"content\"") {
            let after_key = &s[content_key + "\"content\"".len()..];
            let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
            let quote = after_colon.chars().next()?;
            if quote == '"' || quote == '\'' {
                let body = &after_colon[quote.len_utf8()..];
                let marker = body
                    .find("</content>")
                    .or_else(|| body.find("}</tool_call>"));
                if let Some(marker) = marker {
                    let body = body[..marker].trim_end_matches('」');
                    let (generated, _, _) = decode_partial_quoted(body, quote);
                    if !generated.is_empty() && !note_content_has_protocol_residue(&generated) {
                        return Some(vec![json!({
                            "id": "call_lfm_recovered_0",
                            "type": "function",
                            "function": {
                                "name": "write_note",
                                "arguments": json!({ "content": generated }).to_string()
                            }
                        })]);
                    }
                }
            }
        }
    }

    // Fallback: `name({"k":"v"})` or `name(positional)`.
    fn tool_param(name: &str, schemas: &Value) -> Option<&'static str> {
        // Myelin-specific mapping for positional args
        let known: &[(&str, &str)] = &[
            ("write_note", "content"),
            ("search_notes", "query"),
            ("read_note", "note_id"),
            ("fetch_web_page", "url"),
            ("web_search", "query"),
            ("format_note", "operation"),
            ("find_in_note", "query"),
            ("search_documents", "query"),
        ];
        for (n, p) in known {
            if *n == name {
                return Some(p);
            }
        }
        // Fall back to first required param from schema
        first_required_param(schemas, name).map(|s| Box::leak(s.into_boxed_str()) as &str)
    }
    let known = |n: &str| -> bool {
        schemas.as_array().map_or(false, |a| {
            a.iter().any(|t| t["function"]["name"].as_str() == Some(n))
        })
    };
    let pattern = regex::Regex::new(r"(?m)(?:^|\s)`?(\w+)\((\{.*?\}|[^)]*)\)`?").ok()?;
    for cap in pattern.captures_iter(s) {
        let name = cap.get(1).map(|m| m.as_str())?;
        if !known(name) {
            continue;
        }
        let args_str = cap.get(2).map(|m| m.as_str()).unwrap_or("{}").trim();
        if !args_str.starts_with('{') && !args_str.contains('=') && args_str.contains('"') {
            continue;
        }
        let args = if args_str.starts_with('{') {
            args_str.to_string()
        } else if args_str.contains('=') {
            // Liquid LFM's native format is Pythonic:
            //   write_note(content="...", mode="replace")
            // Split only on top-level commas so commas inside JSON-quoted
            // strings and nested values remain part of the argument.
            let mut parts = Vec::new();
            let mut start = 0usize;
            let mut depth = 0i32;
            let mut quote = false;
            let mut escaped = false;
            for (i, ch) in args_str.char_indices() {
                if quote {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        quote = false;
                    }
                    continue;
                }
                match ch {
                    '"' => quote = true,
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        parts.push(&args_str[start..i]);
                        start = i + ch.len_utf8();
                    }
                    _ => {}
                }
            }
            parts.push(&args_str[start..]);

            let mut obj = serde_json::Map::new();
            for part in parts {
                let Some((key, raw)) = part.split_once('=') else {
                    continue;
                };
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                let raw = raw.trim();
                let value = serde_json::from_str::<Value>(raw).unwrap_or_else(|_| match raw {
                    "True" => Value::Bool(true),
                    "False" => Value::Bool(false),
                    "None" => Value::Null,
                    _ => Value::String(raw.to_string()),
                });
                obj.insert(key.to_string(), value);
            }
            // Schema-sanitize: keep only argument keys the tool declares. A
            // malformed Pythonic call (e.g. write_note(foo="…")) previously
            // became a structured call missing `content` — with an unknown-key
            // model error it now fails loudly at argument validation instead.
            let tool_schema = schemas.as_array().and_then(|arr| {
                arr.iter()
                    .find(|t| t["function"]["name"].as_str() == Some(name))
            });
            if let Some(props) =
                tool_schema.and_then(|t| t["function"]["parameters"]["properties"].as_object())
            {
                obj.retain(|k, _| props.contains_key(k));
            }
            let has_required_args = tool_schema
                .and_then(|t| t["function"]["parameters"]["required"].as_array())
                .map(|required| {
                    required.iter().all(|key| {
                        key.as_str()
                            .and_then(|key| obj.get(key))
                            .is_some_and(|value| !value.is_null())
                    })
                })
                .unwrap_or(true);
            if obj.is_empty() || !has_required_args {
                continue;
            }
            Value::Object(obj).to_string()
        } else if let Some(param) = tool_param(name, schemas) {
            json!({ param: args_str }).to_string()
        } else {
            continue;
        };
        calls.push(json!({
            "id": format!("call_{}", calls.len()),
            "type": "function",
            "function": { "name": name, "arguments": args }
        }));
    }
    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

/// Prompt-tools mode: describe the tool set + the exact call format the model
/// should emit (what `parse_text_tool_calls` recovers). For servers with no
/// native tool-calling API.
fn tool_prompt(schemas: &Value) -> String {
    let mut s = String::from(
        "You do NOT have a tool API. To call a tool, reply with ONLY this line and nothing else:\n\
         <tool_call>[{\"name\": \"<tool>\", \"arguments\": { ... }}]\n\
         Follow that JSON format exactly. Do not switch to native `tool_name(arg=...)` syntax. A generated note belongs only inside the JSON `content` string; never place tool names, call wrappers, or closing protocol markers inside `content`.\n\
         Call a tool whenever the user wants to change, create, add to, or remove content from the note — never put that content in a plain-text reply. Reply in plain text ONLY for genuine conversation (greetings, questions, clarifications, refusals); never for note content. Available tools:\n",
    );

    // Build a name→schema lookup from the (already gated) tool array.
    if let Some(arr) = schemas.as_array() {
        let tool_map: std::collections::HashMap<&str, &Value> = arr
            .iter()
            .filter_map(|t| Some((t["function"]["name"].as_str()?, t)))
            .collect();

        // Sections in display order: each has a header and a list of tool names.
        // If none of a section's tools are in the filtered set, the section
        // is skipped entirely.
        let sections: [(&str, &[&str]); 6] = [
            (
                "--- Read & Search ---",
                &[
                    "search_notes",
                    "read_note",
                    "search_documents",
                    "find_in_note",
                ],
            ),
            ("--- Web Fetch ---", &["fetch_web_page", "web_search"]),
            (
                "--- Edit: Add Content ---",
                &[
                    "write_note",
                    "append_note",
                    "prepend_note",
                    "insert_after_line",
                ],
            ),
            (
                "--- Edit: Change or Remove ---",
                &["replace_in_note", "delete_in_note"],
            ),
            ("--- Formatting ---", &["format_note"]),
            ("--- Notebooks ---", &["edit_notebook"]),
        ];

        for (header, tool_names) in sections {
            let mut lines: Vec<String> = Vec::new();
            for name in tool_names {
                if let Some(t) = tool_map.get(name) {
                    let f = &t["function"];
                    let desc = f["description"].as_str().unwrap_or("");
                    let required: Vec<&str> = f["parameters"]["required"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();
                    let params = f["parameters"]["properties"]
                        .as_object()
                        .map(|o| {
                            o.keys()
                                .map(|k| {
                                    if required.contains(&k.as_str()) {
                                        k.clone()
                                    } else {
                                        format!("[{k}]")
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    let short = desc.split(['.', '\n']).next().unwrap_or(desc);
                    lines.push(format!("- {name}({params}): {short}"));
                }
            }
            if !lines.is_empty() {
                s.push_str(&format!("\n  {header}\n"));
                for line in &lines {
                    s.push_str(&format!("    {line}\n"));
                }
            }
        }
    }
    s
}

/// Render internal `tool_calls` back into the text form the model is told to
/// emit, so a prior tool-calling assistant turn round-trips as plain text.
fn render_calls_text(tool_calls: &Value) -> String {
    let items: Vec<Value> = tool_calls
        .as_array()
        .map(|a| {
            a.iter()
                .map(|tc| {
                    let name = tc["function"]["name"].as_str().unwrap_or("");
                    let args_raw = tc["function"]["arguments"].as_str().unwrap_or("{}");
                    let args: Value = serde_json::from_str(args_raw).unwrap_or_else(|_| json!({}));
                    json!({ "name": name, "arguments": args })
                })
                .collect()
        })
        .unwrap_or_default();
    format!("<tool_call>{}", Value::Array(items))
}

/// Flatten history into plain system/user/assistant messages a tool-unaware
/// server accepts: tools described in the system prompt, assistant tool_calls
/// become their text form, and tool results become user messages.
pub fn flatten_for_prompt_tools(history: &[Value], schemas: &Value) -> Vec<Value> {
    history
        .iter()
        .map(|m| match m["role"].as_str().unwrap_or("") {
            "system" => {
                let base = m["content"].as_str().unwrap_or("");
                json!({ "role": "system", "content": format!("{base}\n\n{}", tool_prompt(schemas)) })
            }
            "assistant" if m.get("tool_calls").is_some() => {
                let text = render_calls_text(&m["tool_calls"]);
                let content = m["content"].as_str().filter(|s| !s.is_empty());
                let full = match content {
                    Some(c) => format!("{c}\n{text}"),
                    None => text,
                };
                json!({ "role": "assistant", "content": full })
            }
            "tool" => {
                let c = m["content"].as_str().unwrap_or("");
                json!({ "role": "user", "content": format!("Tool result:\n{c}") })
            }
            _ => m.clone(),
        })
        .collect()
}

// ---- GBNF grammar (strict mode) -------------------------------------------

const GRAMMAR_TAIL: &str = r#"string ::= "\"" ( [^"\\\n\r\t] | "\\" . )* "\""
integer ::= "-"? [0-9]+
"#;

/// The GBNF grammar for a single argument value, tightened by JSON-schema type/enum where
/// we can (string / integer / boolean / one-of-enum), falling back to generic `value`.
fn value_rule_for(spec: &Value) -> String {
    // Enum constraint: restrict to enums when available (most"/"relevant"/" for format_op).
    if let Some(en) = spec["enum"].as_array() {
        let q = |s: &str| format!("\"\\\"{s}\\\"\"");
        let alts: Vec<String> = en.iter().filter_map(|v| v.as_str()).map(q).collect();
        if !alts.is_empty() {
            return format!("( {} )", alts.join(" | "));
        }
    }
    // For the grammar to work with highly-quantized models, keep argument rules
    // simple: string covers most tool arguments; integer covers count/index.
    match spec["type"].as_str().unwrap_or("") {
        "integer" | "number" => "integer".into(),
        _ => "string".into(),
    }
}

pub fn tool_grammar(schemas: &Value, root: &str) -> String {
    let lit = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut g = String::new();
    match root {
        "call" => g.push_str("root ::= call\n"),
        "abstain" => {
            g.push_str("root ::= call | abstain\n");
            g.push_str("abstain ::= \"NO_TOOL\"\n");
        }
        _ => {
            g.push_str("root ::= call | text\n");
            g.push_str("text ::= [^<[{] | [^<[{] textrest\n");
            g.push_str("textrest ::= [^<] | [^<] textrest\n");
        }
    }
    // Compact flat grammar: no whitespace rules (ws), no recursive value/object
    // rules. Each tool call is inlined as a flat sequence of literal characters
    // and simple rules (string, integer, enum). This avoids the model getting
    // stuck in infinite whitespace expansion, which Q2 models do with complex
    // recursive grammars.
    //
    // Allow multiple tool calls. Q2 models sometimes split content across
    // calls (first call has partial content, second has the rest). The sidecar
    // executes all calls via max_calls; write_note overwrites so only the last
    // non-empty call matters.
    g.push_str(&format!(
        "call ::= {} {} obj ( {} obj )* {}\n",
        opt_lit("<tool_call>"),
        lit("["),
        lit(","),
        lit("]")
    ));
    let mut obj_alts: Vec<String> = Vec::new();
    let mut tool_rules = String::new();
    if let Some(arr) = schemas.as_array() {
        for t in arr {
            let name = match t["function"]["name"].as_str() {
                Some(n) => n,
                None => continue,
            };
            // Build the inner arguments object: {"key1": rule1, "key2": rule2, ...}
            let props = t["function"]["parameters"]["properties"].as_object();
            let mut arg_pairs: Vec<String> = Vec::new();
            if let Some(props) = props {
                for (k, spec) in props {
                    let key_lit = lit(&format!("\"{k}\""));
                    let val_rule = value_rule_for(spec);
                    arg_pairs.push(format!("{key_lit} {} {val_rule}", lit(":")));
                }
            }
            let args_obj = if arg_pairs.is_empty() {
                format!("{}{}", lit("{"), lit("}"))
            } else {
                let sep = lit(",");
                format!("{}{}{}", lit("{"), arg_pairs.join(&sep), lit("}"))
            };
            // Full tool object: {"name": "tool_name", "arguments": {args}}
            let mut tool_rule = String::new();
            tool_rule.push_str(&lit("{"));
            tool_rule.push_str(&lit("\"name\""));
            tool_rule.push_str(&lit(":"));
            tool_rule.push_str(&lit(&format!("\"{name}\"")));
            tool_rule.push_str(&lit(","));
            tool_rule.push_str(&lit("\"arguments\""));
            tool_rule.push_str(&lit(":"));
            tool_rule.push_str(&args_obj);
            tool_rule.push_str(&lit("}"));
            tool_rules.push_str(&format!(
                "t-{} ::= {}\n",
                name.replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
                tool_rule
            ));
            obj_alts.push(format!(
                "t-{}",
                name.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
            ));
        }
    }
    let obj = if obj_alts.is_empty() {
        // No tools defined: fall back to a generic JSON value (can only match
        // the text branch anyway since the grammar allows it).
        "string".to_string()
    } else {
        obj_alts.join(" | ")
    };
    g.push_str(&format!("obj ::= {obj}\n"));
    g.push_str(&tool_rules);
    g.push_str(GRAMMAR_TAIL);
    g
}

/// Like `lit` but wraps in optional sequence (the caller is responsible for
/// spacing). Emits `( lit )?` — zero-or-one of the literal.
fn opt_lit(s: &str) -> String {
    format!("( \"{}\" )?", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// ---- live note streaming (ported from Myelin stream_chat.rs) ---------------

/// Extract a complete simple string value for `key` from a partial JSON object
/// (e.g. `mode`). Returns None if the key isn't present yet or isn't closed.
pub fn partial_field(raw: &str, key: &str) -> Option<String> {
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

/// Decode the `content` string value and report whether its closing quote has
/// arrived. This preserves Markdown escapes such as `\\n` as real newlines.
fn decode_partial_quoted(body: &str, quote: char) -> (String, bool, usize) {
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((index, c)) = chars.next() {
        if c == quote {
            return (out, true, index + c.len_utf8());
        }
        match c {
            '\\' => match chars.next() {
                None => return (out, false, body.len()),
                Some((_, e)) => match e {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000C}'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'u' => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            match chars.next() {
                                Some((_, h)) => hex.push(h),
                                None => return (out, false, body.len()),
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
    (out, false, body.len())
}

fn extract_json_content_value(raw: &str) -> Option<(String, bool)> {
    let pat = "\"content\"";
    let kpos = raw.find(pat)?;
    let after = raw[kpos + pat.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let body = after.strip_prefix('"')?;
    let (content, closed, _) = decode_partial_quoted(body, '"');
    Some((content, closed))
}

/// Decode Liquid LFM's native Pythonic argument syntax:
/// `write_note(content="generated text so far`.
fn extract_lfm_content_value_with_end(raw: &str) -> Option<(String, bool, usize)> {
    let call_at = raw.find("write_note(")?;
    let call_start = call_at + "write_note(".len();
    let call = &raw[call_start..];
    let content_at = call.find("content")?;
    let key_end = call_start + content_at + "content".len();
    let untrimmed = &raw[key_end..];
    let after = untrimmed.trim_start();
    let equals_offset = untrimmed.len() - after.len();
    let after = after.strip_prefix('=')?;
    let after_untrimmed = after;
    let after = after.trim_start();
    let value_ws = after_untrimmed.len() - after.len();
    let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let body_start = key_end + equals_offset + 1 + value_ws + quote.len_utf8();
    let body = &raw[body_start..];
    let (content, closed, consumed) = decode_partial_quoted(body, quote);
    Some((content, closed, body_start + consumed))
}

fn extract_lfm_content_value(raw: &str) -> Option<(String, bool)> {
    extract_lfm_content_value_with_end(raw).map(|(content, closed, _)| (content, closed))
}

/// Best-effort decode of the `content` string value from partial JSON like
/// `{"content":"hello wo`. Conservatively stops before any incomplete escape so
/// we never emit a half-decoded character; the next fragment completes it.
pub fn extract_partial_content(raw: &str) -> Option<String> {
    extract_json_content_value(raw)
        .or_else(|| extract_lfm_content_value(raw))
        .map(|(content, _)| content)
}

/// Protocol/template residue is never valid generated note content. Reject the
/// call instead of trimming and saving an approximation: the speculative
/// editor preview can then be restored from its authoritative snapshot.
/// Shared with Myelin's host-side guard so both hosts agree on the markers.
pub fn note_content_has_protocol_residue(content: &str) -> bool {
    myelin_edit_core::note_content_has_protocol_residue(content)
}

#[cfg(test)]
mod tests;
