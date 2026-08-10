use super::*;

    #[test]
    fn embedding_is_stable() {
        assert_eq!(
            hashed_embedding("alpha beta"),
            hashed_embedding("alpha beta")
        );
        assert_eq!(tokenize("Alpha, beta!").len(), 2);
    }

    #[test]
    fn excerpt_truncates_multibyte_text_on_character_boundaries() {
        let body = format!("{}✅ trailing text", "word ".repeat(100));
        let result = super::excerpt(&body);
        assert!(result.ends_with("..."));
        assert!(result.is_char_boundary(result.len() - 3));
        assert_eq!(result.chars().count(), 403);
    }

    #[test]
    fn restored_chat_history_contains_only_text_turns() {
        let history = vec![
            ChatMessage {
                role: "user".into(),
                content: "note A question".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".into(),
                content: "note A answer".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "tool".into(),
                content: "should not be restored".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".into(),
                content: "failed turn".into(),
                error: Some(true),
                ..Default::default()
            },
        ];

        let messages = chat_history_to_messages(&history);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["content"], "note A question");
        assert_eq!(messages[1]["content"], "note A answer");
    }

    #[test]
    fn canonical_conversation_rejects_system_and_generation_primers() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": "duplicate"}),
            serde_json::json!({
                "role": "user",
                "content": "OPEN NOTE TITLE: \"Note\"\n\nINTERNAL TURN POLICY:\npolicy\n\nUSER REQUEST:\nquestion",
                "metadata": {"interaction_mode": "chat"}
            }),
            serde_json::json!({"role": "assistant", "content": "<think></think>"}),
            serde_json::json!({"role": "assistant", "content": "answer"}),
            serde_json::json!({"role": "tool", "content": "result"}),
        ];
        let canonical = canonical_wire_conversation(messages);
        assert_eq!(canonical.len(), 3);
        assert_eq!(canonical[0]["role"], "user");
        // The rendered wire form is preserved verbatim (byte-parity for the KV
        // cache); only metadata, system rows, and empty think placeholders drop.
        assert_eq!(
            canonical[0]["content"],
            "OPEN NOTE TITLE: \"Note\"\n\nINTERNAL TURN POLICY:\npolicy\n\nUSER REQUEST:\nquestion"
        );
        assert!(canonical[0].get("metadata").is_none());
        assert_eq!(canonical[1]["content"], "answer");
        assert_eq!(canonical[2]["role"], "tool");
    }

    #[test]
    fn note_context_always_contains_body_in_the_shared_shape() {
        let context = assemble_note_context("Today", "body text", None);
        assert!(context.contains("Today"));
        assert!(context.contains("body text"));
        assert!(context.contains("--- CURRENT NOTE ---"));
    }

    #[test]
    fn decorated_wire_user_content_is_stable_for_persistence() {
        let sent = assemble_user_content(
            "Project Aurora",
            "CHAT TURN POLICY: Never modify the note.",
            "TURN RULE",
            "What changed?",
        );
        let persisted = serde_json::json!({ "role": "user", "content": sent.clone() });
        assert_eq!(persisted["content"], sent);
        assert!(sent.starts_with("OPEN NOTE TITLE: \"Project Aurora\""));
        assert!(sent.contains("INTERNAL TURN POLICY (not note metadata):"));
        assert!(sent.contains("CHAT TURN POLICY:"));
        assert!(!sent.contains("COMPOSER MODE"));
        assert!(sent.ends_with("USER REQUEST:\nWhat changed?"));
    }

    #[test]
    fn empty_note_context_keeps_the_real_title() {
        let context = assemble_note_context("Untitled idea", "", None);
        assert!(context.contains("titled \"Untitled idea\""));
        assert!(context.contains("CURRENT content is empty"));
    }

    #[test]
    fn runtime_policy_blocks_unsafe_mutations() {
        assert!(authorize_tool_policy("write_note", true, false, false, false, false, false, true).is_err());
        assert!(authorize_tool_policy("write_note", false, true, false, false, false, false, true).is_err());
        assert!(authorize_tool_policy("append_note", false, true, false, false, false, false, true).is_ok());
        assert!(authorize_tool_policy("append_note", false, false, false, true, false, false, true).is_err());
        assert!(authorize_tool_policy("write_note", false, false, false, true, false, false, true).is_ok());
        assert!(authorize_tool_policy("insert_after_line", false, false, true, true, false, false, true).is_ok());
        assert!(authorize_tool_policy("write_note", false, false, false, false, true, false, true).is_err());
        assert!(authorize_tool_policy("edit_notebook", false, false, false, false, true, false, true).is_ok());
        assert!(authorize_tool_policy("edit_notebook", false, false, false, true, true, false, true).is_ok());
        assert!(authorize_tool_policy("write_note", false, false, false, true, true, false, true).is_err());
        assert!(authorize_tool_policy("read_note", false, false, false, false, false, false, false).is_err());
        let blocked = authorize_tool_policy("write_note", false, false, false, false, false, true, true)
            .unwrap_err();
        assert!(blocked.contains("exceeds the model context"));
        assert!(authorize_tool_policy("format_note", false, false, false, false, false, true, true).is_err());
        assert!(authorize_tool_policy("replace_in_note", false, false, false, false, false, true, true).is_ok());
    }

