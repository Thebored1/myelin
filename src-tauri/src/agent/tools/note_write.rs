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
#[derive(Deserialize, JsonSchema)]
pub struct WriteNoteArgs {
    /// The full new note body. Empty string clears the note.
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
        let (_, description, params) = tool_contract("write_note").expect("write_note contract");
        ToolDefinition {
            name: "write_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if note_content_has_protocol_residue(&args.content) {
            return Err(ToolError {
                message: "Generation mixed tool protocol text into the note. Live preview reverted; no changes were saved."
                    .to_string(),
            });
        }
        let content = clean_note_content(&strip_prompt_markers(&args.content));

        if self.state.targeted_write_active()
            && content.trim().is_empty()
            && !wants_targeted_deletion(&self.state.latest_chat_question())
        {
            return Err(ToolError {
                message: "Empty targeted content is allowed only for an explicit deletion request; no changes were made."
                    .to_string(),
            });
        }

        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to write to. Creating new notes from the sidebar chat is not allowed.".to_string());
            }
        };

        if !content.trim().is_empty()
            && !content.chars().any(char::is_alphanumeric)
            && !wants_clear(&self.state.latest_chat_question())
        {
            return Ok(
                "Refused: content contained only punctuation, not the requested text. Retry with the complete meaningful Markdown.".to_string(),
            );
        }

        // Selection-scoped edit: if user armed a selection, splice content into
        // the selected span instead of replacing the whole body.
        let armed_selection = self.state.current_selection();
        let scoped = match armed_selection.as_ref() {
            Some(sel) => {
                let plan = selection_scoped_plan(&existing.body, &content, sel).ok_or_else(|| {
                    log::warn!(
                        "write_note target mismatch: cursor={} body_bytes={} body_trim_bytes={} text_bytes={} before_bytes={} before_trim_bytes={} after_bytes={} after_trim_bytes={}",
                        sel.cursor,
                        existing.body.len(),
                        existing.body.trim().len(),
                        sel.text.len(),
                        sel.before.len(),
                        sel.before.trim().len(),
                        sel.after.len(),
                        sel.after.trim().len(),
                    );
                    ToolError {
                        message: "The armed selection could not be located or the model returned a full-note rewrite; no changes were made.".to_string(),
                    }
                })?;
                Some(plan)
            }
            None => None,
        };

        let new_body = if let Some(ref p) = scoped {
            p.new_body.clone()
        } else {
            content.clone()
        };

        if self.state.deterministic_tools_enabled()
            && new_body.trim().is_empty()
            && !existing.body.trim().is_empty()
            && !wants_clear(&self.state.latest_chat_question())
        {
            return Ok(
                "Refused: that would erase the entire note, which the request did not ask for. \
                 Keep ALL existing content and call write_note again with only the requested change \
                 applied.".to_string(),
            );
        }

        let display_name = if scoped.is_some() { "Replace Text" } else if new_body.trim().is_empty() { "Clear Note" } else { "Write Note" };
        if let Err(msg) =
            check_tool_approval(&self.state, display_name, &existing.title, &content).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", existing.title, content), "mutatesNote": true }),
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
            "Note successfully updated with ID: {}",
            existing.id
        ))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct AppendNoteArgs {
    /// Only the new text to append. Never include any existing note content.
    content: String,
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
        let (_, description, params) = tool_contract("append_note").expect("append_note contract");
        ToolDefinition {
            name: "append_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to append to.".to_string());
            }
        };

        // Safety guard: strip echoed existing note content if the model
        // disregards the tool description, and drop the prompt-framing markers
        // (--- CURRENT NOTE --- etc.) that every other write tool strips.
        let content =
            clean_note_content(&normalize_append_content(&existing.body, &strip_prompt_markers(&args.content)));

        if content.trim().is_empty() {
            return Ok("Nothing to append — content was empty after normalization.".to_string());
        }

        let new_body = if existing.body.trim().is_empty() {
            content.clone()
        } else {
            format!("{}\n\n{content}", existing.body.trim_end())
        };

        let display_name = "Append Note";
        if let Err(msg) =
            check_tool_approval(&self.state, display_name, &existing.title, &content).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", existing.title, content), "mutatesNote": true }),
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
            "Note successfully updated with ID: {}",
            existing.id
        ))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct PrependNoteArgs {
    /// Only the new text to prepend. Never include any existing note content.
    content: String,
}

#[derive(Clone)]
pub struct PrependNoteTool {
    pub state: AppState,
}

impl Tool for PrependNoteTool {
    const NAME: &'static str = "prepend_note";

    type Error = ToolError;
    type Args = PrependNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("prepend_note").expect("prepend_note contract");
        ToolDefinition {
            name: "prepend_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to prepend to.".to_string());
            }
        };

        let content = clean_note_content(&strip_prompt_markers(&args.content));
        if content.trim().is_empty() {
            return Ok("Nothing to prepend — content was empty.".to_string());
        }

        let new_body = if existing.body.trim().is_empty() {
            content.clone()
        } else {
            format!("{content}\n\n{}", existing.body.trim_start())
        };

        let display_name = "Prepend Note";
        if let Err(msg) =
            check_tool_approval(&self.state, display_name, &existing.title, &content).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", existing.title, content), "mutatesNote": true }),
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
            "Note successfully updated with ID: {}",
            existing.id
        ))
    }
}

