use crate::state::AppState;
use rig_core::client::CompletionClient;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;

async fn check_tool_approval(state: &AppState, tool_name: &str, title: &str, content_preview: &str) -> Result<(), String> {
    if !state.is_tool_approval_required() {
        return Ok(());
    }
    let req_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.register_pending_approval(req_id.clone(), tx);

    let _ = state.handle.emit(
        "ai://tool_approval_request",
        serde_json::json!({
            "id": req_id,
            "tool": tool_name,
            "title": title,
            "content": content_preview
        }),
    );

    match rx.await {
        Ok(true) => Ok(()),
        Ok(false) => Err("User rejected this action.".to_string()),
        Err(_) => Err("Approval request cancelled.".to_string()),
    }
}

const WEB_FETCH_LIMIT: usize = 12_000;

/// System preamble for the note assistant. Kept as a single source of truth so
/// the startup cache warm-up replays the exact same prefix the live agent uses.
/// Deliberately small: a role line plus the minimum tool guidance needed to keep
/// a weak model from flooding the (memory-bounded) context with stray web/search
/// calls or describing edits in chat instead of writing them. Tool schemas are
/// still passed separately via `tool_specs` on every request.
pub const MYELIN_PREAMBLE: &str = "You are the assistant inside Myelin, a local notes app. The text of the note currently open in the editor is included in the user's message — you already have it.\n\n- To change the open note (write, rewrite, edit, format, add to, shorten, clear, etc.), call write_note with the full result. Don't just describe the change in chat — make it with the tool.\n- Write real Markdown. A heading is a line that starts with \"# \" (hash + space) for H1, \"## \" for H2 — an H1 is a #-prefixed line, never just bold text. Use \"**text**\" for bold and \"- \" for bullets. Do not copy the \"--- CURRENT NOTE ---\" markers into the note.\n- Use fetch_web_page only when the user gives a URL or web address (like example.com), and search_notes only when the user asks about your other notes. For greetings or general questions, just reply briefly — do not read, search, or fetch.";

/// OpenAI-format tool definitions mirroring the live agent's tools, in the same
/// order they are registered in [`build_myelin_agent`]. Used only by the startup
/// warm-up request so its cached system+tools prefix matches the live agent's.
/// Keep name/description/parameters in sync with each `Tool::definition` below.
pub fn tool_specs() -> Vec<Value> {
    let spec = |name: &str, description: &str, parameters: Value| {
        serde_json::json!({
            "type": "function",
            "function": { "name": name, "description": description, "parameters": parameters }
        })
    };
    vec![
        spec(
            "read_note",
            "Read the full Markdown of ANOTHER note by its id (ids come from search_notes). Do NOT use this for the note currently open in the editor — that note's content is already provided in the prompt below.",
            serde_json::json!({
                "type": "object",
                "properties": { "note_id": { "type": "string", "description": "The id of a DIFFERENT note to read (from search_notes results), not the open note." } },
                "required": ["note_id"]
            }),
        ),
        spec(
            "write_note",
            "Edit the note currently OPEN in the editor — this tool only ever changes that one open note and NEVER creates a separate new note. It handles ANY request to write, create, draft, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, change, add to, or delete from the open note. `mode` selects the operation: \"replace\" (default) sets the ENTIRE note body to `content` (empty string clears it); \"append\" adds `content` to the end (send only the new text); \"edit\" replaces the exact `find` text with `content` (empty `content` deletes the match). Always put the real final Markdown in `content`, never a placeholder.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text payload. For replace/append it is the note body or the new text; for edit it is the replacement (empty string deletes the matched text). Never a placeholder like [insert poem here]." },
                    "mode": { "type": "string", "enum": ["replace", "append", "edit"], "description": "How to apply content: replace (default, whole body) | append (add to end) | edit (swap the `find` snippet)." },
                    "find": { "type": "string", "description": "Required only when mode is \"edit\": the exact existing text in the note to replace or delete." }
                },
                "required": ["content"]
            }),
        ),
        spec(
            "fetch_web_page",
            "Fetch the text content of a public web page. Use this when the user asks to visit, open, fetch, or get details from a URL or domain.",
            serde_json::json!({
                "type": "object",
                "properties": { "url": { "type": "string", "description": "The http(s) URL or domain to fetch." } },
                "required": ["url"]
            }),
        ),
        spec(
            "search_notes",
            "Search the ENTIRE workspace for OTHER notes containing specific keywords. Do NOT use this to search or modify the currently open note.",
            serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "The search keywords." } },
                "required": ["query"]
            }),
        ),
    ]
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

