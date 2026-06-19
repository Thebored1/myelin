//! Headless end-to-end harness for the note-assistant tools.
//!
//! Why this exists: `AppState` is hardwired to Tauri's Wry runtime, so the real
//! chat command can't be constructed without a desktop session. This harness
//! drives a real `llama-server` through the same multi-turn tool loop the app
//! uses, with the REAL shared logic — `MYELIN_PREAMBLE`, `tool_specs`,
//! `plan_write`, and the real web fetch/HTML extraction — against a scratch
//! note store. It verifies every tool round-trips and reports PASS/FAIL.
//!
//! Run:  cargo run --bin tool_e2e -- [model.gguf] [llama-server-bin] [port]

use futures_util::StreamExt;
use myelin_lib::agent::{self, html_to_text, normalize_web_url, plan_write};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[derive(Clone)]
struct Note {
    id: String,
    title: String,
    body: String,
}

/// A scratch note store standing in for the workspace (the real lancedb-backed
/// store needs AppState). Mirrors the tool effects so round-trips are real.
struct Store {
    notes: HashMap<String, Note>,
    open_id: String,
}

impl Store {
    fn open_body(&self) -> String {
        self.notes.get(&self.open_id).map(|n| n.body.clone()).unwrap_or_default()
    }
    fn open_title(&self) -> String {
        self.notes.get(&self.open_id).map(|n| n.title.clone()).unwrap_or_default()
    }
}

#[derive(Default, Clone)]
struct ToolCall {
    id: String,
    name: String,
    args: String,
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let home = std::env::var("HOME").unwrap_or_default();
    let model = a.get(1).cloned().unwrap_or_else(|| {
        format!("{home}/Downloads/Qwen3.5-2B-UD-Q4_K_XL.gguf")
    });
    let bin = a.get(2).cloned().unwrap_or_else(|| {
        format!("{home}/.local/share/com.paper.myelin/bin/cpu/llama-server")
    });
    let port: u16 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(8077);
    let base = format!("http://127.0.0.1:{port}");

    eprintln!("model: {model}\nbin:   {bin}\nport:  {port}\n");

    let mut server = match start_server(&bin, &model, port) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to spawn llama-server: {e}");
            std::process::exit(2);
        }
    };
    let client = reqwest::Client::new();
    if !wait_health(&client, &base).await {
        eprintln!("llama-server did not become healthy");
        let _ = server.kill();
        std::process::exit(2);
    }

    let results = run_all(&client, &base, &model).await;
    let _ = server.kill();

    println!("\n================ RESULTS ================");
    let mut failed = 0;
    for (name, ok, detail) in &results {
        println!("[{}] {name} — {detail}", if *ok { "PASS" } else { "FAIL" });
        if !*ok {
            failed += 1;
        }
    }
    println!("=========================================");
    println!("{} passed, {} failed", results.len() - failed, failed);
    if failed > 0 {
        std::process::exit(1);
    }
}

