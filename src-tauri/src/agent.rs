use crate::state::AppState;
use futures_util::StreamExt;
use rig_core::client::CompletionClient;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;

/// How long the UI may take to answer a tool-approval prompt before the request
/// is refused. Bounds the backend wait; the frontend auto-rejects on the same
/// deadline.
const TOOL_APPROVAL_TIMEOUT_SECS: u64 = 120;

/// Removes the registered approval entry when the wait completes any way: user
/// decision, timeout, cancellation, or the awaiting task being dropped. Without
/// it a cancelled turn leaks a dead sender in `pending_approvals` forever.
struct PendingApprovalGuard<'a> {
    state: &'a AppState,
    id: String,
}

impl Drop for PendingApprovalGuard<'_> {
    fn drop(&mut self) {
        self.state.remove_pending_approval(&self.id);
    }
}

async fn check_tool_approval(
    state: &AppState,
    tool_name: &str,
    title: &str,
    content_preview: &str,
) -> Result<(), String> {
    if !state.is_tool_approval_required() {
        return Ok(());
    }
    let req_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.register_pending_approval(req_id.clone(), tx);
    let _guard = PendingApprovalGuard {
        state,
        id: req_id.clone(),
    };

    let _ = state.handle.emit(
        "ai://tool_approval_request",
        serde_json::json!({
            "id": req_id,
            "tool": tool_name,
            "title": title,
            "content": content_preview
        }),
    );

    tokio::select! {
        result = rx => match result {
            Ok(true) => Ok(()),
            Ok(false) => Err("User rejected this action.".to_string()),
            Err(_) => Err("Approval request cancelled.".to_string()),
        },
        _ = state.wait_for_ai_cancel() => {
            Err("Cancelled by the user while awaiting approval; no changes were made.".to_string())
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(TOOL_APPROVAL_TIMEOUT_SECS)) => {
            Err("Approval request timed out after 120 seconds; no changes were made.".to_string())
        }
    }
}

const WEB_FETCH_LIMIT: usize = 6_000;
/// Raw body cap for `fetch_web_page`: stop reading after this many bytes so a
/// huge page cannot fill memory before the 6,000-character text excerpt is cut.
const WEB_BODY_CAP: usize = 256 * 1024;

/// System preamble for the note assistant. Kept as a single source of truth so
/// the startup cache warm-up replays the exact same prefix the live agent uses.
/// Deliberately small: a role line plus the minimum tool guidance needed to keep
/// the model from flooding the (memory-bounded) context with stray web/search
/// calls or describing edits in chat instead of writing them. Tool schemas are
/// still passed separately via `tool_specs` on every request.
pub const MYELIN_PREAMBLE: &str = concat!(
    "You are the assistant inside Myelin, a local notes app, powered by an open model running locally on the user's own machine. If asked what or who you are, identify yourself as Myelin's built-in AI assistant — do not claim to be proprietary or commercial software. The text of the note currently open in the editor is included in the system context — you already have it.\n\n",
    "- To change the open note (write, rewrite, edit, format, add to, shorten, clear, etc.), pick the matching Edit tool from those listed below. Use write_note to replace the whole note; append_note to add to the end; prepend_note to add to the beginning; insert_after_line to add after a specific line; replace_in_note to change specific text; delete_in_note to remove a part. The ONLY way to change the note is a tool call: never describe the edit, print new note text, or type tool names in your chat reply. When the user says \"write this\", \"put that in the note\", or similar, and a preceding assistant message contains the requested draft, copy that exact draft into `content` — do not compose a substitute or a different version. Preserve its Markdown exactly, including headings, blank lines, lists, bold text, and line breaks.\n",
    "- Write real Markdown: a heading line starts with \"# \" (a hash then a space), \"## \" for a sub-heading; bullets start with \"- \". \"**bold**\" is NOT a heading. Use ONLY plain newline characters (the enter/return key) to separate lines of poetry or paragraphs — never use `<br>` HTML tags, em spaces, asterisks, or any other formatting as line-break separators. Do not include `<`, `<<`, `<>`, or similar markup artifacts — these break the note display.\n",
    "- When editing, reproduce every line that should stay and change only what was asked. Never return an empty or much-shorter note unless the user explicitly asked to clear or shorten it.\n",
    "- When the user asks you to write what you found, researched, learned, or understood, put the ACTUAL information into the note as a finished, self-contained note — the real facts, perspectives, and details (use what you found in the conversation plus what you reliably know about the topic). NEVER write a question, an offer to do more (e.g. \"Would you like me to fetch the full text?\"), or a promise to act later (e.g. \"I will now fetch...\") as the note's content — the note holds finished information, not conversation. If you lack some detail, still write the best complete note you can from what you know rather than asking or deferring.\n",
    "- The currently open note is the default target for every request, including reading, explaining, finding, and editing. Its full text is already in this prompt: answer questions about it directly and never use search_notes or read_note for it. Use search_notes or read_note only when the user explicitly asks about another note, their other notes, or the workspace; use fetch_web_page only when the user explicitly gives or asks to visit a URL/web address. For greetings or general questions, just reply briefly — do not read, search, or fetch.\n",
    "- ROUTING: In this notes app, every instruction, command, or action request is work to perform, never chat. Unless it explicitly names another target, perform it on the open note with the appropriate tool. Note operations include creating, writing, drafting, generating, adding, appending, inserting, replacing, updating, editing, revising, rewriting, correcting, improving, expanding, shortening, summarizing, translating, organizing, restructuring, moving, merging, splitting, titling, renaming, making lists, formatting, converting Markdown, cleaning up, removing, deleting, clearing, restoring, reading, finding, counting, searching, fetching, browsing, looking up, and researching. For example, \"write this on the note\", \"add a poem\", \"put that in the note\", \"summarize this\", \"make this a list\", \"change the title\", and \"remove this paragraph\" all require a tool call. Only a request for a direct answer, explanation, capability description, greeting, thanks, small talk, opinion, or general knowledge is chat; answer it directly and never modify, read, search, or fetch notes unless it explicitly asks you to do so.\n\n",
    "Worked examples show only the editing style — the resulting note text you must pass as the Edit tool's `content` parameter (always via the tool call, never printed in chat):\n\n",

    "Example 1\n",
    "NOTE:\n**Cars**\nThey have engines.\n",
    "USER: make the title a heading\n",
    "(resulting note)\n# Cars\nThey have engines.\n\n",
    "Example 2\n",
    "NOTE:\n## Intro\nPersonal computers changed everything.\n## History\nIt began in the 1970s.\n",
    "USER: remove all headings\n",
    "(resulting note)\nIntro\nPersonal computers changed everything.\nHistory\nIt began in the 1970s.\n\n",
    "Example 3\n",
    "NOTE: (empty)\n",
    "USER: write a short note titled Sea\n",
    "(resulting note)\n# Sea\nThe sea is vast and restless."
);

/// Minimal system policy for a tool-free Chat turn. Sending the full editing
/// manual and worked mutation examples when no tools are present wastes prompt
/// evaluation—especially on recurrent/hybrid models whose llama.cpp cache
/// backend may be unable to restore a prefix between requests.
pub const DIRECT_CHAT_PREAMBLE: &str = concat!(
    "You are Myelin's built-in AI assistant, powered by a local model. ",
    "The currently open note's title and content are included below. ",
    "Answer the user's question directly from that context or general knowledge. ",
    "Do not claim to read, search, or modify anything, and do not emit tool calls. ",
    "Be concise unless the user asks for detail."
);

/// Single source of truth for every tool's model-facing contract: name,
/// description, and argument schema. Drives BOTH the wire path (`tool_specs`:
/// warm-up request, tool gates, sidecar) and the rig path (each `Tool::definition`
/// impl below), so the two can never drift apart again. Keep this table in the
/// same order the tools were historically registered so the warm-up prefix
/// stays stable.
type ToolParams = fn() -> Value;

