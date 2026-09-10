use super::*;

#[test]
fn slot_cache_budget_evicts_oldest_snapshots_and_keeps_manifests_together() {
    let dir = tempfile::tempdir().unwrap();
    let write_slot = |name: &str, bytes: usize, age_secs: u64| {
        let path = dir.path().join(name);
        std::fs::write(&path, vec![b'x'; bytes]).unwrap();
        std::fs::write(dir.path().join(format!("{name}.json")), b"{}").unwrap();
        let modified =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 - age_secs);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    };
    write_slot("note-a.slot", 400, 100); // oldest
    write_slot("note-b.slot", 400, 50);
    write_slot("note-c.slot", 400, 0); // newest
                                       // Orphaned manifests are not snapshots and never count toward the budget.
    std::fs::write(dir.path().join("note-x.slot.json"), b"{}").unwrap();

    enforce_slot_cache_budget(dir.path(), 900);

    assert!(
        !dir.path().join("note-a.slot").exists(),
        "oldest snapshot is evicted first"
    );
    assert!(
        !dir.path().join("note-a.slot.json").exists(),
        "manifest is evicted with its snapshot"
    );
    assert!(dir.path().join("note-b.slot").exists());
    assert!(dir.path().join("note-c.slot").exists());
    assert!(dir.path().join("note-x.slot.json").exists());
}

#[test]
fn slot_cache_budget_does_nothing_under_budget() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note-a.slot"), vec![b'x'; 100]).unwrap();
    std::fs::write(dir.path().join("note-a.slot.json"), b"{}").unwrap();
    enforce_slot_cache_budget(dir.path(), 1024);
    assert!(dir.path().join("note-a.slot").exists());
}

#[test]
fn warmup_prefix_matches_real_chat_turn_prefix() {
    let title = "Parity note";
    let excerpt = "some note body text";
    let (system, tools) = warmup_prefix(title, excerpt, None, "chat", "md", true, false, false);
    let turn = crate::ai_turn::AiTurnBuilder::build(crate::ai_turn::AiTurnInput {
        mode: "chat",
        doc_type: "md",
        note_title: title,
        system_context: &assemble_note_context(title, excerpt, None),
        conversation: &[],
        question: "hello",
        mode_policy: "policy",
        turn_instructions: "",
        has_open_note: true,
        edit_thread: false,
        oversized: false,
        supports_tools: true,
        verbose_tool_schemas: false,
        section_context: false,
    });
    assert_eq!(turn.messages[0]["content"].as_str().unwrap(), system);
    assert_eq!(
        serde_json::to_string(&turn.tools).unwrap(),
        serde_json::to_string(&tools).unwrap()
    );
}

#[test]
fn warmup_prefix_matches_real_targeted_write_turn_prefix() {
    let title = "Write parity note";
    let excerpt = "section source text";
    let context = assemble_targeted_write_context(title, excerpt, None);
    let (system, tools) = warmup_prefix(title, excerpt, None, "write", "md", true, false, false);
    let turn = crate::ai_turn::AiTurnBuilder::build(crate::ai_turn::AiTurnInput {
        mode: "write",
        doc_type: "md",
        note_title: title,
        system_context: &context,
        conversation: &[],
        question: "write a replacement",
        mode_policy: "write policy",
        turn_instructions: "cursor is armed",
        has_open_note: true,
        edit_thread: false,
        oversized: false,
        supports_tools: true,
        verbose_tool_schemas: false,
        section_context: false,
    });
    assert_eq!(turn.messages[0]["content"].as_str().unwrap(), system);
    assert_eq!(
        serde_json::to_string(&turn.tools).unwrap(),
        serde_json::to_string(&tools).unwrap()
    );
    assert!(system.contains("never reproduce surrounding content"));
}

#[test]
fn warmup_prefix_retrieval_chat_uses_tool_free_profile() {
    let (system, tools) = warmup_prefix(
        "Big note", "excerpt", None, "chat", "md", false, false, false,
    );
    assert!(tools.is_empty());
    let turn = crate::ai_turn::AiTurnBuilder::build(crate::ai_turn::AiTurnInput {
        mode: "chat",
        doc_type: "md",
        note_title: "Big note",
        system_context: &assemble_note_context("Big note", "excerpt", None),
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
    assert_eq!(turn.messages[0]["content"].as_str().unwrap(), system);
    assert!(turn.tools.is_empty());
}
