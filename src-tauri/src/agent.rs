mod approval;
mod contracts;
mod factory;
mod intent;
mod prompts;
mod routing;
mod selection;
mod tools;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_routing;

pub use contracts::{compact_tool_specs_for_profile, tool_specs, ReadNoteArgs, ToolError};
pub use factory::build_myelin_agent;
pub use intent::{
    append_request_intent, is_pure_note_write_request, is_small_talk, note_write_intent,
    placement_request_intent, wants_clear, wants_other_notes, wants_search,
};
pub use prompts::{DIRECT_CHAT_PREAMBLE, MYELIN_PREAMBLE, TARGETED_WRITE_PREAMBLE};
pub use routing::{
    chat_tool_intent, detect_format_op, in_edit_thread, interaction_mode_tools, select_chat_tools,
    select_tools, select_tools_cfg, targeted_write_tools, tool_intent, wants_documents,
    wants_fetch, wants_find, wants_partial_removal,
};
pub use selection::{
    apply_format_op, clean_note_content, find_tolerant, is_format_op, locate_selection,
    normalize_append_content, note_content_has_protocol_residue, plan_write,
    selection_insert_after_plan, selection_scoped_plan, strip_prompt_markers, SelectionArg,
    WriteOp, WritePlan, FORMAT_OPS,
};
pub use tools::note_edit::*;
pub use tools::note_read::*;
pub use tools::note_write::*;
pub use tools::notebook::*;
pub use tools::retrieval::*;
pub use tools::web::{html_to_text, normalize_web_url, *};

use approval::{check_tool_approval, WEB_BODY_CAP, WEB_FETCH_LIMIT};
use contracts::tool_contract;
use intent::{contains_any_word, existing_note_operation, has_web_domain, is_negated};
use routing::wants_targeted_deletion;
use tools::edit_notebook_params;