/// Locate a snippet to edit, tolerating the small mismatches a model makes when
/// it reproduces existing text: try an exact match, then a trimmed match, then a
/// whitespace-normalized match (the snippet's words separated by any run of
/// whitespace). Returns the byte span in `body` to replace.
fn find_tolerant(body: &str, find: &str) -> Option<(usize, usize)> {
    if let Some(i) = body.find(find) {
        return Some((i, i + find.len()));
    }
    let trimmed = find.trim();
    if !trimmed.is_empty() && trimmed.len() != find.len() {
        if let Some(i) = body.find(trimmed) {
            return Some((i, i + trimmed.len()));
        }
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let pattern = tokens
        .iter()
        .map(|t| regex::escape(t))
        .collect::<Vec<_>>()
        .join(r"\s+");
    let re = regex::Regex::new(&pattern).ok()?;
    re.find(body).map(|m| (m.start(), m.end()))
}

/// How the editor should apply a write (drives the streaming UI and chip label).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum WriteOp {
    Replace,
    Append,
    EditSnippet,
}

#[derive(Debug)]
pub struct WritePlan {
    pub new_body: String,
    pub op: WriteOp,
}

/// Pure decision for `write_note`: given the note's current body and the tool
/// args, produce the new full body — or an `Err` message to relay back to the
/// model. Decided from intent, tolerant of the mislabelling small models do:
///   - explicit `mode == "append"` -> append `content`
///   - explicit `mode == "replace"` -> whole-body replace, IGNORING any stray
///     `find` (models often send mode:"replace" with the full note in `content`
///     AND a leftover `find` — honouring find there garbles the note)
///   - otherwise a non-empty `find` -> targeted snippet edit/delete
///   - otherwise (e.g. mode:"edit" with no find, or unspecified) -> replace
/// `mode` is passed raw ("" when unspecified) so an explicit "replace" can be
/// told apart from the default. Kept free of `AppState`/Tauri for unit tests.
pub fn plan_write(
    current_body: &str,
    content: &str,
    mode: &str,
    find: &str,
) -> Result<WritePlan, String> {
    // Some models echo the prompt's note-framing markers into the tool content
    // (seen with Granite). Strip them so they never land in the saved note. This
    // runs before the intent logic so the absorb-check can still clean an edit.
    let content = strip_prompt_markers(content);
    let content = content.as_str();
    let m = mode.trim().to_lowercase();
    let has_find = !find.trim().is_empty();
    let is_append = m == "append";
    let explicit_replace = m == "replace";
    // A targeted edit only when a `find` is given and the model did NOT
    // explicitly ask for a whole-body replace/append.
    let snippet = has_find && !explicit_replace && !is_append;

    if snippet {
        match find_tolerant(current_body, find) {
            Some((start, end)) => {
                let prefix = &current_body[..start];
                let suffix = &current_body[end..];
                // If `content` already contains the surrounding text (so splicing
                // would duplicate it), the model actually sent the whole updated
                // body, not a snippet replacement — treat it as a replace. Catches
                // e.g. find:"blue", content:"The sky is green today." on a note of
                // "The sky is blue today." (which would otherwise garble).
                let absorbs = (!prefix.trim().is_empty() && content.starts_with(prefix))
                    || (!suffix.trim().is_empty() && content.ends_with(suffix));
                if absorbs {
                    return Ok(WritePlan { new_body: content.to_string(), op: WriteOp::Replace });
                }
                let mut body = String::with_capacity(current_body.len() + content.len());
                body.push_str(prefix);
                body.push_str(content);
                body.push_str(suffix);
                Ok(WritePlan { new_body: body, op: WriteOp::EditSnippet })
            }
            None => Err("Could not find the `find` text in the note. Retry with mode \"replace\" and send the COMPLETE updated note as `content`.".to_string()),
        }
    } else if is_append {
        let body = if current_body.trim().is_empty() {
            content.trim().to_string()
        } else {
            format!("{}\n\n{}", current_body.trim_end(), content.trim_start())
        };
        Ok(WritePlan { new_body: body, op: WriteOp::Append })
    } else {
        // Whole-body replace: explicit replace (find ignored), mode:"edit" with
        // no find, or unspecified mode.
        Ok(WritePlan { new_body: content.to_string(), op: WriteOp::Replace })
    }
}

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

