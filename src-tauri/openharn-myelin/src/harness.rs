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

/// A single tool result can be huge; cap it so one result can't blow the context.
pub const TOOL_RESULT_CAP: usize = 4_000;

pub fn cap_result_with(mut s: String, cap: usize) -> String {
    if s.chars().count() > cap {
        s = s.chars().take(cap).collect();
        s.push_str(
            "\n…[result truncated — narrow your search (a more specific query) if you need more]",
        );
    }
    s
}

/// Trim the conversation to fit `max_chars`, always keeping the system message and
/// dropping OLDEST whole turns first (a user message plus the assistant/tool
/// messages that follow it), so a tool result is never orphaned from its call.
pub fn fit_context(history: &mut Vec<Value>, max_chars: usize) {
    let total = |h: &[Value]| -> usize { h.iter().map(|m| m.to_string().len()).sum() };
    while total(history) > max_chars && history.len() > 3 {
        let mut end = 2;
        while end < history.len() && history[end]["role"] != "user" {
            end += 1;
        }
        history.drain(1..end);
    }
}

/// In reasoning-off mode a hybrid-thinking model still leaks a (shortened) chain
/// of thought into the content wrapped in stray `<think>…</think>` tags. Keep
/// only the real answer: everything after the last `</think>`, tags removed.
pub fn strip_think(s: &str) -> String {
    let tail = match s.rfind("</think>") {
        Some(i) => &s[i + "</think>".len()..],
        None => s,
    };
    tail.replace("<think>", "")
        .replace("</think>", "")
        .trim()
        .to_string()
}

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

    // JSON tool-call format first (Granite / llama-server style).
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
    }

    // Fallback: `name({"k":"v"})` or `name(positional)`. Only match at line start
    // or after whitespace/backtick so it never fires on code like `fs::write(...)`.
    let pattern = regex::Regex::new(r"(?m)(?:^|\s)`?(\w+)\((\{.*?\}|[^)]*)\)`?").ok()?;
    let known = |n: &str| {
        schemas
            .as_array()
            .map(|a| a.iter().any(|t| t["function"]["name"].as_str() == Some(n)))
            .unwrap_or(false)
    };
    for cap in pattern.captures_iter(s) {
        let name = cap.get(1).map(|m| m.as_str())?;
        if !known(name) {
            continue;
        }
        let args_str = cap.get(2).map(|m| m.as_str()).unwrap_or("{}").trim();
        if !args_str.starts_with('{') && args_str.contains('"') {
            continue;
        }
        let args = if args_str.starts_with('{') {
            args_str.to_string()
        } else if let Some(param) = first_required_param(schemas, name) {
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
         Call a tool whenever the user wants to change, create, add to, or remove content from the note — never put that content in a plain-text reply. Reply in plain text ONLY for genuine conversation (greetings, questions, clarifications, refusals); never for note content. Available tools:\n",
    );
    if let Some(arr) = schemas.as_array() {
        for t in arr {
            let f = &t["function"];
            let name = f["name"].as_str().unwrap_or("");
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
            s.push_str(&format!("- {name}({params}): {short}\n"));
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

const GRAMMAR_TAIL: &str = r#"value ::= object | array | string | number | "true" | "false" | "null"
object ::= "{" ws ( string ws ":" ws value ( ws "," ws string ws ":" ws value )* )? ws "}"
array ::= "[" ws ( value ( ws "," ws value )* )? ws "]"
string ::= "\"" ( [^"\\\n\r\t] | "\\" ["\\/bfnrt] )* "\""
number ::= "-"? [0-9]+ ( "." [0-9]+ )?
integer ::= "-"? [0-9]+
boolean ::= "true" | "false"
ws ::= [ \t]?
"#;

fn value_rule_for(spec: &Value) -> String {
    let q = |s: &str| format!("\"\\\"{s}\\\"\"");
    if let Some(en) = spec["enum"].as_array() {
        let alts: Vec<String> = en.iter().filter_map(|v| v.as_str()).map(q).collect();
        if !alts.is_empty() {
            return format!("( {} )", alts.join(" | "));
        }
    }
    match spec["type"].as_str().unwrap_or("") {
        "string" => "string".into(),
        "integer" | "number" => "integer".into(),
        "boolean" => "boolean".into(),
        _ => "value".into(),
    }
}

/// Generate a GBNF grammar constraining the reply to EITHER a schema-valid tool
/// call `<tool_call>[{"name": <known tool>, "arguments": {<known keys, typed>}}]`
/// OR plain text. A weak model then physically cannot invent a field name,
/// misname a tool, or malform a call.
///
/// `root` controls what the model is allowed to emit:
///   "call | text" — tool call or plain prose (default, safest)
///   "call" — tool call ONLY (used when host has determined tools are needed)
pub fn tool_grammar(schemas: &Value, root: &str) -> String {
    let lit = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let rn = |s: &str| s.replace('_', "-");
    let mut g = String::new();
    g.push_str(&format!("root ::= {root}\n"));
    g.push_str("text ::= [^<] | [^<] text\n");
    g.push_str(&format!(
        "call ::= {} ws {} ws obj ( {} ws obj )* {}\n",
        lit("<tool_call>"),
        lit("["),
        lit(","),
        lit("]")
    ));
    let mut obj_alts: Vec<String> = Vec::new();
    let mut rules = String::new();
    if let Some(arr) = schemas.as_array() {
        for t in arr {
            let name = match t["function"]["name"].as_str() {
                Some(n) => n,
                None => continue,
            };
            let rname = rn(name);
            obj_alts.push(format!("t-{rname}"));
            // Build the arguments object rule from this tool's properties.
            let props = t["function"]["parameters"]["properties"].as_object();
            let required: Vec<String> = t["function"]["parameters"]["required"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let mut key_rules: Vec<String> = Vec::new();
            if let Some(props) = props {
                for (k, spec) in props {
                    let vr = value_rule_for(spec);
                    let optional = !required.contains(k);
                    let kv = format!(
                        "{qkey} ws {colon} ws {vr}",
                        qkey = lit(&format!("\"{k}\"")),
                        colon = lit(":"),
                    );
                    if optional {
                        key_rules.push(format!("( {kv} )?"));
                    } else {
                        key_rules.push(format!("( {kv} )"));
                    }
                }
            }
            let comma = lit(",");
            let inner = key_rules.join(&format!(" ws {comma} ws "));
            let open = lit("{");
            let close = lit("}");
            rules.push_str(&format!("a-{rname} ::= {open} ws {inner} ws {close}\n"));
            let name_lit = lit(&format!("\"{name}\""));
            rules.push_str(&format!(
                "t-{rname} ::= {open} ws {qname} ws {colon} ws {name_lit} ws {comma} ws {qargs} ws {colon} ws a-{rname} ws {close}\n",
                open = lit("{"),
                qname = lit("\"name\""),
                colon = lit(":"),
                comma = lit(","),
                qargs = lit("\"arguments\""),
                close = lit("}"),
            ));
        }
    }
    let obj = if obj_alts.is_empty() {
        "value".to_string()
    } else {
        obj_alts.join(" | ")
    };
    g.push_str(&format!("obj ::= {obj}\n"));
    g.push_str(&rules);
    g.push_str(GRAMMAR_TAIL);
    g
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

/// Best-effort decode of the `content` string value from partial JSON like
/// `{"content":"hello wo`. Conservatively stops before any incomplete escape so
/// we never emit a half-decoded character; the next fragment completes it.
pub fn extract_partial_content(raw: &str) -> Option<String> {
    let pat = "\"content\"";
    let kpos = raw.find(pat)?;
    let after = raw[kpos + pat.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let body = after.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                None => break,
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
                                None => return Some(out),
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

    fn schemas() -> Value {
        json!([
            {"type":"function","function":{"name":"web_search","description":"Search the web.","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}},
            {"type":"function","function":{"name":"write_note","description":"Set note.","parameters":{"type":"object","properties":{"content":{"type":"string"},"mode":{"type":"string","enum":["replace","append"]}},"required":["content"]}}}
        ])
    }

    #[test]
    fn parses_granite_text_tool_call() {
        let c = r#"<tool_call>[{"arguments": {"query": "rust release"}, "name": "web_search"}]"#;
        let calls = parse_text_tool_calls(c, &schemas()).expect("should parse");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["function"]["name"], "web_search");
        let args = calls[0]["function"]["arguments"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(args).unwrap()["query"],
            "rust release"
        );
    }

    #[test]
    fn ignores_prose_and_unknown_tools() {
        assert!(parse_text_tool_calls("The note now has three sections.", &schemas()).is_none());
        assert!(parse_text_tool_calls("", &schemas()).is_none());
        // unknown tool name in `name(...)` form must not be picked up
        assert!(parse_text_tool_calls("fs::write(a.txt)", &schemas()).is_none());
    }

    #[test]
    fn positional_maps_to_first_required() {
        let calls = parse_text_tool_calls("web_search(latest rust)", &schemas()).expect("parse");
        let args = calls[0]["function"]["arguments"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(args).unwrap()["query"],
            "latest rust"
        );
    }

    #[test]
    fn grammar_names_tools_and_has_text_escape() {
        let g = tool_grammar(&schemas(), "call | text");
        assert!(g.contains("root ::= call | text"));
        assert!(g.contains(r#""\"write_note\"""#));
        assert!(g.contains(r#""\"content\"""#));
    }

    #[test]
    fn extract_partial_content_mid_string() {
        assert_eq!(
            extract_partial_content(r#"{"content":"hello wo"#).as_deref(),
            Some("hello wo")
        );
        assert_eq!(
            extract_partial_content(r#"{"mode":"replace","content":"hi"}"#).as_deref(),
            Some("hi")
        );
        assert_eq!(extract_partial_content(r#"{"mode":"replace""#), None);
    }

    #[test]
    fn fit_context_keeps_system_and_whole_turns() {
        let mut h = vec![
            json!({"role":"system","content":"sys"}),
            json!({"role":"user","content":"a".repeat(50)}),
            json!({"role":"assistant","content":"b".repeat(50)}),
            json!({"role":"user","content":"c".repeat(50)}),
            json!({"role":"assistant","content":"d".repeat(50)}),
        ];
        fit_context(&mut h, 200);
        assert_eq!(h[0]["role"], "system");
        assert!(h.len() < 5);
    }
}
