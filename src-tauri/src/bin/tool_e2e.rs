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
        // Accept any Markdown heading the model added (# or ##).
        let ok = used.iter().any(|t| t == "write_note") && body.contains("# ");
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
    // history saying it was written) where a model can chat a fake
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
        let mut fmt_ok = 0;
        for i in 0..rounds {
            let req = reqs[i % reqs.len()];
            let mut store = sample_store();
            store.notes.get_mut(&store.open_id).unwrap().body = short.to_string();
            let used = chat_h(client, base, model, req, history, &mut store).await;
            let body = store.open_body();
            let grew = body.len() > short.len() + 200;
            // Any reasonable Markdown structure: heading, bullet, numbered list,
            // or bold section label.
            let formatted = body.contains("#")
                || body.contains("\n- ")
                || body.contains("\n* ")
                || body.contains("\n• ")
                || body.contains("\n1.")
                || body.contains("\n2.")
                || body.contains("**")
                || body.starts_with("- ");
            // The bug we fixed is "note doesn't change" with REAL content (not
            // meta-commentary). Gate on write-landed + grew + not-meta; formatting
            // is a reported model-quality metric.
            let ok =
                used.iter().any(|t| t == "write_note") && body != short && grew && !is_meta(&body);
            eprintln!(
                "  hammer {}/{}: updated={ok} grew={grew} formatted={formatted} len={} req={:?}",
                i + 1, rounds, body.len(), trunc(req, 40)
            );
            if ok {
                updated += 1;
            }
            if formatted {
                fmt_ok += 1;
            }
        }
        out.push((
            "expand follow-up updates the note (hammer)".into(),
            updated == rounds,
            format!("{updated}/{rounds} updated; {fmt_ok}/{rounds} had markdown formatting"),
        ));
    }

    // ---- HAMMER: removal. The "New note 14" chat showed the model REFUSING
    // ("I can't delete notes") and the backstop generating content for "remove
    // all content". Each removal phrasing must end with an EMPTY note. ----
    {
        let content = "AI is transforming industries by enabling smarter automation and decision-making.\n\nIt impacts healthcare, education, and the creative arts in profound ways.";
        let reqs = [
            "remove all content from the note",
            "delete this note",
            "clear the note completely",
            "empty the note",
        ];
        let mut cleared = 0;
        for (i, req) in reqs.iter().enumerate() {
            let mut store = sample_store();
            store.notes.get_mut(&store.open_id).unwrap().body = content.to_string();
            let used = chat(client, base, model, req, &mut store).await;
            let body = store.open_body();
            let ok = body.trim().is_empty();
            eprintln!("  removal {}/{}: emptied={ok} len={} req={:?}", i + 1, reqs.len(), body.len(), req);
            if ok {
                cleared += 1;
            }
            let _ = used;
        }
        out.push((
            "remove/clear content (hammer)".into(),
            cleared == reqs.len(),
            format!("{cleared}/{} emptied the note", reqs.len()),
        ));
    }

    // ---- terse "expand it" on a short note (the New note 14 case): the model
    // claimed it expanded but didn't. The harvest backstop must actually grow it. ----
    {
        let short = "AI is transforming industries by enabling smarter automation, personalized experiences, and breakthroughs in research.";
        let mut grown = 0;
        let rounds = 3;
        for i in 0..rounds {
            let mut store = sample_store();
            store.notes.get_mut(&store.open_id).unwrap().body = short.to_string();
            let used = chat(client, base, model, "expand it", &mut store).await;
            let body = store.open_body();
            // Must grow with REAL content about AI, not meta-commentary, and not
            // drift to the old harvest-example topic ("dog").
            let lc = body.to_lowercase();
            let ok = used.iter().any(|t| t == "write_note")
                && body.len() > short.len() + 150
                && !is_meta(&body)
                && lc.contains("ai")
                && !lc.contains("dog");
            eprintln!("  expand-it {}/{}: ok={ok} meta={} {}->{} chars", i + 1, rounds, is_meta(&body), short.len(), body.len());
            if ok {
                grown += 1;
            }
        }
        out.push((
            "terse 'expand it' (hammer)".into(),
            grown == rounds,
            format!("{grown}/{rounds} actually expanded the note"),
        ));
    }

    // ---- topic fidelity: expanding a Mona Lisa note must stay about the Mona
    // Lisa, never drift to the harvest example's topic (the "dogs" bug). ----
    {
        let short = "The Mona Lisa is a famous portrait painting by Leonardo da Vinci, housed in the Louvre in Paris.";
        let mut on = 0;
        let rounds = 3;
        for i in 0..rounds {
            let mut store = sample_store();
            store.notes.get_mut(&store.open_id).unwrap().body = short.to_string();
            let used = chat(client, base, model, "expand it", &mut store).await;
            let b = store.open_body().to_lowercase();
            let on_topic = b.contains("mona") || b.contains("lisa") || b.contains("leonardo")
                || b.contains("painting") || b.contains("louvre");
            let drifted = b.contains("dog");
            let ok = used.iter().any(|t| t == "write_note")
                && store.open_body().len() > short.len() + 150
                && on_topic
                && !drifted
                && !is_meta(&store.open_body());
            eprintln!("  mona-lisa {}/{}: ok={ok} on_topic={on_topic} drifted_to_dogs={drifted} len={}", i + 1, rounds, store.open_body().len());
            if ok {
                on += 1;
            }
        }
        out.push((
            "expand stays on topic (mona lisa, hammer)".into(),
            on == rounds,
            format!("{on}/{rounds} stayed on the note's topic"),
        ));
    }

    // ---- Gating fix (the "New note 18" bug): verb-less follow-up corrections
    // in an ACTIVE edit thread must still get write_note. Before the fix,
    // per-message gating stripped the tool on "no, that's wrong" / a typo'd
    // "formate it", so the model could only chat a fake "done". Checks the
    // gating logic deterministically (cold vs in-thread), then confirms one
    // end-to-end round drives a real write_note call. (Some models' weak `#`
    // heading syntax is a separate, documented model ceiling — not this.) ----
    {
        let verbless = [
            "no thats wrong",
            "you didnt do it right",
            "still not what i wanted",
            "nope try again",
        ];
        let n = verbless.len();
        let mut cold = 0; // no edit-thread context → should get NO write_note
        let mut warm = 0; // in an edit thread → should get write_note
        for c in &verbless {
            let has = |edit: bool| {
                agent::select_tools(c, true, edit)
                    .iter()
                    .any(|t| t["function"]["name"] == "write_note")
            };
            if has(false) {
                cold += 1;
            }
            if has(true) {
                warm += 1;
            }
        }
        // End-to-end: a verb-less correction after a prior write must now drive a
        // real write_note call (tool present via the edit-thread signal).
        let history = "User: write a short note about the eiffel tower\nAssistant: Done — I've written it to the note.";
        let mut store = sample_store();
        store.notes.get_mut(&store.open_id).unwrap().body = "The Eiffel Tower is in Paris.".into();
        let used = chat_h(
            client,
            base,
            model,
            "no thats not what i wanted, redo it",
            history,
            &mut store,
        )
        .await;
        let e2e_wrote = used.iter().any(|t| t == "write_note");
        eprintln!("  edit-thread gating: cold={cold}/{n} warm={warm}/{n} e2e_wrote={e2e_wrote}");
        out.push((
            "edit-thread gating keeps write_note on verb-less corrections".into(),
            cold == 0 && warm == n && e2e_wrote,
            format!(
                "verb-less corrections offered write_note: {cold}/{n} cold vs {warm}/{n} in edit thread; e2e correction called write_note={e2e_wrote}"
            ),
        ));
    }

    // ---- Deterministic gating for the new capabilities (no model/server). These
    // assert that `select_tools` offers the right tool set for each phrasing. Each
    // check computes its own bool so a regression pinpoints the exact scenario. ----
    {
        let has = |tools: &[Value], name: &str| tools.iter().any(|t| t["function"]["name"] == name);

        // Web search: an explicit "search the web" request must offer web_search
        // and must NOT offer search_notes (it's not about the user's notes).
        {
            let tools = agent::select_tools("search the web for the latest rust release", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "web_search") && !has(&tools, "search_notes");
            out.push(("gate: web search -> web_search, not search_notes".into(), ok, format!("tools={names:?}")));
        }

        // "google ..." is a web-search idiom.
        {
            let tools = agent::select_tools("google the weather in paris", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "web_search");
            out.push(("gate: google ... -> web_search".into(), ok, format!("tools={names:?}")));
        }

        // Document question ("the pdf"): offer search_documents, and since there's
        // no write intent, do NOT offer write_note.
        {
            let tools = agent::select_tools("what does the pdf say about transformers", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "search_documents") && !has(&tools, "write_note");
            out.push(("gate: pdf question -> search_documents, not write_note".into(), ok, format!("tools={names:?}")));
        }

        // "according to the paper ..." is a document-grounded question.
        {
            let tools = agent::select_tools("according to the paper, what is attention", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "search_documents");
            out.push(("gate: paper question -> search_documents".into(), ok, format!("tools={names:?}")));
        }

        // A full URL must offer fetch_web_page.
        {
            let tools = agent::select_tools("summarize https://example.com", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "fetch_web_page");
            out.push(("gate: full url -> fetch_web_page".into(), ok, format!("tools={names:?}")));
        }

        // A bare domain (no scheme) must also offer fetch_web_page.
        {
            let tools = agent::select_tools("summarize example.com", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "fetch_web_page");
            out.push(("gate: bare domain -> fetch_web_page".into(), ok, format!("tools={names:?}")));
        }

        // Small talk: no tools at all.
        {
            let tools = agent::select_tools("hello there", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = tools.is_empty();
            out.push(("gate: small talk -> no tools".into(), ok, format!("tools={names:?}")));
        }

        // Note-write intent ("rewrite the note") must offer write_note.
        {
            let tools = agent::select_tools("rewrite the note with headings", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "write_note");
            out.push(("gate: rewrite note -> write_note".into(), ok, format!("tools={names:?}")));
        }

        // Searching the user's OTHER notes: offer search_notes + read_note, but NOT
        // web_search (it's a notes query, not a web query).
        {
            let tools = agent::select_tools("search my other notes about pasta", true, false);
            let names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&tools, "search_notes") && has(&tools, "read_note") && !has(&tools, "web_search");
            out.push(("gate: other notes -> search_notes+read_note, not web_search".into(), ok, format!("tools={names:?}")));
        }

        // Edit-thread: a verb-less correction gets write_note ONLY when in an active
        // edit thread; cold (no thread) it must not.
        {
            let warm = agent::select_tools("no thats wrong", true, true);
            let cold = agent::select_tools("no thats wrong", true, false);
            let warm_names: Vec<String> = warm
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let cold_names: Vec<String> = cold
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            let ok = has(&warm, "write_note") && !has(&cold, "write_note");
            out.push((
                "gate: verb-less correction -> write_note only in edit thread".into(),
                ok,
                format!("warm={warm_names:?} cold={cold_names:?}"),
            ));
        }
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

/// Like `chat` but with prior chat history, using the app's REAL per-message
/// gating (`select_tools` on the latest message) to choose the tools.
async fn chat_h(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    request: &str,
    history: &str,
    store: &mut Store,
) -> Vec<String> {
    // Mirror state.rs: derive the edit-thread signal from prior "User:" turns in
    // the embedded history so verb-less follow-ups keep write_note.
    let user_lines: Vec<&str> = history
        .lines()
        .filter_map(|l| l.strip_prefix("User:").map(|s| s.trim()))
        .collect();
    let edit_thread = agent::in_edit_thread(&user_lines);
    let tools = agent::select_tools(request, true, edit_thread);
    chat_t(client, base, model, request, history, store, &tools).await
}

/// The shared multi-turn loop, with the offered `tools` passed in explicitly so
/// tests can compare real gating against a forced tool set.
async fn chat_t(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    request: &str,
    history: &str,
    store: &mut Store,
    tools: &[Value],
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
    let mut used = Vec::new();

    for _ in 0..5 {
        let (text, calls) = match one_turn(client, base, model, &messages, tools).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  turn error: {e}");
                break;
            }
        };
        if calls.is_empty() {
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

/// Stream one completion; collect assistant text + assembled tool calls. Omits
/// the tools/tool_choice when none are gated in, mirroring run_chat.
async fn one_turn(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    messages: &[Value],
    tools: &[Value],
) -> Result<(String, Vec<ToolCall>), String> {
    let mut body = json!({
        "model": model, "messages": messages,
        "temperature": 0.2, "stream": true
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
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
    let port_s = port.to_string();
    let mut args: Vec<String> = vec![
        "-m".into(), model.into(), "--jinja".into(), "--ctx-size".into(), "4096".into(),
        "--port".into(), port_s, "--no-warmup".into(),
    ];
    // Optional chat-template override (e.g. the corrected LFM2.5 template that
    // fixes multi-turn tool calling). Set CHAT_TEMPLATE_FILE to A/B test it.
    if let Ok(tpl) = std::env::var("CHAT_TEMPLATE_FILE") {
        if !tpl.is_empty() {
            args.push("--chat-template-file".into());
            args.push(tpl);
        }
    }
    eprintln!("starting: {} {}", bin, args.join(" "));
    Command::new(bin)
        .args(&args)
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

/// True if the body reads like meta-commentary about the task rather than the
/// actual note content (the failure where "expand it" wrote "the note should be
/// expanded…" instead of the expanded essay).
fn is_meta(body: &str) -> bool {
    let b = body.to_lowercase();
    // Targeted meta phrases (avoid generic ones like "here is the" that can
    // appear in legitimate prose).
    [
        "the note should",
        "the updated note",
        "this expansion will",
        "key points to cover",
        "should be expanded",
        "to enrich the original",
        "the current summary gives",
    ]
    .iter()
    .any(|p| b.contains(p))
}

fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}
