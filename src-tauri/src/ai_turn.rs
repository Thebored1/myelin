//! Pure construction of a model turn.
//!
//! Desktop chat and headless acceptance tests use this module so routing,
//! metadata, prompt layout, and tool schemas cannot drift independently.

use serde_json::{json, Value};

mod context;
mod types;
pub use context::{
    ChatTurnContext, ToolTurnContext, TurnCancellation, TurnMode, TurnPolicy,
};
pub use types::{AiTurn, AiTurnInput, TurnKind};

pub struct AiTurnBuilder;

impl AiTurnBuilder {
    pub fn build(input: AiTurnInput<'_>) -> AiTurn {
        let routed = if input.supports_tools {
            route_tools(
                input.mode,
                input.doc_type,
                input.question,
                input.has_open_note,
                input.edit_thread,
                input.oversized,
            )
        } else {
            Vec::new()
        };
        let tools =
            crate::agent::compact_tool_specs_for_profile(routed, input.verbose_tool_schemas);
        // Deterministic TOOL/CHAT intent, decoupled from the offered schema list.
        // Chat always carries one fixed read-only schema set (so the system/tool
        // prefix is byte-identical every turn and llama-server can reuse the KV
        // cache), so "tools present" can no longer mean "this is a tool turn".
        let intent_is_tool = match input.mode {
            "operation" | "write" | "edit" => true,
            // A chat-only model cannot execute a tool. This also covers the
            // retrieval-backed fast path, which deliberately removes the
            // read-only schemas after the host has already fetched evidence.
            "chat" => input.supports_tools && crate::agent::chat_tool_intent(input.question),
            _ => !tools.is_empty(),
        };
        // A fixed preamble per mode: chat always uses the editing preamble when
        // it offers the read-only schema set, and the minimal chat preamble for
        // tool-less models. The selection depends only on the stable mode+tool
        // capability, never on the current question, so the system message does
        // not flip between turns.
        let preamble = if input.mode == "write" {
            crate::agent::TARGETED_WRITE_PREAMBLE
        } else if input.mode == "chat" && tools.is_empty() {
            crate::agent::DIRECT_CHAT_PREAMBLE
        } else {
            crate::agent::MYELIN_PREAMBLE
        };
        // Chat always renders the minimal direct user content (raw question plus
        // any turn-specific context) regardless of the read-only schemas offered.
        let direct_chat = input.mode == "chat";
        let mut messages = if input.section_context {
            vec![
                json!({
                    "role": "system",
                    "content": input.system_context,
                }),
                json!({
                    "role": "system",
                    "content": preamble,
                }),
            ]
        } else {
            vec![json!({
                "role": "system",
                "content": format!("{preamble}\n\n{}", input.system_context),
            })]
        };
        messages.extend(input.conversation.iter().cloned());
        messages.push(json!({
            "role": "user",
            "content": if direct_chat {
                render_direct_chat_user_content(input.question, input.turn_instructions)
            } else {
                render_user_content(
                    input.note_title,
                    input.mode_policy,
                    input.turn_instructions,
                    input.question,
                )
            },
            "metadata": {
                "open_note_title": input.note_title,
                "interaction_mode": input.mode,
            }
        }));
        AiTurn {
            messages,
            tools,
            intent_is_tool,
            kind: if intent_is_tool {
                TurnKind::ToolSelection
            } else {
                TurnKind::DirectAnswer
            },
        }
    }
}

fn route_tools(
    mode: &str,
    doc_type: &str,
    question: &str,
    has_open_note: bool,
    edit_thread: bool,
    oversized: bool,
) -> Vec<Value> {
    match mode {
        "chat" => crate::agent::select_chat_tools(question, has_open_note),
        "write" => crate::agent::targeted_write_tools(doc_type),
        "edit" => crate::agent::interaction_mode_tools("edit", oversized),
        // Operation/Auto prompts are section-cacheable too. Keep their model
        // schema stable across questions; the host still applies deterministic
        // intent and mutation authorization when a tool call is executed.
        "operation" | "auto" => crate::agent::interaction_mode_tools("operation", oversized),
        _ => crate::agent::select_tools(question, has_open_note, edit_thread),
    }
}

