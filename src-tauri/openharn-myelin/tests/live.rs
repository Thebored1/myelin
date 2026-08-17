//! Live test: drive the REAL sidecar against a REAL llama-server (LFM2-8B).
//!
//! This is the companion to `e2e.rs` (which mocks the model). Here we let the
//! actual model decide whether to emit a `write_note` tool call for a note-writing
//! request — exactly the failure mode seen with "new note 13".
//!
//! Usage:
//!   1. Start llama-server with an LFM2 model, e.g.
//!      ~/.local/share/com.paper.myelin/bin/cpu/llama-server \
//!        --host 127.0.0.1 --port 39300 \
//!        --model /home/paper/Downloads/LFM2-8B-A1B-UD-Q2_K_XL.gguf \
//!        --ctx-size 8192 --n-gpu-layers 0 --parallel 1 \
//!        --chat-template-file ../templates/lfm2.jinja --jinja --reasoning off
//!   2. LLAMA_URL=http://127.0.0.1:39300/v1 cargo test --test live -- --nocapture
//!
//! The test skips (does not fail) if no llama-server is reachable at LLAMA_URL.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use futures_util::stream::StreamExt;
use serde_json::{json, Value};

fn sidecar_bin() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let target_dir =
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| format!("{}/target", manifest));
    for profile in ["debug", "release"] {
        let p = std::path::Path::new(&target_dir)
            .join(profile)
            .join("openharn-myelin");
        if p.exists() {
            return p;
        }
    }
    panic!(
        "sidecar binary not found under {}/<debug|release>/openharn-myelin",
        target_dir
    );
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn llama_health_url(base: &str) -> String {
    base.trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string()
        + "/health"
}

// Verbatim copy of MYELIN_PREAMBLE (src-tauri/src/agent.rs) — the sidecar appends
// its tool_prompt() to this system message, exactly as Myelin does.
const MYELIN_PREAMBLE: &str = concat!(
    "You are the assistant inside Myelin, a local notes app, powered by an open model running locally on the user's own machine. If asked what or who you are, identify yourself as Myelin's built-in AI assistant — do not claim to be proprietary or commercial software. The text of the note currently open in the editor is included in the user's message — you already have it.\n\n",
    "- To change the open note (write, rewrite, edit, format, add to, shorten, clear, etc.), call the write_note tool with the full result in `content`. The ONLY way to change the note is that tool call: never describe the edit, print the new note text, or type \"write_note\" or \"content:\" in your chat reply.\n",
    "- Write real Markdown: a heading line starts with \"# \" (a hash then a space), \"## \" for a sub-heading; bullets start with \"- \". \"**bold**\" is NOT a heading.\n",
    "- When editing, reproduce every line that should stay and change only what was asked. Never return an empty or much-shorter note unless the user explicitly asked to clear or shorten it.\n",
    "- When the user asks you to write what you found, researched, learned, or understood, put the ACTUAL information into the note as a finished, self-contained note — the real facts, perspectives, and details (use what you found in the conversation plus what you reliably know about the topic). NEVER write a question, an offer to do more (e.g. \"Would you like me to fetch the full text?\"), or a promise to act later (e.g. \"I will now fetch...\") as the note's content — the note holds finished information, not conversation. If you lack some detail, still write the best complete note you can from what you know rather than asking or deferring.\n",
    "- Use fetch_web_page only when the user gives a URL or web address (like example.com), and search_notes only when the user asks about your other notes. For greetings or general questions, just reply briefly — do not read, search, or fetch.\n\n",
    "Worked examples show only the editing style — the resulting note text you must pass as write_note's `content` (always via the tool call, never printed in chat):\n\n",
    "Example 1\n",
    "NOTE:\n**Cars**\nThey have engines.\n",
    "USER: make the title a heading\n",
    "(resulting note)\n# Cars\nThey have engines.\n\n",
    "Example 2\n",
    "NOTE:\n## Intro\nPersonal computers changed everything.\n## History\nIt began in the 1970s.\n",
    "USER: remove all headings\n",
    "(resulting note)\nIntro\nPersonal computers changed everything.\nHistory\nIt began in the 1970s.\n\n",
    "Example 3\n",
    "NOTE: (empty)\n",
    "USER: write a short note titled Sea\n",
    "(resulting note)\n# Sea\nThe sea is vast and restless."
);

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
        {"type":"function","function":{"name":"find_in_note","description":"Find in note.","parameters":{"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}}}
    ])
}

