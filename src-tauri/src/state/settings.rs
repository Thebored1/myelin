use super::core::*;
use ::anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RerankerModelStatus {
    pub configured_path: Option<String>,
    pub state: String,
    pub model_name: Option<String>,
    pub context_tokens: Option<usize>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltInModelInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub size_bytes: u64,
    pub installed_path: Option<String>,
}

struct BuiltInModel {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    filename: &'static str,
    url: &'static str,
    sha256: &'static str,
    size: u64,
}
const BUILT_INS: &[BuiltInModel] = &[
    BuiltInModel { id: "nomic-embed-text-v1.5-q4-k-m", name: "Nomic Embed Text v1.5 (Q4_K_M)", kind: "embedding", filename: "nomic-embed-text-v1.5.Q4_K_M.gguf", url: "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q4_K_M.gguf", sha256: "d4e388894e09cf3816e8b0896d81d265b55e7a9fff9ab03fe8bf4ef5e11295ac", size: 84_106_624 },
    BuiltInModel { id: "ms-marco-minilm-l6-v2-q4-k-m", name: "MS MARCO MiniLM-L6-v2 (Q4_K_M)", kind: "reranker", filename: "ms-marco-MiniLM-L6-v2-Q4_K_M.gguf", url: "https://huggingface.co/sinjab/ms-marco-MiniLM-L6-v2-Q4_K_M-GGUF/resolve/main/ms-marco-MiniLM-L6-v2-Q4_K_M.gguf", sha256: "d814d09aa417373ec06320687f3ab2741a1fb6d71f28082f88b3574a7bd0fd95", size: 21_272_256 },
];

impl AppState {
    pub fn ocr_settings(&self) -> crate::ocr::OcrSettings {
        crate::state::context::load_settings(&self.inner.app_data_dir)
            .map(|settings| settings.ocr)
            .unwrap_or_default()
    }

    pub fn ocr_status(&self) -> crate::ocr::OcrStatus {
        crate::ocr::status(&self.ocr_settings())
    }

    pub fn set_ocr_settings(
        &self,
        settings: crate::ocr::OcrSettings,
    ) -> Result<crate::ocr::OcrStatus> {
        settings.validate().map_err(anyhow::Error::msg)?;
        let _lock = self.inner.persistence_lock.lock();
        let mut persisted = crate::state::context::load_settings(&self.inner.app_data_dir)?;
        persisted.ocr = settings;
        crate::state::context::save_settings(&self.inner.app_data_dir, &persisted)?;
        Ok(crate::ocr::status(&persisted.ocr))
    }

    pub fn built_in_models(&self) -> Vec<BuiltInModelInfo> {
        let root = self.inner.app_data_dir.join("models");
        BUILT_INS
            .iter()
            .map(|model| {
                let path = root.join(model.filename);
                BuiltInModelInfo {
                    id: model.id.into(),
                    name: model.name.into(),
                    kind: model.kind.into(),
                    size_bytes: model.size,
                    installed_path: path.is_file().then(|| path.display().to_string()),
                }
            })
            .collect()
    }