/// Keep ordinary direct chat maximally cache-friendly, while ensuring dynamic
/// retrieval/selection instructions are not discarded on grounded turns.
pub fn render_direct_chat_user_content(question: &str, turn_instructions: &str) -> String {
    if turn_instructions.trim().is_empty() {
        question.to_string()
    } else {
        format!(
            "TURN-SPECIFIC CONTEXT AND INSTRUCTIONS:\n{}\n\nUSER REQUEST:\n{}",
            turn_instructions.trim(),
            question
        )
    }
}

/// Requests that intentionally widen beyond the visible page/chapter. Viewer
/// chat defaults to the active section; only these explicit document-wide
/// phrases should trigger RAG over the full source.
pub fn is_whole_document_request(question: &str) -> bool {
    let q = question.to_ascii_lowercase();
    [
        "whole document",
        "whole pdf",
        "whole paper",
        "whole file",
        "entire document",
        "entire pdf",
        "entire paper",
        "entire file",
        "all pages",
        "every page",
        "across the document",
        "across the pdf",
        "across the paper",
        "throughout the document",
        "throughout the pdf",
        "throughout the paper",
        "document overview",
        "pdf overview",
        "paper overview",
        "what is this document about",
        "what is this pdf about",
        "what is this paper about",
        "summarize the document",
        "summarize this document",
        "summarize the pdf",
        "summarize this pdf",
        "summarize the paper",
        "summarize this paper",
    ]
    .iter()
    .any(|marker| q.contains(marker))
}

/// An explicit numbered page different from the visible page is another clear
/// signal to leave the active-section fast path and retrieve that page.
pub fn references_other_page(question: &str, active_label: Option<&str>) -> bool {
    let active_page = active_label.and_then(|label| {
        label
            .split(|c: char| !c.is_ascii_digit())
            .find(|part| !part.is_empty())
            .and_then(|part| part.parse::<u32>().ok())
    });
    let Some(active_page) = active_page else {
        return false;
    };
    let words: Vec<&str> = question
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.windows(2).any(|pair| {
        pair[0].eq_ignore_ascii_case("page")
            && pair[1].parse::<u32>().is_ok_and(|page| page != active_page)
    })
}

/// Build a retrieval query that gives the current question the greatest weight,
/// but carries enough bounded conversational context to resolve follow-ups.
pub fn contextual_retrieval_query(question: &str, history: &[Value]) -> String {
    const HISTORY_CHARS: usize = 600;
    let prior_user = history.iter().rev().find_map(|message| {
        (message["role"].as_str() == Some("user"))
            .then(|| message["content"].as_str())
            .flatten()
    });
    // Retrieval history is supplied from the persisted raw chat turns, not
    // from the model-facing rendered prompt. An exact repeated question is
    // therefore recognized from actual turn data, without parsing prompt
    // markers or silently dropping any retrieved evidence.
    let repeated_question = prior_user.is_some_and(|previous| previous.trim() == question.trim());
    let prior_user = (!repeated_question).then_some(prior_user).flatten();

    let bounded = |text: &str| -> String {
        let chars: Vec<char> = text.chars().collect();
        chars[chars.len().saturating_sub(HISTORY_CHARS)..]
            .iter()
            .collect::<String>()
    };
    let mut query = format!("Latest question: {question}");
    if let Some(user) = prior_user {
        query.push_str("\nPrevious user context: ");
        query.push_str(&bounded(user));
    }
    // Retrieval's lexical query must never inherit claims made by the model.
    // The answer-generation conversation memory remains responsible for the
    // assistant side of a follow-up; retrieval only sees user-authored text.
    query
}

