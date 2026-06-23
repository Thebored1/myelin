use crate::llama_server::{self, ManagedLlamaServer};
use crate::models::{
    AppSnapshot, Backlink, ChatTool, IndexState, LibraryFacets, NoteDocument, NoteSummary,
    ProviderStatus, SearchResponse, SearchResult,
};
use anyhow::{anyhow, Context, Result};
use arrow_array::types::Float32Type;
use arrow_array::{ArrayRef, FixedSizeListArray, RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema::{DataType, Field, Schema};
use chrono::Utc;
use lancedb::connection::Connection;
use lancedb::{connect, Table};
use notify::{recommended_watcher, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};
use reqwest::Client;
use rig_core::completion::{CompletionError, Prompt, PromptError};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{async_runtime::Mutex as AsyncMutex, AppHandle, Emitter, Manager};
use uuid::Uuid;

const EMBEDDING_DIM: i32 = 64;
const INDEX_DIR_NAME: &str = "index";
const MAX_CHAT_HISTORY_MESSAGES_IN_PROMPT: usize = 4;
// How much of the open note to put in the prompt. The assistant edits this note,
// so it must see (nearly) all of it — 400 chars left it editing blind. ~24k chars
// (~6k tokens) fits comfortably in the 32k context alongside the preamble/tools.
const NOTE_BODY_PROMPT_LIMIT: usize = 24_000;
const SETTINGS_FILE_NAME: &str = "settings.json";
const TABLE_NAME: &str = "notes";

fn describe_completion_error(error: &CompletionError) -> String {
    match error {
        CompletionError::HttpError(inner) => {
            format!("Could not reach the local llama server: {inner}")
        }
        CompletionError::ResponseError(message) => {
            format!("The local model returned an invalid response: {message}")
        }
        CompletionError::ProviderError(message) => {
            let lower = message.to_ascii_lowercase();
            if lower.contains("context length") || lower.contains("context_length_exceeded") {
                format!("The note and chat history exceeded the model context window. {message}")
            } else {
                format!("The local model rejected the request: {message}")
            }
        }
        _ => error.to_string(),
    }
}

fn describe_prompt_error(error: &PromptError) -> String {
    match error {
        PromptError::CompletionError(inner) => describe_completion_error(inner),
        PromptError::ToolError(inner) => format!("A note tool failed while answering: {inner}"),
        PromptError::ToolServerError(inner) => {
            format!("The tool server failed while answering: {inner}")
        }
        PromptError::MaxTurnsError { max_turns, .. } => format!(
            "The model kept calling tools without finishing after {max_turns} turns. Try asking a narrower question."
        ),
        PromptError::PromptCancelled { reason, .. } => {
            format!("The AI request was cancelled: {reason}")
        }
        PromptError::UnknownToolCall { tool_name, .. } => format!(
            "The model tried to call an unsupported tool: {tool_name}"
        ),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub handle: AppHandle,
    inner: Arc<InnerState>,
}

struct InnerState {
    app_data_dir: PathBuf,
    runtime: RwLock<RuntimeState>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    index_lock: AsyncMutex<()>,
    llama_server: AsyncMutex<Option<ManagedLlamaServer>>,
    llama_client: Client,
    chat_tools: Mutex<Vec<ChatTool>>,
    latest_chat_question: Mutex<Option<String>>,
    current_note_id: Mutex<Option<String>>,
    require_tool_approval: std::sync::atomic::AtomicBool,
    pending_approvals: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
}

#[derive(Default)]
struct RuntimeState {
    workspace_path: Option<PathBuf>,
    notes: HashMap<String, IndexedNote>,
    custom_note_order: Vec<String>,
    index_state: IndexState,
}

#[derive(Clone)]
struct IndexedNote {
    document: NoteDocument,
    vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct PersistedSettings {
    workspace_path: Option<String>,
    custom_note_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Frontmatter {
    id: Option<String>,
    title: Option<String>,
    tags: Option<Vec<String>>,
    created_at: Option<String>,
    updated_at: Option<String>,
    source_pdf: Option<String>,
}

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

        // Register the bundled-binary directory (shipped CPU/Vulkan builds) so
        // the backend resolver finds them automatically in a packaged app.
        let resource_bin = handle
            .path()
            .resource_dir()
            .ok()
            .map(|dir| dir.join("bin"));
        crate::llama_server::set_resource_bin_dir(resource_bin);

        let settings = load_settings(&app_data_dir)?;
        let workspace_path = settings.workspace_path.map(PathBuf::from);

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
                llama_server: AsyncMutex::new(None),
                llama_client: Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .context("failed to create llama HTTP client")?,
                chat_tools: Mutex::new(Vec::new()),
                latest_chat_question: Mutex::new(None),
                current_note_id: Mutex::new(None),
                require_tool_approval: std::sync::atomic::AtomicBool::new(false),
                pending_approvals: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn reset_chat_tools(&self) {
        self.inner.chat_tools.lock().clear();
    }

    pub fn record_chat_tool(&self, name: impl Into<String>, details: impl Into<String>) {
        self.inner.chat_tools.lock().push(ChatTool {
            name: name.into(),
            details: details.into(),
        });
    }

    pub fn take_chat_tools(&self) -> Vec<ChatTool> {
        std::mem::take(&mut *self.inner.chat_tools.lock())
    }

    pub fn set_latest_chat_question(&self, question: impl Into<String>) {
        *self.inner.latest_chat_question.lock() = Some(question.into());
    }

    pub fn clear_latest_chat_question(&self) {
        *self.inner.latest_chat_question.lock() = None;
    }

    pub fn set_current_note_id(&self, note_id: impl Into<String>) {
        *self.inner.current_note_id.lock() = Some(note_id.into());
    }

    pub fn clear_current_note_id(&self) {
        *self.inner.current_note_id.lock() = None;
    }

    pub fn current_note_id(&self) -> Option<String> {
        self.inner.current_note_id.lock().clone()
    }

    fn note_by_id(&self, id: &str) -> Option<NoteDocument> {
        self.inner
            .runtime
            .read()
            .notes
            .get(id)
            .map(|note| note.document.clone())
    }

    /// Resolve the note a chat tool should act on: always prefer the note that
    /// is currently open in the editor, regardless of the title the model
    /// passed (a small model often gets the title wrong). Fall back to an exact
    /// title match only when no note is open.
    pub fn resolve_chat_target_note(&self, title: &str) -> Option<NoteDocument> {
        if let Some(id) = self.current_note_id() {
            if let Some(doc) = self.note_by_id(&id) {
                return Some(doc);
            }
        }
        self.find_note_by_exact_title(title)
    }

    /// Guard for `write_note`: only let the model mutate the note when the latest
    /// user message actually asked for a note change. A small model otherwise
    /// "helpfully" rewrites the note on a plain Q&A turn. When no question is
    /// recorded (warm-up / programmatic paths) we stay permissive.
    pub fn latest_chat_allows_note_mutation(&self) -> bool {
        self.inner
            .latest_chat_question
            .lock()
            .as_deref()
            .map(crate::agent::note_write_intent)
            .unwrap_or(true)
    }

    pub fn is_tool_approval_required(&self) -> bool {
        self.inner.require_tool_approval.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_require_tool_approval(&self, require: bool) {
        self.inner.require_tool_approval.store(require, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn register_pending_approval(&self, id: String, tx: tokio::sync::oneshot::Sender<bool>) {
        self.inner.pending_approvals.lock().insert(id, tx);
    }

    pub fn resolve_tool_approval(&self, id: &str, approved: bool) {
        if let Some(tx) = self.inner.pending_approvals.lock().remove(id) {
            let _ = tx.send(approved);
        }
    }

    pub async fn bootstrap(&self) -> Result<AppSnapshot> {
        let workspace = self.inner.runtime.read().workspace_path.clone();
        if let Some(workspace) = workspace {
            crate::git_history::init_repo(&workspace)?;
            self.start_watcher(&workspace)?;
            self.reindex_workspace(workspace).await?;
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

        save_settings(
            &self.inner.app_data_dir,
            &PersistedSettings {
                workspace_path: Some(workspace.to_string_lossy().into_owned()),
                custom_note_order: self.inner.runtime.read().custom_note_order.clone(),
            },
        )?;

        self.start_watcher(&workspace)?;
        self.reindex_workspace(workspace).await?;
        Ok(self.snapshot())
    }

    pub async fn set_llama_model_path(&self, model_path: String) -> Result<()> {
        crate::llama_server::set_model_path(&self.inner.app_data_dir, model_path)?;
        Ok(())
    }

    pub async fn set_llama_executable_path(&self, executable_path: String) -> Result<()> {
        crate::llama_server::set_executable_path(&self.inner.app_data_dir, executable_path)?;
        Ok(())
    }

    pub async fn set_llama_advanced_config(
        &self,
        context_size: Option<u32>,
        gpu_layers: Option<i32>,
        threads: Option<u32>,
        temperature: Option<f32>,
        top_p: Option<f32>,
        extra_args: Option<Vec<String>>,
        backend_preference: Option<String>,
        gpu_device: Option<String>,
        thinking: Option<bool>,
        auto_offload: Option<bool>,
        max_turns: Option<u32>,
    ) -> Result<()> {
        crate::llama_server::set_advanced_config(
            &self.inner.app_data_dir,
            context_size,
            gpu_layers,
            threads,
            temperature,
            top_p,
            extra_args,
            backend_preference,
            gpu_device,
            thinking,
            auto_offload,
            max_turns,
        )?;
        Ok(())
    }

    pub fn list_llama_devices(&self, backend: String) -> Vec<crate::llama_server::DeviceInfo> {
        crate::llama_server::list_devices(&self.inner.app_data_dir, &backend)
    }

    pub fn downloadable_backends(&self) -> Vec<String> {
        crate::llama_server::downloadable_backends()
    }

    fn emit_download(&self, backend: &str, phase: &str, percent: f64, message: &str) {
        let _ = self.handle.emit(
            "backend://download",
            serde_json::json!({
                "backend": backend,
                "phase": phase,
                "percent": percent,
                "message": message,
            }),
        );
    }

    /// Download, extract and install a llama.cpp backend build into the
    /// app-data bin dir, emitting `backend://download` progress events.
    pub async fn download_llama_backend(&self, backend: String) -> Result<()> {
        use futures_util::StreamExt;

        let assets = crate::llama_server::assets_for_backend(&backend);
        if assets.is_empty() {
            anyhow::bail!("No downloadable {backend} build is available for this platform.");
        }

        let bin_root = self.inner.app_data_dir.join("bin");
        let backend_dir = bin_root.join(&backend);
        let staging = bin_root.join(format!(".staging-{backend}"));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;

        let result: Result<()> = async {
            let total_assets = assets.len() as f64;
            for (i, asset) in assets.iter().enumerate() {
                let url = crate::llama_server::download_url(asset);
                self.emit_download(&backend, "downloading", (i as f64 / total_assets) * 100.0,
                    &format!("Downloading {} ({}/{})", asset, i + 1, assets.len()));

                let resp = self.inner.llama_client.get(&url).send().await
                    .with_context(|| format!("failed to download {asset}"))?;
                if !resp.status().is_success() {
                    anyhow::bail!("download failed for {asset}: HTTP {}", resp.status());
                }
                let total = resp.content_length().unwrap_or(0);
                let archive_path = staging.join(asset);
                let mut file = fs::File::create(&archive_path)?;
                let mut downloaded: u64 = 0;
                let mut last_pct: i32 = -1;
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    std::io::Write::write_all(&mut file, &chunk)?;
                    downloaded += chunk.len() as u64;
                    if total > 0 {
                        let frac = downloaded as f64 / total as f64;
                        let overall = ((i as f64 + frac) / total_assets) * 100.0;
                        let pct = overall as i32;
                        if pct != last_pct {
                            last_pct = pct;
                            self.emit_download(&backend, "downloading", overall,
                                &format!("Downloading {} ({}/{})", asset, i + 1, assets.len()));
                        }
                    }
                }
                drop(file);

                self.emit_download(&backend, "extracting", 100.0, &format!("Extracting {asset}"));
                crate::llama_server::extract_archive(&archive_path, &staging)?;
                let _ = fs::remove_file(&archive_path);
            }

            self.emit_download(&backend, "installing", 100.0, "Installing");
            let _ = fs::remove_dir_all(&backend_dir);
            crate::llama_server::install_backend_from_staging(&staging, &backend_dir)?;
            Ok(())
        }
        .await;

        let _ = fs::remove_dir_all(&staging);
        match result {
            Ok(()) => {
                self.emit_download(&backend, "done", 100.0, &format!("{backend} backend installed"));
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&backend_dir);
                self.emit_download(&backend, "error", 0.0, &error.to_string());
                Err(error)
            }
        }
    }

    fn ensure_unique_title(&self, requested_title: &str, current_note_id: Option<&str>) -> String {
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
    ) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let now = timestamp_now();
        let id = Uuid::new_v4().to_string();

        let unique_title = self.ensure_unique_title(&title, None);
        let safe_slug = slugify(&unique_title);
        let ext = extension.unwrap_or_else(|| "md".to_string());
        let file_name = format!("{safe_slug}--{}.{ext}", &id[..8]);
        let path = unique_note_path(&workspace, &file_name);
        let relative_path = relative_to_workspace(&workspace, &path);

        let document = NoteDocument {
            id,
            title: unique_title,
            tags: Vec::new(),
            body: String::new(),
            relative_path,
            created_at: now.clone(),
            updated_at: now,
            source_pdf,
            annotations: Vec::new(),
            backlinks: Vec::new(),
            chat_history: Vec::new(),
        };

        let vector = hashed_embedding(&format!(
            "{}\n{}\n{}",
            document.title,
            document.tags.join(" "),
            document.body
        ));

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

        let state = self.clone();
        let workspace_clone = workspace.clone();
        tauri::async_runtime::spawn(async move {
            let _ = state.reindex_workspace(workspace_clone).await;
        });

        Ok(document)
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

        let path = workspace.join(&updated.relative_path);

        let vector = hashed_embedding(&format!(
            "{}\n{}\n{}",
            updated.title,
            updated.tags.join(" "),
            updated.body
        ));

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
        crate::git_history::commit_changes(&workspace, &format!("Update note: {}", updated.title))?;

        let state = self.clone();
        let workspace_clone = workspace.clone();
        tauri::async_runtime::spawn(async move {
            let _ = state.reindex_workspace(workspace_clone).await;
        });

        Ok(updated)
    }

    pub async fn delete_note(&self, note_id: String) -> Result<AppSnapshot> {
        let workspace = self.require_workspace()?;
        let path = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .map(|note| workspace.join(&note.document.relative_path))
                .ok_or_else(|| anyhow!("note not found"))?
        };

        fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;
        crate::git_history::commit_changes(&workspace, &format!("Delete note: {}", note_id))?;
        self.reindex_workspace(workspace).await?;
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
        let file_name = format!("{}--{}.md", slugify(&duplicate_title), &duplicate_id[..8]);
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
        self.reindex_workspace(workspace).await?;
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
        fs::rename(&source_path, &target_path).with_context(|| {
            format!(
                "failed to move {} to {}",
                source_path.display(),
                target_path.display()
            )
        })?;
        crate::git_history::commit_changes(
            &workspace,
            &format!("Move note: {}", source.document.title),
        )?;

        self.reindex_workspace(workspace).await?;
        self.load_note(note_id).await
    }

    pub async fn reorder_note(&self, note_id: String, direction: String) -> Result<AppSnapshot> {
        let workspace = self.require_workspace()?;
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
        self.reindex_workspace(workspace).await?;
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

        let query_vector = hashed_embedding(trimmed);
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
    pub async fn provider_status(&self) -> Result<ProviderStatus> {
        let info = llama_server::inspect_provider(&self.inner.app_data_dir)?;
        // Prefer the backend of the running server; fall back to the backend we
        // would select on this machine.
        let mut active_backend = info.selected_backend.clone();
        let healthy = if let Some(config) = &info.resolved {
            let server = self.inner.llama_server.lock().await;
            if let Some(server) = server.as_ref() {
                active_backend = Some(server.active_backend.label().to_string());
                if server.config.matches_runtime(config) {
                    llama_server::health_check(&self.inner.llama_client, &server.config).await
                } else {
                    info.healthy
                }
            } else {
                info.healthy
            }
        } else {
            false
        };

        Ok(ProviderStatus {
            active_provider: "llama.cpp".into(),
            available_providers: vec!["llama.cpp".into()],
            healthy,
            detail: info.detail,
            config: Some(info.config),
            resolved: info.resolved,
            active_backend,
            nvidia_detected: info.nvidia_detected,
            gpu_available: info.gpu_available,
            gpus: info.gpus,
            installed_backends: info.installed_backends,
        })
    }

    pub async fn summarise_note(&self, note_id: String) -> Result<String> {
        let note = self.load_note(note_id).await?;
        let prompt = format!(
            "Summarise this note in concise plain language.\n\nTitle: {}\n\nTags: {}\n\nBody:\n{}",
            note.title,
            if note.tags.is_empty() {
                "(none)".to_string()
            } else {
                note.tags.join(", ")
            },
            note.body
        );

        self.run_llama_prompt(
            "You summarise the user's note faithfully. Keep the response concise, practical, and grounded only in the provided note.",
            &prompt,
        )
        .await
    }

    pub async fn ask_ai(&self, note_id: String, question: String) -> Result<String> {
        let note = self.load_note(note_id).await?;
        if is_simple_greeting(&question) {
            return Ok("Hello. What would you like to work on?".to_string());
        }
        let history_text = format_chat_history_for_prompt(&note.chat_history, &question);

        let prompt = format!(
            "Answer the user's latest question directly. Use the open note only if it is relevant to that question.\n\nOpen Note Context:\nTitle: {}\nTags: {}\nBody:\n{}\n\nRecent Chat History for background only:\n{}\n\nLatest Question:\n{}",
            note.title,
            if note.tags.is_empty() { "(none)".to_string() } else { note.tags.join(", ") },
            note.body,
            history_text,
            question
        );

        self.run_llama_prompt(
            "You are a helpful AI agent. Answer the latest question directly. Ignore the open note and recent chat history unless they are relevant or explicitly referenced.",
            &prompt,
        )
        .await
    }

    pub async fn ask_ai_stream(
        &self,
        note_id: String,
        question: String,
        request_id: String,
    ) -> Result<()> {
        self.reset_chat_tools();
        self.set_latest_chat_question(question.clone());
        self.set_current_note_id(note_id.clone());
        let result: Result<()> = async {
            let note = self.load_note(note_id).await?;
            let history_text = format_chat_history_for_prompt(&note.chat_history, &question);

            // Truncate on a char boundary (never a raw byte slice — that panics
            // on multi-byte UTF-8). Most notes fit well under the limit.
            let note_body_excerpt = if note.body.chars().count() > NOTE_BODY_PROMPT_LIMIT {
                let head: String = note.body.chars().take(NOTE_BODY_PROMPT_LIMIT).collect();
                format!("{head}\n…[note truncated — ask me to work on a specific section]")
            } else {
                note.body.clone()
            };
            // Give the model the note's CURRENT content as editable text. The old
            // "reference only — do NOT copy" framing (plus a 400-char cap) meant it
            // could neither see nor feel allowed to modify existing content, so it
            // could only write fresh, never edit/format/shorten/delete.
            let mut context = format!("The note currently open is titled \"{}\".", note.title);
            if note_body_excerpt.trim().is_empty() {
                context.push_str(" It is currently empty.");
            } else {
                context.push_str(&format!(
                    "\n\nHere is the note's CURRENT content. When the user asks you to edit, change, format, fix, clean up, rewrite, shorten, expand, reorder, or remove part of the note, treat this as the text to modify — reproduce the parts that stay, apply the change, and pass the full result to write_note. (When you are only answering a question, use it as reference and do not echo it back verbatim.)\n--- CURRENT NOTE ---\n{}\n--- END CURRENT NOTE ---",
                    note_body_excerpt
                ));
            }
            if !history_text.trim().is_empty() {
                context.push_str(&format!("\n\nEarlier in this conversation:\n{}", history_text));
            }
            let prompt = format!("{}\n\nUser request: {}", context, question);

            let config = llama_server::resolve_config(&self.inner.app_data_dir)?;
            self.ensure_llama_server(&config).await?;

            // Per-message tool gating: hand the model ONLY the tools this message
            // warrants so it can't misfire on one it was never given.
            let tools = crate::agent::select_tools(&question, true);

            // Stream directly against llama-server (not through rig) so the note
            // content can be surfaced token-by-token as it is generated. See
            // `stream_chat`.
            crate::stream_chat::run_chat(
                self,
                &config,
                crate::agent::MYELIN_PREAMBLE,
                &prompt,
                tools,
                &request_id,
            )
            .await?;

            Ok(())
        }
        .await;

        self.clear_latest_chat_question();
        self.clear_current_note_id();

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

    pub async fn save_chat_history(
        &self,
        note_id: String,
        chat_history: Vec<crate::models::ChatMessage>,
    ) -> Result<()> {
        let workspace = self.require_workspace()?;
        let mut document = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .cloned()
                .map(|n| n.document)
                .ok_or_else(|| anyhow!("note not found"))?
        };

        document.chat_history = chat_history;

        let chats_dir = self.workspace_data_dir(&workspace).join("chats");
        fs::create_dir_all(&chats_dir)?;
        let chats_path = chats_dir.join(format!("{}.chat.json", document.id));
        let tmp_chat_path = chats_dir.join(format!("{}.chat.tmp", document.id));
        fs::write(
            &tmp_chat_path,
            serde_json::to_string(&document.chat_history)?,
        )?;
        fs::rename(&tmp_chat_path, &chats_path)?;

        {
            let mut runtime = self.inner.runtime.write();
            if let Some(note) = runtime.notes.get_mut(&note_id) {
                note.document.chat_history = document.chat_history;
            }
        }
        Ok(())
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

    pub async fn import_pdf_file(&self, file_path: String) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let src = PathBuf::from(&file_path);

        if !src.exists() {
            return Err(anyhow!("file not found: {}", file_path));
        }

        let dest = if src.starts_with(&workspace) {
            src.clone()
        } else {
            let file_name = src
                .file_name()
                .ok_or_else(|| anyhow!("invalid file path: no filename"))?;
            let dest = workspace.join(file_name);
            if !dest.exists() {
                fs::copy(&src, &dest)
                    .map_err(|e| anyhow!("failed to copy PDF to workspace: {}", e))?;
            }
            dest
        };

        self.reindex_workspace(workspace.clone()).await?;

        let rel_path = relative_to_workspace(&workspace, &dest);
        let runtime = self.inner.runtime.read();
        runtime
            .notes
            .values()
            .find(|n| n.document.relative_path == rel_path)
            .map(|n| n.document.clone())
            .ok_or_else(|| anyhow!("PDF not found in index after import"))
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

    pub async fn compile_latex(&self, note_id: String) -> Result<Vec<u8>> {
        let workspace = self.require_workspace()?;
        let path = {
            let runtime = self.inner.runtime.read();
            let note = runtime.notes.get(&note_id).ok_or_else(|| anyhow!("note not found"))?;
            workspace.join(&note.document.relative_path)
        };
        let mut tex_content = fs::read_to_string(&path)?;
        
        if !tex_content.contains("\\documentclass") {
            tex_content = format!("\\documentclass{{article}}\n\\usepackage{{amsmath}}\n\\begin{{document}}\n{}\n\\end{{document}}", tex_content);
        }
        
        let pdf_data = tectonic::latex_to_pdf(tex_content)
            .map_err(|e| anyhow!("Failed to compile LaTeX using Tectonic: {}", e))?;
            
        Ok(pdf_data)
    }

    pub fn get_all_note_documents(&self) -> Vec<NoteDocument> {
        let runtime = self.inner.runtime.read();
        runtime.notes.values().map(|n| n.document.clone()).collect()
    }

    fn note_summaries(&self) -> Vec<NoteSummary> {
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

    fn require_workspace(&self) -> Result<PathBuf> {
        self.inner
            .runtime
            .read()
            .workspace_path
            .clone()
            .ok_or_else(|| anyhow!("select a workspace first"))
    }

    fn start_watcher(&self, workspace: &Path) -> Result<()> {
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

                let cloned_state = state.clone();
                let watched_workspace = workspace_path.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = cloned_state.handle.emit("index://changed", "filesystem");
                    let _ = cloned_state.reindex_workspace(watched_workspace).await;
                });
            }
        })?;

        watcher.watch(workspace, RecursiveMode::Recursive)?;
        *self.inner.watcher.lock() = Some(watcher);
        Ok(())
    }

    async fn reindex_workspace(&self, workspace: PathBuf) -> Result<()> {
        let _guard = self.inner.index_lock.lock().await;

        {
            let mut runtime = self.inner.runtime.write();
            runtime.index_state.is_indexing = true;
        }

        self.handle.emit("index://status", "started")?;

        let workspace_clone = workspace.clone();
        let workspace_data_dir = self.workspace_data_dir(&workspace);
        let mut notes = tauri::async_runtime::spawn_blocking(move || {
            read_workspace_notes(&workspace_clone, &workspace_data_dir)
        })
        .await
        .map_err(|e| anyhow!("spawn_blocking failed: {}", e))??;

        let mut backlinks_map: HashMap<String, Vec<Backlink>> = HashMap::new();
        for note in &notes {
            let links = extract_links(&note.document.body);
            for link in links {
                let target_note = notes
                    .iter()
                    .find(|n| (n.document.title == link.target || n.document.id == link.target) && n.document.id != note.document.id)
                    .or_else(|| {
                        notes.iter().find(|n| n.document.title == link.target || n.document.id == link.target)
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

        let table = rebuild_lancedb(&self.index_dir(), &notes).await?;
        let note_count = notes.len();

        {
            let mut runtime = self.inner.runtime.write();
            runtime.notes = notes
                .into_iter()
                .map(|note| (note.document.id.clone(), note))
                .collect();
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

    fn index_dir(&self) -> PathBuf {
        self.inner.app_data_dir.join(INDEX_DIR_NAME)
    }

    fn workspace_data_dir(&self, workspace: &Path) -> PathBuf {
        self.inner
            .app_data_dir
            .join("workspaces")
            .join(workspace_storage_key(workspace))
    }

    fn persist_runtime_settings(&self) -> Result<()> {
        let runtime = self.inner.runtime.read();
        save_settings(
            &self.inner.app_data_dir,
            &PersistedSettings {
                workspace_path: runtime
                    .workspace_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                custom_note_order: runtime.custom_note_order.clone(),
            },
        )
    }

    async fn run_llama_prompt(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
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

    async fn ensure_llama_server(&self, config: &llama_server::ResolvedLlamaConfig) -> Result<()> {
        let mut guard = self.inner.llama_server.lock().await;

        if let Some(server) = guard.as_mut() {
            if server.config.matches_runtime(config)
                && llama_server::health_check(&self.inner.llama_client, &server.config).await
            {
                return Ok(());
            }

            // Distinguish an unexpected crash (e.g. a GPU device-lost mid-reply)
            // from a config change, and surface it. start_server then relaunches
            // with its adaptive offload + degrade-on-failure plans.
            if let Ok(Some(status)) = server.child.try_wait() {
                log::warn!("llama-server exited unexpectedly ({status}); relaunching");
                let _ = self.handle.emit(
                    "ai://llama_backend",
                    serde_json::json!({
                        "backend": server.active_backend.label(),
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
                "gpuOffloaded": server.gpu_offloaded,
                "fellBackToCpu": fell_back_to_cpu,
            }),
        );

        *guard = Some(server);
        drop(guard);

        // Pre-warm the prompt cache: the system preamble + tool schemas are a
        // large (~1.1k token) constant prefix. Processing it once now means the
        // user's first real message reuses the cached prefix instead of paying
        // the full prompt-eval cost (tens of seconds on CPU).
        self.spawn_cache_warmup(config);
        Ok(())
    }

    /// Fire a throwaway completion that mirrors the live agent's system + tools
    /// prefix so llama-server caches it in the (single) slot. Fire-and-forget.
    fn spawn_cache_warmup(&self, config: &llama_server::ResolvedLlamaConfig) {
        let client = self.inner.llama_client.clone();
        let url = format!("{}/v1/chat/completions", config.base_url());
        let model = config.model_name();
        tokio::spawn(async move {
            let body = serde_json::json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": crate::agent::MYELIN_PREAMBLE },
                    { "role": "user", "content": "ping" }
                ],
                "tools": crate::agent::tool_specs(),
                "max_tokens": 1,
                "temperature": 0.0,
                "cache_prompt": true
            });
            match client.post(&url).json(&body).send().await {
                Ok(_) => log::info!("llama prompt-cache warm-up complete"),
                Err(error) => log::warn!("llama prompt-cache warm-up failed: {error}"),
            }
        });
    }
}

fn load_settings(app_data_dir: &Path) -> Result<PersistedSettings> {
    let settings_path = app_data_dir.join(SETTINGS_FILE_NAME);
    if !settings_path.exists() {
        return Ok(PersistedSettings::default());
    }

    let raw = fs::read_to_string(&settings_path)
        .with_context(|| format!("failed to read settings at {}", settings_path.display()))?;
    Ok(serde_json::from_str(&raw).context("failed to parse settings")?)
}

fn save_settings(app_data_dir: &Path, settings: &PersistedSettings) -> Result<()> {
    let settings_path = app_data_dir.join(SETTINGS_FILE_NAME);
    let raw = serde_json::to_string_pretty(settings)?;
    fs::write(&settings_path, raw)
        .with_context(|| format!("failed to write settings at {}", settings_path.display()))
}

fn format_chat_history_for_prompt(
    chat_history: &[crate::models::ChatMessage],
    _latest_question: &str,
) -> String {
    let mut messages = chat_history
        .iter()
        .filter(|message| !message.content.trim().is_empty())
        .filter(|message| message.error != Some(true))
        .rev()
        .take(MAX_CHAT_HISTORY_MESSAGES_IN_PROMPT)
        .map(|message| {
            let content = message.content.trim().replace('\n', " ");
            let content: String = content.chars().take(800).collect();
            format!("{}: {}", message.role, content)
        })
        .collect::<Vec<_>>();

    messages.reverse();

    if messages.is_empty() {
        "(none)".to_string()
    } else {
        messages.join("\n")
    }
}

fn is_simple_greeting(question: &str) -> bool {
    let normalized = question
        .trim()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric())
        .to_ascii_lowercase();

    matches!(
        normalized.as_str(),
        "hi" | "hello" | "hey" | "yo" | "sup" | "hiya" | "howdy"
    )
}

fn is_hidden_or_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.depth() > 0 && name.starts_with('.') {
        return true;
    }
    name == "node_modules" || name == "target" || name == "dist" || name == "build"
}

fn read_workspace_notes(workspace: &Path, workspace_data_dir: &Path) -> Result<Vec<IndexedNote>> {
    let mut notes = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e))
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_file() && is_note_file(entry.path()) {
            if let Some(extension) = entry.path().extension().and_then(std::ffi::OsStr::to_str) {
                let doc_result = if extension.eq_ignore_ascii_case("pdf") || extension.eq_ignore_ascii_case("epub") {
                    parse_pdf_file(workspace, workspace_data_dir, entry.path())
                } else {
                    parse_note_file(workspace, workspace_data_dir, entry.path())
                };

                if let Ok(document) = doc_result {
                    let vector = hashed_embedding(&format!(
                        "{}\n{}\n{}",
                        document.title,
                        document.tags.join(" "),
                        document.body
                    ));
                    notes.push(IndexedNote { document, vector });
                }
            }
        }
    }

    notes.sort_by(|left, right| right.document.updated_at.cmp(&left.document.updated_at));
    Ok(notes)
}

fn parse_pdf_file(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) -> Result<NoteDocument> {
    let title = default_title_from_path(path);
    let (created_at, updated_at) = get_file_timestamps(path);
    let id = stable_id_from_path(path);

    let annotations = {
        let annotations_path = sidecar_path(
            workspace,
            workspace_data_dir,
            "annotations",
            &format!("{}.annotations.json", id),
        );
        if annotations_path.exists() {
            fs::read_to_string(&annotations_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
        } else {
            None
        }
    };

    Ok(NoteDocument {
        id: id.clone(),
        title,
        tags: Vec::new(),
        body: String::new(),
        relative_path: relative_to_workspace(workspace, path),
        created_at,
        updated_at,
        source_pdf: None,
        annotations: annotations.unwrap_or_default(),
        backlinks: Vec::new(),
        chat_history: {
            let chats_path = sidecar_path(
                workspace,
                workspace_data_dir,
                "chats",
                &format!("{}.chat.json", id),
            );
            fs::read_to_string(&chats_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        },
    })
}

fn parse_note_file(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) -> Result<NoteDocument> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let (frontmatter, body) = split_frontmatter(&raw);
    let metadata = frontmatter
        .as_deref()
        .and_then(|frontmatter| serde_yaml::from_str::<Frontmatter>(frontmatter).ok())
        .unwrap_or_default();

    let title = metadata
        .title
        .unwrap_or_else(|| first_heading(&body).unwrap_or_else(|| default_title_from_path(path)));
    
    let (file_created, file_updated) = get_file_timestamps(path);
    let created_at = metadata.created_at.unwrap_or(file_created);
    let updated_at = metadata.updated_at.unwrap_or(file_updated);
    let id = metadata.id.unwrap_or_else(|| stable_id_from_path(path));

    let annotations = {
        let annotations_path = sidecar_path(
            workspace,
            workspace_data_dir,
            "annotations",
            &format!("{}.annotations.json", id),
        );
        if annotations_path.exists() {
            fs::read_to_string(&annotations_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
        } else {
            None
        }
    };

    Ok(NoteDocument {
        id: id.clone(),
        title,
        tags: metadata.tags.unwrap_or_default(),
        body,
        relative_path: relative_to_workspace(workspace, path),
        created_at,
        updated_at,
        source_pdf: metadata.source_pdf,
        annotations: annotations.unwrap_or_default(),
        backlinks: Vec::new(),
        chat_history: {
            let chats_path = sidecar_path(
                workspace,
                workspace_data_dir,
                "chats",
                &format!("{}.chat.json", id),
            );
            fs::read_to_string(&chats_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        },
    })
}

fn write_note_file(
    _workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
    document: &NoteDocument,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }

    if !document.annotations.is_empty() {
        let annotations = &document.annotations;
        let annotations_dir = workspace_data_dir.join("annotations");
        fs::create_dir_all(&annotations_dir)?;
        let annotations_path = annotations_dir.join(format!("{}.annotations.json", document.id));
        let tmp_ann_path = annotations_dir.join(format!("{}.annotations.tmp", document.id));
        fs::write(&tmp_ann_path, serde_json::to_string(annotations)?)?;
        fs::rename(&tmp_ann_path, &annotations_path)?;
    }

    if !document.chat_history.is_empty() {
        let chats_dir = workspace_data_dir.join("chats");
        fs::create_dir_all(&chats_dir)?;
        let chats_path = chats_dir.join(format!("{}.chat.json", document.id));
        let tmp_chat_path = chats_dir.join(format!("{}.chat.tmp", document.id));
        fs::write(
            &tmp_chat_path,
            serde_json::to_string(&document.chat_history)?,
        )?;
        fs::rename(&tmp_chat_path, &chats_path)?;
    }

    if path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        == Some("pdf".to_string())
    {
        return Ok(());
    }

    let frontmatter = Frontmatter {
        id: Some(document.id.clone()),
        title: Some(document.title.clone()),
        tags: Some(document.tags.clone()),
        created_at: Some(document.created_at.clone()),
        updated_at: Some(document.updated_at.clone()),
        source_pdf: document.source_pdf.clone(),
    };
    let yaml = serde_yaml::to_string(&frontmatter)?.trim().to_string();
    let rendered = format!("---\n{yaml}\n---\n\n{}", document.body.trim_end());
    let temp_path = path.with_extension("tmp");

    fs::write(&temp_path, rendered)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to move {} to {}",
            temp_path.display(),
            path.display()
        )
    })
}

async fn rebuild_lancedb(index_dir: &Path, notes: &[IndexedNote]) -> Result<Table> {
    if index_dir.exists() {
        fs::remove_dir_all(index_dir)
            .with_context(|| format!("failed to clear index dir {}", index_dir.display()))?;
    }
    fs::create_dir_all(index_dir)
        .with_context(|| format!("failed to create index dir {}", index_dir.display()))?;

    let connection = open_database(index_dir).await?;
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIM,
            ),
            true,
        ),
    ]));

    if notes.is_empty() {
        return connection
            .create_empty_table(TABLE_NAME, schema)
            .execute()
            .await
            .context("failed to create empty lancedb table");
    }

    let ids = StringArray::from_iter_values(notes.iter().map(|note| note.document.id.as_str()));
    let titles =
        StringArray::from_iter_values(notes.iter().map(|note| note.document.title.as_str()));
    let paths = StringArray::from_iter_values(
        notes
            .iter()
            .map(|note| note.document.relative_path.as_str()),
    );
    let updated_at =
        StringArray::from_iter_values(notes.iter().map(|note| note.document.updated_at.as_str()));
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        notes
            .iter()
            .map(|note| Some(note.vector.iter().copied().map(Some).collect::<Vec<_>>())),
        EMBEDDING_DIM,
    );

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids) as ArrayRef,
            Arc::new(titles) as ArrayRef,
            Arc::new(paths) as ArrayRef,
            Arc::new(updated_at) as ArrayRef,
            Arc::new(vectors) as ArrayRef,
        ],
    )?;
    let data = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

    connection
        .create_table(TABLE_NAME, Box::new(data))
        .execute()
        .await
        .context("failed to create lancedb table")
}

