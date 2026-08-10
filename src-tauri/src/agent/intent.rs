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
/// Word-boundary check: true if any of `words` (lowercase literals/phrases)
/// appears as a whole word in the already-lowercased `haystack`. Avoids
/// substring false hits like "fix" inside "prefix" or "add" inside "address".
pub(super) fn contains_any_word(haystack: &str, words: &[&str]) -> bool {
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
pub(super) fn is_negated(message: &str, keywords: &[&str]) -> bool {
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
pub(super) fn existing_note_operation(message: &str) -> bool {
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
pub(super) fn has_web_domain(m: &str) -> bool {
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