/// Keep document-grounded prompts small while making conversation memory
/// universal across viewer sections. Select at most one relevant prior
/// user/assistant pair from anywhere in canonical history; pronoun-style
/// follow-ups fall back to the latest pair. This avoids both branched section
/// histories and replaying the full conversation after every restored KV.
pub fn compact_document_conversation(question: &str, history: &[Value]) -> Vec<Value> {
    let q = question.to_ascii_lowercase();
    let needs_history = q.split(|c: char| !c.is_ascii_alphanumeric()).any(|word| {
        matches!(
            word,
            "it" | "that" | "they" | "them" | "those" | "these" | "former" | "latter"
        )
    }) || [
        "why",
        "what about",
        "you said",
        "earlier",
        "previous answer",
        "compare",
        "difference",
        "same as",
    ]
    .iter()
    .any(|marker| q.contains(marker));
    let sanitized_content = |message: &Value| -> Option<String> {
        let mut content = message["content"].as_str()?;
        // Canonical history may contain the exact model-facing wrapper from an
        // older RAG turn. Universal memory needs the user's question, not a
        // duplicated block of retrieved evidence.
        if message["role"].as_str() == Some("user") {
            if let Some((_, request)) = content.rsplit_once("\n\nUSER REQUEST:\n") {
                content = request;
            }
        }
        Some(content.to_string())
    };
    let significant_terms = |text: &str| -> std::collections::HashSet<String> {
        const STOP: &[&str] = &[
            "about", "answer", "could", "document", "does", "from", "have", "mention", "page",
            "paper", "please", "section", "should", "that", "their", "there", "these", "they",
            "this", "those", "what", "when", "where", "which", "with", "would", "your",
        ];
        text.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| word.len() >= 4 && !STOP.contains(word))
            .map(str::to_string)
            .collect()
    };
    let query_terms = significant_terms(question);

    // Find a semantically useful prior exchange with a tiny lexical lookup.
    // Generic words such as "page" and "document" are excluded so repeating
    // "what is on this page?" after navigation cannot import the old page's
    // answer into the new one.
    let mut best_pair: Option<(usize, Option<usize>, usize)> = None;
    for (index, message) in history.iter().enumerate() {
        if message["role"].as_str() != Some("user") {
            continue;
        }
        let Some(content) = sanitized_content(message) else {
            continue;
        };
        let terms = significant_terms(&content);
        let score = query_terms.intersection(&terms).count();
        if score == 0 {
            continue;
        }
        let assistant = history
            .get(index + 1)
            .filter(|next| next["role"].as_str() == Some("assistant"))
            .map(|_| index + 1);
        if best_pair
            .as_ref()
            .is_none_or(|(best_index, _, best_score)| {
                score > *best_score || (score == *best_score && index > *best_index)
            })
        {
            best_pair = Some((index, assistant, score));
        }
    }

    let selected = if let Some((user, assistant, _)) = best_pair {
        Some((user, assistant))
    } else if needs_history {
        history
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, message)| {
                (message["role"].as_str() == Some("user")).then(|| {
                    let assistant = history
                        .get(index + 1)
                        .filter(|next| next["role"].as_str() == Some("assistant"))
                        .map(|_| index + 1);
                    (index, assistant)
                })
            })
    } else {
        None
    };
    let Some((user_index, assistant_index)) = selected else {
        return Vec::new();
    };

    // This pair is deliberately small: it is universal app memory appended
    // after any restored section KV, so it must be cheap to re-evaluate on a
    // section switch even on CPU-only inference.
    const MAX_HISTORY_CHARS: usize = 1_200;
    let mut kept = Vec::new();
    let mut chars = 0;
    for index in std::iter::once(user_index).chain(assistant_index) {
        let message = &history[index];
        let Some(content) = sanitized_content(message) else {
            continue;
        };
        if chars >= MAX_HISTORY_CHARS {
            break;
        }
        let remaining = MAX_HISTORY_CHARS - chars;
        let bounded: String = content.chars().take(remaining).collect();
        chars += bounded.chars().count();
        let mut copy = message.clone();
        copy["content"] = Value::String(bounded);
        kept.push(copy);
    }
    kept
}

/// Broad retrieval requests usually ask for a set, an exhaustive scan, or a
/// document-level overview. They use adaptive retrieval: once the returned
/// passages keep confirming the requested topic, the caller fetches another
/// page instead of treating the first two chunks as the whole answer.
pub fn is_broad_retrieval_request(question: &str) -> bool {
    let q = question.to_ascii_lowercase();
    let markers = [
        "all ",
        "every ",
        "each ",
        "list ",
        "throughout",
        "find all",
        "show all",
        "which poems",
        "what poems",
        "recite",
        "quote",
        "transcribe",
        "reproduce",
        "read aloud",
        "summarize",
        "summary",
        "overview",
        "give me an overview",
        "what does this pdf contain",
        "what does the pdf contain",
        "what does this document contain",
        "what does the document contain",
        "what is this pdf about",
        "what is the pdf about",
        "what is this document about",
        "what is the document about",
        "what is the attached pdf about",
        "what is the attached document about",
        "what is this paper about",
        "what is the paper about",
        "what is the attached paper about",
        "what is this file about",
        "what is the file about",
        "tell me about this pdf",
        "tell me about the pdf",
        "tell me about this document",
        "tell me about the document",
        "describe this pdf",
        "describe the pdf",
        "describe this document",
        "describe the document",
    ];
    markers.iter().any(|marker| q.contains(marker))
}

