use super::*;
use serde_json::Value;
const NOTE: &str = "Cars are fast. They have engines. People drive them daily.";
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
    assert!(
        w.len() >= 6,
        "expected at least 6 note tools, got {}",
        w.len()
    );
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
