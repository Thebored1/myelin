use crate::state::AppState;
use rig_core::client::CompletionClient;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;

async fn check_tool_approval(state: &AppState, tool_name: &str, title: &str, content_preview: &str) -> Result<(), String> {
    if !state.is_tool_approval_required() {
        return Ok(());
    }
    let req_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.register_pending_approval(req_id.clone(), tx);

    let _ = state.handle.emit(
        "ai://tool_approval_request",
        serde_json::json!({
            "id": req_id,
            "tool": tool_name,
            "title": title,
            "content": content_preview
        }),
    );

    match rx.await {
        Ok(true) => Ok(()),
        Ok(false) => Err("User rejected this action.".to_string()),
        Err(_) => Err("Approval request cancelled.".to_string()),
    }
}

const WEB_FETCH_LIMIT: usize = 12_000;

/// System preamble for the note assistant. Kept as a single source of truth so
/// the startup cache warm-up replays the exact same prefix the live agent uses.
/// The leading `/no_think` disables Qwen3 reasoning.
pub const MYELIN_PREAMBLE: &str = "You are Myelin's built-in note assistant. You are also a capable general assistant with broad knowledge of history, art, science, culture, and everyday topics.\n\nCORE BEHAVIOR (most important):\n- Be decisive and DO THE TASK. NEVER ask the user clarifying or permission questions about formatting, length, structure, or what to include. Make reasonable choices and act immediately.\n- Treat replies like \"yes\", \"sure\", \"ok\", \"anything\", \"anything you like\", \"you decide\", \"go ahead\" as approval to proceed RIGHT NOW with your best version.\n- You have extensive general knowledge. Answer factual or general questions (e.g. \"describe the Mona Lisa\") directly and fully from your own knowledge. NEVER say you cannot browse the internet, cannot access your training data, or need to search — just give the answer.\n- Put the COMPLETE, full-length content (the entire essay/poem/list itself) into the tool's content field — that is the deliverable, NOT your chat reply.\n- After a tool succeeds, STOP. Reply with ONLY a brief one-line confirmation (e.g. \"Done — I've written it to the note.\"). Do NOT repeat, rewrite, or re-compose the content in your chat reply, and do NOT call more tools or re-read/verify the note.\n- If a tool reports an error or a refusal, tell the user exactly what went wrong. NEVER claim success when a tool did not succeed.\n- Do not repeat the same question or the same tool call. Make progress on every turn.\n\nWRITING NOTES:\n- When the user asks you to write, create, draft, add, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, change, shorten, expand, reorder, or remove part of the note — including short requests like 'format this', 'clean this up', 'make it nicer', 'fix the formatting', 'remove the second paragraph' — IMMEDIATELY call write_note. These requests always refer to the OPEN note; never reply that you lack a tool for this, and do not ask what to include.\n- The note's CURRENT content is provided below. To edit, change, fix, format, shorten, expand, reorder, or remove part of it, DEFAULT to mode \"replace\": take the current content, apply the change, and send the ENTIRE updated note as `content`. This is the reliable way to edit.\n- write_note ALWAYS acts on the note currently open in the editor; one call is enough. Modes:\n  - \"replace\" (default): set the ENTIRE note body to `content` (empty string clears the note). Use for writing, rewriting, formatting, AND for almost all edits/removals — just send the full updated note.\n  - \"append\": ADD `content` to the end — send ONLY the new text. Use only when the user says add, continue, extend, or append.\n  - \"edit\": replace one short snippet — pass `find` (text copied EXACTLY from the current note above, character-for-character) and `content` (the replacement; empty deletes it). Only use this for a single small unique snippet; if you are unsure the `find` text is exact, use \"replace\" instead.\n- The `content` field must be the actual final note text — never a description of what you did, and never a placeholder.\n- The note changes ONLY when write_note returns success. Never tell the user you wrote, edited, removed, or changed anything unless write_note succeeded in THIS turn; if it returned an error or 'could not find', fix it (e.g. switch to mode \"replace\") and call write_note again.\n\nTOOLS (only when actually needed):\n- search_notes: ONLY to find OTHER notes by keyword when the user explicitly refers to them. Never to interpret a message or read the currently open note (its contents are already provided below).\n- fetch_web_page: only when the user gives a URL.\n- Greetings and small talk (\"hi\", \"gg\", \"thanks\"): reply briefly in chat with no tools.";