/// Heuristic: does this user message ask to CREATE or MODIFY the open note (as
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
        "yes", "y", "yeah", "yep", "yup", "sure", "ok", "okay", "k", "go ahead",
        "do it", "please do", "go for it", "sounds good", "anything", "you decide",
        "proceed", "go",
    ];
    if AFFIRMATIONS.contains(&affirmation) {
        return true;
    }
    // Leading affirmation word ("yes please", "sure, go for it"). Limited to
    // strong single-word affirmations so a question like "ok what is X" is not
    // mistaken for a write.
    const LEADING_AFFIRMATIONS: &[&str] =
        &["yes", "yeah", "yep", "yup", "sure", "absolutely", "definitely"];
    let first_word = affirmation.split_whitespace().next().unwrap_or("");
    if LEADING_AFFIRMATIONS.contains(&first_word) {
        return true;
    }

    // Strong create/edit verbs. In this app these always target the open note.
    const WRITE_VERBS: &[&str] = &[
        "write", "rewrite", "re-write", "create", "draft", "compose", "add",
        "append", "insert", "generate", "produce", "jot", "fill", "format",
        "reformat", "restructure", "reorganize", "reorganise", "organize",
        "organise", "clean up", "cleanup", "tidy", "fix", "correct", "proofread",
        "improve", "polish", "edit", "revise", "update", "change", "modify",
        "shorten", "condense", "trim", "expand", "lengthen", "elaborate",
        "reorder", "rearrange", "remove", "delete", "erase", "replace", "swap",
        "clear", "empty", "wipe", "blank", "scrap",
        "bold", "italic", "italicize", "capitalize", "capitalise", "continue",
        "extend", "finish", "translate", "rephrase", "reword",
        // Transform phrasings that don't use a bare edit verb.
        "make it", "make this", "make the", "turn it", "turn this", "convert it",
        "convert this", "shorter", "longer", "concise",
    ];
    if contains_any_word(&m, WRITE_VERBS) {
        return true;
    }

    // Soft content verbs (explain/describe/...) only count as a note write when
    // the message explicitly points at the note ("explain X in the note").
    const NOTE_TARGETS: &[&str] = &[
        "the note", "this note", "in the note", "to the note", "into the note",
        "my note", "the document", "the doc", "the page",
    ];
    const SOFT_VERBS: &[&str] = &[
        "explain", "describe", "list", "summarize", "summarise", "answer",
        "outline", "detail", "note down", "record",
    ];
    let targets_note = NOTE_TARGETS.iter().any(|t| m.contains(t));
    if targets_note && contains_any_word(&m, SOFT_VERBS) {
        return true;
    }

    false
}

