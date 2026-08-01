//! End-to-end test for the openharn-myelin sidecar.
//!
//! Spawns the real sidecar binary and a mock llama-server (OpenAI-compatible
//! `/v1/chat/completions` that returns scripted tool calls), then drives a
//! faithful, Myelin-mirroring tool executor against a real `.md` file. For every
//! one of Myelin's nine tools we assert the sidecar (a) recovers and dispatches
//! the tool call and (b) the executor's effect lands on disk (the note / notebook
//! actually changes). Web/search tools return canned-but-realistic results; the
//! sidecar's tool-call recovery and HTTP callback round-trip are fully exercised
//! for all of them.
//!
//! The write_note / format_note executors use the shared `myelin-edit-core`
//! crate (the same `plan_write` / `apply_format_op` the app runs), so the
//! on-disk edits are byte-identical to what the app would do. This is the
//! strict + prompt_tools path (the settings the user enabled).

use std::convert::Infallible;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use axum::{
    response::sse::{Event, Sse},
    routing::post,
    Json, Router,
};
use futures_util::stream::{self, Stream, StreamExt};
use myelin_edit_core::{apply_format_op, plan_write};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Test context + faithful tool executor
// ---------------------------------------------------------------------------

struct TestCtx {
    note_path: PathBuf,
    notebook_path: PathBuf,
}

fn run_tool(name: &str, args: &Value, ctx: &TestCtx) -> String {
    match name {
        "write_note" => {
            let content = args["content"].as_str().unwrap_or("");
            let mode = args["mode"].as_str().unwrap_or("");
            let find = args["find"].as_str().unwrap_or("");
            let body = std::fs::read_to_string(&ctx.note_path).unwrap_or_default();
            let plan = plan_write(&body, content, mode, find).expect("plan_write");
            std::fs::write(&ctx.note_path, &plan.new_body).unwrap();
            "Note successfully updated with ID: test-note".to_string()
        }
        "format_note" => {
            let op = args["operation"].as_str().unwrap_or("");
            let body = std::fs::read_to_string(&ctx.note_path).unwrap_or_default();
            let new_body = apply_format_op(&body, op);
            std::fs::write(&ctx.note_path, &new_body).unwrap();
            "Note successfully updated with ID: test-note".to_string()
        }
        "edit_notebook" => {
            let index = args["index"].as_i64().unwrap_or(0) as usize;
            let content = args["content"].as_str().unwrap_or("");
            let mut nb: Value =
                serde_json::from_str(&std::fs::read_to_string(&ctx.notebook_path).unwrap()).unwrap();
            if let Some(cells) = nb["cells"].as_array_mut() {
                if let Some(cell) = cells.get_mut(index) {
                    cell["source"] = json!([content.to_string()]);
                }
            }
            std::fs::write(&ctx.notebook_path, serde_json::to_string_pretty(&nb).unwrap()).unwrap();
            "Notebook cell updated.".to_string()
        }
        "read_note" => {
            let id = args["note_id"].as_str().unwrap_or("?");
            format!("Title: Other Note {id}\n\nThis is the body of note {id}.")
        }
        "fetch_web_page" => {
            "<html><body><h1>Example Domain</h1><p>Example page text for testing.</p></body></html>"
                .to_string()
        }
        "web_search" => {
            "[{\"title\":\"Rust Async\",\"url\":\"https://rust-lang.org/async\",\"snippet\":\"async/await in Rust\"}]"
                .to_string()
        }
        "search_notes" => {
            "[{\"id\":\"note-1\",\"title\":\"Project Plan\",\"snippet\":\"Quarterly project plan.\"}]"
                .to_string()
        }
        "search_documents" => {
            "[{\"id\":\"doc-1\",\"title\":\"Annual Report\",\"snippet\":\"The annual report summarizes results.\"}]"
                .to_string()
        }
        "find_in_note" => {
            let query = args["query"].as_str().unwrap_or("");
            let body = std::fs::read_to_string(&ctx.note_path).unwrap_or_default();
            let re = regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(query))).unwrap();
            let count = re.find_iter(&body).count();
            if count == 0 {
                format!("The text \"{query}\" does NOT appear in the open note.")
            } else {
                format!("Yes — \"{query}\" appears {count} time(s) in the open note.")
            }
        }
        other => format!("unknown tool {other}"),
    }
}

// ---------------------------------------------------------------------------
// Mock llama-server (returns scripted tool calls via SSE)
// ---------------------------------------------------------------------------