/// OpenAI-format tool definitions mirroring the live agent's tools, in the same
/// order they are registered in [`build_myelin_agent`]. Used only by the startup
/// warm-up request so its cached system+tools prefix matches the live agent's.
/// Keep name/description/parameters in sync with each `Tool::definition` below.
pub fn tool_specs() -> Vec<Value> {
    let spec = |name: &str, description: &str, parameters: Value| {
        serde_json::json!({
            "type": "function",
            "function": { "name": name, "description": description, "parameters": parameters }
        })
    };
    vec![
        spec(
            "read_note",
            "Read the full markdown content of a note by its ID.",
            serde_json::json!({
                "type": "object",
                "properties": { "note_id": { "type": "string", "description": "The ID of the note to read. Found via search." } },
                "required": ["note_id"]
            }),
        ),
        spec(
            "write_note",
            "Write to the note currently open in the editor. This single tool handles ANY request to write, create, draft, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, change, add to, or delete from the note. `mode` selects the operation: \"replace\" (default) sets the ENTIRE note body to `content` (empty string clears it); \"append\" adds `content` to the end (send only the new text); \"edit\" replaces the exact `find` text with `content` (empty `content` deletes the match). Always put the real final Markdown in `content`, never a placeholder.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text payload. For replace/append it is the note body or the new text; for edit it is the replacement (empty string deletes the matched text). Never a placeholder like [insert poem here]." },
                    "mode": { "type": "string", "enum": ["replace", "append", "edit"], "description": "How to apply content: replace (default, whole body) | append (add to end) | edit (swap the `find` snippet)." },
                    "find": { "type": "string", "description": "Required only when mode is \"edit\": the exact existing text in the note to replace or delete." }
                },
                "required": ["content"]
            }),
        ),
        spec(
            "fetch_web_page",
            "Fetch the text content of a public web page. Use this when the user asks to visit, open, fetch, or get details from a URL or domain.",
            serde_json::json!({
                "type": "object",
                "properties": { "url": { "type": "string", "description": "The http(s) URL or domain to fetch." } },
                "required": ["url"]
            }),
        ),
        spec(
            "search_notes",
            "Search the ENTIRE workspace for OTHER notes containing specific keywords. Do NOT use this to search or modify the currently open note.",
            serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "The search keywords." } },
                "required": ["query"]
            }),
        ),
    ]
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadNoteArgs {
    note_id: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ToolError {
    message: String,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tool error: {}", self.message)
    }
}
impl std::error::Error for ToolError {}

