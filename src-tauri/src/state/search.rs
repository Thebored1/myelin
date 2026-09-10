pub(crate) use crate::llama_server::{self};
pub(crate) use crate::models::ProviderStatus;
pub(crate) use anyhow::{Context, Result};
pub(crate) use chrono::Utc;
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use sha2::{Digest, Sha256};
pub(crate) use std::borrow::Cow;
pub(crate) use std::ffi::OsStr;
pub(crate) use std::fs;
pub(crate) use std::hash::{Hash, Hasher};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use uuid::Uuid;

// GTE-small width. Notes use real embeddings when an embed model is

use super::documents_helpers::*;
use super::*;

pub(crate) fn get_file_timestamps(path: &Path) -> (String, String) {
    let fallback = timestamp_now();
    if let Ok(metadata) = std::fs::metadata(path) {
        let created = metadata
            .created()
            .unwrap_or_else(|_| std::time::SystemTime::now());
        let modified = metadata
            .modified()
            .unwrap_or_else(|_| std::time::SystemTime::now());
        let created_dt: chrono::DateTime<Utc> = created.into();
        let modified_dt: chrono::DateTime<Utc> = modified.into();
        (created_dt.to_rfc3339(), modified_dt.to_rfc3339())
    } else {
        (fallback.clone(), fallback)
    }
}

pub(crate) fn excerpt(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    // `len()` counts bytes, but Rust string slices must end on a UTF-8
    // character boundary. Workspace notes can contain emoji and other
    // multibyte characters, so truncate by characters instead of slicing at
    // an arbitrary byte offset.
    let mut chars = flat.chars();
    let excerpt: String = chars.by_ref().take(400).collect();
    if chars.next().is_some() {
        format!("{excerpt}...")
    } else {
        excerpt
    }
}

pub(crate) fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn hashed_embedding(text: &str) -> Vec<f32> {
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

pub(crate) fn normalize(vector: &mut [f32]) {
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if magnitude > 0.0 {
        for value in vector.iter_mut() {
            *value /= magnitude;
        }
    }
}

pub(crate) fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum::<f32>()
        .max(0.0)
}