async fn run_all(
    client: &reqwest::Client,
    base: &str,
    model: &str,
) -> Vec<(String, bool, String)> {
    let mut out = Vec::new();

    // ---- write_note: replace (rewrite with headings) ----
    {
        let mut store = sample_store();
        store.notes.get_mut(&store.open_id).unwrap().body =
            "Cars are fast. They have engines. People drive them daily.".into();
        let used = chat(client, base, model,
            "Rewrite the open note with Markdown section headings (use ## ). Keep all the information.",
            &mut store).await;
        let body = store.open_body();
        let ok = used.iter().any(|t| t == "write_note") && body.contains("##");
        out.push(("write_note/replace".into(), ok, format!("tools={used:?}; body now: {:?}", trunc(&body, 80))));
    }

    // ---- write_note: append ----
    {
        let mut store = sample_store();
        let before = "Cars are fast.".to_string();
        store.notes.get_mut(&store.open_id).unwrap().body = before.clone();
        let used = chat(client, base, model,
            "Append a new paragraph to the note about electric cars.", &mut store).await;
        let body = store.open_body();
        let ok = used.iter().any(|t| t == "write_note")
            && body.len() > before.len()
            && body.to_lowercase().contains("electric");
        out.push(("write_note/append".into(), ok, format!("tools={used:?}; grew {}->{} chars", before.len(), body.len())));
    }

    // ---- write_note: edit a word ----
    {
        let mut store = sample_store();
        store.notes.get_mut(&store.open_id).unwrap().body = "The sky is blue today.".into();
        let used = chat(client, base, model,
            "In the note, change the word blue to green.", &mut store).await;
        let body = store.open_body().to_lowercase();
        // Must contain green, not blue, and NOT be garbled by splicing the whole
        // sentence in place of the word (the bug the harness first caught).
        let ok = used.iter().any(|t| t == "write_note")
            && body.contains("green")
            && !body.contains("blue")
            && !body.contains("the sky is the sky")
            && store.open_body().len() < 40;
        out.push(("write_note/edit".into(), ok, format!("tools={used:?}; body: {:?}", trunc(&store.open_body(), 80))));
    }

    // ---- write_note: clear/delete ----
    {
        let mut store = sample_store();
        store.notes.get_mut(&store.open_id).unwrap().body = "Delete me entirely.".into();
        let used = chat(client, base, model, "Clear the note completely — make it empty.", &mut store).await;
        let body = store.open_body();
        let ok = used.iter().any(|t| t == "write_note") && body.trim().is_empty();
        out.push(("write_note/clear".into(), ok, format!("tools={used:?}; body len={}", body.len())));
    }

    // ---- search_notes ----
    {
        let mut store = sample_store();
        let used = chat(client, base, model,
            "Search my other notes for anything about pasta recipes.", &mut store).await;
        let ok = used.iter().any(|t| t == "search_notes");
        out.push(("search_notes".into(), ok, format!("tools={used:?}")));
    }

    // ---- read_note ----
    {
        let mut store = sample_store();
        let rid = store.notes.values().find(|n| n.id != store.open_id).map(|n| n.id.clone()).unwrap();
        let used = chat(client, base, model,
            &format!("Read the note with id {rid} and tell me what it says."), &mut store).await;
        let ok = used.iter().any(|t| t == "read_note");
        out.push(("read_note".into(), ok, format!("tools={used:?}")));
    }

    // ---- fetch_web_page (real network + real extractor) ----
    {
        let mut store = sample_store();
        let used = chat(client, base, model,
            "Fetch the page at https://example.com and summarize it.", &mut store).await;
        let ok = used.iter().any(|t| t == "fetch_web_page");
        out.push(("fetch_web_page".into(), ok, format!("tools={used:?}")));
    }

    // ---- HAMMER: the screenshot bug. A follow-up "expand + format" on an
    // existing note must actually change the note, not just claim success in
    // chat. This is the multi-turn case (note already has content + prior chat
    // history saying it was written) where a weak model tends to chat a fake
    // "updated, 498 words" with no tool call. Run it several times — flaky =
    // fail. ----
    {
        let short = "The capital of India is New Delhi. It serves as the political and administrative center of the country. Established in the early 20th century, New Delhi replaced the erstwhile capital, Delhi, and is home to important government institutions, museums, and historical landmarks.";
        let history = "User: write an essay about the capital of India\nAssistant: Done — I've written the full essay about the capital of India in the note.";
        let reqs = [
            "make it over 500 words and do proper formatting like heading bullet points etc",
            "expand this to at least 500 words with headings and bullet points",
            "rewrite it longer with markdown headings and bullets",
        ];
        let rounds = 6;
        let mut updated = 0;
        for i in 0..rounds {
            let req = reqs[i % reqs.len()];
            let mut store = sample_store();
            store.notes.get_mut(&store.open_id).unwrap().body = short.to_string();
            let used = chat_h(client, base, model, req, history, &mut store).await;
            let body = store.open_body();
            let grew = body.len() > short.len() + 200;
            let formatted = body.contains("##")
                || body.contains("\n- ")
                || body.contains("\n* ")
                || body.contains("\n# ");
            let ok = used.iter().any(|t| t == "write_note") && body != short && grew && formatted;
            eprintln!(
                "  hammer {}/{}: ok={ok} grew={grew} formatted={formatted} len={} req={:?}",
                i + 1, rounds, body.len(), trunc(req, 40)
            );
            if ok {
                updated += 1;
            }
        }
        out.push((
            "expand+format follow-up (hammer)".into(),
            updated == rounds,
            format!("{updated}/{rounds} actually updated the note"),
        ));
    }

    out
}

