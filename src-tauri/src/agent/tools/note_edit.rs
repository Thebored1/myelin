use super::*;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::Deserialize;
use tauri::Emitter;
#[derive(Deserialize, JsonSchema)]
pub struct ReplaceInNoteArgs {
    /// The exact existing text in the note to find and replace.
    find: String,
    /// The new text to put in its place. Empty string deletes the matched text.
    replace: String,
}

#[derive(Clone)]
pub struct ReplaceInNoteTool {
    pub turn: ToolTurnContext,
}

impl Tool for ReplaceInNoteTool {
    const NAME: &'static str = "replace_in_note";

    type Error = ToolError;
    type Args = ReplaceInNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) =
            tool_contract("replace_in_note").expect("replace_in_note contract");
        ToolDefinition {
            name: "replace_in_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let existing = match self.turn.resolve_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to edit.".to_string());
            }
        };

        let replacement = clean_note_content(&strip_prompt_markers(&args.replace));

        match find_tolerant(&existing.body, &args.find) {
            Some((start, end)) => {
                let new_body = format!(
                    "{}{}{}",
                    &existing.body[..start],
                    replacement,
                    &existing.body[end..]
                );

                let display_name = if replacement.trim().is_empty() {
                    "Delete Text"
                } else {
                    "Replace Text"
                };
                let preview = format!("Find:\n{}\n\nReplace with:\n{replacement}", args.find);
                if let Err(msg) =
                    check_tool_approval(&self.turn, display_name, &existing.title, &preview).await
                {
                    return Ok(msg);
                }
                self.turn.record_tool(display_name, existing.title.clone());
                let _ = self.turn.state.handle.emit(
                    "ai://chat_tool",
                    serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }),
                );

                self.turn.state
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
                let _ = self.turn.state.handle.emit(
                    "ai://note_written",
                    serde_json::json!({ "noteId": existing.id, "content": new_body, "mode": "write" }),
                );
                Ok(format!(
                    "Note successfully updated with ID: {}",
                    existing.id
                ))
            }
            None => Err(ToolError {
                message: format!("Could not find '{}' in the note.", args.find),
            }),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct InsertAfterLineArgs {
    /// Text in an existing line to insert after.
    marker: String,
    /// The new text to insert after the matching line.
    content: String,
}

#[derive(Clone)]
pub struct InsertAfterLineTool {
    pub turn: ToolTurnContext,
}

impl Tool for InsertAfterLineTool {
    const NAME: &'static str = "insert_after_line";

