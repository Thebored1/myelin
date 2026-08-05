//! Pure construction of a model turn.
//!
//! Desktop chat and headless acceptance tests use this module so routing,
//! metadata, prompt layout, and tool schemas cannot drift independently.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    DirectAnswer,
    ToolSelection,
}

#[derive(Debug, Clone)]
pub struct AiTurn {
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
    pub intent_is_tool: bool,
    pub kind: TurnKind,
}

pub struct AiTurnInput<'a> {
    pub mode: &'a str,
    pub doc_type: &'a str,
    pub note_title: &'a str,
    pub system_context: &'a str,
    pub conversation: &'a [Value],
    pub question: &'a str,
    pub mode_policy: &'a str,
    pub turn_instructions: &'a str,
    pub has_open_note: bool,
    pub edit_thread: bool,
    pub oversized: bool,
    pub supports_tools: bool,
    pub verbose_tool_schemas: bool,
}

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
        let tools = crate::agent::compact_tool_specs_for_profile(
            routed,
            input.verbose_tool_schemas,
        );
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
        let mut messages = vec![json!({
            "role": "system",
            "content": format!("{preamble}\n\n{}", input.system_context),
        })];
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
            && pair[1]
                .parse::<u32>()
                .is_ok_and(|page| page != active_page)
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
    let needs_assistant = {
        let q = question.to_ascii_lowercase();
        let has_reference = q
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| {
                matches!(
                    word,
                    "it" | "they" | "them" | "that" | "those" | "this" | "these" | "he" | "she"
                )
            });
        has_reference
            || ["why did you", "what about", "what do you mean"]
                .iter()
                .any(|term| q.contains(term))
    };
    let prior_assistant = needs_assistant
        .then(|| {
            history.iter().rev().find_map(|message| {
                (message["role"].as_str() == Some("assistant"))
                    .then(|| message["content"].as_str())
                    .flatten()
            })
        })
        .flatten();

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
    if let Some(assistant) = prior_assistant {
        query.push_str("\nPrevious assistant context (may be incorrect): ");
        query.push_str(&bounded(assistant));
    }
    query
}

