// Reranker model-server lifecycle and document reranking.
// Split out of retrieval.rs to keep each model-server lifecycle readable.

use super::core::*;
use ::anyhow::{Context, Result};

impl AppState {
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

    pub(crate) async fn rerank_documents_if_ready(
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
}
