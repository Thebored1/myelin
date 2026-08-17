use super::core::*;
use anyhow::Result;

impl AppState {
    /// Reuse a source document's RAG chunks when a byte-identical PDF is copied
    /// into an attachment-specific identity. This keeps annotations isolated
    /// without paying for a second tokenizer/embedding pass.
    pub(crate) async fn clone_document_ingestion(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<bool> {
        let entry = {
            let _guard = self.inner.note_ingest_manifest_lock.lock().await;
            self.load_note_ingestion_manifest()
                .entries
                .get(source_id)
                .cloned()
        };
        let Some(entry) = entry else {
            return Ok(false);
        };
        if !crate::rag_copy::clone_document(&self.rag_dir(), source_id, target_id).await? {
            return Ok(false);
        }
        let _guard = self.inner.note_ingest_manifest_lock.lock().await;
        let mut manifest = self.load_note_ingestion_manifest();
        manifest.entries.insert(target_id.to_string(), entry);
        self.save_note_ingestion_manifest(&manifest)?;
        Ok(true)
    }
}
