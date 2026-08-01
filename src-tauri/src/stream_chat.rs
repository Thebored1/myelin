//! Tool execution dispatcher shared by the chat paths.
//!
//! `execute_tool` is the single entry point that runs a rig `Tool` from a raw
//! arguments JSON string. It is used by the openharn sidecar path
//! (`crate::sidecar`) so every guard rail, approval gate and save stays in one
//! place. (The legacy in-process `run_chat` streaming loop that used to live
//! here was deleted — the sidecar is the production chat path.)

use crate::agent::{
    AppendNoteArgs, AppendNoteTool, DeleteInNoteArgs, DeleteInNoteTool, EditNotebookArgs,
    EditNotebookTool, FetchWebPageArgs, FetchWebPageTool, FindInNoteArgs, FindInNoteTool,
    FormatNoteArgs, FormatNoteTool, InsertAfterLineArgs, InsertAfterLineTool, PrependNoteArgs,
    PrependNoteTool, ReadNoteArgs, ReadNoteTool, ReplaceInNoteArgs, ReplaceInNoteTool,
    SearchDocumentsArgs, SearchDocumentsTool, SearchNotesArgs, SearchNotesTool, WebSearchArgs,
    WebSearchTool, WriteNoteArgs, WriteNoteTool,
};
use crate::state::AppState;
use rig_core::tool::Tool;
use serde_json::{json, Value};

/// Coerce a JSON value to fix common type mismatches that the model emits in
/// native FC mode. For example, the model may emit `find: []` (an empty array)
/// where a string is expected, or `index: "0"` (a string) where a number is
/// expected. This function recursively walks the JSON and coerces:
/// - arrays → null (for `Option<String>` fields that received `[]`)
/// - strings that look like numbers → numbers (for `usize`/`u32` fields)
/// - numbers → strings (for `Option<String>` fields that received a number)
fn coerce_args(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                out.insert(k, coerce_args(val));
            }
            Value::Object(out)
        }
        Value::Array(arr) => {
            // Empty arrays often appear where Option<String> is expected.
            // Convert to null so serde's Option<String> accepts it.
            if arr.is_empty() {
                Value::Null
            } else {
                Value::Array(arr.into_iter().map(coerce_args).collect())
            }
        }
        Value::String(s) => {
            // If the string looks like an integer, convert to a number.
            // This helps fields like `index: usize` that received `"0"`.
            if let Ok(n) = s.parse::<i64>() {
                Value::from(n)
            } else if let Ok(n) = s.parse::<u64>() {
                Value::from(n)
            } else {
                Value::String(s)
            }
        }
        other => other,
    }
}