#[derive(Default)]
struct LiveOutcome {
    tools: Vec<String>,
    tool_arguments: Vec<Value>,
    chat: String,
    done: bool,
    error: bool,
}

/// Drive one request through the sidecar and record protocol-level behavior.
async fn run_mode(
    client: &reqwest::Client,
    sidecar_base: &str,
    llama_base: &str,
    request_id: &str,
    options: Value,
) -> LiveOutcome {
    run_request(
        client,
        sidecar_base,
        llama_base,
        request_id,
        "NOTE: (empty)\n\nUser request: write a poem about the sea in the note",
        options,
    )
    .await
}

async fn run_request(
    client: &reqwest::Client,
    sidecar_base: &str,
    llama_base: &str,
    request_id: &str,
    user_msg: &str,
    options: Value,
) -> LiveOutcome {
    let req = json!({
        "request_id": request_id,
        "base_url": llama_base,
        "model": "lfm2",
        "messages": [
            {"role": "system", "content": MYELIN_PREAMBLE},
            {"role": "user", "content": user_msg}
        ],
        "tools": tool_schemas(),
        "temperature": 0.0,
        "max_tokens": 256,
        "options": options,
    });

    let resp = client
        .post(format!("{}/v1/chat/stream", sidecar_base))
        .json(&req)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .expect("POST /v1/chat/stream");
    assert!(
        resp.status().is_success(),
        "chat/stream HTTP {}",
        resp.status()
    );

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut event_name = String::new();
    let mut data = String::new();
    let mut outcome = LiveOutcome::default();

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
                            let v: Value = serde_json::from_str(&data).expect("tool json");
                            let name = v["name"].as_str().unwrap_or("?").to_string();
                            let args = v["arguments"].as_str().unwrap_or("{}");
                            eprintln!("[live]   TOOL CALL: {name} args={args}");
                            outcome.tools.push(name.clone());
                            outcome
                                .tool_arguments
                                .push(serde_json::from_str(args).unwrap_or_else(|_| json!({})));
                            client
                                .post(format!("{}/v1/tool-result", sidecar_base))
                                .json(&json!({
                                    "request_id": request_id,
                                    "tool_call_id": v["id"].as_str().unwrap_or(""),
                                    "result": if name == "write_note" {
                                        "Note successfully updated with ID: test-note"
                                    } else if name == "search_notes" {
                                        "Found other note: machine learning notes"
                                    } else if name == "read_note" {
                                        "Title: Other Note\n\nMachine learning notes"
                                    } else {
                                        "Read-only result"
                                    },
                                }))
                                .send()
                                .await
                                .expect("tool-result");
                        }
                        "chat_chunk" => {
                            if let Some(d) = serde_json::from_str::<Value>(&data).ok() {
                                if let Some(delta) = d["delta"].as_str() {
                                    outcome.chat.push_str(delta);
                                }
                            }
                        }
                        "done" => {
                            outcome.done = true;
                            eprintln!("[live]   DONE");
                        }
                        "error" => {
                            outcome.error = true;
                            eprintln!("[live]   ERROR: {data}");
                        }
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
    }
    outcome
}

