// Embedding model-server lifecycle and embedding contracts.
// Split out of retrieval.rs to keep each model-server lifecycle readable.

use super::core::*;
use ::anyhow::{Context, Result};

impl AppState {
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
}
