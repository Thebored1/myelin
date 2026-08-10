use super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub(crate) fn ensure_unique_title(&self, requested_title: &str, current_note_id: Option<&str>) -> String {
        let runtime = self.inner.runtime.read();

        let base_title = if requested_title.trim().is_empty() {
            "Untitled note"
        } else {
            requested_title.trim()
        };

        let mut title = base_title.to_string();
        let mut counter = 1;

        loop {
            let exists = runtime.notes.iter().any(|(id, note)| {
                note.document.title.to_lowercase() == title.to_lowercase()
                    && Some(id.as_str()) != current_note_id
            });

            if !exists {
                return title;
            }

            title = format!("{} {}", base_title, counter);
            counter += 1;
        }
    }

    pub async fn create_note(
        &self,
        title: String,
        source_pdf: Option<String>,
        extension: Option<String>,
        notebook: Option<String>,
    ) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let now = timestamp_now();
        let id = Uuid::new_v4().to_string();

        let unique_title = self.ensure_unique_title(&title, None);
        let safe_slug = slugify(&unique_title);
        let ext = extension.unwrap_or_else(|| "md".to_string());
        let file_name = format!("{safe_slug}--{}.{ext}", &id[..8]);
        // When a notebook (folder) is given, create the note inside it.
        let target_dir = match notebook {
            Some(name) if !name.trim().is_empty() && !name.trim().eq_ignore_ascii_case("root") => {
                let safe = sanitize_relative_folder(&name)?;
                let dir = workspace.join(folder_to_relative_path(&safe));
                fs::create_dir_all(&dir)
                    .with_context(|| format!("failed to open notebook {}", dir.display()))?;
                dir
            }
            _ => workspace.clone(),
        };
        let path = unique_note_path(&target_dir, &file_name);
        let relative_path = relative_to_workspace(&workspace, &path);
        // A newly-created notebook must be a valid nbformat document from its
        // first byte on disk. The editor treats the same shape as an empty
        // notebook, and subsequent saves preserve its raw JSON representation.
        let body = initial_note_body(&path);

        let document = NoteDocument {
            id,
            title: unique_title,
            tags: Vec::new(),
            body,
            relative_path,
            created_at: now.clone(),
            updated_at: now,
            source_pdf,
            annotations: Vec::new(),
            backlinks: Vec::new(),
            chat_history: Vec::new(),
        };

        let vector = self
            .note_embedding(
                &format!(
                    "{}\n{}\n{}",
                    document.title,
                    document.tags.join(" "),
                    document.body
                ),
                false,
            )
            .await;

        {
            let mut runtime = self.inner.runtime.write();
            runtime.notes.insert(
                document.id.clone(),
                IndexedNote {
                    document: document.clone(),
                    vector,
                },
            );
        }

        write_note_file(
            &workspace,
            &self.workspace_data_dir(&workspace),
            &path,
            &document,
        )?;
        crate::git_history::commit_changes(
            &workspace,
            &format!("Create note: {}", document.title),
        )?;

        // The watcher will usually report this same write. Both paths only mark
        // the shared scheduler dirty, so they collapse into one debounced pass.
        let _ = self.request_reindex(workspace.clone(), true);

        Ok(document)
    }

    /// Create a notebook — a top-level folder in the workspace that holds notes
    /// of any kind. Returns the updated list of notebooks.
    pub fn create_notebook(&self, name: String) -> Result<Vec<String>> {
        let workspace = self.require_workspace()?;
        let safe = sanitize_relative_folder(&name)?;
        if safe == "Root" {
            return Err(anyhow!("notebook name cannot be empty"));
        }
        let dir = workspace.join(folder_to_relative_path(&safe));
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create notebook {}", dir.display()))?;
        self.list_notebooks()
    }

    /// List notebooks: the top-level folders in the workspace (filesystem is the
    /// source of truth), excluding hidden/ignored dirs. Includes empty ones.
    pub fn list_notebooks(&self) -> Result<Vec<String>> {
        let workspace = self.require_workspace()?;
        let mut names = Vec::new();
        if let Ok(entries) = fs::read_dir(&workspace) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.')
                    || name == "node_modules"
                    || name == "target"
                    || name == "dist"
                    || name == "build"
                    || name == "tasks"
                {
                    continue;
                }
                names.push(name);
            }
        }
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        Ok(names)
    }

    pub async fn load_note(&self, note_id: String) -> Result<NoteDocument> {
        let runtime = self.inner.runtime.read();
        runtime
            .notes
            .get(&note_id)
            .map(|note| note.document.clone())
            .ok_or_else(|| anyhow!("note not found"))
    }

    pub fn find_note_by_exact_title(&self, title: &str) -> Option<NoteDocument> {
        let normalized = title.trim().to_lowercase();
        if normalized.is_empty() {
            return None;
        }

        let runtime = self.inner.runtime.read();
        runtime
            .notes
            .values()
            .find(|note| note.document.title.trim().to_lowercase() == normalized)
            .map(|note| note.document.clone())
    }

    pub async fn save_note(
        &self,
        note_id: String,
        title: String,
        tags: Vec<String>,
        body: String,
        source_pdf: Option<String>,
        annotations: Option<Vec<crate::models::PdfAnnotation>>,
    ) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let existing = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .cloned()
                .ok_or_else(|| anyhow!("note not found"))?
        };
        let path = workspace.join(&existing.document.relative_path);
        validate_native_body(&path, &body)?;
        let prompt_changed = existing.document.body != body
            || existing.document.title != title
            || existing.document.source_pdf != source_pdf;

        let unique_title = self.ensure_unique_title(&title, Some(&note_id));

        let updated = NoteDocument {
            id: existing.document.id,
            title: unique_title,
            tags: tags
                .into_iter()
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect(),
            body,
            relative_path: existing.document.relative_path.clone(),
            created_at: existing.document.created_at,
            updated_at: timestamp_now(),
            source_pdf,
            annotations: annotations.unwrap_or_default(),
            backlinks: existing.document.backlinks,
            chat_history: existing.document.chat_history,
        };

        if prompt_changed {
            let slot = self.inner.app_data_dir.join("llama-cache").join("slots")
                .join(Self::slot_filename(&note_id));
            let _ = fs::remove_file(&slot);
            let _ = fs::remove_file(slot.with_file_name(format!("{}.json", slot.file_name().unwrap_or_default().to_string_lossy())));
        }

        let vector = self
            .note_embedding(
                &format!(
                    "{}\n{}\n{}",
                    updated.title,
                    updated.tags.join(" "),
                    updated.body
                ),
                false,
            )
            .await;

        {
            let mut runtime = self.inner.runtime.write();
            runtime.notes.insert(
                note_id.clone(),
                IndexedNote {
                    document: updated.clone(),
                    vector,
                },
            );
        }

        write_note_file(
            &workspace,
            &self.workspace_data_dir(&workspace),
            &path,
            &updated,
        )?;
        // Version history is useful, but it must never make a successfully
        // persisted Markdown edit look like a failed note write. A workspace may
        // have an inaccessible Git index or an unsupported file type; keep the
        // note and report history failure only in the application log.
        if let Err(error) = crate::git_history::commit_changes(
            &workspace,
            &format!("Update note: {}", updated.title),
        ) {
            log::warn!("saved note but could not create Git history entry: {error}");
        }

        // Explicitly mark the index dirty in case the platform watcher drops or
        // coalesces its event. The scheduler merges the duplicate watcher signal.
        let _ = self.request_reindex(workspace.clone(), true);
        let ingest_state = self.clone();
        let ingest_note = updated.clone();
        tauri::async_runtime::spawn(async move {
            let known = ingest_state.note_has_ingestion_entry(&ingest_note.id);
            let ctx = ingest_state
                .running_ctx_size()
                .await
                .or_else(|| {
                    crate::llama_server::resolve_config(&ingest_state.inner.app_data_dir)
                        .ok()
                        .map(|config| config.context_size)
                })
                .unwrap_or(4096) as usize;
            let oversized = crate::note_prompt::NotePromptShape::build(
                &ingest_note.body,
                &ingest_note.relative_path,
                ctx,
            )
            .oversized;
            if known || oversized {
                if let Err(error) = ingest_state.ensure_oversized_note_ingested(&ingest_note).await {
                    log::warn!("background oversized-note ingestion failed: {error}");
                }
            }
        });

        Ok(updated)
    }

    pub async fn delete_note(&self, note_id: String) -> Result<AppSnapshot> {
        let workspace = self.require_workspace()?;
        let slot_dir = self.inner.app_data_dir.join("llama-cache").join("slots");
        let slot_name = Self::slot_filename(&note_id);
        let _ = fs::remove_file(slot_dir.join(&slot_name));
        let _ = fs::remove_file(slot_dir.join(format!("{slot_name}.json")));
        let path = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .map(|note| workspace.join(&note.document.relative_path))
                .ok_or_else(|| anyhow!("note not found"))?
        };

        fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;

        // Delete the note's sidecars too — the chat session and annotations are
        // keyed by note id and would otherwise orphan in the workspace data dir
        // (stale sessions lingering after the note is gone).
        let data_dir = self.workspace_data_dir(&workspace);
        remove_native_metadata_sidecar(&workspace, &data_dir, &path);
        let _ = fs::remove_file(data_dir.join("chats").join(format!("{note_id}.chat.json")));
        let _ = fs::remove_file(data_dir.join("chats").join(format!("{note_id}.chat.tmp")));
        let _ = fs::remove_file(
            data_dir
                .join("annotations")
                .join(format!("{note_id}.annotations.json")),
        );
        // Drop any RAG chunks ingested for this note from the document store.
        let _ = self.delete_document(&note_id).await;
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let mut ingestion = self.load_note_ingestion_manifest();
            if ingestion.entries.remove(&note_id).is_some() {
                let _ = self.save_note_ingestion_manifest(&ingestion);
            }
        }

        crate::git_history::commit_changes(&workspace, &format!("Delete note: {}", note_id))?;
        self.reindex_workspace_after_change(workspace).await?;
        Ok(self.snapshot())
    }

    pub async fn duplicate_note(&self, note_id: String) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let source = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .cloned()
                .ok_or_else(|| anyhow!("note not found"))?
        };

        let now = timestamp_now();
        let duplicate_id = Uuid::new_v4().to_string();
        let duplicate_title = format!("{} Copy", source.document.title);
        let source_path = workspace.join(&source.document.relative_path);
        let duplicate_extension = duplicate_note_extension(&source_path);
        let file_name = format!(
            "{}--{}.{}",
            slugify(&duplicate_title),
            &duplicate_id[..8],
            duplicate_extension
        );
        let path = unique_note_path(
            &workspace.join(folder_to_relative_path(&folder_from_relative_path(
                &source.document.relative_path,
            ))),
            &file_name,
        );
        let document = NoteDocument {
            id: duplicate_id.clone(),
            title: duplicate_title,
            tags: source.document.tags.clone(),
            body: source.document.body.clone(),
            relative_path: relative_to_workspace(&workspace, &path),
            created_at: now.clone(),
            updated_at: now,
            source_pdf: source.document.source_pdf.clone(),
            annotations: source.document.annotations.clone(),
            backlinks: source.document.backlinks,
            chat_history: source.document.chat_history.clone(),
        };

        write_note_file(
            &workspace,
            &self.workspace_data_dir(&workspace),
            &path,
            &document,
        )?;
        crate::git_history::commit_changes(
            &workspace,
            &format!("Duplicate note: {}", document.title),
        )?;
        self.reindex_workspace_after_change(workspace).await?;
        self.load_note(duplicate_id).await
    }

    pub async fn move_note(&self, note_id: String, target_folder: String) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let source = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .cloned()
                .ok_or_else(|| anyhow!("note not found"))?
        };

        let target_folder = sanitize_relative_folder(&target_folder)?;
        let source_path = workspace.join(&source.document.relative_path);
        let file_name = source_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| anyhow!("invalid note filename"))?;
        let target_base = workspace.join(folder_to_relative_path(&target_folder));
        fs::create_dir_all(&target_base)
            .with_context(|| format!("failed to create target folder {}", target_base.display()))?;
        let target_path = unique_note_path(&target_base, file_name);
        let data_dir = self.workspace_data_dir(&workspace);
        if is_native_text_file(&source_path) {
            // The sidecar key is derived from the relative path. Prepare the new
            // key before moving the source, then remove it again if the file move
            // fails so the two operations cannot leave a phantom note metadata
            // record behind.
            write_native_metadata_sidecar(
                &workspace,
                &data_dir,
                &target_path,
                &source.document,
            )?;
        }
        if let Err(error) = fs::rename(&source_path, &target_path) {
            remove_native_metadata_sidecar(&workspace, &data_dir, &target_path);
            return Err(error).with_context(|| {
                format!(
                    "failed to move {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            });
        }
        remove_native_metadata_sidecar(&workspace, &data_dir, &source_path);
        crate::git_history::commit_changes(
            &workspace,
            &format!("Move note: {}", source.document.title),
        )?;

        self.reindex_workspace_after_change(workspace).await?;
        self.load_note(note_id).await
    }

    pub async fn reorder_note(&self, note_id: String, direction: String) -> Result<AppSnapshot> {
        self.require_workspace()?;
        let normalized_direction = direction.trim().to_lowercase();
        if normalized_direction != "up" && normalized_direction != "down" {
            return Err(anyhow!("direction must be 'up' or 'down'"));
        }

        {
            let mut runtime = self.inner.runtime.write();
            let ordered_ids = normalized_custom_order(&runtime.custom_note_order, &runtime.notes);
            let Some(index) = ordered_ids.iter().position(|id| id == &note_id) else {
                return Err(anyhow!("note not found"));
            };
            let swap_index = if normalized_direction == "up" {
                index.checked_sub(1)
            } else if index + 1 < ordered_ids.len() {
                Some(index + 1)
            } else {
                None
            };

            if let Some(swap_index) = swap_index {
                let mut reordered = ordered_ids;
                reordered.swap(index, swap_index);
                runtime.custom_note_order = reordered;
            }
        }

        self.persist_runtime_settings()?;
        Ok(self.snapshot())
    }

    pub async fn search_notes(&self, query: String) -> Result<SearchResponse> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(SearchResponse {
                query,
                results: self
                    .note_summaries()
                    .into_iter()
                    .take(20)
                    .map(|note| SearchResult {
                        note,
                        score: 0.0,
                        reason: "recent".into(),
                    })
                    .collect(),
            });
        }

        let notes = {
            let runtime = self.inner.runtime.read();
            runtime.notes.values().cloned().collect::<Vec<_>>()
        };

        let query_vector = self.note_embedding(trimmed, true).await;
        let keyword_terms = tokenize(trimmed);
        let mut results = notes
            .into_iter()
            .map(|note| {
                let haystack = format!(
                    "{}\n{}\n{}",
                    note.document.title.to_lowercase(),
                    note.document.tags.join(" ").to_lowercase(),
                    note.document.body.to_lowercase()
                );
                let keyword_score = keyword_terms
                    .iter()
                    .map(|term| haystack.matches(term).count() as f32)
                    .sum::<f32>();
                let vector_score = cosine_similarity(&query_vector, &note.vector);
                let score = keyword_score * 0.7 + vector_score * 0.3;
                let reason = if keyword_score > 0.0 && vector_score > 0.0 {
                    "keyword + vector".into()
                } else if keyword_score > 0.0 {
                    "keyword".into()
                } else {
                    "vector".into()
                };

                SearchResult {
                    note: summarize(&note.document),
                    score,
                    reason,
                }
            })
            .filter(|result| result.score > 0.25)
            .collect::<Vec<_>>();

        results.sort_by(|left, right| right.score.total_cmp(&left.score));

        Ok(SearchResponse {
            query,
            results: results.into_iter().take(20).collect(),
        })
    }
    pub async fn read_pdf_binary(&self, note_id: String) -> Result<Vec<u8>> {
        let workspace = self.require_workspace()?;
        let path = {
            let runtime = self.inner.runtime.read();
            let note = runtime
                .notes
                .get(&note_id)
                .ok_or_else(|| anyhow!("note not found"))?;
            workspace.join(&note.document.relative_path)
        };
        fs::read(path).map_err(|e| anyhow!("failed to read PDF: {}", e))
    }

    pub async fn get_note_history(
        &self,
        note_id: String,
    ) -> Result<Vec<crate::git_history::GitCommit>> {
        let workspace = self.require_workspace()?;
        let path = {
            let runtime = self.inner.runtime.read();
            let note = runtime
                .notes
                .get(&note_id)
                .ok_or_else(|| anyhow!("note not found"))?;
            workspace.join(&note.document.relative_path)
        };
        let path_str = path.to_str().unwrap();
        let history = crate::git_history::get_file_history(&workspace, path_str)?;

        let mut filtered = Vec::new();
        for commit in history {
            if let Ok(content) =
                crate::git_history::get_file_at_commit(&workspace, &commit.hash, path_str)
            {
                let mut body = content.as_str();
                if body.starts_with("---\n") {
                    if let Some(end_idx) = body[4..].find("\n---\n") {
                        body = &body[end_idx + 9..];
                    }
                }
                if !body.trim().is_empty() {
                    filtered.push(commit);
                }
            }
        }

        Ok(filtered)
    }

    pub async fn get_note_version(&self, note_id: String, commit_hash: String) -> Result<String> {
        let workspace = self.require_workspace()?;
        let path = {
            let runtime = self.inner.runtime.read();
            let note = runtime
                .notes
                .get(&note_id)
                .ok_or_else(|| anyhow!("note not found"))?;
            workspace.join(&note.document.relative_path)
        };
        crate::git_history::get_file_at_commit(&workspace, &commit_hash, path.to_str().unwrap())
    }
    pub fn get_all_note_documents(&self) -> Vec<NoteDocument> {
        let runtime = self.inner.runtime.read();
        runtime.notes.values().map(|n| n.document.clone()).collect()
    }

    pub(crate) fn note_summaries(&self) -> Vec<NoteSummary> {
        let runtime = self.inner.runtime.read();
        let notes = runtime
            .notes
            .values()
            .map(|note| summarize(&note.document))
            .collect::<Vec<_>>();
        sort_summaries_by_custom_order(
            notes,
            &normalized_custom_order(&runtime.custom_note_order, &runtime.notes),
        )
    }

    // ── Tasks ──
    // Each task is a self-contained JSON file (file-per-item: portable, separately
    // copyable, Drive-syncable). Default location is `<workspace>/tasks/<id>.json`;
    // a task assigned to a notebook lives at `<workspace>/<notebook>/tasks/<id>.json`.
    // The note indexer ignores them (not a note extension).

}
