use super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub(crate) fn request_reindex(
        &self,
        workspace: PathBuf,
        debounce: bool,
    ) -> Option<IndexRequestReceipt> {
        let runtime = self.inner.runtime.read();
        if runtime.workspace_path.as_ref() != Some(&workspace) {
            return None;
        }
        let receipt = self
            .inner
            .index_scheduler
            .lock()
            .request(workspace, debounce);
        drop(runtime);

        if receipt.spawn_worker {
            let worker_id = receipt
                .worker_id
                .expect("a spawned index worker must have an id");
            let state = self.clone();
            // Construct the guard before handing the future to the runtime. If
            // shutdown drops an unpolled task, generation waiters still fail
            // promptly instead of waiting on a worker that never started.
            let exit_guard = IndexWorkerGuard {
                inner: self.inner.clone(),
                worker_id,
                armed: true,
            };
            tauri::async_runtime::spawn(async move {
                state.run_index_worker(exit_guard).await;
            });
        }
        Some(receipt)
    }

    pub(crate) async fn wait_for_index_generation(&self, generation: u64) -> Result<()> {
        loop {
            // Register before checking the predicate so a completion between the
            // check and await cannot strand this waiter.
            let mut notified = Box::pin(self.inner.index_completion.notified());
            let _ = notified.as_mut().enable();
            if let Some(completion) = self
                .inner
                .index_scheduler
                .lock()
                .completion_for(generation)
            {
                return completion.map_err(anyhow::Error::msg);
            }
            notified.await;
        }
    }

    pub(crate) async fn run_index_worker(&self, mut exit_guard: IndexWorkerGuard) {
        loop {
            let Some(mut request) = self.inner.index_scheduler.lock().take_pending() else {
                log::error!("index worker started without pending work");
                return;
            };

            // Trailing-edge debounce: each filesystem event restarts the quiet
            // period. An explicit/manual request upgrades the batch to immediate.
            while request.debounce {
                tokio::time::sleep(INDEX_DEBOUNCE).await;
                let newer = self.inner.index_scheduler.lock().take_pending();
                match newer {
                    Some(pending) => request = pending,
                    None => break,
                }
            }

            let generation = request.generation;
            let result = self.reindex_workspace_once(request.workspace).await;
            let error = result.as_ref().err().map(ToString::to_string);
            if let Some(error) = &error {
                log::error!("workspace index failed: {error}");
                {
                    let mut runtime = self.inner.runtime.write();
                    runtime.index_state.is_indexing = false;
                }
                let _ = self.handle.emit("index://status", "failed");
            }

            let mut scheduler = self.inner.index_scheduler.lock();
            let has_pending_rerun = scheduler.finish_pass(generation, error);
            if !has_pending_rerun {
                // Disarm while the scheduler lock is still held. Otherwise a
                // new request could start worker B before this worker's guard is
                // dropped, and worker A's guard could mistake B for its own run.
                exit_guard.disarm();
            }
            drop(scheduler);
            self.inner.index_completion.notify_waiters();
            if !has_pending_rerun {
                return;
            }
        }
    }

    pub(crate) async fn reindex_workspace(&self, workspace: PathBuf) -> Result<()> {
        let receipt = self
            .request_reindex(workspace, false)
            .ok_or_else(|| anyhow!("workspace changed before indexing could start"))?;
        self.wait_for_index_generation(receipt.generation).await
    }

    /// Mutations that need indexed data before returning still wait for their
    /// generation, but allow the platform watcher's duplicate event to join the
    /// same quiet-period batch instead of forcing a second scan.
    pub(crate) async fn reindex_workspace_after_change(&self, workspace: PathBuf) -> Result<()> {
        let receipt = self
            .request_reindex(workspace, true)
            .ok_or_else(|| anyhow!("workspace changed before indexing could start"))?;
        self.wait_for_index_generation(receipt.generation).await
    }

    pub(crate) async fn reindex_workspace_once(&self, workspace: PathBuf) -> Result<()> {
        let _guard = self.inner.index_lock.lock().await;

        if self.inner.runtime.read().workspace_path.as_ref() != Some(&workspace) {
            return Ok(());
        }

        {
            let mut runtime = self.inner.runtime.write();
            runtime.index_state.is_indexing = true;
        }

        self.handle.emit("index://status", "started")?;

        let workspace_clone = workspace.clone();
        let workspace_data_dir = self.workspace_data_dir(&workspace);
        let scan = tauri::async_runtime::spawn_blocking(move || {
            read_workspace_notes(&workspace_clone, &workspace_data_dir)
        })
        .await
        .map_err(|e| anyhow!("spawn_blocking failed: {}", e))??;
        let mut notes = scan.notes;
        self.replace_storage_issues_matching(scan.issues, |issue| {
            matches!(
                issue.code.as_str(),
                "workspace-traversal" | "note-parse" | "duplicate-note-id"
            )
        });

        if self.inner.runtime.read().workspace_path.as_ref() != Some(&workspace) {
            return Ok(());
        }

        // Publish parsed notes before the secondary index work begins. This makes
        // the library and first note available while embeddings and LanceDB finish
        // in the background.
        let note_count = notes.len();
        {
            let mut runtime = self.inner.runtime.write();
            runtime.notes = notes
                .iter()
                .cloned()
                .map(|note| (note.document.id.clone(), note))
                .collect();
            runtime.custom_note_order =
                normalized_custom_order(&runtime.custom_note_order, &runtime.notes);
            runtime.index_state = IndexState {
                is_indexing: true,
                last_indexed_at: runtime.index_state.last_indexed_at.clone(),
                note_count,
                backend: "indexing".to_string(),
            };
        }
        self.handle.emit("index://status", "notes_ready")?;

        // Build source-preserving, non-overlapping retrieval chunks before
        // replacing the derived workspace index. A configured-model failure is
        // represented by nullable vectors and keyword-only search.
        let workspace_chunks = self.build_workspace_note_chunks(&notes).await;

        // Self-heal: remove orphaned chat sessions whose note no longer exists
        // (left behind by older deletes that didn't clean up the sidecar).
        {
            let live_ids: std::collections::HashSet<&str> =
                notes.iter().map(|n| n.document.id.as_str()).collect();
            let chats_dir = self.workspace_data_dir(&workspace).join("chats");
            if let Ok(entries) = std::fs::read_dir(&chats_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if let Some(id) = name.strip_suffix(".chat.json") {
                        if !live_ids.contains(id) {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }

        let mut backlinks_map: HashMap<String, Vec<Backlink>> = HashMap::new();
        for note in &notes {
            let links = extract_links(&note.document.body);
            for link in links {
                let target_note = notes
                    .iter()
                    .find(|n| {
                        (n.document.title == link.target || n.document.id == link.target)
                            && n.document.id != note.document.id
                    })
                    .or_else(|| {
                        notes.iter().find(|n| {
                            n.document.title == link.target || n.document.id == link.target
                        })
                    });
                if let Some(target) = target_note {
                    let backlink = Backlink {
                        source_id: note.document.id.clone(),
                        source_title: note.document.title.clone(),
                        target_block: link.block.clone(),
                        context_excerpt: excerpt_around(
                            &note.document.body,
                            link.start_index,
                            link.end_index,
                        ),
                    };
                    backlinks_map
                        .entry(target.document.id.clone())
                        .or_default()
                        .push(backlink);
                }
            }
        }

        for note in &mut notes {
            if let Some(links) = backlinks_map.remove(&note.document.id) {
                note.document.backlinks = links;
            } else {
                note.document.backlinks = Vec::new();
            }
        }

        if self.inner.runtime.read().workspace_path.as_ref() != Some(&workspace) {
            return Ok(());
        }
        let table = rebuild_lancedb(&self.index_dir(), &workspace_chunks).await?;

        {
            let mut runtime = self.inner.runtime.write();
            if runtime.workspace_path.as_ref() != Some(&workspace) {
                return Ok(());
            }
            // Notes can be edited, created, or deleted while the expensive vector
            // work runs. Preserve that live state; the watcher queues a follow-up
            // reindex to refresh any vectors affected by concurrent edits.
            let live_notes = runtime.notes.clone();
            notes.retain(|note| live_notes.contains_key(&note.document.id));
            for note in &mut notes {
                if let Some(live) = live_notes.get(&note.document.id) {
                    if live.document.updated_at != note.document.updated_at {
                        *note = live.clone();
                    }
                }
            }
            let mut indexed_notes: HashMap<String, IndexedNote> = notes
                .into_iter()
                .map(|note| (note.document.id.clone(), note))
                .collect();
            for (id, note) in live_notes {
                indexed_notes.entry(id).or_insert(note);
            }
            let note_count = indexed_notes.len();
            runtime.notes = indexed_notes;
            runtime.custom_note_order =
                normalized_custom_order(&runtime.custom_note_order, &runtime.notes);
            runtime.index_state = IndexState {
                is_indexing: false,
                last_indexed_at: Some(timestamp_now()),
                note_count,
                backend: format!("lancedb:{}", table.name()),
            };
        }

        self.persist_runtime_settings()?;
        self.handle.emit("index://status", "completed")?;
        Ok(())
    }

}