#[tokio::test]
async fn live_lfm2_writes_note() {
    let llama_base =
        std::env::var("LLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:39300/v1".to_string());

    let probe = reqwest::Client::new();
    let reachable = probe
        .get(llama_health_url(&llama_base))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    if !reachable {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI live test requires a reachable pinned llama-server at {llama_base}"
        );
        eprintln!("[live] SKIP: no llama-server at {llama_base} (set LLAMA_URL and start one)");
        return;
    }
    eprintln!("[live] using llama-server at {llama_base}");

    let sidecar_port = free_port();
    let sidecar_base = format!("http://127.0.0.1:{}", sidecar_port);
    let mut child = Command::new(sidecar_bin())
        .arg("--port")
        .arg(sidecar_port.to_string())
        .spawn()
        .expect("spawn sidecar");
    for _ in 0..150 {
        if probe
            .get(format!("{}/health", sidecar_base))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let client = reqwest::Client::new();

    // Mode A: the settings the user enabled — strict + prompt_tools.
    eprintln!("\n[live] ===== MODE A: strict + prompt_tools (current settings) =====");
    let a = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-A",
        json!({
            "strict": true, "prompt_tools": true, "no_think": false,
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    )
    .await;
    eprintln!("[live] MODE A chat text:\n{}", a.chat);
    eprintln!(
        "[live] MODE A: write_note was{} called",
        if a.tools.iter().any(|tool| tool == "write_note") {
            ""
        } else {
            " NOT"
        }
    );

    // Mode B: native tool calling (strict/prompt_tools OFF) — Myelin's corrected
    // LFM2 template is built for this; may work where prompt-tools doesn't.
    eprintln!("\n[live] ===== MODE B: native tools (strict/prompt_tools OFF) =====");
    let b = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-B",
        json!({
            "strict": false, "prompt_tools": false, "no_think": false,
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    )
    .await;
    eprintln!("[live] MODE B chat text:\n{}", b.chat);
    eprintln!(
        "[live] MODE B: write_note was{} called",
        if b.tools.iter().any(|tool| tool == "write_note") {
            ""
        } else {
            " NOT"
        }
    );

    // Mode C: force the write_note tool via native tool_choice (the reliable
    // forcing path — llama-server honors `tool_choice`, unlike the GBNF grammar
    // which this model/server ignores). This is the fix for weak models that
    // chat instead of calling.
    eprintln!("\n[live] ===== MODE C: force_tool=write_note (native tool_choice) =====");
    let c = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-C",
        json!({
            "strict": false, "prompt_tools": false, "no_think": false,
            "force_tool": "write_note",
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    )
    .await;
    eprintln!("[live] MODE C chat text:\n{}", c.chat);
    eprintln!(
        "[live] MODE C: write_note was{} called",
        if c.tools.iter().any(|tool| tool == "write_note") {
            ""
        } else {
            " NOT"
        }
    );

    // Mode D: the host has already classified this operation and requires a
    // mutation. Restrict the schema to the relevant tool and remove the text
    // branch from prompt-tools grammar.
    eprintln!("\n[live] ===== MODE D: prompt-tools call-only + write_note subset =====");
    let d = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-D",
        json!({
            "strict": true, "prompt_tools": true, "no_think": false,
            "friendly_results": true, "call_only": true, "intent_is_tool": true,
            "tool_subset": ["write_note"],
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    )
    .await;
    eprintln!("[live] MODE D chat text:\n{}", d.chat);
    eprintln!(
        "[live] MODE D: write_note was{} called",
        if d.tools.iter().any(|tool| tool == "write_note") {
            ""
        } else {
            " NOT"
        }
    );
    assert!(
        d.done && !d.error,
        "mutating request must finish cleanly: done={}, error={}",
        d.done,
        d.error
    );
    assert_eq!(
        d.tools.as_slice(),
        ["write_note"],
        "mutation must terminate after the intended single tool call"
    );

    // Mode E: keep the model's native LFM call format but force a call and
    // expose only the relevant mutation schema.
    eprintln!("\n[live] ===== MODE E: native required + write_note subset =====");
    let e = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-E",
        json!({
            "strict": false, "prompt_tools": false, "no_think": false,
            "friendly_results": true, "call_only": true, "intent_is_tool": true,
            "tool_subset": ["write_note"],
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    )
    .await;
    eprintln!("[live] MODE E chat text:\n{}", e.chat);
    eprintln!(
        "[live] MODE E: write_note was{} called",
        if e.tools.iter().any(|tool| tool == "write_note") {
            ""
        } else {
            " NOT"
        }
    );

    // Chat mode: a normal question must produce text without an unintended
    // mutation or read-only tool call.
    eprintln!("\n[live] ===== CHAT: answer without mutation =====");
    let chat = run_request(
        &client,
        &sidecar_base,
        &llama_base,
        "live-chat",
        "What is two plus two? Answer briefly.",
        json!({
            "chat_mode": true, "strict": false, "prompt_tools": false,
            "friendly_results": true, "intent_is_tool": false,
            "max_calls": 1, "total_max": 2, "tool_timeout_secs": 120, "max_tokens": 256
        }),
    )
    .await;
    assert!(
        chat.done && !chat.error,
        "chat request must complete cleanly"
    );
    assert!(
        !chat.chat.trim().is_empty(),
        "chat request must return a nonempty answer"
    );
    assert!(
        chat.tools.is_empty(),
        "chat request must not call tools: {:?}",
        chat.tools
    );

    // Read-only routing: search and read requests must select their named
    // read-only tool, not a write tool or a generic answer.
    let search = run_request(
        &client,
        &sidecar_base,
        &llama_base,
        "live-search",
        "Search my other notes for machine learning.",
        json!({
            "strict": true, "prompt_tools": true, "friendly_results": true,
            "call_only": true, "intent_is_tool": true, "chat_mode": true,
            "tool_subset": ["search_notes"],
            "max_calls": 1, "total_max": 2, "tool_timeout_secs": 120, "max_tokens": 512
        }),
    )
    .await;
    assert!(
        search.done && !search.error,
        "search request must complete cleanly"
    );
    assert_eq!(
        search.tools.as_slice(),
        ["search_notes"],
        "search must select search_notes"
    );

    let read = run_request(
        &client,
        &sidecar_base,
        &llama_base,
        "live-read",
        "Read my other note with id other.",
        json!({
            "strict": true, "prompt_tools": true, "friendly_results": true,
            "call_only": true, "intent_is_tool": true, "chat_mode": true,
            "tool_subset": ["read_note"],
            "max_calls": 1, "total_max": 2, "tool_timeout_secs": 120, "max_tokens": 512
        }),
    )
    .await;
    assert!(
        read.done && !read.error,
        "read request must complete cleanly"
    );
    assert_eq!(
        read.tools.as_slice(),
        ["read_note"],
        "read must select read_note"
    );

    assert!(
        d.tool_arguments
            .first()
            .and_then(|args| args["content"].as_str())
            .is_some_and(|content| !content.trim().is_empty()),
        "write request must carry nonempty note content"
    );

    // Cancellation: cancel immediately after headers and require the sidecar
    // to close with a recoverable completion rather than hanging.
    let cancel_id = "live-cancel";
    let cancel_response = client
        .post(format!("{sidecar_base}/v1/chat/stream"))
        .json(&json!({
            "request_id": cancel_id,
            "base_url": llama_base,
            "model": "lfm2",
            "messages": [{"role": "user", "content": "Write a detailed essay about the sea."}],
            "tools": tool_schemas(),
            "max_tokens": 2048,
            "options": {"generation_timeout_secs": 120}
        }))
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .expect("start live cancellation request");
    assert!(cancel_response.status().is_success());
    let mut cancel_stream = cancel_response.bytes_stream();
    let cancel_response = client
        .post(format!("{sidecar_base}/v1/cancel"))
        .json(&json!({"request_id": cancel_id}))
        .send()
        .await
        .expect("cancel live request");
    assert!(cancel_response.status().is_success());
    let mut cancelled_bytes = Vec::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(10), cancel_stream.next())
        .await
        .expect("cancelled live stream timeout")
    {
        cancelled_bytes.extend_from_slice(&chunk.expect("cancelled live stream chunk"));
    }
    assert!(
        String::from_utf8_lossy(&cancelled_bytes).contains("event: done"),
        "cancellation must end with done"
    );

    child.kill().expect("stop sidecar");
    child.wait().expect("wait for sidecar shutdown");

    // Focused gate. Mode D is the strongest constraint the harness owns
    // (grammar-constrained prompt-tools, call-only, write_note-only schema): a
    // working model/server pair MUST produce a call here. Silence in this mode
    // is exactly the "new note 13" regression. It is the only asserted mode —
    // A/B/C/E depend on the model's native FC health and stay advisory
    // (their `[live] MODE x: write_note was/was NOT called` lines above).
    assert!(
        d.tools.iter().any(|tool| tool == "write_note"),
        "MODE D (strict prompt-tools + call_only + write_note subset) produced no write_note call"
    );
}
