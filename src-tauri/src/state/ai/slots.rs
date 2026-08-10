use super::super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub async fn save_active_slot_before_quit(&self) {
        let record = match self.inner.last_slot_save.lock().clone() {
            Some(record) => record,
            None => return,
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            self.save_note_slot(&record.0, &record.1).await;
        })
        .await;
    }
}
