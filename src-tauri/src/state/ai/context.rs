use super::super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub fn reset_chat_tools(&self) {
        self.inner.chat_tools.lock().clear();
    }

    pub fn record_chat_tool(&self, name: impl Into<String>, details: impl Into<String>) {
        self.inner.chat_tools.lock().push(ChatTool {
            name: name.into(),
            details: details.into(),
        });
    }

    pub fn take_chat_tools(&self) -> Vec<ChatTool> {
        std::mem::take(&mut *self.inner.chat_tools.lock())
    }

    pub fn set_latest_chat_question(&self, question: impl Into<String>) {
        *self.inner.latest_chat_question.lock() = Some(question.into());
    }

    pub fn clear_latest_chat_question(&self) {
        *self.inner.latest_chat_question.lock() = None;
    }

    /// The user's current chat message (for intent checks during tool calls).
    pub fn latest_chat_question(&self) -> String {
        self.inner
            .latest_chat_question
            .lock()
            .clone()
            .unwrap_or_default()
    }

    pub fn set_current_selection(&self, selection: Option<crate::agent::SelectionArg>) {
        *self.inner.current_selection.lock() = selection;
    }

    pub fn current_selection(&self) -> Option<crate::agent::SelectionArg> {
        self.inner.current_selection.lock().clone()
    }

    pub fn set_current_doc_type(&self, doc_type: Option<String>) {
        *self.inner.current_doc_type.lock() = doc_type;
    }

    pub fn current_doc_type(&self) -> String {
        self.inner
            .current_doc_type
            .lock()
            .clone()
            .unwrap_or_else(|| "md".to_string())
    }

    pub fn targeted_write_active(&self) -> bool {
        self.inner
            .targeted_write
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn pause_section_cache_for_turn(&self) {
        self.inner
            .section_cache_preempt
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.section_cache_resume.notify_waiters();
    }

    pub(crate) fn resume_section_cache_after_turn(&self) {
        self.inner
            .section_cache_preempt
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.inner.section_cache_resume.notify_waiters();
    }

    pub fn request_ai_cancel(&self) {
        self.inner
            .cancel_ai
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.cancel_notify.notify_waiters();
    }

    pub fn ai_cancel_requested(&self) -> bool {
        self.inner
            .cancel_ai
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn set_current_note_id(&self, note_id: impl Into<String>) {
        *self.inner.current_note_id.lock() = Some(note_id.into());
    }

    pub fn clear_current_note_id(&self) {
        *self.inner.current_note_id.lock() = None;
    }

    pub fn current_note_id(&self) -> Option<String> {
        self.inner.current_note_id.lock().clone()
    }

    /// The live conversation (real message array) for a note — empty if none yet.
    pub fn conversation(&self, note_id: &str) -> Vec<serde_json::Value> {
        let mut conversations = self.inner.conversations.lock();
        if !conversations.contains_key(note_id) {
            let path = self
                .inner
                .app_data_dir
                .join("conversations")
                .join(format!("{note_id}.json"));
            match fs::read_to_string(&path) {
                Ok(raw) => match serde_json::from_str::<Vec<serde_json::Value>>(&raw) {
                    Ok(messages) => {
                        conversations.insert(note_id.to_string(), canonical_wire_conversation(messages));
                    }
                    Err(error) => self.record_storage_issues([StorageIssue {
                        code: "conversation-parse".into(),
                        severity: "error".into(),
                        path: Some(path.display().to_string()),
                        message: format!("A saved chat conversation is malformed and was left untouched: {error}"),
                        recoverable: true,
                    }]),
                },
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => self.record_storage_issues([StorageIssue {
                    code: "conversation-read".into(),
                    severity: "error".into(),
                    path: Some(path.display().to_string()),
                    message: "A saved chat conversation could not be read and was left untouched.".into(),
                    recoverable: true,
                }]),
                Err(_) => {}
            }
        }
        conversations
            .get(note_id)
            .cloned()
            .map(canonical_wire_conversation)
            .unwrap_or_default()
    }

    /// Replace a note's live conversation after a turn (already trimmed by caller).
    pub fn save_conversation(&self, note_id: &str, msgs: Vec<serde_json::Value>) -> Result<()> {
        let messages = canonical_wire_conversation(msgs);
        let dir = self.inner.app_data_dir.join("conversations");
        let path = dir.join(format!("{note_id}.json"));
        if let Err(error) = fs::create_dir_all(&dir) {
            self.record_storage_issues([StorageIssue {
                code: "conversation-write".into(),
                severity: "error".into(),
                path: Some(path.display().to_string()),
                message: "Chat conversation could not be saved; the in-memory conversation remains available.".into(),
                recoverable: true,
            }]);
            return Err(error.into());
        }
        crate::persistence::atomic_write_json(&path, &messages).map_err(|error| {
            self.record_storage_issues([StorageIssue {
                code: "conversation-write".into(),
                severity: "error".into(),
                path: Some(path.display().to_string()),
                message: "Chat conversation could not be saved; the in-memory conversation remains available.".into(),
                recoverable: true,
            }]);
            error
        })?;
        self.inner
            .conversations
            .lock()
            .insert(note_id.to_string(), messages);
        Ok(())
    }

    /// Forget a note's live conversation (e.g. when the user clears chat).
    pub fn clear_conversation(&self, note_id: &str) {
        self.inner.conversations.lock().remove(note_id);
        let path = self
            .inner
            .app_data_dir
            .join("conversations")
            .join(format!("{note_id}.json"));
        if let Err(error) = crate::persistence::atomic_remove(&path) {
            self.record_storage_issues([StorageIssue {
                code: "conversation-delete".into(),
                severity: "warning".into(),
                path: Some(path.display().to_string()),
                message: "Saved chat conversation could not be removed.".into(),
                recoverable: true,
            }]);
            log::warn!("could not remove saved conversation {}: {error}", path.display());
        }
        *self.inner.active_slot_cache.lock() = None;
    }

    pub(crate) fn note_by_id(&self, id: &str) -> Option<NoteDocument> {
        self.inner
            .runtime
            .read()
            .notes
            .get(id)
            .map(|note| note.document.clone())
    }

    /// Body of the note currently open in the editor (for the find_in_note tool).
    pub fn open_note_body(&self) -> Option<String> {
        self.current_note_id()
            .and_then(|id| self.note_by_id(&id))
            .map(|doc| doc.body)
    }

    pub fn current_attachment_scope(&self) -> Vec<String> {
        let Some(id) = self.current_note_id() else {
            return Vec::new();
        };
        if let Some(note) = self.note_by_id(&id) {
            if let Some(source_id) = note.source_pdf {
                return vec![id, source_id];
            }
            if note.relative_path.to_ascii_lowercase().ends_with(".pdf") {
                return vec![id];
            }
        }
        self.oversized_doc_active().then_some(vec![id]).unwrap_or_default()
    }

    /// Resolve the note a chat tool should act on: always prefer the note that
    /// is currently open in the editor, regardless of the title the model
    /// passed (a model can get the title wrong). Fall back to an exact
    /// title match only when no note is open.
    pub fn resolve_chat_target_note(&self, title: &str) -> Option<NoteDocument> {
        if let Some(id) = self.current_note_id() {
            if let Some(doc) = self.note_by_id(&id) {
                return Some(doc);
            }
        }
        self.find_note_by_exact_title(title)
    }

    pub fn is_tool_approval_required(&self) -> bool {
        self.inner
            .require_tool_approval
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_require_tool_approval(&self, require: bool) {
        self.inner
            .require_tool_approval
            .store(require, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn deterministic_tools_enabled(&self) -> bool {
        self.inner
            .deterministic_tools
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn tool_gating_enabled(&self) -> bool {
        self.inner
            .tool_gating
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_turn_tool_policy(
        &self,
        chat_mode: bool,
        append_only: bool,
        placement_edit: bool,
        oversized_doc: bool,
        tools_supported: bool,
    ) {
        use std::sync::atomic::Ordering;
        self.inner.chat_mode.store(chat_mode, Ordering::SeqCst);
        self.inner.append_only.store(append_only, Ordering::SeqCst);
        self.inner.placement_edit.store(placement_edit, Ordering::SeqCst);
        self.inner.oversized_doc.store(oversized_doc, Ordering::SeqCst);
        self.inner.tools_supported.store(tools_supported, Ordering::SeqCst);
    }

    pub fn authorize_tool_call(&self, name: &str) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        authorize_tool_policy(
            name,
            self.inner.chat_mode.load(Ordering::SeqCst),
            self.inner.append_only.load(Ordering::SeqCst),
            self.inner.placement_edit.load(Ordering::SeqCst),
            self.current_selection().is_some(),
            self.current_doc_type() == "ipynb",
            self.inner.oversized_doc.load(Ordering::SeqCst),
            self.inner.tools_supported.load(Ordering::SeqCst),
        )
    }

    pub fn oversized_doc_active(&self) -> bool {
        self.inner
            .oversized_doc
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The openharn sidecar settings (port, binary override, harness tuning).
    pub fn openharn_settings(&self) -> OpenharnSettings {
        self.inner.openharn_settings.lock().clone()
    }

    /// Persist new openharn settings and update the live mirror. Drops any
    /// running sidecar so the next chat respawns it with the new port / binary /
    /// harness tuning (the running process keeps the old launch args).
    pub fn set_openharn_settings(&self, settings: OpenharnSettings) -> Result<()> {
        self.invalidate_ai_pipeline();
        let mut all = load_settings(&self.inner.app_data_dir)?;
        all.openharn = settings.clone();
        save_settings(&self.inner.app_data_dir, &all)?;
        *self.inner.openharn_settings.lock() = settings;
        if let Ok(mut guard) = self.inner.sidecar.try_lock() {
            *guard = None;
        }
        Ok(())
    }

    pub fn register_pending_approval(&self, id: String, tx: tokio::sync::oneshot::Sender<bool>) {
        self.inner.pending_approvals.lock().insert(id, tx);
    }

    pub fn resolve_tool_approval(&self, id: &str, approved: bool) {
        if let Some(tx) = self.inner.pending_approvals.lock().remove(id) {
            let _ = tx.send(approved);
        }
    }

    /// Drop a pending approval without resolving it (timeout / cancellation).
    pub fn remove_pending_approval(&self, id: &str) {
        self.inner.pending_approvals.lock().remove(id);
    }

}