/// Keep document-grounded prompts small while making conversation memory
/// universal across viewer sections. Select at most one relevant prior
/// user/assistant pair from anywhere in canonical history; pronoun-style
/// follow-ups fall back to the latest pair. This avoids both branched section
/// histories and replaying the full conversation after every restored KV.
pub fn compact_document_conversation(question: &str, history: &[Value]) -> Vec<Value> {
    let q = question.to_ascii_lowercase();
    let needs_history = q
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| {
            matches!(
                word,
                "it"
                    | "that"
                    | "they"
                    | "them"
                    | "those"
                    | "these"
                    | "former"
                    | "latter"
            )
        })
        || [
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
            "about", "answer", "could", "document", "does", "from", "have", "mention",
            "page", "paper", "please", "section", "should", "that", "their", "there",
            "these", "they", "this", "those", "what", "when", "where", "which", "with",
            "would", "your",
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
        history.iter().enumerate().rev().find_map(|(index, message)| {
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
pub fn retrieval_support_count(
    question: &str,
    chunks: &[crate::rag::RetrievedChunk],
) -> usize {
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
        "about", "all", "an", "are", "as", "attached", "a", "author", "authors", "can", "contains", "contain",
        "describe", "do", "document", "does", "each", "every", "file", "find", "from", "give",
        "in", "is", "it", "know", "list", "me", "mention", "mentions", "of", "on", "paper",
        "pdf", "poem", "poems", "read", "recite", "reference", "references", "show", "source",
        "summarize", "summary", "tell", "that", "the", "this", "to", "what", "which", "with",
        "write", "you",
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
mod tests {
    use super::*;

    fn build(mode: &str, question: &str) -> AiTurn {
        AiTurnBuilder::build(AiTurnInput {
            mode,
            doc_type: "md",
            note_title: "Real title",
            system_context: "The note currently open is titled \"Real title\".",
            conversation: &[],
            question,
            mode_policy: "policy",
            turn_instructions: "",
            has_open_note: true,
            edit_thread: false,
            oversized: false,
            supports_tools: true,
            verbose_tool_schemas: false,
        })
    }

    fn names(turn: &AiTurn) -> Vec<&str> {
        turn.tools
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect()
    }

    #[test]
    fn direct_chat_offers_fixed_read_only_schema_and_keeps_real_title_metadata() {
        let turn = build("chat", "What is this note about?");
        // Chat always carries one fixed read-only schema set so the system/tool
        // prefix is byte-identical across turns (KV reuse), regardless of the
        // question. Intent is computed deterministically and stays direct here.
        assert!(!turn.tools.is_empty());
        assert!(!turn.intent_is_tool);
        assert_eq!(turn.kind, TurnKind::DirectAnswer);
        assert_eq!(
            turn.messages[1]["metadata"]["open_note_title"],
            "Real title"
        );
        assert_eq!(turn.messages[1]["metadata"]["interaction_mode"], "chat");
        assert_eq!(turn.messages[1]["content"], "What is this note about?");
    }

    #[test]
    fn chat_rewrite_is_a_direct_draft_not_a_read_only_tool_call() {
        let turn = build("chat", "rewrite the INTRODUCTION in a couple of lines");
        assert!(!turn.tools.is_empty(), "chat keeps its stable read-only schema prefix");
        assert!(!turn.intent_is_tool);
        assert_eq!(turn.kind, TurnKind::DirectAnswer);
        assert!(!names(&turn).contains(&"write_note"));

        let operation = build(
            "operation",
            "rewrite the INTRODUCTION in a couple of lines",
        );
        assert!(operation.intent_is_tool);
        assert_eq!(operation.kind, TurnKind::ToolSelection);
    }

    #[test]
    fn chat_always_offers_the_fixed_read_only_tool_set() {
        // Question-dependent tool gating would change the rendered prompt prefix
        // between turns; chat instead offers one stable read-only set and relies
        // on deterministic tool_intent for routing.
        let expected = ["fetch_web_page", "find_in_note", "read_note", "search_documents", "search_notes", "web_search"];
        assert_eq!(names(&build("chat", "does this note contain aardvark?")), expected);
        assert_eq!(names(&build("chat", "summarize https://example.com")), expected);
        assert_eq!(names(&build("chat", "search the web for rust news")), expected);
        assert_eq!(names(&build("chat", "search my other notes for pasta")), expected);
        assert_eq!(names(&build("chat", "what does my PDF say about attention?")), expected);
    }

    #[test]
    fn operation_keeps_a_stable_tool_schema_for_section_cache_reuse() {
        let first_turn = build("operation", "rewrite the introduction");
        let second_turn = build("operation", "search the web for sources");
        let first = names(&first_turn);
        let second = names(&second_turn);
        assert_eq!(first, second);
        assert!(first.contains(&"write_note"));
    }

    #[test]
    fn write_mode_offers_only_the_targeted_note_tool() {
        let turn = build("write", "based on the active source, write a poem");
        assert!(turn.intent_is_tool);
        assert_eq!(names(&turn), vec!["write_note"]);
        assert!(turn.messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("focused Write editor"));
    }

    #[test]
    fn latex_write_mode_still_offers_only_the_targeted_note_tool() {
        let turn = AiTurnBuilder::build(AiTurnInput {
            mode: "write",
            doc_type: "tex",
            note_title: "Paper",
            system_context: "latex context",
            conversation: &[],
            question: "rewrite the selected equation",
            mode_policy: "write policy",
            turn_instructions: "selection is armed",
            has_open_note: true,
            edit_thread: false,
            oversized: false,
            supports_tools: true,
            verbose_tool_schemas: false,
        });
        assert!(turn.intent_is_tool);
        assert_eq!(names(&turn), vec!["write_note"]);
    }

    #[test]
    fn notebook_write_mode_only_allows_edit_operation() {
        let turn = AiTurnBuilder::build(AiTurnInput {
            mode: "write",
            doc_type: "ipynb",
            note_title: "Notebook",
            system_context: "notebook context",
            conversation: &[],
            question: "rewrite the selected cell",
            mode_policy: "write policy",
            turn_instructions: "cell 2 is armed",
            has_open_note: true,
            edit_thread: false,
            oversized: false,
            supports_tools: true,
            verbose_tool_schemas: false,
        });
        assert!(turn.intent_is_tool);
        assert_eq!(names(&turn), vec!["edit_notebook"]);
        let operation = &turn.tools[0]["function"]["parameters"]["properties"]["operation"]["enum"];
        assert_eq!(operation, &serde_json::json!(["edit"]));
    }

    #[test]
    fn oversized_note_keeps_chat_read_only_schema() {
        let turn = AiTurnBuilder::build(AiTurnInput {
            mode: "chat",
            doc_type: "md",
            note_title: "Large",
            system_context: "retrieval-backed",
            conversation: &[],
            question: "does this note contain aardvark?",
            mode_policy: "policy",
            turn_instructions: "",
            has_open_note: true,
            edit_thread: false,
            oversized: true,
            supports_tools: true,
            verbose_tool_schemas: false,
        });
        let got = names(&turn);
        assert!(got.contains(&"find_in_note"));
        assert!(!got.contains(&"write_note"));
    }

    #[test]
    fn strong_profile_schemas_have_no_descriptions() {
        let turn = build("chat", "summarize https://example.com");
        let rendered = serde_json::to_string(&turn.tools).unwrap();
        assert!(!rendered.contains("description"));
    }

    #[test]
    fn direct_chat_uses_small_preamble_only_when_tool_less() {
        // Chat with tool support carries the read-only schema and the standard
        // editing preamble (fixed prefix). The minimal DIRECT_CHAT_PREAMBLE is
        // reserved for tool-less models so they never see a mutation manual.
        let turn = build("chat", "hello");
        let system = turn.messages[0]["content"].as_str().unwrap();
        assert!(system.starts_with(crate::agent::MYELIN_PREAMBLE));
        assert!(!turn.tools.is_empty());

        let tool_less = AiTurnBuilder::build(AiTurnInput {
            mode: "chat",
            doc_type: "md",
            note_title: "Real title",
            system_context: "The note currently open is titled \"Real title\".",
            conversation: &[],
            question: "hello",
            mode_policy: "policy",
            turn_instructions: "",
            has_open_note: true,
            edit_thread: false,
            oversized: false,
            supports_tools: false,
            verbose_tool_schemas: false,
        });
        assert!(tool_less.tools.is_empty());
        let small = tool_less.messages[0]["content"].as_str().unwrap();
        assert!(small.starts_with(crate::agent::DIRECT_CHAT_PREAMBLE));
        assert!(small.len() < 1_000);
    }

    #[test]
    fn direct_chat_history_stays_raw_and_policy_appears_once() {
        let conversation = vec![
            json!({"role": "user", "content": "first question"}),
            json!({"role": "assistant", "content": "first answer"}),
        ];
        let turn = AiTurnBuilder::build(AiTurnInput {
            mode: "chat",
            doc_type: "md",
            note_title: "Real title",
            system_context: "The note currently open is titled \"Real title\".",
            conversation: &conversation,
            question: "second question",
            mode_policy: "CHAT TURN POLICY: answer only.",
            turn_instructions: "",
            has_open_note: true,
            edit_thread: false,
            oversized: false,
            supports_tools: true,
            verbose_tool_schemas: false,
        });
        assert_eq!(turn.messages.len(), 4);
        assert_eq!(turn.messages[1]["content"], "first question");
        assert_eq!(turn.messages[3]["content"], "second question");
        let rendered = serde_json::to_string(&turn.messages).unwrap();
        assert!(!rendered.contains("INTERNAL TURN POLICY"));
        assert_eq!(rendered.matches("Real title").count(), 2);
    }

    #[test]
    fn direct_chat_retains_dynamic_retrieval_context() {
        let turn = AiTurnBuilder::build(AiTurnInput {
            mode: "chat",
            doc_type: "md",
            note_title: "Paper",
            system_context: "retrieval-backed",
            conversation: &[],
            question: "what about MiniCPM?",
            mode_policy: "policy",
            turn_instructions: "AUTOMATIC RETRIEVAL:\na 2-bit LFM2 model scores 47.5%",
            has_open_note: true,
            edit_thread: false,
            oversized: false,
            supports_tools: true,
            verbose_tool_schemas: false,
        });
        let content = turn.messages.last().unwrap()["content"].as_str().unwrap();
        assert!(content.contains("a 2-bit LFM2 model scores 47.5%"));
        assert!(content.ends_with("what about MiniCPM?"));
    }

    #[test]
    fn contextual_query_resolves_pronoun_followups_with_bounded_history() {
        let history = vec![
            json!({"role": "user", "content": "Does the paper discuss LFM2 and MiniCPM?"}),
            json!({"role": "assistant", "content": "They did not have a 2-bit result."}),
        ];
        let query = contextual_retrieval_query(
            "why did you say they did not have it?",
            &history,
        );
        assert!(query.starts_with("Latest question: why did you say"));
        assert_eq!(query.matches("Latest question:").count(), 1);
        assert!(query.contains("LFM2 and MiniCPM"));
        assert!(query.contains("They did not have a 2-bit result"));
    }

    #[test]
    fn repeated_raw_question_keeps_retrieval_query_cacheable() {
        let question = "could you describe all the points mentioned in HARNESS DESIGN?";
        let history = vec![
            json!({"role": "user", "content": question}),
            json!({"role": "assistant", "content": "previous answer"}),
        ];
        assert_eq!(contextual_retrieval_query(question, &history), format!("Latest question: {question}"));
    }

    #[test]
    fn independent_document_questions_drop_unrelated_history() {
        let history = vec![
            json!({"role": "user", "content": "What is GPTQ?"}),
            json!({"role": "assistant", "content": "A quantization method."}),
        ];
        assert!(compact_document_conversation("Does this paper mention MiniCPM-V?", &history).is_empty());
        assert_eq!(compact_document_conversation("Why did you say that?", &history).len(), 2);
    }

    #[test]
    fn universal_memory_keeps_question_but_drops_old_retrieval_wrapper() {
        let history = vec![
            json!({
                "role": "user",
                "content": "TURN-SPECIFIC CONTEXT AND INSTRUCTIONS:\nold evidence that must not be replayed\n\nUSER REQUEST:\nWhat did page 2 say?"
            }),
            json!({"role": "assistant", "content": "It described the cache design."}),
        ];
        let memory = compact_document_conversation("Compare that with this page", &history);
        assert_eq!(memory[0]["content"], "What did page 2 say?");
        assert!(!memory[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("old evidence"));
    }

    #[test]
    fn universal_memory_finds_a_relevant_exchange_from_an_older_section() {
        let history = vec![
            json!({"role": "user", "content": "What does Sonnet XVIII say about summer?"}),
            json!({"role": "assistant", "content": "It contrasts summer with enduring verse."}),
            json!({"role": "user", "content": "What is cached on this page?"}),
            json!({"role": "assistant", "content": "This page discusses KV cache files."}),
        ];
        let memory = compact_document_conversation(
            "How does Sonnet XVIII treat time?",
            &history,
        );
        assert_eq!(memory.len(), 2);
        assert!(memory[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Sonnet XVIII"));
        assert!(memory[1]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("enduring verse"));
    }

    #[test]
    fn broad_retrieval_requests_are_detected() {
        assert!(is_broad_retrieval_request("find all poems by Shakespeare"));
        assert!(is_broad_retrieval_request("list every mention of climate"));
        assert!(is_broad_retrieval_request("recite Auguries of Innocence"));
        assert!(is_broad_retrieval_request("what is the attached PDF about?"));
        assert!(is_broad_retrieval_request("summarize this document"));
        assert!(!is_broad_retrieval_request("what does the introduction say?"));
        assert!(!is_broad_retrieval_request("what does the paper say about transformers?"));

        assert_eq!(
            normalize_document_question("reciete 'Auguries' from the doc"),
            "recite 'Auguries' from the doc"
        );
        assert!(is_verbatim_document_request("recite Auguries of Innocence"));
    }

    #[test]
    fn document_wide_and_other_page_requests_leave_the_section_fast_path() {
        assert!(is_whole_document_request("Summarize this PDF"));
        assert!(is_whole_document_request("Find it throughout the document"));
        assert!(!is_whole_document_request("What does this paragraph mean?"));
        assert!(references_other_page("What did page 2 say?", Some("Page 3")));
        assert!(!references_other_page("What does page 3 say?", Some("Page 3")));
    }

    #[test]
    fn adaptive_retrieval_continues_only_for_new_supporting_chunks() {
        let chunk = |index: i32, text: &str| crate::rag::RetrievedChunk {
            doc_id: "doc".into(),
            source: "source".into(),
            chunk_index: index,
            text: text.into(),
            distance: 0.0,
        };
        let previous = vec![chunk(0, "a poem by William Shakespeare")];
        let supported = vec![
            chunk(0, "a poem by William Shakespeare"),
            chunk(1, "another Shakespeare poem appears here"),
        ];
        let unsupported = vec![
            chunk(0, "a poem by William Shakespeare"),
            chunk(1, "a recipe for soup appears here"),
        ];

        assert!(retrieval_expansion_has_signal(
            "find all poems by Shakespeare",
            &previous,
            &supported,
        ));
        assert!(!retrieval_expansion_has_signal(
            "find all poems by Shakespeare",
            &previous,
            &unsupported,
        ));
        assert!(retrieval_expansion_has_signal(
            "what is this document about",
            &previous,
            &unsupported,
        ));
        assert_eq!(retrieval_support_count("find all poems by Shakespeare", &supported), 2);
    }
}
