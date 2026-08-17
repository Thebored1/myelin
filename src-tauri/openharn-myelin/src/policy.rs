//! Stable safety policy shared by routing, previews, and the tool loop.

pub(crate) fn is_terminal_mutation(name: &str, result: &str) -> bool {
    if !is_mutating_tool(name) {
        return false;
    }
    let result = result.trim_start().to_ascii_lowercase();
    result.contains("successfully updated") || result.starts_with("notebook cell updated")
}

pub(crate) fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "write_note"
            | "append_note"
            | "prepend_note"
            | "replace_in_note"
            | "insert_after_line"
            | "delete_in_note"
            | "format_note"
            | "edit_notebook"
    )
}

pub(crate) fn request_allows_replace_preview(user_text: &str) -> bool {
    let text = user_text.to_ascii_lowercase();
    !text.starts_with("add ")
        && ![
            "append",
            " below",
            "add below",
            "add to",
            "insert",
            "add a section",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
}

pub(crate) fn should_stream_note_preview(user_text: &str, selection_scoped: bool) -> bool {
    selection_scoped || request_allows_replace_preview(user_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_policy_is_explicit_and_terminal_only_on_success() {
        assert!(is_mutating_tool("write_note"));
        assert!(is_mutating_tool("edit_notebook"));
        assert!(!is_mutating_tool("search_notes"));
        assert!(is_terminal_mutation(
            "write_note",
            "Successfully updated note"
        ));
        assert!(!is_terminal_mutation("write_note", "validation failed"));
        assert!(!is_terminal_mutation(
            "search_notes",
            "Successfully updated note"
        ));
    }

    #[test]
    fn preview_policy_protects_append_and_insertion_requests() {
        assert!(request_allows_replace_preview("replace the note"));
        assert!(!request_allows_replace_preview(
            "add this below the heading"
        ));
        assert!(!request_allows_replace_preview("insert a paragraph"));
        assert!(should_stream_note_preview("insert a paragraph", true));
    }
}