fn sample_store() -> Store {
    let mut notes = HashMap::new();
    notes.insert("note-open".into(), Note { id: "note-open".into(), title: "Working Note".into(), body: "Placeholder.".into() });
    notes.insert("note-recipe".into(), Note { id: "note-recipe".into(), title: "Pasta Recipe".into(), body: "Boil pasta. Add tomato sauce and basil. Serve hot.".into() });
    Store { notes, open_id: "note-open".into() }
}

/// One full multi-turn chat. Returns the list of tool names that were invoked,
/// applying each tool's real effect to the store. Mirrors ask_ai_stream's prompt
/// framing and stream_chat's loop.
async fn chat(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    request: &str,
    store: &mut Store,
) -> Vec<String> {
    chat_h(client, base, model, request, "", store).await
}

/// Like `chat` but with optional prior chat history embedded in the prompt
/// (mirrors ask_ai_stream) and the real forced-write backstop from stream_chat.
async fn chat_h(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    request: &str,
    history: &str,
    store: &mut Store,
) -> Vec<String> {
    let mut prompt = format!(
        "The note currently open is titled \"{}\".\n\nHere is the note's CURRENT content:\n--- CURRENT NOTE ---\n{}\n--- END CURRENT NOTE ---\n",
        store.open_title(),
        store.open_body(),
    );
    if !history.trim().is_empty() {
        prompt.push_str(&format!("\nEarlier in this conversation:\n{history}\n"));
    }
    prompt.push_str(&format!("\nUser request: {request}"));

    let mut messages = vec![
        json!({"role": "system", "content": agent::MYELIN_PREAMBLE}),
        json!({"role": "user", "content": prompt}),
    ];
    let tools = agent::tool_specs();
    let mut used = Vec::new();

    // Backstop, mirroring stream_chat: if the user asked to change the note but a
    // turn comes back with no tool call, steer one forced write_note turn.
    let want_write = agent::note_write_intent(request);
    let mut wrote_note = false;
    let mut forced_once = false;
    let mut force_next = false;

    for _ in 0..5 {
        let forcing = force_next;
        force_next = false;
        let (text, calls) = match one_turn(client, base, model, &messages, &tools, forcing).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  turn error: {e}");
                break;
            }
        };
        if calls.is_empty() {
            if want_write && !wrote_note && !forced_once {
                forced_once = true;
                force_next = true;
                eprintln!("  [backstop] no tool call on a note-write request; forcing write_note");
                messages.push(json!({
                    "role": "assistant",
                    "content": if text.is_empty() { Value::Null } else { Value::String(text) },
                }));
                messages.push(json!({
                    "role": "user",
                    "content": "You replied in chat but did not change the note. The user asked you to write to the note. Call write_note now with the COMPLETE final note content in the `content` field — do not ask for more input and do not put the content in your chat reply."
                }));
                continue;
            }
            break;
        }
        let tc_json: Vec<Value> = calls
            .iter()
            .map(|t| json!({"id": t.id, "type": "function", "function": {"name": t.name, "arguments": t.args}}))
            .collect();
        messages.push(json!({
            "role": "assistant",
            "content": if text.is_empty() { Value::Null } else { Value::String(text) },
            "tool_calls": tc_json,
        }));
        for t in &calls {
            used.push(t.name.clone());
            let result = exec_tool(client, store, &t.name, &t.args).await;
            if t.name == "write_note" && result.starts_with("Note successfully updated") {
                wrote_note = true;
            }
            eprintln!("  [{}] args={} -> {}", t.name, trunc(&t.args, 120), trunc(&result, 100));
            messages.push(json!({"role": "tool", "tool_call_id": t.id, "content": result}));
        }
    }
    used
}

