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
pub const MYELIN_PREAMBLE: &str = "You are Myelin's built-in note assistant. You are also a capable general assistant with broad knowledge of history, art, science, culture, and everyday topics.\n\nCORE BEHAVIOR (most important):\n- Be decisive and DO THE TASK. NEVER ask the user clarifying or permission questions about formatting, length, structure, or what to include. Make reasonable choices and act immediately.\n- Treat replies like \"yes\", \"sure\", \"ok\", \"anything\", \"anything you like\", \"you decide\", \"go ahead\" as approval to proceed RIGHT NOW with your best version.\n- You have extensive general knowledge. Answer factual or general questions (e.g. \"describe the Mona Lisa\") directly and fully from your own knowledge. NEVER say you cannot browse the internet, cannot access your training data, or need to search — just give the answer.\n- After a tool reports success, STOP calling tools. Do NOT re-read, search, or verify the note. Reply with one short sentence confirming what you did.\n- If a tool reports an error or a refusal, tell the user exactly what went wrong. NEVER claim success when a tool did not succeed.\n- Do not repeat the same question or the same tool call. Make progress on every turn.\n\nWRITING NOTES:\n- When the user asks you to write, create, draft, add, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, or change the note — including short requests like 'format this', 'clean this up', 'make it nicer', 'fix the formatting' — IMMEDIATELY call write_note (or append_note to extend existing content) with the COMPLETE, finished text. These requests always refer to the OPEN note; never reply that you lack a tool for this. Do not ask what to include — just produce the full updated note in Markdown.\n- write_note, append_note and replace_text ALWAYS act on the note currently open in the editor. You do NOT need to read or search for it first. Pass the title shown in 'Open Note:' and the full content; one call is enough.\n- The content field must be the actual final text — never a description of what you did, and never a placeholder.\n- Use replace_text to change a specific snippet; use write_note with an empty string to clear the note.\n\nTOOLS (only when actually needed):\n- search_notes: ONLY to find OTHER notes by keyword when the user explicitly refers to them. Never to interpret a message or read the currently open note (its contents are already provided below).\n- fetch_web_page: only when the user gives a URL.\n- Greetings and small talk (\"hi\", \"gg\", \"thanks\"): reply briefly in chat with no tools.";

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
            "Write or replace the open note's content. Use this for ANY request to write, create, draft, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, or change the note — including reformatting what's already there. Provide the COMPLETE final note body in Markdown (pass an empty string to clear the note). To add to the end without rewriting, use append_note instead.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string", "description": "The complete final note body to save. Never use placeholders like [insert poem here]." },
                    "tags": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["title", "content"]
            }),
        ),
        spec(
            "replace_text",
            "Find exact text in the note and replace it with new text. Use an empty string for replacement_text to delete the target_text. Use this when the user asks to modify, replace, or remove a specific small portion of the note without rewriting everything.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "target_text": { "type": "string", "description": "The exact string of text to find and remove/replace. Must be an exact match of what is in the note." },
                    "replacement_text": { "type": "string", "description": "The new text to insert instead. Use an empty string to delete the target_text." }
                },
                "required": ["title", "target_text", "replacement_text"]
            }),
        ),
        spec(
            "append_note",
            "Append additional content to the end of an existing note. Use this when the user asks to add another paragraph, continue, extend, or append. Do NOT use this to clear or replace the note; use write_note for that.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string", "description": "Only the new content to append to the existing note. Do not repeat the whole note body and do not use placeholders." }
                },
                "required": ["title", "content"]
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
    title: String,
    content: String,
    tags: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AppendNoteArgs {
    title: String,
    content: String,
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
                "Write or replace the open note's content. Use this for ANY request to write, create, draft, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, or change the note — including reformatting what's already there. Provide the COMPLETE final note body in Markdown (pass an empty string to clear the note). To add to the end without rewriting, use append_note instead."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string", "description": "The complete final note body to save. Never use placeholders like [insert poem here]." },
                    "tags": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["title", "content"]
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

        if let Err(msg) = check_tool_approval(&self.state, "Write Note", &args.title, &args.content).await {
            return Ok(msg);
        }
        let display_name = if args.content.trim().is_empty() {
            "Clear Note"
        } else {
            "Write Note"
        };
        self.state
            .record_chat_tool(display_name, args.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", args.title, args.content) }),
        );
        if looks_like_placeholder(&args.content) {
            return Ok(
                "Refused to save because write_note received placeholder text instead of the full final content. Call write_note again with the actual poem body."
                    .to_string(),
            );
        }
        if looks_like_meta_status(&args.content) {
            return Ok(
                "Refused to save because write_note received a status/update sentence instead of the actual note body. Call write_note again with only the final poem/content that should appear in the note."
                    .to_string(),
            );
        }
        if let Some(existing) = self.state.resolve_chat_target_note(&args.title) {
            let tags = args.tags.unwrap_or(existing.tags.clone());
            self.state
                .save_note(
                    existing.id.clone(),
                    existing.title,
                    tags,
                    args.content.clone(),
                    existing.source_pdf,
                    Some(existing.annotations),
                )
                .await
                .map_err(|e| ToolError {
                    message: e.to_string(),
                })?;
            let _ = self.state.handle.emit(
                "ai://note_written",
                serde_json::json!({ "noteId": existing.id, "content": args.content, "mode": "write" }),
            );
            Ok(format!(
                "Note successfully updated with ID: {}",
                existing.id
            ))
        } else {
            Ok("Creating new notes from the sidebar chat is not allowed. I can only write to or append content on the currently open note.".to_string())
        }
    }
}

