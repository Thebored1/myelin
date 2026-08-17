use super::*;
use serde_json::{json, Value};

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
fn parses_chat_template_closing_tool_wrapper() {
    let c = r#"<tool_call>[{"arguments":{"content":"A short essay."},"name":"write_note"}]</tool_call>"#;
    let calls = parse_text_tool_calls(c, &schemas()).expect("should parse");
    assert_eq!(calls[0]["function"]["name"], "write_note");
    let args = calls[0]["function"]["arguments"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(args).unwrap()["content"],
        "A short essay."
    );
}

#[test]
fn recovers_lfm_wrapper_with_missing_json_quote() {
    let raw = r##"<tool_call>[{"name":"write_note","arguments":{"content":"# Sea
Waves move beneath a gray sky.
</content>}]</tool_call>"##;
    let calls = parse_text_tool_calls(raw, &schemas()).expect("recover malformed LFM wrapper");
    assert_eq!(calls[0]["function"]["name"], "write_note");
    let args: Value =
        serde_json::from_str(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(args["content"], "# Sea\nWaves move beneath a gray sky.\n");
}

#[test]
fn does_not_recover_protocol_residue_as_note_content() {
    let raw = r##"<tool_call>[{"name":"write_note","arguments":{"content":"# Sea
</tool_call>
</content>}]</tool_call>"##;
    assert!(parse_text_tool_calls(raw, &schemas()).is_none());
}

#[test]
fn recovers_lfm_wrapper_with_unclosed_quote_and_template_punctuation() {
    let raw = r##"<tool_call>[{"name":"write_note","arguments":{"content":"# Sea
The sea breathes wide, deep and wide—
A whisper of salt, a restless tide.  」}</tool_call>"##;
    let calls = parse_text_tool_calls(raw, &schemas()).expect("recover incomplete wrapper");
    let args: Value =
        serde_json::from_str(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(
        args["content"],
        "# Sea\nThe sea breathes wide, deep and wide—\nA whisper of salt, a restless tide.  "
    );
}

