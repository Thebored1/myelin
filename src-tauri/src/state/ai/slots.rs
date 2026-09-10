use super::super::core::*;

impl AppState {
    pub async fn save_active_slot_before_quit(&self) {
        let record = match self.inner.ai.last_slot_save.lock().clone() {
            Some(record) => record,
            None => return,
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            self.save_note_slot(&record.0, &record.1).await;
        })
        .await;
    }

    pub(crate) fn slot_filename(note_id: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        note_id.hash(&mut hasher);
        format!("note-{:016x}.slot", hasher.finish())
    }
    pub(crate) fn section_slot_filename(
        note_id: &str,
        section: &crate::models::ActiveSection,
        identity: &str,
    ) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        note_id.hash(&mut hasher);
        section.key.hash(&mut hasher);
        section_excerpt(section).hash(&mut hasher);
        identity.hash(&mut hasher);
        format!("section-{:016x}.slot", hasher.finish())
    }
    pub(crate) fn slot_manifest_path(
        config: &llama_server::ResolvedLlamaConfig,
        note_id: &str,
    ) -> PathBuf {
        config
            .slot_save_path
            .join(format!("{}.json", Self::slot_filename(note_id)))
    }
    pub(crate) fn slot_identity(
        config: &llama_server::ResolvedLlamaConfig,
        running_ctx: u32,
        interaction_mode: &str,
        system: &str,
        tools_json: &str,
        template_kwargs: &Option<String>,
    ) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        config.inference_engine.hash(&mut hasher);
        config.executable_path.hash(&mut hasher);
        let exe_meta = std::fs::metadata(&config.executable_path).ok();
        exe_meta
            .as_ref()
            .map(|m| m.len())
            .unwrap_or(0)
            .hash(&mut hasher);
        exe_meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .hash(&mut hasher);
        config.model_path.hash(&mut hasher);
        let model_meta = std::fs::metadata(&config.model_path).ok();
        model_meta
            .as_ref()
            .map(|m| m.len())
            .unwrap_or(0)
            .hash(&mut hasher);
        model_meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .hash(&mut hasher);
        running_ctx.hash(&mut hasher);
        config.backend.hash(&mut hasher);
        config.gpu_layers.hash(&mut hasher);
        config.threads.hash(&mut hasher);
        config.gpu_device.hash(&mut hasher);
        config.chat_format.hash(&mut hasher);
        config.thinking.hash(&mut hasher);
        let wire_revision = if interaction_mode == "section-common" {
            "slot-wire-v5-shared-section-system-boundary"
        } else {
            "slot-wire-v4-exact-user-boundary"
        };
        wire_revision.hash(&mut hasher);
        config.chat_template_override.hash(&mut hasher);
        match config.chat_template_override.as_deref() {
            Some("lfm2") => include_str!("../../../templates/lfm2.jinja").hash(&mut hasher),
            Some("lfm25") => include_str!("../../../templates/lfm25.jinja").hash(&mut hasher),
            Some(path) if !path.trim().is_empty() => {
                std::fs::read(path).unwrap_or_default().hash(&mut hasher)
            }
            _ => {}
        }
        config.extra_args.hash(&mut hasher);
        crate::tool_capability::fingerprint(&config.model_path).hash(&mut hasher);
        interaction_mode.hash(&mut hasher);
        system.hash(&mut hasher);
        tools_json.hash(&mut hasher);
        template_kwargs.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
    pub(crate) fn slot_file_is_valid(
        config: &llama_server::ResolvedLlamaConfig,
        filename: &str,
        identity: &str,
    ) -> bool {
        let slot_path = config.slot_save_path.join(filename);
        let manifest_path = config.slot_save_path.join(format!("{filename}.json"));
        let manifest_matches = |path: &Path| {
            fs::read_to_string(path)
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        };
        if slot_path.exists()
            && manifest_matches(&manifest_path)
                .as_ref()
                .is_some_and(|value| {
                    value["identity"].as_str() == Some(identity)
                        && value["nSaved"].as_u64().is_some_and(|count| count > 0)
                })
        {
            return true;
        }
        let Some(slots_root) = config.slot_save_path.parent().and_then(Path::parent) else {
            return false;
        };
        let legacy_dir = slots_root.join(&config.inference_engine);
        let legacy_slot = legacy_dir.join(filename);
        let legacy_manifest = legacy_dir.join(format!("{filename}.json"));
        if !legacy_slot.exists()
            || !manifest_matches(&legacy_manifest)
                .as_ref()
                .is_some_and(|value| {
                    value["identity"].as_str() == Some(identity)
                        && value["nSaved"].as_u64().is_some_and(|count| count > 0)
                })
        {
            return false;
        }
        if fs::create_dir_all(&config.slot_save_path).is_err()
            || fs::copy(&legacy_slot, &slot_path).is_err()
            || fs::copy(&legacy_manifest, &manifest_path).is_err()
        {
            return false;
        }
        log::info!(
            "promoted compatible legacy slot cache {} into {}",
            legacy_slot.display(),
            slot_path.display()
        );
        true
    }
    pub(crate) async fn restore_slot_file(
        &self,
        config: &llama_server::ResolvedLlamaConfig,
        filename: &str,
        identity: &str,
    ) -> bool {
        let slot_path = config.slot_save_path.join(filename);
        if !slot_path.exists() {
            return false;
        }
        let manifest_path = config.slot_save_path.join(format!("{filename}.json"));
        let manifest = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
        let expected_tokens = manifest
            .as_ref()
            .filter(|value| value["identity"].as_str() == Some(identity))
            .and_then(|value| value["nSaved"].as_u64());
        if expected_tokens.is_none_or(|count| count == 0) {
            let _ = fs::remove_file(&slot_path);
            let _ = fs::remove_file(&manifest_path);
            return false;
        }
        let url = format!("{}/slots/0?action=restore", config.base_url());
        match self
            .inner
            .llama_client
            .post(url)
            .json(&serde_json::json!({"filename": filename}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let payload = response
                    .json::<serde_json::Value>()
                    .await
                    .unwrap_or_default();
                let restored = payload["n_restored"].as_u64().unwrap_or(0);
                let expected = expected_tokens.unwrap_or(0);
                if restored != expected {
                    log::warn!(
                        "slot restore count mismatch for {filename}: expected {expected}, restored {restored}"
                    );
                    let _ = fs::remove_file(&slot_path);
                    let _ = fs::remove_file(&manifest_path);
                    return false;
                }
                if let Ok(file) = fs::File::options().write(true).open(&slot_path) {
                    let _ = file.set_modified(std::time::SystemTime::now());
                }
                let restore_ms = payload["timings"]["restore_ms"].as_f64().unwrap_or(0.0);
                log::info!(
                    "restored section slot {filename}: {restored} tokens in {restore_ms:.1} ms"
                );
                true
            }
            Ok(response) => {
                log::warn!("slot restore failed for {filename}: {}", response.status());
                let _ = fs::remove_file(&slot_path);
                let _ = fs::remove_file(&manifest_path);
                false
            }
            Err(error) => {
                log::warn!("slot restore request failed for {filename}: {error}");
                false
            }
        }
    }
    pub(crate) async fn save_slot_file(
        &self,
        config: &llama_server::ResolvedLlamaConfig,
        filename: &str,
        identity: &str,
        kind: &str,
    ) -> bool {
        let url = format!("{}/slots/0?action=save", config.base_url());
        match self
            .inner
            .llama_client
            .post(url)
            .json(&serde_json::json!({"filename": filename}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let payload = response
                    .json::<serde_json::Value>()
                    .await
                    .unwrap_or_default();
                let n_saved = payload["n_saved"].as_u64().unwrap_or(0);
                if n_saved == 0 {
                    log::warn!("slot save returned zero tokens for {filename}");
                    let _ = fs::remove_file(config.slot_save_path.join(filename));
                    return false;
                }
                let manifest_path = config.slot_save_path.join(format!("{filename}.json"));
                let manifest_tmp = config.slot_save_path.join(format!("{filename}.json.tmp"));
                let manifest = serde_json::json!({
                    "identity": identity,
                    "kind": kind,
                    "profile": kind,
                    "nSaved": n_saved,
                });
                if let Err(error) = fs::write(&manifest_tmp, manifest.to_string())
                    .and_then(|_| fs::rename(&manifest_tmp, &manifest_path))
                {
                    log::warn!("could not commit slot manifest for {filename}: {error}");
                    let _ = fs::remove_file(&manifest_tmp);
                    let _ = fs::remove_file(config.slot_save_path.join(filename));
                    return false;
                }
                enforce_slot_cache_budget(&config.slot_save_path, SLOT_CACHE_BUDGET_BYTES);
                let save_ms = payload["timings"]["save_ms"].as_f64().unwrap_or(0.0);
                log::info!("saved {kind} slot {filename}: {n_saved} tokens in {save_ms:.1} ms");
                true
            }
            Ok(response) => {
                log::warn!("slot save failed for {filename}: {}", response.status());
                false
            }
            Err(error) => {
                log::warn!("slot save request failed for {filename}: {error}");
                false
            }
        }
    }
    pub(crate) async fn prepare_section_slot(
        &self,
        config: &llama_server::ResolvedLlamaConfig,
        note_id: &str,
        section: &crate::models::ActiveSection,
        system: &str,
        template_kwargs: &Option<String>,
        profile_mode: &str,
    ) {
        if !config.prompt_cache || section.content.trim().is_empty() {
            return;
        }
        let running_ctx = self.running_ctx_size().await.unwrap_or(config.context_size);
        let clean_identity = Self::slot_identity(
            config,
            running_ctx,
            "section-common",
            system,
            "",
            template_kwargs,
        );
        let filename = Self::section_slot_filename(note_id, section, &clean_identity);
        if self
            .inner.ai
            .active_slot_cache
            .lock()
            .as_ref()
            .is_some_and(|cached| {
                cached.note_id == note_id
                    && cached.filename == filename
                    && cached.identity == clean_identity
            })
        {
            log::debug!(
                "section cache resident: profile={profile_mode} file={filename}; reusing active slot"
            );
            return;
        }
        self.cancel_prompt_warmup().await;
        if self
            .restore_slot_file(config, &filename, &clean_identity)
            .await
        {
            log::info!("section cache hit: profile={profile_mode} file={filename}");
            *self.inner.ai.active_slot_cache.lock() = Some(ActiveSlotCache {
                note_id: note_id.to_string(),
                filename,
                identity: clean_identity,
            });
            return;
        }
        log::info!(
            "section cache miss: profile={profile_mode} file={filename}; priming synchronously"
        );
        let body =
            match section_prime_body(&self.inner.llama_client, config, system, template_kwargs)
                .await
            {
                Ok(body) => body,
                Err(error) => {
                    log::warn!("section warm-up template render failed: {error}");
                    return;
                }
            };
        let url = format!("{}/completion", config.base_url());
        match self.inner.llama_client.post(url).json(&body).send().await {
            Ok(response) if response.status().is_success() => {
                if self
                    .save_slot_file(config, &filename, &clean_identity, "shared")
                    .await
                {
                    *self.inner.ai.active_slot_cache.lock() = Some(ActiveSlotCache {
                        note_id: note_id.to_string(),
                        filename,
                        identity: clean_identity,
                    });
                }
            }
            Ok(response) => log::warn!("section warm-up returned {}", response.status()),
            Err(error) => log::warn!("section warm-up failed: {error}"),
        }
    }
    pub(crate) async fn restore_note_slot(
        &self,
        config: &llama_server::ResolvedLlamaConfig,
        note_id: &str,
        identity: &str,
    ) {
        let filename = Self::slot_filename(note_id);
        let slot_path = config.slot_save_path.join(&filename);
        if !slot_path.exists() {
            return;
        }
        let recorded = std::fs::read_to_string(Self::slot_manifest_path(config, note_id))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v["identity"].as_str().map(str::to_string));
        if recorded.as_deref() != Some(identity) {
            log::info!("discarding stale llama slot for {note_id} (identity mismatch)");
            let _ = fs::remove_file(&slot_path);
            let _ = fs::remove_file(Self::slot_manifest_path(config, note_id));
            return;
        }
        let url = format!("{}/slots/0?action=restore", config.base_url());
        match self
            .inner
            .llama_client
            .post(url)
            .json(&serde_json::json!({"filename": filename}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                log::info!("restored persistent llama slot for note {note_id}");
                if let Ok(file) = std::fs::File::options().write(true).open(&slot_path) {
                    let _ = file.set_modified(std::time::SystemTime::now());
                }
            }
            Ok(response) => {
                log::warn!("slot restore failed for {note_id}: {}", response.status());
                let _ = fs::remove_file(&slot_path);
                let _ = fs::remove_file(Self::slot_manifest_path(config, note_id));
            }
            Err(error) => log::warn!("slot restore request failed for {note_id}: {error}"),
        }
    }
    pub(crate) async fn save_note_slot(&self, note_id: &str, identity: &str) {
        let config = {
            let server = self.inner.ai.llama_server.lock().await;
            match server.as_ref() {
                Some(server) => server.config.clone(),
                None => return,
            }
        };
        if !config.prompt_cache {
            return;
        }
        let filename = Self::slot_filename(note_id);
        let url = format!("{}/slots/0?action=save", config.base_url());
        match self
            .inner
            .llama_client
            .post(url)
            .json(&serde_json::json!({"filename": filename}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                log::info!("saved persistent llama slot for note {note_id}");
                let manifest_path = config.slot_save_path.join(format!("{filename}.json"));
                let _ = std::fs::write(
                    &manifest_path,
                    serde_json::json!({ "identity": identity }).to_string(),
                );
                *self.inner.ai.last_slot_save.lock() =
                    Some((note_id.to_string(), identity.to_string()));
                enforce_slot_cache_budget(&config.slot_save_path, SLOT_CACHE_BUDGET_BYTES);
            }
            Ok(response) => log::warn!("slot save failed for {note_id}: {}", response.status()),
            Err(error) => log::warn!("slot save request failed for {note_id}: {error}"),
        }
    }
    pub(crate) fn spawn_note_cache_warmup(
        &self,
        config: &llama_server::ResolvedLlamaConfig,
        note_id: String,
        interaction_mode: String,
        system: String,
        tools: Vec<serde_json::Value>,
        template_kwargs: Option<String>,
        identity: String,
    ) {
        use std::hash::{Hash, Hasher};
        let state = self.clone();
        let no_think = config.thinking && {
            let oh = self.openharn_settings();
            oh.no_think || interaction_mode == "chat"
        };
        let client = self.inner.llama_client.clone();
        let url = format!("{}/v1/chat/completions", config.base_url());
        let model = config.model_name();
        let persist = config.prompt_cache;
        let parsed_template_kwargs = template_kwargs
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        model.hash(&mut hasher);
        system.hash(&mut hasher);
        interaction_mode.hash(&mut hasher);
        serde_json::to_string(&tools)
            .unwrap_or_default()
            .hash(&mut hasher);
        template_kwargs.hash(&mut hasher);
        let key = hasher.finish();
        let mut warmup = self.inner.ai.prompt_warmup.lock();
        if let Some((existing_key, handle)) = warmup.as_ref() {
            if *existing_key == key && !handle.is_finished() {
                return;
            }
            if !handle.is_finished() {
                handle.abort();
            }
        }
        let handle = tokio::spawn(async move {
            let _slot_guard = state.inner.ai.llama_slot_lock.lock().await;
            *state.inner.ai.active_slot_cache.lock() = None;
            let mut messages = vec![
                serde_json::json!({ "role": "system", "content": system }),
                serde_json::json!({ "role": "user", "content": " " }),
            ];
            if no_think {
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": " thinking response",
                }));
            }
            let mut body = serde_json::json!({
                "model": model,
                "messages": messages,
                "max_tokens": 1,
                "temperature": 0.0,
                "cache_prompt": true,
                "id_slot": 0,
            });
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
                body["tool_choice"] = serde_json::json!("none");
            }
            if let Some(kwargs) = parsed_template_kwargs {
                body["chat_template_kwargs"] = kwargs;
            }
            match client.post(&url).json(&body).send().await {
                Ok(response) if response.status().is_success() => {
                    let usage = response
                        .json::<serde_json::Value>()
                        .await
                        .ok()
                        .and_then(|value| value.get("usage").cloned());
                    log::info!("llama note prompt-cache warm-up complete; usage={usage:?}");
                    if persist {
                        state.save_note_slot(&note_id, &identity).await;
                    }
                }
                Ok(response) => log::warn!(
                    "llama note prompt-cache warm-up returned {}",
                    response.status()
                ),
                Err(error) => log::warn!("llama note prompt-cache warm-up failed: {error}"),
            }
        });
        *warmup = Some((key, handle));
    }
}
