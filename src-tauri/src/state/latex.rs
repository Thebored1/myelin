use super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;

impl AppState {
    pub(crate) fn tectonic_cache_dir(&self) -> PathBuf {
        self.inner.app_data_dir.join(TECTONIC_CACHE_DIR_NAME)
    }

    pub(crate) fn tectonic_warmed_marker(&self) -> PathBuf {
        self.tectonic_cache_dir().join(TECTONIC_WARMED_MARKER)
    }

    pub(crate) fn is_tectonic_warmed(&self) -> bool {
        self.tectonic_warmed_marker().exists()
    }

    pub(crate) fn mark_tectonic_warmed(&self) {
        let _ = fs::write(self.tectonic_warmed_marker(), b"1");
    }

    /// Cache state for the Settings UI: whether the support bundle has been
    /// fetched at least once, and how much disk the cache currently occupies.
    pub fn tectonic_cache_status(&self) -> TectonicCacheStatus {
        TectonicCacheStatus {
            warmed: self.is_tectonic_warmed(),
            size_bytes: dir_size(&self.tectonic_cache_dir()),
        }
    }

    /// Compile `tex` to PDF bytes. The heavy Tectonic call runs on a blocking
    /// thread so it never stalls the async runtime. When the package cache hasn't
    /// been warmed yet (first run ⇒ ~50 MB bundle fetch) we emit `latex://download`
    /// events (`start` / `progress` with byte counts / `done` / `error`) so the UI
    /// can show a real download indicator instead of a generic spinner.
    pub(crate) async fn run_tectonic(&self, tex: String, line_map: Vec<usize>, input_root: Option<PathBuf>) -> Result<Vec<u8>> {
        use std::sync::atomic::{AtomicBool, Ordering};

        // One Tectonic run at a time — concurrent runs corrupt the format cache.
        let _tectonic_guard = self.inner.tectonic_lock.lock().await;

        let needs_fetch = !self.is_tectonic_warmed();
        let cache_dir = self.tectonic_cache_dir();
        let handle = self.handle.clone();

        // While the (blocking) compile downloads the bundle, a side thread polls
        // the cache directory size and streams real progress to the frontend.
        let stop = Arc::new(AtomicBool::new(false));
        let poller = if needs_fetch {
            let _ = handle.emit(
                "latex://download",
                serde_json::json!({ "phase": "start", "bytes": dir_size(&cache_dir) }),
            );
            let stop_poll = stop.clone();
            let poll_handle = handle.clone();
            let poll_dir = cache_dir.clone();
            Some(std::thread::spawn(move || {
                while !stop_poll.load(Ordering::Relaxed) {
                    let _ = poll_handle.emit(
                        "latex://download",
                        serde_json::json!({ "phase": "progress", "bytes": dir_size(&poll_dir) }),
                    );
                    std::thread::sleep(std::time::Duration::from_millis(700));
                }
            }))
        } else {
            None
        };

        let result = tauri::async_runtime::spawn_blocking(move || {
			compile_with_tectonic(&tex, input_root.as_deref())
		}).await;

        stop.store(true, Ordering::Relaxed);
        if let Some(p) = poller {
            let _ = p.join();
        }

        match result {
            Ok(Ok(pdf)) => {
                self.mark_tectonic_warmed();
                if needs_fetch {
                    let _ = handle.emit(
                        "latex://download",
                        serde_json::json!({ "phase": "done", "bytes": dir_size(&cache_dir) }),
                    );
                }
                Ok(pdf)
            }
            Ok(Err(failure)) => {
                // A non-empty TeX log means the engine actually ran (bundle present),
                // so a LaTeX *content* error shouldn't keep re-triggering the
                // first-run download UI. An empty log ⇒ the bundle fetch itself
                // failed (e.g. offline) — surface that as a download error.
                let engine_ran = !failure.log.is_empty();
                if engine_ran {
                    self.mark_tectonic_warmed();
                }
                if needs_fetch {
                    let phase = if engine_ran { "done" } else { "error" };
                    let _ = handle.emit(
                        "latex://download",
                        serde_json::json!({
                            "phase": phase,
                            "bytes": dir_size(&cache_dir),
                            "message": failure.message,
                        }),
                    );
                }
                // Serialise as JSON so the frontend can place line markers in the
                // editor; it falls back to showing the message verbatim otherwise.
                let payload = serde_json::json!({
                    "message": failure.message,
                    "log": failure.log,
                    "diagnostics": parse_tex_log(&failure.log, &line_map),
                });
                Err(anyhow!("{payload}"))
            }
            Err(e) => {
                if needs_fetch {
                    let _ = handle.emit(
                        "latex://download",
                        serde_json::json!({ "phase": "error", "message": e.to_string() }),
                    );
                }
                Err(anyhow!("LaTeX compile task failed: {}", e))
            }
        }
    }

