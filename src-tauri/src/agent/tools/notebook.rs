use super::*;
use crate::state::AppState;
use futures_util::StreamExt;
use rig_core::client::CompletionClient;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;

pub(crate) fn edit_notebook_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "operation": { "type": "string", "enum": ["edit", "insert", "delete"], "description": "edit: replace cell `index`'s source with `content`. insert: add a new cell BEFORE `index`. delete: remove cell `index`." },
            "index": { "type": "integer", "description": "0-based cell index, as shown in the notebook listing." },
            "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "For insert only: the kind of cell to add (code = Python, markdown = Markdown)." },
            "content": { "type": "string", "description": "The cell's source text (for edit/insert). Markdown cells use Markdown; code cells use Python." }
        },
        "required": ["operation", "index"]
    })
}

#[derive(Deserialize, JsonSchema)]
pub struct EditNotebookArgs {
    pub operation: String,
    /// Required: a missing index must never silently become cell 0.
    pub index: usize,
    #[serde(default)]
    pub cell_type: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Clone)]
pub struct EditNotebookTool {
    pub state: AppState,
}

impl Tool for EditNotebookTool {
    const NAME: &'static str = "edit_notebook";

    type Error = ToolError;
    type Args = EditNotebookArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("edit_notebook").expect("edit_notebook contract");
        ToolDefinition {
            name: "edit_notebook".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => return Ok("No notebook is currently open to edit.".to_string()),
        };
        let op_name = args.operation.trim();
        // A missing content must not silently empty a cell (or insert an empty
        // one). Refuse instead of guessing.
        if matches!(op_name, "edit" | "insert")
            && args.content.as_deref().map(str::trim).unwrap_or("").is_empty()
        {
            return Ok(
                "Refused: `content` is required for edit/insert operations — a missing content would erase the cell. Provide the cell's source text and retry."
                    .to_string(),
            );
        }
        let op = crate::notebook::NotebookOp {
            operation: op_name,
            index: args.index,
            cell_type: args.cell_type.as_deref().unwrap_or("code"),
            content: args.content.as_deref().unwrap_or(""),
        };
        let armed_target = self.state.current_selection();
        let new_body_result = if let Some(selection) =
            armed_target.as_ref().filter(|selection| selection.cell_index.is_some())
        {
            crate::notebook::apply_targeted(
                &existing.body,
                &op,
                &crate::notebook::CellTarget {
                    index: selection.cell_index.unwrap_or_default(),
                    text: &selection.text,
                    before: &selection.before,
                    after: &selection.after,
                    cursor: selection.cursor,
                },
            )
        } else {
            crate::notebook::apply(&existing.body, &op)
        };
        let new_body = match new_body_result {
            Ok(b) => b,
            Err(msg) => return Ok(msg),
        };
        let display_name = match op.operation {
            "insert" => "Add Cell",
            "delete" => "Delete Cell",
            _ => "Edit Cell",
        };
        if let Err(msg) =
            check_tool_approval(&self.state, display_name, &existing.title, &new_body).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Cell {} · {}", args.index, existing.title), "mutatesNote": true }),
        );
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
            "Notebook updated (cell {} {}).",
            args.index, op.operation
        ))
    }
}