async fn open_database(index_dir: &Path) -> Result<Connection> {
    connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open lancedb")
}

fn summarize(document: &NoteDocument) -> NoteSummary {
    NoteSummary {
        id: document.id.clone(),
        title: document.title.clone(),
        tags: document.tags.clone(),
        folder: folder_from_relative_path(&document.relative_path),
        excerpt: excerpt(&document.body),
        relative_path: document.relative_path.clone(),
        created_at: document.created_at.clone(),
        updated_at: document.updated_at.clone(),
        source_pdf: document.source_pdf.clone(),
        backlinks: document.backlinks.clone(),
    }
}

fn build_library_facets<'a>(documents: impl Iterator<Item = &'a NoteDocument>) -> LibraryFacets {
    let mut folders = Vec::new();
    let mut tags = Vec::new();
    for document in documents {
        let folder = folder_from_relative_path(&document.relative_path);
        if !folders.contains(&folder) {
            folders.push(folder);
        }
        for tag in &document.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
    }
    folders.sort();
    tags.sort();
    LibraryFacets { folders, tags }
}

fn split_frontmatter(raw: &str) -> (Option<String>, String) {
    if !raw.starts_with("---\n") {
        return (None, raw.to_string());
    }

    let remaining = &raw[4..];
    if let Some(index) = remaining.find("\n---\n") {
        let frontmatter = remaining[..index].to_string();
        let body = remaining[index + 5..].trim_start_matches('\n').to_string();
        return (Some(frontmatter), body);
    }

    (None, raw.to_string())
}

fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .find_map(|line| line.strip_prefix("# ").map(str::trim).map(str::to_string))
        .filter(|title| !title.is_empty())
}

fn default_title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("Untitled note")
        .replace("--", " ")
}

fn relative_to_workspace(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn folder_from_relative_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .parent()
        .and_then(|parent| parent.to_str())
        .map(|value| value.replace('\\', "/"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Root".into())
}

fn folder_to_relative_path(folder: &str) -> PathBuf {
    if folder == "Root" || folder.trim().is_empty() {
        PathBuf::new()
    } else {
        PathBuf::from(folder.replace('/', std::path::MAIN_SEPARATOR_STR))
    }
}

fn sanitize_relative_folder(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("root") {
        return Ok("Root".into());
    }

    let normalized = trimmed.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() || normalized.split('/').any(|segment| segment == "..") {
        return Err(anyhow!("folder must stay inside the workspace"));
    }

    Ok(normalized
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/"))
}

fn normalized_custom_order(
    current_order: &[String],
    notes: &HashMap<String, IndexedNote>,
) -> Vec<String> {
    let mut ordered = current_order
        .iter()
        .filter(|id| notes.contains_key(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut missing = notes
        .values()
        .map(|note| note.document.clone())
        .collect::<Vec<_>>();
    missing.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    for note in missing {
        if !ordered.contains(&note.id) {
            ordered.push(note.id);
        }
    }
    ordered
}

fn sort_summaries_by_custom_order(
    mut notes: Vec<NoteSummary>,
    custom_order: &[String],
) -> Vec<NoteSummary> {
    let order_map = custom_order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect::<HashMap<_, _>>();
    notes.sort_by(|left, right| {
        order_map
            .get(&left.id)
            .cmp(&order_map.get(&right.id))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    notes
}

fn timestamp_now() -> String {
    Utc::now().to_rfc3339()
}

fn get_file_timestamps(path: &Path) -> (String, String) {
    let fallback = timestamp_now();
    if let Ok(metadata) = std::fs::metadata(path) {
        let created = metadata.created().unwrap_or_else(|_| std::time::SystemTime::now());
        let modified = metadata.modified().unwrap_or_else(|_| std::time::SystemTime::now());
        let created_dt: chrono::DateTime<Utc> = created.into();
        let modified_dt: chrono::DateTime<Utc> = modified.into();
        (created_dt.to_rfc3339(), modified_dt.to_rfc3339())
    } else {
        (fallback.clone(), fallback)
    }
}

fn excerpt(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() > 400 {
        format!("{}...", &flat[..400])
    } else {
        flat
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn hashed_embedding(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0_f32; EMBEDDING_DIM as usize];
    for token in tokenize(text) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hasher);
        let hash = hasher.finish();
        let index = (hash as usize) % vector.len();
        let sign = if (hash >> 8) & 1 == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
    }
    normalize(&mut vector);
    vector
}

fn normalize(vector: &mut [f32]) {
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if magnitude > 0.0 {
        for value in vector.iter_mut() {
            *value /= magnitude;
        }
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum::<f32>()
        .max(0.0)
}

fn slugify(input: &str) -> String {
    let raw = input.trim();
    let title = if raw.is_empty() {
        Cow::Borrowed("untitled-note")
    } else {
        Cow::Borrowed(raw)
    };
    let mut slug = String::new();
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if (character.is_whitespace() || character == '-' || character == '_')
            && !slug.ends_with('-')
        {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "untitled-note".into()
    } else if is_reserved_windows_name(&slug) {
        format!("{slug}-note")
    } else {
        slug
    }
}

fn is_reserved_windows_name(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn unique_note_path(workspace: &Path, file_name: &str) -> PathBuf {
    let mut candidate = workspace.join(file_name);
    if !candidate.exists() {
        return candidate;
    }

    let stem = Path::new(file_name)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("note");
    let extension = Path::new(file_name)
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("md");

    for index in 2..=9_999 {
        candidate = workspace.join(format!("{stem}-{index}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    workspace.join(format!("{stem}-{}.{}", Uuid::new_v4(), extension))
}

fn is_note_file(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(|extension| {
            extension.eq_ignore_ascii_case("md")
                || extension.eq_ignore_ascii_case("pdf")
                || extension.eq_ignore_ascii_case("epub")
                || extension.eq_ignore_ascii_case("tex")
                || extension.eq_ignore_ascii_case("ipynb")
        })
        .unwrap_or(false)
}

fn stable_id_from_path(path: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    format!("legacy-{:x}", hasher.finish())
}

struct ParsedLink {
    target: String,
    block: Option<String>,
    start_index: usize,
    end_index: usize,
}

fn extract_links(body: &str) -> Vec<ParsedLink> {
    let mut links = Vec::new();

    // Parse [[Wikilinks]]
    if let Ok(re) = regex::Regex::new(r"\[\[([^\]]+)\]\]") {
        for cap in re.captures_iter(body) {
            if let Some(m) = cap.get(0) {
                let inner = cap.get(1).unwrap().as_str().to_string();
                let (target, block) = if let Some(idx) = inner.find('#') {
                    (
                        inner[..idx].trim().to_string(),
                        Some(inner[idx + 1..].trim().to_string()),
                    )
                } else {
                    (inner.trim().to_string(), None)
                };
                links.push(ParsedLink {
                    target,
                    block,
                    start_index: m.start(),
                    end_index: m.end(),
                });
            }
        }
    }

    // Parse standard markdown note links: [Text](url)
    // Vditor might rewrite `/notes/id` to `http://localhost:1420/notes/id`
    if let Ok(re) = regex::Regex::new(r"\[.*?\]\(([^)]+)\)") {
        for cap in re.captures_iter(body) {
            if let Some(m) = cap.get(0) {
                let inner = cap.get(1).unwrap().as_str().to_string();

                let (url_part, block) = if let Some(idx) = inner.find('#') {
                    (&inner[..idx], Some(inner[idx + 1..].trim().to_string()))
                } else {
                    (inner.as_str(), None)
                };

                let url_part = url_part.trim();

                // Extract the last segment (e.g., UUID from /notes/uuid or http://.../notes/uuid)
                let target = if let Some(idx) = url_part.rfind('/') {
                    url_part[idx + 1..].to_string()
                } else if url_part.starts_with("note:") {
                    url_part[5..].to_string()
                } else {
                    url_part.to_string()
                };

                links.push(ParsedLink {
                    target,
                    block,
                    start_index: m.start(),
                    end_index: m.end(),
                });
            }
        }
    }

    links
}

fn excerpt_around(body: &str, start: usize, end: usize) -> String {
    let context_chars = 40;
    let pre_start = start.saturating_sub(context_chars);
    let post_end = std::cmp::min(body.len(), end + context_chars);

    let mut excerpt = String::new();
    if pre_start > 0 {
        excerpt.push_str("...");
    }
    excerpt.push_str(&body[pre_start..post_end].replace('\n', " "));
    if post_end < body.len() {
        excerpt.push_str("...");
    }
    excerpt
}

fn sidecar_path(
    workspace: &Path,
    workspace_data_dir: &Path,
    kind: &str,
    file_name: &str,
) -> PathBuf {
    let app_path = workspace_data_dir.join(kind).join(file_name);
    if app_path.exists() {
        return app_path;
    }

    workspace.join(".myelin").join(kind).join(file_name)
}

fn workspace_storage_key(workspace: &Path) -> String {
    slugify(&workspace.to_string_lossy())
}

fn default_provider_status(app_data_dir: &Path) -> ProviderStatus {
    if let Ok(info) = llama_server::inspect_provider(app_data_dir) {
        return ProviderStatus {
            active_provider: "llama.cpp".into(),
            available_providers: vec!["llama.cpp".into()],
            healthy: info.healthy,
            detail: info.detail,
            config: Some(info.config),
            resolved: info.resolved,
            active_backend: info.selected_backend,
            nvidia_detected: info.nvidia_detected,
            gpu_available: info.gpu_available,
            gpus: info.gpus,
            installed_backends: info.installed_backends,
        };
    }

    ProviderStatus {
        active_provider: "llama.cpp".into(),
        available_providers: vec!["llama.cpp".into()],
        healthy: false,
        detail: "Choose a .gguf model and llama-server executable in Settings.".into(),
        config: None,
        resolved: None,
        active_backend: None,
        nvidia_detected: llama_server::detect_nvidia(),
        gpu_available: llama_server::gpu_available(),
        gpus: llama_server::detect_gpus().0,
        installed_backends: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{hashed_embedding, slugify, split_frontmatter, tokenize};

    #[test]
    fn slugify_avoids_reserved_names() {
        assert_eq!(slugify("CON"), "con-note");
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn frontmatter_split_handles_markdown() {
        let raw = "---\ntitle: Test\n---\n\n# Hello";
        let (frontmatter, body) = split_frontmatter(raw);
        assert!(frontmatter.is_some());
        assert_eq!(body, "# Hello");
    }

    #[test]
    fn embedding_is_stable() {
        assert_eq!(
            hashed_embedding("alpha beta"),
            hashed_embedding("alpha beta")
        );
        assert_eq!(tokenize("Alpha, beta!").len(), 2);
    }
}