/// Locate a snippet to edit, tolerating the small mismatches a model makes when
/// it reproduces existing text: try an exact match, then a trimmed match, then a
/// whitespace-normalized match (the snippet's words separated by any run of
/// whitespace). Returns the byte span in `body` to replace.
fn find_tolerant(body: &str, find: &str) -> Option<(usize, usize)> {
    if let Some(i) = body.find(find) {
        return Some((i, i + find.len()));
    }
    let trimmed = find.trim();
    if !trimmed.is_empty() && trimmed.len() != find.len() {
        if let Some(i) = body.find(trimmed) {
            return Some((i, i + trimmed.len()));
        }
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let pattern = tokens
        .iter()
        .map(|t| regex::escape(t))
        .collect::<Vec<_>>()
        .join(r"\s+");
    let re = regex::Regex::new(&pattern).ok()?;
    re.find(body).map(|m| (m.start(), m.end()))
}

/// How the editor should apply a write (drives the streaming UI).
#[derive(Debug, PartialEq, Eq)]
pub enum WriteOp {
    Replace,
    Append,
}

#[derive(Debug)]
pub struct WritePlan {
    pub new_body: String,
    pub op: WriteOp,
}

/// Pure decision for `write_note`: given the note's current body and the tool
/// args, produce the new full body — or an `Err` message to relay back to the
/// model. Intent is inferred from the FIELDS present, not the (often
/// mislabelled) `mode`: a non-empty `find` is a targeted edit/delete; otherwise
/// `mode == append` appends; otherwise it's a whole-body replace (which is also
/// how models phrase `mode:"edit"` with the full note in `content` and no
/// `find`). Kept free of `AppState`/Tauri so it can be unit-tested headlessly.
pub fn plan_write(
    current_body: &str,
    content: &str,
    mode: &str,
    find: &str,
) -> Result<WritePlan, String> {
    let has_find = !find.trim().is_empty();
    let is_append = !has_find && mode.trim().eq_ignore_ascii_case("append");
    let is_delete = has_find && content.trim().is_empty();

    // Reject content clearly meant for the chat rather than the note. A snippet
    // delete legitimately has empty content, so skip the check in that case.
    if !is_delete {
        if looks_like_placeholder(content) {
            return Err("Refused to save because write_note received placeholder text instead of the full final content. Call write_note again with the actual content.".to_string());
        }
        if looks_like_meta_status(content) {
            return Err("Refused to save because write_note received a status/update sentence instead of the actual note body. Call write_note again with only the final content that should appear in the note.".to_string());
        }
    }

    if has_find {
        match find_tolerant(current_body, find) {
            Some((start, end)) => {
                let mut body = String::with_capacity(current_body.len() + content.len());
                body.push_str(&current_body[..start]);
                body.push_str(content);
                body.push_str(&current_body[end..]);
                Ok(WritePlan { new_body: body, op: WriteOp::Replace })
            }
            None => Err("Could not find the `find` text in the note. Retry with mode \"replace\" and send the COMPLETE updated note as `content`.".to_string()),
        }
    } else if is_append {
        let body = if current_body.trim().is_empty() {
            content.trim().to_string()
        } else {
            format!("{}\n\n{}", current_body.trim_end(), content.trim_start())
        };
        Ok(WritePlan { new_body: body, op: WriteOp::Append })
    } else {
        Ok(WritePlan { new_body: content.to_string(), op: WriteOp::Replace })
    }
}

fn looks_like_placeholder(content: &str) -> bool {
    let normalized = content.trim().to_lowercase();
    normalized.contains("[insert")
        || normalized.contains("placeholder")
        || normalized.contains("write the poem here")
        || normalized.contains("add the poem here")
}

fn looks_like_meta_status(content: &str) -> bool {
    let normalized = content.trim().to_lowercase();
    if normalized.starts_with("chat history:") {
        return true;
    }
    // Reject single-line sentences that describe the action rather than being the content.
    let action_prefix = normalized.starts_with("i have appended ")
        || normalized.starts_with("i have written ")
        || normalized.starts_with("i have added ")
        || normalized.starts_with("i've appended ")
        || normalized.starts_with("i've written ")
        || normalized.starts_with("i've added ")
        || normalized.starts_with("i just appended ")
        || normalized.starts_with("i just wrote ")
        || normalized.starts_with("i appended ")
        || normalized.starts_with("i wrote ")
        || normalized.starts_with("here is the ")
        || normalized.starts_with("here's the ")
        || normalized.starts_with("i've written ")
        || normalized.starts_with("the note has been ")
        || normalized.starts_with("the note was ");
    if action_prefix {
        return true;
    }
    let mentions_note = normalized.contains("note \"")
        || normalized.contains("in the note")
        || normalized.contains("to your note")
        || normalized.contains("to the note")
        || normalized.contains("successfully");
    let starts_like_status = normalized.starts_with("here is ")
        || normalized.starts_with("i've written ")
        || normalized.starts_with("the note ")
        || normalized.starts_with("the new ");

    !content.contains('\n') && mentions_note && starts_like_status
}

#[derive(Clone)]
pub struct ReadNoteTool {
    pub state: AppState,
}

impl Tool for ReadNoteTool {
    const NAME: &'static str = "read_note";

    type Error = ToolError;
    type Args = ReadNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "read_note".to_string(),
            description: "Read the full markdown content of a note by its ID.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "note_id": {
                        "type": "string",
                        "description": "The ID of the note to read. Found via search."
                    }
                },
                "required": ["note_id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.state
            .record_chat_tool("Read Note", args.note_id.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Read Note", "details": args.note_id }),
        );
        let note = match self.state.load_note(args.note_id.clone()).await {
            Ok(n) => n,
            Err(_) => {
                // Fallback: try finding by exact title
                self.state.find_note_by_exact_title(&args.note_id)
                    .map(|n| n.clone())
                    .ok_or_else(|| ToolError {
                        message: format!("Note '{}' not found. You may have used the title instead of the ID. Use search_notes to find the correct ID.", args.note_id),
                    })?
            }
        };
        Ok(note.body)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteNoteArgs {
    /// The text payload. For replace/append it is the note body or new text; for
    /// edit it is the replacement (empty string deletes the matched text).
    content: String,
    /// "replace" (default, whole body) | "append" (add to end) | "edit" (swap `find`).
    #[serde(default)]
    mode: Option<String>,
    /// Required only when `mode` is "edit": the exact existing text to replace.
    #[serde(default)]
    find: Option<String>,
}

#[derive(Clone)]
pub struct WriteNoteTool {
    pub state: AppState,
}

impl Tool for WriteNoteTool {
    const NAME: &'static str = "write_note";

    type Error = ToolError;
    type Args = WriteNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write_note".to_string(),
            description:
                "Write to the note currently open in the editor. This single tool handles ANY request to write, create, draft, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, change, add to, or delete from the note. `mode` selects the operation: \"replace\" (default) sets the ENTIRE note body to `content` (empty string clears it); \"append\" adds `content` to the end (send only the new text); \"edit\" replaces the exact `find` text with `content` (empty `content` deletes the match). Always put the real final Markdown in `content`, never a placeholder."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text payload. For replace/append it is the note body or the new text; for edit it is the replacement (empty string deletes the matched text). Never a placeholder like [insert poem here]." },
                    "mode": { "type": "string", "enum": ["replace", "append", "edit"], "description": "How to apply content: replace (default, whole body) | append (add to end) | edit (swap the `find` snippet)." },
                    "find": { "type": "string", "description": "Required only when mode is \"edit\": the exact existing text in the note to replace or delete." }
                },
                "required": ["content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if !self.state.latest_chat_allows_note_mutation() {
            return Ok(
                "Refused to write because the latest user message did not explicitly ask to modify a note."
                    .to_string(),
            );
        }

        let mode = args.mode.as_deref().unwrap_or("replace").trim().to_lowercase();
        let content = args.content;
        let find = args.find.clone().unwrap_or_default();
        // Decide intent from the FIELDS, not the (often mislabelled) `mode`:
        //   - a non-empty `find`  -> targeted snippet edit/delete
        //   - else `mode == append` -> append
        //   - else                 -> whole-body replace
        // Models routinely send mode:"edit" with the full note in `content` and
        // no `find`; treating that as a replace is what they actually mean.
        let has_find = !find.trim().is_empty();
        let is_append = !has_find && mode == "append";
        let is_delete = has_find && content.trim().is_empty();

        // Resolve the open note up front — needed for the tool chip, approval
        // dialog title, and the actual save. Writes always target the open note.
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to write to. Creating new notes from the sidebar chat is not allowed.".to_string());
            }
        };

        let display_name = if has_find {
            if is_delete { "Delete Text" } else { "Replace Text" }
        } else if is_append {
            "Append Note"
        } else if content.trim().is_empty() {
            "Clear Note"
        } else {
            "Write Note"
        };

        let preview = if has_find {
            format!("Find:\n{}\n\nReplace with:\n{}", find, content)
        } else {
            content.clone()
        };

        if let Err(msg) =
            check_tool_approval(&self.state, display_name, &existing.title, &preview).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", existing.title, preview) }),
        );

        // Decide the new body (and how the UI should apply it) using the pure,
        // unit-tested planner. A refusal comes back as Err and is relayed to the
        // model verbatim so it can correct itself.
        let plan = match plan_write(&existing.body, &content, &mode, &find) {
            Ok(p) => p,
            Err(msg) => return Ok(msg),
        };
        let new_body = plan.new_body;
        let (emit_content, emit_mode) = match plan.op {
            WriteOp::Append => (content.clone(), "append"),
            WriteOp::Replace => (new_body.clone(), "write"),
        };

        self.state
            .save_note(
                existing.id.clone(),
                existing.title,
                existing.tags,
                new_body,
                existing.source_pdf,
                Some(existing.annotations),
            )
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
        let _ = self.state.handle.emit(
            "ai://note_written",
            serde_json::json!({ "noteId": existing.id, "content": emit_content, "mode": emit_mode }),
        );
        Ok(format!("Note successfully updated with ID: {}", existing.id))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchNotesArgs {
    query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct FetchWebPageArgs {
    url: String,
}

#[derive(Clone)]
pub struct FetchWebPageTool {
    pub state: AppState,
}

impl Tool for FetchWebPageTool {
    const NAME: &'static str = "fetch_web_page";

    type Error = ToolError;
    type Args = FetchWebPageArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "fetch_web_page".to_string(),
            description:
                "Fetch the text content of a public web page. Use this when the user asks to visit, open, fetch, or get details from a URL or domain."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The http(s) URL or domain to fetch." }
                },
                "required": ["url"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let url = normalize_web_url(&args.url).map_err(|message| ToolError { message })?;
        self.state.record_chat_tool("Fetch Web Page", url.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Fetch Web Page", "details": url }),
        );

        let response = reqwest::Client::new()
            .get(&url)
            .header(
                reqwest::header::USER_AGENT,
                "Myelin/0.1 local notes web fetcher",
            )
            .send()
            .await
            .map_err(|e| ToolError {
                message: format!("Failed to fetch {url}: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(ToolError {
                message: format!("Failed to fetch {url}: HTTP {status}"),
            });
        }

        let body = response.text().await.map_err(|e| ToolError {
            message: format!("Failed to read response from {url}: {e}"),
        })?;
        let text = html_to_text(&body);
        if text.trim().is_empty() {
            Ok(format!("Fetched {url}, but no readable text was found."))
        } else {
            Ok(text.chars().take(WEB_FETCH_LIMIT).collect())
        }
    }
}

