pub(crate) use anyhow::{Context, Result};
pub(crate) use std::fs;
pub(crate) use std::path::Path;

// GTE-small width. Notes use real embeddings when an embed model is

use super::latex_support::*;
use super::types::*;

pub(crate) fn load_settings(app_data_dir: &Path) -> Result<PersistedSettings> {
    let settings_path = app_data_dir.join(SETTINGS_FILE_NAME);
    if !settings_path.exists() {
        return Ok(PersistedSettings::default());
    }

    let raw = fs::read_to_string(&settings_path)
        .with_context(|| format!("failed to read settings at {}", settings_path.display()))?;
    Ok(serde_json::from_str(&raw).context("failed to parse settings")?)
}

pub(crate) fn save_settings(app_data_dir: &Path, settings: &PersistedSettings) -> Result<()> {
    let settings_path = app_data_dir.join(SETTINGS_FILE_NAME);
    crate::persistence::atomic_write_json(&settings_path, settings)
        .with_context(|| format!("failed to write settings at {}", settings_path.display()))
}

/// Seed a live conversation from the frontend's saved chat history on the first
/// turn of a session. Only text turns survive (tool results were never persisted),
/// but it keeps continuity after an app restart instead of starting blank.
pub(crate) fn chat_history_to_messages(
    chat_history: &[crate::models::ChatMessage],
) -> Vec<serde_json::Value> {
    chat_history
        .iter()
        .filter(|m| m.error != Some(true) && m.is_streaming != Some(true))
        .filter(|m| !m.content.trim().is_empty())
        .filter(|m| m.role == "user" || m.role == "assistant")
        .rev()
        .take(MAX_CHAT_HISTORY_MESSAGES_IN_PROMPT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect()
}

/// The system prompt and generation primers are rebuilt for every request and
/// must never enter the retained system-less conversation. Keeping them would
/// change the fixed prefix and recursively inflate subsequent prompts. User
/// messages are preserved EXACTLY as rendered on the wire (including any
/// TURN-SPECIFIC/OPEN-NOTE wrapper) so the next request reproduces the
/// byte-identical prefix and llama-server reuses the KV cache; only system
/// rows and the sidecar's empty `<think>` placeholders are dropped.
pub(crate) fn canonical_wire_conversation(
    messages: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    messages
        .into_iter()
        .map(|mut message| {
            if message["role"].as_str() == Some("user") {
                if let Some(object) = message.as_object_mut() {
                    object.remove("metadata");
                }
            }
            message
        })
        .filter(|message| message["role"].as_str() != Some("system"))
        .filter(|message| {
            if message["role"].as_str() != Some("assistant") {
                return true;
            }
            let content = message["content"].as_str().unwrap_or("").trim();
            !matches!(
                content,
                "<think></think>"
                    | "<think></think><think></think>"
                    | "<think>\n</think>"
                    | "<think>\n</think><think>\n</think>"
            )
        })
        .collect()
}

pub(crate) fn assemble_note_context(
    title: &str,
    body_excerpt: &str,
    notebook_cells: Option<&str>,
) -> String {
    let mut context = format!("The note currently open is titled \"{title}\".");
    if let Some(cells) = notebook_cells {
        context.push_str(&format!("\n\n{cells}"));
    } else if body_excerpt.trim().is_empty() {
        context.push_str("\n\nThe note's CURRENT content is empty.");
    } else {
        context.push_str(&format!(
            "\n\nHere is the note's CURRENT content. When the user asks you to edit, change, format, fix, clean up, rewrite, shorten, expand, reorder, or remove part of the note, treat this as the text to modify — reproduce the parts that stay, apply the change, and pass the full result to write_note. (When you are only answering a question, use it as reference and do not echo it back verbatim.)\n--- CURRENT NOTE ---\n{body_excerpt}\n--- END CURRENT NOTE ---"
        ));
    }
    context
}

/// Stable system context for focused Write. The generic note context is
/// intentionally more permissive for legacy operation turns and tells a model
/// to regenerate a full note; doing that inside the targeted profile would
/// contradict the cursor/selection contract and make a valid selection look
/// like an unsafe whole-note rewrite.
pub(crate) fn assemble_targeted_write_context(
    title: &str,
    body_excerpt: &str,
    notebook_cells: Option<&str>,
) -> String {
    let mut context = format!("The note currently open is titled \"{title}\".");
    if let Some(cells) = notebook_cells {
        context.push_str(&format!(
            "\n\n{cells}\n\nUse these cells only as reference. Preserve notebook syntax and return only the replacement source for the armed cell."
        ));
    } else if body_excerpt.trim().is_empty() {
        context.push_str(
            "\n\nThe note's CURRENT content is empty. Generate only the requested insertion fragment.",
        );
    } else {
        context.push_str(&format!(
            "\n\nHere is the note's CURRENT content as reference material. Generate only the requested insertion or replacement fragment for the armed cursor or text selection; never reproduce surrounding content or the full note.\n--- CURRENT NOTE REFERENCE ---\n{body_excerpt}\n--- END CURRENT NOTE REFERENCE ---"
        ));
    }
    context
}

/// Shared, mode-neutral context for a viewer section. Chat and Write must see
/// the exact same bytes before their profile-specific system message; this is
/// the only part that is persisted in the shared section KV snapshot.
pub(crate) fn assemble_section_context(
    title: &str,
    section_excerpt: &str,
    notebook_cells: Option<&str>,
) -> String {
    let mut context = format!("The note currently open is titled \"{title}\".");
    if let Some(cells) = notebook_cells {
        context.push_str("\n\n");
        context.push_str(cells);
    } else if section_excerpt.trim().is_empty() {
        context.push_str("\n\nThe active source section is empty.");
    } else {
        context.push_str(
            "\n\nThe active source section is included below as reference material. Preserve its document syntax when answering or writing.\n--- ACTIVE SOURCE SECTION ---\n",
        );
        context.push_str(section_excerpt);
        context.push_str("\n--- END ACTIVE SOURCE SECTION ---");
    }
    context
}

/// Maple/TQ2's template does not reliably honor llama.cpp's reasoning switch
/// or `enable_thinking:false` for tool-bearing requests. Its in-prompt control
/// is `/no_think`. Keep the directive in the stable system prefix whenever
/// thinking is explicitly disabled, so live requests and saved section KV
/// snapshots remain byte-compatible.
pub(crate) fn add_no_think_directive(system: &str, enabled: bool) -> String {
    if enabled && !system.trim_start().starts_with("/no_think") {
        format!("/no_think\n{system}")
    } else {
        system.to_string()
    }
}

#[cfg(test)]
pub(crate) fn assemble_user_content(
    note_title: &str,
    mode_instruction: &str,
    turn_instructions: &str,
    question: &str,
) -> String {
    format!(
        "OPEN NOTE TITLE: {note_title:?}\n\n\
         INTERNAL TURN POLICY (not note metadata):\n{mode_instruction}\n\n\
         {turn_instructions}\n\n\
         USER REQUEST:\n{question}"
    )
}

pub(crate) fn is_note_mutation_tool(name: &str) -> bool {
    matches!(
        name,
        "write_note"
            | "append_note"
            | "prepend_note"
            | "replace_in_note"
            | "insert_after_line"
            | "delete_in_note"
            | "format_note"
            | "edit_notebook"
    )
}

pub(crate) fn authorize_tool_policy(
    name: &str,
    chat_mode: bool,
    append_only: bool,
    placement_edit: bool,
    has_selection: bool,
    is_notebook: bool,
    oversized_doc: bool,
    tools_supported: bool,
) -> Result<(), String> {
    if !tools_supported {
        return Err("Tool execution is disabled for this model.".to_string());
    }
    if !is_note_mutation_tool(name) {
        return Ok(());
    }
    if chat_mode {
        return Err("Note mutations are disabled in Chat mode.".to_string());
    }
    if oversized_doc && matches!(name, "write_note" | "format_note") {
        return Err(
            "This note exceeds the model context; retrieve the relevant region with \
             search_documents, then use replace_in_note, insert_after_line, delete_in_note, \
             append_note, or prepend_note."
                .to_string(),
        );
    }
    if is_notebook && name != "edit_notebook" {
        return Err("Notebook documents may only be mutated with edit_notebook.".to_string());
    }
    if !is_notebook && name == "edit_notebook" {
        return Err("edit_notebook is only available for notebook documents.".to_string());
    }
    if has_selection {
        let allowed = if is_notebook {
            name == "edit_notebook"
        } else if placement_edit {
            name == "insert_after_line"
        } else {
            name == "write_note"
        };
        if !allowed {
            return Err("This mutation is outside the armed selection.".to_string());
        }
    } else if append_only && name != "append_note" {
        return Err("This is an append-only turn; only append_note is permitted.".to_string());
    }
    Ok(())
}

pub(crate) fn turn_contains_note_mutation(messages: &[serde_json::Value]) -> bool {
    let mutation_ids: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .filter(|call| {
            call["function"]["name"]
                .as_str()
                .is_some_and(is_note_mutation_tool)
        })
        .filter_map(|call| call["id"].as_str())
        .collect();
    messages.iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"]
                .as_str()
                .is_some_and(|id| mutation_ids.contains(id))
            && message["content"].as_str().is_some_and(|content| {
                content.starts_with("Note successfully updated with ID:")
                    || content.starts_with("Notebook updated (cell ")
            })
    })
}

