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
type ToolParams = fn() -> Value;

pub(super) const TOOL_CONTRACTS: &[(&str, &str, ToolParams)] = &[
    (
        "read_note",
        "Read the full Markdown of ANOTHER note by its id (ids come from search_notes). Do NOT use this for the note currently open in the editor — that note's content is already provided in the prompt below.",
        read_note_params,
    ),
    (
        "write_note",
        "Replace the ENTIRE body of the note currently OPEN in the editor with `content`. Empty string clears the note. Use ONLY when the user asks to write, create, draft, generate, rewrite, or replace the whole note. DO NOT use this for additions, insertions, or targeted edits — use append_note, prepend_note, replace_in_note, insert_after_line, or delete_in_note instead. Never put a placeholder like [insert poem here] in content; write the real final Markdown.",
        write_note_params,
    ),
    (
        "append_note",
        "Add a new paragraph or block of text to the END of the note currently OPEN. `content` must contain ONLY the NEW text to add — never reproduce or quote any existing lines from the note. Use this when the user asks to add, append, extend, continue, or elaborate.",
        append_note_params,
    ),
    (
        "prepend_note",
        "Add a new paragraph or block of text to the BEGINNING of the note currently OPEN. `content` must contain ONLY the NEW text to add — never reproduce or quote any existing lines.",
        prepend_note_params,
    ),
    (
        "replace_in_note",
        "Replace a specific piece of existing text in the note with new text. Finds the exact `find` text and replaces it with `replace`. Empty `replace` deletes the matched text. Use this for surgical edits like fixing a word, swapping a phrase, or removing a specific sentence. DO NOT use this for whole-note operations.",
        replace_in_note_params,
    ),
    (
        "insert_after_line",
        "Insert a new block of text in the note AFTER a line containing `marker`. The marker should be a unique heading, phrase, or sentence already in the note. The new content is added on a new line right after the matching line. Use this when the user asks to insert, add between sections, or place text after a specific part.",
        insert_after_line_params,
    ),
    (
        "delete_in_note",
        "Delete specific text from the note. The `target` is the exact text to remove — it can be a heading, line, sentence, or phrase. The matching text is removed from the note. Use this when the user asks to delete, remove, erase, or drop a specific part of the note. Do NOT use this for whole-note clearing.",
        delete_in_note_params,
    ),
    (
        "format_note",
        "Apply a structural Markdown transform to the OPEN note, performed exactly in code (not by you): remove headings/bold/italic/bullets/numbering/links/images/code/quotes/strikethrough/dividers/blank lines, strip ALL formatting to plain text, convert headings<->bold, promote/demote headings, convert between bulleted and numbered lists, or change case. ALWAYS prefer this over write_note when the user asks to remove, strip, or convert any of these — it is reliable where a full rewrite is not.",
        format_note_params,
    ),
    (
        "fetch_web_page",
        "Fetch the text content of a public web page. Use this when the user asks to visit, open, fetch, or get details from a URL or domain.",
        fetch_web_page_params,
    ),
    (
        "web_search",
        "Search the web for current information when the user asks you to look something up, search online, or find recent info and you have NO URL. Returns a ranked list of {title, url, snippet}. After searching, call fetch_web_page on the most relevant result to read it in full. Do NOT use this when the user already gave a URL — fetch that directly.",
        web_search_params,
    ),
    (
        "search_documents",
        "Search the user's ingested source documents (PDFs, books, web pages, etc.) for passages relevant to a query, and get the most relevant excerpts with their source. Use this when the user asks about their documents, sources, a PDF, a book, or a paper — NOT for the note open in the editor (that text is already in the prompt).",
        search_documents_params,
    ),
    (
        "find_in_note",
        "Check whether an exact word or phrase appears in the note currently open in the editor, and how many times. Use this whenever the user asks if the note contains a word, or to find/locate a specific word in the note — it searches the exact text reliably instead of you scanning by eye.",
        find_in_note_params,
    ),
    (
        "search_notes",
        "Search the ENTIRE workspace for OTHER notes containing specific keywords. Do NOT use this to search or modify the currently open note.",
        search_notes_params,
    ),
    (
        "edit_notebook",
        "Edit the OPEN Jupyter notebook (.ipynb) one cell at a time. operation \"edit\" replaces cell `index`'s source with `content`; \"insert\" adds a new `cell_type` cell BEFORE `index`; \"delete\" removes cell `index`. Cells are 0-indexed as shown in the notebook listing. Use this for notebooks INSTEAD of write_note.",
        edit_notebook_params,
    ),
];