    pub async fn download_built_in_model(&self, id: &str) -> Result<String> {
        use futures_util::StreamExt;
        use sha2::{Digest, Sha256};
        let model = BUILT_INS
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| anyhow!("unknown built-in model '{id}'"))?;
        let root = self.inner.app_data_dir.join("models");
        fs::create_dir_all(&root)?;
        let target = root.join(model.filename);
        let temporary = root.join(format!("{}.part", model.filename));
        let result: Result<()> = async {
            let response = self
                .inner
                .llama_client
                .get(model.url)
                .send()
                .await?
                .error_for_status()?;
            if response
                .content_length()
                .is_some_and(|size| size != model.size)
            {
                anyhow::bail!("model download size does not match the built-in manifest");
            }
            let mut file = fs::File::create(&temporary)?;
            let mut stream = response.bytes_stream();
            let mut hash = Sha256::new();
            let mut received = 0_u64;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                received += chunk.len() as u64;
                if received > model.size {
                    anyhow::bail!("model download exceeds the built-in manifest size");
                }
                std::io::Write::write_all(&mut file, &chunk)?;
                hash.update(&chunk);
            }
            if received != model.size {
                anyhow::bail!(
                    "model download is incomplete ({received} of {} bytes)",
                    model.size
                );
            }
            let actual = format!("{:x}", hash.finalize());
            if actual != model.sha256 {
                anyhow::bail!("model download checksum mismatch");
            }
            fs::rename(&temporary, &target)?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        let path = target.display().to_string();
        match model.kind {
            "embedding" => {
                self.set_embed_model_path(Some(path.clone())).await?;
            }
            "reranker" => {
                self.set_reranker_model_path(Some(path.clone())).await?;
            }
            _ => unreachable!(),
        }
        Ok(path)
    }
    pub fn ai_config_status(&self) -> crate::ai_config::AiConfigStatus {
        crate::ai_config::status(&self.inner.app_data_dir)
    }


    pub fn set_deterministic_tools_runtime(&self, enabled: bool) {
        self.inner.ai
            .deterministic_tools
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_tool_gating_runtime(&self, enabled: bool) {
        self.inner.ai
            .tool_gating
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    pub async fn set_llama_model_path(&self, model_path: String) -> Result<()> {
        self.invalidate_ai_pipeline();
        crate::llama_server::set_model_path(&self.inner.app_data_dir, model_path)?;
        Ok(())
    }

    pub async fn set_llama_executable_path(&self, executable_path: String) -> Result<()> {
        self.invalidate_ai_pipeline();
        crate::llama_server::set_executable_path(&self.inner.app_data_dir, executable_path)?;
        Ok(())
    }

    pub async fn set_deterministic_tools(&self, enabled: bool) -> Result<()> {
        crate::llama_server::set_deterministic_tools(&self.inner.app_data_dir, enabled)?;
        self.set_deterministic_tools_runtime(enabled);
        Ok(())
    }

    pub async fn set_tool_gating(&self, enabled: bool) -> Result<()> {
        crate::llama_server::set_tool_gating(&self.inner.app_data_dir, enabled)?;
        self.set_tool_gating_runtime(enabled);
        Ok(())
    }

    pub async fn set_prompt_cache(&self, enabled: bool) -> Result<()> {
        crate::llama_server::set_prompt_cache(&self.inner.app_data_dir, enabled)?;
        self.stop_llama_server().await;
        Ok(())
    }

    pub async fn set_inference_engine(&self, engine: String) -> Result<()> {
        crate::llama_server::set_inference_engine(&self.inner.app_data_dir, engine)?;
        self.stop_llama_server().await;
        Ok(())
    }

    pub fn llama_cache_status(&self) -> LlamaCacheStatus {
        LlamaCacheStatus {
            enabled: crate::llama_server::resolve_config(&self.inner.app_data_dir)
                .map(|c| c.prompt_cache)
                .unwrap_or(true),
            size_bytes: dir_size(&self.inner.app_data_dir.join("llama-cache")),
        }
    }

    pub async fn clear_llama_cache(&self) -> Result<()> {
        self.stop_llama_server().await;
        let path = self.inner.app_data_dir.join("llama-cache");
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        Ok(())
    }

    pub async fn set_llama_advanced_config(
        &self,
        context_size: Option<u32>,
        gpu_layers: Option<i32>,
        threads: Option<u32>,
        temperature: Option<f32>,
        top_p: Option<f32>,
        extra_args: Option<Vec<String>>,
        backend_preference: Option<String>,
        gpu_device: Option<String>,
        thinking: Option<bool>,
        auto_offload: Option<bool>,
        max_turns: Option<u32>,
    ) -> Result<()> {
        self.invalidate_ai_pipeline();
        crate::llama_server::set_advanced_config(
            &self.inner.app_data_dir,
            context_size,
            gpu_layers,
            threads,
            temperature,
            top_p,
            extra_args,
            backend_preference,
            gpu_device,
            thinking,
            auto_offload,
            max_turns,
        )?;
        Ok(())
    }

    pub fn list_llama_devices(&self, backend: String) -> Vec<crate::llama_server::DeviceInfo> {
        crate::llama_server::list_devices(&self.inner.app_data_dir, &backend)
    }

    pub fn downloadable_backends(&self) -> Vec<String> {
        crate::llama_server::downloadable_backends()
    }

    pub fn downloadable_bee_backends(&self) -> Vec<String> {
        crate::llama_server::downloadable_bee_backends()
    }

    pub(crate) fn emit_download(&self, backend: &str, phase: &str, percent: f64, message: &str) {
        let _ = self.handle.emit(
            "backend://download",
            serde_json::json!({
                "backend": backend,
                "phase": phase,
                "percent": percent,
                "message": message,
            }),
        );
    }

    /// Download, extract and install a llama.cpp backend build into the
    /// app-data bin dir, emitting `backend://download` progress events.
    pub async fn download_llama_backend(&self, backend: String) -> Result<()> {
        self.download_backend(backend, false).await
    }

    pub async fn download_bee_backend(&self, backend: String) -> Result<()> {
        self.download_backend(backend, true).await
    }

    pub(crate) async fn download_backend(&self, backend: String, bee: bool) -> Result<()> {
        use futures_util::StreamExt;
        use sha2::{Digest, Sha256};

        let assets = if bee {
            crate::llama_server::bee_assets_for_backend(&backend)
        } else {
            crate::llama_server::assets_for_backend(&backend)
        };
        if assets.is_empty() {
            anyhow::bail!("No downloadable {backend} build is available for this platform.");
        }

        let bin_root = if bee {
            self.inner.app_data_dir.join("bin").join("bee")
        } else {
            self.inner.app_data_dir.join("bin")
        };
        let backend_dir = bin_root.join(&backend);
        let staging = bin_root.join(format!(".staging-{backend}"));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;

        // Backend archives are hundreds of MB and can take minutes. The shared
        // `llama_client` has a 120s TOTAL timeout tuned for chat/health requests,
        // which aborts a large download mid-body — surfacing as the misleading
        // "error decoding response body". Use a dedicated client with a per-read
        // idle timeout (catches stalled/dead connections) but NO overall cap, so a
        // slow-but-progressing download isn't killed.
        let download_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(120))
            .user_agent("Myelin")
            .build()
            .unwrap_or_else(|_| self.inner.llama_client.clone());

        let result: Result<()> = async {
            let total_assets = assets.len() as f64;
            for (i, asset) in assets.iter().enumerate() {
                let url = if bee {
                    crate::llama_server::bee_download_url(asset)
                } else {
                    crate::llama_server::download_url(asset)
                };
                self.emit_download(&backend, "downloading", (i as f64 / total_assets) * 100.0,
                    &format!("Downloading {} ({}/{})", asset, i + 1, assets.len()));

                let resp = download_client.get(&url).send().await
                    .with_context(|| format!("failed to download {asset}"))?;
                if !resp.status().is_success() {
                    anyhow::bail!("download failed for {asset}: HTTP {}", resp.status());
                }
                let total = resp.content_length().unwrap_or(0);
                let archive_path = staging.join(asset);
                let mut file = fs::File::create(&archive_path)?;
                let mut downloaded: u64 = 0;
                let mut last_pct: i32 = -1;
                let mut digest = Sha256::new();
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.with_context(|| {
                        format!("download stream interrupted for {asset} (network stalled or connection dropped)")
                    })?;
                    std::io::Write::write_all(&mut file, &chunk)?;
                    digest.update(&chunk);
                    downloaded += chunk.len() as u64;
                    if total > 0 {
                        let frac = downloaded as f64 / total as f64;
                        let overall = ((i as f64 + frac) / total_assets) * 100.0;
                        let pct = overall as i32;
                        if pct != last_pct {
                            last_pct = pct;
                            self.emit_download(&backend, "downloading", overall,
                                &format!("Downloading {} ({}/{})", asset, i + 1, assets.len()));
                        }
                    }
                }
                drop(file);
                if bee {
                    let expected = crate::llama_server::bee_asset_sha256(asset)
                        .ok_or_else(|| anyhow!("No pinned checksum for {asset}"))?;
                    let actual = format!("{:x}", digest.finalize());
                    if actual != expected {
                        anyhow::bail!(
                            "Checksum mismatch for {asset}: expected {expected}, got {actual}"
                        );
                    }
                }

                self.emit_download(&backend, "extracting", 100.0, &format!("Extracting {asset}"));
                crate::llama_server::extract_archive(&archive_path, &staging)?;
                let _ = fs::remove_file(&archive_path);
            }

            self.emit_download(&backend, "installing", 100.0, "Installing");
            let _ = fs::remove_dir_all(&backend_dir);
            crate::llama_server::install_backend_from_staging(&staging, &backend_dir)?;
            Ok(())
        }
        .await;

        let _ = fs::remove_dir_all(&staging);
        match result {
            Ok(()) => {
                self.emit_download(
                    &backend,
                    "done",
                    100.0,
                    &format!(
                        "{} {backend} backend installed",
                        if bee { "BeeLlama" } else { "llama.cpp" }
                    ),
                );
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&backend_dir);
                self.emit_download(&backend, "error", 0.0, &error.to_string());
                Err(error)
            }
        }
    }

    pub async fn provider_status(&self) -> Result<ProviderStatus> {
        let info = llama_server::inspect_provider(&self.inner.app_data_dir)?;
        // Prefer the backend of the running server; fall back to the backend we
        // would select on this machine.
        let mut active_backend = info.selected_backend.clone();
        let mut active_engine = info
            .resolved
            .as_ref()
            .map(|config| config.inference_engine.clone());
        let configured_engine =
            crate::llama_server::normalize_engine(info.config.inference_engine.as_deref());
        let healthy = if let Some(config) = &info.resolved {
            let server = self.inner.ai.llama_server.lock().await;
            if let Some(server) = server.as_ref() {
                active_backend = Some(server.active_backend.label().to_string());
                active_engine = Some(server.active_engine.clone());
                if config.accepts_running(&server.config) {
                    llama_server::health_check(&self.inner.llama_client, &server.config).await
                } else {
                    info.healthy
                }
            } else {
                info.healthy
            }
        } else {
            false
        };

        Ok(ProviderStatus {
            active_provider: if active_engine.as_deref() == Some("beellama") {
                "BeeLlama".into()
            } else {
                "llama.cpp".into()
            },
            available_providers: vec!["llama.cpp".into(), "BeeLlama".into()],
            healthy,
            ready: healthy && self.ai_pipeline_ready(),
            detail: info.detail,
            config: Some(info.config),
            resolved: info.resolved,
            active_backend,
            configured_engine,
            active_engine,
            nvidia_detected: info.nvidia_detected,
            gpu_available: info.gpu_available,
            gpus: info.gpus,
            installed_backends: info.installed_backends,
            installed_bee_backends: info.installed_bee_backends,
            recommended_threads: llama_server::default_inference_threads(),
        })
    }

    pub fn background_settings(&self) -> BackgroundSettings {
        self.inner.background_settings.lock().clone()
    }

    pub fn set_background_settings(&self, settings: BackgroundSettings) -> Result<()> {
        let mut persisted = load_settings(&self.inner.app_data_dir)?;
        persisted.background = settings.clone();
        save_settings(&self.inner.app_data_dir, &persisted)?;
        *self.inner.background_settings.lock() = settings;
        Ok(())
    }

    pub fn searxng_url(&self) -> Option<String> {
        crate::llama_server::searxng_url(&self.inner.app_data_dir)
    }

    /// Set (or clear, when empty) the SearXNG base URL for web search.
    pub fn set_searxng_url(&self, url: Option<String>) -> Result<()> {
        crate::llama_server::set_searxng_url(&self.inner.app_data_dir, url)
    }

    /// The configured embedding model GGUF path (None → embeddings disabled).
    pub fn quick_shortcut(&self) -> String {
        crate::llama_server::quick_capture_shortcut(&self.inner.app_data_dir)
    }

    pub fn set_quick_shortcut(&self, shortcut: String) -> Result<()> {
        crate::llama_server::set_quick_capture_shortcut(&self.inner.app_data_dir, shortcut)
    }

    pub fn embed_model_path(&self) -> Option<String> {
        crate::llama_server::embed_model_path(&self.inner.app_data_dir)
    }

    /// All known model profiles (bundled + user) for the compatibility list.
    pub fn list_model_profiles(&self) -> Vec<crate::model_profiles::ModelProfile> {
        crate::model_profiles::all_profiles(&self.inner.app_data_dir)
    }

    /// Validate a candidate before changing persistent configuration. Derived
    /// vector data is reset only after the new contract is proven live.
    pub async fn set_embed_model_path(
        &self,
        path: Option<String>,
    ) -> Result<crate::embeddings::EmbeddingModelContract> {
        match path.filter(|path| !path.trim().is_empty()) {
            Some(path) => {
                let contract = self.validate_embedding_candidate(&path).await?;
                crate::llama_server::set_embed_model_path(
                    &self.inner.app_data_dir,
                    Some(contract.model_path.display().to_string()),
                )?;
                self.invalidate_embedding_derived_data().await?;
                Ok(contract)
            }
            None => {
                crate::llama_server::set_embed_model_path(&self.inner.app_data_dir, None)?;
                self.invalidate_embedding_derived_data().await?;
                Ok(self.hashed_embedding_contract())
            }
        }
    }

    pub async fn get_reranker_model_status(&self) -> RerankerModelStatus {
        let configured_path = crate::llama_server::reranker_model_path(&self.inner.app_data_dir);
        let server = self.inner.ai.reranker_server.lock().await;
        let ready = server.is_some();
        let context_tokens = server.as_ref().map(|server| server.context_tokens);
        let model_name = configured_path.as_ref().and_then(|path| {
            PathBuf::from(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        });
        RerankerModelStatus {
            configured_path,
            state: if ready {
                "ready".into()
            } else if model_name.is_some() {
                "error".into()
            } else {
                "none".into()
            },
            model_name,
            context_tokens,
            last_error: None,
        }
    }

    pub async fn set_reranker_model_path(
        &self,
        path: Option<String>,
    ) -> Result<RerankerModelStatus> {
        match path.filter(|path| !path.trim().is_empty()) {
            Some(path) => {
                let (model_path, _) = self.validate_reranker_candidate(&path).await?;
                crate::llama_server::set_reranker_model_path(
                    &self.inner.app_data_dir,
                    Some(model_path.display().to_string()),
                )?;
            }
            None => crate::llama_server::set_reranker_model_path(&self.inner.app_data_dir, None)?,
        }
        if let Some(mut server) = self.inner.ai.reranker_server.lock().await.take() {
            crate::llama_server::stop_reranker_server(&mut server).await;
        }
        *self.inner.ai.reranker_circuit.lock() = crate::state::types::RerankerCircuit::default();
        if crate::llama_server::reranker_model_path(&self.inner.app_data_dir).is_some() {
            let state = self.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = state.ensure_reranker_server().await {
                    log::warn!("reranker did not become resident: {error:#}");
                }
            });
        }
        Ok(self.get_reranker_model_status().await)
    }
}
