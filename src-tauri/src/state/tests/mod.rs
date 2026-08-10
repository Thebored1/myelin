use super::*;


    use super::{
        assemble_note_context, assemble_targeted_write_context, assemble_user_content,
        authorize_tool_policy,
        canonical_wire_conversation, chat_history_to_messages, enforce_slot_cache_budget,
        duplicate_note_extension, ensure_packages, hashed_embedding, initial_note_body,
        native_metadata_app_path, parse_note_file, parse_tex_log, remove_native_metadata_sidecar,
        slugify, split_frontmatter, tokenize, unique_pdf_path, validate_native_body, warmup_prefix,
        wrap_bare_latex, write_native_metadata_sidecar, write_note_file, IndexScheduler,
        EMPTY_IPYNB,
    };
    use crate::models::{ChatMessage, NoteDocument};

mod support;
pub(crate) use support::*;
mod scheduler;
mod latex;
mod slots;
mod documents;
mod context;
