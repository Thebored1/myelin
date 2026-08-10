use ::anyhow::{anyhow, Context, Result};
use super::core::*;
use super::*;

impl AppState {
    pub fn new(handle: AppHandle) -> Result<Self> {
        let app_data_dir = handle
            .path()
            .app_data_dir()
            .context("failed to resolve app data directory")?;
        fs::create_dir_all(&app_data_dir).with_context(|| {
            format!(
                "failed to create app data dir at {}",
                app_data_dir.display()
            )
        })?;

        // Create the schema-backed AI configuration alongside legacy settings.
        // The launcher still reads its legacy projection for now; this makes
        // the file available immediately without changing an existing user's
        // active runtime during the migration.
        crate::ai_config::ensure_file(&app_data_dir)?;
        crate::ai_config::ensure_schema(&app_data_dir)?;

        // Pin Tectonic's package cache under app data (see TECTONIC_CACHE_DIR_NAME).
        // Honoured by tectonic via the TECTONIC_CACHE_DIR env var (>= v0.9). Set
        // here at startup, before any compile, so the bundle lands where we expect.
        let tectonic_cache = app_data_dir.join(TECTONIC_CACHE_DIR_NAME);
        let _ = fs::create_dir_all(&tectonic_cache);
        std::env::set_var("TECTONIC_CACHE_DIR", &tectonic_cache);

        // Register the bundled-binary directory (shipped CPU/Vulkan builds) so
        // the backend resolver finds them automatically in a packaged app.
        let resource_bin = handle.path().resource_dir().ok().map(|dir| dir.join("bin"));
        crate::llama_server::set_resource_bin_dir(resource_bin);

        let settings = load_settings(&app_data_dir)?;
        let workspace_path = settings.workspace_path.map(PathBuf::from);
        // The applied schema-backed config is authoritative for managed
        // OpenHarn settings. Fall back to the legacy mirror only when no valid
        // applied config exists, preserving compatibility during migration.
        let openharn_settings = match crate::ai_config::load_applied(&app_data_dir) {
            Ok(Some(config)) if crate::ai_config::require_valid(&config).is_ok() =>
                project_ai_agent_settings(&settings.openharn, &config.agent),
            _ => settings.openharn.clone(),
        };
        let background_settings = settings.background.clone();

        Ok(Self {
            handle,
            inner: Arc::new(InnerState {
                app_data_dir,
                runtime: RwLock::new(RuntimeState {
                    workspace_path,
                    notes: HashMap::new(),
                    custom_note_order: settings.custom_note_order,
                    index_state: IndexState {
                        is_indexing: false,
                        last_indexed_at: None,
                        note_count: 0,
                        backend: "lancedb".into(),
                    },
                }),
                watcher: Mutex::new(None),
                index_lock: AsyncMutex::new(()),
                index_scheduler: Mutex::new(IndexScheduler::default()),
                index_completion: tokio::sync::Notify::new(),
                tectonic_lock: AsyncMutex::new(()),
                llama_server: AsyncMutex::new(None),
                ai_pipeline_lock: AsyncMutex::new(()),
                ai_pipeline_ready: std::sync::atomic::AtomicBool::new(false),
                embed_server: AsyncMutex::new(None),
                sidecar: AsyncMutex::new(None),
                chat_lock: AsyncMutex::new(()),
                llama_slot_lock: AsyncMutex::new(()),
                section_cache_preempt: std::sync::atomic::AtomicBool::new(false),
                section_cache_resume: tokio::sync::Notify::new(),
                openharn_settings: Mutex::new(openharn_settings),
                background_settings: Mutex::new(background_settings),
                llama_client: Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .context("failed to create llama HTTP client")?,
                chat_tools: Mutex::new(Vec::new()),
                latest_chat_question: Mutex::new(None),
                current_selection: Mutex::new(None),
                current_doc_type: Mutex::new(None),
                current_note_id: Mutex::new(None),
                last_slot_save: Mutex::new(None),
                active_slot_cache: Mutex::new(None),
                section_cache: Mutex::new(None),
                cancel_ai: std::sync::atomic::AtomicBool::new(false),
                cancel_notify: tokio::sync::Notify::new(),
                require_tool_approval: std::sync::atomic::AtomicBool::new(false),
                deterministic_tools: std::sync::atomic::AtomicBool::new(true),
                tool_gating: std::sync::atomic::AtomicBool::new(false),
                targeted_write: std::sync::atomic::AtomicBool::new(false),
                chat_mode: std::sync::atomic::AtomicBool::new(false),
                append_only: std::sync::atomic::AtomicBool::new(false),
                placement_edit: std::sync::atomic::AtomicBool::new(false),
                oversized_doc: std::sync::atomic::AtomicBool::new(false),
                tools_supported: std::sync::atomic::AtomicBool::new(true),
                note_ingest_locks: Mutex::new(HashMap::new()),
                note_ingest_manifest_lock: AsyncMutex::new(()),
                query_embedding_cache: Mutex::new(None),
                prompt_warmup: Mutex::new(None),
                pending_approvals: Mutex::new(HashMap::new()),
                conversations: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub(crate) async fn wait_for_ai_cancel(&self) {
        self.inner.cancel_notify.notified().await;
    }

    pub async fn bootstrap(&self) -> Result<AppSnapshot> {
        let workspace = self.inner.runtime.read().workspace_path.clone();
        if let Some(workspace) = workspace {
            crate::git_history::init_repo(&workspace)?;
            self.start_watcher(&workspace)?;
            {
                let mut runtime = self.inner.runtime.write();
                runtime.index_state.is_indexing = true;
                runtime.index_state.backend = "scanning".to_string();
            }
            // Startup only needs enough data to open notes. Keep the expensive
            // embedding and LanceDB work off the first-paint path; reindex emits a
            // status event as soon as parsed notes are available.
            let _ = self.request_reindex(workspace, false);
        }
        Ok(self.snapshot())
    }

    pub async fn set_workspace(&self, workspace_path: String) -> Result<AppSnapshot> {
        let workspace = PathBuf::from(workspace_path);
        fs::create_dir_all(&workspace)
            .with_context(|| format!("failed to create workspace at {}", workspace.display()))?;
        crate::git_history::init_repo(&workspace)?;

        {
            let mut runtime = self.inner.runtime.write();
            runtime.workspace_path = Some(workspace.clone());
        }

        let mut settings = load_settings(&self.inner.app_data_dir)?;
        settings.workspace_path = Some(workspace.to_string_lossy().into_owned());
        settings.custom_note_order = self.inner.runtime.read().custom_note_order.clone();
        save_settings(&self.inner.app_data_dir, &settings)?;

        self.start_watcher(&workspace)?;
        self.reindex_workspace(workspace).await?;
        Ok(self.snapshot())
    }

    pub(crate) fn invalidate_ai_pipeline(&self) {
        self.inner
            .ai_pipeline_ready
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn ai_pipeline_ready(&self) -> bool {
        self.inner
            .ai_pipeline_ready
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub async fn rebuild_index(&self) -> Result<AppSnapshot> {
        let workspace = self.require_workspace()?;
        self.reindex_workspace(workspace).await?;
        Ok(self.snapshot())
    }

    pub async fn save_pdf_annotations(
        &self,
        note_id: String,
        annotations: Vec<crate::models::PdfAnnotation>,
    ) -> Result<()> {
        let workspace = self.require_workspace()?;
        let workspace_data_dir = self.workspace_data_dir(&workspace);
        let annotations_dir = workspace_data_dir.join("annotations");
        fs::create_dir_all(&annotations_dir)?;
        let annotations_path = annotations_dir.join(format!("{}.annotations.json", note_id));
        if annotations.is_empty() {
            let _ = fs::remove_file(&annotations_path);
        } else {
            let tmp_path = annotations_dir.join(format!("{}.annotations.tmp", note_id));
            fs::write(&tmp_path, serde_json::to_string(&annotations)?)?;
            fs::rename(&tmp_path, &annotations_path)?;
        }
        {
            let mut runtime = self.inner.runtime.write();
            if let Some(note) = runtime.notes.get_mut(&note_id) {
                note.document.annotations = annotations;
            }
        }
        Ok(())
    }

    pub async fn import_pdf_file(
        &self,
        file_path: String,
        notebook: Option<String>,
    ) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let src = PathBuf::from(&file_path);

        if !src.exists() {
            return Err(anyhow!("file not found: {}", file_path));
        }

        let file_name = src
            .file_name()
            .ok_or_else(|| anyhow!("invalid file path: no filename"))?;
        // Place every import inside the requested notebook (folder), else the
        // workspace root. Even a file already in the workspace is copied so a
        // second attachment gets its own source identity and annotations.
        let target_dir = match &notebook {
            Some(name)
                if !name.trim().is_empty() && !name.trim().eq_ignore_ascii_case("root") =>
            {
                let safe = sanitize_relative_folder(name)?;
                let dir = workspace.join(folder_to_relative_path(&safe));
                fs::create_dir_all(&dir)
                    .map_err(|e| anyhow!("failed to open notebook: {}", e))?;
                dir
            }
            _ => workspace.clone(),
        };
        let dest = unique_pdf_path(&target_dir, file_name);
        fs::copy(&src, &dest)
            .map_err(|e| anyhow!("failed to copy PDF to workspace: {}", e))?;

        self.reindex_workspace_after_change(workspace.clone()).await?;

        let rel_path = relative_to_workspace(&workspace, &dest);
        let runtime = self.inner.runtime.read();
        runtime
            .notes
            .values()
            .find(|n| n.document.relative_path == rel_path)
            .map(|n| n.document.clone())
            .ok_or_else(|| anyhow!("PDF not found in index after import"))
    }

    /// Copy an existing workspace PDF into the destination notebook so each
    /// attached note owns an independent source document.
    pub async fn clone_pdf_for_attachment(
        &self,
        note_id: String,
        notebook: Option<String>,
    ) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let source = self
            .note_by_id(&note_id)
            .ok_or_else(|| anyhow!("PDF note not found: {note_id}"))?;
        if !source.relative_path.to_ascii_lowercase().ends_with(".pdf") {
            return Err(anyhow!("note is not a PDF: {note_id}"));
        }
        let source_path = workspace.join(&source.relative_path);
        if !source_path.is_file() {
            return Err(anyhow!("PDF file not found: {}", source.relative_path));
        }
        let target_dir = match &notebook {
            Some(name)
                if !name.trim().is_empty() && !name.trim().eq_ignore_ascii_case("root") =>
            {
                let safe = sanitize_relative_folder(name)?;
                let dir = workspace.join(folder_to_relative_path(&safe));
                fs::create_dir_all(&dir)
                    .map_err(|e| anyhow!("failed to open notebook: {}", e))?;
                dir
            }
            _ => workspace.clone(),
        };
        let file_name = source_path
            .file_name()
            .ok_or_else(|| anyhow!("invalid PDF path"))?;
        let dest = unique_pdf_path(&target_dir, file_name);
        fs::copy(&source_path, &dest)
            .map_err(|e| anyhow!("failed to copy PDF for attachment: {}", e))?;

        self.reindex_workspace_after_change(workspace.clone()).await?;
        let rel_path = relative_to_workspace(&workspace, &dest);
        let runtime = self.inner.runtime.read();
        runtime
            .notes
            .values()
            .find(|n| n.document.relative_path == rel_path)
            .map(|n| n.document.clone())
            .ok_or_else(|| anyhow!("cloned PDF not found in index after copy"))
    }

    pub fn snapshot(&self) -> AppSnapshot {
        let runtime = self.inner.runtime.read();
        let note_summaries = runtime
            .notes
            .values()
            .map(|note| summarize(&note.document))
            .collect::<Vec<_>>();
        let custom_note_order = normalized_custom_order(&runtime.custom_note_order, &runtime.notes);
        let notes = sort_summaries_by_custom_order(note_summaries, &custom_note_order);

        AppSnapshot {
            workspace_path: runtime
                .workspace_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            notes,
            custom_note_order,
            library_facets: build_library_facets(runtime.notes.values().map(|note| &note.document)),
            provider_status: default_provider_status(&self.inner.app_data_dir),
            index_state: runtime.index_state.clone(),
        }
    }

    pub fn shutdown_servers_sync(&self) {
        if let Ok(mut guard) = self.inner.llama_server.try_lock() {
            if let Some(server) = guard.as_mut() {
                let _ = server.child.kill();
            }
        }
        if let Ok(mut guard) = self.inner.embed_server.try_lock() {
            if let Some(server) = guard.as_mut() {
                let _ = server.child.kill();
            }
        }
        if let Ok(mut guard) = self.inner.sidecar.try_lock() { *guard = None; }
    }

    pub(crate) fn require_workspace(&self) -> Result<PathBuf> {
        self.inner
            .runtime
            .read()
            .workspace_path
            .clone()
            .ok_or_else(|| anyhow!("select a workspace first"))
    }

    pub(crate) fn start_watcher(&self, workspace: &Path) -> Result<()> {
        let state = self.clone();
        let workspace_path = workspace.to_path_buf();
        let mut watcher = recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                // Ignore read/open/close access events to avoid infinite reindexing loops when files are read
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }

                let is_markdown = event.paths.iter().any(|path| is_note_file(path));
                if !is_markdown {
                    return;
                }

                if let Some(receipt) =
                    state.request_reindex(workspace_path.clone(), true)
                {
                    // A burst only needs one UI invalidation. Further events are
                    // folded into the pending generation or the single dirty rerun.
                    if receipt.spawn_worker {
                        let _ = state.handle.emit("index://changed", "filesystem");
                    }
                }
            }
        })?;

        watcher.watch(workspace, RecursiveMode::Recursive)?;
        *self.inner.watcher.lock() = Some(watcher);
        Ok(())
    }

    /// Enqueue one full-workspace refresh. All callers share one worker and one
    /// pending slot, so event bursts cannot build an unbounded queue of scans.
    /// Holding the runtime read guard through scheduler insertion prevents a
    /// late event from an old watcher replacing a newer workspace request.
    pub(crate) async fn ensure_llama_server(&self, config: &llama_server::ResolvedLlamaConfig) -> Result<()> {
        let _ = self.handle.emit("ai://debug_event", serde_json::json!({
            "kind": "startup",
            "msg": format!(
                "Starting {} / checking llama-server readiness",
                if config.inference_engine == "beellama" { "BeeLlama" } else { "llama.cpp" }
            )
        }));
        let mut guard = self.inner.llama_server.lock().await;

        if let Some(server) = guard.as_mut() {
            if config.accepts_running(&server.config)
                && llama_server::health_check(&self.inner.llama_client, &server.config).await
            {
                return Ok(());
            }

            self.invalidate_ai_pipeline();
            // Distinguish an unexpected crash (e.g. a GPU device-lost mid-reply)
            // from a config change, and surface it. start_server then relaunches
            // with its adaptive offload + degrade-on-failure plans.
            if let Ok(Some(status)) = server.child.try_wait() {
                log::warn!("llama-server exited unexpectedly ({status}); relaunching");
                let _ = self.handle.emit(
                    "ai://llama_backend",
                    serde_json::json!({
                        "backend": server.active_backend.label(),
                        "engine": server.active_engine,
                        "gpuOffloaded": false,
                        "fellBackToCpu": false,
                        "crashed": true,
                    }),
                );
            }

            llama_server::stop_server(server).await;
            *guard = None;
        }

        let server = llama_server::start_server(&self.inner.llama_client, config).await?;

        // Surface which compute backend actually loaded so the UI can show it,
        // and warn loudly if we wanted a GPU but silently landed on CPU.
        let backend = server.active_backend.label().to_string();
        let fell_back_to_cpu = server.requested_gpu && !server.gpu_offloaded;
        if fell_back_to_cpu {
            log::warn!(
                "llama-server fell back to CPU: a GPU backend was requested but no device was used. Install a GPU backend under <app_data>/bin/ for full speed."
            );
        } else if server.gpu_offloaded {
            log::info!("llama-server running on GPU backend: {backend}");
        } else {
            log::info!("llama-server running on CPU backend");
        }
        let _ = self.handle.emit(
            "ai://llama_backend",
            serde_json::json!({
                "backend": backend,
                "engine": server.active_engine,
                "gpuOffloaded": server.gpu_offloaded,
                "fellBackToCpu": fell_back_to_cpu,
            }),
        );
        let _ = self.handle.emit(
            "ai://debug_event",
            serde_json::json!({
                "kind": "startup",
                "msg": format!(
                    "Inference engine active: {} ({backend})",
                    if server.active_engine == "beellama" { "BeeLlama" } else { "llama.cpp" }
                )
            }),
        );

        *guard = Some(server);
        Ok(())
    }


}