/// Correct a small set of unambiguous document-reading typos before retrieval
/// and model inference. Keep the original user text for the UI/history; this
/// only prevents a typo such as "reciete" from being interpreted as "recipe".
pub fn normalize_document_question(question: &str) -> String {
    question
        .split_whitespace()
        .map(|word| {
            let (prefix, core, suffix) = word
                .char_indices()
                .find(|(_, c)| c.is_alphanumeric())
                .map(|(start, _)| {
                    let end = word
                        .char_indices()
                        .rev()
                        .find(|(_, c)| c.is_alphanumeric())
                        .map(|(index, c)| index + c.len_utf8())
                        .unwrap_or(word.len());
                    (&word[..start], &word[start..end], &word[end..])
                })
                .unwrap_or(("", word, ""));
            let replacement = match core.to_ascii_lowercase().as_str() {
                "reciete" | "reciet" | "reicte" => "recite",
                _ => core,
            };
            format!("{prefix}{replacement}{suffix}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn is_verbatim_document_request(question: &str) -> bool {
    let q = question.to_ascii_lowercase();
    ["recite", "quote", "transcribe", "reproduce", "read aloud"]
        .iter()
        .any(|marker| q.contains(marker))
}

/// Count retrieved passages containing at least one meaningful query term.
/// Stop words include document-routing language so words such as "attached"
/// and "document" do not make every chunk look relevant.
pub fn retrieval_support_count(question: &str, chunks: &[crate::rag::RetrievedChunk]) -> usize {
    let terms = retrieval_query_terms(question);
    chunks
        .iter()
        .filter(|chunk| {
            let text = chunk.text.to_ascii_lowercase();
            terms.iter().any(|term| text.contains(term))
        })
        .count()
}

fn retrieval_query_terms(question: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "about",
        "all",
        "an",
        "are",
        "as",
        "attached",
        "a",
        "author",
        "authors",
        "can",
        "contains",
        "contain",
        "describe",
        "do",
        "document",
        "does",
        "each",
        "every",
        "file",
        "find",
        "from",
        "give",
        "in",
        "is",
        "it",
        "know",
        "list",
        "me",
        "mention",
        "mentions",
        "of",
        "on",
        "paper",
        "pdf",
        "poem",
        "poems",
        "read",
        "recite",
        "reference",
        "references",
        "show",
        "source",
        "summarize",
        "summary",
        "tell",
        "that",
        "the",
        "this",
        "to",
        "what",
        "which",
        "with",
        "write",
        "you",
    ];
    question
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|term| term.len() >= 4 && !STOP.contains(term))
        .map(str::to_string)
        .collect()
}

/// Cheap confirmation for an adaptive retrieval pass. Search results are
/// cumulative (top-k), so only newly added chunks count. For overview queries
/// there are no meaningful lexical terms; any new ranked chunk is useful and
/// the caller stops when the index has no more new chunks to return.
pub fn retrieval_expansion_has_signal(
    question: &str,
    previous: &[crate::rag::RetrievedChunk],
    expanded: &[crate::rag::RetrievedChunk],
) -> bool {
    let previous_keys: std::collections::HashSet<(String, i32)> = previous
        .iter()
        .map(|chunk| (chunk.doc_id.clone(), chunk.chunk_index))
        .collect();
    let new_chunks: Vec<&crate::rag::RetrievedChunk> = expanded
        .iter()
        .filter(|chunk| !previous_keys.contains(&(chunk.doc_id.clone(), chunk.chunk_index)))
        .collect();
    if new_chunks.is_empty() {
        return false;
    }

    let new_owned: Vec<crate::rag::RetrievedChunk> = new_chunks.into_iter().cloned().collect();
    retrieval_query_terms(question).is_empty() || retrieval_support_count(question, &new_owned) > 0
}

pub fn render_user_content(
    note_title: &str,
    mode_policy: &str,
    turn_instructions: &str,
    question: &str,
) -> String {
    format!(
        "OPEN NOTE TITLE: {note_title:?}\n\n\
         INTERNAL TURN POLICY (not note metadata):\n{mode_policy}\n\n\
         {turn_instructions}\n\n\
         USER REQUEST:\n{question}"
    )
}

#[cfg(test)]
mod tests;
