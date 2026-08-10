pub(crate) use crate::llama_server::{self, ManagedLlamaServer};
pub(crate) use crate::models::{
    AppSnapshot, Backlink, ChatTool, IndexState, LibraryFacets, NoteDocument, NoteSummary,
    ProviderStatus, SearchResponse, SearchResult, Task,
};
pub(crate) use crate::sidecar::ManagedSidecar;
pub(crate) use anyhow::{anyhow, Context, Result};
pub(crate) use arrow_array::types::Float32Type;
pub(crate) use arrow_array::{ArrayRef, FixedSizeListArray, RecordBatch, RecordBatchIterator, StringArray};
pub(crate) use arrow_schema::{DataType, Field, Schema};
pub(crate) use chrono::Utc;
pub(crate) use lancedb::connection::Connection;
pub(crate) use lancedb::{connect, Table};
pub(crate) use notify::{recommended_watcher, RecommendedWatcher, RecursiveMode, Watcher};
pub(crate) use parking_lot::{Mutex, RwLock};
pub(crate) use reqwest::Client;
pub(crate) use rig_core::completion::{CompletionError, Prompt, PromptError};
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use sha2::{Digest, Sha256};
pub(crate) use std::borrow::Cow;
pub(crate) use std::collections::HashMap;
pub(crate) use std::ffi::OsStr;
pub(crate) use std::fs;
pub(crate) use std::hash::{Hash, Hasher};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::Arc;
pub(crate) use tauri::{async_runtime::Mutex as AsyncMutex, AppHandle, Emitter, Manager};
pub(crate) use uuid::Uuid;

// GTE-small width. Notes use real embeddings when an embed model is

use super::*;

pub(crate) fn is_hidden_or_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.depth() > 0 && name.starts_with('.') {
        return true;
    }
    name == "node_modules" || name == "target" || name == "dist" || name == "build"
}

