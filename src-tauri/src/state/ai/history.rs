use super::super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub async fn save_chat_history(
        &self,
        note_id: String,
        chat_history: Vec<crate::models::ChatMessage>,
    ) -> Result<()> {
        let workspace = self.require_workspace()?;
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
        fs::create_dir_all(&chats_dir)?;
        let chats_path = chats_dir.join(format!("{}.chat.json", document.id));
        let tmp_chat_path = chats_dir.join(format!("{}.chat.tmp", document.id));
        fs::write(
            &tmp_chat_path,
            serde_json::to_string(&document.chat_history)?,
        )?;
        fs::rename(&tmp_chat_path, &chats_path)?;

        {
            let mut runtime = self.inner.runtime.write();
            if let Some(note) = runtime.notes.get_mut(&note_id) {
                note.document.chat_history = document.chat_history;
            }
        }
        Ok(())
    }

}