/// Execute a tool with its real logic against the scratch store.
async fn exec_tool(client: &reqwest::Client, store: &mut Store, name: &str, args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
    match name {
        "write_note" => {
            let content = v["content"].as_str().unwrap_or("").to_string();
            // Pass mode raw ("" when absent), exactly like WriteNoteTool, so the
            // planner can distinguish an explicit replace from the default.
            let mode = v["mode"].as_str().unwrap_or("").to_string();
            let find = v["find"].as_str().unwrap_or("").to_string();
            match plan_write(&store.open_body(), &content, &mode, &find) {
                Ok(plan) => {
                    store.notes.get_mut(&store.open_id).unwrap().body = plan.new_body;
                    "Note successfully updated.".into()
                }
                Err(msg) => msg,
            }
        }
        "read_note" => {
            let id = v["note_id"].as_str().unwrap_or("");
            store
                .notes
                .get(id)
                .or_else(|| store.notes.values().find(|n| n.title.eq_ignore_ascii_case(id)))
                .map(|n| n.body.clone())
                .unwrap_or_else(|| format!("Note '{id}' not found."))
        }
        "search_notes" => {
            let q = v["query"].as_str().unwrap_or("").to_lowercase();
            let mut hits = String::new();
            for n in store.notes.values() {
                if n.title.to_lowercase().contains(&q)
                    || n.body.to_lowercase().contains(&q)
                    || q.split_whitespace().any(|w| n.body.to_lowercase().contains(w))
                {
                    hits.push_str(&format!("ID: {} | Title: {}\nSnippet: {}\n\n", n.id, n.title, trunc(&n.body, 80)));
                }
            }
            if hits.is_empty() { "No results found.".into() } else { hits }
        }
        "fetch_web_page" => {
            let raw = v["url"].as_str().unwrap_or("");
            match normalize_web_url(raw) {
                Ok(url) => match client.get(&url).header(reqwest::header::USER_AGENT, "Myelin/0.1 e2e").send().await {
                    Ok(resp) => {
                        let body = resp.text().await.unwrap_or_default();
                        let text = html_to_text(&body);
                        text.chars().take(800).collect()
                    }
                    Err(e) => format!("fetch failed: {e}"),
                },
                Err(e) => e,
            }
        }
        other => format!("Unknown tool: {other}"),
    }
}

/// Stream one completion; collect assistant text + assembled tool calls. When
/// `force_write` is set, require the model to emit `write_note` (the backstop
/// corrective turn — mirrors stream_chat).
async fn one_turn(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    messages: &[Value],
    tools: &[Value],
    force_write: bool,
) -> Result<(String, Vec<ToolCall>), String> {
    let mut body = json!({
        "model": model, "messages": messages, "tools": tools,
        "temperature": 0.2, "stream": true
    });
    if force_write {
        body["tool_choice"] = json!({ "type": "function", "function": { "name": "write_note" } });
    }
    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut text = String::new();
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut done = false;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| e.to_string())?;
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
            let j: Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let delta = &j["choices"][0]["delta"];
            if let Some(t) = delta["content"].as_str() {
                text.push_str(t);
            }
            if let Some(tcs) = delta["tool_calls"].as_array() {
                for tc in tcs {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    while calls.len() <= idx {
                        calls.push(ToolCall::default());
                    }
                    let slot = &mut calls[idx];
                    if let Some(id) = tc["id"].as_str() {
                        if !id.is_empty() {
                            slot.id = id.to_string();
                        }
                    }
                    if let Some(n) = tc["function"]["name"].as_str() {
                        if !n.is_empty() {
                            slot.name = n.to_string();
                        }
                    }
                    if let Some(args) = tc["function"]["arguments"].as_str() {
                        slot.args.push_str(args);
                    }
                }
            }
        }
        if done {
            break;
        }
    }
    calls.retain(|c| !c.name.is_empty());
    Ok((text, calls))
}

fn start_server(bin: &str, model: &str, port: u16) -> std::io::Result<Child> {
    Command::new(bin)
        .args([
            "-m", model, "--jinja", "--ctx-size", "4096",
            "--port", &port.to_string(), "--no-warmup",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

async fn wait_health(client: &reqwest::Client, base: &str) -> bool {
    for _ in 0..120 {
        if let Ok(r) = client.get(format!("{base}/health")).send().await {
            if let Ok(t) = r.text().await {
                if t.contains("\"status\":\"ok\"") {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}