#[derive(Clone)]
pub struct AppendNoteTool {
    pub state: AppState,
}

impl Tool for AppendNoteTool {
    const NAME: &'static str = "append_note";

    type Error = ToolError;
    type Args = AppendNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "append_note".to_string(),
            description:
                "Append additional content to the end of an existing note. Use this when the user asks to add another paragraph, continue, extend, or append. Do NOT use this to clear or replace the note; use write_note for that."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string", "description": "Only the new content to append to the existing note. Do not repeat the whole note body and do not use placeholders." }
                },
                "required": ["title", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if !self.state.latest_chat_allows_note_mutation() {
            return Ok(
                "Refused to append because the latest user message did not explicitly ask to modify a note."
                    .to_string(),
            );
        }

        if let Err(msg) = check_tool_approval(&self.state, "Append Note", &args.title, &args.content).await {
            return Ok(msg);
        }
        self.state
            .record_chat_tool("Append Note", args.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Append Note", "details": format!("Title: {}\n\n{}", args.title, args.content) }),
        );
        if looks_like_placeholder(&args.content) {
            return Ok(
                "Refused to append because append_note received placeholder text instead of the final paragraph."
                    .to_string(),
            );
        }
        if looks_like_meta_status(&args.content) {
            return Ok(
                "Refused to append because append_note received a status/update sentence instead of the actual paragraph to append."
                    .to_string(),
            );
        }

        let existing = self
            .state
            .resolve_chat_target_note(&args.title)
            .ok_or_else(|| ToolError {
                message: "No note is currently open to append to.".to_string(),
            })?;

        let appended_body = if existing.body.trim().is_empty() {
            args.content.trim().to_string()
        } else {
            format!(
                "{}\n\n{}",
                existing.body.trim_end(),
                args.content.trim_start()
            )
        };

        self.state
            .save_note(
                existing.id.clone(),
                existing.title,
                existing.tags,
                appended_body,
                existing.source_pdf,
                Some(existing.annotations),
            )
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
        let _ = self.state.handle.emit(
            "ai://note_written",
            serde_json::json!({ "noteId": existing.id, "content": args.content, "mode": "append" }),
        );

        Ok(format!(
            "Content successfully appended to note with ID: {}",
            existing.id
        ))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReplaceTextArgs {
    title: String,
    target_text: String,
    replacement_text: String,
}

#[derive(Clone)]
pub struct ReplaceTextTool {
    pub state: AppState,
}

impl Tool for ReplaceTextTool {
    const NAME: &'static str = "replace_text";

    type Error = ToolError;
    type Args = ReplaceTextArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "replace_text".to_string(),
            description:
                "Find exact text in the note and replace it with new text. Use an empty string for replacement_text to delete the target_text. Use this when the user asks to modify, replace, or remove a specific small portion of the note without rewriting everything."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "target_text": { "type": "string", "description": "The exact string of text to find and remove/replace. Must be an exact match of what is in the note." },
                    "replacement_text": { "type": "string", "description": "The new text to insert instead. Use an empty string to delete the target_text." }
                },
                "required": ["title", "target_text", "replacement_text"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if !self.state.latest_chat_allows_note_mutation() {
            return Ok(
                "Refused to replace because the latest user message did not explicitly ask to modify a note."
                    .to_string(),
            );
        }

        if let Err(msg) = check_tool_approval(&self.state, "Replace Text", &args.title, &args.replacement_text).await {
            return Ok(msg);
        }

        let display_name = if args.replacement_text.trim().is_empty() {
            "Delete Text"
        } else {
            "Replace Text"
        };

        self.state
            .record_chat_tool(display_name, args.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\nTarget:\n{}\n\nReplacement:\n{}", args.title, args.target_text, args.replacement_text) }),
        );

        let existing = self
            .state
            .resolve_chat_target_note(&args.title)
            .ok_or_else(|| ToolError {
                message: "No note is currently open to edit.".to_string(),
            })?;

        if !existing.body.contains(&args.target_text) {
            return Ok("Could not find the target_text in the note. Make sure to quote the exact text you want to replace or delete.".to_string());
        }

        let new_body = existing.body.replacen(&args.target_text, &args.replacement_text, 1);

        self.state
            .save_note(
                existing.id.clone(),
                existing.title,
                existing.tags,
                new_body.clone(),
                existing.source_pdf,
                Some(existing.annotations),
            )
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;

        let _ = self.state.handle.emit(
            "ai://note_written",
            serde_json::json!({ "noteId": existing.id, "content": new_body, "mode": "write" }),
        );

        Ok(format!(
            "Text successfully replaced in note with ID: {}",
            existing.id
        ))
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
        .tool(ReplaceTextTool {
            state: state.clone(),
        })
        .tool(AppendNoteTool {
            state: state.clone(),
        })
        .tool(FetchWebPageTool {
            state: state.clone(),
        })
        .tool(SearchNotesTool { state })
        .build()
}

fn normalize_web_url(raw: &str) -> Result<String, String> {
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

fn html_to_text(raw: &str) -> String {
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
