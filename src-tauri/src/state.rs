use crate::models::{
    AppSnapshot, Backlink, IndexState, LibraryFacets, NoteDocument, NoteSummary, ProviderStatus,
    SearchResponse, SearchResult,
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
const SETTINGS_FILE_NAME: &str = "settings.json";
const TABLE_NAME: &str = "notes";

#[derive(Clone)]
pub struct AppState {
    handle: AppHandle,
    inner: Arc<InnerState>,
}

struct InnerState {
    app_data_dir: PathBuf,
    runtime: RwLock<RuntimeState>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    index_lock: AsyncMutex<()>,
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
            }),
        })
    }

    pub async fn bootstrap(&self) -> Result<AppSnapshot> {
        let workspace = self.inner.runtime.read().workspace_path.clone();
        if let Some(workspace) = workspace {
            self.start_watcher(&workspace)?;
            self.reindex_workspace(workspace).await?;
        }
        Ok(self.snapshot())
    }

    pub async fn set_workspace(&self, workspace_path: String) -> Result<AppSnapshot> {
        let workspace = PathBuf::from(workspace_path);
        fs::create_dir_all(&workspace)
            .with_context(|| format!("failed to create workspace at {}", workspace.display()))?;

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

    pub async fn create_note(&self, title: String) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let now = timestamp_now();
        let id = Uuid::new_v4().to_string();
        let safe_slug = slugify(&title);
        let file_name = format!("{safe_slug}--{}.md", &id[..8]);
        let path = unique_note_path(&workspace, &file_name);
        let relative_path = relative_to_workspace(&workspace, &path);
        let document = NoteDocument {
            id,
            title: if title.trim().is_empty() {
                "Untitled note".into()
            } else {
                title.trim().into()
            },
            tags: Vec::new(),
            body: String::new(),
            relative_path,
            created_at: now.clone(),
            updated_at: now,
            backlinks: Vec::new(),
        };

        write_note_file(&path, &document)?;
        self.reindex_workspace(workspace).await?;
        self.load_note(document.id).await
    }

    pub async fn load_note(&self, note_id: String) -> Result<NoteDocument> {
        let runtime = self.inner.runtime.read();
        runtime
            .notes
            .get(&note_id)
            .map(|note| note.document.clone())
            .ok_or_else(|| anyhow!("note not found"))
    }

    pub async fn save_note(
        &self,
        note_id: String,
        title: String,
        tags: Vec<String>,
        body: String,
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

        let updated = NoteDocument {
            id: existing.document.id,
            title: title.trim().to_string(),
            tags: tags
                .into_iter()
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect(),
            body,
            relative_path: existing.document.relative_path.clone(),
            created_at: existing.document.created_at,
            updated_at: timestamp_now(),
            backlinks: existing.document.backlinks,
        };

        let path = workspace.join(&updated.relative_path);
        write_note_file(&path, &updated)?;
        self.reindex_workspace(workspace).await?;
        self.load_note(note_id).await
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
            backlinks: source.document.backlinks,
        };

        write_note_file(&path, &document)?;
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

    pub async fn provider_status(&self) -> Result<ProviderStatus> {
        Ok(default_provider_status())
    }

    pub async fn rebuild_index(&self) -> Result<AppSnapshot> {
        let workspace = self.require_workspace()?;
        self.reindex_workspace(workspace).await?;
        Ok(self.snapshot())
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
            provider_status: default_provider_status(),
            index_state: runtime.index_state.clone(),
        }
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

                let is_markdown = event.paths.iter().any(|path| is_markdown_file(path));
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
        let mut notes = read_workspace_notes(&workspace)?;
        
        let mut backlinks_map: HashMap<String, Vec<Backlink>> = HashMap::new();
        for note in &notes {
            let links = extract_links(&note.document.body);
            for link in links {
                let target_note = notes.iter().find(|n| n.document.title == link.target || n.document.id == link.target);
                if let Some(target) = target_note {
                    let backlink = Backlink {
                        source_id: note.document.id.clone(),
                        source_title: note.document.title.clone(),
                        target_block: link.block.clone(),
                        context_excerpt: excerpt_around(&note.document.body, link.start_index, link.end_index),
                    };
                    backlinks_map.entry(target.document.id.clone()).or_default().push(backlink);
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

fn read_workspace_notes(workspace: &Path) -> Result<Vec<IndexedNote>> {
    let mut notes = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_file() && is_markdown_file(entry.path()) {
            if let Ok(document) = parse_note_file(workspace, entry.path()) {
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

    notes.sort_by(|left, right| right.document.updated_at.cmp(&left.document.updated_at));
    Ok(notes)
}

fn parse_note_file(workspace: &Path, path: &Path) -> Result<NoteDocument> {
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
    let created_at = metadata.created_at.unwrap_or_else(timestamp_now);
    let updated_at = metadata.updated_at.unwrap_or_else(timestamp_now);

    Ok(NoteDocument {
        id: metadata.id.unwrap_or_else(|| stable_id_from_path(path)),
        title,
        tags: metadata.tags.unwrap_or_default(),
        body,
        relative_path: relative_to_workspace(workspace, path),
        created_at,
        updated_at,
        backlinks: Vec::new(),
    })
}

fn write_note_file(path: &Path, document: &NoteDocument) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }

    let frontmatter = Frontmatter {
        id: Some(document.id.clone()),
        title: Some(document.title.clone()),
        tags: Some(document.tags.clone()),
        created_at: Some(document.created_at.clone()),
        updated_at: Some(document.updated_at.clone()),
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

fn excerpt(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() > 180 {
        format!("{}...", &flat[..180])
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

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.eq_ignore_ascii_case("md"))
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
                    (inner[..idx].trim().to_string(), Some(inner[idx+1..].trim().to_string()))
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
                    (&inner[..idx], Some(inner[idx+1..].trim().to_string()))
                } else {
                    (inner.as_str(), None)
                };
                
                let url_part = url_part.trim();
                
                // Extract the last segment (e.g., UUID from /notes/uuid or http://.../notes/uuid)
                let target = if let Some(idx) = url_part.rfind('/') {
                    url_part[idx+1..].to_string()
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

fn default_provider_status() -> ProviderStatus {
    ProviderStatus {
        active_provider: "portable-local-hash".into(),
        available_providers: vec!["portable-local-hash".into(), "future-ollama".into(), "future-openai".into()],
        healthy: true,
        detail: "Provider surface is ready. Semantic indexing currently uses a portable local embedding fallback until a real provider is configured.".into(),
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
