use crate::state::AppState;
use rig_core::client::CompletionClient;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tauri::Emitter;

const WEB_FETCH_LIMIT: usize = 12_000;

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
        let note = self
            .state
            .load_note(args.note_id)
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
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
                "Create a note if it does not exist, or fully replace the existing note when the title exactly matches an existing note title. Do not use this to add another paragraph or continue existing content; use append_note for that. The content must be the full final note body, never placeholder text."
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
        self.state
            .record_chat_tool("Write Note", args.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Write Note", "details": args.title }),
        );
        if !self.state.latest_chat_allows_note_mutation() {
            return Ok(
                "Refused to write because the latest user message did not explicitly ask to modify a note."
                    .to_string(),
            );
        }
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
        if let Some(existing) = self.state.find_note_by_exact_title(&args.title) {
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
                "Append additional content to the end of an existing note whose title exactly matches the given title. Use this when the user asks to add another paragraph, continue, extend, or append to an existing note."
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
        self.state
            .record_chat_tool("Append Note", args.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Append Note", "details": args.title }),
        );
        if !self.state.latest_chat_allows_note_mutation() {
            return Ok(
                "Refused to append because the latest user message did not explicitly ask to modify a note."
                    .to_string(),
            );
        }
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
            .find_note_by_exact_title(&args.title)
            .ok_or_else(|| ToolError {
                message: format!("No existing note found with exact title: {}", args.title),
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
            description: "Search the workspace for notes containing specific keywords.".to_string(),
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
        .default_max_turns(4)
        .tool(ReadNoteTool {
            state: state.clone(),
        })
        .tool(WriteNoteTool {
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
