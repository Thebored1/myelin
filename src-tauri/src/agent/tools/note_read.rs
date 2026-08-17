use super::*;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use tauri::Emitter;
#[derive(Clone)]
pub struct ReadNoteTool {
    pub turn: ToolTurnContext,
}

impl Tool for ReadNoteTool {
    const NAME: &'static str = "read_note";

    type Error = ToolError;
    type Args = ReadNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("read_note").expect("read_note contract");
        ToolDefinition {
            name: "read_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // A small model can occasionally ask to read the note already present in
        // its prompt. Intercept that mistake before resolving any other note.
        if !self.turn.note_id.is_empty() {
            let open_id = self.turn.note_id.clone();
            if let Ok(open_note) = self.turn.state.load_note(open_id.clone()).await {
                let normalize = |value: &str| {
                    value
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_lowercase()
                };
                if args.note_id == open_id
                    || normalize(&args.note_id) == normalize(&open_note.title)
                {
                    return Ok(format!(
                        "ALREADY-OPEN NOTE — its current body was already supplied in the prompt. \
                         Answer the user directly without another lookup.\n\n{}",
                        open_note.body
                    ));
                }
            }
        }
        self.turn.record_tool("Read Note", args.note_id.clone());
        let _ = self.turn.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Read Note", "details": args.note_id }),
        );
        let note = match self.turn.state.load_note(args.note_id.clone()).await {
            Ok(n) => n,
            Err(_) => {
                // Fallback: try finding by exact title
                self.turn.state.find_note_by_exact_title(&args.note_id)
                    .ok_or_else(|| ToolError {
                        message: format!("Note '{}' not found. You may have used the title instead of the ID. Use search_notes to find the correct ID.", args.note_id),
                    })?
            }
        };
        Ok(note.body)
    }
}
