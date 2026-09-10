use super::super::core::*;
use ::anyhow::Result;

impl AppState {
    pub(crate) fn pause_section_cache_for_turn(&self) {
        self.inner.ai
            .section_cache_preempt
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.ai.section_cache_resume.notify_waiters();
    }

    pub(crate) fn resume_section_cache_after_turn(&self) {
        self.inner.ai
            .section_cache_preempt
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.inner.ai.section_cache_resume.notify_waiters();
    }

    pub(crate) fn install_turn_cancellation(
        &self,
        cancellation: crate::ai_turn::TurnCancellation,
    ) {
        *self.inner.ai.active_turn_cancel.lock() = Some(cancellation);
    }

    pub(crate) fn clear_turn_cancellation(&self) {
        *self.inner.ai.active_turn_cancel.lock() = None;
    }

    pub fn request_ai_cancel(&self) {
        if let Some(cancellation) = self.inner.ai.active_turn_cancel.lock().clone() {
            cancellation.cancel();
        }
    }

    pub fn ai_cancel_requested(&self) -> bool {
        self.inner.ai
            .active_turn_cancel
            .lock()
            .as_ref()
            .is_some_and(crate::ai_turn::TurnCancellation::is_cancelled)
    }

    /// The live conversation (real message array) for a note — empty if none yet.
    pub fn conversation(&self, note_id: &str) -> Vec<serde_json::Value> {
        if let Err(error) = validate_note_storage_id(note_id) {
            log::warn!("refusing conversation read for invalid note id: {error}");
            return Vec::new();
        }
        let mut conversations = self.inner.ai.conversations.lock();
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
        validate_note_storage_id(note_id)?;
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
        self.inner.ai
            .conversations
            .lock()
            .insert(note_id.to_string(), messages);
        Ok(())
    }

    /// Forget a note's live conversation (e.g. when the user clears chat).
    pub fn clear_conversation(&self, note_id: &str) -> Result<()> {
        validate_note_storage_id(note_id)?;
        self.inner.ai.conversations.lock().remove(note_id);
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
            log::warn!(
                "could not remove saved conversation {}: {error}",
                path.display()
            );
        }
        *self.inner.ai.active_slot_cache.lock() = None;
        Ok(())
    }

    pub(crate) fn note_by_id(&self, id: &str) -> Option<NoteDocument> {
        self.inner
            .runtime
            .read()
            .notes
            .get(id)
            .map(|note| note.document.clone())
    }

    pub fn is_tool_approval_required(&self) -> bool {
        self.inner.ai
            .require_tool_approval
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_require_tool_approval(&self, require: bool) {
        self.inner.ai
            .require_tool_approval
            .store(require, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn deterministic_tools_enabled(&self) -> bool {
        self.inner.ai
            .deterministic_tools
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn tool_gating_enabled(&self) -> bool {
        self.inner.ai
            .tool_gating
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
        if let Ok(mut guard) = self.inner.ai.sidecar.try_lock() {
            *guard = None;
        }
        Ok(())
    }

    pub fn register_pending_approval(&self, id: String, tx: tokio::sync::oneshot::Sender<bool>) {
        self.inner.ai.pending_approvals.lock().insert(id, tx);
    }

    pub fn resolve_tool_approval(&self, id: &str, approved: bool) {
        if let Some(tx) = self.inner.ai.pending_approvals.lock().remove(id) {
            let _ = tx.send(approved);
        }
    }

    /// Drop a pending approval without resolving it (timeout / cancellation).
    pub fn remove_pending_approval(&self, id: &str) {
        self.inner.ai.pending_approvals.lock().remove(id);
    }
}