const TOOL_CONTRACTS: &[(&str, &str, ToolParams)] = &[
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
fn tool_contract(name: &str) -> Option<(&'static str, &'static str, ToolParams)> {
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

/// An editor text selection the user armed, sent alongside the chat request.
/// `text` is the selected source markdown; `before`/`after` are short surrounding
/// context snippets used to pin the exact occurrence so repeats — and a body that
/// drifts between turns — don't move the target. Sent as text+context rather than
/// raw offsets to avoid the JS-UTF16 vs Rust-UTF8 index mismatch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionArg {
    pub text: String,
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
    /// A zero-length target captured from the editor caret. In this mode
    /// `before`/`after` locate an insertion boundary rather than selected text.
    #[serde(default)]
    pub cursor: bool,
    /// For Jupyter notebooks, the 0-based cell containing this source target.
    #[serde(default)]
    pub cell_index: Option<usize>,
}

/// Locate the byte range in `body` an armed selection refers to. Picks the
/// occurrence of `text` whose surrounding context best matches `before`/`after`,
/// so it survives repeats and edits elsewhere. None if the text no longer occurs
/// (the user changed that span — caller falls back to normal planning).
pub fn locate_selection(body: &str, sel: &SelectionArg) -> Option<(usize, usize)> {
    // The selection captured from the rendered editor often has leading/trailing
    // whitespace that isn't in the source (e.g. it ran past a paragraph), so trim
    // before matching — we only ever want to replace the text itself.
    let text = sel.text.trim();
    if text.is_empty() {
        return None;
    }
    // Compare anchors whitespace-tolerantly: the captured selection's boundaries
    // rarely line up byte-for-byte with the source (markers, stray newlines).
    let before = sel.before.trim();
    let after = sel.after.trim();
    let mut best: Option<(u8, usize, usize)> = None; // (context score, start, end)
    let mut from = 0;
    while let Some(rel) = body[from..].find(text) {
        let start = from + rel;
        let end = start + text.len();
        let before_ok = before.is_empty() || body[..start].trim_end().ends_with(before);
        let after_ok = after.is_empty() || body[end..].trim_start().starts_with(after);
        let score = before_ok as u8 + after_ok as u8;
        if best.map(|(s, _, _)| score > s).unwrap_or(true) {
            best = Some((score, start, end));
            if score == 2 {
                break; // both anchors match — definitely the right occurrence
            }
        }
        from = start + 1;
    }
    // Fallback for a selection that doesn't match the source verbatim — most often
    // a FORMATTED span where the rendered selection dropped the markdown markers
    // (**bold**, `code`, a heading's `# `). find_tolerant matches the inner words
    // (whitespace-tolerant), so we splice inside the markers and keep them.
    best.map(|(_, s, e)| (s, e))
        .or_else(|| find_tolerant(body, text))
}

/// If the user armed a selection, build a plan that replaces ONLY that span with
/// the model's `content`, keeping the rest of the note byte-identical. Returns
/// None (caller falls through to normal planning) when there's no usable span
/// or the model
/// regenerated the surrounding note (its content already contains the text just
/// before/after the selection → it did a full rewrite, which we honor instead).
pub fn selection_scoped_plan(body: &str, content: &str, sel: &SelectionArg) -> Option<WritePlan> {
    let content = strip_prompt_markers(content);
    if sel.cursor {
        // Markdown frontmatter parsing may represent a visually empty editor as
        // one or more newlines while the frontend's cursor target has no anchors.
        // Every boundary would otherwise match and be rejected as ambiguous,
        // even though an empty note has only one meaningful insertion location.
        if body.trim().is_empty()
            && sel.before.trim().is_empty()
            && sel.after.trim().is_empty()
        {
            return Some(WritePlan {
                new_body: content,
                op: WriteOp::EditSnippet,
            });
        }
        let positions = (0..=body.len()).filter(|position| {
            body.is_char_boundary(*position)
                && (sel.before.is_empty() || body[..*position].ends_with(&sel.before))
                && (sel.after.is_empty() || body[*position..].starts_with(&sel.after))
        });
        let matches: Vec<usize> = positions.take(2).collect();
        if matches.len() != 1 {
            return None;
        }
        let position = matches[0];
        let mut insertion = content;
        let left_alnum = body[..position].chars().next_back().is_some_and(char::is_alphanumeric);
        let right_alnum = body[position..].chars().next().is_some_and(char::is_alphanumeric);
        if left_alnum && insertion.chars().next().is_some_and(char::is_alphanumeric) {
            insertion.insert(0, ' ');
        }
        if right_alnum && insertion.chars().next_back().is_some_and(char::is_alphanumeric) {
            insertion.push(' ');
        }
        let mut new_body = String::with_capacity(body.len() + insertion.len());
        new_body.push_str(&body[..position]);
        new_body.push_str(&insertion);
        new_body.push_str(&body[position..]);
        return Some(WritePlan { new_body, op: WriteOp::EditSnippet });
    }
    let (start, end) = locate_selection(body, sel)?;
    let regenerated_whole = (!sel.after.trim().is_empty() && content.contains(sel.after.trim()))
        || (!sel.before.trim().is_empty() && content.contains(sel.before.trim()));
    if regenerated_whole {
        return None;
    }
    let mut new_body = String::with_capacity(body.len() + content.len());
    new_body.push_str(&body[..start]);
    new_body.push_str(&content);
    new_body.push_str(&body[end..]);
    Some(WritePlan {
        new_body,
        op: WriteOp::EditSnippet,
    })
}

/// Validate an insertion marker against the armed selection and insert only
/// immediately after the selected line.
pub fn selection_insert_after_plan(
    body: &str,
    marker: &str,
    content: &str,
    sel: &SelectionArg,
) -> Result<WritePlan, String> {
    let (start, end) = locate_selection(body, sel)
        .ok_or_else(|| "The armed selection is no longer present in the note.".to_string())?;
    let marker = marker.trim();
    let selected = &body[start..end];
    if marker.is_empty()
        || (marker != selected
            && !selected.contains(marker)
            && !marker.contains(selected)
            && !sel.text.trim().contains(marker))
    {
        return Err("Rejected insertion marker: it does not match the armed selection.".to_string());
    }
    let line_end = body[end..]
        .find('\n')
        .map(|i| end + i + 1)
        .unwrap_or(body.len());
    let content = clean_note_content(&strip_prompt_markers(content));
    let new_body = format!("{}{}\n\n{}", &body[..line_end], content, &body[line_end..]);
    Ok(WritePlan { new_body, op: WriteOp::Append })
}

/// Pure decision for `write_note`, `append_note` and the shared edit helpers:
/// `plan_write`, `apply_format_op`, `clean_note_content`,
/// `note_content_has_protocol_residue`, `normalize_append_content`,
/// `strip_prompt_markers`, `find_tolerant`, `FORMAT_OPS`, `is_format_op` and
/// the `WriteOp`/`WritePlan` types live in `myelin-edit-core` so the sidecar's
/// on-disk edits stay byte-identical to the app's.
pub use myelin_edit_core::{
    apply_format_op, clean_note_content, find_tolerant, is_format_op,
    note_content_has_protocol_residue, normalize_append_content, plan_write,
    strip_prompt_markers, WriteOp, WritePlan, FORMAT_OPS,
};

/// Word-boundary check: true if any of `words` (lowercase literals/phrases)
/// appears as a whole word in the already-lowercased `haystack`. Avoids
/// substring false hits like "fix" inside "prefix" or "add" inside "address".
fn contains_any_word(haystack: &str, words: &[&str]) -> bool {
    let alternation = words
        .iter()
        .map(|w| regex::escape(w))
        .collect::<Vec<_>>()
        .join("|");
    regex::Regex::new(&format!(r"\b(?:{alternation})\b"))
        .map(|re| re.is_match(haystack))
        .unwrap_or(false)
}

/// Negation words that invert the intent of a following keyword.
const NEGATIONS: &[&str] = &[
    "don't",
    "doesn't",
    "didn't",
    "won't",
    "wouldn't",
    "shouldn't",
    "couldn't",
    "isn't",
    "aren't",
    "wasn't",
    "weren't",
    "haven't",
    "hasn't",
    "hadn't",
    "can't",
    "cannot",
    "without",
    "never",
    "not",
    "no",
];

/// Check if a negation word appears before the FIRST occurrence of any keyword
/// in the lowercased message. This catches "don't search notes" (negated) but
/// not "search notes" (not negated). A heuristic — it won't catch every
/// construction, but it prevents the common false positives where a negated
/// request still triggers a tool offer.
fn is_negated(message: &str, keywords: &[&str]) -> bool {
    let m = message.to_lowercase();
    let first_pos = keywords.iter().filter_map(|kw| m.find(kw)).min();
    if let Some(pos) = first_pos {
        let before = &m[..pos];
        // Word-boundary match: a raw substring check on "not"/"no" misfires on
        // ordinary words — "note" contains "not", "know" contains "no" — which
        // silently negated e.g. "does this note contain X?".
        return NEGATIONS.iter().any(|n| contains_any_word(&before, &[n]));
    }
    false
}

/// Heuristic: does this user message ask to CREATE or MODIFY the open note (as
/// Whether this request asks to add new text rather than replace the body. Used
/// to offer an append-only tool contract so the model never needs to regenerate
/// the existing note just to add a paragraph.
pub fn append_request_intent(message: &str) -> bool {
    let text = message.trim().to_ascii_lowercase();
    text.starts_with("add ")
        || text.contains("append")
        || text.contains("add below")
        || text.contains("add to")
        || text.contains("insert")
        || placement_request_intent(message)
}

// Routing-only distinction: prompt construction always includes the body, but
// the specialized write contract still needs to know whether a request is a
// fresh whole-note creation or an edit that must preserve existing text.
fn existing_note_operation(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "edit", "rewrite", "revis", "format", "shorten", "expand", "reorder", "remove",
        "delete", "replace", "clean", "fix", "change", "update", "summari", "condens",
        "turn this", "keep the rest",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn placement_request_intent(message: &str) -> bool {
    let text = message.to_ascii_lowercase();
    ["below it", "under it", "after it", "beneath it", "below this", "under this", "after this", "beneath this"]
        .iter()
        .any(|phrase| text.contains(phrase))
}

/// opposed to just chatting / asking a question)? Used by `select_tools` to
/// decide whether to offer `write_note` this turn. In Myelin the chat is a
/// note-assistant sidebar, so virtually every edit verb refers to the open note.
/// Pure and unit-tested.
pub fn note_write_intent(message: &str) -> bool {
    let m = message.trim().to_lowercase();
    if m.is_empty() {
        return false;
    }

    // Short affirmations greenlight a write the user just asked for. The preamble
    // also treats these as "proceed now", so honour them here too.
    let affirmation = m.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    const AFFIRMATIONS: &[&str] = &[
        "yes",
        "y",
        "yeah",
        "yep",
        "yup",
        "sure",
        "ok",
        "okay",
        "k",
        "go ahead",
        "do it",
        "please do",
        "go for it",
        "sounds good",
        "anything",
        "you decide",
        "proceed",
        "go",
    ];
    if AFFIRMATIONS.contains(&affirmation) {
        return true;
    }
    // Leading affirmation word ("yes please", "sure, go for it"). Limited to
    // strong single-word affirmations so a question like "ok what is X" is not
    // mistaken for a write.
    const LEADING_AFFIRMATIONS: &[&str] = &[
        "yes",
        "yeah",
        "yep",
        "yup",
        "sure",
        "absolutely",
        "definitely",
    ];
    let first_word = affirmation.split_whitespace().next().unwrap_or("");
    if LEADING_AFFIRMATIONS.contains(&first_word) {
        return true;
    }

    // Strong create/edit verbs. In this app these always target the open note.
    const WRITE_VERBS: &[&str] = &[
        "write",
        "rewrite",
        "re-write",
        "create",
        "draft",
        "compose",
        "add",
        "append",
        "insert",
        "generate",
        "produce",
        "jot",
        "fill",
        "format",
        "reformat",
        "restructure",
        "reorganize",
        "reorganise",
        "organize",
        "organise",
        "clean up",
        "cleanup",
        "tidy",
        "fix",
        "correct",
        "proofread",
        "improve",
        "polish",
        "edit",
        "revise",
        "update",
        "change",
        "modify",
        "shorten",
        "condense",
        "trim",
        "expand",
        "lengthen",
        "elaborate",
        "reorder",
        "rearrange",
        "remove",
        "delete",
        "erase",
        "replace",
        "swap",
        "clear",
        "empty",
        "wipe",
        "blank",
        "scrap",
        "bold",
        "italic",
        "italicize",
        "capitalize",
        "capitalise",
        "continue",
        "extend",
        "finish",
        "translate",
        "rephrase",
        "reword",
        // Transform phrasings that don't use a bare edit verb.
        "make it",
        "make this",
        "make the",
        "turn it",
        "turn this",
        "convert it",
        "convert this",
        "shorter",
        "longer",
        "concise",
    ];
    // Negation guard: "don't write a note about X" must NOT match — the user
    // is declining a write, not requesting one. Only applies to the verb list,
    // not to affirmations (which are inherently positive).
    if !is_negated(message, WRITE_VERBS) && contains_any_word(&m, WRITE_VERBS) {
        return true;
    }

    // Soft content verbs (explain/describe/...) only count as a note write when
    // the message explicitly points at the note ("explain X in the note").
    const NOTE_TARGETS: &[&str] = &[
        "the note",
        "this note",
        "in the note",
        "to the note",
        "into the note",
        "my note",
        "the document",
        "the doc",
        "the page",
    ];
    const SOFT_VERBS: &[&str] = &[
        "explain",
        "describe",
        "list",
        "summarize",
        "summarise",
        "answer",
        "outline",
        "detail",
        "note down",
        "record",
    ];
    let targets_note = NOTE_TARGETS.iter().any(|t| m.contains(t));
    if targets_note && !is_negated(message, SOFT_VERBS) && contains_any_word(&m, SOFT_VERBS) {
        return true;
    }

    false
}

/// Whether the request can be completed by one write to the currently open note.
/// Requests that need information first must use the normal multi-tool loop so it
/// can search/read/fetch before producing the final `write_note` call.
pub fn is_pure_note_write_request(message: &str) -> bool {
    note_write_intent(message)
        && !wants_search(message)
        && !wants_fetch(message)
        && !wants_other_notes(message)
        && !wants_documents(message)
        && !wants_find(message)
}

/// Pure greeting / acknowledgement vocabulary. If the whole message is made of
/// these words, it's small talk → offer NO tools so the model can't reflexively
/// call one. The word count is capped at 6 (not 4) to catch natural greetings
/// like "thanks for the help" and "how are you doing today" without matching
/// real requests. (Pattern borrowed from the ggufplay experiment.)
const SMALL_TALK: &[&str] = &[
    "hi",
    "hello",
    "hey",
    "yo",
    "sup",
    "hiya",
    "howdy",
    "gg",
    "wsg",
    "thanks",
    "thank",
    "you",
    "your",
    "welcome",
    "thankyou",
    "thx",
    "ty",
    "cheers",
    "ok",
    "okay",
    "k",
    "kk",
    "cool",
    "nice",
    "great",
    "awesome",
    "perfect",
    "got",
    "it",
    "gotcha",
    "sounds",
    "good",
    "sure",
    "yep",
    "yeah",
    "yup",
    "yes",
    "no",
    "nope",
    "lol",
    "haha",
    "hah",
    "np",
    "problem",
    "morning",
    "afternoon",
    "evening",
    "night",
    "so",
    "much",
    "please",
    "mate",
    "man",
    "bro",
    "how",
    "are",
    "whats",
    "up",
    "doing",
    "going",
    "for",
    "the",
    "help",
    "there",
    "today",
    "anytime",
    "alright",
    "aight",
    "sup",
    "wassup",
    "waddup",
];

pub fn is_small_talk(message: &str) -> bool {
    let words: Vec<String> = message
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if words.is_empty() {
        return true;
    }
    // Cap at 6 words: long enough for "thanks for the help" / "how are you doing
    // today" but short enough that real requests (which carry content words
    // like "write", "search", "remove") are never mistaken for small talk.
    if words.len() > 6 {
        return false;
    }
    words.iter().all(|w| SMALL_TALK.contains(&w.as_str()))
}

/// Does the message refer to OTHER notes in the workspace (search/read), as
/// opposed to the open note whose content is already in the prompt? Precise on
/// purpose: "write a note about X" (creating content in the OPEN note) must NOT
/// match, or it would needlessly offer search/read.
pub fn wants_other_notes(message: &str) -> bool {
    let m = message.to_lowercase();
    // Keywords whose presence signals "look at OTHER notes". If a negation
    // word appears before the first match, the request is declined — don't offer
    // search/read tools.
    const KW: &[&str] = &[
        "other note",
        "my notes",
        "another note",
        "which note",
        "note with id",
        "note id",
        "note titled",
        "note called",
        "read the note with",
        "look up",
        "search my note",
        "search note",
        "search for a note",
        "find a note",
        "find the note",
        "find my note",
        "search",
        "find",
    ];
    let matched = m.contains("other note")
        || m.contains("my notes")
        || m.contains("another note")
        || m.contains("which note")
        || m.contains("note with id")
        || m.contains("note id")
        || m.contains("note titled")
        || m.contains("note called")
        || m.contains("read the note with")
        || m.contains("look up")
        || m.contains("search my note")
        || m.contains("search note")
        || m.contains("search for a note")
        || m.contains("find a note")
        || m.contains("find the note")
        || m.contains("find my note")
        || (contains_any_word(&m, &["search", "find"]) && m.contains("notes"));
    matched && !is_negated(message, KW)
}

/// TLD allowlist used to spot a BARE domain (example.com, speediq.ai) in a
/// message while keeping real file names out (notes.txt, model.gguf, poem.md are
/// NOT web targets). Ported from the ggufplay gating experiment.
fn has_web_domain(m: &str) -> bool {
    const WEB_TLD: &str =
        "com|org|net|io|ai|dev|co|app|gov|edu|me|xyz|info|biz|us|uk|ca|de|fr|in|cloud|tech|news|gg|so";
    regex::Regex::new(&format!(
        r"(?i)\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{WEB_TLD})\b"
    ))
    .map(|re| re.is_match(m))
    .unwrap_or(false)
}

/// Real intent to SEARCH the open web (no URL in hand) — kept precise (explicit
/// search phrasing or an online/web/internet qualifier) so it doesn't fire on
/// incidental words or clobber note-search. web_search finds pages; the model
/// then opens one with fetch_web_page.
pub fn wants_search(message: &str) -> bool {
    let m = message.to_lowercase();
    // Keywords whose presence signals "search the open web". If a negation
    // word appears before the first match, the request is declined.
    const KW: &[&str] = &[
        "search the web",
        "search online",
        "web search",
        "search the internet",
        "on the internet",
        "browse the web",
        "look online",
        "look it up online",
        "google",
        "search",
        "find",
        "look",
        "lookup",
    ];
    let matched = m.contains("search the web")
        || m.contains("search online")
        || m.contains("web search")
        || m.contains("search the internet")
        || m.contains("on the internet")
        || m.contains("browse the web")
        || m.contains("look online")
        || m.contains("look it up online")
        || m.contains("google ")
        || (contains_any_word(&m, &["search", "find", "look", "lookup"])
            && contains_any_word(&m, &["online", "web", "internet"]));
    matched && !is_negated(message, KW)
}

/// True only when the user explicitly asked to empty/clear/delete the WHOLE
/// note. Deliberately narrow: "remove all headings", "delete the intro", etc.
/// are partial edits and must NOT match — they keep the rest of the note.
pub fn wants_clear(message: &str) -> bool {
    let m = message.to_lowercase();
    const PHRASES: &[&str] = &[
        "clear the note",
        "clear note",
        "clear it",
        "empty the note",
        "empty it",
        "make it blank",
        "make it empty",
        "delete the note",
        "delete everything",
        "delete all the text",
        "delete all text",
        "remove everything",
        "remove all the text",
        "remove all text",
        "erase everything",
        "erase the note",
        "wipe the note",
        "start over",
        "start fresh",
        "blank note",
    ];
    let matched = PHRASES.iter().any(|p| m.contains(p));
    // "don't clear the note" must NOT match — the user is declining, not requesting.
    matched && !is_negated(message, PHRASES)
}

/// True for a request to remove PART of the open note (a paragraph, heading,
/// section, line, sentence, etc.) and nothing else — as opposed to a whole-note
/// clear ([`wants_clear`]) or a removal mixed with new content. Used by
/// `write_note` to do a surgical deletion from the real body instead of trusting
/// the model's `content` (which models tend to fill with the whole regenerated
/// note — slow, and a truncation risk on long notes).
pub fn wants_partial_removal(message: &str) -> bool {
    if wants_clear(message) {
        return false;
    }
    let m = message.to_lowercase();
    let remove_kw = ["remove", "delete", "erase", "cut", "drop", "omit"];
    let has_remove = contains_any_word(&m, &remove_kw)
        || m.contains("get rid of")
        || m.contains("take out")
        || m.contains("strip out");
    // A removal mixed with new/changed content ("delete X and add Y", "replace the
    // intro") is a real edit, not a pure deletion — leave it to the model's content.
    let add_kw = [
        "add", "insert", "append", "include", "put", "write", "replace", "change", "rewrite",
        "rename", "make", "turn",
    ];
    let has_add = contains_any_word(&m, &add_kw);
    // "don't remove the second item" must NOT match — the user is declining.
    has_remove && !has_add && !is_negated(message, &remove_kw)
}

/// Recognize a deterministic Markdown transform the model is bad at but a regex
/// nails (strip/convert headings, emphasis, lists, links, code, …). Returns the
/// `apply_format_op` operation, or None. Never fires on a request to CREATE fresh
/// content ("write a numbered list…") so it can't hijack a real write.
pub fn detect_format_op(message: &str) -> Option<&'static str> {
    let m = message.to_lowercase();
    let creating = contains_any_word(
        &m,
        &[
            "write", "create", "draft", "compose", "generate", "give", "jot",
        ],
    );

    // Negation guard: "don't remove the headings" must NOT trigger a format op.
    let negated = is_negated(
        message,
        &["remove", "delete", "strip", "drop", "clear", "kill"],
    );

    // ---- removals (need a removal verb; very low false-positive) ----
    let removal = !negated
        && (contains_any_word(&m, &["remove", "delete", "strip", "drop", "clear", "kill"])
            || m.contains("get rid of")
            || m.contains("take out")
            || m.contains("without the")
            || m.contains("without any")
            || m.contains("no more"));
    if removal {
        if m.contains("all formatting")
            || m.contains("all markdown")
            || m.contains("markdown formatting")
            || m.contains("plain text")
        {
            return Some("strip_markdown");
        }
        if m.contains("heading") || m.contains("header") {
            return Some("remove_headings");
        }
        if (m.contains("bold") && m.contains("italic")) || m.contains("emphasis") {
            return Some("remove_emphasis");
        }
        if m.contains("bold") {
            return Some("remove_bold");
        }
        if m.contains("italic") {
            return Some("remove_italic");
        }
        if m.contains("image") || m.contains("picture") {
            return Some("remove_images");
        }
        if m.contains("link") {
            return Some("remove_links");
        }
        if m.contains("code") {
            return Some("remove_code");
        }
        if m.contains("quote") {
            return Some("remove_blockquotes");
        }
        if m.contains("strikethrough")
            || m.contains("strike-through")
            || m.contains("strike through")
        {
            return Some("remove_strikethrough");
        }
        if m.contains("divider")
            || m.contains("horizontal rule")
            || m.contains("horizontal line")
            || m.contains("separator")
        {
            return Some("remove_horizontal_rules");
        }
        if m.contains("blank line") || m.contains("empty line") || m.contains("extra line") {
            return Some("remove_blank_lines");
        }
        if m.contains("checkbox") || m.contains("check box") || m.contains("task list") {
            return Some("tasks_to_bullets");
        }
        if m.contains("number") {
            return Some("remove_numbering");
        }
        if m.contains("bullet") {
            return Some("remove_bullets");
        }
    }

    // Past here we are transforming EXISTING content; never hijack a fresh write
    // ("write this in uppercase", "create a numbered list …").
    if creating {
        return None;
    }

    // ---- case transforms ----
    if m.contains("uppercase")
        || m.contains("upper case")
        || m.contains("all caps")
        || m.contains("capital letters")
    {
        return Some("uppercase");
    }
    if m.contains("lowercase") || m.contains("lower case") {
        return Some("lowercase");
    }
    if m.contains("title case") || m.contains("titlecase") {
        return Some("title_case");
    }

    // ---- conversions ----
    if (m.contains("heading") || m.contains("header")) && m.contains("bold") {
        let hi = m.find("head").unwrap_or(usize::MAX);
        let bi = m.find("bold").unwrap_or(usize::MAX);
        return Some(if hi < bi {
            "headings_to_bold"
        } else {
            "bold_to_headings"
        });
    }
    if m.contains("heading") || m.contains("header") {
        if m.contains("promote") || m.contains("up a level") || m.contains("larger") {
            return Some("promote_headings");
        }
        if m.contains("demote") || m.contains("down a level") || m.contains("smaller") {
            return Some("demote_headings");
        }
    }
    let convert = contains_any_word(&m, &["convert", "change", "turn", "make", "switch"])
        || m.contains(" to ")
        || m.contains(" into ");
    if convert && (m.contains("bullet") || m.contains("number") || m.contains("ordered")) {
        // The TARGET style is the one mentioned LAST ("turn the numbered list
        // into bullets" → target = bullets → numbered_to_bullets).
        let bullet_pos = m.rfind("bullet");
        let number_pos = m.rfind("number").or_else(|| m.rfind("ordered"));
        match (bullet_pos, number_pos) {
            (Some(b), Some(n)) => {
                return Some(if b > n {
                    "numbered_to_bullets"
                } else {
                    "bullets_to_numbered"
                })
            }
            (Some(_), None) => return Some("numbered_to_bullets"),
            (None, Some(_)) => return Some("bullets_to_numbered"),
            (None, None) => {}
        }
    }
    None
}

/// Does the message ask whether a specific word/phrase is in the OPEN note (or
/// to find/locate one there)? Routed to the deterministic find_in_note tool so
/// the model doesn't have to eyeball-scan the text and get it wrong.
pub fn wants_find(message: &str) -> bool {
    let m = message.to_lowercase();
    // Keywords whose presence signals "find a word in the open note". If a
    // negation word appears before the first match, the request is declined.
    const KW: &[&str] = &[
        "the word",
        "the phrase",
        "the term",
        "search the note",
        "find",
        "locate",
        "see",
        "contains",
        "contain",
        "appear",
        "appears",
        "mention",
        "mentioned",
    ];
    let matched = m.contains("the word")
        || m.contains("the phrase")
        || m.contains("the term")
        || m.contains("search the note")
        || (contains_any_word(
            &m,
            &[
                "find",
                "locate",
                "see",
                "contains",
                "contain",
                "appear",
                "appears",
                "mention",
                "mentioned",
            ],
        ) && contains_any_word(&m, &["note", "here", "text", "above"]));
    matched && !is_negated(message, KW)
}

/// Does the message ask about the user's ingested SOURCE documents (PDF/book/
/// paper/source) — as opposed to the note open in the editor? Precise so it
/// doesn't fire on "this note".
pub fn wants_documents(message: &str) -> bool {
    let m = message.to_lowercase();
    // Keywords whose presence signals "search my documents". If a negation word
    // appears before the first match, the request is declined.
    const KW: &[&str] = &[
        "the pdf",
        "this pdf",
        "my pdf",
        "a pdf",
        "the document",
        "this document",
        "my document",
        "the source",
        "the book",
        "this book",
        "the paper",
        "the article",
        "according to the",
        "in the text",
    ];
    let matched = m.contains("the pdf")
        || m.contains("this pdf")
        || m.contains("my pdf")
        || m.contains("a pdf")
        || m.contains("this pdf")
        || m.contains("the document")
        || m.contains("this document")
        || m.contains("my document")
        || m.contains("the source")
        || m.contains("the book")
        || m.contains("this book")
        || m.contains("the paper")
        || m.contains("the article")
        || m.contains("according to the")
        || m.contains("in the text");
    matched && !is_negated(message, KW)
}

/// Does the message ask to fetch a specific web page — a full URL, a bare
/// domain, or an explicit "fetch/open/visit the page"?
pub fn wants_fetch(message: &str) -> bool {
    let m = message.to_lowercase();
    // Keywords whose presence signals "fetch a web page". If a negation word
    // appears before the first match, the request is declined.
    const KW: &[&str] = &[
        "http://", "https://", "www.", "fetch", "download", "open", "visit", "go to", "load",
        "scrape", "page", "url", "site", "website", "link",
    ];
    let matched = m.contains("http://")
        || m.contains("https://")
        || m.contains("www.")
        || has_web_domain(&m)
        || (contains_any_word(
            &m,
            &[
                "fetch", "download", "open", "visit", "go to", "load", "scrape",
            ],
        ) && contains_any_word(&m, &["page", "url", "site", "website", "link"]));
    matched && !is_negated(message, KW)
}

/// True when recent conversation shows an ACTIVE note-editing thread, so a
/// follow-up correction that carries no fresh verb ("no, that's wrong", "you
/// didn't do it", a typo'd "formate it") should still get write_note. Without
/// this, per-message gating strips the tool on those turns and the model can
/// only claim success in chat — the "New note 18" bug. Looks back over the last
/// few user turns for any write intent. Pass recent USER messages (any order).
pub fn in_edit_thread(recent_user_messages: &[&str]) -> bool {
    recent_user_messages
        .iter()
        .rev()
        .take(4)
        .any(|m| note_write_intent(m))
}

/// Deterministically classify whether a message requests an operation/tool.
/// This mirrors the routing predicates used by `select_tools_cfg`, allowing the
/// host to avoid a second model inference for the same decision.
pub fn tool_intent(message: &str, edit_thread: bool) -> bool {
    note_write_intent(message)
        || edit_thread
        || wants_other_notes(message)
        || wants_search(message)
        || wants_documents(message)
        || wants_fetch(message)
        || wants_find(message)
}

/// Tools available in Chat mode are read-only. Mutation wording such as
/// "rewrite the introduction" asks for a prose draft in Chat; it must not force
/// a tool call when no write tool is present. Actual mutations are routed by
/// Operation/Edit mode instead.
pub fn chat_tool_intent(message: &str) -> bool {
    wants_other_notes(message)
        || wants_search(message)
        || wants_documents(message)
        || wants_fetch(message)
        || wants_find(message)
}

/// Per-message tool gating: hand the model ONLY the tools its message warrants,
/// so the model can't misfire on one it was never given. write_note is
/// the primary action (the open note is the workspace); search_notes/read_note
/// and fetch_web_page are opt-in by intent; small talk gets nothing. When
/// `edit_thread` is set, write_note stays available even without a fresh verb
/// so follow-up corrections keep editing the note.
pub fn select_tools(message: &str, has_open_note: bool, edit_thread: bool) -> Vec<Value> {
    select_tools_cfg(message, has_open_note, edit_thread, true, true)
}

/// Chat always receives one fixed read-only schema. Question-dependent schemas
/// change the rendered prompt prefix and defeat llama-server KV reuse.
pub fn select_chat_tools(_message: &str, _has_open_note: bool) -> Vec<Value> {
    specs_for(&[
        "read_note",
        "fetch_web_page",
        "web_search",
        "search_documents",
        "find_in_note",
        "search_notes",
    ])
}

/// Stable tool schema for both real turns and synthetic prefix warm-up.
///
/// The frontend currently exposes only chat and edit. Keep operation and auto
/// compatible with the full schema for external callers and a future composer
/// mode toggle.
pub fn interaction_mode_tools(mode: &str, oversized: bool) -> Vec<Value> {
    match mode {
        // Chat always renders one fixed read-only set — even for oversized
        // notes — so the synthetic warm-up prefix is byte-identical to the
        // real chat turn and its persisted KV snapshot stays reusable.
        "chat" => select_chat_tools("", true),
        "edit" => specs_for(&["write_note"]),
        _ if oversized => tool_specs()
            .into_iter()
            .filter(|tool| {
                !matches!(
                    tool["function"]["name"].as_str(),
                    Some("write_note" | "format_note")
                )
            })
            .collect(),
        _ => tool_specs(),
    }
}

/// Filter the full tool spec list down to a set of tool names.
fn specs_for(names: &[&str]) -> Vec<Value> {
    tool_specs()
        .into_iter()
        .filter(|t| {
            t["function"]["name"]
                .as_str()
                .map(|n| names.contains(&n))
                .unwrap_or(false)
        })
        .collect()
}

/// Like [`select_tools`], but with the two assist layers toggled independently:
///
/// - `gating` — per-message tool gating: hand the model only the tools its
///   message warrants. Off → the full general tool set is offered every turn and
///   the model chooses for itself (suited to larger, more capable models).
/// - `deterministic` — the deterministic correctness tools: route structural
///   cleanups to the regex `format_note` tool (instead of an LLM rewrite) and
///   word lookups to `find_in_note`. These are *correctness* assists, not a
///   gating crutch, so they apply whether or not gating is on.
pub fn select_tools_cfg(
    message: &str,
    has_open_note: bool,
    edit_thread: bool,
    gating: bool,
    deterministic: bool,
) -> Vec<Value> {
    // Deterministic format override (independent of gating): a clean whole-doc
    // structural cleanup (remove all headings/bold/bullets) goes to the regex
    // format_note tool, exclusively, so the model can't fumble the rewrite —
    // echo the note back unchanged, or empty it. Regex beats an LLM rewrite at
    // this for any model size, which is why it sits above gating.
    if deterministic && has_open_note && detect_format_op(message).is_some() {
        return specs_for(&["format_note"]);
    }

    // Fresh whole-note creation does not need the model to choose between
    // several mutation strategies.  Before the specialized append/insert/
    // delete tools were introduced, requests such as "write a poem" exposed
    // only write_note; retain that reliable contract for creation requests.
    // Requests that need the existing body (rewrite, format, targeted edits,
    // etc.) continue through the broader mutation routing below.
    let fresh_whole_note = has_open_note
        && note_write_intent(message)
        && !append_request_intent(message)
        && !existing_note_operation(message);

    // Gating off: offer the full general tool set every turn and let the model
    // decide. Read/search tools are harmless on a misfire, so they're always on
    // (this is what keeps web search working — gating's brittle keyword routing
    // was the thing that broke it). The DESTRUCTIVE write tool is the exception:
    // it still needs edit intent, so a small model can't misfire it on a question
    // or greeting and clobber the note (the "what can you do" → wrote the title
    // bug). find_in_note rides along when the deterministic layer is on.
    if !gating {
        let mut names = vec![
            "search_notes",
            "read_note",
            "search_documents",
            "fetch_web_page",
            "web_search",
        ];
        if has_open_note && (note_write_intent(message) || edit_thread) {
            names.push("write_note");
            if !fresh_whole_note {
                names.push("append_note");
                names.push("prepend_note");
                names.push("replace_in_note");
                names.push("insert_after_line");
                names.push("delete_in_note");
            }
        }
        if deterministic && has_open_note && wants_find(message) {
            names.push("find_in_note");
        }
        return specs_for(&names);
    }

    // Gating on: hand the model ONLY the tools its message warrants.
    if is_small_talk(message) {
        return Vec::new();
    }
    let mut names: Vec<&str> = Vec::new();
    // detect_format_op is included so a format request still gets write_note when
    // the deterministic format path is OFF (when it's on, we returned above).
    if has_open_note
        && (note_write_intent(message) || edit_thread || detect_format_op(message).is_some())
    {
        names.push("write_note");
        if !fresh_whole_note {
            names.push("append_note");
            names.push("prepend_note");
            names.push("replace_in_note");
            names.push("insert_after_line");
            names.push("delete_in_note");
        }
    }
    if wants_other_notes(message) {
        names.push("search_notes");
        names.push("read_note");
    }
    if wants_search(message) {
        names.push("web_search");
        names.push("fetch_web_page"); // so it can open a result it found
    }
    if wants_documents(message) {
        names.push("search_documents");
    }
    if deterministic && has_open_note && wants_find(message) {
        names.push("find_in_note");
    }
    if wants_fetch(message) {
        names.push("fetch_web_page");
    }
    specs_for(&names)
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
        if let Some(open_id) = self.state.current_note_id() {
            if let Ok(open_note) = self.state.load_note(open_id.clone()).await {
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

#[derive(Deserialize, JsonSchema)]
pub struct ReplaceInNoteArgs {
    /// The exact existing text in the note to find and replace.
    find: String,
    /// The new text to put in its place. Empty string deletes the matched text.
    replace: String,
}

#[derive(Clone)]
pub struct ReplaceInNoteTool {
    pub state: AppState,
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
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to edit.".to_string());
            }
        };

        let replacement = clean_note_content(&strip_prompt_markers(&args.replace));

        match find_tolerant(&existing.body, &args.find) {
            Some((start, end)) => {
                let new_body = format!("{}{}{}", &existing.body[..start], replacement, &existing.body[end..]);

                let display_name = if replacement.trim().is_empty() { "Delete Text" } else { "Replace Text" };
                let preview = format!("Find:\n{}\n\nReplace with:\n{replacement}", args.find);
                if let Err(msg) =
                    check_tool_approval(&self.state, display_name, &existing.title, &preview).await
                {
                    return Ok(msg);
                }
                self.state
                    .record_chat_tool(display_name, existing.title.clone());
                let _ = self.state.handle.emit(
                    "ai://chat_tool",
                    serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }),
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
    pub state: AppState,
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
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to edit.".to_string());
            }
        };

        let content = clean_note_content(&strip_prompt_markers(&args.content));
        let body = &existing.body;

        // An armed selection is authoritative: placement may not target an
        // unrelated model-supplied marker.
        if let Some(sel) = self.state.current_selection() {
            let plan = selection_insert_after_plan(body, &args.marker, &content, &sel)
                .map_err(|message| ToolError { message })?;
            let display_name = "Insert After";
            let preview = format!("Insert after '{}':\n\n{content}", args.marker);
            if let Err(msg) = check_tool_approval(&self.state, display_name, &existing.title, &preview).await {
                return Ok(msg);
            }
            self.state.record_chat_tool(display_name, existing.title.clone());
            let _ = self.state.handle.emit("ai://chat_tool", serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }));
            self.state.save_note(existing.id.clone(), existing.title, existing.tags, plan.new_body.clone(), existing.source_pdf, Some(existing.annotations)).await.map_err(|e| ToolError { message: e.to_string() })?;
            let _ = self.state.handle.emit("ai://note_written", serde_json::json!({ "noteId": existing.id, "content": plan.new_body, "mode": "write" }));
            return Ok(format!("Note successfully updated with ID: {}", existing.id));
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
            check_tool_approval(&self.state, display_name, &existing.title, &preview).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }),
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
pub struct DeleteInNoteArgs {
    /// The exact text in the note to delete.
    target: String,
}

