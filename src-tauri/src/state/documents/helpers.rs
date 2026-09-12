use super::*;
pub(crate) use crate::models::*;
pub(crate) use anyhow::{anyhow, Context, Result};
pub(crate) use arrow_schema::DataType;
pub(crate) use chrono::Utc;
pub(crate) use sha2::{Digest, Sha256};
pub(crate) use std::collections::HashMap;
pub(crate) use std::ffi::OsStr;
pub(crate) use std::fs;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::Arc;
pub(crate) fn is_hidden_or_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.depth() > 0 && name.starts_with('.') {
        return true;
    }
    name == "node_modules" || name == "target" || name == "dist" || name == "build"
}
pub(crate) struct WorkspaceScanResult {
    pub(crate) notes: Vec<IndexedNote>,
    pub(crate) issues: Vec<StorageIssue>,
}
pub(crate) fn read_workspace_notes(
    workspace: &Path,
    workspace_data_dir: &Path,
) -> Result<WorkspaceScanResult> {
    let mut notes = Vec::new();
    let mut issues = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                issues.push(StorageIssue {
                    code: "workspace-traversal".into(),
                    severity: "error".into(),
                    path: error.path().map(|path| path.display().to_string()),
                    message: "A workspace file or directory could not be scanned.".into(),
                    recoverable: true,
                });
                continue;
            }
        };
        if entry.file_type().is_file() && is_note_file(entry.path()) {
            if let Some(extension) = entry.path().extension().and_then(std::ffi::OsStr::to_str) {
                let doc_result = if extension.eq_ignore_ascii_case("pdf")
                    || extension.eq_ignore_ascii_case("epub")
                {
                    parse_pdf_file(workspace, workspace_data_dir, entry.path())
                } else {
                    parse_note_file(workspace, workspace_data_dir, entry.path())
                };
                match doc_result {
                    Ok(document) => {
                        if let Some(existing) = notes
                            .iter()
                            .find(|note: &&IndexedNote| note.document.id == document.id)
                        {
                            issues.push(StorageIssue {
                                code: "duplicate-note-id".into(),
                                severity: "error".into(),
                                path: Some(entry.path().display().to_string()),
                                message: format!(
                                    "Note ID conflicts with {}.",
                                    existing.document.relative_path
                                ),
                                recoverable: false,
                            });
                            continue;
                        }
                        let vector = hashed_embedding(&format!(
                            "{}\n{}\n{}",
                            document.title,
                            document.tags.join(" "),
                            document.body
                        ));
                        notes.push(IndexedNote { document, vector });
                    }
                    Err(error) => issues.push(StorageIssue {
                        code: "note-parse".into(),
                        severity: "error".into(),
                        path: Some(entry.path().display().to_string()),
                        message: format!("This note could not be loaded: {error}"),
                        recoverable: true,
                    }),
                }
            }
        }
    }
    notes.sort_by(|left, right| {
        left.document
            .relative_path
            .cmp(&right.document.relative_path)
    });
    Ok(WorkspaceScanResult { notes, issues })
}
pub(crate) fn parse_pdf_file(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) -> Result<NoteDocument> {
    let stored_metadata = read_document_metadata_sidecar(workspace, workspace_data_dir, path)?;
    let fallback_title = default_title_from_path(path);
    let (created_at, updated_at) = get_file_timestamps(path);
    let id = stored_metadata
        .as_ref()
        .and_then(|metadata| metadata.id.clone())
        .unwrap_or_else(|| stable_id_from_path(path));
    let title = stored_metadata
        .as_ref()
        .and_then(|metadata| metadata.title.clone())
        .unwrap_or(fallback_title);
    let created_at = stored_metadata
        .as_ref()
        .and_then(|metadata| metadata.created_at.clone())
        .unwrap_or(created_at);
    let updated_at = stored_metadata
        .as_ref()
        .and_then(|metadata| metadata.updated_at.clone())
        .unwrap_or(updated_at);
    let annotations = {
        let annotations_path = sidecar_path(
            workspace,
            workspace_data_dir,
            "annotations",
            &format!("{}.annotations.json", id),
        );
        if annotations_path.exists() {
            let raw = fs::read_to_string(&annotations_path).with_context(|| {
                format!("failed to read annotations {}", annotations_path.display())
            })?;
            Some(serde_json::from_str(&raw).with_context(|| {
                format!("annotations are invalid at {}", annotations_path.display())
            })?)
        } else {
            None
        }
    };
    let source_annotations = {
        let source_annotations_path = sidecar_path(
            workspace,
            workspace_data_dir,
            "annotations",
            &format!("{}.source.annotations.json", id),
        );
        if source_annotations_path.exists() {
            let raw = fs::read_to_string(&source_annotations_path).with_context(|| {
                format!(
                    "failed to read source annotations {}",
                    source_annotations_path.display()
                )
            })?;
            Some(serde_json::from_str(&raw).with_context(|| {
                format!(
                    "source annotations are invalid at {}",
                    source_annotations_path.display()
                )
            })?)
        } else {
            None
        }
    };
    let document = NoteDocument {
        id: id.clone(),
        title,
        tags: Vec::new(),
        body: String::new(),
        relative_path: relative_to_workspace(workspace, path),
        created_at,
        updated_at,
        source_pdf: stored_metadata.and_then(|metadata| metadata.source_pdf),
        annotations: annotations.unwrap_or_default(),
        source_annotations: source_annotations.unwrap_or_default(),
        backlinks: Vec::new(),
        chat_history: {
            let chats_path = sidecar_path(
                workspace,
                workspace_data_dir,
                "chats",
                &format!("{}.chat.json", id),
            );
            if chats_path.exists() {
                let raw = fs::read_to_string(&chats_path).with_context(|| {
                    format!("failed to read chat history {}", chats_path.display())
                })?;
                serde_json::from_str(&raw).with_context(|| {
                    format!("chat history is invalid at {}", chats_path.display())
                })?
            } else {
                Vec::new()
            }
        },
    };
    if !native_metadata_app_path(workspace, workspace_data_dir, path).exists() {
        write_document_metadata_sidecar(workspace, workspace_data_dir, path, &document)?;
    }
    Ok(document)
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
        let object = notebook
            .as_object()
            .ok_or_else(|| anyhow!("notebook {} must be a JSON object", path.display()))?;
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
    crate::persistence::atomic_write(path, contents.as_bytes())
        .with_context(|| format!("failed to persist {}", path.display()))
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
) -> Result<Option<Frontmatter>> {
    let file_name = native_metadata_file_name(workspace, path);
    let metadata_path = sidecar_path(
        workspace,
        workspace_data_dir,
        NATIVE_METADATA_DIR,
        &file_name,
    );
    if !metadata_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&metadata_path)
        .with_context(|| format!("failed to read native metadata {}", metadata_path.display()))?;
    let sidecar = serde_json::from_str::<DocumentMetadataSidecar>(&raw)
        .with_context(|| format!("native metadata is invalid at {}", metadata_path.display()))?;
    let relative_path = relative_to_workspace(workspace, path);
    if sidecar.relative_path != relative_path {
        return Err(anyhow!(
            "native metadata path does not match {}",
            path.display()
        ));
    }
    Ok(Some(sidecar.metadata))
}
pub(crate) fn read_document_metadata_sidecar(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) -> Result<Option<Frontmatter>> {
    read_native_metadata_sidecar(workspace, workspace_data_dir, path)
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
        schema_version: 2,
        metadata: frontmatter_from_document(document),
        relative_path: relative_to_workspace(workspace, path),
    };
    crate::persistence::atomic_write_json(&metadata_path, &sidecar).with_context(|| {
        format!(
            "failed to persist native metadata {}",
            metadata_path.display()
        )
    })
}
pub(crate) fn write_document_metadata_sidecar(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
    document: &NoteDocument,
) -> Result<()> {
    if !is_metadata_document(path) {
        return Ok(());
    }
    let metadata_path = native_metadata_app_path(workspace, workspace_data_dir, path);
    let sidecar = DocumentMetadataSidecar {
        schema_version: 2,
        metadata: frontmatter_from_document(document),
        relative_path: relative_to_workspace(workspace, path),
    };
    crate::persistence::atomic_write_json(&metadata_path, &sidecar)
}
pub(crate) fn is_metadata_document(path: &Path) -> bool {
    is_native_text_file(path) || is_binary_document(path)
}
#[cfg(test)]
pub(crate) fn remove_native_metadata_sidecar(
    workspace: &Path,
    workspace_data_dir: &Path,
    path: &Path,
) {
    if !is_native_text_file(path) {
        return;
    }
    let file_name = native_metadata_file_name(workspace, path);
    let app_path = native_metadata_app_path(workspace, workspace_data_dir, path);
    let _ = crate::persistence::atomic_remove(&app_path);
    // Remove a portable legacy fallback too if one exists. Current writes never
    // place metadata inside the workspace.
    let legacy_path = workspace
        .join(".myelin")
        .join(NATIVE_METADATA_DIR)
        .join(file_name);
    let _ = crate::persistence::atomic_remove(&legacy_path);
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
            .map(serde_yaml::from_str::<Frontmatter>)
            .transpose()
            .with_context(|| format!("frontmatter is invalid at {}", path.display()))?;
        (metadata, body, false)
    };
    validate_native_body(path, &body)?;
    let stored_metadata = if native {
        read_native_metadata_sidecar(workspace, workspace_data_dir, path)?
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
    let id = metadata
        .id
        .clone()
        .unwrap_or_else(|| stable_id_from_path(path));
    let annotations = {
        let annotations_path = sidecar_path(
            workspace,
            workspace_data_dir,
            "annotations",
            &format!("{}.annotations.json", id),
        );
        if annotations_path.exists() {
            let raw = fs::read_to_string(&annotations_path).with_context(|| {
                format!("failed to read annotations {}", annotations_path.display())
            })?;
            Some(serde_json::from_str(&raw).with_context(|| {
                format!("annotations are invalid at {}", annotations_path.display())
            })?)
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
            if chats_path.exists() {
                let raw = fs::read_to_string(&chats_path).with_context(|| {
                    format!("failed to read chat history {}", chats_path.display())
                })?;
                serde_json::from_str(&raw).with_context(|| {
                    format!("chat history is invalid at {}", chats_path.display())
                })?
            } else {
                Vec::new()
            }
        },
    source_annotations: Vec::new(),
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
pub(crate) fn build_library_facets<'a>(
    documents: impl Iterator<Item = &'a NoteDocument>,
) -> LibraryFacets {
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
pub(crate) fn notebook_from_task_path(workspace: &Path, path: &Path) -> Option<String> {
    let holder = path.parent()?.parent()?; // the folder that contains the `tasks` dir
    let rel = relative_to_workspace(workspace, holder);
    if rel.is_empty() || rel == "." {
        None
    } else {
        Some(rel)
    }
}
pub(crate) fn task_files_for(workspace: &Path, id: &str) -> Vec<PathBuf> {
    let target = format!("{id}.json");
    let mut paths = walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e))
        .filter_map(|e| e.ok())
        .map(|entry| entry.into_path())
        .filter(|path| {
            is_task_file(path) && path.file_name().and_then(OsStr::to_str) == Some(target.as_str())
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
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