    pub async fn compile_latex(&self, note_id: String, source: Option<String>) -> Result<Vec<u8>> {
        let workspace = self.require_workspace()?;
        let raw = if let Some(source) = source {
            // Auto-preview compiles the immutable editor snapshot supplied by
            // the caller, rather than racing the debounced note save.
            if !self.inner.runtime.read().notes.contains_key(&note_id) {
                return Err(anyhow!("note not found"));
            }
            source
        } else {
            let path = {
                let runtime = self.inner.runtime.read();
                let note = runtime
                    .notes
                    .get(&note_id)
                    .ok_or_else(|| anyhow!("note not found"))?;
                workspace.join(&note.document.relative_path)
            };
            fs::read_to_string(&path)?
        };
        // Current .tex files are raw source. Strip only a recognized Myelin YAML
        // wrapper left by an older release; arbitrary TeX beginning with `---`
        // remains untouched.
        let tex_content = split_legacy_native_frontmatter(&raw).1;
        if tex_content.trim().is_empty() {
            return Err(anyhow!(
                "This note is empty — add some LaTeX before compiling."
            ));
        }

        // Bare notes get the full default preamble prepended; notes with their own
        // \documentclass get any missing common packages injected. Either way the
        // returned offset is how many lines we added before the body, so TeX error
        // lines map back to what the editor shows.
        let transform = if !has_active_documentclass(&tex_content) {
            wrap_bare_latex(&tex_content)
        } else {
            ensure_packages(&tex_content)
        };

        let note_dir = {
			let runtime = self.inner.runtime.read();
			let note = runtime.notes.get(&note_id).ok_or_else(|| anyhow!("note not found"))?;
			workspace.join(&note.document.relative_path).parent().map(Path::to_path_buf)
		};
		self.run_tectonic(transform.source, transform.line_map, note_dir).await
    }

	pub fn import_latex_asset(&self, note_id: String, source_path: String) -> Result<String> {
		let workspace = self.require_workspace()?.canonicalize()?;
		let note_path = {
			let runtime = self.inner.runtime.read();
			let note = runtime.notes.get(&note_id).ok_or_else(|| anyhow!("note not found"))?;
			workspace.join(&note.document.relative_path)
		};
		let source = PathBuf::from(source_path).canonicalize()?;
		if !source.is_file() {
			return Err(anyhow!("selected image is not a file"));
		}
		let extension = source.extension().and_then(|v| v.to_str()).unwrap_or("").to_ascii_lowercase();
		if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "pdf") {
			return Err(anyhow!("unsupported LaTeX image type: {extension}"));
		}
		let note_dir = note_path.parent().ok_or_else(|| anyhow!("note has no parent directory"))?;
		let stem = note_path.file_stem().and_then(|v| v.to_str()).unwrap_or("note");
		let assets_dir = note_dir.join(format!("{stem}-assets"));
		fs::create_dir_all(&assets_dir)?;
		let original_name = source.file_name().ok_or_else(|| anyhow!("image has no filename"))?;
		let mut target = assets_dir.join(original_name);
		let mut suffix = 2;
		while target.exists() {
			let base = source.file_stem().and_then(|v| v.to_str()).unwrap_or("image");
			target = assets_dir.join(format!("{base}-{suffix}.{extension}"));
			suffix += 1;
		}
		fs::copy(&source, &target)?;
		let relative = target.strip_prefix(note_dir).map_err(|_| anyhow!("asset path escaped note directory"))?;
		Ok(relative.to_string_lossy().replace('\\', "/"))
	}

    /// Pre-download Tectonic's support bundle by compiling a tiny stub document,
    /// so users can warm the cache from Settings instead of paying the first-run
    /// fetch when they hit "Compile to PDF".
    pub async fn prewarm_tectonic(&self) -> Result<()> {
        let stub = wrap_bare_latex("Myelin LaTeX warm-up: $E = mc^2$, \\textbf{ready}.");
        self.run_tectonic(stub.source, stub.line_map, None).await.map(|_| ())
    }

}
