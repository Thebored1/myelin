//! Per-request tool filtering and generation policy for the runner.

use crate::harness;
use crate::protocol::Options;
use serde_json::Value;
use std::collections::HashSet;

pub(crate) const HISTORY_BUDGET: usize = 16_000;

pub(crate) fn max_tokens(
    requested: u32,
    chat_mode: bool,
    intent_is_tool: Option<bool>,
    targeted_write: bool,
) -> u32 {
    if chat_mode && intent_is_tool == Some(false) {
        requested.min(768)
    } else {
        requested.min(if targeted_write { 1024 } else { 4096 })
    }
}

pub(crate) fn request_schemas(tools: &Value) -> Value {
    if tools.is_array() {
        tools.clone()
    } else {
        Value::Array(Vec::new())
    }
}

pub(crate) struct GenerationPolicy {
    pub plan_len: usize,
    pub targeted_native: bool,
    pub stream_note_preview: bool,
    pub strict: bool,
    pub prompt_tools: bool,
    pub no_think: bool,
    pub call_only: bool,
}

pub(crate) fn effective_schemas(schemas: &Value, opts: &Options) -> Value {
    let mut effective = schemas.clone();
    if let Some(arr) = effective.as_array_mut() {
        if opts.narrow {
            arr.retain(|tool| {
                let name = tool["function"]["name"].as_str().unwrap_or("");
                !matches!(
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
            });
        }
        if !opts.tool_subset.is_empty() {
            let wanted: HashSet<String> = opts.tool_subset.iter().cloned().collect();
            arr.retain(|tool| wanted.contains(tool["function"]["name"].as_str().unwrap_or("")));
        }
    }
    effective
}

pub(crate) fn generation_policy(
    user_text: &str,
    effective_schemas: &Value,
    opts: &Options,
) -> GenerationPolicy {
    let has_tools = effective_schemas
        .as_array()
        .map(|tools| !tools.is_empty())
        .unwrap_or(false);
    let lexical_plan_len = if has_tools {
        harness::harness_decompose(user_text, effective_schemas).len()
    } else {
        0
    };
    let plan_len = if has_tools && opts.intent_is_tool == Some(true) && lexical_plan_len == 0 {
        1
    } else {
        lexical_plan_len
    };
    let stream_note_preview =
        crate::agent::should_stream_note_preview(user_text, opts.selection_scoped);
    let targeted_native = opts.targeted_write && has_tools && opts.intent_is_tool == Some(true);
    let strict = opts.strict
        || opts.narrow
        || (!opts.native_first
            && !targeted_native
            && (plan_len > 1 || (opts.prefers_prompt_tools && plan_len <= 1)));
    GenerationPolicy {
        plan_len,
        targeted_native,
        stream_note_preview,
        strict,
        prompt_tools: strict || opts.prompt_tools,
        no_think: opts.no_think && !strict,
        call_only: opts.call_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Options;
    use serde_json::json;

    fn schemas() -> Value {
        json!([
            {"function": {"name": "write_note"}},
            {"function": {"name": "search_notes"}}
        ])
    }

    #[test]
    fn schema_policy_keeps_read_only_tools_in_narrow_mode() {
        let mut opts = Options::default();
        opts.narrow = true;
        let filtered = effective_schemas(&schemas(), &opts);
        assert_eq!(filtered[0]["function"]["name"], "search_notes");
    }

    #[test]
    fn schema_policy_applies_named_subset() {
        let mut opts = Options::default();
        opts.tool_subset = vec!["write_note".into()];
        let filtered = effective_schemas(&schemas(), &opts);
        assert_eq!(filtered[0]["function"]["name"], "write_note");
    }

    #[test]
    fn token_caps_are_mode_specific() {
        assert_eq!(max_tokens(4096, true, Some(false), false), 768);
        assert_eq!(max_tokens(4096, false, Some(true), true), 1024);
        assert_eq!(max_tokens(9000, false, None, false), 4096);
    }
}