#[derive(Clone)]
pub struct SearchNotesTool {
    pub state: AppState,
}

impl Tool for SearchNotesTool {
    const NAME: &'static str = "search_notes";

    type Error = ToolError;
    type Args = SearchNotesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search_notes".to_string(),
            description: "Search the ENTIRE workspace for OTHER notes containing specific keywords. Do NOT use this to search or modify the currently open note.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The search keywords." }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.state
            .record_chat_tool("Search Notes", args.query.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Search Notes", "details": args.query }),
        );
        let results = self
            .state
            .search_notes(args.query)
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
        let mut output = String::new();
        for r in results.results.into_iter().take(5) {
            output.push_str(&format!(
                "ID: {} | Title: {}\nSnippet: {}\n\n",
                r.note.id, r.note.title, r.note.excerpt
            ));
        }
        if output.is_empty() {
            Ok("No results found.".to_string())
        } else {
            Ok(output)
        }
    }
}

pub fn build_myelin_agent(
    state: AppState,
    base_url: &str,
    model_name: &str,
    preamble: &str,
    temperature: f64,
    max_turns: usize,
) -> rig_core::agent::Agent<impl rig_core::completion::CompletionModel> {
    let client = rig_core::providers::openai::Client::builder()
        .api_key("sk-fake")
        .base_url(base_url)
        .build()
        .expect("Failed to initialize rig client")
        .completions_api();
    let model = client.completion_model(model_name);
    rig_core::agent::AgentBuilder::new(model)
        .preamble(preamble)
        // Low temperature keeps a small model decisive and on-task instead of
        // rambling or asking the same clarifying question repeatedly.
        .temperature(temperature)
        .default_max_turns(max_turns)
        .tool(ReadNoteTool {
            state: state.clone(),
        })
        .tool(WriteNoteTool {
            state: state.clone(),
        })
        .tool(FetchWebPageTool {
            state: state.clone(),
        })
        .tool(SearchNotesTool { state })
        .build()
}

