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
        let intent_is_tool = matches!(input.mode, "operation" | "edit") || !tools.is_empty();
        let preamble = if input.mode == "chat" && tools.is_empty() {
            crate::agent::DIRECT_CHAT_PREAMBLE
        } else {
            crate::agent::MYELIN_PREAMBLE
        };
        let direct_chat = input.mode == "chat" && tools.is_empty();
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
    question: &str,
    has_open_note: bool,
    edit_thread: bool,
    oversized: bool,
) -> Vec<Value> {
    match mode {
        "chat" => {
            let names: &[&str] = if crate::agent::wants_other_notes(question) {
                if oversized {
                    &["search_notes", "read_note"]
                } else {
                    &["search_notes"]
                }
            } else if crate::agent::wants_search(question) {
                &["web_search", "fetch_web_page"]
            } else if crate::agent::wants_fetch(question) {
                &["fetch_web_page"]
            } else if crate::agent::wants_documents(question) {
                &["search_documents"]
            } else if oversized && has_open_note && crate::agent::wants_find(question) {
                &["find_in_note"]
            } else {
                &[]
            };
            crate::agent::tool_specs()
                .into_iter()
                .filter(|tool| {
                    tool["function"]["name"]
                        .as_str()
                        .is_some_and(|name| names.contains(&name))
                })
                .collect()
        }
        "edit" => crate::agent::interaction_mode_tools("edit", oversized),
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

/// Build a retrieval query that gives the current question the greatest weight,
/// but carries enough bounded conversational context to resolve follow-ups.
pub fn contextual_retrieval_query(question: &str, history: &[Value]) -> String {
    const HISTORY_CHARS: usize = 600;
    let prior_user = history.iter().rev().find_map(|message| {
        (message["role"].as_str() == Some("user"))
            .then(|| message["content"].as_str())
            .flatten()
    });
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
    let mut query = format!("Latest question: {question}\nLatest question: {question}");
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
    fn direct_chat_has_no_schema_and_keeps_real_title_metadata() {
        let turn = build("chat", "What is this note about?");
        assert!(turn.tools.is_empty());
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
    fn chat_routes_only_relevant_read_tools() {
        assert!(names(&build("chat", "does this note contain aardvark?")).is_empty());
        assert_eq!(names(&build("chat", "summarize https://example.com")), ["fetch_web_page"]);
        assert_eq!(
            names(&build("chat", "search the web for rust news")),
            ["fetch_web_page", "web_search"]
        );
        assert_eq!(
            names(&build("chat", "search my other notes for pasta")),
            ["search_notes"]
        );
        assert_eq!(
            names(&build("chat", "what does my PDF say about attention?")),
            ["search_documents"]
        );
    }

    #[test]
    fn oversized_note_enables_exact_find() {
        let mut turn = build("chat", "does this note contain aardvark?");
        assert!(turn.tools.is_empty());
        turn = AiTurnBuilder::build(AiTurnInput {
            mode: "chat",
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
        assert_eq!(names(&turn), ["find_in_note"]);
    }

    #[test]
    fn strong_profile_schemas_have_no_descriptions() {
        let turn = build("chat", "summarize https://example.com");
        let rendered = serde_json::to_string(&turn.tools).unwrap();
        assert!(!rendered.contains("description"));
    }

    #[test]
    fn direct_chat_uses_small_preamble() {
        let turn = build("chat", "hello");
        let system = turn.messages[0]["content"].as_str().unwrap();
        assert!(system.starts_with(crate::agent::DIRECT_CHAT_PREAMBLE));
        assert!(!system.contains("Worked examples"));
        assert!(system.len() < 1_000);
    }

    #[test]
    fn direct_chat_history_stays_raw_and_policy_appears_once() {
        let conversation = vec![
            json!({"role": "user", "content": "first question"}),
            json!({"role": "assistant", "content": "first answer"}),
        ];
        let turn = AiTurnBuilder::build(AiTurnInput {
            mode: "chat",
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
        assert_eq!(query.matches("Latest question:").count(), 2);
        assert!(query.contains("LFM2 and MiniCPM"));
        assert!(query.contains("They did not have a 2-bit result"));
    }
}