fn scripted_call(scenario: &str) -> String {
    let call = |name: &str, args: Value| {
        let args_str = serde_json::to_string(&args).unwrap();
        format!("<tool_call>[{{\"name\":\"{}\",\"arguments\":{}}}]", name, args_str)
    };
    match scenario {
        "write_note_replace" => call("write_note", json!({ "content": "# Title\nHello world" })),
        "write_note_append" => call("write_note", json!({ "content": " more", "mode": "append" })),
        "write_note_edit" => {
            call("write_note", json!({ "content": "Hi", "mode": "edit", "find": "Hello world" }))
        }
        "write_note_clear" => call("write_note", json!({ "content": "" })),
        "write_note_incomplete" => {
            "<tool_call>[{\"name\":\"write_note\",\"arguments\":{\"content\":\"This replacement arrives in several streamed chunks"
                .to_string()
        }
        "write_note_protocol_residue" => call(
            "write_note",
            json!({
                "content": "# Essay\nUseful prose.\n/content>}   > write_note(content="
            }),
        ),
        "write_note_lfm_native" => {
            r##"<|tool_call_start|>[write_note(content="# Native\nStreams (with Markdown links).")]<|tool_call_end|>"##
                .to_string()
        }
        "format_remove_headings" => call("format_note", json!({ "operation": "remove_headings" })),
        "format_uppercase" => call("format_note", json!({ "operation": "uppercase" })),
        "format_strip_markdown" => call("format_note", json!({ "operation": "strip_markdown" })),
        "read_note" => call("read_note", json!({ "note_id": "note-xyz" })),
        "fetch_web_page" => call("fetch_web_page", json!({ "url": "https://example.com" })),
        "web_search" => call("web_search", json!({ "query": "rust async" })),
        "search_notes" => call("search_notes", json!({ "query": "project" })),
        "search_documents" => call("search_documents", json!({ "query": "report" })),
        "find_in_note" => call("find_in_note", json!({ "query": "world" })),
        "edit_notebook" => {
            call("edit_notebook", json!({ "operation": "edit", "index": 0, "content": "print(42)" }))
        }
        "no_tool" => "Here is a short reply with no tool call.".to_string(),
        other => format!("Unknown scenario: {}", other),
    }
}

fn route(body: &Value) -> String {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    // Follow-up turn: a tool result is already in the history -> just close out.
    // In prompt-tools mode the tool result is flattened to a `user` message
    // prefixed with "Tool result:", so detect both shapes.
    if messages.iter().any(|m| {
        m["role"].as_str() == Some("tool")
            || m["content"]
                .as_str()
                .map(|c| c.contains("Tool result:"))
                .unwrap_or(false)
    }) {
        return "Understood. The note has been updated.".to_string();
    }
    for m in &messages {
        if let Some(text) = m["content"].as_str() {
            if let Some(scenario) = text.strip_prefix("__SCENARIO__:") {
                return scripted_call(scenario.trim());
            }
        }
    }
    "This is a plain-text reply with no tool call.".to_string()
}

fn build_sse(content: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut buf = String::new();
    for c in content.chars() {
        buf.push(c);
        if buf.len() >= 24 {
            events.push(
                Event::default().data(
                    json!({ "choices": [{ "index": 0, "delta": { "content": buf.clone() } }] })
                        .to_string(),
                ),
            );
            buf.clear();
        }
    }
    if !buf.is_empty() {
        events.push(
            Event::default().data(
                json!({ "choices": [{ "index": 0, "delta": { "content": buf.clone() } }] })
                    .to_string(),
            ),
        );
    }
    events.push(Event::default().data("[DONE]"));
    events
}

async fn completions(Json(body): Json<Value>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let content = route(&body);
    let events = build_sse(&content);
    Sse::new(stream::iter(events.into_iter().map(Ok::<_, Infallible>)))
}

fn mock_router() -> Router {
    Router::new().route("/v1/chat/completions", post(completions))
}

// ---------------------------------------------------------------------------
// Test driver
// ---------------------------------------------------------------------------

struct Outcome {
    tool: Option<String>,
    done: bool,
    error: Option<String>,
    note_deltas: usize,
    last_result: Option<String>,
    new_messages: usize,
}