pub(crate) fn read_workspace_notes(workspace: &Path, workspace_data_dir: &Path) -> Result<Vec<IndexedNote>> {
    let mut notes = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e))
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_file() && is_note_file(entry.path()) {
            if let Some(extension) = entry.path().extension().and_then(std::ffi::OsStr::to_str) {
                let doc_result = if extension.eq_ignore_ascii_case("pdf")
                    || extension.eq_ignore_ascii_case("epub")
                {
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

pub(crate) fn parse_pdf_file(
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

pub(crate) fn frontmatter_from_document(document: &NoteDocument) -> Frontmatter {
    Frontmatter {
        id: Some(document.id.clone()),
        title: Some(document.title.clone()),
        tags: Some(document.tags.clone()),
        created_at: Some(document.created_at.clone()),
        updated_at: Some(document.updated_at.clone()),
        source_pdf: document.source_pdf.clone(),
    }
}

pub(crate) fn frontmatter_has_myelin_metadata(metadata: &Frontmatter) -> bool {
    metadata.id.is_some()
        || metadata.title.is_some()
        || metadata.tags.is_some()
        || metadata.created_at.is_some()
        || metadata.updated_at.is_some()
        || metadata.source_pdf.is_some()
}

/// Detect the legacy representation without mistaking ordinary TeX that starts
/// with horizontal-rule-like text for a Myelin wrapper.
pub(crate) fn split_legacy_native_frontmatter(raw: &str) -> (Option<Frontmatter>, String, bool) {
    let (frontmatter, body) = split_frontmatter(raw);
    let metadata = frontmatter
        .as_deref()
        .and_then(|value| serde_yaml::from_str::<Frontmatter>(value).ok())
        .filter(frontmatter_has_myelin_metadata);
    if metadata.is_some() {
        (metadata, body, true)
    } else {
        (None, raw.to_string(), false)
    }
}

pub(crate) fn initial_note_body(path: &Path) -> String {
    if extension_is(path, "ipynb") {
        EMPTY_IPYNB.to_string()
    } else {
        String::new()
    }
}

pub(crate) fn duplicate_note_extension(path: &Path) -> &str {
    if is_native_text_file(path) {
        path.extension().and_then(OsStr::to_str).unwrap_or("md")
    } else {
        "md"
    }
}

pub(crate) fn validate_native_body(path: &Path, body: &str) -> Result<()> {
    if extension_is(path, "ipynb") {
        let notebook = serde_json::from_str::<serde_json::Value>(body).with_context(|| {
            format!(
                "notebook {} must contain valid JSON before it can be saved",
                path.display()
            )
        })?;
        let object = notebook.as_object().ok_or_else(|| {
            anyhow!(
                "notebook {} must be a JSON object",
                path.display()
            )
        })?;
        if !object.get("cells").is_some_and(serde_json::Value::is_array) {
            return Err(anyhow!(
                "notebook {} must contain a cells array",
                path.display()
            ));
        }
        if !object
            .get("nbformat")
            .is_some_and(serde_json::Value::is_number)
        {
            return Err(anyhow!(
                "notebook {} must contain a numeric nbformat",
                path.display()
            ));
        }
        if !object
            .get("nbformat_minor")
            .is_some_and(serde_json::Value::is_number)
        {
            return Err(anyhow!(
                "notebook {} must contain a numeric nbformat_minor",
                path.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn write_raw_document(path: &Path, contents: &str) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("document");
    let temp_path = path.with_file_name(format!(".{file_name}.myelin.tmp"));
    fs::write(&temp_path, contents)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to move {} to {}",
            temp_path.display(),
            path.display()
        )
    })
}

pub(crate) fn native_metadata_file_name(workspace: &Path, path: &Path) -> String {
    let relative_path = relative_to_workspace(workspace, path);
    let mut hasher = Sha256::new();
    hasher.update(relative_path.as_bytes());
    format!("{:x}.metadata.json", hasher.finalize())
}

pub(crate) fn native_metadata_app_path(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) -> PathBuf {
    workspace_data_dir
        .join(NATIVE_METADATA_DIR)
        .join(native_metadata_file_name(workspace, path))
}

pub(crate) fn read_native_metadata_sidecar(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) -> Option<Frontmatter> {
    let file_name = native_metadata_file_name(workspace, path);
    let metadata_path = sidecar_path(
        workspace,
        workspace_data_dir,
        NATIVE_METADATA_DIR,
        &file_name,
    );
    let sidecar = fs::read_to_string(metadata_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<NativeMetadataSidecar>(&raw).ok())?;
    let relative_path = relative_to_workspace(workspace, path);
    (sidecar.relative_path == relative_path).then_some(sidecar.metadata)
}

pub(crate) fn write_native_metadata_sidecar(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
    document: &NoteDocument,
) -> Result<()> {
    if !is_native_text_file(path) {
        return Ok(());
    }
    let metadata_path = native_metadata_app_path(workspace, workspace_data_dir, path);
    let metadata_dir = metadata_path
        .parent()
        .ok_or_else(|| anyhow!("native metadata path has no parent"))?;
    fs::create_dir_all(metadata_dir).with_context(|| {
        format!(
            "failed to create native metadata directory {}",
            metadata_dir.display()
        )
    })?;
    let sidecar = NativeMetadataSidecar {
        metadata: frontmatter_from_document(document),
        relative_path: relative_to_workspace(workspace, path),
    };
    let temp_path = metadata_path.with_extension("tmp");
    fs::write(&temp_path, serde_json::to_vec_pretty(&sidecar)?)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    fs::rename(&temp_path, &metadata_path).with_context(|| {
        format!(
            "failed to move {} to {}",
            temp_path.display(),
            metadata_path.display()
        )
    })
}

pub(crate) fn remove_native_metadata_sidecar(workspace: &Path, workspace_data_dir: &Path, path: &Path) {
    if !is_native_text_file(path) {
        return;
    }
    let file_name = native_metadata_file_name(workspace, path);
    let app_path = native_metadata_app_path(workspace, workspace_data_dir, path);
    let _ = fs::remove_file(&app_path);
    let _ = fs::remove_file(app_path.with_extension("tmp"));

    // Remove a portable legacy fallback too if one exists. Current writes never
    // place metadata inside the workspace.
    let legacy_path = workspace
        .join(".myelin")
        .join(NATIVE_METADATA_DIR)
        .join(file_name);
    let _ = fs::remove_file(&legacy_path);
    let _ = fs::remove_file(legacy_path.with_extension("tmp"));
}

pub(crate) fn parse_note_file(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) -> Result<NoteDocument> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let native = is_native_text_file(path);
    let (legacy_metadata, body, legacy_wrapped) = if native {
        split_legacy_native_frontmatter(&raw)
    } else {
        let (frontmatter, body) = split_frontmatter(&raw);
        let metadata = frontmatter
            .as_deref()
            .and_then(|frontmatter| serde_yaml::from_str::<Frontmatter>(frontmatter).ok());
        (metadata, body, false)
    };
    validate_native_body(path, &body)?;

    let stored_metadata = if native {
        read_native_metadata_sidecar(workspace, workspace_data_dir, path)
    } else {
        None
    };
    let metadata = stored_metadata
        .clone()
        .or(legacy_metadata)
        .unwrap_or_default();

    let title = metadata
        .title
        .clone()
        .unwrap_or_else(|| first_heading(&body).unwrap_or_else(|| default_title_from_path(path)));

    let (file_created, file_updated) = get_file_timestamps(path);
    let created_at = metadata.created_at.clone().unwrap_or(file_created);
    let updated_at = metadata.updated_at.clone().unwrap_or(file_updated);
    let id = metadata.id.clone().unwrap_or_else(|| stable_id_from_path(path));

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

    let document = NoteDocument {
        id: id.clone(),
        title,
        tags: metadata.tags.clone().unwrap_or_default(),
        body,
        relative_path: relative_to_workspace(workspace, path),
        created_at,
        updated_at,
        source_pdf: metadata.source_pdf.clone(),
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
    };

    if native {
        let app_sidecar = native_metadata_app_path(workspace, workspace_data_dir, path);
        if stored_metadata.is_none() || !app_sidecar.exists() || legacy_wrapped {
            write_native_metadata_sidecar(workspace, workspace_data_dir, path, &document)?;
        }
        if legacy_wrapped {
            // Older Myelin builds wrote YAML ahead of TeX/JSON. Once its fields
            // have safely reached app data, atomically replace the legacy file
            // with its native body so other TeX/Jupyter tools can open it.
            write_raw_document(path, &document.body)?;
        }
    }

    Ok(document)
}

pub(crate) fn write_note_file(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
    document: &NoteDocument,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    if !is_binary_document(path) {
        validate_native_body(path, &document.body)?;
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

    // Viewer-only document formats are never rewritten. Annotations and chat
    // still live in their dedicated app-data sidecars above.
    if is_binary_document(path) {
        return Ok(());
    }

    if is_native_text_file(path) {
        write_native_metadata_sidecar(workspace, workspace_data_dir, path, document)?;
        return write_raw_document(path, &document.body);
    }

    let frontmatter = frontmatter_from_document(document);
    let yaml = serde_yaml::to_string(&frontmatter)?.trim().to_string();
    let rendered = format!("---\n{yaml}\n---\n\n{}", document.body.trim_end());
    write_raw_document(path, &rendered)
}

pub(crate) async fn rebuild_lancedb(index_dir: &Path, notes: &[IndexedNote]) -> Result<Table> {
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

pub(crate) async fn open_database(index_dir: &Path) -> Result<Connection> {
    connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open lancedb")
}

pub(crate) fn summarize(document: &NoteDocument) -> NoteSummary {
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

pub(crate) fn build_library_facets<'a>(documents: impl Iterator<Item = &'a NoteDocument>) -> LibraryFacets {
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

pub(crate) fn split_frontmatter(raw: &str) -> (Option<String>, String) {
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

pub(crate) fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .find_map(|line| line.strip_prefix("# ").map(str::trim).map(str::to_string))
        .filter(|title| !title.is_empty())
}

pub(crate) fn default_title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("Untitled note")
        .replace("--", " ")
}

pub(crate) fn relative_to_workspace(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Directory holding a task's file for a given notebook (None = workspace root).
pub(crate) fn task_dir_for(workspace: &Path, notebook: Option<&str>) -> PathBuf {
    match notebook.map(str::trim).filter(|s| !s.is_empty()) {
        Some(nb) => workspace.join(nb).join("tasks"),
        None => workspace.join("tasks"),
    }
}

/// True for a `.../tasks/<name>.json` file (a task file we own).
pub(crate) fn is_task_file(path: &Path) -> bool {
    let is_json = path
        .extension()
        .and_then(OsStr::to_str)
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    let in_tasks_dir = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(OsStr::to_str)
        .map(|n| n == "tasks")
        .unwrap_or(false);
    is_json && in_tasks_dir
}

/// Notebook a task file belongs to, derived from its path. None = root tasks.
pub(crate) fn notebook_from_task_path(workspace: &Path, path: &Path) -> Option<String> {
    let holder = path.parent()?.parent()?; // the folder that contains the `tasks` dir
    let rel = relative_to_workspace(workspace, holder);
    if rel.is_empty() || rel == "." {
        None
    } else {
        Some(rel)
    }
}

/// Delete every `<id>.json` task file across the workspace except `keep`.
pub(crate) fn remove_task_files(workspace: &Path, id: &str, keep: Option<&Path>) {
    let target = format!("{id}.json");
    for entry in walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e))
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if is_task_file(path)
            && path.file_name().and_then(OsStr::to_str) == Some(target.as_str())
            && keep != Some(path)
        {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) fn validate_task_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(anyhow!("invalid task id"));
    }
    Ok(())
}

/// Reject notebook paths that could escape the workspace (absolute, `..`, roots).
pub(crate) fn validate_relative_dir(dir: &str) -> Result<()> {
    let p = Path::new(dir);
    let bad = p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
            )
        });
    if bad {
        return Err(anyhow!("invalid notebook path"));
    }
    Ok(())
}

pub(crate) fn folder_from_relative_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .parent()
        .and_then(|parent| parent.to_str())
        .map(|value| value.replace('\\', "/"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Root".into())
}

pub(crate) fn folder_to_relative_path(folder: &str) -> PathBuf {
    if folder == "Root" || folder.trim().is_empty() {
        PathBuf::new()
    } else {
        PathBuf::from(folder.replace('/', std::path::MAIN_SEPARATOR_STR))
    }
}

pub(crate) fn sanitize_relative_folder(input: &str) -> Result<String> {
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

pub(crate) fn normalized_custom_order(
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

pub(crate) fn sort_summaries_by_custom_order(
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

pub(crate) fn timestamp_now() -> String {
    Utc::now().to_rfc3339()
}