pub(crate) fn slugify(input: &str) -> String {
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

pub(crate) fn is_reserved_windows_name(value: &str) -> bool {
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

pub(crate) fn unique_note_path(workspace: &Path, file_name: &str) -> PathBuf {
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

pub(crate) fn unique_pdf_path(workspace: &Path, file_name: &OsStr) -> PathBuf {
    let file_name = file_name.to_string_lossy();
    let stem = Path::new(file_name.as_ref())
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("document");
    let extension = Path::new(file_name.as_ref())
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("pdf");
    let candidate_name = |suffix: Option<usize>| match suffix {
        Some(index) => format!("{stem} {index}.{extension}"),
        None => format!("{stem}.{extension}"),
    };
    let matches_existing = |name: &str| {
        workspace
            .read_dir()
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .any(|existing| existing.eq_ignore_ascii_case(name))
    };

    let base = candidate_name(None);
    if !matches_existing(&base) {
        return workspace.join(base);
    }
    for index in 1..=9_999 {
        let candidate = candidate_name(Some(index));
        if !matches_existing(&candidate) {
            return workspace.join(candidate);
        }
    }
    workspace.join(format!("{stem} {}.{}", Uuid::new_v4(), extension))
}

pub(crate) fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

pub(crate) fn is_native_text_file(path: &Path) -> bool {
    extension_is(path, "tex") || extension_is(path, "ipynb")
}

pub(crate) fn is_binary_document(path: &Path) -> bool {
    extension_is(path, "pdf") || extension_is(path, "epub")
}

pub(crate) fn is_note_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| {
            extension.eq_ignore_ascii_case("md")
                || extension.eq_ignore_ascii_case("pdf")
                || extension.eq_ignore_ascii_case("epub")
                || extension.eq_ignore_ascii_case("tex")
                || extension.eq_ignore_ascii_case("ipynb")
        })
        .unwrap_or(false)
}

pub(crate) fn stable_id_from_path(path: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    format!("legacy-{:x}", hasher.finish())
}

pub(crate) struct ParsedLink {
    pub(crate) target: String,
    pub(crate) block: Option<String>,
    pub(crate) start_index: usize,
    pub(crate) end_index: usize,
}

pub(crate) fn extract_links(body: &str) -> Vec<ParsedLink> {
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

pub(crate) fn excerpt_around(body: &str, start: usize, end: usize) -> String {
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

pub(crate) fn sidecar_path(
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

pub(crate) fn legacy_workspace_storage_key(workspace: &Path) -> String {
    slugify(&workspace.to_string_lossy())
}

pub(crate) fn workspace_storage_key(workspace: &Path) -> String {
    let identity = canonical_workspace_identity(workspace).unwrap_or_else(|_| {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            workspace.as_os_str().as_bytes().to_vec()
        }
        #[cfg(not(unix))]
        {
            workspace.to_string_lossy().as_bytes().to_vec()
        }
    });
    format!("ws-v2-{:x}", Sha256::digest(identity))
}

fn canonical_workspace_identity(workspace: &Path) -> Result<Vec<u8>> {
    let canonical = fs::canonicalize(workspace)
        .with_context(|| format!("failed to canonicalize workspace {}", workspace.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(canonical.as_os_str().as_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        Ok(canonical
            .as_os_str()
            .encode_wide()
            .map(|unit| {
                let unit = if unit == b'\\' as u16 {
                    b'/' as u16
                } else {
                    unit
                };
                if (b'A' as u16..=b'Z' as u16).contains(&unit) {
                    unit + 32
                } else {
                    unit
                }
            })
            .flat_map(u16::to_le_bytes)
            .collect())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(canonical.to_string_lossy().as_bytes().to_vec())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceManifest {
    schema_version: u32,
    canonical_identity: String,
    display_path: String,
    legacy_key: String,
    migrated_at: String,
}

pub(crate) fn prepare_workspace_data_dir(app_data_dir: &Path, workspace: &Path) -> Result<PathBuf> {
    let workspaces = app_data_dir.join("workspaces");
    fs::create_dir_all(&workspaces)?;
    let key = workspace_storage_key(workspace);
    let target = workspaces.join(&key);
    if target.exists() {
        for error in crate::persistence::recover_transactions(&target)? {
            log::error!("workspace persistence recovery issue: {error}");
        }
        return Ok(target);
    }

    let staging = workspaces.join(format!(".{key}.staging-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging)?;
    let legacy_key = legacy_workspace_storage_key(workspace);
    let mut sources = Vec::new();
    let mut owned_sources = Vec::new();
    let legacy = workspaces.join(&legacy_key);
    if legacy.exists() {
        sources.push((legacy, false));
    }
    let quarantine = workspaces.join("_legacy-quarantine");
    if quarantine.exists() {
        let mut entries = fs::read_dir(&quarantine)
            .with_context(|| format!("failed to scan {}", quarantine.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| name.starts_with(&format!("{legacy_key}-")))
            })
            .map(|path| (path, true))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        sources.extend(entries);
    }
    for (source, quarantined) in sources {
        copy_workspace_data(
            &source,
            &staging,
            &source
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("legacy"),
        )?;
        if !quarantined {
            let ownership = source.join("workspace.json");
            let owned = fs::read_to_string(&ownership)
                .ok()
                .and_then(|raw| serde_json::from_str::<WorkspaceManifest>(&raw).ok())
                .is_some_and(|manifest| {
                    manifest.canonical_identity == canonical_identity_string(workspace)
                });
            if owned {
                // Keep the source until the v2 directory has been installed.
                // If the final rename fails, the next startup can retry from
                // the still-authoritative legacy copy.
                owned_sources.push(source);
            } else {
                let quarantine_dir = workspaces.join("_legacy-quarantine");
                fs::create_dir_all(&quarantine_dir)?;
                let destination = quarantine_dir.join(format!("{}-{}", legacy_key, Uuid::new_v4()));
                fs::rename(&source, destination)?;
            }
        }
    }
    let manifest = WorkspaceManifest {
        schema_version: 2,
        canonical_identity: canonical_identity_string(workspace),
        display_path: workspace.to_string_lossy().into_owned(),
        legacy_key,
        migrated_at: Utc::now().to_rfc3339(),
    };
    crate::persistence::atomic_write_json(&staging.join("workspace.json"), &manifest)?;
    fs::rename(&staging, &target)
        .with_context(|| format!("failed to install workspace data {}", target.display()))?;
    for source in owned_sources {
        if let Err(error) = fs::remove_dir_all(&source) {
            log::warn!(
                "verified legacy workspace data remains at {}: {error}",
                source.display()
            );
        }
    }
    Ok(target)
}

pub(crate) fn workspace_storage_issues(data_dir: &Path) -> Vec<StorageIssue> {
    let conflicts = data_dir.join("conflicts");
    if !conflicts.exists() {
        return Vec::new();
    }
    walkdir::WalkDir::new(&conflicts)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| StorageIssue {
            code: "workspace-storage-conflict".into(),
            severity: "error".into(),
            path: Some(entry.path().display().to_string()),
            message: "A legacy workspace sidecar conflicted during migration and was preserved under conflicts.".into(),
            recoverable: true,
        })
        .collect()
}

fn canonical_identity_string(workspace: &Path) -> String {
    let bytes = canonical_workspace_identity(workspace)
        .unwrap_or_else(|_| workspace.to_string_lossy().as_bytes().to_vec());
    format!("{:x}", Sha256::digest(bytes))
}

fn copy_workspace_data(source: &Path, target: &Path, source_id: &str) -> Result<()> {
    for entry in walkdir::WalkDir::new(source)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        let relative = entry.path().strip_prefix(source).unwrap_or(entry.path());
        if relative.as_os_str().is_empty() {
            continue;
        }
        // The v2 manifest is authoritative. Legacy ownership manifests are
        // inspected before copying and must not overwrite the new manifest.
        if relative == Path::new("workspace.json") {
            continue;
        }
        let destination = target.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination)?;
            continue;
        }
        if destination.exists() {
            if fs::read(&destination)? != fs::read(entry.path())? {
                let conflict = target.join("conflicts").join(source_id).join(relative);
                if let Some(parent) = conflict.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), conflict)?;
            }
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), &destination)?;
    }
    Ok(())
}

pub(crate) fn default_provider_status(app_data_dir: &Path) -> ProviderStatus {
    if let Ok(info) = llama_server::inspect_provider(app_data_dir) {
        let configured_engine =
            llama_server::normalize_engine(info.config.inference_engine.as_deref());
        let active_engine = info
            .resolved
            .as_ref()
            .map(|config| config.inference_engine.clone());
        return ProviderStatus {
            active_provider: if active_engine.as_deref() == Some("beellama") {
                "BeeLlama".into()
            } else {
                "llama.cpp".into()
            },
            available_providers: vec!["llama.cpp".into(), "BeeLlama".into()],
            healthy: info.healthy,
            ready: false,
            detail: info.detail,
            config: Some(info.config),
            resolved: info.resolved,
            active_backend: info.selected_backend,
            configured_engine,
            active_engine,
            nvidia_detected: info.nvidia_detected,
            gpu_available: info.gpu_available,
            gpus: info.gpus,
            installed_backends: info.installed_backends,
            installed_bee_backends: info.installed_bee_backends,
            recommended_threads: llama_server::default_inference_threads(),
        };
    }
    ProviderStatus {
        active_provider: "llama.cpp".into(),
        available_providers: vec!["llama.cpp".into(), "BeeLlama".into()],
        healthy: false,
        ready: false,
        detail: "Choose a .gguf model and llama-server executable in Settings.".into(),
        config: None,
        resolved: None,
        active_backend: None,
        configured_engine: "llama_cpp".into(),
        active_engine: None,
        nvidia_detected: llama_server::detect_nvidia(),
        gpu_available: llama_server::gpu_available(),
        gpus: llama_server::detect_gpus().0,
        installed_backends: Vec::new(),
        installed_bee_backends: Vec::new(),
        recommended_threads: llama_server::default_inference_threads(),
    }
}
