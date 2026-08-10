use super::*;
use crate::state::AppState;
use serde_json::Value;
    use super::*;

    const NOTE: &str = "Cars are fast. They have engines. People drive them daily.";

    #[test]
    fn tool_specs_matches_contract_table() {
        let specs = tool_specs();
        assert_eq!(specs.len(), TOOL_CONTRACTS.len(), "every contract has a spec");
        for (name, _, _) in TOOL_CONTRACTS {
            assert!(
                specs.iter().any(|s| s["function"]["name"] == *name),
                "missing spec for {name}"
            );
        }
        // The wire schema must expose doc_id so the model can scope a search.
        let sd = specs
            .iter()
            .find(|s| s["function"]["name"] == "search_documents")
            .expect("search_documents spec");
        assert!(
            sd["function"]["parameters"]["properties"].get("doc_id").is_some(),
            "search_documents must advertise doc_id"
        );
    }

    #[test]
    fn negation_matching_ignores_words_that_contain_not() {
        // "note"/"know" contain "not"/"no" as substrings — must NOT read as negation.
        assert!(wants_find("does this note contain aardvark?"));
        assert!(!wants_find("do not find anything in the note"));
    }

    #[test]
    fn clean_note_content_converts_repeated_asterisk_line_separators() {
        let malformed = "**Beneath the sky, the river flows,** *Where stones whisper secrets, soft and low.* *The wind holds still, the world is calm,** *A mirrored path where shadows call.* *In twilight's hush, the stars align,** *A quiet night, a sacred sign.*";
        assert_eq!(
            clean_note_content(malformed),
            "**Beneath the sky, the river flows,**\nWhere stones whisper secrets, soft and low.\nThe wind holds still, the world is calm,\nA mirrored path where shadows call.\nIn twilight's hush, the stars align,\nA quiet night, a sacred sign."
        );
    }

    #[test]
    fn clean_note_content_preserves_ordinary_markdown_emphasis() {
        let ordinary = "This is *italic* and **bold**.";
        assert_eq!(clean_note_content(ordinary), ordinary);
    }

    #[test]
    fn generated_note_protocol_residue_is_rejected_without_false_positive() {
        assert!(note_content_has_protocol_residue(
            "# Essay\nUseful prose.\n/content>} > write_note(content="
        ));
        assert!(!note_content_has_protocol_residue(
            "# API\nCall `write_note(content)` in this example."
        ));
    }

    #[test]
    fn clean_note_content_normalizes_break_variants() {
        assert_eq!(
            clean_note_content("First<br>Second Third\\nFourth"),
            "First\nSecond\nThird\nFourth"
        );
    }

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

    // The "deleted the entire note" bug: "remove all headings" must NOT read as a
    // request to clear the note (the destructive-write guard relies on this), but
    // an explicit wipe must.
    // The exact case the 1B model failed: "remove all headings" must keep every
    // line and drop only the leading # markers — done in code, not by the model.
    #[test]
    fn format_op_strips_headings_keeps_text() {
        let body =
            "## Intro\nPersonal computers changed everything.\n### History\nIt began in the 1970s.";
        assert_eq!(
            detect_format_op("remove all headings"),
            Some("remove_headings")
        );
        assert_eq!(
            apply_format_op(body, "remove_headings"),
            "Intro\nPersonal computers changed everything.\nHistory\nIt began in the 1970s."
        );
        assert_eq!(detect_format_op("make the title a heading"), None);
    }

    #[test]
    fn format_op_removals() {
        assert_eq!(
            apply_format_op("a **bold** word", "remove_bold"),
            "a bold word"
        );
        assert_eq!(
            apply_format_op("a *italic* word", "remove_italic"),
            "a italic word"
        );
        // Italic-only must leave bold markers intact.
        assert_eq!(
            apply_format_op("**b** and *i*", "remove_italic"),
            "**b** and i"
        );
        assert_eq!(
            apply_format_op("**b** and *i*", "remove_emphasis"),
            "b and i"
        );
        assert_eq!(
            apply_format_op("- one\n- two", "remove_bullets"),
            "one\ntwo"
        );
        assert_eq!(
            apply_format_op("1. one\n2. two", "remove_numbering"),
            "one\ntwo"
        );
        assert_eq!(
            apply_format_op("see [Rust](https://r.org) here", "remove_links"),
            "see Rust here"
        );
        // remove_links keeps images.
        assert_eq!(
            apply_format_op("![p](a.png) and [x](y)", "remove_links"),
            "![p](a.png) and x"
        );
        assert_eq!(
            apply_format_op("![p](a.png) text", "remove_images"),
            " text"
        );
        assert_eq!(
            apply_format_op("use `code` now", "remove_code"),
            "use code now"
        );
        assert_eq!(
            apply_format_op("> quoted\n> more", "remove_blockquotes"),
            "quoted\nmore"
        );
        assert_eq!(
            apply_format_op("a ~~no~~ b", "remove_strikethrough"),
            "a no b"
        );
        assert_eq!(
            apply_format_op("x\n\n---\n\ny", "remove_horizontal_rules"),
            "x\n\n\ny"
        );
        assert_eq!(
            apply_format_op("a\n\n\n\nb", "remove_blank_lines"),
            "a\n\nb"
        );
        assert_eq!(
            apply_format_op("# H\n- **b** [l](u)", "strip_markdown"),
            "H\nb l"
        );
    }

    #[test]
    fn format_op_conversions() {
        assert_eq!(
            apply_format_op("# Title\nbody", "headings_to_bold"),
            "**Title**\nbody"
        );
        assert_eq!(
            apply_format_op("**Title**\nbody", "bold_to_headings"),
            "# Title\nbody"
        );
        assert_eq!(apply_format_op("## A\n# B", "promote_headings"), "# A\n# B");
        assert_eq!(
            apply_format_op("# A\n## B", "demote_headings"),
            "## A\n### B"
        );
        assert_eq!(
            apply_format_op("- a\n- b\n- c", "bullets_to_numbered"),
            "1. a\n2. b\n3. c"
        );
        assert_eq!(
            apply_format_op("1. a\n2. b", "numbered_to_bullets"),
            "- a\n- b"
        );
        assert_eq!(
            apply_format_op("- [ ] todo\n- [x] done", "tasks_to_bullets"),
            "- todo\n- done"
        );
        assert_eq!(apply_format_op("Hi There", "uppercase"), "HI THERE");
        assert_eq!(apply_format_op("Hi There", "lowercase"), "hi there");
        assert_eq!(apply_format_op("hello world", "title_case"), "Hello World");
    }

    #[test]
    fn detect_format_op_routes_and_guards() {
        assert_eq!(detect_format_op("strip the bold"), Some("remove_bold"));
        assert_eq!(
            detect_format_op("get rid of the bullet points"),
            Some("remove_bullets")
        );
        assert_eq!(detect_format_op("remove the links"), Some("remove_links"));
        assert_eq!(
            detect_format_op("remove all the images"),
            Some("remove_images")
        );
        assert_eq!(
            detect_format_op("strip all formatting"),
            Some("strip_markdown")
        );
        assert_eq!(detect_format_op("make it all uppercase"), Some("uppercase"));
        assert_eq!(
            detect_format_op("convert the bullets to a numbered list"),
            Some("bullets_to_numbered")
        );
        assert_eq!(
            detect_format_op("turn the numbered list into bullets"),
            Some("numbered_to_bullets")
        );
        assert_eq!(
            detect_format_op("change the headings to bold"),
            Some("headings_to_bold")
        );
        // Never hijack a request to CREATE fresh content.
        assert_eq!(detect_format_op("write a numbered list of fruits"), None);
        assert_eq!(detect_format_op("write this note in uppercase"), None);
        assert_eq!(detect_format_op("make the title a heading"), None);
        // Every op the detector returns must be applicable.
        for op in FORMAT_OPS {
            assert_eq!(apply_format_op("unchanged", "bogus_op"), "unchanged");
            assert!(is_format_op(op));
        }
    }

    #[test]
    fn wants_clear_is_narrow() {
        assert!(!wants_clear("remove all headings"));
        assert!(!wants_clear("remove the bullet points"));
        assert!(!wants_clear("delete the introduction"));
        assert!(wants_clear("clear the note"));
        assert!(wants_clear("delete everything"));
        assert!(wants_clear("erase the note and start over"));
        assert!(wants_clear("make it blank"));
    }

    #[test]
    fn wants_partial_removal_pure_deletes_only() {
        // Pure removals of a part → true.
        assert!(wants_partial_removal("remove the My Take section"));
        assert!(wants_partial_removal("delete the second paragraph"));
        assert!(wants_partial_removal("get rid of the bulleted list"));
        assert!(wants_partial_removal("take out the heading"));
        // Whole-note clears are handled elsewhere → false.
        assert!(!wants_partial_removal("delete everything"));
        assert!(!wants_partial_removal("clear the note"));
        // Removal mixed with new/changed content is a real edit → false.
        assert!(!wants_partial_removal(
            "delete the intro and add a conclusion"
        ));
        assert!(!wants_partial_removal("replace the first paragraph"));
        assert!(!wants_partial_removal(
            "rewrite the summary without the last line"
        ));
        // Non-removal requests → false.
        assert!(!wants_partial_removal("make the title a heading"));
        assert!(!wants_partial_removal("what does this note say"));
    }

    #[test]
    fn surgical_delete_removes_only_find_from_real_body() {
        // The path the override produces: mode "edit", empty content, find = the
        // block to remove. plan_write must delete exactly that, byte-faithfully.
        let body = "# Title\n\nKeep this paragraph.\n\n**My Take:**\nRemove me.";
        let plan = plan_write(body, "", "edit", "**My Take:**\nRemove me.").unwrap();
        assert_eq!(plan.op, WriteOp::EditSnippet);
        assert_eq!(plan.new_body, "# Title\n\nKeep this paragraph.\n\n");
    }

    #[test]
    fn gating_off_protects_write_note_but_keeps_search() {
        let has = |v: &[Value], name: &str| {
            v.iter()
                .any(|t| t["function"]["name"].as_str() == Some(name))
        };
        // gating OFF (default), deterministic ON.
        // A capability question must NOT get write_note (the "what can you do →
        // wrote the title" misfire), but read/search stay available.
        let q = select_tools_cfg("what can you do", true, false, false, true);
        assert!(
            !has(&q, "write_note"),
            "a question must not offer write_note"
        );
        assert!(has(&q, "web_search"), "read/search stay on with gating off");
        // An explicit edit request still gets write_note.
        let e = select_tools_cfg("rewrite the introduction", true, false, false, true);
        assert!(has(&e, "write_note"));
        // A verb-less follow-up in an edit thread keeps write_note.
        let f = select_tools_cfg("no, shorter", true, true, false, true);
        assert!(has(&f, "write_note"));
        // Web search is never withheld (the thing brittle gating used to break).
        let s = select_tools_cfg(
            "search the web for the latest rust release",
            true,
            false,
            false,
            true,
        );
        assert!(has(&s, "web_search"));
        assert!(!has(&s, "write_note"));
    }

    #[test]
    fn fresh_whole_note_creation_uses_only_write_note() {
        let has = |v: &[Value], name: &str| {
            v.iter()
                .any(|t| t["function"]["name"].as_str() == Some(name))
        };

        for tools in [
            select_tools_cfg("write a poem", true, false, true, true),
            select_tools_cfg("write a poem", true, false, false, true),
        ] {
            assert!(has(&tools, "write_note"));
            for mutation in [
                "append_note",
                "prepend_note",
                "replace_in_note",
                "insert_after_line",
                "delete_in_note",
            ] {
                assert!(!has(&tools, mutation), "unexpected tool: {mutation}");
            }
        }

        // Existing-note rewrites still need the broader edit choices.
        let rewrite = select_tools_cfg("rewrite the introduction", true, false, true, true);
        assert!(has(&rewrite, "write_note"));
        assert!(has(&rewrite, "replace_in_note"));

        // Additions still expose the append path; the state-layer append-only
        // filter removes the overwrite and targeted-edit tools before dispatch.
        let append = select_tools_cfg("add a poem", true, false, true, true);
        assert!(has(&append, "append_note"));
    }

    #[test]
    fn locate_selection_disambiguates_repeats_via_context() {
        let body = "Cats are nice.\n\nDogs are loyal.\n\nCats are nice.";
        // The phrase occurs twice; the `before` anchor pins the SECOND one.
        let sel = SelectionArg {
            text: "Cats are nice.".into(),
            before: "loyal.\n\n".into(),
            after: "".into(),
            cursor: false,
            cell_index: None,
        };
        let (s, e) = locate_selection(body, &sel).unwrap();
        assert_eq!(s, body.rfind("Cats are nice.").unwrap());
        assert_eq!(&body[s..e], "Cats are nice.");
    }

    #[test]
    fn locate_selection_tolerates_markers_and_trailing_whitespace() {
        // Real failure from "New note 8": the user selected a BOLD paragraph, so
        // the captured text dropped the ** markers and ran past with newlines.
        let body = "# Food\n\n## Intro\n\n**Food is essential and good.**\n\n## More\n\n- a";
        let sel = SelectionArg {
            text: "Food is essential and good.\n\n".into(),
            before: "Intro\n\n**".into(),
            after: "**\n\n## More".into(),
            cursor: false,
            cell_index: None,
        };
        let (s, e) = locate_selection(body, &sel).unwrap();
        assert_eq!(&body[s..e], "Food is essential and good.");
        // Splicing keeps the surrounding ** and the rest of the note.
        let plan =
            selection_scoped_plan(body, "Food is vital, nutritious, and cultural.", &sel).unwrap();
        assert_eq!(
            plan.new_body,
            "# Food\n\n## Intro\n\n**Food is vital, nutritious, and cultural.**\n\n## More\n\n- a"
        );
    }

    #[test]
    fn selection_scoped_plan_replaces_only_the_selected_span() {
        let body = "Intro line.\n\nOld paragraph here.\n\nClosing line.";
        let sel = SelectionArg {
            text: "Old paragraph here.".into(),
            before: "Intro line.\n\n".into(),
            after: "\n\nClosing line.".into(),
            cursor: false,
            cell_index: None,
        };
        let plan = selection_scoped_plan(body, "New paragraph.", &sel).unwrap();
        assert_eq!(plan.op, WriteOp::EditSnippet);
        assert_eq!(
            plan.new_body,
            "Intro line.\n\nNew paragraph.\n\nClosing line."
        );
    }

    #[test]
    fn selection_scoped_plan_can_delete_only_the_selected_span() {
        let body = "Keep this.\n\nRemove this.\n\nKeep that.";
        let sel = SelectionArg {
            text: "Remove this.".into(),
            before: "Keep this.\n\n".into(),
            after: "\n\nKeep that.".into(),
            cursor: false,
            cell_index: None,
        };
        let plan = selection_scoped_plan(body, "", &sel).unwrap();
        assert_eq!(plan.new_body, "Keep this.\n\n\n\nKeep that.");
    }

    #[test]
    fn cursor_scoped_plan_inserts_at_the_unique_anchor() {
        let body = "Alpha beta.";
        let sel = SelectionArg {
            text: String::new(),
            before: "Alpha ".into(),
            after: "beta.".into(),
            cursor: true,
            cell_index: None,
        };
        let plan = selection_scoped_plan(body, "bright", &sel).unwrap();
        assert_eq!(plan.new_body, "Alpha bright beta.");
    }

    #[test]
    fn cursor_scoped_plan_rejects_ambiguous_anchor() {
        let sel = SelectionArg {
            text: String::new(),
            before: String::new(),
            after: String::new(),
            cursor: true,
            cell_index: None,
        };
        assert!(selection_scoped_plan("abc", "x", &sel).is_none());
    }

    #[test]
    fn cursor_scoped_plan_inserts_into_newline_normalized_empty_note() {
        let sel = SelectionArg {
            text: String::new(),
            before: "\n".to_string(),
            after: String::new(),
            cursor: true,
            cell_index: None,
        };
        let plan = selection_scoped_plan("\n", "A useful essay.", &sel).unwrap();
        assert_eq!(plan.new_body, "A useful essay.");
    }

    #[test]
    fn selection_scoped_plan_defers_when_model_regenerated_whole_note() {
        let body = "Intro line.\n\nOld paragraph here.\n\nClosing line.";
        let sel = SelectionArg {
            text: "Old paragraph here.".into(),
            before: "Intro line.\n\n".into(),
            after: "\n\nClosing line.".into(),
            cursor: false,
            cell_index: None,
        };
        // Model returned the WHOLE note (contains the after-anchor text) → fall
        // through to normal planning (None) instead of splicing the whole note in.
        let whole = "Intro line.\n\nNew paragraph.\n\nClosing line.";
        assert!(selection_scoped_plan(body, whole, &sel).is_none());
    }

    // Some models echo the prompt's note-framing markers into a direct tool call;
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
        assert_eq!(
            strip_prompt_markers("body\n-- end current note ---"),
            "body"
        );
        assert_eq!(strip_prompt_markers("clean note"), "clean note");
        // Bled "--- Title" delimiter (dashes + text on one line) is stripped...
        assert_eq!(strip_prompt_markers("--- Example Domain"), "Example Domain");
        // ...but a real horizontal rule (dashes alone on a line) is preserved.
        assert_eq!(
            strip_prompt_markers("# Title\n\n---\nmore"),
            "# Title\n\n---\nmore"
        );
    }

