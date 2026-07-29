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
        let p = std::path::Path::new(&target_dir).join(profile).join("openharn-myelin");
        if p.exists() {
            return p;
        }
    }
    panic!("sidecar binary not found under {}/<debug|release>/openharn-myelin", target_dir);
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
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

/// Drive one request through the sidecar and report whether the model emitted a
/// `write_note` tool call, plus any chat text it produced.
async fn run_mode(
    client: &reqwest::Client,
    sidecar_base: &str,
    llama_base: &str,
    request_id: &str,
    options: Value,
) -> (bool, String) {
    let user_msg = "NOTE: (empty)\n\nUser request: write a poem about the sea in the note";
    let req = json!({
        "request_id": request_id,
        "base_url": llama_base,
        "model": "lfm2",
        "messages": [
            {"role": "system", "content": MYELIN_PREAMBLE},
            {"role": "user", "content": user_msg}
        ],
        "tools": tool_schemas(),
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
    assert!(resp.status().is_success(), "chat/stream HTTP {}", resp.status());

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut event_name = String::new();
    let mut data = String::new();
    let mut saw_write_note = false;
    let mut chat = String::new();

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
                            if name == "write_note" {
                                saw_write_note = true;
                            }
                            client
                                .post(format!("{}/v1/tool-result", sidecar_base))
                                .json(&json!({
                                    "request_id": request_id,
                                    "tool_call_id": v["id"].as_str().unwrap_or(""),
                                    "result": "Note successfully updated with ID: test-note",
                                }))
                                .send()
                                .await
                                .expect("tool-result");
                        }
                        "chat_chunk" => {
                            if let Some(d) = serde_json::from_str::<Value>(&data).ok() {
                                if let Some(delta) = d["delta"].as_str() {
                                    chat.push_str(delta);
                                }
                            }
                        }
                        "done" => eprintln!("[live]   DONE"),
                        "error" => eprintln!("[live]   ERROR: {data}"),
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
    (saw_write_note, chat)
}

#[tokio::test]
async fn live_lfm2_writes_note() {
    let llama_base = std::env::var("LLAMA_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:39300/v1".to_string());

    let probe = reqwest::Client::new();
    let reachable = probe
        .get(format!("{}/health", llama_base))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    if !reachable {
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
    let (a_called, a_chat) = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-A",
        json!({
            "strict": true, "prompt_tools": true, "no_think": false,
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    ).await;
    eprintln!("[live] MODE A chat text:\n{a_chat}");
    eprintln!(
        "[live] MODE A: write_note was{} called",
        if a_called { "" } else { " NOT" }
    );

    // Mode B: native tool calling (strict/prompt_tools OFF) — Myelin's corrected
    // LFM2 template is built for this; may work where prompt-tools doesn't.
    eprintln!("\n[live] ===== MODE B: native tools (strict/prompt_tools OFF) =====");
    let (b_called, b_chat) = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-B",
        json!({
            "strict": false, "prompt_tools": false, "no_think": false,
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    ).await;
    eprintln!("[live] MODE B chat text:\n{b_chat}");
    eprintln!(
        "[live] MODE B: write_note was{} called",
        if b_called { "" } else { " NOT" }
    );

    // Mode C: force the write_note tool via native tool_choice (the reliable
    // forcing path — llama-server honors `tool_choice`, unlike the GBNF grammar
    // which this model/server ignores). This is the fix for weak models that
    // chat instead of calling.
    eprintln!("\n[live] ===== MODE C: force_tool=write_note (native tool_choice) =====");
    let (c_called, c_chat) = run_mode(
        &client,
        &sidecar_base,
        &llama_base,
        "live-C",
        json!({
            "strict": false, "prompt_tools": false, "no_think": false,
            "force_tool": "write_note",
            "max_calls": 1, "total_max": 4, "tool_timeout_secs": 120, "max_tokens": 1024
        }),
    ).await;
    eprintln!("[live] MODE C chat text:\n{c_chat}");
    eprintln!(
        "[live] MODE C: write_note was{} called",
        if c_called { "" } else { " NOT" }
    );

    // Mode D: the host has already classified this operation and requires a
    // mutation. Restrict the schema to the relevant tool and remove the text
    // branch from prompt-tools grammar.
    eprintln!("\n[live] ===== MODE D: prompt-tools call-only + write_note subset =====");
    let (d_called, d_chat) = run_mode(
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
    ).await;
    eprintln!("[live] MODE D chat text:\n{d_chat}");
    eprintln!(
        "[live] MODE D: write_note was{} called",
        if d_called { "" } else { " NOT" }
    );

    // Mode E: keep the model's native LFM call format but force a call and
    // expose only the relevant mutation schema.
    eprintln!("\n[live] ===== MODE E: native required + write_note subset =====");
    let (e_called, e_chat) = run_mode(
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
    ).await;
    eprintln!("[live] MODE E chat text:\n{e_chat}");
    eprintln!(
        "[live] MODE E: write_note was{} called",
        if e_called { "" } else { " NOT" }
    );

    let _ = child.kill();

    // Focused gate. Mode D is the strongest constraint the harness owns
    // (grammar-constrained prompt-tools, call-only, write_note-only schema): a
    // working model/server pair MUST produce a call here. Silence in this mode
    // is exactly the "new note 13" regression. It is the only asserted mode —
    // A/B/C/E depend on the model's native FC health and stay advisory
    // (their `[live] MODE x: write_note was/was NOT called` lines above).
    assert!(
        d_called,
        "MODE D (strict prompt-tools + call_only + write_note subset) produced no write_note call"
    );
}
