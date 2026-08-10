use super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub(crate) async fn ensure_embed_server(&self) -> Result<String> {
        let model = llama_server::embed_model_path(&self.inner.app_data_dir).ok_or_else(|| {
            anyhow::anyhow!("no embedding model configured (set one in Settings)")
        })?;
        let model_path = std::path::PathBuf::from(&model);
        let config = llama_server::resolve_config(&self.inner.app_data_dir)?;
        let executable = llama_server::resolve_embedding_executable(&self.inner.app_data_dir)?;
        let host = config.host.clone();
        let port = config.port.saturating_add(1);
        let base = format!("http://{host}:{port}");

        let mut guard = self.inner.embed_server.lock().await;
        if let Some(server) = guard.as_ref() {
            let healthy = self
                .inner
                .llama_client
                .get(format!("{base}/health"))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if server.model_path == model_path
                && server.executable_path == executable
                && healthy
            {
                return Ok(base);
            }
            if let Some(mut old) = guard.take() {
                llama_server::stop_embed_server(&mut old).await;
            }
        }
        let server = llama_server::start_embed_server(
            &self.inner.llama_client,
            &executable,
            &model_path,
            &host,
            port,
        )
        .await?;
        *guard = Some(server);
        Ok(base)
    }

    /// Embed a batch of texts via the local embedding server (starting it if
    /// needed). GTE-small uses the same plain-text representation for queries
    /// and documents; `is_query` remains part of the API for callers.
    pub async fn embed_texts(&self, texts: &[String], is_query: bool) -> Result<Vec<Vec<f32>>> {
        let base = self.ensure_embed_server().await?;
        crate::embeddings::embed(
            &self.inner.llama_client,
            &base,
            "gte-small",
            texts,
            is_query,
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    /// LanceDB dir for the GTE-small document RAG store. The model/dimension is
    /// part of the directory name so an old 768-dimensional Nomic table cannot
    /// be queried with 384-dimensional GTE vectors.
    pub(crate) fn rag_dir(&self) -> PathBuf {
        self.inner.app_data_dir.join("rag-index-gte-small-384")
    }

    pub(crate) fn note_ingestion_manifest_path(&self) -> PathBuf {
        self.rag_dir().join(NOTE_INGEST_MANIFEST)
    }

    pub(crate) fn load_note_ingestion_manifest(&self) -> NoteIngestionManifest {
        fs::read_to_string(self.note_ingestion_manifest_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save_note_ingestion_manifest(&self, manifest: &NoteIngestionManifest) -> Result<()> {
        fs::create_dir_all(self.rag_dir())?;
        let path = self.note_ingestion_manifest_path();
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, serde_json::to_vec_pretty(manifest)?)?;
        fs::rename(temp, path)?;
        Ok(())
    }

    pub(crate) fn embedding_fingerprint(&self) -> String {
        let Some(path) = crate::llama_server::embed_model_path(&self.inner.app_data_dir) else {
            return "hashed-384-v1".to_string();
        };
        let metadata = fs::metadata(&path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let runtime = crate::llama_server::resolve_embedding_executable(&self.inner.app_data_dir)
            .ok();
        let runtime_metadata = runtime.as_ref().and_then(|path| fs::metadata(path).ok());
        let runtime_size = runtime_metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let runtime_modified = runtime_metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let runtime_path = runtime
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing-stock-cpu-runtime".to_string());
        format!(
            "gte-small-v1:{path}:{size}:{modified}:{runtime_path}:{runtime_size}:{runtime_modified}"
        )
    }

    pub(crate) fn query_embedding_cache_path(&self) -> PathBuf {
        self.inner.app_data_dir.join(QUERY_EMBEDDING_CACHE)
    }

    pub(crate) fn query_embedding_key(&self, query: &str) -> String {
        let normalized = query.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut hasher = Sha256::new();
        hasher.update(self.embedding_fingerprint().as_bytes());
        hasher.update(b"\0query\0");
        hasher.update(normalized.to_ascii_lowercase().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub(crate) fn cached_query_embedding(&self, query: &str) -> Option<Vec<f32>> {
        let fingerprint = self.embedding_fingerprint();
        let mut cache = self.inner.query_embedding_cache.lock();
        if cache.is_none() {
            *cache = fs::read(self.query_embedding_cache_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .filter(|file: &QueryEmbeddingCacheFile| file.embedding == fingerprint);
        }
        cache
            .as_ref()
            .and_then(|file| file.entries.get(&self.query_embedding_key(query)).cloned())
    }

    pub(crate) fn store_query_embedding(&self, query: &str, vector: Vec<f32>) {
        let fingerprint = self.embedding_fingerprint();
        let key = self.query_embedding_key(query);
        let mut cache = self.inner.query_embedding_cache.lock();
        let file = cache.get_or_insert_with(|| QueryEmbeddingCacheFile {
            embedding: fingerprint.clone(),
            entries: HashMap::new(),
        });
        if file.embedding != fingerprint {
            file.embedding = fingerprint;
            file.entries.clear();
        }
        if file.entries.len() >= 256 && !file.entries.contains_key(&key) {
            if let Some(oldest) = file.entries.keys().next().cloned() {
                file.entries.remove(&oldest);
            }
        }
        file.entries.insert(key, vector);
        if let Ok(bytes) = serde_json::to_vec(file) {
            let path = self.query_embedding_cache_path();
            let temp = path.with_extension("json.tmp");
            if fs::write(&temp, bytes).is_ok() {
                let _ = fs::rename(temp, path);
            }
        }
    }

    pub(crate) fn note_ingestion_entry(&self, body: &str) -> NoteIngestionEntry {
        let digest = Sha256::digest(body.as_bytes());
        NoteIngestionEntry {
            body_hash: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            chunker: NOTE_CHUNKER_VERSION.to_string(),
            embedding: self.embedding_fingerprint(),
        }
    }

    /// Ensure arbitrary extracted document text is present in the persistent RAG
    /// store. The content/model fingerprint prevents re-embedding unchanged PDFs
    /// every time their viewer is opened.
    pub async fn ensure_document_ingested(
        &self,
        doc_id: &str,
        source: &str,
        text: &str,
    ) -> Result<DocumentIngestionResult> {
        let lock = {
            let mut locks = self.inner.note_ingest_locks.lock();
            Arc::clone(
                locks
                    .entry(doc_id.to_string())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let _guard = lock.lock().await;
        let expected = self.note_ingestion_entry(text);
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let matches = self
                .load_note_ingestion_manifest()
                .entries
                .get(doc_id)
                == Some(&expected);
            if matches
                && (text.trim().is_empty()
                    || crate::rag::contains_document(&self.rag_dir(), doc_id)
                        .await
                        .unwrap_or(false))
            {
                return Ok(DocumentIngestionResult {
                    status: if text.trim().is_empty() { "empty" } else { "cached" }.into(),
                    chunks: 0,
                });
            }
        }

        let chunks = self.ingest_document(doc_id, source, text, false).await?;
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let mut manifest = self.load_note_ingestion_manifest();
            manifest.entries.insert(doc_id.to_string(), expected);
            self.save_note_ingestion_manifest(&manifest)?;
        }
        Ok(DocumentIngestionResult {
            status: if chunks == 0 { "empty" } else { "indexed" }.into(),
            chunks,
        })
    }

    pub(crate) async fn ensure_oversized_note_ingested(&self, note: &NoteDocument) -> Result<usize> {
        let lock = {
            let mut locks = self.inner.note_ingest_locks.lock();
            Arc::clone(
                locks
                    .entry(note.id.clone())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let _guard = lock.lock().await;
        let expected = self.note_ingestion_entry(&note.body);
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let manifest_matches = self
                .load_note_ingestion_manifest()
                .entries
                .get(&note.id)
                == Some(&expected);
            if manifest_matches
                && crate::rag::contains_document(&self.rag_dir(), &note.id)
                    .await
                    .unwrap_or(false)
            {
                return Ok(0);
            }
        }

        let _ = self.handle.emit(
            "ai://indexing_progress",
            serde_json::json!({
                "noteId": note.id,
                "status": "started",
                "message": "Indexing the complete oversized note…"
            }),
        );
        let chunks = match self
            .ingest_document(&note.id, &note.title, &note.body, false)
            .await
        {
            Ok(chunks) => chunks,
            Err(error) => {
                let _ = self.handle.emit(
                    "ai://indexing_progress",
                    serde_json::json!({
                        "noteId": note.id,
                        "status": "failed",
                        "message": error.to_string()
                    }),
                );
                return Err(error);
            }
        };
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let mut manifest = self.load_note_ingestion_manifest();
            manifest.entries.insert(note.id.clone(), expected);
            self.save_note_ingestion_manifest(&manifest)?;
        }
        let _ = self.handle.emit(
            "ai://indexing_progress",
            serde_json::json!({
                "noteId": note.id,
                "status": "done",
                "chunks": chunks
            }),
        );
        Ok(chunks)
    }

    pub(crate) fn note_has_ingestion_entry(&self, note_id: &str) -> bool {
        self.load_note_ingestion_manifest()
            .entries
            .contains_key(note_id)
    }

    /// Ingest a document into the RAG store: chunk → embed → store. Re-ingesting
    /// the same doc_id replaces its chunks. `contextual` (for the working doc /
    /// "deep index") embeds each chunk with a one-sentence LLM context that
    /// situates it in the document, while STORING the clean chunk text. Plain
    /// (sources) skips that for speed. Returns the number of chunks stored.
    pub async fn ingest_document(
        &self,
        doc_id: &str,
        source: &str,
        text: &str,
        contextual: bool,
    ) -> Result<usize> {
        // Keep a generous margin below GTE-small's 512-token input limit.
        // Word counts are only an approximation because PDF punctuation can
        // tokenize into multiple pieces.
        let chunks = crate::embeddings::chunk_text(text, 192, 32);
        if chunks.is_empty() {
            crate::rag::upsert_document(&self.rag_dir(), doc_id, Vec::new()).await?;
            return Ok(0);
        }

        // Contextual: one LLM summary of the doc, prepended to each chunk's
        // EMBED text (not its stored text) so the vector carries document context.
        let prefix = if contextual {
            let excerpt: String = text.chars().take(3000).collect();
            match self
                .run_llama_prompt(
                    "You write a single short sentence situating a document, for search context.",
                    &format!(
                        "Document:\n{excerpt}\n\nIn ONE sentence, say what this document is about (for retrieval context). Reply with only the sentence."
                    ),
                )
                .await
            {
                Ok(s) => format!("[Context: {}] ", s.trim()),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        };

        let embed_input: Vec<String> = chunks
            .iter()
            .map(|c| format!("{prefix}{}", c.text))
            .collect();
        let vectors = if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            self.embed_texts(&embed_input, false).await?
        } else {
            embed_input
                .iter()
                .map(|text| hashed_embedding(text))
                .collect()
        };
        let docs: Vec<crate::rag::DocChunk> = chunks
            .iter()
            .zip(vectors)
            .map(|(c, v)| crate::rag::DocChunk {
                doc_id: doc_id.to_string(),
                source: source.to_string(),
                chunk_index: c.index as i32,
                text: c.text.clone(),
                vector: v,
            })
            .collect();
        let n = docs.len();
        crate::rag::upsert_document(&self.rag_dir(), doc_id, docs).await?;
        Ok(n)
    }

    /// Remove a document's chunks from the RAG store.
    pub async fn delete_document(&self, doc_id: &str) -> Result<()> {
        crate::rag::upsert_document(&self.rag_dir(), doc_id, Vec::new()).await?;
        let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
        let mut manifest = self.load_note_ingestion_manifest();
        if manifest.entries.remove(doc_id).is_some() {
            self.save_note_ingestion_manifest(&manifest)?;
        }
        Ok(())
    }

    /// Embedding for a note / query: real GTE vectors when an embed model is
    /// configured (semantic search), else the lexical hashed fallback. Always
    /// EMBEDDING_DIM-wide so both paths are interchangeable.
    pub(crate) async fn note_embedding(&self, text: &str, is_query: bool) -> Vec<f32> {
        if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            let input: String = text.chars().take(4000).collect();
            if let Ok(mut v) = self.embed_texts(&[input], is_query).await {
                if let Some(vec) = v.pop() {
                    if vec.len() == EMBEDDING_DIM as usize {
                        return vec;
                    }
                }
            }
        }
        hashed_embedding(text)
    }

    /// Re-embed a batch of notes with real GTE vectors when an embed model is
    /// configured (one batched call); otherwise leaves their hashed vectors.
    pub(crate) async fn reembed_notes(&self, notes: &mut [IndexedNote]) {
        if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_none() {
            return;
        }
        let inputs: Vec<String> = notes
            .iter()
            .map(|n| {
                let body: String = n.document.body.chars().take(4000).collect();
                format!("{}\n{}", n.document.title, body)
            })
            .collect();
        if let Ok(vectors) = self.embed_texts(&inputs, false).await {
            if vectors.len() == notes.len() {
                for (n, v) in notes.iter_mut().zip(vectors) {
                    if v.len() == EMBEDDING_DIM as usize {
                        n.vector = v;
                    } else {
                        n.vector = hashed_embedding(&n.document.body);
                    }
                }
            } else {
                for n in notes {
                    n.vector = hashed_embedding(&n.document.body);
                }
            }
        } else {
            for n in notes {
                n.vector = hashed_embedding(&n.document.body);
            }
        }
    }

    /// Retrieve the top-K document chunks most relevant to a query.
    pub async fn retrieve_chunks(
        &self,
        query: &str,
        k: usize,
        doc_id: Option<&str>,
    ) -> Result<Vec<crate::rag::RetrievedChunk>> {
        let doc_ids = doc_id.map(|id| vec![id.to_string()]);
        self.retrieve_chunks_scoped(query, k, doc_ids.as_deref()).await
    }

    /// Retrieve only from the active attachment workspace. An empty scope is
    /// intentionally equivalent to no results, never a workspace-wide search.
    pub async fn retrieve_chunks_scoped(
        &self,
        query: &str,
        k: usize,
        doc_ids: Option<&[String]>,
    ) -> Result<Vec<crate::rag::RetrievedChunk>> {
        if doc_ids.is_some_and(<[String]>::is_empty) {
            return Ok(Vec::new());
        }
        let qvec = if let Some(cached) = self.cached_query_embedding(query) {
            cached
        } else if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            let vector = self
                .embed_texts(&[query.to_string()], true)
                .await?
                .into_iter()
                .next()
                .unwrap_or_default();
            if !vector.is_empty() {
                self.store_query_embedding(query, vector.clone());
            }
            vector
        } else {
            let vector = hashed_embedding(query);
            self.store_query_embedding(query, vector.clone());
            vector
        };
        if qvec.is_empty() {
            return Ok(Vec::new());
        }
        crate::rag::search_hybrid(&self.rag_dir(), qvec, query, k, doc_ids).await
    }

}
