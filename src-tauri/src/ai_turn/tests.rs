use super::*;
use serde_json::json;

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
        section_context: false,
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
fn section_context_is_shared_before_chat_or_write_profile_tail() {
    let shared = "COMMON SECTION BYTES";
    let chat = AiTurnBuilder::build(AiTurnInput {
        mode: "chat",
        doc_type: "md",
        note_title: "Section note",
        system_context: shared,
        conversation: &[],
        question: "what is here?",
        mode_policy: "chat policy",
        turn_instructions: "",
        has_open_note: true,
        edit_thread: false,
        oversized: false,
        supports_tools: false,
        verbose_tool_schemas: false,
        section_context: true,
    });
    let write = AiTurnBuilder::build(AiTurnInput {
        mode: "write",
        doc_type: "md",
        note_title: "Section note",
        system_context: shared,
        conversation: &[],
        question: "write here",
        mode_policy: "write policy",
        turn_instructions: "cursor is armed",
        has_open_note: true,
        edit_thread: false,
        oversized: false,
        supports_tools: true,
        verbose_tool_schemas: false,
        section_context: true,
    });
    assert_eq!(chat.messages[0]["content"], shared);
    assert_eq!(write.messages[0]["content"], shared);
    assert_ne!(chat.messages[1]["content"], write.messages[1]["content"]);
    assert_eq!(chat.messages[1]["role"], "system");
    assert_eq!(write.messages[1]["role"], "system");
}

#[test]
fn chat_rewrite_is_a_direct_draft_not_a_read_only_tool_call() {
    let turn = build("chat", "rewrite the INTRODUCTION in a couple of lines");
    assert!(
        !turn.tools.is_empty(),
        "chat keeps its stable read-only schema prefix"
    );
    assert!(!turn.intent_is_tool);
    assert_eq!(turn.kind, TurnKind::DirectAnswer);
    assert!(!names(&turn).contains(&"write_note"));

    let operation = build("operation", "rewrite the INTRODUCTION in a couple of lines");
    assert!(operation.intent_is_tool);
    assert_eq!(operation.kind, TurnKind::ToolSelection);
}

#[test]
fn chat_always_offers_the_fixed_read_only_tool_set() {
    // Question-dependent tool gating would change the rendered prompt prefix
    // between turns; chat instead offers one stable read-only set and relies
    // on deterministic tool_intent for routing.
    let expected = [
        "fetch_web_page",
        "find_in_note",
        "read_note",
        "search_documents",
        "search_notes",
        "web_search",
    ];
    assert_eq!(
        names(&build("chat", "does this note contain aardvark?")),
        expected
    );
    assert_eq!(
        names(&build("chat", "summarize https://example.com")),
        expected
    );
    assert_eq!(
        names(&build("chat", "search the web for rust news")),
        expected
    );
    assert_eq!(
        names(&build("chat", "search my other notes for pasta")),
        expected
    );
    assert_eq!(
        names(&build("chat", "what does my PDF say about attention?")),
        expected
    );
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
        section_context: false,
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
        section_context: false,
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
        section_context: false,
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
        section_context: false,
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
        section_context: false,
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
        section_context: false,
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
    let query = contextual_retrieval_query("why did you say they did not have it?", &history);
    assert!(query.starts_with("Latest question: why did you say"));
    assert_eq!(query.matches("Latest question:").count(), 1);
    assert!(query.contains("LFM2 and MiniCPM"));
    assert!(!query.contains("They did not have a 2-bit result"));
}

#[test]
fn repeated_raw_question_keeps_retrieval_query_cacheable() {
    let question = "could you describe all the points mentioned in HARNESS DESIGN?";
    let history = vec![
        json!({"role": "user", "content": question}),
        json!({"role": "assistant", "content": "previous answer"}),
    ];
    assert_eq!(
        contextual_retrieval_query(question, &history),
        format!("Latest question: {question}")
    );
}

#[test]
fn independent_document_questions_drop_unrelated_history() {
    let history = vec![
        json!({"role": "user", "content": "What is GPTQ?"}),
        json!({"role": "assistant", "content": "A quantization method."}),
    ];
    assert!(
        compact_document_conversation("Does this paper mention MiniCPM-V?", &history).is_empty()
    );
    assert_eq!(
        compact_document_conversation("Why did you say that?", &history).len(),
        2
    );
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
    let memory = compact_document_conversation("How does Sonnet XVIII treat time?", &history);
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
    assert!(is_broad_retrieval_request(
        "what is the attached PDF about?"
    ));
    assert!(is_broad_retrieval_request("summarize this document"));
    assert!(!is_broad_retrieval_request(
        "what does the introduction say?"
    ));
    assert!(!is_broad_retrieval_request(
        "what does the paper say about transformers?"
    ));

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
    assert!(references_other_page(
        "What did page 2 say?",
        Some("Page 3")
    ));
    assert!(!references_other_page(
        "What does page 3 say?",
        Some("Page 3")
    ));
}

#[test]
fn adaptive_retrieval_continues_only_for_new_supporting_chunks() {
    let chunk = |index: i32, text: &str| crate::rag::RetrievedChunk {
        doc_id: "doc".into(),
        source: "source".into(),
        chunk_index: index,
        text: text.into(),
        distance: 0.0,
        ..Default::default()
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
    assert_eq!(
        retrieval_support_count("find all poems by Shakespeare", &supported),
        2
    );
}
