use super::core::*;
use super::workspace_search::{workspace_chunks, WorkspaceSearchHit};
use crate::persistence::{FileMutation, FileTransaction, MutationRoot};
use ::anyhow::{anyhow, Context, Result};

impl AppState {
    pub(crate) fn ensure_unique_title(
        &self,
        requested_title: &str,
        current_note_id: Option<&str>,
    ) -> String {
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

        let _persistence_guard = self.inner.persistence_lock.lock();
        write_note_file(
            &workspace,
            &self.workspace_data_dir(&workspace),
            &path,
            &document,
        )?;
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
        if let Err(error) = crate::git_history::commit_changes(
            &workspace,
            &format!("Create note: {}", document.title),
        ) {
            log::warn!("created note but could not create Git history entry: {error}");
        }

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
            let slot = self
                .inner
                .app_data_dir
                .join("llama-cache")
                .join("slots")
                .join(Self::slot_filename(&note_id));
            let _ = fs::remove_file(&slot);
            let _ = fs::remove_file(slot.with_file_name(format!(
                "{}.json",
                slot.file_name().unwrap_or_default().to_string_lossy()
            )));
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

        let _persistence_guard = self.inner.persistence_lock.lock();
        write_note_file(
            &workspace,
            &self.workspace_data_dir(&workspace),
            &path,
            &updated,
        )?;
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
                if let Err(error) = ingest_state
                    .ensure_oversized_note_ingested(&ingest_note)
                    .await
                {
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

        let data_dir = self.workspace_data_dir(&workspace);
        {
            let _persistence_guard = self.inner.persistence_lock.lock();
            FileTransaction::new(
                &workspace,
                &data_dir,
                note_delete_plan(&workspace, &data_dir, &path, &note_id)?,
            )?
            .commit()?;
        }
        self.inner.runtime.write().notes.remove(&note_id);
        // Drop any RAG chunks ingested for this note from the document store.
        let _ = self.delete_document(&note_id).await;
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let mut ingestion = self.load_note_ingestion_manifest();
            if ingestion.entries.remove(&note_id).is_some() {
                let _ = self.save_note_ingestion_manifest(&ingestion);
            }
        }

        if let Err(error) =
            crate::git_history::commit_changes(&workspace, &format!("Delete note: {}", note_id))
        {
            log::warn!("deleted note but could not create Git history entry: {error}");
        }
        if let Err(error) = self.reindex_workspace_after_change(workspace).await {
            log::warn!("deleted note but reindexing failed: {error}");
        }
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

        {
            let _persistence_guard = self.inner.persistence_lock.lock();
            write_note_file(
                &workspace,
                &self.workspace_data_dir(&workspace),
                &path,
                &document,
            )?;
        }
        if let Err(error) = crate::git_history::commit_changes(
            &workspace,
            &format!("Duplicate note: {}", document.title),
        ) {
            log::warn!("duplicated note but could not create Git history entry: {error}");
        }
        {
            let mut runtime = self.inner.runtime.write();
            runtime.notes.insert(
                duplicate_id.clone(),
                IndexedNote {
                    document: document.clone(),
                    vector: source.vector.clone(),
                },
            );
        }
        if let Err(error) = self.reindex_workspace_after_change(workspace).await {
            log::warn!("duplicated note but reindexing failed: {error}");
        }
        Ok(document)
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
        let target_path = unique_note_path(&target_base, file_name);
        let data_dir = self.workspace_data_dir(&workspace);
        let mut mutations = vec![FileMutation::Move {
            root: MutationRoot::Workspace,
            from: PathBuf::from(relative_to_workspace(&workspace, &source_path)),
            to: PathBuf::from(relative_to_workspace(&workspace, &target_path)),
        }];
        if is_metadata_document(&source_path) {
            // The sidecar key is derived from the relative path. Prepare the new
            // key before moving the source, then remove it again if the file move
            // fails so the two operations cannot leave a phantom note metadata
            // record behind.
            let metadata_path = native_metadata_app_path(&workspace, &data_dir, &target_path);
            let sidecar = NativeMetadataSidecar {
                schema_version: 2,
                metadata: frontmatter_from_document(&source.document),
                relative_path: relative_to_workspace(&workspace, &target_path),
            };
            mutations.push(FileMutation::Write {
                root: MutationRoot::WorkspaceData,
                relative_path: metadata_path.strip_prefix(&data_dir).map(PathBuf::from)?,
                bytes: serde_json::to_vec_pretty(&sidecar)?,
            });
            mutations.push(FileMutation::Delete {
                root: MutationRoot::WorkspaceData,
                relative_path: native_metadata_app_path(&workspace, &data_dir, &source_path)
                    .strip_prefix(&data_dir)
                    .map(PathBuf::from)?,
            });
        }
        {
            let _persistence_guard = self.inner.persistence_lock.lock();
            FileTransaction::new(&workspace, &data_dir, mutations)?.commit()?;
        }
        if let Err(error) = crate::git_history::commit_changes(
            &workspace,
            &format!("Move note: {}", source.document.title),
        ) {
            log::warn!("moved note but could not create Git history entry: {error}");
        }

        let moved = NoteDocument {
            relative_path: relative_to_workspace(&workspace, &target_path),
            ..source.document.clone()
        };
        {
            let mut runtime = self.inner.runtime.write();
            runtime.notes.insert(
                note_id.clone(),
                IndexedNote {
                    document: moved.clone(),
                    vector: source.vector.clone(),
                },
            );
        }

        if let Err(error) = self.reindex_workspace_after_change(workspace).await {
            log::warn!("moved note but reindexing failed: {error}");
        }
        Ok(moved)
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
                        match_excerpt: None,
                        matched_section: None,
                    })
                    .collect(),
            });
        }

        let notes = {
            let runtime = self.inner.runtime.read();
            runtime.notes.clone()
        };
        let query_vector = self.note_embedding(trimmed, true).await;
        let keyword_terms = tokenize(trimmed);
        let chunks = workspace_chunks(self).await?;
        let mut per_note: HashMap<String, Vec<(f32, WorkspaceSearchHit, bool, bool)>> =
            HashMap::new();
        for chunk in chunks {
            let haystack = format!(
                "{}\n{}",
                chunk.title.to_lowercase(),
                chunk.text.to_lowercase()
            );
            let lexical = keyword_terms
                .iter()
                .map(|term| haystack.matches(term).count() as f32)
                .sum::<f32>();
            let semantic = chunk
                .vector
                .as_ref()
                .filter(|v| {
                    chunk.embedding_fingerprint == self.embedding_fingerprint()
                        && v.len() == query_vector.len()
                        && !query_vector.is_empty()
                })
                .map(|v| cosine_similarity(&query_vector, v));
            let score = match semantic {
                Some(value) => 0.70 * (lexical / (lexical + 1.0)) + 0.30 * value,
                None => lexical / (lexical + 1.0),
            };
            if score > 0.0 {
                per_note.entry(chunk.note_id.clone()).or_default().push((
                    score,
                    chunk,
                    lexical > 0.0,
                    semantic.is_some(),
                ));
            }
        }
        let mut results = per_note
            .into_iter()
            .filter_map(|(note_id, mut hits)| {
                let note = notes.get(&note_id)?;
                hits.sort_by(|a, b| b.0.total_cmp(&a.0));
                let (best_score, best, lexical, semantic) = hits.remove(0);
                let second = hits
                    .into_iter()
                    .find(|(_, candidate, _, _)| {
                        match (
                            best.char_start,
                            best.char_end,
                            candidate.char_start,
                            candidate.char_end,
                        ) {
                            (Some(a), Some(b), Some(c), Some(d)) => {
                                ((b.min(d) - a.max(c)).max(0) as f32) / ((d - c).max(1) as f32)
                                    < 0.60
                            }
                            _ => true,
                        }
                    })
                    .map(|hit| hit.0)
                    .unwrap_or(0.0);
                let title_boost = keyword_terms
                    .iter()
                    .filter(|term| {
                        note.document.title.to_lowercase().contains(term.as_str())
                            || note
                                .document
                                .tags
                                .iter()
                                .any(|tag| tag.to_lowercase().contains(term.as_str()))
                    })
                    .count() as f32
                    / keyword_terms.len().max(1) as f32;
                let score = 0.75 * best_score + 0.15 * second + 0.10 * title_boost;
                Some(SearchResult {
                    note: summarize(&note.document),
                    score,
                    reason: if semantic && lexical {
                        "hybrid".into()
                    } else if semantic {
                        "vector".into()
                    } else {
                        "keyword".into()
                    },
                    match_excerpt: Some(excerpt(&best.text)),
                    matched_section: best.section,
                })
            })
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
