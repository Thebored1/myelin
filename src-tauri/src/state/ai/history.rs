use super::super::core::*;
use ::anyhow::{anyhow, Result};

impl AppState {
    pub async fn save_chat_history(
        &self,
        note_id: String,
        chat_history: Vec<crate::models::ChatMessage>,
    ) -> Result<()> {
        let workspace = self.require_workspace()?;
        let _persistence_guard = self.inner.persistence_lock.lock();
        let mut document = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .cloned()
                .map(|n| n.document)
                .ok_or_else(|| anyhow!("note not found"))?
        };

        document.chat_history = chat_history;

        let chats_dir = self.workspace_data_dir(&workspace).join("chats");
        let chats_path = chats_dir.join(format!("{}.chat.json", document.id));
        crate::persistence::atomic_write_json(&chats_path, &document.chat_history)?;

        {
            let mut runtime = self.inner.runtime.write();
            if let Some(note) = runtime.notes.get_mut(&note_id) {
                note.document.chat_history = document.chat_history;
            }
        }
        Ok(())
    }
}
