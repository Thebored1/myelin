use super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub fn ai_config_status(&self) -> crate::ai_config::AiConfigStatus {
        crate::ai_config::status(&self.inner.app_data_dir)
    }

    pub async fn install_ai_runtime(&self, runtime_id: &str) -> anyhow::Result<()> {
        use futures_util::StreamExt;
        use sha2::{Digest, Sha256};
        let config = crate::ai_config::load(&self.inner.app_data_dir)?;
        let runtime = config.runtimes.get(runtime_id).ok_or_else(|| anyhow::anyhow!("runtime '{runtime_id}' is not configured"))?.clone();
        let crate::ai_config::RuntimeSource::Download { url, sha256, archive_format, binary_path } = runtime.source else {
            anyhow::bail!("runtime '{runtime_id}' is not a downloadable runtime")
        };
        if !url.starts_with("https://") { anyhow::bail!("runtime download URL must use HTTPS") }
        let root = self.inner.app_data_dir.join("bin").join("runtimes").join(runtime_id);
        let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&staging)?;
        let result: anyhow::Result<()> = async {
            let response = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(30)).read_timeout(std::time::Duration::from_secs(120)).user_agent("Myelin").build()?.get(&url).send().await?.error_for_status()?;
            if response.content_length().is_some_and(|n| n > 4 * 1024 * 1024 * 1024) { anyhow::bail!("runtime archive exceeds 4 GiB") }
            let archive = staging.join("download");
            let mut file = fs::File::create(&archive)?;
            let mut hash = Sha256::new(); let mut bytes = 0u64; let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await { let chunk = chunk?; bytes += chunk.len() as u64; if bytes > 4 * 1024 * 1024 * 1024 { anyhow::bail!("runtime archive exceeds 4 GiB") } hash.update(&chunk); std::io::Write::write_all(&mut file, &chunk)?; }
            drop(file);
            let actual = format!("{:x}", hash.finalize()); if !actual.eq_ignore_ascii_case(&sha256) { anyhow::bail!("runtime checksum mismatch: expected {sha256}, got {actual}") }
            let payload = staging.join("payload"); fs::create_dir_all(&payload)?;
            match archive_format {
                crate::ai_config::RuntimeArchiveFormat::Raw => { fs::copy(&archive, payload.join(binary_path.as_ref().and_then(|p| p.file_name()).and_then(|p| p.to_str()).unwrap_or("llama-server")))?; }
                crate::ai_config::RuntimeArchiveFormat::Zip => { let status = std::process::Command::new("unzip").args(["-q", "-:", archive.to_str().unwrap_or_default(), "-d", payload.to_str().unwrap_or_default()]).status()?; if !status.success() { anyhow::bail!("unzip failed") } }
                crate::ai_config::RuntimeArchiveFormat::TarGz => { let status = std::process::Command::new("tar").args(["--no-absolute-names", "--warning=no-unknown-keyword", "-xzf", archive.to_str().unwrap_or_default(), "-C", payload.to_str().unwrap_or_default()]).status()?; if !status.success() { anyhow::bail!("tar extraction failed") } }
            }
            let executable = if let Some(relative) = binary_path { payload.join(relative) } else { payload.join("llama-server") };
            if !executable.is_file() { anyhow::bail!("configured runtime binary was not found at {}", executable.display()) }
            let installed = root.join(&actual[..16]); fs::create_dir_all(&root)?; if installed.exists() { fs::remove_dir_all(&installed)?; }
            fs::rename(&payload, &installed)?;
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let mode = fs::metadata(&installed)?.permissions().mode(); for entry in walkdir::WalkDir::new(&installed).into_iter().flatten() { if entry.file_type().is_file() { let current = fs::metadata(entry.path())?.permissions().mode(); fs::set_permissions(entry.path(), fs::Permissions::from_mode(if entry.path() == executable { 0o755 } else { current | (mode & 0o111) }))?; } } }
            Ok(())
        }.await;
        let _ = fs::remove_dir_all(&staging);
        result
    }

    pub fn ensure_ai_config(&self) -> anyhow::Result<()> {
        crate::ai_config::ensure_file(&self.inner.app_data_dir)?;
        crate::ai_config::ensure_schema(&self.inner.app_data_dir)?;
        Ok(())
    }

    pub async fn validate_ai_config(&self) -> anyhow::Result<crate::ai_config::AiConfigStatus> {
        let _chat_guard = self.inner.chat_lock.try_lock().map_err(|_| anyhow::anyhow!("cannot validate while a model turn is active"))?;
        let _slot_guard = self.inner.llama_slot_lock.try_lock().map_err(|_| anyhow::anyhow!("cannot validate while a cache operation is active"))?;
        self.ensure_ai_config()?;
        let config = crate::ai_config::load(&self.inner.app_data_dir)?;
        crate::ai_config::require_valid(&config)?;
        let profile = config.profiles.get(&config.active_profile).ok_or_else(|| anyhow::anyhow!("active profile is missing"))?;
        if let Some(runtime) = config.runtimes.get(&profile.runtime) {
            match &runtime.source {
                crate::ai_config::RuntimeSource::Path { executable } if !executable.is_file() => anyhow::bail!("runtime executable does not exist: {}", executable.display()),
                crate::ai_config::RuntimeSource::Download { binary_path, .. } => {
                    let root = self.inner.app_data_dir.join("bin").join("runtimes").join(&profile.runtime);
                    let found = fs::read_dir(&root).ok().into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).any(|dir| binary_path.as_ref().map(|p| dir.join(p).is_file()).unwrap_or_else(|| dir.join("llama-server").is_file()));
                    if !found { anyhow::bail!("downloaded runtime '{}' is not installed", profile.runtime) }
                }
                _ => {}
            }
        }
        let resolved = crate::llama_server::resolve_config(&self.inner.app_data_dir)?;
        if !crate::llama_server::health_check(&self.inner.llama_client, &resolved).await {
            anyhow::bail!("configured runtime is not healthy at {}", resolved.base_url())
        }
        let request = serde_json::json!({
            "model": resolved.model_name(),
            "messages": [{"role": "user", "content": "Reply with one token."}],
            "max_tokens": 1,
            "stream": false,
            "cache_prompt": true,
            "id_slot": 0
        });
        let response = self.inner.llama_client.post(format!("{}/v1/chat/completions", resolved.base_url())).json(&request).send().await?.error_for_status()?;
        let body: serde_json::Value = response.json().await?;
        if body.get("error").is_some() { anyhow::bail!("runtime rejected cached inference probe: {body}") }
        // The slot endpoints require a filename in the JSON body. Use a
        // unique probe name so validation cannot collide with a real section
        // cache, then remove the temporary file whether the probe succeeds or
        // fails.
        let validation_filename = format!("myelin-validation-{}.slot", uuid::Uuid::new_v4());
        let validation_path = resolved.slot_save_path.join(&validation_filename);
        let probe_result: anyhow::Result<(u64, u64)> = async {
            let save = self
                .inner
                .llama_client
                .post(format!("{}/slots/0?action=save", resolved.base_url()))
                .json(&serde_json::json!({"filename": validation_filename}))
                .send()
                .await?
                .error_for_status()?;
            let save_body: serde_json::Value = save.json().await?;
            let saved = save_body.get("n_saved").and_then(|v| v.as_u64()).unwrap_or(0);
            if saved == 0 { anyhow::bail!("runtime slot save probe returned no saved tokens: {save_body}") }

            let restore = self
                .inner
                .llama_client
                .post(format!("{}/slots/0?action=restore", resolved.base_url()))
                .json(&serde_json::json!({"filename": validation_filename}))
                .send()
                .await?
                .error_for_status()?;
            let restore_body: serde_json::Value = restore.json().await?;
            let restored = restore_body.get("n_restored").and_then(|v| v.as_u64()).unwrap_or(0);
            if restored != saved { anyhow::bail!("runtime slot restore probe mismatch: saved {saved}, restored {restored}") }
            Ok((saved, restored))
        }.await;
        let _ = fs::remove_file(&validation_path);
        let (saved, restored) = probe_result?;
        log::info!("runtime validation passed: slot save/restore saved={saved} restored={restored}");
        Ok(self.ai_config_status())
    }

    pub async fn apply_ai_config(&self, candidate_hash: &str) -> anyhow::Result<crate::ai_config::AiConfigStatus> {
        if self.inner.chat_lock.try_lock().is_err() || self.inner.llama_slot_lock.try_lock().is_err() {
            anyhow::bail!("AI configuration cannot be applied while a model turn or cache operation is active")
        }
        self.ensure_ai_config()?;
        let config = crate::ai_config::load(&self.inner.app_data_dir)?;
        crate::ai_config::require_valid(&config)?;
        let actual = crate::ai_config::canonical_hash(&config)?;
        if actual != candidate_hash { anyhow::bail!("AI configuration changed after validation; validate it again") }
        crate::ai_config::write_atomic(&crate::ai_config::applied_path(&self.inner.app_data_dir), &config)?;
        // Apply changes to the in-memory sidecar settings as part of the same
        // lifecycle transition. Without this projection, the next request
        // continued using the legacy settings.json values until process exit,
        // which made a correctly applied native TQ2 profile still log as
        // prompt-mode.
        let projected = project_ai_agent_settings(&self.openharn_settings(), &config.agent);
        *self.inner.openharn_settings.lock() = projected;
        self.invalidate_ai_pipeline();
        if let Ok(mut guard) = self.inner.sidecar.try_lock() {
            *guard = None;
        }
        *self.inner.active_slot_cache.lock() = None;
        Ok(self.ai_config_status())
    }

    pub fn set_deterministic_tools_runtime(&self, enabled: bool) {
        self.inner
            .deterministic_tools
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_tool_gating_runtime(&self, enabled: bool) {
        self.inner
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
            let server = self.inner.llama_server.lock().await;
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

    pub fn background_settings(&self) -> BackgroundSettings { self.inner.background_settings.lock().clone() }

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

    /// Set (or clear, when empty) the embedding model GGUF path.
    pub fn set_embed_model_path(&self, path: Option<String>) -> Result<()> {
        crate::llama_server::set_embed_model_path(&self.inner.app_data_dir, path)
    }
}
