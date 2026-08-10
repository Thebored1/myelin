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
/// True when a focused Write request explicitly asks to remove the armed
/// fragment. This is intentionally separate from `wants_clear`, which only
/// authorizes emptying the entire note.
pub(super) fn wants_targeted_deletion(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    wants_clear(message)
        || [
            "delete selected",
            "delete the selected",
            "remove selected",
            "remove the selected",
            "erase selected",
            "erase the selected",
            "delete this",
            "remove this",
            "erase this",
            "make this blank",
        ]
        .iter()
        .any(|phrase| m.contains(phrase))
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

/// Tool profile for the target-scoped Write mode. Notebook schemas are kept
/// target-compatible by constraining the operation enum to `edit`.
pub fn targeted_write_tools(doc_type: &str) -> Vec<Value> {
    if doc_type.eq_ignore_ascii_case("ipynb") || doc_type.to_ascii_lowercase().ends_with(".ipynb") {
        let mut specs = specs_for(&["edit_notebook"]);
        if let Some(function) = specs
            .first_mut()
            .and_then(|spec| spec.get_mut("function"))
        {
            function["description"] = serde_json::json!(
                "Edit only the armed notebook cell target. Use operation \"edit\" and return the replacement source in content."
            );
        }
        if let Some(parameters) = specs
            .first_mut()
            .and_then(|spec| spec.get_mut("function"))
            .and_then(|function| function.get_mut("parameters"))
        {
            if let Some(operation) = parameters
                .get_mut("properties")
                .and_then(|properties| properties.get_mut("operation"))
            {
                operation["enum"] = serde_json::json!(["edit"]);
            }
        }
        specs
    } else {
        let mut specs = specs_for(&["write_note"]);
        if let Some(function) = specs
            .first_mut()
            .and_then(|spec| spec.get_mut("function"))
        {
            function["description"] = serde_json::json!(
                "Replace only the armed cursor or text selection with the requested finished content. Send only the new fragment in content."
            );
            function["parameters"]["properties"]["content"]["description"] = serde_json::json!(
                "Only the new insertion or replacement fragment; never include surrounding note text."
            );
        }
        specs
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
