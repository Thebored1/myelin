use crate::ai_turn::ToolTurnContext;
use tauri::Emitter;
const TOOL_APPROVAL_TIMEOUT_SECS: u64 = 120;

/// Removes the registered approval entry when the wait completes any way: user
/// decision, timeout, cancellation, or the awaiting task being dropped. Without
/// it a cancelled turn leaks a dead sender in `pending_approvals` forever.
struct PendingApprovalGuard<'a> {
    turn: &'a ToolTurnContext,
    id: String,
}

impl Drop for PendingApprovalGuard<'_> {
    fn drop(&mut self) {
        self.turn.state.remove_pending_approval(&self.id);
    }
}

pub(super) async fn check_tool_approval(
    turn: &ToolTurnContext,
    tool_name: &str,
    title: &str,
    content_preview: &str,
) -> Result<(), String> {
    if !turn.policy.require_tool_approval {
        return Ok(());
    }
    let req_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    turn.state.register_pending_approval(req_id.clone(), tx);
    let _guard = PendingApprovalGuard {
        turn,
        id: req_id.clone(),
    };

    let _ = turn.state.handle.emit(
        "ai://tool_approval_request",
        serde_json::json!({
            "id": req_id,
            "tool": tool_name,
            "title": title,
            "content": content_preview
        }),
    );

    tokio::select! {
        result = rx => match result {
            Ok(true) => Ok(()),
            Ok(false) => Err("User rejected this action.".to_string()),
            Err(_) => Err("Approval request cancelled.".to_string()),
        },
        _ = turn.cancellation.cancelled() => {
            Err("Cancelled by the user while awaiting approval; no changes were made.".to_string())
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(TOOL_APPROVAL_TIMEOUT_SECS)) => {
            Err("Approval request timed out after 120 seconds; no changes were made.".to_string())
        }
    }
}

pub(super) const WEB_FETCH_LIMIT: usize = 6_000;
pub(super) const WEB_BODY_CAP: usize = 256 * 1024;
