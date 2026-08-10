use super::super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub(crate) async fn ensure_external_pipeline_ready(&self) -> Result<llama_server::ResolvedLlamaConfig> {
        let _pipeline_guard = self.inner.ai_pipeline_lock.lock().await;
        let config = llama_server::resolve_config(&self.inner.app_data_dir).unwrap_or_else(|_| {
            let settings = self.openharn_settings();
            llama_server::ResolvedLlamaConfig {
                inference_engine: "external".into(),
                executable_path: PathBuf::new(),
                runtime_fingerprint: "external-endpoint".into(),
                model_path: PathBuf::from(
                    settings
                        .external_model
                        .as_deref()
                        .unwrap_or("external-model"),
                ),
                host: "127.0.0.1".into(),
                port: 0,
                context_size: 24_096,
                gpu_layers: None,
                threads: None,
                temperature: 0.0,
                top_p: 0.95,
                chat_format: None,
                extra_args: Vec::new(),
                backend: None,
                backend_preference: "auto".into(),
                gpu_device: None,
                thinking: false,
                auto_offload: false,
                max_turns: 4,
                chat_template_override: None,
                model_role: "chat".into(),
                supports_tools: Some(true),
                prefers_prompt_tools: None,
                verbose_tool_schemas: false,
                tool_choice: None,
                template_kwargs: None,
                deterministic_tools: false,
                tool_gating: false,
                prompt_cache: false,
                slot_save_path: PathBuf::new(),
                candidates: Vec::new(),
            }
        });
        crate::sidecar::ensure_sidecar(self).await?;
        Ok(config)
    }

    /// Prepare every process/check that would otherwise delay the first model
    /// request. The lock makes startup and a quickly submitted chat share the
    /// same work instead of launching duplicate capability probes.
    pub(crate) async fn ensure_ai_pipeline_ready(&self) -> Result<llama_server::ResolvedLlamaConfig> {
        let _pipeline_guard = self.inner.ai_pipeline_lock.lock().await;
        if !self.ai_pipeline_ready() {
            let _ = self.handle.emit(
                "ai://llama_warmup",
                serde_json::json!({ "status": "started" }),
            );
        }

        let result: Result<llama_server::ResolvedLlamaConfig> = async {
            let config = llama_server::resolve_config(&self.inner.app_data_dir)?;
            self.ensure_llama_server(&config).await?;

            // Sidecar startup is independent of the one-time model capability
            // probe, so overlap them after llama-server becomes healthy.
            let model_id = config.model_path.to_string_lossy().to_string();
            let llama_base = config.base_url();
            let sidecar = crate::sidecar::ensure_sidecar(self);
            let capability = crate::tool_capability::supports_tools(
                &self.inner.llama_client,
                &llama_base,
                &self.inner.app_data_dir,
                &config.model_path,
                &model_id,
                config.supports_tools,
            );
            let (sidecar_result, _) = tokio::join!(sidecar, capability);
            sidecar_result?;
            Ok(config)
        }
        .await;

        match result {
            Ok(config) => {
                let announce_ready = !self.ai_pipeline_ready();
                self.inner
                    .ai_pipeline_ready
                    .store(true, std::sync::atomic::Ordering::Release);
                if announce_ready {
                    let _ = self.handle.emit(
                        "ai://llama_warmup",
                        serde_json::json!({ "status": "ready" }),
                    );
                }
                Ok(config)
            }
            Err(error) => {
                self.invalidate_ai_pipeline();
                let _ = self.handle.emit(
                    "ai://llama_warmup",
                    serde_json::json!({ "status": "failed", "message": error.to_string() }),
                );
                Err(error)
            }
        }
    }

    pub(crate) async fn cancel_prompt_warmup(&self) {
        let handle = self
            .inner
            .prompt_warmup
            .lock()
            .take()
            .map(|(_, handle)| handle);
        if let Some(handle) = handle {
            if !handle.is_finished() {
                handle.abort();
                let _ = handle.await;
                log::debug!("preempted note prompt-cache warm-up for user chat");
            }
        }
    }

    /// Wait for a note-prefix warm-up that was already started on note open.
    /// Retrieval runs concurrently with it; a short wait makes a nearly
    /// finished warm-up useful without adding a long cold-start delay.
    pub(crate) async fn finish_prompt_warmup(&self) {
        let handle = self
            .inner
            .prompt_warmup
            .lock()
            .take()
            .map(|(_, handle)| handle);
        let Some(mut handle) = handle else { return };
        if handle.is_finished() {
            let _ = handle.await;
            return;
        }
        if tokio::time::timeout(std::time::Duration::from_secs(1), &mut handle)
            .await
            .is_err()
        {
            handle.abort();
            let _ = handle.await;
            log::debug!("prompt-cache warm-up exceeded 1 second; proceeding with compact prompt");
        }
    }

}
