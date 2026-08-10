use super::super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;
impl AppState {
    pub async fn ask_ai_stream(
        &self,
        note_id: String,
        question: String,
        request_id: String,
        selection: Option<crate::agent::SelectionArg>,
        doc_type: Option<String>,
        interaction_mode: Option<String>,
        active_section: Option<crate::models::ActiveSection>,
    ) -> Result<()> {
        let _chat_guard = match self.inner.chat_lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.request_ai_cancel();
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    self.inner.chat_lock.lock(),
                )
                .await
                .map_err(|_| anyhow!("The previous AI request did not stop within 10 seconds."))?
            }
        };
        let interaction_mode = match interaction_mode.as_deref() {
            None | Some("auto") => "auto",
            Some("chat") => "chat",
            Some("write") => "write",
            Some("operation") => "operation",
            Some("edit") => "edit",
            Some(mode) => return Err(anyhow!("unknown AI interaction mode: {mode}")),
        };
        self.inner.targeted_write.store(
            interaction_mode == "write",
            std::sync::atomic::Ordering::SeqCst,
        );
        self.reset_chat_tools();
        self.inner
            .cancel_ai
            .store(false, std::sync::atomic::Ordering::Release);
        self.set_latest_chat_question(question.clone());
        let selection = selection.filter(|s| s.cursor || !s.text.trim().is_empty());
        self.set_current_selection(selection.clone());
        let doc_type = doc_type.unwrap_or_else(|| "md".to_string());
        self.set_current_doc_type(Some(doc_type.clone()));
        self.set_current_note_id(note_id.clone());
        self.pause_section_cache_for_turn();
        let result: Result<()> = async {
            let setup_started = std::time::Instant::now();
            let _ = self.handle.emit(
                "ai://debug_event",
                serde_json::json!({
                    "kind": "setup",
                    "msg": "begin request setup",
                    "requestId": request_id,
                }),
            );
            let note = self.load_note(note_id).await?;
            let _ = self.handle.emit(
                "ai://debug_event",
                serde_json::json!({
                    "kind": "setup",
                    "msg": format!("note loaded in {} ms", setup_started.elapsed().as_millis()),
                    "requestId": request_id,
                }),
            );
            if matches!(interaction_mode, "write" | "operation" | "edit") && selection.is_none() {
                return Err(anyhow!(
                    "Place the cursor where you want to write, or select text to rewrite. Then send again."
                ));
            }
            let external_settings = self.openharn_settings();
            if external_settings.external_enabled && !external_settings.external_ready() {
                return Err(anyhow!(
                    "External model mode is enabled, but its API base URL and model name are not configured."
                ));
            }
            let pipeline_started = std::time::Instant::now();
            let external_model = external_settings.external_ready();
            let config = if external_model {
                self.ensure_external_pipeline_ready().await?
            } else {
                self.ensure_ai_pipeline_ready().await?
            };
            let _ = self.handle.emit(
                "ai://debug_event",
                serde_json::json!({
                    "kind": "setup",
                    "msg": format!("pipeline ready in {} ms; total {} ms", pipeline_started.elapsed().as_millis(), setup_started.elapsed().as_millis()),
                    "requestId": request_id,
                }),
            );
            let deterministic_tools = false;
            self.set_deterministic_tools_runtime(deterministic_tools);
            self.set_tool_gating_runtime(config.tool_gating);
            let ctx_tokens = if external_model {
                config.context_size as usize
            } else {
                self.running_ctx_size()
                    .await
                    .unwrap_or(config.context_size) as usize
            };
            let prompt_shape = crate::note_prompt::NotePromptShape::build(
                &note.body,
                &note.relative_path,
                ctx_tokens,
            );
            let attachment_backed = note.source_pdf.is_some();
            let pdf_only = note.relative_path.to_ascii_lowercase().ends_with(".pdf");
            let model_question = crate::ai_turn::normalize_document_question(&question);
            let whole_document_request =
                crate::ai_turn::is_whole_document_request(&model_question);
            let other_page_request = active_section.as_ref().is_some_and(|section| {
                crate::ai_turn::references_other_page(
                    &model_question,
                    section.label.as_deref(),
                )
            });
            let active_section_request = matches!(interaction_mode, "chat" | "write")
                && active_section.is_some()
                && !whole_document_request
                && !other_page_request
                && !crate::agent::wants_other_notes(&model_question)
                && !crate::agent::wants_fetch(&model_question)
                && !crate::agent::wants_search(&model_question);
            let oversized_ready = if prompt_shape.oversized && !active_section_request {
                self.ensure_oversized_note_ingested(&note).await.is_ok()
            } else {
                false
            };
            let attachment_note_ready = if attachment_backed && !active_section_request {
                self.ensure_document_ingested(&note.id, &note.title, &note.body)
                    .await
                    .is_ok()
            } else {
                false
            };
            let retrieval_backed = prompt_shape.oversized || attachment_backed || pdf_only;
            let mut retrieval_evidence_ready = false;
            let base_note_body_excerpt = if let Some(source_id) = note.source_pdf.as_deref() {
                let source_title = self
                    .note_by_id(source_id)
                    .map(|source| source.title)
                    .unwrap_or_else(|| "Attached source".to_string());
                format!(
                    "[ATTACHED NOTE — retrieval-backed]\n\
                     Working note ID: {}\nAttached source: {source_title} (ID: {source_id})\n\
                     Relevant passages from both are supplied per turn.",
                    note.id,
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
                prompt_shape.body.clone()
            };
            let note_body_excerpt = active_section
                .as_ref()
                .filter(|section| !section.content.trim().is_empty())
                .map(section_excerpt)
                .unwrap_or(base_note_body_excerpt);
            let isolated_edit = interaction_mode == "edit";
            let append_only = !isolated_edit
                && interaction_mode != "write"
                && crate::agent::append_request_intent(&question);
            let placement = crate::agent::placement_request_intent(&question);
            let has_selection = selection.is_some();
            let notebook_cells = if doc_type == "ipynb" {
                crate::notebook::present(&note.body)
            } else {
                None
            };
            let context = if interaction_mode == "write" {
                if active_section_request {
                    assemble_section_context(
                        &note.title,
                        &note_body_excerpt,
                        notebook_cells.as_deref(),
                    )
                } else {
                    assemble_targeted_write_context(
                        &note.title,
                        &note_body_excerpt,
                        notebook_cells.as_deref(),
                    )
                }
            } else if active_section_request {
                assemble_section_context(
                    &note.title,
                    &note_body_excerpt,
                    notebook_cells.as_deref(),
                )
            } else {
                assemble_note_context(&note.title, &note_body_excerpt, notebook_cells.as_deref())
            };
            let stable_context = add_no_think_directive(
                &context,
                self.openharn_settings().no_think,
            );
            let mut turn_instructions = String::new();
            if (attachment_backed || pdf_only || oversized_ready)
                && !is_simple_greeting(&question)
                && !active_section_request
            {
                let mut scope = vec![note.id.clone()];
                if let Some(source_id) = note.source_pdf.clone() {
                    scope.push(source_id);
                }
                if attachment_note_ready || pdf_only || oversized_ready || scope.len() > 1 {
                    let retrieval_history = chat_history_to_messages(&note.chat_history);
                    let retrieval_query = crate::ai_turn::contextual_retrieval_query(
                        &model_question,
                        &retrieval_history,
                    );
                    let query_preview: String = retrieval_query.chars().take(1_000).collect();
                    let _ = self.handle.emit(
                        "ai://debug_event",
                        serde_json::json!({
                            "kind": "retrieval",
                            "msg": format!("query={query_preview:?}; scope={scope:?}"),
                            "requestId": request_id,
                        }),
                    );
                    let broad_query =
                        crate::ai_turn::is_broad_retrieval_request(&model_question);
                    let direct_document_request = interaction_mode == "chat"
                        && !crate::agent::wants_other_notes(&model_question)
                        && !crate::agent::wants_fetch(&model_question)
                        && !crate::agent::wants_search(&model_question);
                    let first_pass_k = if broad_query { 2 } else { 3 };
                    match self
                        .retrieve_chunks_scoped(&retrieval_query, first_pass_k, Some(&scope))
                        .await
                    {
                        Ok(mut chunks) => {
                            let mut retrieval_passes = 1;
                            if broad_query {
                                const RETRIEVAL_STEP: usize = 2;
                                const MAX_RETRIEVAL_K: usize = 12;
                                let mut current_k = first_pass_k;
                                loop {
                                    let next_k = (current_k + RETRIEVAL_STEP)
                                        .min(MAX_RETRIEVAL_K);
                                    if next_k <= current_k {
                                        break;
                                    }
                                    let Ok(expanded) = self
                                        .retrieve_chunks_scoped(
                                            &retrieval_query,
                                            next_k,
                                            Some(&scope),
                                        )
                                        .await
                                    else {
                                        break;
                                    };
                                    if !crate::ai_turn::retrieval_expansion_has_signal(
                                        &model_question,
                                        &chunks,
                                        &expanded,
                                    ) {
                                        break;
                                    }
                                    chunks = expanded;
                                    retrieval_passes += 1;
                                    current_k = next_k;
                                }
                            }
                            let mut document_ids: Vec<&str> =
                                chunks.iter().map(|chunk| chunk.doc_id.as_str()).collect();
                            document_ids.sort_unstable();
                            document_ids.dedup();
                            let _ = self.handle.emit(
                                "ai://debug_event",
                                serde_json::json!({
                                    "kind": "retrieval",
                                    "msg": format!(
                                        "results={}; document_ids={document_ids:?}",
                                        chunks.len()
                                    ),
                                    "requestId": request_id,
                                }),
                            );
                            let char_budget = ctx_tokens
                                .saturating_mul(4)
                                .min(if broad_query {
                                    16_000
                                } else if direct_document_request {
                                    800
                                } else {
                                    4_000
                                })
                                .max(if direct_document_request { 600 } else { 1_600 });
                            let evidence = if direct_document_request && !broad_query {
                                crate::rag::pack_passages_focused(
                                    chunks,
                                    &model_question,
                                    char_budget,
                                    1,
                                )
                            } else {
                                crate::rag::pack_passages_limited(
                                    chunks,
                                    char_budget,
                                    if broad_query { 12 } else { 3 },
                                )
                            };
                            let _ = self.handle.emit(
                                "ai://debug_event",
                                serde_json::json!({
                                    "kind": "retrieval",
                                    "msg": format!(
                                        "passes={retrieval_passes}; evidence_chars={}; evidence_budget_chars={char_budget}",
                                        evidence.chars().count(),
                                    ),
                                    "requestId": request_id,
                                }),
                            );
                            if !evidence.is_empty() {
                                retrieval_evidence_ready = true;
                                turn_instructions.push_str(
                                    "\n\nAUTOMATIC RETRIEVAL FROM THE OPEN NOTE + ATTACHED SOURCE:",
                                );
                                turn_instructions.push_str(&evidence);
                                turn_instructions.push_str(
                                    "\n\nEVIDENCE POLICY: Retrieved passages and explicit quotations from the user outrank prior assistant claims. Never claim a term or fact is absent when the supplied evidence contains it. If the evidence contradicts an earlier answer, acknowledge and correct that mistake. If the evidence is insufficient, say so instead of inventing a definitive negative. Use only passages relevant to the request.",
                                );
                                if crate::ai_turn::is_verbatim_document_request(&model_question) {
                                    turn_instructions.push_str(
                                        "\n\nVERBATIM DOCUMENT REQUEST: The user wants text from the retrieved document. Treat 'recite', 'quote', 'transcribe', and 'reproduce' as requests to reproduce the matching passage faithfully. Do not reinterpret the request as a recipe or as a question about whether the document contains instructions. Preserve the document's wording and say clearly if the supplied passages are incomplete.",
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            let _ = self.handle.emit(
                                "ai://debug_event",
                                serde_json::json!({
                                    "kind": "retrieval_error",
                                    "msg": format!("automatic retrieval failed: {error:#}"),
                                    "requestId": request_id,
                                }),
                            );
                            turn_instructions.push_str(
                                "\n\nEVIDENCE POLICY: Automatic retrieval failed for this turn. Do not infer or claim that a term or fact is absent. Say the available evidence is insufficient for a definitive answer.",
                            );
                        }
                    };
                }
            }
            if append_only && !has_selection {
                turn_instructions.push_str(
                    "\n\nAPPEND-ONLY TURN: The current note (if shown) is reference material only. \
                     Add ONLY the new requested text; never reproduce, quote, or regenerate any \
                     existing line. Call append_note with only the new paragraph in content."
                );
            }
            if doc_type == "tex" {
                turn_instructions.push_str(
                    "\n\nIMPORTANT: This open document is a LaTeX (.tex) source file, NOT Markdown. \
                     Write and edit it using LaTeX syntax only — e.g. \\section{...}, \\subsection{...}, \
                     \\textbf{...}, \\emph{...}, \\begin{itemize}\\item ...\\end{itemize}, $...$ for math, \
                     \\begin{equation}...\\end{equation}. Do NOT use Markdown (#, **, -). Preserve the \
                     document's preamble and \\begin{document}/\\end{document} structure.",
                );
            }
            if let Some(sel) = &selection {
                if interaction_mode == "chat" {
                    if !sel.cursor {
                        let cell_context = sel
                            .cell_index
                            .map(|index| format!(" in notebook cell {index}"))
                            .unwrap_or_default();
                        turn_instructions.push_str(&format!(
                            "\n\nThe user selected this excerpt{cell_context} in the open note. \
                             Use it as the primary context for the latest question:\n\"\"\"\n{}\n\"\"\"",
                            sel.text
                        ));
                    }
                } else if doc_type == "ipynb" {
                    let cell_index = sel
                        .cell_index
                        .ok_or_else(|| anyhow!("The notebook target is missing its cell index."))?;
                    if sel.cursor {
                        turn_instructions.push_str(&format!(
                            "\n\nThe editor supplied an exact CURSOR target inside notebook cell {cell_index}. \
                             For text insertion, call edit_notebook with operation \"edit\", index {cell_index}, \
                             and ONLY the new text to insert as content. Do not reproduce the existing cell."
                        ));
                    } else {
                        turn_instructions.push_str(&format!(
                            "\n\nThe user SELECTED this exact source span inside notebook cell {cell_index}:\n\
                             \"\"\"\n{}\n\"\"\"\n\
                             For a text edit, call edit_notebook with operation \"edit\", index {cell_index}, \
                             and ONLY the replacement fragment as content. Empty content removes the selection. \
                             Do not reproduce the rest of the cell.",
                            sel.text
                        ));
                    }
                    turn_instructions.push_str(&format!(
                        " Cell deletion must target index {cell_index}. A new cell may be inserted only at index \
                         {cell_index} or {}.",
                        cell_index + 1
                    ));
                } else if sel.cursor {
                    turn_instructions.push_str(
                        "\n\nThe editor supplied an exact CURSOR target. Generate only the new text to insert there and call write_note with that text as content. Never reproduce existing note text.",
                    );
                } else {
                    turn_instructions.push_str(&format!(
                        "\n\nThe user has SELECTED the following part of the note. This request applies to the SELECTION ONLY — leave every character outside it unchanged:\n\"\"\"\n{}\n\"\"\"\n\nUse write_note for this selection and send ONLY replacement content. For removal, send an empty `content`. Do not reproduce the whole note or call a mutation tool that targets text outside this selection.",
                        sel.text
                    ));
                    if placement {
                        turn_instructions.push_str(" For a below/after/under/beneath request, use insert_after_line with marker set to the selected text and insert immediately after its line.");
                    }
                }
            }
            let recent_user_msgs: Vec<&str> = note
                .chat_history
                .iter()
                .filter(|m| m.role == "user" && m.error != Some(true))
                .map(|m| m.content.as_str())
                .collect();
            let edit_thread = crate::agent::in_edit_thread(&recent_user_msgs);
            let supports_tools = if external_model {
                true
            } else {
                let model_id = config.model_path.to_string_lossy().to_string();
                crate::tool_capability::supports_tools(
                    &self.inner.llama_client,
                    &config.base_url(),
                    &self.inner.app_data_dir,
                    &config.model_path,
                    &model_id,
                    config.supports_tools,
                )
                .await
            };
            let _ = self.handle.emit(
                "ai://debug_event",
                serde_json::json!({
                    "kind": "setup",
                    "msg": format!("tool capability ready; total {} ms", setup_started.elapsed().as_millis()),
                    "requestId": request_id,
                }),
            );
            self.set_turn_tool_policy(
                interaction_mode == "chat",
                append_only && !has_selection,
                placement && has_selection && interaction_mode != "write",
                prompt_shape.oversized,
                supports_tools,
            );
            let nid = self.current_note_id().unwrap_or_default();
            let mut convo = if isolated_edit { Vec::new() } else { self.conversation(&nid) };
            if convo.is_empty() && !isolated_edit {
                convo = chat_history_to_messages(&note.chat_history);
            }
            let mode_instruction = match interaction_mode {
                "chat" => "CHAT TURN POLICY: The open note identified above is the default target for every read or question. Answer questions about it directly; do not call search_notes or read_note unless the user explicitly asks about another note, their notes/workspace, or names a different note. Use web/document retrieval only when explicitly requested. Never modify the note for this turn.",
                "write" => "WRITE TURN POLICY: Perform exactly the requested edit on the armed cursor or selection using the single targeted write tool. Do not target any other location or call an unrelated tool.",
                "operation" => "OPERATION TURN POLICY: Perform the user's requested operation using the appropriate tool. The open note identified above is the default target.",
                "edit" => "EDITOR ACTION: Perform exactly this isolated edit with write_note. Return only the replacement or insertion content in the tool call; never reproduce text outside the target.",
                _ => "AUTO TURN POLICY: Decide whether the user needs a direct answer or an operation.",
            };
            if !external_model {
                self.finish_prompt_warmup().await;
            }
            let _slot_guard = self.inner.llama_slot_lock.lock().await;
            let fast_document_answer = interaction_mode == "chat"
                && (retrieval_evidence_ready || active_section_request)
                && !crate::agent::wants_other_notes(&model_question)
                && !crate::agent::wants_fetch(&model_question)
                && !crate::agent::wants_search(&model_question);
            let compact_write_conversation = interaction_mode == "write";
            let turn_conversation = if fast_document_answer || compact_write_conversation {
                crate::ai_turn::compact_document_conversation(&model_question, &convo)
            } else {
                convo.clone()
            };
            let turn = crate::ai_turn::AiTurnBuilder::build(crate::ai_turn::AiTurnInput {
                mode: interaction_mode,
                doc_type: &doc_type,
                note_title: &note.title,
                system_context: &stable_context,
                conversation: &turn_conversation,
                question: &model_question,
                mode_policy: mode_instruction,
                turn_instructions: &turn_instructions,
                has_open_note: true,
                edit_thread,
                oversized: retrieval_backed,
                supports_tools: supports_tools && !fast_document_answer,
                verbose_tool_schemas: config.verbose_tool_schemas,
                section_context: active_section_request,
            });
            let intent_is_tool = Some(turn.intent_is_tool);
            let user_content = turn
                .messages
                .last()
                .and_then(|message| message["content"].as_str())
                .unwrap_or(&question)
                .to_string();
            let messages = turn.messages;
            let tools = if fast_document_answer {
                Vec::new()
            } else {
                turn.tools
            };
            let template_kwargs = self.openharn_settings().template_kwargs;
            if !external_model {
                if let Some(section) = active_section
                .as_ref()
                .filter(|_| active_section_request)
            {
                let system = messages
                    .first()
                    .and_then(|message| message["content"].as_str())
                    .unwrap_or_default();
                let cache_prepare_started = std::time::Instant::now();
                let _ = self.handle.emit(
                    "ai://debug_event",
                    serde_json::json!({
                        "kind": "cache_prepare",
                        "msg": format!("profile={} section={} start", interaction_mode, section.key),
                        "requestId": request_id,
                    }),
                );
                self.prepare_section_slot(
                    &config,
                    &nid,
                    section,
                    system,
                    &template_kwargs,
                    interaction_mode,
                )
                .await;
                let _ = self.handle.emit(
                    "ai://debug_event",
                    serde_json::json!({
                        "kind": "cache_prepare",
                        "msg": format!(
                            "profile={} section={} done in {} ms",
                            interaction_mode,
                            section.key,
                            cache_prepare_started.elapsed().as_millis()
                        ),
                        "requestId": request_id,
                    }),
                );
                }
            } else {
                *self.inner.active_slot_cache.lock() = None;
            }
            let tool_names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            log::info!(
                "[ask_ai_stream] mode={} tools_offered={} gating={} deterministic={} edit_thread={} supports_tools={}",
                interaction_mode, tool_names.join(","), config.tool_gating && interaction_mode != "operation", deterministic_tools, edit_thread, supports_tools
            );
            let tool_mode = self.openharn_settings().tool_mode;
            let _ = self.handle.emit(
                "ai://debug_event",
                serde_json::json!({
                    "kind": "config",
                    "msg": format!(
                        "mode={}, tools: {}, gate={}, determ={}, edit={}, tools_supported={}, retrieval_evidence={}, fast_document={}, tool_intent={}, tool_mode={}",
                        interaction_mode,
                        tool_names.join(", "),
                        config.tool_gating && interaction_mode != "operation",
                        deterministic_tools,
                        edit_thread,
                        supports_tools,
                        retrieval_evidence_ready,
                        fast_document_answer,
                        turn.intent_is_tool,
                        tool_mode,
                    ),
                    "requestId": request_id,
                }),
            );
            let final_messages = crate::sidecar::run_chat(
                self,
                &config,
                messages,
                tools,
                &request_id,
                &nid,
                intent_is_tool,
                interaction_mode == "chat",
                matches!(interaction_mode, "write" | "operation") || isolated_edit,
                selection.is_some(),
                interaction_mode == "write",
            )
            .await?;
            if !isolated_edit {
                convo.push(serde_json::json!({
                    "role": "user",
                    "content": user_content
                }));
                convo.extend(final_messages.iter().cloned());
                let trimmed = trim_conversation(convo, MAX_LIVE_CONVERSATION_CHARS);
                if let Err(error) = self.save_conversation(&nid, trimmed) {
                    log::warn!("chat response retained in memory but conversation persistence failed: {error}");
                }
            }
            if !external_model && turn_contains_note_mutation(&final_messages) {
                let state = self.clone();
                let note_id = nid.clone();
                let warm_mode = interaction_mode.to_string();
                tokio::spawn(async move {
                    if let Err(error) = state
                        .warm_llama_server_for_note(Some(note_id), Some(warm_mode), None)
                        .await
                    {
                        log::debug!("post-write prompt-cache warm-up skipped: {error}");
                    }
                });
            } else if !external_model && config.prompt_cache && active_section.is_none() {
                let (system, tools) = warmup_prefix(
                    &note.title,
                    &note_body_excerpt,
                    notebook_cells.as_deref(),
                    interaction_mode,
                    &doc_type,
                    supports_tools,
                    retrieval_backed,
                    config.verbose_tool_schemas,
                );
                let template_kwargs = self.openharn_settings().template_kwargs;
                let identity = Self::slot_identity(
                    &config,
                    ctx_tokens as u32,
                    interaction_mode,
                    &system,
                    &serde_json::to_string(&tools).unwrap_or_default(),
                    &template_kwargs,
                );
                self.save_note_slot(&nid, &identity).await;
            }
            Ok(())
        }
        .await;
        self.resume_section_cache_after_turn();
        self.clear_latest_chat_question();
        self.clear_current_note_id();
        drop(_chat_guard);
        match result {
            Ok(()) => {
                let tools = self.take_chat_tools();
                self.handle.emit(
                    "ai://chat_done",
                    serde_json::json!({
                        "requestId": request_id,
                        "tools": tools
                    }),
                )?;
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                let tools = self.take_chat_tools();
                log::error!("AI chat failed: {message}");
                let _ = self.handle.emit(
                    "ai://chat_error",
                    serde_json::json!({
                        "requestId": request_id,
                        "message": message,
                        "tools": tools
                    }),
                );
                Err(error)
            }
        }
    }
}