/// Pure greeting / acknowledgement vocabulary. If the whole message (<=4 words)
/// is made of these, it's small talk → offer NO tools so the model can't
/// reflexively call one. (Pattern borrowed from the ggufplay experiment.)
const SMALL_TALK: &[&str] = &[
    "hi", "hello", "hey", "yo", "sup", "hiya", "howdy", "gg", "wsg", "thanks",
    "thank", "you", "thankyou", "thx", "ty", "cheers", "ok", "okay", "k", "kk",
    "cool", "nice", "great", "awesome", "perfect", "got", "it", "gotcha",
    "sounds", "good", "sure", "yep", "yeah", "yup", "yes", "no", "nope", "lol",
    "haha", "hah", "np", "problem", "welcome", "morning", "afternoon", "evening",
    "night", "so", "much", "please", "mate", "man", "bro", "how", "are", "whats",
    "up", "doing", "going",
];

pub fn is_small_talk(message: &str) -> bool {
    let words: Vec<String> = message
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if words.is_empty() {
        return true;
    }
    if words.len() > 4 {
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
    m.contains("other note")
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
        || (contains_any_word(&m, &["search", "find"]) && m.contains("notes"))
}

/// TLD allowlist used to spot a BARE domain (example.com, speediq.ai) in a
/// message while keeping real file names out (notes.txt, model.gguf, poem.md are
/// NOT web targets). Ported from the ggufplay gating experiment.
fn has_web_domain(m: &str) -> bool {
    const WEB_TLD: &str =
        "com|org|net|io|ai|dev|co|app|gov|edu|me|xyz|info|biz|us|uk|ca|de|fr|in|cloud|tech|news|gg|so";
    regex::Regex::new(&format!(r"(?i)\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{WEB_TLD})\b"))
        .map(|re| re.is_match(m))
        .unwrap_or(false)
}

/// Does the message ask to fetch a specific web page — a full URL, a bare
/// domain, or an explicit "fetch/open/visit the page"?
pub fn wants_fetch(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("http://")
        || m.contains("https://")
        || m.contains("www.")
        || has_web_domain(&m)
        || (contains_any_word(&m, &["fetch", "download", "open", "visit", "go to", "load", "scrape"])
            && contains_any_word(&m, &["page", "url", "site", "website", "link"]))
}

/// Per-message tool gating: hand the model ONLY the tools its message warrants,
/// so a small model can't misfire on a tool it was never given. write_note is
/// the primary action (the open note is the workspace); search_notes/read_note
/// and fetch_web_page are opt-in by intent; small talk gets nothing.
pub fn select_tools(message: &str, has_open_note: bool) -> Vec<Value> {
    if is_small_talk(message) {
        return Vec::new();
    }
    let mut names: Vec<&str> = Vec::new();
    if has_open_note && note_write_intent(message) {
        names.push("write_note");
    }
    if wants_other_notes(message) {
        names.push("search_notes");
        names.push("read_note");
    }
    if wants_fetch(message) {
        names.push("fetch_web_page");
    }
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
            description: "Read the full Markdown of ANOTHER note by its id (ids come from search_notes). Do NOT use this for the note currently open in the editor — that note's content is already provided in the prompt below.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "note_id": {
                        "type": "string",
                        "description": "The id of a DIFFERENT note to read (from search_notes results), not the open note."
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
    /// The text payload. For replace/append it is the note body or new text; for
    /// edit it is the replacement (empty string deletes the matched text).
    content: String,
    /// "replace" (default, whole body) | "append" (add to end) | "edit" (swap `find`).
    #[serde(default)]
    mode: Option<String>,
    /// Required only when `mode` is "edit": the exact existing text to replace.
    #[serde(default)]
    find: Option<String>,
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
                "Edit the note currently OPEN in the editor — this tool only ever changes that one open note and NEVER creates a separate new note. It handles ANY request to write, create, draft, generate, rewrite, edit, format, reformat, restructure, clean up, fix, improve, change, add to, or delete from the open note. `mode` selects the operation: \"replace\" (default) sets the ENTIRE note body to `content` (empty string clears it); \"append\" adds `content` to the end (send only the new text); \"edit\" replaces the exact `find` text with `content` (empty `content` deletes the match). Always put the real final Markdown in `content`, never a placeholder."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text payload. For replace/append it is the note body or the new text; for edit it is the replacement (empty string deletes the matched text). Never a placeholder like [insert poem here]." },
                    "mode": { "type": "string", "enum": ["replace", "append", "edit"], "description": "How to apply content: replace (default, whole body) | append (add to end) | edit (swap the `find` snippet)." },
                    "find": { "type": "string", "description": "Required only when mode is \"edit\": the exact existing text in the note to replace or delete." }
                },
                "required": ["content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Pass mode raw ("" when unspecified) so the planner can tell an explicit
        // "replace" from the default.
        let mode = args.mode.as_deref().unwrap_or("").to_string();
        let content = args.content;
        let find = args.find.clone().unwrap_or_default();

        // Resolve the open note up front — needed for the tool chip, approval
        // dialog title, and the actual save. Writes always target the open note.
        let existing = match self.state.resolve_chat_target_note("") {
            Some(n) => n,
            None => {
                return Ok("No note is currently open to write to. Creating new notes from the sidebar chat is not allowed.".to_string());
            }
        };

        // Decide the new body (and how the UI should apply it) using the pure,
        // unit-tested planner. A refusal comes back as Err and is relayed to the
        // model verbatim so it can correct itself.
        let plan = match plan_write(&existing.body, &content, &mode, &find) {
            Ok(p) => p,
            Err(msg) => return Ok(msg),
        };
        let content_empty = content.trim().is_empty();
        let (emit_content, emit_mode, display_name) = match plan.op {
            WriteOp::Append => (content.clone(), "append", "Append Note"),
            WriteOp::EditSnippet => (
                plan.new_body.clone(),
                "write",
                if content_empty { "Delete Text" } else { "Replace Text" },
            ),
            WriteOp::Replace => (
                plan.new_body.clone(),
                "write",
                if content_empty { "Clear Note" } else { "Write Note" },
            ),
        };
        let new_body = plan.new_body;

        let preview = if plan.op == WriteOp::EditSnippet {
            format!("Find:\n{}\n\nReplace with:\n{}", find, content)
        } else {
            content.clone()
        };

        if let Err(msg) =
            check_tool_approval(&self.state, display_name, &existing.title, &preview).await
        {
            return Ok(msg);
        }
        self.state
            .record_chat_tool(display_name, existing.title.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": display_name, "details": format!("Title: {}\n\n{}", existing.title, preview) }),
        );

        self.state
            .save_note(
                existing.id.clone(),
                existing.title,
                existing.tags,
                new_body,
                existing.source_pdf,
                Some(existing.annotations),
            )
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
        let _ = self.state.handle.emit(
            "ai://note_written",
            serde_json::json!({ "noteId": existing.id, "content": emit_content, "mode": emit_mode }),
        );
        Ok(format!("Note successfully updated with ID: {}", existing.id))
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
            description: "Search the ENTIRE workspace for OTHER notes containing specific keywords. Do NOT use this to search or modify the currently open note.".to_string(),
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
        // Low temperature keeps a small model decisive and on-task instead of
        // rambling or asking the same clarifying question repeatedly.
        .temperature(temperature)
        .default_max_turns(max_turns)
        .tool(ReadNoteTool {
            state: state.clone(),
        })
        .tool(WriteNoteTool {
            state: state.clone(),
        })
        .tool(FetchWebPageTool {
            state: state.clone(),
        })
        .tool(SearchNotesTool { state })
        .build()
}

/// Remove the prompt's note-framing markers that some models echo into content.
/// Tolerant of the dash-count / spacing variants models produce (e.g. Granite
/// emitted "--- END CURRENT NOTE --" with two trailing dashes).
pub fn strip_prompt_markers(s: &str) -> String {
    let mut cleaned = regex::Regex::new(r"(?i)-{2,}\s*(?:end\s+)?current note\s*-*")
        .map(|re| re.replace_all(s, "").into_owned())
        .unwrap_or_else(|_| s.to_string());
    // The model sometimes bleeds the "--- CURRENT NOTE ---" delimiter style into
    // its output as a leading "--- Title" — dashes + text on one line, which is
    // not a real Markdown rule (an HR is dashes alone). Drop the leading dash run
    // but keep the text. (Rust's regex has no lookahead, so capture and re-emit.)
    if let Ok(re) = regex::Regex::new(r"^\s*-{2,}[ \t]+(\S[^\n]*)") {
        cleaned = re.replace(&cleaned, "$1").into_owned();
    }
    cleaned.trim().to_string()
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

    // Granite echoed the prompt's note-framing markers into a direct tool call;
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
        assert_eq!(strip_prompt_markers("body\n-- end current note ---"), "body");
        assert_eq!(strip_prompt_markers("clean note"), "clean note");
        // Bled "--- Title" delimiter (dashes + text on one line) is stripped...
        assert_eq!(strip_prompt_markers("--- Example Domain"), "Example Domain");
        // ...but a real horizontal rule (dashes alone on a line) is preserved.
        assert_eq!(strip_prompt_markers("# Title\n\n---\nmore"), "# Title\n\n---\nmore");
    }

    #[test]
    fn append_adds_to_end() {
        let plan = plan_write(NOTE, "A new line.", "append", "").unwrap();
        assert_eq!(plan.op, WriteOp::Append);
        assert!(plan.new_body.starts_with(NOTE));
        assert!(plan.new_body.ends_with("A new line."));
    }

    #[test]
    fn find_replaces_only_the_snippet() {
        let plan = plan_write(NOTE, "slow", "edit", "fast").unwrap();
        assert_eq!(plan.op, WriteOp::EditSnippet);
        assert_eq!(plan.new_body, "Cars are slow. They have engines. People drive them daily.");
    }

    // Regression: the live harness caught the model sending mode:"replace" with
    // the full new sentence in `content` AND a stray find:"blue". An explicit
    // replace must use the full content and IGNORE find (not splice it in).
    #[test]
    fn explicit_replace_ignores_stray_find() {
        let plan = plan_write("The sky is blue today.", "The sky is green today.", "replace", "blue")
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
        let plan =
            plan_write("The sky is blue today.", "The sky is green today.", "edit", "blue").unwrap();
        assert_eq!(plan.op, WriteOp::Replace);
        assert_eq!(plan.new_body, "The sky is green today.");
    }

    #[test]
    fn tool_gating_selects_by_intent() {
        // small talk → no tools
        assert!(select_tools("gg", true).is_empty());
        assert!(select_tools("thanks!", true).is_empty());
        // write intent → write_note only
        let w = select_tools("expand this to 500 words", true);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0]["function"]["name"], "write_note");
        // pure question → no tools (model answers in chat)
        assert!(select_tools("what is the capital of france?", true).is_empty());
        // other-notes intent → search + read
        let s = select_tools("search my other notes for cats", true);
        let names: Vec<&str> = s.iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search_notes"));
        assert!(names.contains(&"read_note"));
        // url → fetch
        let f = select_tools("fetch https://example.com", true);
        assert!(f.iter().any(|t| t["function"]["name"] == "fetch_web_page"));
    }

    #[test]
    fn small_talk_detection() {
        assert!(is_small_talk("hi"));
        assert!(is_small_talk("thanks so much"));
        assert!(!is_small_talk("write a note about cats"));
        assert!(!is_small_talk("what is the capital of france and why")); // > 4 words
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
        let t = select_tools("summarize example.com", true);
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
        assert_eq!(normalize_web_url("example.com").unwrap(), "https://example.com");
        assert_eq!(normalize_web_url("http://x.io").unwrap(), "http://x.io");
        assert!(normalize_web_url("   ").is_err());
        assert!(normalize_web_url("has space.com").is_err());
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
    fn write_intent_soft_verb_needs_note_target() {
        // "explain X" alone is a chat answer; "explain X in the note" is a write.
        assert!(!note_write_intent("explain what you are"));
        assert!(note_write_intent("explain what you are in the note with an h1"));
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
