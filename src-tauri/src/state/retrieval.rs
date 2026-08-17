use super::core::*;
use ::anyhow::{anyhow, Context, Result};

impl AppState {
    /// Ask the already-running chat server for alternate retrieval queries.
    /// This path never starts a model and is used only for explicit complex requests.
    pub(crate) async fn plan_complex_retrieval(&self, query: &str) -> Vec<String> {
        let analyzed = crate::retrieval_pipeline::RetrievalQuery::analyze(query);
        if !matches!(
            analyzed.intent,
            crate::retrieval_pipeline::RetrievalIntent::Comparison
                | crate::retrieval_pipeline::RetrievalIntent::Synthesis
                | crate::retrieval_pipeline::RetrievalIntent::MultiHop
        ) {
            return Vec::new();
        }
        let Ok(guard) = self.inner.ai.llama_server.try_lock() else {
            return Vec::new();
        };
        if guard.is_none() {
            return Vec::new();
        }
        drop(guard);
        let config = match crate::llama_server::resolve_config(&self.inner.app_data_dir) {
            Ok(config) => config,
            Err(_) => return Vec::new(),
        };
        let base = format!("http://{}:{}", config.host, config.port);
        let healthy = self
            .inner
            .llama_client
            .get(format!("{base}/health"))
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false);
        if !healthy {
            return Vec::new();
        }
        let body = serde_json::json!({"model": config.model_path.file_name().and_then(|name| name.to_str()).unwrap_or("local-chat"), "messages": [{"role": "system", "content": "Return JSON only: {\"queries\":[up to 3 concise alternate search queries]}. Do not answer the user."}, {"role": "user", "content": query}], "max_tokens": 96, "temperature": 0});
        let response = match tokio::time::timeout(
            std::time::Duration::from_millis(600),
            self.inner
                .llama_client
                .post(format!("{base}/v1/chat/completions"))
                .json(&body)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) if response.status().is_success() => response,
            _ => return Vec::new(),
        };
        let value: serde_json::Value = match response.json().await {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };
        let content = value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let parsed: serde_json::Value = match serde_json::from_str(content.trim()) {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };
        let Some(queries) = parsed.get("queries").and_then(|queries| queries.as_array()) else {
            return Vec::new();
        };
        let original = analyzed.normalized.to_lowercase();
        queries
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|query| {
                !query.is_empty() && query.to_lowercase() != original && query.len() <= 512
            })
            .take(3)
            .map(str::to_string)
            .collect()
    }

    pub(crate) async fn ensure_reranker_server(&self) -> Result<(String, usize)> {
        let raw = crate::llama_server::reranker_model_path(&self.inner.app_data_dir)
            .ok_or_else(|| anyhow!("no reranker model configured"))?;
        let model_path = fs::canonicalize(raw).context("reranker model path does not exist")?;
        let executable =
            crate::llama_server::resolve_reranker_executable(&self.inner.app_data_dir)?;
        let context = crate::gguf::read_gguf_info(&model_path)
            .ok()
            .and_then(|info| info.context_length)
            .unwrap_or(512)
            .clamp(128, 8192) as usize;
        let chat = crate::llama_server::resolve_config(&self.inner.app_data_dir)
            .ok()
            .map(|config| config.model_path);
        let embed =
            crate::llama_server::embed_model_path(&self.inner.app_data_dir).map(PathBuf::from);
        if chat.as_ref() == Some(&model_path) || embed.as_ref() == Some(&model_path) {
            anyhow::bail!("reranker model must differ from the chat and embedding models");
        }
        let config = crate::llama_server::resolve_config(&self.inner.app_data_dir)?;
        let host = config.host;
        let port = config.port.saturating_add(2);
        let base = format!("http://{host}:{port}");
        let mut guard = self.inner.ai.reranker_server.lock().await;
        if let Some(server) = guard.as_ref() {
            let healthy = self
                .inner
                .llama_client
                .get(format!("{base}/health"))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if healthy && server.model_path == model_path && server.executable_path == executable {
                return Ok((base, server.context_tokens));
            }
        }
        if let Some(mut old) = guard.take() {
            crate::llama_server::stop_reranker_server(&mut old).await;
        }
        let server = crate::llama_server::start_reranker_server(
            &self.inner.llama_client,
            &executable,
            &model_path,
            &host,
            port,
            context,
        )
        .await?;
        *guard = Some(server);
        Ok((base, context))
    }

    pub(crate) async fn validate_reranker_candidate(&self, raw: &str) -> Result<(PathBuf, usize)> {
        let model_path = fs::canonicalize(raw).context("reranker model path does not exist")?;
        if !model_path.is_file()
            || !model_path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        {
            anyhow::bail!("reranker model must be a readable .gguf file");
        }
        let executable =
            crate::llama_server::resolve_reranker_executable(&self.inner.app_data_dir)?;
        let context = crate::gguf::read_gguf_info(&model_path)
            .ok()
            .and_then(|info| info.context_length)
            .unwrap_or(512)
            .clamp(128, 8192) as usize;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .context("failed to reserve reranker validation port")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let mut server = crate::llama_server::start_reranker_server(
            &self.inner.llama_client,
            &executable,
            &model_path,
            "127.0.0.1",
            port,
            context,
        )
        .await?;
        let endpoint = format!("http://127.0.0.1:{port}/v1/rerank");
        let body = serde_json::json!({ "query": "What is the capital of France?", "documents": ["Paris is the capital of France.", "A banana is a fruit.", "Saturn has rings."], "top_n": 3 });
        let result = async {
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.inner.llama_client.post(endpoint).json(&body).send(),
            )
            .await
            .map_err(|_| anyhow!("reranker validation timed out"))??
            .error_for_status()?;
            let value: serde_json::Value = response.json().await?;
            let rows = value
                .get("results")
                .and_then(|value| value.as_array())
                .or_else(|| value.as_array())
                .ok_or_else(|| anyhow!("reranker response has no results array"))?;
            let mut seen = std::collections::HashSet::new();
            let first = rows
                .first()
                .and_then(|row| row.get("index"))
                .and_then(|value| value.as_u64())
                .ok_or_else(|| anyhow!("reranker response has no result indices"))?;
            for row in rows {
                let index = row
                    .get("index")
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| anyhow!("reranker result index is invalid"))?;
                let score = row
                    .get("score")
                    .or_else(|| row.get("relevance_score"))
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| anyhow!("reranker score is invalid"))?;
                if index >= 3 || !seen.insert(index) || !score.is_finite() {
                    anyhow::bail!("reranker returned malformed results");
                }
            }
            if first != 0 {
                anyhow::bail!("reranker validation did not rank the relevant passage first");
            }
            Ok(())
        }
        .await;
        crate::llama_server::stop_reranker_server(&mut server).await;
        result?;
        Ok((model_path, context))
    }

    pub(crate) async fn rerank_documents(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        if documents.len() > 12 {
            anyhow::bail!("reranking is capped at 12 candidates");
        }
        let (base, _) = self.ensure_reranker_server().await?;
        let response = tokio::time::timeout(std::time::Duration::from_millis(800), self.inner.llama_client.post(format!("{base}/v1/rerank")).json(&serde_json::json!({"query": query, "documents": documents, "top_n": documents.len()})).send()).await
            .map_err(|_| anyhow!("reranker timed out"))??.error_for_status()?;
        let value: serde_json::Value = response.json().await?;
        let rows = value
            .get("results")
            .and_then(|value| value.as_array())
            .or_else(|| value.as_array())
            .ok_or_else(|| anyhow!("reranker response has no results array"))?;
        if rows.len() != documents.len() {
            anyhow::bail!("reranker returned a partial result set");
        }
        let mut scores = vec![None; documents.len()];
        for row in rows {
            let index = row
                .get("index")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| anyhow!("reranker result index is invalid"))?
                as usize;
            let score =
                row.get("score")
                    .or_else(|| row.get("relevance_score"))
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| anyhow!("reranker score is invalid"))? as f32;
            if index >= scores.len() || !score.is_finite() || scores[index].replace(score).is_some()
            {
                anyhow::bail!("reranker returned malformed results");
            }
        }
        scores
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow!("reranker response omitted candidates"))
    }

    async fn rerank_documents_if_ready(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Option<Vec<f32>>> {
        {
            let circuit = self.inner.ai.reranker_circuit.lock();
            if circuit
                .disabled_until
                .is_some_and(|until| until > std::time::Instant::now())
            {
                return Ok(None);
            }
        }
        let Ok(guard) = self.inner.ai.reranker_server.try_lock() else {
            return Ok(None);
        };
        let Some(_server) = guard.as_ref() else {
            return Ok(None);
        };
        let config = crate::llama_server::resolve_config(&self.inner.app_data_dir)?;
        let base = format!("http://{}:{}", config.host, config.port.saturating_add(2));
        let healthy = self
            .inner
            .llama_client
            .get(format!("{base}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        drop(guard);
        if !healthy || documents.len() < 4 {
            return Ok(None);
        }
        match self.rerank_documents(query, documents).await {
            Ok(scores) => {
                let mut circuit = self.inner.ai.reranker_circuit.lock();
                circuit.failures = 0;
                circuit.disabled_until = None;
                Ok(Some(scores))
            }
            Err(error) => {
                let mut circuit = self.inner.ai.reranker_circuit.lock();
                circuit.failures = circuit.failures.saturating_add(1);
                if circuit.failures >= 3 {
                    circuit.disabled_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
                }
                Err(error)
            }
        }
    }
    /// Build all workspace-search rows before the index is replaced.  A
    /// configured model is all-or-nothing: if it cannot embed every chunk we
    /// publish a keyword-only index instead of mixing semantic spaces.
    pub(crate) async fn build_workspace_note_chunks(
        &self,
        notes: &[IndexedNote],
    ) -> Vec<WorkspaceNoteChunk> {
        let configured = crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some();
        let runtime = if configured {
            self.ensure_embed_server().await.ok()
        } else {
            None
        };
        let (fingerprint, dimension, counter, config) = if let Some(runtime) = runtime.as_ref() {
            let counter = crate::embeddings::ServerTokenCounter::new(
                &self.inner.llama_client,
                &runtime.base_url,
            );
            let prefix =
                crate::embeddings::TokenCounter::count(&counter, &runtime.contract.document_prefix)
                    .await
                    .unwrap_or(0);
            let hard = 384.min(runtime.contract.max_input_tokens.saturating_sub(prefix));
            if hard < 64 {
                return Vec::new();
            }
            (
                runtime.contract.fingerprint.clone(),
                runtime.contract.dimensions,
                Some(counter),
                crate::embeddings::ChunkerConfig {
                    target_tokens: 256.min(hard),
                    overlap_tokens: 0,
                    hard_max_tokens: hard,
                    min_tail_tokens: 64,
                },
            )
        } else {
            (
                "hashed-384-v1".to_string(),
                crate::embeddings::HASHED_EMBEDDING_DIMENSIONS,
                None,
                crate::embeddings::ChunkerConfig {
                    target_tokens: 256,
                    overlap_tokens: 0,
                    hard_max_tokens: 384,
                    min_tail_tokens: 64,
                },
            )
        };
        let mut rows = Vec::new();
        for note in notes {
            let source = if note.document.body.trim().is_empty() {
                note.document.title.clone()
            } else {
                note.document.body.clone()
            };
            if source.trim().is_empty() {
                continue;
            }
            let result = if let Some(counter) = counter.as_ref() {
                crate::embeddings::chunk_document(
                    &source,
                    crate::embeddings::DocumentFormat::from_path(&note.document.relative_path),
                    counter,
                    config,
                )
                .await
            } else {
                crate::embeddings::chunk_document(
                    &source,
                    crate::embeddings::DocumentFormat::from_path(&note.document.relative_path),
                    &crate::embeddings::ByteTokenCounter,
                    config,
                )
                .await
            };
            for chunk in result.chunks {
                rows.push(WorkspaceNoteChunk {
                    note_id: note.document.id.clone(),
                    title: note.document.title.clone(),
                    tags_text: note.document.tags.join(" "),
                    path: note.document.relative_path.clone(),
                    updated_at: note.document.updated_at.clone(),
                    chunk_index: chunk.index as i32,
                    lexical_text: crate::retrieval_pipeline::lexical_text(
                        &note.document.title,
                        None,
                        &format!("{}\n{}", note.document.tags.join(" "), chunk.text),
                    ),
                    text: chunk.text,
                    token_count: chunk.token_count as i32,
                    char_start: chunk.char_start.map(|v| v as i64),
                    char_end: chunk.char_end.map(|v| v as i64),
                    section_start: chunk.section_start,
                    section_end: chunk.section_end,
                    embedding_fingerprint: fingerprint.clone(),
                    vector: None,
                });
            }
        }
        if rows.is_empty() {
            return rows;
        }
        if !configured {
            for row in &mut rows {
                row.vector = Some(hashed_embedding(&format!(
                    "{}\n{}\n{}",
                    row.title, row.tags_text, row.text
                )));
            }
            return rows;
        }
        let inputs = rows
            .iter()
            .map(|row| format!("{}\n{}\n{}", row.title, row.tags_text, row.text))
            .collect::<Vec<_>>();
        match self
            .embed_texts(&inputs, crate::embeddings::EmbeddingInputKind::Document)
            .await
        {
            Ok(vectors)
                if vectors.len() == rows.len()
                    && vectors.iter().all(|vector| {
                        vector.len() == dimension && vector.iter().all(|value| value.is_finite())
                    }) =>
            {
                for (row, vector) in rows.iter_mut().zip(vectors) {
                    row.vector = Some(vector);
                }
            }
            Ok(_) | Err(_) => log::warn!(
                "[index] embedding workspace chunks failed; publishing keyword-only index"
            ),
        }
        rows
    }
    pub(crate) fn hashed_embedding_contract(&self) -> crate::embeddings::EmbeddingModelContract {
        crate::embeddings::EmbeddingModelContract {
            contract_version: crate::embeddings::EMBEDDING_CONTRACT_VERSION,
            fingerprint: "hashed-384-v1".into(),
            model_id: "hashed".into(),
            display_name: "Lexical hashed fallback".into(),
            model_path: PathBuf::new(),
            dimensions: crate::embeddings::HASHED_EMBEDDING_DIMENSIONS,
            server_context_tokens: 512,
            max_input_tokens: 384,
            pooling: None,
            query_prefix: String::new(),
            document_prefix: String::new(),
            normalization: crate::embeddings::EmbeddingNormalization::L2,
            profiled: false,
        }
    }

    async fn embedding_launch_spec(
        &self,
        model_path: &std::path::Path,
    ) -> Result<(
        crate::model_profiles::ResolvedProfile,
        usize,
        Option<crate::embeddings::EmbeddingPooling>,
    )> {
        let gguf = crate::gguf::read_gguf_info(model_path)
            .context("failed to read embedding GGUF metadata")?;
        let profile = crate::model_profiles::resolve(
            &self.inner.app_data_dir,
            Some(&gguf),
            &model_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
        );
        if profile.profile_name.is_some() && profile.role == crate::model_profiles::ModelRole::Chat
        {
            return Err(anyhow!(
                "the selected GGUF is recognized as a chat model, not an embedding model"
            ));
        }
        let context = profile
            .embedding_context_tokens
            .or_else(|| gguf.context_length.map(|value| value as usize))
            .unwrap_or(512)
            .clamp(128, 8192);
        let pooling = profile
            .embedding_pooling
            .as_deref()
            .map(crate::embeddings::EmbeddingPooling::parse)
            .transpose()
            .map_err(|error| anyhow!(error))?;
        Ok((profile, context, pooling))
    }

    async fn contract_for_server(
        &self,
        base: &str,
        model_path: PathBuf,
        executable: PathBuf,
        profile: crate::model_profiles::ResolvedProfile,
        context: usize,
        pooling: Option<crate::embeddings::EmbeddingPooling>,
    ) -> Result<crate::embeddings::EmbeddingModelContract> {
        let fallback_id = model_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("embedding-model");
        let model_id =
            crate::embeddings::server_model_id(&self.inner.llama_client, base, fallback_id).await;
        let dimensions = crate::embeddings::probe_embedding_server(
            &self.inner.llama_client,
            base,
            &model_id,
            profile.embedding_dimensions,
        )
        .await
        .map_err(|error| anyhow!(error))?;
        let metadata = fs::metadata(&model_path).ok();
        let size = metadata
            .as_ref()
            .map(|value| value.len())
            .unwrap_or_default();
        let modified = metadata
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_secs())
            .unwrap_or_default();
        let runtime = fs::metadata(&executable).ok();
        let runtime_size = runtime
            .as_ref()
            .map(|value| value.len())
            .unwrap_or_default();
        let runtime_modified = runtime
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_secs())
            .unwrap_or_default();
        let query_prefix = profile.embedding_query_prefix.unwrap_or_default();
        let document_prefix = profile.embedding_document_prefix.unwrap_or_default();
        let fingerprint = format!(
            "embedding-v{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{:?}",
            crate::embeddings::EMBEDDING_CONTRACT_VERSION,
            model_path.display(),
            size,
            modified,
            executable.display(),
            runtime_size,
            runtime_modified,
            dimensions,
            context,
            query_prefix,
            pooling
        );
        Ok(crate::embeddings::EmbeddingModelContract {
            contract_version: crate::embeddings::EMBEDDING_CONTRACT_VERSION,
            fingerprint,
            model_id,
            display_name: profile
                .profile_name
                .unwrap_or_else(|| fallback_id.to_string()),
            model_path,
            dimensions,
            server_context_tokens: context,
            max_input_tokens: context.saturating_sub(16),
            pooling,
            query_prefix,
            document_prefix,
            normalization: crate::embeddings::EmbeddingNormalization::L2,
            profiled: profile.verified,
        })
    }

    pub(crate) async fn ensure_embed_server(&self) -> Result<crate::embeddings::EmbeddingRuntime> {
        let model = llama_server::embed_model_path(&self.inner.app_data_dir).ok_or_else(|| {
            anyhow::anyhow!("no embedding model configured (set one in Settings)")
        })?;
        let model_path = std::path::PathBuf::from(&model);
        let config = llama_server::resolve_config(&self.inner.app_data_dir)?;
        let executable = llama_server::resolve_embedding_executable(&self.inner.app_data_dir)?;
        let (profile, context, pooling) = self.embedding_launch_spec(&model_path).await?;
        let host = config.host.clone();
        let port = config.port.saturating_add(1);
        let base = format!("http://{host}:{port}");

        let mut guard = self.inner.ai.embed_server.lock().await;
        if let Some(server) = guard.as_ref() {
            let healthy = self
                .inner
                .llama_client
                .get(format!("{base}/health"))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if server.model_path == model_path && server.executable_path == executable && healthy {
                if let Some(contract) = &server.contract {
                    return Ok(crate::embeddings::EmbeddingRuntime {
                        base_url: base,
                        contract: contract.clone(),
                    });
                }
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
            context,
            pooling,
        )
        .await?;
        let contract = self
            .contract_for_server(
                &base,
                model_path.clone(),
                executable.clone(),
                profile,
                context,
                pooling,
            )
            .await?;
        let mut server = server;
        server.contract = Some(contract.clone());
        *guard = Some(server);
        Ok(crate::embeddings::EmbeddingRuntime {
            base_url: base,
            contract,
        })
    }

    /// Probe a candidate on an isolated loopback port. The persisted setting and
    /// running production server remain unchanged until this succeeds.
    pub(crate) async fn validate_embedding_candidate(
        &self,
        raw_path: &str,
    ) -> Result<crate::embeddings::EmbeddingModelContract> {
        let model_path =
            fs::canonicalize(raw_path).context("embedding model path does not exist")?;
        if !model_path.is_file()
            || !model_path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
        {
            anyhow::bail!("embedding model must be a readable .gguf file");
        }
        let executable = llama_server::resolve_embedding_executable(&self.inner.app_data_dir)?;
        let (profile, context, pooling) = self.embedding_launch_spec(&model_path).await?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .context("failed to reserve a validation port")?;
        let port = listener
            .local_addr()
            .context("failed to inspect validation port")?
            .port();
        drop(listener);
        let mut server = llama_server::start_embed_server(
            &self.inner.llama_client,
            &executable,
            &model_path,
            "127.0.0.1",
            port,
            context,
            pooling,
        )
        .await?;
        let base = format!("http://127.0.0.1:{port}");
        let result = self
            .contract_for_server(&base, model_path, executable, profile, context, pooling)
            .await;
        llama_server::stop_embed_server(&mut server).await;
        result
    }

    pub(crate) async fn invalidate_embedding_derived_data(&self) -> Result<()> {
        if let Some(mut server) = self.inner.ai.embed_server.lock().await.take() {
            llama_server::stop_embed_server(&mut server).await;
        }
        *self.inner.query_embedding_cache.lock() = None;
        let paths = [
            self.query_embedding_cache_path(),
            self.index_dir(),
            self.rag_dir(),
            self.inner.app_data_dir.join("rag-index-gte-small-384"),
        ];
        for path in paths {
            if path.is_dir() {
                fs::remove_dir_all(&path).with_context(|| {
                    format!("failed to reset derived embedding data {}", path.display())
                })?;
            } else if path.is_file() {
                fs::remove_file(&path).with_context(|| {
                    format!("failed to reset derived embedding cache {}", path.display())
                })?;
            }
        }
        if let Some(workspace) = self.inner.runtime.read().workspace_path.clone() {
            let _ = self.request_reindex(workspace, false);
        }
        Ok(())
    }

    /// Embed a batch of texts via the local embedding server (starting it if
    /// needed). GTE-small uses the same plain-text representation for queries
    /// and documents; `is_query` remains part of the API for callers.
    pub async fn embed_texts(
        &self,
        texts: &[String],
        kind: crate::embeddings::EmbeddingInputKind,
    ) -> Result<Vec<Vec<f32>>> {
        let runtime = self.ensure_embed_server().await?;
        let result = crate::embeddings::embed(&self.inner.llama_client, &runtime, texts, kind)
            .await
            .map_err(|e| anyhow!(e))?;
        log::info!(
            "[embedding] {} inputs in {} requests, {} tokens, {} ms",
            result.metrics.input_count,
            result.metrics.request_count,
            result.metrics.token_count,
            result.metrics.elapsed_ms
        );
        Ok(result.vectors)
    }

    pub(crate) fn rag_dir(&self) -> PathBuf {
        self.inner.app_data_dir.join("rag-index")
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

    pub(crate) fn save_note_ingestion_manifest(
        &self,
        manifest: &NoteIngestionManifest,
    ) -> Result<()> {
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
        let runtime =
            crate::llama_server::resolve_embedding_executable(&self.inner.app_data_dir).ok();
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
        format: Option<crate::embeddings::DocumentFormat>,
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
            let matches =
                self.load_note_ingestion_manifest().entries.get(doc_id) == Some(&expected);
            if matches
                && (text.trim().is_empty()
                    || crate::rag::contains_document(&self.rag_dir(), doc_id)
                        .await
                        .unwrap_or(false))
            {
                return Ok(DocumentIngestionResult {
                    status: if text.trim().is_empty() {
                        "empty"
                    } else {
                        "cached"
                    }
                    .into(),
                    chunks: 0,
                });
            }
        }

        let chunks = self
            .ingest_document(doc_id, source, text, false, format)
            .await?;
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

    pub(crate) async fn ensure_oversized_note_ingested(
        &self,
        note: &NoteDocument,
    ) -> Result<usize> {
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
            let manifest_matches =
                self.load_note_ingestion_manifest().entries.get(&note.id) == Some(&expected);
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
            .ingest_document(
                &note.id,
                &note.title,
                &note.body,
                false,
                Some(crate::embeddings::DocumentFormat::from_path(
                    &note.relative_path,
                )),
            )
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
        format: Option<crate::embeddings::DocumentFormat>,
    ) -> Result<usize> {
        // Keep a generous margin below GTE-small's 512-token input limit.
        // Word counts are only an approximation because PDF punctuation can
        // tokenize into multiple pieces.
        let format = format.unwrap_or(crate::embeddings::DocumentFormat::PlainText);
        let chunk_started = std::time::Instant::now();
        let chunked = if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            let runtime = self.ensure_embed_server().await?;
            crate::embeddings::chunk_document(
                text,
                format,
                &crate::embeddings::ServerTokenCounter::new(
                    &self.inner.llama_client,
                    &runtime.base_url,
                ),
                crate::embeddings::RAG_CHUNKER,
            )
            .await
        } else {
            crate::embeddings::chunk_document(
                text,
                format,
                &crate::embeddings::ByteTokenCounter,
                crate::embeddings::RAG_CHUNKER,
            )
            .await
        };
        if let Some(warning) = &chunked.warning {
            log::warn!("[rag] {warning}");
        }
        log::info!(
            "[rag] chunked {} chunks in {} ms; tokenizer={} requests={}",
            chunked.chunks.len(),
            chunk_started.elapsed().as_millis(),
            chunked.mode.as_str(),
            chunked.tokenizer_requests
        );
        let chunks = chunked.chunks;
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
            self.embed_texts(
                &embed_input,
                crate::embeddings::EmbeddingInputKind::Document,
            )
            .await?
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
                lexical_text: crate::retrieval_pipeline::lexical_text(
                    source,
                    c.section_start.as_deref(),
                    &c.text,
                ),
                vector: v,
                token_count: c.token_count as i32,
                char_start: c.char_start.map(|value| value as i64),
                char_end: c.char_end.map(|value| value as i64),
                page_start: c.page_start,
                page_end: c.page_end,
                section_start: c.section_start.clone(),
                section_end: c.section_end.clone(),
                tokenizer_mode: c.tokenizer_mode.as_str().into(),
                chunker_version: crate::embeddings::CHUNKER_VERSION.into(),
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

    /// Embedding for a note/query. A configured model failure returns an empty
    /// vector so callers degrade to keyword search instead of mixing spaces.
    pub(crate) async fn note_embedding(&self, text: &str, is_query: bool) -> Vec<f32> {
        if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            let Ok(runtime) = self.ensure_embed_server().await else {
                return Vec::new();
            };
            let input = text.to_string();
            if let Ok(mut v) = self
                .embed_texts(
                    &[input],
                    if is_query {
                        crate::embeddings::EmbeddingInputKind::Query
                    } else {
                        crate::embeddings::EmbeddingInputKind::Document
                    },
                )
                .await
            {
                if let Some(vec) = v.pop() {
                    if vec.len() == runtime.contract.dimensions {
                        return vec;
                    }
                }
            }
            return Vec::new();
        }
        hashed_embedding(text)
    }

    /// Retrieve the top-K document chunks most relevant to a query.
    pub async fn retrieve_chunks(
        &self,
        query: &str,
        k: usize,
        doc_id: Option<&str>,
    ) -> Result<Vec<crate::rag::RetrievedChunk>> {
        let doc_ids = doc_id.map(|id| vec![id.to_string()]);
        self.retrieve_chunks_scoped(query, k, doc_ids.as_deref())
            .await
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
        let analyzed_query = crate::retrieval_pipeline::RetrievalQuery::analyze_cached(query);
        let lexical_query = if analyzed_query.normalized.is_empty() {
            analyzed_query.original.clone()
        } else {
            analyzed_query.normalized.clone()
        };
        let qvec = if let Some(cached) = self.cached_query_embedding(&analyzed_query.original) {
            Some(cached)
        } else if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            let vector = self
                .embed_texts(
                    &[analyzed_query.original.clone()],
                    crate::embeddings::EmbeddingInputKind::Query,
                )
                .await
                .ok()
                .and_then(|mut vectors| vectors.pop())
                .unwrap_or_default();
            if !vector.is_empty() {
                self.store_query_embedding(&analyzed_query.original, vector.clone());
            }
            (!vector.is_empty()).then_some(vector)
        } else {
            let vector = hashed_embedding(&analyzed_query.original);
            self.store_query_embedding(&analyzed_query.original, vector.clone());
            Some(vector)
        };
        let retrieved = match qvec {
            Some(vector) => {
                crate::rag::search_hybrid(
                    &self.rag_dir(),
                    vector,
                    &lexical_query,
                    k.max(12),
                    doc_ids,
                )
                .await
            }
            None => {
                log::warn!(
                    "[retrieval] query embedding unavailable; using FTS-only document retrieval"
                );
                crate::rag::search_fts_only(&self.rag_dir(), &lexical_query, k.max(12), doc_ids)
                    .await
            }
        }?;
        let rerank_eligible = matches!(
            analyzed_query.intent,
            crate::retrieval_pipeline::RetrievalIntent::Comparison
                | crate::retrieval_pipeline::RetrievalIntent::Synthesis
                | crate::retrieval_pipeline::RetrievalIntent::MultiHop
        );
        if !rerank_eligible || retrieved.len() < 4 {
            return Ok(retrieved.into_iter().take(k).collect());
        }
        let top = retrieved.iter().take(12).cloned().collect::<Vec<_>>();
        let texts = top
            .iter()
            .map(|chunk| format!("{}\n{}", chunk.source, chunk.text))
            .collect::<Vec<_>>();
        match self
            .rerank_documents_if_ready(&analyzed_query.original, &texts)
            .await
        {
            Ok(Some(scores)) => {
                let mut reranked = top;
                crate::retrieval_pipeline::blend_reranker_scores(&mut reranked, &scores);
                log::debug!(
                    "[retrieval] reranked {} candidates with 0.65/0.35 score blend",
                    reranked.len()
                );
                Ok(reranked.into_iter().take(k).collect())
            }
            Ok(None) => Ok(retrieved.into_iter().take(k).collect()),
            Err(error) => {
                log::warn!("[retrieval] reranker unavailable; returning hybrid ranking: {error:#}");
                Ok(retrieved.into_iter().take(k).collect())
            }
        }
    }
}