/// Deserialize the arguments for a named tool and run the matching rig `Tool`,
/// reusing all of its guard rails / save logic. Returns the tool's result text
/// (tools return `Ok(message)` even for refusals). Used by the openharn
/// sidecar path (`crate::sidecar`), so the real tools stay in Myelin's process.
pub async fn execute_tool(state: &AppState, name: &str, args: &str) -> String {
    if let Err(reason) = state.authorize_tool_call(name) {
        log::warn!("[execute_tool] blocked {name}: {reason}");
        return format!("Tool call rejected: {reason}");
    }
    let v: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
    match name {
        "write_note" => match serde_json::from_value::<WriteNoteArgs>(v.clone()) {
            Ok(a) => WriteNoteTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => {
                // Retry with coerced arguments (fixes type mismatches like
                // `find: []` where a string is expected).
                match serde_json::from_value::<WriteNoteArgs>(coerce_args(v)) {
                    Ok(a) => WriteNoteTool {
                        state: state.clone(),
                    }
                    .call(a)
                    .await
                    .unwrap_or_else(|e| e.to_string()),
                    Err(e2) => format!("Invalid write_note arguments: {e} (coerced: {e2})"),
                }
            }
        },
        "append_note" => match serde_json::from_value::<AppendNoteArgs>(v.clone()) {
            Ok(a) => AppendNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<AppendNoteArgs>(coerce_args(v)) {
                Ok(a) => AppendNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid append_note arguments: {e} (coerced: {e2})"),
            },
        },
        "prepend_note" => match serde_json::from_value::<PrependNoteArgs>(v.clone()) {
            Ok(a) => PrependNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<PrependNoteArgs>(coerce_args(v)) {
                Ok(a) => PrependNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid prepend_note arguments: {e} (coerced: {e2})"),
            },
        },
        "replace_in_note" => match serde_json::from_value::<ReplaceInNoteArgs>(v.clone()) {
            Ok(a) => ReplaceInNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<ReplaceInNoteArgs>(coerce_args(v)) {
                Ok(a) => ReplaceInNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid replace_in_note arguments: {e} (coerced: {e2})"),
            },
        },
        "insert_after_line" => match serde_json::from_value::<InsertAfterLineArgs>(v.clone()) {
            Ok(a) => InsertAfterLineTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<InsertAfterLineArgs>(coerce_args(v)) {
                Ok(a) => InsertAfterLineTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid insert_after_line arguments: {e} (coerced: {e2})"),
            },
        },
        "delete_in_note" => match serde_json::from_value::<DeleteInNoteArgs>(v.clone()) {
            Ok(a) => DeleteInNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<DeleteInNoteArgs>(coerce_args(v)) {
                Ok(a) => DeleteInNoteTool { state: state.clone() }.call(a).await.unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid delete_in_note arguments: {e} (coerced: {e2})"),
            },
        },
        "read_note" => match serde_json::from_value::<ReadNoteArgs>(v.clone()) {
            Ok(a) => ReadNoteTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<ReadNoteArgs>(coerce_args(v)) {
                Ok(a) => ReadNoteTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid read_note arguments: {e} (coerced: {e2})"),
            },
        },
        "search_notes" => match serde_json::from_value::<SearchNotesArgs>(v.clone()) {
            Ok(a) => SearchNotesTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<SearchNotesArgs>(coerce_args(v)) {
                Ok(a) => SearchNotesTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid search_notes arguments: {e} (coerced: {e2})"),
            },
        },
        "fetch_web_page" => match serde_json::from_value::<FetchWebPageArgs>(v.clone()) {
            Ok(a) => FetchWebPageTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<FetchWebPageArgs>(coerce_args(v)) {
                Ok(a) => FetchWebPageTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid fetch_web_page arguments: {e} (coerced: {e2})"),
            },
        },
        "web_search" => match serde_json::from_value::<WebSearchArgs>(v.clone()) {
            Ok(a) => WebSearchTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<WebSearchArgs>(coerce_args(v)) {
                Ok(a) => WebSearchTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid web_search arguments: {e} (coerced: {e2})"),
            },
        },
        "search_documents" => match serde_json::from_value::<SearchDocumentsArgs>(v.clone()) {
            Ok(a) => SearchDocumentsTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<SearchDocumentsArgs>(coerce_args(v)) {
                Ok(a) => SearchDocumentsTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid search_documents arguments: {e} (coerced: {e2})"),
            },
        },
        "find_in_note" => match serde_json::from_value::<FindInNoteArgs>(v.clone()) {
            Ok(a) => FindInNoteTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<FindInNoteArgs>(coerce_args(v)) {
                Ok(a) => FindInNoteTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid find_in_note arguments: {e} (coerced: {e2})"),
            },
        },
        "format_note" => match serde_json::from_value::<FormatNoteArgs>(v.clone()) {
            Ok(a) => FormatNoteTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<FormatNoteArgs>(coerce_args(v)) {
                Ok(a) => FormatNoteTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid format_note arguments: {e} (coerced: {e2})"),
            },
        },
        "edit_notebook" => match serde_json::from_value::<EditNotebookArgs>(v.clone()) {
            Ok(a) => EditNotebookTool {
                state: state.clone(),
            }
            .call(a)
            .await
            .unwrap_or_else(|e| e.to_string()),
            Err(e) => match serde_json::from_value::<EditNotebookArgs>(coerce_args(v)) {
                Ok(a) => EditNotebookTool {
                    state: state.clone(),
                }
                .call(a)
                .await
                .unwrap_or_else(|e| e.to_string()),
                Err(e2) => format!("Invalid edit_notebook arguments: {e} (coerced: {e2})"),
            },
        },
        other => format!("Unknown tool: {other}"),
    }
}