/// Look up a tool's canonical (description, parameters) pair.
pub(super) fn tool_contract(name: &str) -> Option<(&'static str, &'static str, ToolParams)> {
    TOOL_CONTRACTS
        .iter()
        .copied()
        .find(|(n, _, _)| *n == name)
}

/// The full schema list served to the wire path (warm-up request, tool gates,
/// sidecar). Same source as each `Tool::definition` below.
pub fn tool_specs() -> Vec<Value> {
    let spec = |name: &str, description: &str, parameters: Value| {
        serde_json::json!({
            "type": "function",
            "function": { "name": name, "description": description, "parameters": parameters }
        })
    };
    TOOL_CONTRACTS
        .iter()
        .map(|(name, description, params)| spec(name, description, params()))
        .collect()
}

fn read_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": { "note_id": { "type": "string", "description": "The id of a DIFFERENT note to read (from search_notes results), not the open note." } },
        "required": ["note_id"]
    })
}

fn write_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "content": { "type": "string", "description": "The full new note body. Empty string clears the note. Never a placeholder — write the real content." }
        },
        "required": ["content"]
    })
}

fn append_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "content": { "type": "string", "description": "Only the new text to append. Never include any existing note content." }
        },
        "required": ["content"]
    })
}

fn prepend_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "content": { "type": "string", "description": "Only the new text to prepend. Never include any existing note content." }
        },
        "required": ["content"]
    })
}

fn replace_in_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "find": { "type": "string", "description": "The exact existing text in the note to find and replace." },
            "replace": { "type": "string", "description": "The new text to put in its place. Empty string deletes the matched text." }
        },
        "required": ["find", "replace"]
    })
}

fn insert_after_line_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "marker": { "type": "string", "description": "Text in an existing line to insert after. Should be unique enough to identify the location." },
            "content": { "type": "string", "description": "The new text to insert after the matching line." }
        },
        "required": ["marker", "content"]
    })
}

fn delete_in_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "target": { "type": "string", "description": "The exact text in the note to delete." }
        },
        "required": ["target"]
    })
}

fn format_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": { "operation": { "type": "string", "enum": FORMAT_OPS, "description": "Which structural transform to apply to the open note." } },
        "required": ["operation"]
    })
}

fn fetch_web_page_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": { "url": { "type": "string", "description": "The http(s) URL or domain to fetch." } },
        "required": ["url"]
    })
}

fn web_search_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "The search query." },
            "count": { "type": "integer", "description": "How many results to return (default 5, max 10)." }
        },
        "required": ["query"]
    })
}

fn search_documents_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "What to look for in the documents." },
            "count": { "type": "integer", "description": "How many passages to return (default 5, max 10)." },
            "doc_id": { "type": "string", "description": "Optional document or note ID to search within." }
        },
        "required": ["query"]
    })
}

fn find_in_note_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": { "query": { "type": "string", "description": "The exact word or phrase to look for in the open note." } },
        "required": ["query"]
    })
}

fn search_notes_params() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": { "query": { "type": "string", "description": "The search keywords." } },
        "required": ["query"]
    })
}

/// Compact model-facing schemas. Function descriptions are always omitted
/// because the stable preamble already teaches tool semantics. Parameter
/// descriptions are retained only for profiles that explicitly need them.
pub fn compact_tool_specs_for_profile(
    specs: Vec<Value>,
    verbose_parameter_descriptions: bool,
) -> Vec<Value> {
    fn strip_descriptions(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.remove("description");
                for child in map.values_mut() {
                    strip_descriptions(child);
                }
            }
            Value::Array(items) => {
                for item in items {
                    strip_descriptions(item);
                }
            }
            _ => {}
        }
    }
    let mut compacted: Vec<Value> = specs
        .into_iter()
        .filter_map(|spec| {
            let function = spec.get("function")?;
            let name = function.get("name")?.clone();
            let mut parameters = function.get("parameters")?.clone();
            if !verbose_parameter_descriptions {
                strip_descriptions(&mut parameters);
            }
            Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": name,
                    "parameters": parameters
                }
            }))
        })
        .collect();
    // Tool selection is semantic; its order must not vary with heuristic paths.
    // A fixed order gives llama-server's prompt cache a stable schema prefix.
    compacted.sort_by(|a, b| {
        a["function"]["name"]
            .as_str()
            .cmp(&b["function"]["name"].as_str())
    });
    compacted
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadNoteArgs {
    pub(super) note_id: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ToolError {
    pub(super) message: String,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tool error: {}", self.message)
    }
}
impl std::error::Error for ToolError {}