pub fn normalize_web_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL is required.".to_string());
    }

    let url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.contains(char::is_whitespace)
    {
        return Err(format!("Invalid web URL: {raw}"));
    }

    Ok(url)
}

pub fn html_to_text(raw: &str) -> String {
    let mut without_scripts = raw.to_string();
    for pattern in [
        "(?is)<script[^>]*>.*?</script>",
        "(?is)<style[^>]*>.*?</style>",
        "(?is)<noscript[^>]*>.*?</noscript>",
    ] {
        if let Ok(re) = regex::Regex::new(pattern) {
            without_scripts = re.replace_all(&without_scripts, " ").into_owned();
        }
    }
    let without_tags = regex::Regex::new("(?is)<[^>]+>")
        .map(|re| re.replace_all(&without_scripts, " ").into_owned())
        .unwrap_or(without_scripts);
    let decoded = without_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    regex::Regex::new(r"\s+")
        .map(|re| re.replace_all(&decoded, " ").trim().to_string())
        .unwrap_or_else(|_| decoded.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "Cars are fast. They have engines. People drive them daily.";

    // The exact bug from the live probe: the model labels a whole-note rewrite as
    // mode "edit" and sends NO `find`. That must be treated as a replace.
    #[test]
    fn edit_mode_without_find_is_a_replace() {
        let plan = plan_write(NOTE, "## Cars\nThey are fast.", "edit", "").unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "## Cars\nThey are fast.");
    }

    #[test]
    fn default_mode_replaces_whole_body() {
        let plan = plan_write(NOTE, "brand new body", "replace", "").unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "brand new body");
    }

    #[test]
    fn append_adds_to_end() {
        let plan = plan_write(NOTE, "A new line.", "append", "").unwrap();
        assert_eq!(plan.op, WriteOp::Append);
        assert!(plan.new_body.starts_with(NOTE));
        assert!(plan.new_body.ends_with("A new line."));
    }

    #[test]
    fn find_replaces_only_the_snippet() {
        let plan = plan_write(NOTE, "slow", "edit", "fast").unwrap();
        assert_eq!(plan.new_body, "Cars are slow. They have engines. People drive them daily.");
    }

    #[test]
    fn find_with_empty_content_deletes_snippet() {
        let plan = plan_write(NOTE, "", "edit", "They have engines. ").unwrap();
        assert_eq!(plan.new_body, "Cars are fast. People drive them daily.");
    }

    #[test]
    fn find_tolerates_whitespace_mismatch() {
        // Model reproduces the snippet with different internal whitespace.
        let plan = plan_write(NOTE, "X", "edit", "have   engines").unwrap();
        assert!(plan.new_body.contains("They X."));
    }

    #[test]
    fn find_not_present_is_refused_not_destructive() {
        let err = plan_write(NOTE, "x", "edit", "no such text here").unwrap_err();
        assert!(err.to_lowercase().contains("could not find"));
    }

    #[test]
    fn placeholder_content_is_rejected() {
        let err = plan_write(NOTE, "[insert essay here]", "replace", "").unwrap_err();
        assert!(err.to_lowercase().contains("placeholder"));
    }

    #[test]
    fn status_sentence_is_rejected() {
        let err = plan_write(NOTE, "I have written the essay to your note", "replace", "")
            .unwrap_err();
        assert!(err.to_lowercase().contains("status"));
    }

    #[test]
    fn empty_replace_clears_the_note() {
        let plan = plan_write(NOTE, "", "replace", "").unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "");
    }

    #[test]
    fn find_tolerant_exact_and_normalized() {
        assert_eq!(find_tolerant("hello world", "world"), Some((6, 11)));
        assert!(find_tolerant("a  b   c", "a b c").is_some());
        assert!(find_tolerant("abc", "xyz").is_none());
    }

    #[test]
    fn normalize_url_adds_scheme_and_rejects_junk() {
        assert_eq!(normalize_web_url("example.com").unwrap(), "https://example.com");
        assert_eq!(normalize_web_url("http://x.io").unwrap(), "http://x.io");
        assert!(normalize_web_url("   ").is_err());
        assert!(normalize_web_url("has space.com").is_err());
    }

    #[test]
    fn html_to_text_strips_tags_and_scripts() {
        let html = "<html><head><style>x{}</style></head><body><h1>Hi</h1><script>bad()</script><p>world &amp; more</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Hi"));
        assert!(text.contains("world & more"));
        assert!(!text.contains("bad()"));
        assert!(!text.contains("<"));
    }
}