    type Error = ToolError;
    type Args = InsertAfterLineArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) =
            tool_contract("insert_after_line").expect("insert_after_line contract");
        ToolDefinition {
            name: "insert_after_line".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let existing = match self.turn.resolve_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to edit.".to_string());
            }
        };

        let content = clean_note_content(&strip_prompt_markers(&args.content));
        let body = &existing.body;

        // An armed selection is authoritative: placement may not target an
        // unrelated model-supplied marker.
        if let Some(sel) = self.turn.selection.clone() {
            let plan = selection_insert_after_plan(body, &args.marker, &content, &sel)
                .map_err(|message| ToolError { message })?;
            let display_name = "Insert After";
            let preview = format!("Insert after '{}':\n\n{content}", args.marker);
            if let Err(msg) =
                check_tool_approval(&self.turn, display_name, &existing.title, &preview).await
            {
                return Ok(msg);
            }
            self.turn.record_tool(display_name, existing.title.clone());
            let _ = self.turn.state.handle.emit("ai://chat_tool", serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }));
            self.turn.state
                .save_note(
                    existing.id.clone(),
                    existing.title,
                    existing.tags,
                    plan.new_body.clone(),
                    existing.source_pdf,
                    Some(existing.annotations),
                )
                .await
                .map_err(|e| ToolError {
                    message: e.to_string(),
                })?;
            let _ = self.turn.state.handle.emit("ai://note_written", serde_json::json!({ "noteId": existing.id, "content": plan.new_body, "mode": "write" }));
            return Ok(format!(
                "Note successfully updated with ID: {}",
                existing.id
            ));
        }

        // Find the marker text in the note body.
        let pos = body.find(&args.marker).ok_or_else(|| ToolError {
            message: format!("Could not find '{}' in the note.", args.marker),
        })?;

        // Find the end of the line containing the marker.
        let line_end = body[pos..]
            .find('\n')
            .map(|i| pos + i + 1)
            .unwrap_or(body.len());

        let new_body = format!("{}{}\n\n{}", &body[..line_end], content, &body[line_end..]);

        let display_name = "Insert After";
        let preview = format!("Insert after '{}':\n\n{content}", args.marker);
        if let Err(msg) =
            check_tool_approval(&self.turn, display_name, &existing.title, &preview).await
        {
            return Ok(msg);
        }
        self.turn.record_tool(display_name, existing.title.clone());
        let _ = self.turn.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }),
        );

        self.turn.state
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
        let _ = self.turn.state.handle.emit(
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
pub struct DeleteInNoteArgs {
    /// The exact text in the note to delete.
    target: String,
}

#[derive(Clone)]
pub struct DeleteInNoteTool {
    pub turn: ToolTurnContext,
}

impl Tool for DeleteInNoteTool {
    const NAME: &'static str = "delete_in_note";

    type Error = ToolError;
    type Args = DeleteInNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) =
            tool_contract("delete_in_note").expect("delete_in_note contract");
        ToolDefinition {
            name: "delete_in_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let existing = match self.turn.resolve_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to edit.".to_string());
            }
        };

        match find_tolerant(&existing.body, &args.target) {
            Some((start, end)) => {
                let new_body = format!("{}{}", &existing.body[..start], &existing.body[end..]);

                if self.turn.policy.deterministic_tools
                    && new_body.trim().is_empty()
                    && !existing.body.trim().is_empty()
                    && !wants_clear(&self.turn.question)
                {
                    return Ok(
                        "Refused: that would erase the entire note. Use write_note with empty content if you intend to clear it.".to_string(),
                    );
                }

                let display_name = "Delete Text";
                let preview = format!("Delete:\n{}", args.target);
                if let Err(msg) =
                    check_tool_approval(&self.turn, display_name, &existing.title, &preview).await
                {
                    return Ok(msg);
                }
                self.turn.record_tool(display_name, existing.title.clone());
                let _ = self.turn.state.handle.emit(
                    "ai://chat_tool",
                    serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }),
                );

                self.turn.state
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
                let _ = self.turn.state.handle.emit(
                    "ai://note_written",
                    serde_json::json!({ "noteId": existing.id, "content": new_body, "mode": "write" }),
                );
                Ok(format!(
                    "Note successfully updated with ID: {}",
                    existing.id
                ))
            }
            None => Err(ToolError {
                message: format!("Could not find '{}' in the note.", args.target),
            }),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct FormatNoteArgs {
    /// Which structural cleanup to apply: remove_headings | remove_bold | remove_bullets.
    operation: String,
}

#[derive(Clone)]
pub struct FormatNoteTool {
    pub turn: ToolTurnContext,
}

impl Tool for FormatNoteTool {
    const NAME: &'static str = "format_note";

    type Error = ToolError;
    type Args = FormatNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("format_note").expect("format_note contract");
        ToolDefinition {
            name: "format_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Trust the model's operation only if it's a known op; otherwise fall
        // back to what the user's message clearly asked for. The transform
        // itself is deterministic either way. When neither yields a valid op,
        // refuse: falling back to strip_markdown would silently erase the note
        // on a misrouted request.
        let requested = args.operation.trim();
        let op = if is_format_op(requested) {
            requested.to_string()
        } else if let Some(detected) = detect_format_op(&self.turn.question) {
            detected.to_string()
        } else {
            return Ok(format!(
                "Refused: '{requested}' is not a supported format operation. Supported operations: {}.",
                FORMAT_OPS.join(", ")
            ));
        };

        let existing = match self.turn.resolve_target_note("") {
            Some(n) => n,
            None => return Ok("No note is currently open to format.".to_string()),
        };
        let new_body = apply_format_op(&existing.body, &op);
        let pretty = op.replace('_', " ");
        if new_body == existing.body {
            return Ok(format!(
                "Nothing to change — no matching content to {pretty} in the note."
            ));
        }

        let display_name = "Format Note";
        if let Err(msg) =
            check_tool_approval(&self.turn, display_name, &existing.title, &new_body).await
        {
            return Ok(msg);
        }
        self.turn.record_tool(display_name, existing.title.clone());
        let _ = self.turn.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", existing.title, pretty), "mutatesNote": true }),
        );
        self.turn.state
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
        let _ = self.turn.state.handle.emit(
            "ai://note_written",
            serde_json::json!({ "noteId": existing.id, "content": new_body, "mode": "write" }),
        );
        Ok(format!(
            "Note successfully updated with ID: {}",
            existing.id
        ))
    }
}
