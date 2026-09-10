// AI runtime installation and config validation for the settings surface.
// Split out of settings.rs: download/staging/validate has its own I/O-heavy
// lifecycle distinct from the plain settings accessors.

use super::core::*;

impl AppState {
    pub async fn install_ai_runtime(&self, runtime_id: &str) -> anyhow::Result<()> {
        use futures_util::StreamExt;
        use sha2::{Digest, Sha256};
        let config = crate::ai_config::load(&self.inner.app_data_dir)?;
        crate::ai_config::require_valid(&config)?;
        let runtime = config
            .runtimes
            .get(runtime_id)
            .ok_or_else(|| anyhow::anyhow!("runtime '{runtime_id}' is not configured"))?
            .clone();
        let crate::ai_config::RuntimeSource::Download {
            url,
            sha256,
            archive_format,
            binary_path,
        } = runtime.source
        else {
            anyhow::bail!("runtime '{runtime_id}' is not a downloadable runtime")
        };
        if !url.starts_with("https://") {
            anyhow::bail!("runtime download URL must use HTTPS")
        }
        let root = self
            .inner
            .app_data_dir
            .join("bin")
            .join("runtimes")
            .join(runtime_id);
        let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&staging)?;
        let result: anyhow::Result<()> = async {
            let response = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .read_timeout(std::time::Duration::from_secs(120))
                .user_agent("Myelin")
                .build()?
                .get(&url)
                .send()
                .await?
                .error_for_status()?;
            if response
                .content_length()
                .is_some_and(|n| n > 4 * 1024 * 1024 * 1024)
            {
                anyhow::bail!("runtime archive exceeds 4 GiB")
            }
            let archive = staging.join("download");
            let mut file = fs::File::create(&archive)?;
            let mut hash = Sha256::new();
            let mut bytes = 0u64;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                bytes += chunk.len() as u64;
                if bytes > 4 * 1024 * 1024 * 1024 {
                    anyhow::bail!("runtime archive exceeds 4 GiB")
                }
                hash.update(&chunk);
                std::io::Write::write_all(&mut file, &chunk)?;
            }
            drop(file);
            let actual = format!("{:x}", hash.finalize());
            if !actual.eq_ignore_ascii_case(&sha256) {
                anyhow::bail!("runtime checksum mismatch: expected {sha256}, got {actual}")
            }
            let payload = staging.join("payload");
            fs::create_dir_all(&payload)?;
            match archive_format {
                crate::ai_config::RuntimeArchiveFormat::Raw => {
                    fs::copy(
                        &archive,
                        payload.join(
                            binary_path
                                .as_ref()
                                .and_then(|p| p.file_name())
                                .and_then(|p| p.to_str())
                                .unwrap_or("llama-server"),
                        ),
                    )?;
                }
                crate::ai_config::RuntimeArchiveFormat::Zip => {
                    let status = std::process::Command::new("unzip")
                        .args([
                            "-q",
                            archive.to_str().unwrap_or_default(),
                            "-d",
                            payload.to_str().unwrap_or_default(),
                        ])
                        .status()?;
                    if !status.success() {
                        anyhow::bail!("unzip failed")
                    }
                }
                crate::ai_config::RuntimeArchiveFormat::TarGz => {
                    let status = std::process::Command::new("tar")
                        .args([
                            "--no-absolute-names",
                            "--warning=no-unknown-keyword",
                            "-xzf",
                            archive.to_str().unwrap_or_default(),
                            "-C",
                            payload.to_str().unwrap_or_default(),
                        ])
                        .status()?;
                    if !status.success() {
                        anyhow::bail!("tar extraction failed")
                    }
                }
            }
            let executable = if let Some(relative) = binary_path {
                payload.join(relative)
            } else {
                payload.join("llama-server")
            };
            let payload_root = fs::canonicalize(&payload)?;
            let executable_root = fs::canonicalize(&executable)
                .with_context(|| format!("runtime binary path is invalid: {}", executable.display()))?;
            if !executable_root.starts_with(&payload_root) {
                anyhow::bail!("runtime binary path escapes the extracted payload")
            }
            if !executable.is_file() {
                anyhow::bail!(
                    "configured runtime binary was not found at {}",
                    executable.display()
                )
            }
            let installed = root.join(&actual[..16]);
            fs::create_dir_all(&root)?;
            if installed.exists() {
                fs::remove_dir_all(&installed)?;
            }
            fs::rename(&payload, &installed)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&installed)?.permissions().mode();
                for entry in walkdir::WalkDir::new(&installed).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        let current = fs::metadata(entry.path())?.permissions().mode();
                        fs::set_permissions(
                            entry.path(),
                            fs::Permissions::from_mode(if entry.path() == executable {
                                0o755
                            } else {
                                current | (mode & 0o111)
                            }),
                        )?;
                    }
                }
            }
            Ok(())
        }
        .await;
        let _ = fs::remove_dir_all(&staging);
        result
    }

    pub fn ensure_ai_config(&self) -> anyhow::Result<()> {
        crate::ai_config::ensure_file(&self.inner.app_data_dir)?;
        crate::ai_config::ensure_schema(&self.inner.app_data_dir)?;
        Ok(())
    }

    pub async fn validate_ai_config(&self) -> anyhow::Result<crate::ai_config::AiConfigStatus> {
        let _chat_guard = self
            .inner.ai
            .chat_lock
            .try_lock()
            .map_err(|_| anyhow::anyhow!("cannot validate while a model turn is active"))?;
        let _slot_guard =
            self.inner.ai.llama_slot_lock.try_lock().map_err(|_| {
                anyhow::anyhow!("cannot validate while a cache operation is active")
            })?;
        self.ensure_ai_config()?;
        let config = crate::ai_config::load(&self.inner.app_data_dir)?;
        crate::ai_config::require_valid(&config)?;
        let profile = config
            .profiles
            .get(&config.active_profile)
            .ok_or_else(|| anyhow::anyhow!("active profile is missing"))?;
        if let Some(runtime) = config.runtimes.get(&profile.runtime) {
            match &runtime.source {
                crate::ai_config::RuntimeSource::Path { executable } if !executable.is_file() => {
                    anyhow::bail!(
                        "runtime executable does not exist: {}",
                        executable.display()
                    )
                }
                crate::ai_config::RuntimeSource::Download { binary_path, .. } => {
                    let root = self
                        .inner
                        .app_data_dir
                        .join("bin")
                        .join("runtimes")
                        .join(&profile.runtime);
                    let found = fs::read_dir(&root)
                        .ok()
                        .into_iter()
                        .flatten()
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .any(|dir| {
                            binary_path
                                .as_ref()
                                .map(|p| dir.join(p).is_file())
                                .unwrap_or_else(|| dir.join("llama-server").is_file())
                        });
                    if !found {
                        anyhow::bail!("downloaded runtime '{}' is not installed", profile.runtime)
                    }
                }
                _ => {}
            }
        }
        let resolved = crate::llama_server::resolve_config(&self.inner.app_data_dir)?;
        if !crate::llama_server::health_check(&self.inner.llama_client, &resolved).await {
            anyhow::bail!(
                "configured runtime is not healthy at {}",
                resolved.base_url()
            )
        }
        let request = serde_json::json!({
            "model": resolved.model_name(),
            "messages": [{"role": "user", "content": "Reply with one token."}],
            "max_tokens": 1,
            "stream": false,
            "cache_prompt": true,
            "id_slot": 0
        });
        let response = self
            .inner
            .llama_client
            .post(format!("{}/v1/chat/completions", resolved.base_url()))
            .json(&request)
            .send()
            .await?
            .error_for_status()?;
        let body: serde_json::Value = response.json().await?;
        if body.get("error").is_some() {
            anyhow::bail!("runtime rejected cached inference probe: {body}")
        }
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
            let saved = save_body
                .get("n_saved")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if saved == 0 {
                anyhow::bail!("runtime slot save probe returned no saved tokens: {save_body}")
            }

            let restore = self
                .inner
                .llama_client
                .post(format!("{}/slots/0?action=restore", resolved.base_url()))
                .json(&serde_json::json!({"filename": validation_filename}))
                .send()
                .await?
                .error_for_status()?;
            let restore_body: serde_json::Value = restore.json().await?;
            let restored = restore_body
                .get("n_restored")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if restored != saved {
                anyhow::bail!(
                    "runtime slot restore probe mismatch: saved {saved}, restored {restored}"
                )
            }
            Ok((saved, restored))
        }
        .await;
        let _ = fs::remove_file(&validation_path);
        let (saved, restored) = probe_result?;
        log::info!(
            "runtime validation passed: slot save/restore saved={saved} restored={restored}"
        );
        Ok(self.ai_config_status())
    }

    pub async fn apply_ai_config(
        &self,
        candidate_hash: &str,
    ) -> anyhow::Result<crate::ai_config::AiConfigStatus> {
        if self.inner.ai.chat_lock.try_lock().is_err()
            || self.inner.ai.llama_slot_lock.try_lock().is_err()
        {
            anyhow::bail!("AI configuration cannot be applied while a model turn or cache operation is active")
        }
        self.ensure_ai_config()?;
        let config = crate::ai_config::load(&self.inner.app_data_dir)?;
        crate::ai_config::require_valid(&config)?;
        let actual = crate::ai_config::canonical_hash(&config)?;
        if actual != candidate_hash {
            anyhow::bail!("AI configuration changed after validation; validate it again")
        }
        crate::ai_config::write_atomic(
            &crate::ai_config::applied_path(&self.inner.app_data_dir),
            &config,
        )?;
        // Apply changes to the in-memory sidecar settings as part of the same
        // lifecycle transition. Without this projection, the next request
        // continued using the legacy settings.json values until process exit,
        // which made a correctly applied native TQ2 profile still log as
        // prompt-mode.
        let projected = project_ai_agent_settings(&self.openharn_settings(), &config.agent);
        *self.inner.openharn_settings.lock() = projected;
        self.invalidate_ai_pipeline();
        if let Ok(mut guard) = self.inner.ai.sidecar.try_lock() {
            *guard = None;
        }
        *self.inner.ai.active_slot_cache.lock() = None;
        Ok(self.ai_config_status())
    }
}
