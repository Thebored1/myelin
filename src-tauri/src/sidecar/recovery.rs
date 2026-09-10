//! Conversation-delta validation and safe assistant-text recovery.

use anyhow::{anyhow, Result};
use serde_json::Value;

/// Convert a legacy full-conversation response into only the suffix that was
/// produced by the current request. Reject incompatible histories rather than
/// duplicating prior turns into persisted chat state.
pub(super) fn conversation_delta(submitted: &[Value], returned: &[Value]) -> Result<Vec<Value>> {
    if returned.len() < submitted.len() || returned[..submitted.len()] != *submitted {
        return Err(anyhow!(
            "sidecar returned neither new_messages nor a compatible conversation suffix"
        ));
    }
    Ok(returned[submitted.len()..].to_vec())
}

/// Recover only a non-empty assistant answer from the current-turn messages.
/// Tool-call assistant messages are deliberately excluded.
pub(super) fn recoverable_assistant_text(messages: &[Value]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        (message["role"].as_str() == Some("assistant") && message["tool_calls"].is_null())
            .then(|| message["content"].as_str())
            .flatten()
            .map(str::trim)
            .filter(|content| !content.is_empty())
            .map(str::to_string)
    })
}
