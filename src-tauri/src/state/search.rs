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
use super::documents_helpers::*;
use super::types::*;

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

pub(crate) fn workspace_storage_key(workspace: &Path) -> String {
    slugify(&workspace.to_string_lossy())
}

pub(crate) fn default_provider_status(app_data_dir: &Path) -> ProviderStatus {
    if let Ok(info) = llama_server::inspect_provider(app_data_dir) {
        let configured_engine = llama_server::normalize_engine(info.config.inference_engine.as_deref());
        let active_engine = info.resolved.as_ref().map(|config| config.inference_engine.clone());
        return ProviderStatus {
            active_provider: if active_engine.as_deref() == Some("beellama") { "BeeLlama".into() } else { "llama.cpp".into() },
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
