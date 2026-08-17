#[path = "context.rs"]
pub(super) mod context;
#[path = "documents/helpers.rs"]
pub(super) mod documents_helpers;
#[path = "documents/persistence.rs"]
pub(super) mod documents_persistence;
#[path = "latex_support.rs"]
pub(super) mod latex_support;
#[path = "search.rs"]
pub(super) mod search;
#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
#[path = "types.rs"]
pub(super) mod types;

/// Validate identifiers before they are interpolated into app-data filenames.
/// Note IDs are UUIDs today, but accepting a conservative filename-safe subset
/// also keeps older imported workspaces readable without permitting traversal.
pub(crate) fn validate_note_storage_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        anyhow::bail!("invalid note id")
    }
    Ok(())
}

pub(crate) use context::*;
pub(crate) use documents_helpers::*;
pub(crate) use documents_persistence::*;
pub(crate) use latex_support::*;
pub(crate) use search::*;
pub(crate) use types::*;

pub use latex_support::{compile_tex_source, TectonicCacheStatus};
pub use types::{
    AppState, BackgroundSettings, DocumentIngestionResult, LlamaCacheStatus, OpenharnSettings,
};