/// Keep the most recent whole turns of a live conversation under a rough char
/// budget. A "turn" starts at a `user` message and includes the assistant/tool
/// messages that follow it, so trimming never orphans a tool result from its
/// assistant tool_call (which llama-server would reject).
pub(crate) fn trim_conversation(
    msgs: Vec<serde_json::Value>,
    max_chars: usize,
) -> Vec<serde_json::Value> {
    let mut groups: Vec<Vec<serde_json::Value>> = Vec::new();
    for m in msgs {
        if m["role"] == "user" || groups.is_empty() {
            groups.push(vec![m]);
        } else {
            groups.last_mut().unwrap().push(m);
        }
    }
    let cost = |m: &serde_json::Value| -> usize {
        let c = m["content"].as_str().map(|s| s.len()).unwrap_or(0);
        let a = m["tool_calls"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|t| {
                        t["function"]["arguments"]
                            .as_str()
                            .map(|s| s.len())
                            .unwrap_or(0)
                    })
                    .sum::<usize>()
            })
            .unwrap_or(0);
        c + a
    };
    let mut kept: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut total = 0usize;
    for g in groups.into_iter().rev() {
        let g_cost: usize = g.iter().map(&cost).sum();
        if !kept.is_empty() && total + g_cost > max_chars {
            break;
        }
        total += g_cost;
        kept.push(g);
    }
    kept.reverse();
    kept.into_iter().flatten().collect()
}

pub(crate) fn is_simple_greeting(question: &str) -> bool {
    let normalized = question
        .trim()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric())
        .to_ascii_lowercase();

    matches!(
        normalized.as_str(),
        "hi" | "hello" | "hey" | "yo" | "sup" | "hiya" | "howdy"
    )
}