#[derive(Clone)]
pub struct DeleteInNoteTool {
    pub state: AppState,
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
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to edit.".to_string());
            }
        };

        match find_tolerant(&existing.body, &args.target) {
            Some((start, end)) => {
                let new_body = format!("{}{}", &existing.body[..start], &existing.body[end..]);

                if self.state.deterministic_tools_enabled()
                    && new_body.trim().is_empty()
                    && !existing.body.trim().is_empty()
                    && !wants_clear(&self.state.latest_chat_question())
                {
                    return Ok(
                        "Refused: that would erase the entire note. Use write_note with empty content if you intend to clear it.".to_string(),
                    );
                }

                let display_name = "Delete Text";
                let preview = format!("Delete:\n{}", args.target);
                if let Err(msg) =
                    check_tool_approval(&self.state, display_name, &existing.title, &preview).await
                {
                    return Ok(msg);
                }
                self.state
                    .record_chat_tool(display_name, existing.title.clone());
                let _ = self.state.handle.emit(
                    "ai://chat_tool",
                    serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{preview}", existing.title), "mutatesNote": true }),
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
    pub state: AppState,
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
        } else if let Some(detected) = detect_format_op(&self.state.latest_chat_question()) {
            detected.to_string()
        } else {
            return Ok(format!(
                "Refused: '{requested}' is not a supported format operation. Supported operations: {}.",
                FORMAT_OPS.join(", ")
            ));
        };

        let existing = match self.state.resolve_chat_target_note("") {
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
            check_tool_approval(&self.state, display_name, &existing.title, &new_body).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", existing.title, pretty), "mutatesNote": true }),
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

fn edit_notebook_params() -> Value {
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
        let (_, description, params) = tool_contract("fetch_web_page").expect("fetch_web_page contract");
        ToolDefinition {
            name: "fetch_web_page".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let url = normalize_web_url(&args.url).map_err(|message| ToolError { message })?;
        // SSRF guard: never fetch loopback/private/link-local addresses (the
        // user's own machine or LAN), checked again on every redirect hop.
        let resolved_addr = crate::web_search::resolve_public_url(&url)
            .await
            .map_err(|message| ToolError { message })?;
        self.state.record_chat_tool("Fetch Web Page", url.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Fetch Web Page", "details": url }),
        );

        // Pin the initial hostname to the already-validated public address and
        // disable redirects. Following redirects would require an async DNS
        // validation step for every hop; refusing them keeps the guarantee
        // between validation and connection establishment explicit.
        let mut client_builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(6))
            .timeout(std::time::Duration::from_secs(20))
            .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(addr) = resolved_addr {
            if let Some(host) = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_string)) {
                client_builder = client_builder.resolve(&host, addr);
            }
        }
        let client = client_builder.build()
            .map_err(|e| ToolError {
                message: format!("Failed to build web client: {e}"),
            })?;

        let response = client
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

        // Cap the raw body read so a huge page cannot fill memory before the
        // 6,000-character text excerpt is extracted below.
        let mut body_bytes: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while body_bytes.len() < WEB_BODY_CAP {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    let take = (WEB_BODY_CAP - body_bytes.len()).min(chunk.len());
                    body_bytes.extend_from_slice(&chunk[..take]);
                }
                Some(Err(e)) => {
                    return Err(ToolError {
                        message: format!("Failed to read response from {url}: {e}"),
                    })
                }
                None => break,
            }
        }
        let body = String::from_utf8_lossy(&body_bytes);
        let text = html_to_text(&body);
        if text.trim().is_empty() {
            Ok(format!("Fetched {url}, but no readable text was found."))
        } else {
            Ok(text.chars().take(WEB_FETCH_LIMIT).collect())
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchDocumentsArgs {
    query: String,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    doc_id: Option<String>,
}

#[derive(Clone)]
pub struct SearchDocumentsTool {
    pub state: AppState,
}

impl Tool for SearchDocumentsTool {
    const NAME: &'static str = "search_documents";

    type Error = ToolError;
    type Args = SearchDocumentsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("search_documents").expect("search_documents contract");
        ToolDefinition {
            name: "search_documents".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let k = args.count.unwrap_or(5).clamp(1, 10) as usize;
        let scope = self.state.current_attachment_scope();
        if scope.is_empty() {
            return Ok("Document search is unavailable because no document is authorized for the active AI workspace.".to_string());
        }
        let scoped_ids = match args.doc_id {
            Some(id) if scope.contains(&id) => vec![id],
            Some(_) => {
                return Ok("That document is outside the active note and attachment scope.".to_string())
            }
            None => scope,
        };
        self.state
            .record_chat_tool("Search Documents", args.query.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Search Documents", "details": args.query.clone() }),
        );
        match self
            .state
            .retrieve_chunks_scoped(&args.query, k, Some(&scoped_ids))
            .await
        {
            Ok(chunks) if !chunks.is_empty() => {
                let mut out = format!("Passages from your documents for \"{}\":\n\n", args.query);
                for (i, c) in chunks.iter().enumerate() {
                    out.push_str(&format!("{}. [{}]\n{}\n\n", i + 1, c.source, c.text.trim()));
                }
                Ok(out)
            }
            Ok(_) => Ok("No relevant passages found in your documents.".to_string()),
            Err(e) => Ok(format!("Document search failed: {e}")),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct FindInNoteArgs {
    query: String,
}

#[derive(Clone)]
pub struct FindInNoteTool {
    pub state: AppState,
}

impl Tool for FindInNoteTool {
    const NAME: &'static str = "find_in_note";

    type Error = ToolError;
    type Args = FindInNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("find_in_note").expect("find_in_note contract");
        ToolDefinition {
            name: "find_in_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let q = args.query.trim().to_string();
        self.state.record_chat_tool("Find in Note", q.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Find in Note", "details": q.clone() }),
        );
        if q.is_empty() {
            return Ok("No search term was given.".to_string());
        }
        let body = self.state.open_note_body().unwrap_or_default();
        // Whole-word (boundary) match, not substring: "fix" must not hit
        // "prefix" and "add" must not hit "address".
        let pattern = format!(r"(?i)\b{}\b", regex::escape(&q));
        let count = regex::Regex::new(&pattern)
            .map(|re| re.find_iter(&body).count())
            .unwrap_or_else(|_| 0);
        if count == 0 {
            Ok(format!(
                "The text \"{q}\" does NOT appear in the open note."
            ))
        } else {
            Ok(format!(
                "Yes — \"{q}\" appears {count} time(s) in the open note."
            ))
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    query: String,
    #[serde(default)]
    count: Option<u32>,
}

#[derive(Clone)]
pub struct WebSearchTool {
    pub state: AppState,
}

impl Tool for WebSearchTool {
    const NAME: &'static str = "web_search";

    type Error = ToolError;
    type Args = WebSearchArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("web_search").expect("web_search contract");
        ToolDefinition {
            name: "web_search".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let count = args.count.unwrap_or(5).clamp(1, 10) as usize;
        self.state
            .record_chat_tool("Web Search", args.query.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Web Search", "details": args.query.clone() }),
        );
        let searxng = self.state.searxng_url();
        match crate::web_search::web_search(&args.query, count, searxng.as_deref()).await {
            Ok(results) => Ok(crate::web_search::format_results(&args.query, &results)),
            Err(e) => Ok(format!("Web search failed: {e}")),
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
        let (_, description, params) = tool_contract("search_notes").expect("search_notes contract");
        ToolDefinition {
            name: "search_notes".to_string(),
            description: description.to_string(),
            parameters: params(),
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
        // Low temperature keeps the model decisive and on-task instead of
        // rambling or asking the same clarifying question repeatedly.
        .temperature(temperature)
        .default_max_turns(max_turns)
        .tool(ReadNoteTool {
            state: state.clone(),
        })
        .tool(WriteNoteTool {
            state: state.clone(),
        })
        .tool(AppendNoteTool {
            state: state.clone(),
        })
        .tool(PrependNoteTool {
            state: state.clone(),
        })
        .tool(ReplaceInNoteTool {
            state: state.clone(),
        })
        .tool(InsertAfterLineTool {
            state: state.clone(),
        })
        .tool(DeleteInNoteTool {
            state: state.clone(),
        })
        .tool(FormatNoteTool {
            state: state.clone(),
        })
        .tool(FetchWebPageTool {
            state: state.clone(),
        })
        .tool(WebSearchTool {
            state: state.clone(),
        })
        .tool(SearchDocumentsTool {
            state: state.clone(),
        })
        .tool(FindInNoteTool {
            state: state.clone(),
        })
        .tool(SearchNotesTool {
            state: state.clone(),
        })
        .tool(EditNotebookTool { state })
        .build()
}

pub fn normalize_web_url(raw: &str) -> Result<String, String> {
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

pub fn html_to_text(raw: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "Cars are fast. They have engines. People drive them daily.";

    #[test]
    fn tool_specs_matches_contract_table() {
        let specs = tool_specs();
        assert_eq!(specs.len(), TOOL_CONTRACTS.len(), "every contract has a spec");
        for (name, _, _) in TOOL_CONTRACTS {
            assert!(
                specs.iter().any(|s| s["function"]["name"] == *name),
                "missing spec for {name}"
            );
        }
        // The wire schema must expose doc_id so the model can scope a search.
        let sd = specs
            .iter()
            .find(|s| s["function"]["name"] == "search_documents")
            .expect("search_documents spec");
        assert!(
            sd["function"]["parameters"]["properties"].get("doc_id").is_some(),
            "search_documents must advertise doc_id"
        );
    }

    #[test]
    fn negation_matching_ignores_words_that_contain_not() {
        // "note"/"know" contain "not"/"no" as substrings — must NOT read as negation.
        assert!(wants_find("does this note contain aardvark?"));
        assert!(!wants_find("do not find anything in the note"));
    }

    #[test]
    fn clean_note_content_converts_repeated_asterisk_line_separators() {
        let malformed = "**Beneath the sky, the river flows,** *Where stones whisper secrets, soft and low.* *The wind holds still, the world is calm,** *A mirrored path where shadows call.* *In twilight's hush, the stars align,** *A quiet night, a sacred sign.*";
        assert_eq!(
            clean_note_content(malformed),
            "**Beneath the sky, the river flows,**\nWhere stones whisper secrets, soft and low.\nThe wind holds still, the world is calm,\nA mirrored path where shadows call.\nIn twilight's hush, the stars align,\nA quiet night, a sacred sign."
        );
    }

    #[test]
    fn clean_note_content_preserves_ordinary_markdown_emphasis() {
        let ordinary = "This is *italic* and **bold**.";
        assert_eq!(clean_note_content(ordinary), ordinary);
    }

    #[test]
    fn generated_note_protocol_residue_is_rejected_without_false_positive() {
        assert!(note_content_has_protocol_residue(
            "# Essay\nUseful prose.\n/content>} > write_note(content="
        ));
        assert!(!note_content_has_protocol_residue(
            "# API\nCall `write_note(content)` in this example."
        ));
    }

    #[test]
    fn clean_note_content_normalizes_break_variants() {
        assert_eq!(
            clean_note_content("First<br>Second Third\\nFourth"),
            "First\nSecond\nThird\nFourth"
        );
    }

    // The exact bug from the live probe: the model labels a whole-note rewrite as
    // mode "edit" and sends NO `find`. That must be treated as a replace.
    #[test]
    fn edit_mode_without_find_is_a_replace() {
        let plan = plan_write(NOTE, "## Cars\nThey are fast.", "edit", "").unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "## Cars\nThey are fast.");
    }

    #[test]
    fn default_mode_replaces_whole_body() {
        let plan = plan_write(NOTE, "brand new body", "replace", "").unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "brand new body");
    }

    // The "deleted the entire note" bug: "remove all headings" must NOT read as a
    // request to clear the note (the destructive-write guard relies on this), but
    // an explicit wipe must.
    // The exact case the 1B model failed: "remove all headings" must keep every
    // line and drop only the leading # markers — done in code, not by the model.
    #[test]
    fn format_op_strips_headings_keeps_text() {
        let body =
            "## Intro\nPersonal computers changed everything.\n### History\nIt began in the 1970s.";
        assert_eq!(
            detect_format_op("remove all headings"),
            Some("remove_headings")
        );
        assert_eq!(
            apply_format_op(body, "remove_headings"),
            "Intro\nPersonal computers changed everything.\nHistory\nIt began in the 1970s."
        );
        assert_eq!(detect_format_op("make the title a heading"), None);
    }

    #[test]
    fn format_op_removals() {
        assert_eq!(
            apply_format_op("a **bold** word", "remove_bold"),
            "a bold word"
        );
        assert_eq!(
            apply_format_op("a *italic* word", "remove_italic"),
            "a italic word"
        );
        // Italic-only must leave bold markers intact.
        assert_eq!(
            apply_format_op("**b** and *i*", "remove_italic"),
            "**b** and i"
        );
        assert_eq!(
            apply_format_op("**b** and *i*", "remove_emphasis"),
            "b and i"
        );
        assert_eq!(
            apply_format_op("- one\n- two", "remove_bullets"),
            "one\ntwo"
        );
        assert_eq!(
            apply_format_op("1. one\n2. two", "remove_numbering"),
            "one\ntwo"
        );
        assert_eq!(
            apply_format_op("see [Rust](https://r.org) here", "remove_links"),
            "see Rust here"
        );
        // remove_links keeps images.
        assert_eq!(
            apply_format_op("![p](a.png) and [x](y)", "remove_links"),
            "![p](a.png) and x"
        );
        assert_eq!(
            apply_format_op("![p](a.png) text", "remove_images"),
            " text"
        );
        assert_eq!(
            apply_format_op("use `code` now", "remove_code"),
            "use code now"
        );
        assert_eq!(
            apply_format_op("> quoted\n> more", "remove_blockquotes"),
            "quoted\nmore"
        );
        assert_eq!(
            apply_format_op("a ~~no~~ b", "remove_strikethrough"),
            "a no b"
        );
        assert_eq!(
            apply_format_op("x\n\n---\n\ny", "remove_horizontal_rules"),
            "x\n\n\ny"
        );
        assert_eq!(
            apply_format_op("a\n\n\n\nb", "remove_blank_lines"),
            "a\n\nb"
        );
        assert_eq!(
            apply_format_op("# H\n- **b** [l](u)", "strip_markdown"),
            "H\nb l"
        );
    }

    #[test]
    fn format_op_conversions() {
        assert_eq!(
            apply_format_op("# Title\nbody", "headings_to_bold"),
            "**Title**\nbody"
        );
        assert_eq!(
            apply_format_op("**Title**\nbody", "bold_to_headings"),
            "# Title\nbody"
        );
        assert_eq!(apply_format_op("## A\n# B", "promote_headings"), "# A\n# B");
        assert_eq!(
            apply_format_op("# A\n## B", "demote_headings"),
            "## A\n### B"
        );
        assert_eq!(
            apply_format_op("- a\n- b\n- c", "bullets_to_numbered"),
            "1. a\n2. b\n3. c"
        );
        assert_eq!(
            apply_format_op("1. a\n2. b", "numbered_to_bullets"),
            "- a\n- b"
        );
        assert_eq!(
            apply_format_op("- [ ] todo\n- [x] done", "tasks_to_bullets"),
            "- todo\n- done"
        );
        assert_eq!(apply_format_op("Hi There", "uppercase"), "HI THERE");
        assert_eq!(apply_format_op("Hi There", "lowercase"), "hi there");
        assert_eq!(apply_format_op("hello world", "title_case"), "Hello World");
    }

    #[test]
    fn detect_format_op_routes_and_guards() {
        assert_eq!(detect_format_op("strip the bold"), Some("remove_bold"));
        assert_eq!(
            detect_format_op("get rid of the bullet points"),
            Some("remove_bullets")
        );
        assert_eq!(detect_format_op("remove the links"), Some("remove_links"));
        assert_eq!(
            detect_format_op("remove all the images"),
            Some("remove_images")
        );
        assert_eq!(
            detect_format_op("strip all formatting"),
            Some("strip_markdown")
        );
        assert_eq!(detect_format_op("make it all uppercase"), Some("uppercase"));
        assert_eq!(
            detect_format_op("convert the bullets to a numbered list"),
            Some("bullets_to_numbered")
        );
        assert_eq!(
            detect_format_op("turn the numbered list into bullets"),
            Some("numbered_to_bullets")
        );
        assert_eq!(
            detect_format_op("change the headings to bold"),
            Some("headings_to_bold")
        );
        // Never hijack a request to CREATE fresh content.
        assert_eq!(detect_format_op("write a numbered list of fruits"), None);
        assert_eq!(detect_format_op("write this note in uppercase"), None);
        assert_eq!(detect_format_op("make the title a heading"), None);
        // Every op the detector returns must be applicable.
        for op in FORMAT_OPS {
            assert_eq!(apply_format_op("unchanged", "bogus_op"), "unchanged");
            assert!(is_format_op(op));
        }
    }

    #[test]
    fn wants_clear_is_narrow() {
        assert!(!wants_clear("remove all headings"));
        assert!(!wants_clear("remove the bullet points"));
        assert!(!wants_clear("delete the introduction"));
        assert!(wants_clear("clear the note"));
        assert!(wants_clear("delete everything"));
        assert!(wants_clear("erase the note and start over"));
        assert!(wants_clear("make it blank"));
    }

    #[test]
    fn wants_partial_removal_pure_deletes_only() {
        // Pure removals of a part → true.
        assert!(wants_partial_removal("remove the My Take section"));
        assert!(wants_partial_removal("delete the second paragraph"));
        assert!(wants_partial_removal("get rid of the bulleted list"));
        assert!(wants_partial_removal("take out the heading"));
        // Whole-note clears are handled elsewhere → false.
        assert!(!wants_partial_removal("delete everything"));
        assert!(!wants_partial_removal("clear the note"));
        // Removal mixed with new/changed content is a real edit → false.
        assert!(!wants_partial_removal(
            "delete the intro and add a conclusion"
        ));
        assert!(!wants_partial_removal("replace the first paragraph"));
        assert!(!wants_partial_removal(
            "rewrite the summary without the last line"
        ));
        // Non-removal requests → false.
        assert!(!wants_partial_removal("make the title a heading"));
        assert!(!wants_partial_removal("what does this note say"));
    }

    #[test]
    fn surgical_delete_removes_only_find_from_real_body() {
        // The path the override produces: mode "edit", empty content, find = the
        // block to remove. plan_write must delete exactly that, byte-faithfully.
        let body = "# Title\n\nKeep this paragraph.\n\n**My Take:**\nRemove me.";
        let plan = plan_write(body, "", "edit", "**My Take:**\nRemove me.").unwrap();
        assert_eq!(plan.op, WriteOp::EditSnippet);
        assert_eq!(plan.new_body, "# Title\n\nKeep this paragraph.\n\n");
    }

    #[test]
    fn gating_off_protects_write_note_but_keeps_search() {
        let has = |v: &[Value], name: &str| {
            v.iter()
                .any(|t| t["function"]["name"].as_str() == Some(name))
        };
        // gating OFF (default), deterministic ON.
        // A capability question must NOT get write_note (the "what can you do →
        // wrote the title" misfire), but read/search stay available.
        let q = select_tools_cfg("what can you do", true, false, false, true);
        assert!(
            !has(&q, "write_note"),
            "a question must not offer write_note"
        );
        assert!(has(&q, "web_search"), "read/search stay on with gating off");
        // An explicit edit request still gets write_note.
        let e = select_tools_cfg("rewrite the introduction", true, false, false, true);
        assert!(has(&e, "write_note"));
        // A verb-less follow-up in an edit thread keeps write_note.
        let f = select_tools_cfg("no, shorter", true, true, false, true);
        assert!(has(&f, "write_note"));
        // Web search is never withheld (the thing brittle gating used to break).
        let s = select_tools_cfg(
            "search the web for the latest rust release",
            true,
            false,
            false,
            true,
        );
        assert!(has(&s, "web_search"));
        assert!(!has(&s, "write_note"));
    }

    #[test]
    fn fresh_whole_note_creation_uses_only_write_note() {
        let has = |v: &[Value], name: &str| {
            v.iter()
                .any(|t| t["function"]["name"].as_str() == Some(name))
        };

        for tools in [
            select_tools_cfg("write a poem", true, false, true, true),
            select_tools_cfg("write a poem", true, false, false, true),
        ] {
            assert!(has(&tools, "write_note"));
            for mutation in [
                "append_note",
                "prepend_note",
                "replace_in_note",
                "insert_after_line",
                "delete_in_note",
            ] {
                assert!(!has(&tools, mutation), "unexpected tool: {mutation}");
            }
        }

        // Existing-note rewrites still need the broader edit choices.
        let rewrite = select_tools_cfg("rewrite the introduction", true, false, true, true);
        assert!(has(&rewrite, "write_note"));
        assert!(has(&rewrite, "replace_in_note"));

        // Additions still expose the append path; the state-layer append-only
        // filter removes the overwrite and targeted-edit tools before dispatch.
        let append = select_tools_cfg("add a poem", true, false, true, true);
        assert!(has(&append, "append_note"));
    }

    #[test]
    fn locate_selection_disambiguates_repeats_via_context() {
        let body = "Cats are nice.\n\nDogs are loyal.\n\nCats are nice.";
        // The phrase occurs twice; the `before` anchor pins the SECOND one.
        let sel = SelectionArg {
            text: "Cats are nice.".into(),
            before: "loyal.\n\n".into(),
            after: "".into(),
            cursor: false,
            cell_index: None,
        };
        let (s, e) = locate_selection(body, &sel).unwrap();
        assert_eq!(s, body.rfind("Cats are nice.").unwrap());
        assert_eq!(&body[s..e], "Cats are nice.");
    }

    #[test]
    fn locate_selection_tolerates_markers_and_trailing_whitespace() {
        // Real failure from "New note 8": the user selected a BOLD paragraph, so
        // the captured text dropped the ** markers and ran past with newlines.
        let body = "# Food\n\n## Intro\n\n**Food is essential and good.**\n\n## More\n\n- a";
        let sel = SelectionArg {
            text: "Food is essential and good.\n\n".into(),
            before: "Intro\n\n**".into(),
            after: "**\n\n## More".into(),
            cursor: false,
            cell_index: None,
        };
        let (s, e) = locate_selection(body, &sel).unwrap();
        assert_eq!(&body[s..e], "Food is essential and good.");
        // Splicing keeps the surrounding ** and the rest of the note.
        let plan =
            selection_scoped_plan(body, "Food is vital, nutritious, and cultural.", &sel).unwrap();
        assert_eq!(
            plan.new_body,
            "# Food\n\n## Intro\n\n**Food is vital, nutritious, and cultural.**\n\n## More\n\n- a"
        );
    }

    #[test]
    fn selection_scoped_plan_replaces_only_the_selected_span() {
        let body = "Intro line.\n\nOld paragraph here.\n\nClosing line.";
        let sel = SelectionArg {
            text: "Old paragraph here.".into(),
            before: "Intro line.\n\n".into(),
            after: "\n\nClosing line.".into(),
            cursor: false,
            cell_index: None,
        };
        let plan = selection_scoped_plan(body, "New paragraph.", &sel).unwrap();
        assert_eq!(plan.op, WriteOp::EditSnippet);
        assert_eq!(
            plan.new_body,
            "Intro line.\n\nNew paragraph.\n\nClosing line."
        );
    }

    #[test]
    fn selection_scoped_plan_can_delete_only_the_selected_span() {
        let body = "Keep this.\n\nRemove this.\n\nKeep that.";
        let sel = SelectionArg {
            text: "Remove this.".into(),
            before: "Keep this.\n\n".into(),
            after: "\n\nKeep that.".into(),
            cursor: false,
            cell_index: None,
        };
        let plan = selection_scoped_plan(body, "", &sel).unwrap();
        assert_eq!(plan.new_body, "Keep this.\n\n\n\nKeep that.");
    }

    #[test]
    fn cursor_scoped_plan_inserts_at_the_unique_anchor() {
        let body = "Alpha beta.";
        let sel = SelectionArg {
            text: String::new(),
            before: "Alpha ".into(),
            after: "beta.".into(),
            cursor: true,
            cell_index: None,
        };
        let plan = selection_scoped_plan(body, "bright", &sel).unwrap();
        assert_eq!(plan.new_body, "Alpha bright beta.");
    }

    #[test]
    fn cursor_scoped_plan_rejects_ambiguous_anchor() {
        let sel = SelectionArg {
            text: String::new(),
            before: String::new(),
            after: String::new(),
            cursor: true,
            cell_index: None,
        };
        assert!(selection_scoped_plan("abc", "x", &sel).is_none());
    }

    #[test]
    fn cursor_scoped_plan_inserts_into_newline_normalized_empty_note() {
        let sel = SelectionArg {
            text: String::new(),
            before: "\n".to_string(),
            after: String::new(),
            cursor: true,
            cell_index: None,
        };
        let plan = selection_scoped_plan("\n", "A useful essay.", &sel).unwrap();
        assert_eq!(plan.new_body, "A useful essay.");
    }

    #[test]
    fn selection_scoped_plan_defers_when_model_regenerated_whole_note() {
        let body = "Intro line.\n\nOld paragraph here.\n\nClosing line.";
        let sel = SelectionArg {
            text: "Old paragraph here.".into(),
            before: "Intro line.\n\n".into(),
            after: "\n\nClosing line.".into(),
            cursor: false,
            cell_index: None,
        };
        // Model returned the WHOLE note (contains the after-anchor text) → fall
        // through to normal planning (None) instead of splicing the whole note in.
        let whole = "Intro line.\n\nNew paragraph.\n\nClosing line.";
        assert!(selection_scoped_plan(body, whole, &sel).is_none());
    }

    // Some models echo the prompt's note-framing markers into a direct tool call;
    // they must never reach the saved note.
    #[test]
    fn strips_echoed_prompt_markers_from_content() {
        let plan = plan_write(
            "The sky is blue today.",
            "--- CURRENT NOTE ---\nThe sky is green today.\n--- END CURRENT NOTE ---",
            "edit",
            "blue",
        )
        .unwrap();
        assert_eq!(plan.new_body, "The sky is green today.");
        assert!(!plan.new_body.contains("CURRENT NOTE"));
    }

    #[test]
    fn strips_malformed_marker_variants() {
        // Models emit dash/spacing variants — all must be stripped.
        assert_eq!(strip_prompt_markers("hi\n--- END CURRENT NOTE --"), "hi");
        assert_eq!(strip_prompt_markers("---CURRENT NOTE---\nbody"), "body");
        assert_eq!(
            strip_prompt_markers("body\n-- end current note ---"),
            "body"
        );
        assert_eq!(strip_prompt_markers("clean note"), "clean note");
        // Bled "--- Title" delimiter (dashes + text on one line) is stripped...
        assert_eq!(strip_prompt_markers("--- Example Domain"), "Example Domain");
        // ...but a real horizontal rule (dashes alone on a line) is preserved.
        assert_eq!(
            strip_prompt_markers("# Title\n\n---\nmore"),
            "# Title\n\n---\nmore"
        );
    }

    #[test]
    fn append_adds_to_end() {
        let plan = plan_write(NOTE, "A new line.", "append", "").unwrap();
        assert_eq!(plan.op, WriteOp::Append);
        assert!(plan.new_body.starts_with(NOTE));
        assert!(plan.new_body.ends_with("A new line."));
    }

    #[test]
    fn append_strips_echoed_note_and_tool_wrapper() {
        let payload = format!("{NOTE}\n\nMeaning: A reflection on motion.</content>,");
        let plan = plan_write(NOTE, &payload, "append", "").unwrap();
        assert_eq!(plan.op, WriteOp::Append);
        assert_eq!(
            plan.new_body,
            format!("{NOTE}\n\nMeaning: A reflection on motion.")
        );
    }

    #[test]
    fn find_replaces_only_the_snippet() {
        let plan = plan_write(NOTE, "slow", "edit", "fast").unwrap();
        assert_eq!(plan.op, WriteOp::EditSnippet);
        assert_eq!(
            plan.new_body,
            "Cars are slow. They have engines. People drive them daily."
        );
    }

    // Regression: the live harness caught the model sending mode:"replace" with
    // the full new sentence in `content` AND a stray find:"blue". An explicit
    // replace must use the full content and IGNORE find (not splice it in).
    #[test]
    fn explicit_replace_ignores_stray_find() {
        let plan = plan_write(
            "The sky is blue today.",
            "The sky is green today.",
            "replace",
            "blue",
        )
        .unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "The sky is green today.");
    }

    // A `find` with no explicit mode is a snippet edit (the model means to swap
    // just that text), so content is the replacement, not the whole body.
    #[test]
    fn find_without_mode_is_snippet_edit() {
        let plan = plan_write("The sky is blue.", "green", "", "blue").unwrap();
        assert_eq!(plan.op, WriteOp::EditSnippet);
        assert_eq!(plan.new_body, "The sky is green.");
    }

    // Regression: LFM sends find:"blue" but the WHOLE updated sentence as content.
    // Splicing would garble ("The sky is The sky is green today. today.") — detect
    // the absorbed surrounding text and treat it as a replace.
    #[test]
    fn find_with_full_sentence_content_replaces() {
        let plan = plan_write(
            "The sky is blue today.",
            "The sky is green today.",
            "edit",
            "blue",
        )
        .unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "The sky is green today.");
    }

    #[test]
    fn tool_gating_selects_by_intent() {
        // small talk → no tools
        assert!(select_tools("gg", true, false).is_empty());
        assert!(select_tools("thanks!", true, false).is_empty());
        // write intent → all note-mutation tools
        let w = select_tools("expand this to 500 words", true, false);
        assert!(w.len() >= 6, "expected at least 6 note tools, got {}", w.len());
        let has = |v: &[Value], name: &str| {
            v.iter()
                .any(|t| t["function"]["name"].as_str() == Some(name))
        };
        assert!(has(&w, "write_note"));
        assert!(has(&w, "append_note"));
        assert!(has(&w, "prepend_note"));
        assert!(has(&w, "replace_in_note"));
        assert!(has(&w, "insert_after_line"));
        assert!(has(&w, "delete_in_note"));
        // pure question → no tools (model answers in chat)
        assert!(select_tools("what is the capital of france?", true, false).is_empty());
        // other-notes intent → search + read
        let s = select_tools("search my other notes for cats", true, false);
        let names: Vec<&str> = s
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"search_notes"));
        assert!(names.contains(&"read_note"));
        // url → fetch
        let f = select_tools("fetch https://example.com", true, false);
        assert!(f.iter().any(|t| t["function"]["name"] == "fetch_web_page"));
    }

    #[test]
    fn chat_tools_are_relevant_and_never_mutating() {
        let names = |tools: Vec<Value>| {
            tools
                .into_iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };

        let expected = vec![
            "read_note",
            "fetch_web_page",
            "web_search",
            "search_documents",
            "find_in_note",
            "search_notes",
        ];
        for prompt in [
            "hello there",
            "rewrite the introduction",
            "search the web for current Rust news",
            "search my other notes for cats",
            "find the exact phrase neural net in this note",
        ] {
            assert_eq!(names(select_chat_tools(prompt, true)), expected);
        }

        for prompt in [
            "open https://example.com",
            "search my documents for the citation",
            "delete the note",
        ] {
            assert!(select_chat_tools(prompt, true).iter().all(|tool| {
                !matches!(
                    tool["function"]["name"].as_str(),
                    Some(
                        "write_note"
                            | "append_note"
                            | "prepend_note"
                            | "replace_in_note"
                            | "insert_after_line"
                            | "delete_in_note"
                            | "format_note"
                            | "edit_notebook"
                    )
                )
            }));
        }
    }

    #[test]
    fn edit_thread_keeps_write_on_verbless_corrections() {
        // A verb-less correction gets NO write_note cold...
        let cold = select_tools("no thats wrong", true, false);
        assert!(!cold.iter().any(|t| t["function"]["name"] == "write_note"));
        // ...but DOES inside an active edit thread.
        let warm = select_tools("no thats wrong", true, true);
        assert!(warm.iter().any(|t| t["function"]["name"] == "write_note"));
        // Small talk stays tool-free even in an edit thread.
        assert!(select_tools("thanks!", true, true).is_empty());
        // in_edit_thread fires when a recent user turn asked to write/edit.
        assert!(in_edit_thread(&[
            "write a note about cats",
            "no thats wrong"
        ]));
        assert!(!in_edit_thread(&["what is rust?", "who are you?"]));
    }

    #[test]
    fn small_talk_detection() {
        assert!(is_small_talk("hi"));
        assert!(is_small_talk("thanks so much"));
        assert!(!is_small_talk("write a note about cats"));
        assert!(!is_small_talk("what is the capital of france and why")); // > 6 words
    }

    #[test]
    fn fetch_gating_bare_domains_not_filenames() {
        // bare domain (no scheme) → fetch
        assert!(wants_fetch("summarize example.com"));
        assert!(wants_fetch("what's on speediq.ai"));
        assert!(wants_fetch("fetch https://x.org/page"));
        // file names are NOT web targets
        assert!(!wants_fetch("fix the typo in notes.md"));
        assert!(!wants_fetch("rename model.gguf"));
        assert!(!wants_fetch("just chatting about cats"));
        // and via select_tools: "summarize example.com" → fetch (not a write/clear)
        let t = select_tools("summarize example.com", true, false);
        assert!(t.iter().any(|x| x["function"]["name"] == "fetch_web_page"));
    }

    #[test]
    fn find_with_empty_content_deletes_snippet() {
        let plan = plan_write(NOTE, "", "edit", "They have engines. ").unwrap();
        assert_eq!(plan.new_body, "Cars are fast. People drive them daily.");
    }

    #[test]
    fn find_tolerates_whitespace_mismatch() {
        // Model reproduces the snippet with different internal whitespace.
        let plan = plan_write(NOTE, "X", "edit", "have   engines").unwrap();
        assert!(plan.new_body.contains("They X."));
    }

    #[test]
    fn find_not_present_is_refused_not_destructive() {
        let err = plan_write(NOTE, "x", "edit", "no such text here").unwrap_err();
        assert!(err.to_lowercase().contains("could not find"));
    }

    #[test]
    fn empty_replace_clears_the_note() {
        let plan = plan_write(NOTE, "", "replace", "").unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "");
    }

    #[test]
    fn find_tolerant_exact_and_normalized() {
        assert_eq!(find_tolerant("hello world", "world"), Some((6, 11)));
        assert!(find_tolerant("a  b   c", "a b c").is_some());
        assert!(find_tolerant("abc", "xyz").is_none());
    }

    #[test]
    fn normalize_url_adds_scheme_and_rejects_junk() {
        assert_eq!(
            normalize_web_url("example.com").unwrap(),
            "https://example.com"
        );
        assert_eq!(normalize_web_url("http://x.io").unwrap(), "http://x.io");
        assert!(normalize_web_url("   ").is_err());
        assert!(normalize_web_url("has space.com").is_err());
    }

    #[test]
    fn negation_prevents_false_positive_intents() {
        // "don't search notes" must NOT trigger wants_other_notes.
        assert!(!wants_other_notes("don't search notes for cats"));
        assert!(!wants_other_notes("i don't want to search my notes"));
        // "don't clear the note" must NOT trigger wants_clear.
        assert!(!wants_clear("don't clear the note"));
        assert!(!wants_clear("don't delete everything"));
        // "don't write a note" must NOT trigger note_write_intent.
        assert!(!note_write_intent("don't write a note about cats"));
        // "don't remove the headings" must NOT trigger detect_format_op.
        assert_eq!(detect_format_op("don't remove the headings"), None);
        // "don't remove the second item" must NOT trigger wants_partial_removal.
        assert!(!wants_partial_removal("don't remove the second item"));
        // "don't search the web" must NOT trigger wants_search.
        assert!(!wants_search("don't search the web for rust"));
        // "don't fetch the page" must NOT trigger wants_fetch.
        assert!(!wants_fetch("don't fetch https://example.com"));
        // "don't find the word" must NOT trigger wants_find.
        assert!(!wants_find("don't find the word cat in the note"));
        // "don't read the document" must NOT trigger wants_documents.
        assert!(!wants_documents("don't read the document about physics"));
    }

    #[test]
    fn non_negated_intents_still_match() {
        // Positive requests must still trigger their respective functions.
        assert!(wants_other_notes("search my notes for cats"));
        assert!(wants_clear("clear the note"));
        assert!(note_write_intent("write a note about cats"));
        assert_eq!(
            detect_format_op("remove all the headings"),
            Some("remove_headings")
        );
        assert!(wants_partial_removal("remove the second item"));
        assert!(wants_search("search the web for rust"));
        assert!(wants_fetch("fetch https://example.com"));
        assert!(wants_find("find the word cat in the note"));
        assert!(wants_documents("read the document about physics"));
    }

    #[test]
    fn small_talk_detects_longer_greetings() {
        // These previously failed the ≤4 word cap or missing-word check.
        assert!(is_small_talk("thanks for the help"));
        assert!(is_small_talk("how are you doing today"));
        assert!(is_small_talk("thanks so much"));
        assert!(is_small_talk("hey there"));
        // Real requests are still NOT small talk.
        assert!(!is_small_talk("write a note about cats"));
        assert!(!is_small_talk("what is the capital of france and why"));
        // Genuine questions with content words are not small talk even if short.
        assert!(!is_small_talk("how do i use this"));
    }

    #[test]
    fn html_to_text_strips_tags_and_scripts() {
        let html = "<html><head><style>x{}</style></head><body><h1>Hi</h1><script>bad()</script><p>world &amp; more</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Hi"));
        assert!(text.contains("world & more"));
        assert!(!text.contains("bad()"));
        assert!(!text.contains("<"));
    }

    #[test]
    fn write_intent_detects_edit_verbs() {
        for msg in [
            "write a poem about cars",
            "format this",
            "clean up the formatting",
            "remove the second paragraph",
            "make the intro shorter",
            "rewrite it more formally",
            "add a conclusion",
            "fix the spelling",
        ] {
            assert!(note_write_intent(msg), "expected write intent: {msg}");
        }
    }

    #[test]
    fn pure_note_write_excludes_research_workflows() {
        assert!(is_pure_note_write_request("write a poem on the note"));
        assert!(!is_pure_note_write_request(
            "search the web for Rust news and write the findings in my note"
        ));
        assert!(!is_pure_note_write_request(
            "read my other notes about Rust, then write a summary in this note"
        ));
        assert!(!is_pure_note_write_request(
            "fetch https://example.com and add a summary to the note"
        ));
    }

    #[test]
    fn write_intent_soft_verb_needs_note_target() {
        // "explain X" alone is a chat answer; "explain X in the note" is a write.
        assert!(!note_write_intent("explain what you are"));
        assert!(note_write_intent(
            "explain what you are in the note with an h1"
        ));
        assert!(note_write_intent("summarise this into the note"));
    }

    #[test]
    fn write_intent_affirmations_greenlight() {
        for msg in ["yes", "sure", "ok", "go ahead", "do it", "Yes please!"] {
            assert!(note_write_intent(msg), "expected affirmation: {msg}");
        }
    }

    #[test]
    fn write_intent_rejects_plain_questions() {
        for msg in [
            "what is the capital of France?",
            "who painted the mona lisa",
            "hi there",
            "thanks!",
            "describe the ocean",
        ] {
            assert!(!note_write_intent(msg), "expected no write intent: {msg}");
        }
    }

    #[test]
    fn write_intent_ignores_substring_false_positives() {
        // "address" contains "add", "prefix" contains "fix" — must not trigger.
        assert!(!note_write_intent("what is my ip address"));
        assert!(!note_write_intent("what does the prefix mean"));
    }
}