fn tool_schemas() -> Value {
    json!([
        {"type":"function","function":{"name":"write_note","description":"Edit the open note.","parameters":{"type":"object","properties":{"content":{"type":"string"},"mode":{"type":"string"},"find":{"type":"string"}},"required":["content"]}}},
        {"type":"function","function":{"name":"format_note","description":"Format the open note.","parameters":{"type":"object","properties":{"operation":{"type":"string"}},"required":["operation"]}}},
        {"type":"function","function":{"name":"edit_notebook","description":"Edit a notebook.","parameters":{"type":"object","properties":{"operation":{"type":"string"},"index":{"type":"integer"},"content":{"type":"string"}},"required":["operation","index"]}}},
        {"type":"function","function":{"name":"read_note","description":"Read another note.","parameters":{"type":"object","properties":{"note_id":{"type":"string"}},"required":["note_id"]}}},
        {"type":"function","function":{"name":"fetch_web_page","description":"Fetch a web page.","parameters":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}}},
        {"type":"function","function":{"name":"web_search","description":"Search the web.","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}},
        {"type":"function","function":{"name":"search_notes","description":"Search notes.","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}},
        {"type":"function","function":{"name":"search_documents","description":"Search documents.","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}},
        {"type":"function","function":{"name":"find_in_note","description":"Find in note.","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}}
    ])
}

async fn run_chat(
    client: &reqwest::Client,
    sidecar_base: &str,
    mock_base: &str,
    request_id: &str,
    scenario: &str,
    ctx: &TestCtx,
) -> Outcome {
    let req = json!({
        "request_id": request_id,
        "base_url": mock_base,
        "model": "mock",
        "messages": [{ "role": "user", "content": format!("__SCENARIO__:{}", scenario) }],
        "tools": tool_schemas(),
        "options": {
            "strict": true,
            "prompt_tools": true,
            "no_think": false,
            "max_calls": 1,
            "total_max": 4,
            "tool_timeout_secs": 20,
            // This mirrors Myelin's host-authoritative operation routing.
            // The sidecar must not require lexical overlap for an operation.
            "friendly_results": true,
            "intent_is_tool": true,
            "call_only": matches!(
                scenario,
                "write_note_incomplete" | "write_note_protocol_residue"
            ),
        }
    });
    let resp = client
        .post(format!("{}/v1/chat/stream", sidecar_base))
        .json(&req)
        .send()
        .await
        .expect("POST /v1/chat/stream");
    assert!(
        resp.status().is_success(),
        "chat/stream HTTP error: {}",
        resp.status()
    );

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut event_name = String::new();
    let mut data = String::new();
    let mut out = Outcome {
        tool: None,
        done: false,
        error: None,
        note_deltas: 0,
        last_result: None,
        new_messages: 0,
    };

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.expect("stream chunk");
        buf.extend_from_slice(&bytes);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&buf[..nl]).trim_end().to_string();
            buf.drain(..=nl);
            if line.is_empty() {
                    if !event_name.is_empty() || !data.is_empty() {
                        match event_name.as_str() {
                        "tool" => {
                            let v: Value = serde_json::from_str(&data).expect("tool event json");
                            let id = v["id"].as_str().unwrap_or("").to_string();
                            let name = v["name"].as_str().expect("tool name").to_string();
                            let args = v["arguments"].as_str().unwrap_or("{}");
                            let args_val: Value =
                                serde_json::from_str(args).unwrap_or(json!({}));
                            assert!(
                                out.tool.is_none(),
                                "scenario {scenario}: expected at most one tool call, got {name}"
                            );
                            out.tool = Some(name.clone());
                            let result = run_tool(&name, &args_val, ctx);
                            out.last_result = Some(result.clone());
                            client
                                .post(format!("{}/v1/tool-result", sidecar_base))
                                .json(&json!({
                                    "request_id": request_id,
                                    "tool_call_id": id,
                                    "result": result,
                                }))
                                .send()
                                .await
                                .expect("POST /v1/tool-result");
                        }
                        "note_delta" => out.note_deltas += 1,
                        "done" => {
                            out.done = true;
                            let v: Value = serde_json::from_str(&data).expect("done event json");
                            out.new_messages = v["new_messages"].as_array().map_or(0, Vec::len);
                        }
                        "error" => out.error = Some(data.clone()),
                        _ => {}
                    }
                    event_name.clear();
                    data.clear();
                }
                continue;
            }
            if let Some(v) = line.strip_prefix("event:") {
                event_name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push_str(v.trim());
            }
        }
        if out.done || out.error.is_some() {
            break;
        }
    }
    out
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn sidecar_bin() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let target_dir =
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| format!("{}/target", manifest));
    for profile in ["debug", "release"] {
        let p = std::path::Path::new(&target_dir).join(profile).join("openharn-myelin");
        if p.exists() {
            return p;
        }
    }
    panic!("sidecar binary not found under {}/<debug|release>/openharn-myelin", target_dir);
}

async fn wait_healthy(base: &str) {
    let client = reqwest::Client::new();
    for _ in 0..150 {
        if client
            .get(format!("{}/health", base))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("sidecar did not become healthy at {base}");
}

async fn run_scenario(
    client: &reqwest::Client,
    sidecar_base: &str,
    mock_base: &str,
    request_id: &str,
    scenario: &str,
    ctx: &TestCtx,
) -> Outcome {
    eprintln!("[e2e] scenario: {scenario}");
    match tokio::time::timeout(
        Duration::from_secs(30),
        run_chat(client, sidecar_base, mock_base, request_id, scenario, ctx),
    )
    .await
    {
        Ok(o) => o,
        Err(_) => panic!("scenario {scenario} timed out after 30s"),
    }
}

#[tokio::test]
async fn every_tool_round_trips_and_edits_md() {
    let dir = std::env::temp_dir().join(format!("myelin-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = TestCtx {
        note_path: dir.join("note.md"),
        notebook_path: dir.join("nb.ipynb"),
    };

    // Seed the notebook the edit_notebook scenario mutates.
    let nb = json!({
        "cells": [{ "cell_type": "code", "source": ["print(1)"] }],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5,
    });
    std::fs::write(&ctx.notebook_path, serde_json::to_string_pretty(&nb).unwrap()).unwrap();

    // Mock llama-server.
    let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_port = mock_listener.local_addr().unwrap().port();
    let mock_base = format!("http://127.0.0.1:{}/v1", mock_port);
    tokio::spawn(async move {
        axum::serve(mock_listener, mock_router()).await.unwrap();
    });

    // Real sidecar binary.
    let sidecar_port = free_port();
    let sidecar_base = format!("http://127.0.0.1:{}", sidecar_port);
    let bin = sidecar_bin();
    let mut child = Command::new(&bin)
        .arg("--port")
        .arg(sidecar_port.to_string())
        .spawn()
        .expect("spawn sidecar");
    wait_healthy(&sidecar_base).await;

    let client = reqwest::Client::new();
    let note = || std::fs::read_to_string(&ctx.note_path).unwrap_or_default();

    // --- write_note: replace ---
    std::fs::write(&ctx.note_path, "").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-replace", "write_note_replace", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("write_note"), "write_note replace dispatched");
    assert!(o.done, "write_note replace completed");
    assert!(o.new_messages >= 2, "done includes assistant/tool-result delta");
    assert_eq!(note().trim(), "# Title\nHello world", "write_note replace body");

    // --- incomplete strict write_note: preview streams, but nothing executes ---
    std::fs::write(&ctx.note_path, "Original remains authoritative").unwrap();
    let o = run_scenario(
        &client,
        &sidecar_base,
        &mock_base,
        "req-incomplete",
        "write_note_incomplete",
        &ctx,
    )
    .await;
    assert!(
        o.note_deltas > 1,
        "incomplete write_note should expose multiple real upstream deltas"
    );
    assert!(o.tool.is_none(), "incomplete write_note must not execute a tool");
    assert!(
        o.error
            .as_deref()
            .unwrap_or_default()
            .contains("Generation ended before write_note completed"),
        "incomplete write_note should report an unfinished generation: {:?}",
        o.error
    );
    assert_eq!(
        note().trim(),
        "Original remains authoritative",
        "incomplete preview must not persist"
    );

    // --- protocol residue: preview streams, but a complete contaminated call
    // is rejected before tool dispatch and cannot be saved ---
    std::fs::write(&ctx.note_path, "Original remains authoritative").unwrap();
    let o = run_scenario(
        &client,
        &sidecar_base,
        &mock_base,
        "req-protocol-residue",
        "write_note_protocol_residue",
        &ctx,
    )
    .await;
    assert!(o.note_deltas > 1, "contaminated content should have previewed");
    assert!(o.tool.is_none(), "contaminated write_note must not execute");
    assert!(
        o.error
            .as_deref()
            .unwrap_or_default()
            .contains("mixed tool protocol text"),
        "contaminated call should report protocol residue: {:?}",
        o.error
    );
    assert_eq!(
        note().trim(),
        "Original remains authoritative",
        "contaminated preview must not persist"
    );

    // --- framed LFM text call: native content streams and parses without
    // leaking its wrapper into the saved body ---
    std::fs::write(&ctx.note_path, "").unwrap();
    let o = run_scenario(
        &client,
        &sidecar_base,
        &mock_base,
        "req-lfm-native",
        "write_note_lfm_native",
        &ctx,
    )
    .await;
    assert!(o.note_deltas > 1, "LFM text content should stream incrementally");
    assert_eq!(o.tool.as_deref(), Some("write_note"));
    assert_eq!(
        note().trim(),
        "# Native\nStreams (with Markdown links).",
        "native wrapper must not enter note content"
    );

    // --- write_note: append ---
    std::fs::write(&ctx.note_path, "Base").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-append", "write_note_append", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("write_note"), "write_note append dispatched");
    let f = note();
    assert!(f.contains("Base") && f.contains("more"), "write_note append result: {:?}", f);

    // --- write_note: edit (find/replace) ---
    std::fs::write(&ctx.note_path, "Hello world").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-edit", "write_note_edit", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("write_note"), "write_note edit dispatched");
    assert_eq!(note().trim(), "Hi", "write_note edit body");

    // --- write_note: clear (planner replaces whole body with empty) ---
    std::fs::write(&ctx.note_path, "Something").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-clear", "write_note_clear", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("write_note"), "write_note clear dispatched");
    assert!(note().trim().is_empty(), "write_note clear body");

    // --- format_note: remove_headings ---
    std::fs::write(&ctx.note_path, "# Title\nBody\n## Sub").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-fmt-h", "format_remove_headings", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("format_note"), "format_note dispatched");
    assert_eq!(note().trim(), "Title\nBody\nSub", "format_note remove_headings body");

    // --- format_note: uppercase ---
    std::fs::write(&ctx.note_path, "hello world").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-fmt-u", "format_uppercase", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("format_note"), "format_note uppercase dispatched");
    assert_eq!(note().trim(), "HELLO WORLD", "format_note uppercase body");

    // --- format_note: strip_markdown ---
    std::fs::write(&ctx.note_path, "# H\n**bold**\n- item").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-fmt-s", "format_strip_markdown", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("format_note"), "format_note strip_markdown dispatched");
    let f = note();
    assert!(!f.contains('#') && !f.contains("**") && !f.contains("- item"), "strip_markdown result: {:?}", f);

    // --- read_note (canned, but dispatch + result asserted) ---
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-read", "read_note", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("read_note"), "read_note dispatched");
    assert!(o.last_result.as_deref().unwrap().contains("Other Note"), "read_note result");

    // --- fetch_web_page ---
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-fetch", "fetch_web_page", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("fetch_web_page"), "fetch_web_page dispatched");
    assert!(o.last_result.as_deref().unwrap().contains("Example"), "fetch_web_page result");

    // --- web_search ---
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-search", "web_search", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("web_search"), "web_search dispatched");
    assert!(o.last_result.as_deref().unwrap().contains("rust-lang.org"), "web_search result");

    // --- search_notes ---
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-snotes", "search_notes", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("search_notes"), "search_notes dispatched");
    assert!(o.last_result.as_deref().unwrap().contains("Project Plan"), "search_notes result");

    // --- search_documents ---
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-sdocs", "search_documents", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("search_documents"), "search_documents dispatched");
    assert!(o.last_result.as_deref().unwrap().contains("Annual Report"), "search_documents result");

    // --- find_in_note (reads the real note file) ---
    std::fs::write(&ctx.note_path, "world here\nnothing").unwrap();
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-find", "find_in_note", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("find_in_note"), "find_in_note dispatched");
    assert!(
        o.last_result.as_deref().unwrap().contains("appears 1 time(s)"),
        "find_in_note result: {:?}",
        o.last_result
    );

    // --- edit_notebook (.ipynb actually changes) ---
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-nb", "edit_notebook", &ctx).await;
    assert_eq!(o.tool.as_deref(), Some("edit_notebook"), "edit_notebook dispatched");
    let nb: Value = serde_json::from_str(&std::fs::read_to_string(&ctx.notebook_path).unwrap()).unwrap();
    assert_eq!(
        nb["cells"][0]["source"][0].as_str(),
        Some("print(42)"),
        "edit_notebook cell source"
    );

    // --- no_tool: plain text, no dispatch, completes ---
    let o = run_scenario(&client, &sidecar_base, &mock_base, "req-none", "no_tool", &ctx).await;
    assert!(o.tool.is_none(), "no_tool must not dispatch a tool");
    assert!(o.done, "no_tool completed");
    assert!(o.error.is_none(), "no_tool had no error: {:?}", o.error);

    // Cleanup.
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&dir);
}
