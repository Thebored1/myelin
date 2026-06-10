use crate::state::AppState;
use rig_core::tool::Tool;
use rig_core::completion::ToolDefinition;
use rig_core::client::CompletionClient;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tauri::Emitter;

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
        let _ = self.state.handle.emit("ai://chat_tool", serde_json::json!({ "tool": "Read Note", "details": args.note_id }));
        let note = self.state.load_note(args.note_id).await.map_err(|e| ToolError { message: e.to_string() })?;
        Ok(note.body)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteNoteArgs {
    title: String,
    content: String,
    tags: Option<Vec<String>>,
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
            description: "Create a new note or overwrite an existing one with the given title and content.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["title", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let _ = self.state.handle.emit("ai://chat_tool", serde_json::json!({ "tool": "Write Note", "details": args.title }));
        let mut note = self.state.create_note(args.title, None).await.map_err(|e| ToolError { message: e.to_string() })?;
        if let Some(tags) = args.tags {
            note.tags = tags;
        }
        self.state.save_note(note.id.clone(), note.title, note.tags, args.content, None, None).await.map_err(|e| ToolError { message: e.to_string() })?;
        Ok(format!("Note successfully written with ID: {}", note.id))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchNotesArgs {
    query: String,
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
        let _ = self.state.handle.emit("ai://chat_tool", serde_json::json!({ "tool": "Search Notes", "details": args.query }));
        let results = self.state.search_notes(args.query).await.map_err(|e| ToolError { message: e.to_string() })?;
        let mut output = String::new();
        for r in results.results.into_iter().take(5) {
            output.push_str(&format!("ID: {} | Title: {}\nSnippet: {}\n\n", r.note.id, r.note.title, r.note.excerpt));
        }
        if output.is_empty() {
            Ok("No results found.".to_string())
        } else {
            Ok(output)
        }
    }
}

pub fn build_myelin_agent(state: AppState, base_url: &str, model_name: &str, preamble: &str) -> rig_core::agent::Agent<impl rig_core::completion::CompletionModel> {
    let client = rig_core::providers::openai::Client::builder()
        .api_key("sk-fake")
        .base_url(base_url)
        .build()
        .expect("Failed to initialize rig client");
    let model = client.completion_model(model_name);
    rig_core::agent::AgentBuilder::new(model)
        .preamble(preamble)
        .tool(ReadNoteTool { state: state.clone() })
        .tool(WriteNoteTool { state: state.clone() })
        .tool(SearchNotesTool { state })
        .build()
}
