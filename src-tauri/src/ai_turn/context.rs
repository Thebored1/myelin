use crate::agent::SelectionArg;
use crate::models::ActiveSection;
use crate::models::{ChatTool, NoteDocument};
use crate::state::AppState;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Immutable inputs captured for one chat turn after request validation.
/// Keeping these values together prevents later async retrieval/tool work from
/// accidentally reading mutable per-request state.
#[derive(Debug, Clone)]
pub struct ChatTurnContext {
    pub note_id: String,
    pub mode: String,
    pub question: String,
    pub selection: Option<SelectionArg>,
    pub active_section: Option<ActiveSection>,
    pub retrieval_backed: bool,
    pub retrieval_evidence_ready: bool,
    pub external_model: bool,
    pub context_size: usize,
    pub supports_tools: bool,
}

/// The immutable policy and request data exposed to tools for one AI turn.
///
/// This deliberately owns the values that used to live in `InnerState` as
/// mutable "current" fields. A tool can now only act on the note, selection,
/// question, and policy captured for its request, even if another request is
/// queued or a background task is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnMode {
    Chat,
    Auto,
    Write,
    Operation,
    Edit,
}

impl TurnMode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "chat" => Self::Chat,
            "auto" => Self::Auto,
            "write" => Self::Write,
            "operation" => Self::Operation,
            "edit" => Self::Edit,
            _ => return None,
        })
    }

    pub fn is_chat(self) -> bool {
        matches!(self, Self::Chat)
    }

    pub fn is_targeted_write(self) -> bool {
        matches!(self, Self::Write)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TurnPolicy {
    pub require_tool_approval: bool,
    pub deterministic_tools: bool,
    pub tool_gating: bool,
    pub targeted_write: bool,
    pub chat_mode: bool,
    pub append_only: bool,
    pub placement_edit: bool,
    pub oversized_doc: bool,
    pub notebook: bool,
    pub tools_supported: bool,
}

/// A cancellation source owned by one request. `AppState` only keeps the
/// currently active handle so the public cancel command can find it; the
/// cancellation state itself is never shared between turns.
#[derive(Clone, Debug)]
pub struct TurnCancellation {
    cancelled: Arc<AtomicBool>,
    signal: tokio::sync::watch::Sender<bool>,
}

impl TurnCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.signal.send(true);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        let mut signal = self.signal.subscribe();
        if self.is_cancelled() {
            return;
        }
        while signal.changed().await.is_ok() {
            if self.is_cancelled() {
                return;
            }
        }
    }
}

impl Default for TurnCancellation {
    fn default() -> Self {
        let (signal, _) = tokio::sync::watch::channel(false);
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            signal,
        }
    }
}

/// Context shared by the sidecar event loop and every tool instance spawned
/// for that loop. The tool log is local to the request, not global application
/// state, so completion/error events cannot consume another turn's tools.
#[derive(Clone)]
pub struct ToolTurnContext {
    pub state: AppState,
    pub note_id: String,
    pub question: String,
    pub selection: Option<SelectionArg>,
    pub doc_type: String,
    pub mode: TurnMode,
    pub policy: TurnPolicy,
    pub cancellation: TurnCancellation,
    tools: Arc<Mutex<Vec<ChatTool>>>,
}

impl ToolTurnContext {
    pub fn new(
        state: AppState,
        note_id: String,
        question: String,
        selection: Option<SelectionArg>,
        doc_type: String,
        mode: TurnMode,
        policy: TurnPolicy,
        cancellation: TurnCancellation,
    ) -> Self {
        Self {
            state,
            note_id,
            question,
            selection,
            doc_type,
            mode,
            policy,
            cancellation,
            tools: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Compatibility context for the legacy in-process rig agent. It has no
    /// editor target, so a tool must use an explicit title/id argument.
    pub fn standalone(state: AppState) -> Self {
        Self::new(
            state,
            String::new(),
            String::new(),
            None,
            "md".into(),
            TurnMode::Auto,
            TurnPolicy {
                require_tool_approval: false,
                deterministic_tools: true,
                tool_gating: false,
                targeted_write: false,
                chat_mode: false,
                append_only: false,
                placement_edit: false,
                oversized_doc: false,
                notebook: false,
                tools_supported: true,
            },
            TurnCancellation::default(),
        )
    }

    pub fn record_tool(&self, name: impl Into<String>, details: impl Into<String>) {
        self.tools.lock().push(ChatTool {
            name: name.into(),
            details: details.into(),
        });
    }

    pub fn take_tools(&self) -> Vec<ChatTool> {
        std::mem::take(&mut *self.tools.lock())
    }

    pub fn resolve_target_note(&self, title: &str) -> Option<NoteDocument> {
        if !self.note_id.is_empty() {
            if let Some(note) = self.state.note_by_id(&self.note_id) {
                return Some(note);
            }
        }
        self.state.find_note_by_exact_title(title)
    }

    pub fn open_note_body(&self) -> Option<String> {
        self.resolve_target_note("").map(|note| note.body)
    }

    pub fn attachment_scope(&self) -> Vec<String> {
        let Some(note) = self.resolve_target_note("") else {
            return Vec::new();
        };
        if let Some(source_id) = note.source_pdf {
            return vec![note.id, source_id];
        }
        if note.relative_path.to_ascii_lowercase().ends_with(".pdf")
            || self.policy.oversized_doc
        {
            return vec![note.id];
        }
        Vec::new()
    }

    pub fn authorize_tool_call(&self, name: &str) -> Result<(), String> {
        crate::state::authorize_tool_policy(
            name,
            self.policy.chat_mode,
            self.policy.append_only,
            self.policy.placement_edit,
            self.selection.is_some(),
            self.policy.notebook,
            self.policy.oversized_doc,
            self.policy.tools_supported,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_owned_and_can_be_cloned_for_async_boundaries() {
        let context = ChatTurnContext {
            note_id: "note-1".into(),
            mode: "chat".into(),
            question: "hello".into(),
            selection: None,
            active_section: None,
            retrieval_backed: false,
            retrieval_evidence_ready: false,
            external_model: true,
            context_size: 8192,
            supports_tools: false,
        };
        let copy = context.clone();
        assert_eq!(copy.note_id, "note-1");
        assert_eq!(copy.context_size, 8192);
    }

    #[test]
    fn turn_modes_are_explicit_and_unknown_modes_are_rejected() {
        assert_eq!(TurnMode::parse("chat"), Some(TurnMode::Chat));
        assert_eq!(TurnMode::parse("write"), Some(TurnMode::Write));
        assert_eq!(TurnMode::parse("operation"), Some(TurnMode::Operation));
        assert_eq!(TurnMode::parse("unknown"), None);
    }

    #[tokio::test]
    async fn cancellation_wakes_only_the_cloned_request_token() {
        let first = TurnCancellation::default();
        let second = TurnCancellation::default();
        let waiter = {
            let first = first.clone();
            tokio::spawn(async move {
                first.cancelled().await;
                first.is_cancelled()
            })
        };
        second.cancel();
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        first.cancel();
        assert!(waiter.await.expect("cancellation waiter panicked"));
    }
}
