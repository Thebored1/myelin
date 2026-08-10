use super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub(crate) fn index_dir(&self) -> PathBuf {
        self.inner.app_data_dir.join(INDEX_DIR_NAME)
    }

    pub(crate) fn workspace_data_dir(&self, workspace: &Path) -> PathBuf {
        crate::state::prepare_workspace_data_dir(&self.inner.app_data_dir, workspace)
            .unwrap_or_else(|error| {
                log::error!("failed to prepare workspace sidecar storage: {error}");
                self.inner.app_data_dir.join("workspaces").join(workspace_storage_key(workspace))
            })
    }

    pub(crate) fn persist_runtime_settings(&self) -> Result<()> {
        let runtime = self.inner.runtime.read();
        let mut settings = load_settings(&self.inner.app_data_dir)?;
        settings.workspace_path = runtime
            .workspace_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        settings.custom_note_order = runtime.custom_note_order.clone();
        save_settings(&self.inner.app_data_dir, &settings)?;
        Ok(())
    }

    pub(crate) async fn run_llama_prompt(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let config = llama_server::resolve_config(&self.inner.app_data_dir)?;
        self.ensure_llama_server(&config).await?;

        let full_prompt = system_prompt.to_string();
        let agent = crate::agent::build_myelin_agent(
            self.clone(),
            &format!("{}/v1", config.base_url()),
            &config.model_name(),
            &full_prompt,
            config.temperature as f64,
            config.max_turns as usize,
        );

        agent
            .prompt(user_prompt)
            .max_turns(config.max_turns as usize)
            .await
            .map_err(|error| anyhow!(describe_prompt_error(&error)))
    }

    /// Context window (tokens) the running llama-server launched with, if any.
    pub(crate) async fn running_ctx_size(&self) -> Option<u32> {
        self.inner
            .llama_server
            .lock()
            .await
            .as_ref()
            .map(|s| s.ctx_size)
    }

    /// The configured SearXNG base URL for web search (None → DuckDuckGo).
    pub async fn warm_llama_server(&self) -> Result<()> {
        self.warm_llama_server_for_note(None, None, None).await
    }

    pub async fn warm_llama_server_for_note(
        &self,
        note_id: Option<String>,
        interaction_mode: Option<String>,
        active_section: Option<crate::models::ActiveSection>,
    ) -> Result<()> {
        let configured = self.ensure_ai_pipeline_ready().await?;
        // Fingerprint and address the server that actually won candidate
        // selection. The configured preference may differ from the running
        // binary/backend after startup fallback.
        let (config, ctx_tokens) = {
            let server = self.inner.llama_server.lock().await;
            match server.as_ref() {
                Some(server) => (server.config.clone(), server.ctx_size as usize),
                None => (configured.clone(), configured.context_size as usize),
            }
        };
        let Some(note_id) = note_id else { return Ok(()); };
        let note = self.load_note(note_id.clone()).await?;
        let prompt_shape = crate::note_prompt::NotePromptShape::build(
            &note.body,
            &note.relative_path,
            ctx_tokens,
        );
        let section_scoped = active_section
            .as_ref()
            .is_some_and(|section| !section.content.trim().is_empty());
        let oversized_ready = if prompt_shape.oversized && !section_scoped {
            self.ensure_oversized_note_ingested(&note).await.is_ok()
        } else {
            false
        };
        let attachment_backed = note.source_pdf.is_some();
        let pdf_only = note.relative_path.to_ascii_lowercase().ends_with(".pdf");
        if attachment_backed && !section_scoped {
            let _ = self
                .ensure_document_ingested(&note.id, &note.title, &note.body)
                .await;
        }
        let retrieval_backed = prompt_shape.oversized || attachment_backed || pdf_only;
        let base_excerpt = if let Some(source_id) = note.source_pdf.as_deref() {
            let source_title = self
                .note_by_id(source_id)
                .map(|source| source.title)
                .unwrap_or_else(|| "Attached source".to_string());
            format!(
                "[ATTACHED NOTE — retrieval-backed]\n\
                 Working note ID: {}\nAttached source: {source_title} (ID: {source_id})\n\
                 Relevant passages from both are supplied per turn.",
                note.id
            )
        } else if pdf_only {
            format!(
                "[PDF — read-only retrieval-backed]\nDocument: {} (ID: {})\nRelevant passages are supplied per turn.",
                note.title, note.id
            )
        } else if prompt_shape.oversized && !oversized_ready {
            let limit = ctx_tokens.saturating_mul(2).clamp(4_000, 400_000);
            let head: String = note.body.chars().take(limit).collect();
            format!("{head}\n…[note truncated because full-note indexing failed]")
        } else {
            prompt_shape.body
        };
        let excerpt = active_section
            .as_ref()
            .filter(|section| !section.content.trim().is_empty())
            .map(section_excerpt)
            .unwrap_or(base_excerpt);
        let interaction_mode = match interaction_mode.as_deref() {
            Some("write") => "write",
            Some("operation") => "operation",
            Some("edit") => "edit",
            Some("auto") => "auto",
            _ => "chat",
        };
        let doc_type = note.relative_path.to_ascii_lowercase();
        let cells = if doc_type.ends_with(".ipynb") {
            crate::notebook::present(&note.body)
        } else {
            None
        };
        let template_kwargs = self.openharn_settings().template_kwargs;
        if config.prompt_cache
            && section_scoped
            && matches!(interaction_mode, "chat" | "write")
        {
            let common_system = add_no_think_directive(
                &assemble_section_context(
                    &note.title,
                    &excerpt,
                    cells.as_deref(),
                ),
                self.openharn_settings().no_think,
            );
            // Never enqueue synthetic inference while a real chat owns the
            // slot. This path is only a warm request for the shared prefix.
            if self.inner.chat_lock.try_lock().is_err() {
                return Ok(());
            }
            let _slot_guard = self.inner.llama_slot_lock.lock().await;
            if let Some(section) = active_section.as_ref() {
                self.prepare_section_slot(
                    &config,
                    &note_id,
                    section,
                    &common_system,
                    &template_kwargs,
                    interaction_mode,
                )
                .await;
                return Ok(());
            }
        }
        let model_id = config.model_path.to_string_lossy().to_string();
        let supports_tools = crate::tool_capability::supports_tools(
            &self.inner.llama_client,
            &config.base_url(),
            &self.inner.app_data_dir,
            &config.model_path,
            &model_id,
            config.supports_tools,
        )
        .await;
        // Retrieval-backed chat uses the same tool-free profile as the direct
        // document-answer route, so the warm-up can be reused by that request.
        // Ordinary tool-capable chat retains its fixed read-only schema prefix.
        let (system, tools) = warmup_prefix(
            &note.title,
            &excerpt,
            cells.as_deref(),
            interaction_mode,
            &doc_type,
            if interaction_mode == "chat" { false } else { supports_tools },
            retrieval_backed,
            config.verbose_tool_schemas,
        );
        let tools_json = serde_json::to_string(&tools).unwrap_or_default();
        let identity = Self::slot_identity(
            &config,
            ctx_tokens as u32,
            if interaction_mode == "chat" { "section" } else { interaction_mode },
            &system,
            &tools_json,
            &template_kwargs,
        );
        // Never enqueue synthetic inference while a real chat owns the turn.
        if self.inner.chat_lock.try_lock().is_err() {
            return Ok(());
        }
        let _slot_guard = self.inner.llama_slot_lock.lock().await;
        if config.prompt_cache {
            if let Some(section) = active_section.as_ref() {
                self.prepare_section_slot(
                    &config,
                    &note_id,
                    section,
                    &system,
                    &template_kwargs,
                    interaction_mode,
                )
                .await;
                return Ok(());
            }
            self.restore_note_slot(&config, &note_id, &identity).await;
        }
        self.spawn_note_cache_warmup(
            &config,
            note_id,
            interaction_mode.to_string(),
            system,
            tools,
            template_kwargs,
            identity,
        );
        Ok(())
    }

    /// Eagerly evaluate and persist one slot snapshot per document section so a
    /// later ask restores the active section's KV instead of re-evaluating it
    /// inline (the synchronous prime that cost ~9.7s in the trace). The scan is
    /// backgrounded: it yields to any in-flight chat turn (chat_lock polling)
    /// and its handle is tracked separately from the prompt warm-up so a turn
    /// never aborts it. Sections already persisted from an earlier session are
    /// skipped by the file-validity check, making a re-open effectively free.
    pub async fn stop_llama_server(&self) {
        self.invalidate_ai_pipeline();
        self.cancel_prompt_warmup().await;
        if let Some(handle) = self.inner.section_cache.lock().take() {
            if !handle.is_finished() {
                handle.abort();
            }
        }
        let mut guard = self.inner.llama_server.lock().await;
        if let Some(mut server) = guard.take() {
            llama_server::stop_server(&mut server).await;
            log::info!("llama-server stopped (note closed)");
        }
    }
}