#[test]
fn extracts_partial_lfm_native_content() {
    assert_eq!(
        extract_partial_content(
            r##"<|tool_call_start|>[write_note(content="# Sea\nThe sea is va"##
        )
        .as_deref(),
        Some("# Sea\nThe sea is va")
    );
    assert_eq!(
        extract_partial_content(r#"write_note(content='hello wo"#).as_deref(),
        Some("hello wo")
    );
}

#[test]
fn detects_generated_tool_protocol_residue() {
    assert!(note_content_has_protocol_residue(
        "# Essay\nUseful prose.\n/content>}   > write_note(content="
    ));
    assert!(note_content_has_protocol_residue(
        "# Essay\nUseful prose.</tool_call>"
    ));
    assert!(!note_content_has_protocol_residue(
        "# Rust\nUse `write_note(content)` as an ordinary API example."
    ));
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
fn parses_lfm_pythonic_named_arguments() {
    let calls = parse_text_tool_calls(
        r#"write_note(content="Sea, sky, and shore", mode="replace")"#,
        &schemas(),
    )
    .expect("parse");
    let args: Value =
        serde_json::from_str(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(args["content"], "Sea, sky, and shore");
    assert_eq!(args["mode"], "replace");
}

#[test]
fn parses_framed_lfm_content_with_markdown_parentheses() {
    let calls = parse_text_tool_calls(
            r##"<|tool_call_start|>[write_note(content="# Links\nUse [Rust](https://rust-lang.org).")]<|tool_call_end|>"##,
            &schemas(),
        )
        .expect("parse framed LFM call");
    let args: Value =
        serde_json::from_str(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(
        args["content"],
        "# Links\nUse [Rust](https://rust-lang.org)."
    );
}

#[test]
fn pythonic_unknown_keys_are_dropped() {
    // A hallucinated kwarg must not smuggle a malformed call through:
    // `foo` is not in the write_note schema and gets dropped.
    let calls = parse_text_tool_calls(r#"write_note(content="hello", foo="bar")"#, &schemas())
        .expect("parse");
    let args: Value =
        serde_json::from_str(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(args["content"], "hello");
    assert!(args.get("foo").is_none());
    // A call whose ENTIRE argument set is unknown is discarded, not
    // forwarded to the host as a content-less write_note.
    assert!(
        parse_text_tool_calls(r#"write_note(wrong_key="boom")"#, &schemas()).is_none(),
        "all-unknown Pythonic call must be discarded"
    );
}

#[test]
fn pythonic_calls_missing_required_arguments_are_rejected() {
    assert!(parse_text_tool_calls(r#"write_note(mode="replace")"#, &schemas()).is_none());
    assert!(
        parse_text_tool_calls(r#"write_note(content=None, mode="replace")"#, &schemas()).is_none()
    );
}

#[test]
fn grammar_names_tools_and_has_text_escape() {
    let g = tool_grammar(&schemas(), "call | text");
    assert!(g.contains("root ::= call | text"));
    assert!(g.contains(r#""\"write_note\""#));
    assert!(g.contains(r#""\"content\""#));
    assert!(g.contains("string ::= "));
    // Raw newlines, carriage returns, and tabs are excluded from JSON strings.
    assert!(g.contains(r#"[^"\\\n\r\t]"#));
    assert!(g.contains("t-"));
    // The new compact grammar has no ws rules and no recursive value/object.
    assert!(!g.contains("ws ::="));
    assert!(!g.contains("value ::= object"));
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
    // The latest user turn survives trimming.
    assert_eq!(h.last().unwrap()["content"], "d".repeat(50));
}

#[test]
fn fit_context_never_drops_the_current_user_turn() {
    // A single-turn multi-tool exchange: [system, user, assistant(tool_calls),
    // tool, ...]. The current user request and its tool results must survive
    // even when the conversation alone exceeds the budget.
    let mut h = vec![
        json!({"role":"system","content":"sys"}),
        json!({"role":"user","content":"search then write".repeat(40)}),
        json!({"role":"assistant","content":null,"tool_calls":[json!({"id":"1","function":{"name":"search_notes","arguments":"{}"}})]}),
        json!({"role":"tool","tool_call_id":"1","content":"result".repeat(40)}),
    ];
    let changed = fit_context(&mut h, 200);
    assert!(!changed, "the only turn must never be dropped");
    assert_eq!(h.len(), 4);
    assert_eq!(h[1]["content"], "search then write".repeat(40));
    assert_eq!(h[3]["tool_call_id"], "1");
}

#[test]
fn fit_context_excludes_the_note_bearing_system_from_budget() {
    // The system message embeds the note body; it must not consume the
    // conversation budget, or the model would see zero prior turns on long
    // notes even though the host persisted them.
    let mut h = vec![
        json!({"role":"system","content":"n".repeat(20_000)}),
        json!({"role":"user","content":"old q".repeat(10)}),
        json!({"role":"assistant","content":"old a".repeat(10)}),
        json!({"role":"user","content":"new q".repeat(10)}),
    ];
    fit_context(&mut h, 100);
    assert_eq!(h[0]["role"], "system");
    // The oldest turn is dropped, the current user turn is kept.
    assert_eq!(h[1]["content"], "new q".repeat(10));
    assert_eq!(h.len(), 2);
}

#[test]
fn decompose_single_call() {
    let plan = harness_decompose("write a note about rust", &schemas());
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].0, "write_note");
}

#[test]
fn decompose_multi_call() {
    let plan = harness_decompose("search the web for rust and write a note", &schemas());
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].0, "web_search");
    assert_eq!(plan[1].0, "write_note");
}

#[test]
fn decompose_no_match() {
    let plan = harness_decompose("hello how are you today", &schemas());
    assert_eq!(plan.len(), 0);
}

#[test]
fn decompose_empty() {
    let plan = harness_decompose("", &schemas());
    assert_eq!(plan.len(), 0);
}
