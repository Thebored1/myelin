use super::super::core::*;
use ::anyhow::Result;
impl AppState {
    pub async fn cache_note_sections(
        &self,
        note_id: String,
        sections: Vec<crate::models::ActiveSection>,
        active_section_key: Option<String>,
        _legacy_interaction_mode: Option<String>,
    ) -> Result<()> {
        let sections: Vec<crate::models::ActiveSection> = sections
            .into_iter()
            .filter(|section| !section.content.trim().is_empty())
            .collect();
        if sections.is_empty() {
            return Ok(());
        }
        let scan_note_id = note_id.clone();
        let scan_total = sections.len();
        let scan_total_profiles = scan_total;
        let emit_progress = |state: &AppState,
                             done: usize,
                             section_done: usize,
                             label: &str,
                             profile: &str,
                             failed: usize,
                             failed_details: &[String],
                             finished: bool| {
            let _ = state.handle.emit(
                "ai://section_cache_progress",
                serde_json::json!({
                    "noteId": scan_note_id,
                    "done": done,
                    "total": scan_total_profiles,
                    "sectionDone": section_done,
                    "sectionTotal": scan_total,
                    "label": label,
                    "profile": profile,
                    "failed": failed,
                    "failedDetails": failed_details,
                    "finished": finished,
                }),
            );
        };
        emit_progress(self, 0, 0, "", "", 0, &[], false);
        if let Some(handle) = self.inner.ai.section_cache.lock().take() {
            if !handle.is_finished() {
                handle.abort();
            }
        }
        let configured = match self.ensure_ai_pipeline_ready().await {
            Ok(configured) => configured,
            Err(error) => {
                emit_progress(self, 0, 0, "", "", scan_total_profiles, &[], true);
                return Err(error);
            }
        };
        let (config, ctx_tokens) = {
            let server = self.inner.ai.llama_server.lock().await;
            match server.as_ref() {
                Some(server) => (server.config.clone(), server.ctx_size as usize),
                None => (configured.clone(), configured.context_size as usize),
            }
        };
        if !config.prompt_cache {
            emit_progress(self, 0, 0, "", "", scan_total_profiles, &[], true);
            return Ok(());
        }
        let note = match self.load_note(note_id.clone()).await {
            Ok(note) => note,
            Err(error) => {
                emit_progress(self, 0, 0, "", "", scan_total_profiles, &[], true);
                return Err(error);
            }
        };
        let doc_type = note.relative_path.to_ascii_lowercase();
        let cells = if doc_type.ends_with(".ipynb") {
            crate::notebook::present(&note.body)
        } else {
            None
        };
        let template_kwargs = self.openharn_settings().template_kwargs;
        struct SectionProfile {
            section: crate::models::ActiveSection,
            profile: &'static str,
            common_system: String,
            identity: String,
            filename: String,
        }
        let active_index = active_section_key
            .as_deref()
            .and_then(|key| sections.iter().position(|section| section.key == key))
            .unwrap_or(0);
        let mut ordered_sections = Vec::with_capacity(sections.len());
        ordered_sections.push(sections[active_index].clone());
        ordered_sections.extend(
            sections
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != active_index)
                .map(|(_, section)| section.clone()),
        );
        let mut prepared_by_section: std::collections::HashMap<String, u8> =
            std::collections::HashMap::new();
        let mut preprepared = 0usize;
        let prefailed = 0usize;
        let prefailed_details = Vec::new();
        let mut scheduled = Vec::with_capacity(scan_total_profiles);
        let mut enqueue = |section: &crate::models::ActiveSection| {
            let excerpt = section_excerpt(section);
            let common_system = add_no_think_directive(
                &assemble_section_context(&note.title, &excerpt, cells.as_deref()),
                self.openharn_settings().no_think,
            );
            let identity = Self::slot_identity(
                &config,
                ctx_tokens as u32,
                "section-common",
                &common_system,
                "",
                &template_kwargs,
            );
            let filename = Self::section_slot_filename(&note_id, section, &identity);
            let valid = Self::slot_file_is_valid(&config, &filename, &identity)
                || self
                    .inner.ai
                    .active_slot_cache
                    .lock()
                    .as_ref()
                    .is_some_and(|cached| {
                        cached.note_id == note_id
                            && cached.filename == filename
                            && cached.identity == identity
                    });
            if valid {
                preprepared += 1;
                prepared_by_section.insert(section.key.clone(), 1);
            } else {
                scheduled.push(SectionProfile {
                    section: section.clone(),
                    profile: "shared",
                    common_system,
                    identity,
                    filename,
                });
            }
        };
        for section in &ordered_sections {
            enqueue(section);
        }
        drop(enqueue);
        let section_done = prepared_by_section
            .values()
            .filter(|count| **count >= 1)
            .count();
        if scheduled.is_empty() {
            emit_progress(
                self,
                preprepared,
                section_done,
                "",
                "",
                prefailed,
                &prefailed_details,
                true,
            );
            return Ok(());
        }
        let state = self.clone();
        let client = self.inner.llama_client.clone();
        let url = format!("{}/completion", config.base_url());
        let config_for_save = config.clone();
        let handle = tokio::spawn(async move {
            let total = scan_total_profiles;
            let scan_note_id = note_id.clone();
            let mut prepared_by_section = prepared_by_section;
            let emit = |done: usize,
                        section_done: usize,
                        label: &str,
                        profile: &str,
                        failed: usize,
                        failed_details: &[String],
                        finished: bool| {
                let _ = state.handle.emit(
                    "ai://section_cache_progress",
                    serde_json::json!({
                        "noteId": scan_note_id,
                        "done": done,
                        "total": total,
                        "sectionDone": section_done,
                        "sectionTotal": scan_total,
                        "label": label,
                        "profile": profile,
                        "failed": failed,
                        "failedDetails": failed_details,
                        "finished": finished,
                    }),
                );
            };
            let mut prepared = preprepared;
            let mut failed = prefailed;
            let mut failed_details = prefailed_details;
            emit(
                prepared,
                section_done,
                "",
                "",
                failed,
                &failed_details,
                false,
            );
            for profile in &scheduled {
                while state
                    .inner.ai
                    .section_cache_preempt
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                while state.inner.ai.chat_lock.try_lock().is_err() {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                if state
                    .inner.ai
                    .section_cache_preempt
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    continue;
                }
                let _slot_guard = state.inner.ai.llama_slot_lock.lock().await;
                if state
                    .inner.ai
                    .section_cache_preempt
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    continue;
                }
                let label = profile.section.label.clone().unwrap_or_default();
                let section_done = prepared_by_section
                    .values()
                    .filter(|count| **count >= 1)
                    .count();
                emit(
                    prepared,
                    section_done,
                    &label,
                    profile.profile,
                    failed,
                    &failed_details,
                    false,
                );
                let body = match tokio::select! {
                    result = section_prime_body(
                        &client,
                        &config,
                        &profile.common_system,
                        &template_kwargs,
                    ) => result,
                    _ = state.inner.ai.section_cache_resume.notified(),
                        if state.inner.ai.section_cache_preempt.load(std::sync::atomic::Ordering::SeqCst) =>
                    {
                        continue;
                    }
                } {
                    Ok(body) => body,
                    Err(error) => {
                        failed += 1;
                        failed_details.push(format!("{} · {}", label, profile.profile));
                        log::warn!("section cache template render failed: {error}");
                        continue;
                    }
                };
                let response = tokio::select! {
                    result = client.post(&url).json(&body).send() => result,
                    _ = state.inner.ai.section_cache_resume.notified(),
                        if state.inner.ai.section_cache_preempt.load(std::sync::atomic::Ordering::SeqCst) =>
                    {
                        continue;
                    }
                };
                match response {
                    Ok(response) if response.status().is_success() => {
                        if state
                            .save_slot_file(
                                &config_for_save,
                                &profile.filename,
                                &profile.identity,
                                profile.profile,
                            )
                            .await
                        {
                            *state.inner.ai.active_slot_cache.lock() = Some(ActiveSlotCache {
                                note_id: scan_note_id.clone(),
                                filename: profile.filename.clone(),
                                identity: profile.identity.clone(),
                            });
                            prepared += 1;
                            prepared_by_section.insert(profile.section.key.clone(), 1);
                            log::info!(
                                "section cache: cached {label} {} ({}/{})",
                                profile.profile,
                                prepared,
                                total
                            );
                        } else {
                            *state.inner.ai.active_slot_cache.lock() = None;
                            failed += 1;
                            failed_details.push(format!("{} · {}", label, profile.profile));
                        }
                    }
                    Ok(response) => {
                        failed += 1;
                        failed_details.push(format!("{} · {}", label, profile.profile));
                        log::warn!("section cache prime returned {}", response.status())
                    }
                    Err(error) => {
                        failed += 1;
                        failed_details.push(format!("{} · {}", label, profile.profile));
                        log::warn!("section cache prime failed: {error}")
                    }
                }
            }
            let section_done = prepared_by_section
                .values()
                .filter(|count| **count >= 1)
                .count();
            emit(
                prepared,
                section_done,
                "",
                "",
                failed,
                &failed_details,
                true,
            );
        });
        let mut active_scan = self.inner.ai.section_cache.lock();
        if let Some(handle) = active_scan.take() {
            if !handle.is_finished() {
                handle.abort();
            }
        }
        *active_scan = Some(handle);
        Ok(())
    }
}
