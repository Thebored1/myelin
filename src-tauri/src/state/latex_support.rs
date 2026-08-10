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

// configured (semantic search), else a same-width lexical hashed fallback.
pub(crate) const EMBEDDING_DIM: i32 = 384;
pub(crate) const INDEX_DIR_NAME: &str = "index";
// Fast-chat profile: retain only the latest user/assistant pair when rebuilding
// context after restart. Live tool turns are also capped below.
pub(crate) const MAX_CHAT_HISTORY_MESSAGES_IN_PROMPT: usize = 2;
pub(crate) const MAX_LIVE_CONVERSATION_CHARS: usize = 8_000;
pub(crate) const SETTINGS_FILE_NAME: &str = "settings.json";
pub(crate) const TABLE_NAME: &str = "notes";
pub(crate) const NOTE_INGEST_MANIFEST: &str = "note-ingestion.json";
pub(crate) const QUERY_EMBEDDING_CACHE: &str = "query-embeddings.json";
pub(crate) const NOTE_CHUNKER_VERSION: &str = "words-192-overlap-32-gte-small-v2";
pub(crate) const NATIVE_METADATA_DIR: &str = "native-metadata";
pub(crate) const EMPTY_IPYNB: &str = "{\n  \"cells\": [],\n  \"metadata\": {},\n  \"nbformat\": 4,\n  \"nbformat_minor\": 5\n}\n";
pub(crate) const INDEX_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
// Tectonic downloads its LaTeX support bundle (~50 MB on first use) on demand.
// We pin that package cache to a directory we own under app data so it lands in
// a known place we can measure, pre-warm from Settings, and report on.
pub(crate) const TECTONIC_CACHE_DIR_NAME: &str = "tectonic-cache";
pub(crate) const TECTONIC_WARMED_MARKER: &str = ".myelin_warmed";

// Preamble used to wrap bare .tex notes that lack their own \documentclass. Kept
// deliberately broad so typical documents (math, figures, tables, links, colour,
// sensible margins) compile without the user hand-rolling a preamble. The prewarm
// stub uses the SAME preamble so "Download now" caches exactly these packages.
pub(crate) const DEFAULT_TEX_PREAMBLE: &str = "\\documentclass[11pt]{article}\n\
     \\usepackage[margin=1in]{geometry}\n\
     \\usepackage{amsmath,amssymb,amsfonts,mathtools}\n\
     \\usepackage{graphicx}\n\
     \\usepackage{booktabs}\n\
     \\usepackage{enumitem}";

/// Wrap bare LaTeX body text (no `\documentclass`) in the default preamble.
#[derive(Debug, Clone)]
pub(crate) struct TexTransform {
    pub(crate) source: String,
    /// Compiled line number (1-based) to editor line number; 0 means generated.
    pub(crate) line_map: Vec<usize>,
}

pub(crate) fn tex_line_without_comment(line: &str) -> String {
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if ch == '%' && !escaped {
            return line[..index].to_string();
        }
        if ch == '\\' {
            escaped = !escaped;
        } else {
            escaped = false;
        }
    }
    line.to_string()
}

pub(crate) fn tex_package_declaration(line: &str) -> Option<(&'static str, Vec<String>)> {
    let clean = tex_line_without_comment(line);
    let trimmed = clean.trim_start();
    let (kind, mut rest) = if let Some(rest) = trimmed.strip_prefix("\\usepackage") {
        ("use", rest)
    } else if let Some(rest) = trimmed.strip_prefix("\\RequirePackage") {
        ("require", rest)
    } else if let Some(rest) = trimmed.strip_prefix("\\PassOptionsToPackage") {
        ("pass", rest)
    } else {
        return None;
    };
    rest = rest.trim_start();
    if rest.starts_with('[') {
        let close = rest.find(']')?;
        rest = rest[close + 1..].trim_start();
    }
    let first_open = rest.find('{')?;
    let first_close = first_open + 1 + rest[first_open + 1..].find('}')?;
    let package_text = if kind == "pass" {
        let second_open = first_close + 1 + rest[first_close + 1..].find('{')?;
        let second_close = second_open + 1 + rest[second_open + 1..].find('}')?;
        &rest[second_open + 1..second_close]
    } else {
        &rest[first_open + 1..first_close]
    };
    let packages = package_text
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!packages.is_empty()).then_some((kind, packages))
}

pub(crate) fn has_active_documentclass(source: &str) -> bool {
    source
        .lines()
        .any(|line| tex_line_without_comment(line).trim_start().starts_with("\\documentclass"))
}

pub(crate) fn wrap_bare_latex(body: &str) -> TexTransform {
    // A note may contain a partial preamble (for example
    // `\\usepackage[dvipsnames]{xcolor}`) without a document class. Keep those
    // declarations before `\\begin{document}` so packages see their options.
    let mut preamble = Vec::new();
    let mut preamble_map = Vec::new();
    let mut content = Vec::new();
    let mut content_map = Vec::new();
    let mut in_preamble = true;
    for (line_number, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        let is_preamble_command = tex_package_declaration(line).is_some();
        if in_preamble && (is_preamble_command || trimmed.is_empty() || trimmed.starts_with('%')) {
            preamble.push(line);
            preamble_map.push(line_number + 1);
        } else {
            in_preamble = false;
            content.push(line);
            content_map.push(line_number + 1);
        }
    }
    let mut lines = DEFAULT_TEX_PREAMBLE.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut line_map = vec![0; lines.len()];
    lines.extend(preamble.iter().map(|line| (*line).to_string()));
    line_map.extend(preamble_map);
    lines.push("\\begin{document}".to_string());
    line_map.push(0);
    lines.extend(content.iter().map(|line| (*line).to_string()));
    line_map.extend(content_map);
    lines.push("\\end{document}".to_string());
    line_map.push(0);
    TexTransform { source: lines.join("\n"), line_map }
}

/// Faithful test entrypoint mirroring [`AppState::compile_latex`]'s transform
/// (legacy-frontmatter migration → preamble wrap / package injection → compile)
/// for a raw note file. Used by the `texcheck` diagnostic bin. Returns PDF bytes
/// or the first-line error message.
pub fn compile_tex_source(raw: &str) -> std::result::Result<Vec<u8>, String> {
    let body = split_legacy_native_frontmatter(raw).1;
    if body.trim().is_empty() {
        return Err("This note is empty — add some LaTeX before compiling.".to_string());
    }
    let final_tex = if !has_active_documentclass(&body) {
        wrap_bare_latex(&body).source
    } else {
        ensure_packages(&body).source
    };
    compile_with_tectonic(&final_tex, None).map_err(|f| f.message)
}

// Packages commonly used in notes that we make sure are available even when the
// note brings its own (often thin) preamble — e.g. AI/template notes that use
// \mathbb but only load amsmath. geometry/inputenc are intentionally excluded:
// they change layout, and a note with its own preamble may set them itself.
pub(crate) const ENSURE_PACKAGES: &[&str] = &[
    "amsmath",
    "amssymb",
    "amsfonts",
    "mathtools",
    "graphicx",
    "booktabs",
    "enumitem",
    "hyperref",
];

/// For a document that has its own `\documentclass`, inject `\usepackage{…}` lines
/// for any [`ENSURE_PACKAGES`] not already referenced, right after the
/// `\documentclass` line. Returns the new source and how many lines were inserted
/// (so TeX error lines can be mapped back to the editor). Skipping already-present
/// packages avoids LaTeX "option clash" errors.
pub(crate) fn ensure_packages(src: &str) -> TexTransform {
    // AI-generated documents occasionally repeat a package with different
    // options (most commonly xcolor), which LaTeX rejects as an option clash.
    // Keep the first declaration—the document author’s options—and discard
    // later duplicates before adding any missing defaults.
    let managed: std::collections::HashSet<&str> = ENSURE_PACKAGES
        .iter()
        .copied()
        .chain(["xcolor"])
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    let mut line_map = Vec::new();
    for (line_number, line) in src.lines().enumerate() {
        let declaration = tex_package_declaration(line);
        let duplicate = declaration.as_ref().is_some_and(|(kind, packages)| {
            *kind != "pass"
                && packages.iter().all(|pkg| managed.contains(pkg.as_str()) && seen.contains(pkg))
        });
        if !duplicate {
            lines.push(line.to_string());
            line_map.push(line_number + 1);
        }
        if let Some((kind, packages)) = declaration {
            if kind != "pass" {
                seen.extend(packages);
            }
        }
    }
    let missing: Vec<&str> = ENSURE_PACKAGES
        .iter()
        .copied()
        .filter(|pkg| !seen.contains(*pkg))
        .collect();
    if missing.is_empty() {
        return TexTransform { source: lines.join("\n"), line_map };
    }
    let Some(dc) = lines.iter().position(|line| {
        tex_line_without_comment(line).trim_start().starts_with("\\documentclass")
    }) else {
        return TexTransform { source: lines.join("\n"), line_map };
    };
    // Insert after the end of the \documentclass line.
    let injected = missing
        .iter()
        .map(|pkg| format!("\\usepackage{{{pkg}}}"))
        .collect::<Vec<_>>();
    let mut out = Vec::with_capacity(lines.len() + injected.len());
    let mut out_map = Vec::with_capacity(line_map.len() + injected.len());
    for index in 0..=dc {
        out.push(lines[index].clone());
        out_map.push(line_map[index]);
    }
    out.extend(injected);
    out_map.extend(std::iter::repeat(0).take(missing.len()));
    out.extend(lines.into_iter().skip(dc + 1));
    out_map.extend(line_map.into_iter().skip(dc + 1));
    TexTransform { source: out.join("\n"), line_map: out_map }
}

/// A failed Tectonic run: the engine's high-level message plus the raw TeX log
/// (which carries the `l.NN` line markers we parse into editor diagnostics).
pub(crate) struct TexFailure {
    pub(crate) message: String,
    pub(crate) log: String,
}

/// StatusBackend that captures the TeX error log instead of printing it. The
/// one-shot `tectonic::latex_to_pdf` uses a Noop backend that throws this away,
/// so we drive the session ourselves to get line-level diagnostics.
#[derive(Default, Clone)]
pub(crate) struct CapturingStatus {
    pub(crate) log: Arc<Mutex<Vec<u8>>>,
    pub(crate) messages: Arc<Mutex<Vec<String>>>,
}

impl tectonic::status::StatusBackend for CapturingStatus {
    fn report(
        &mut self,
        kind: tectonic::status::MessageKind,
        args: std::fmt::Arguments,
        _err: Option<&anyhow::Error>,
    ) {
        if matches!(
            kind,
            tectonic::status::MessageKind::Error | tectonic::status::MessageKind::Warning
        ) {
            self.messages.lock().push(format!("{args}"));
        }
    }

    fn dump_error_logs(&mut self, output: &[u8]) {
        self.log.lock().extend_from_slice(output);
    }
}

/// Delete the cached LaTeX format(s) so Tectonic rebuilds them. Used to recover
/// from a corrupt format whose catcode table breaks every compile.
pub(crate) fn clear_tectonic_format_cache() {
    if let Ok(dir) = std::env::var("TECTONIC_CACHE_DIR") {
        let _ = fs::remove_dir_all(Path::new(&dir).join("formats"));
    }
}

/// Compile `tex` to PDF bytes. Self-heals a corrupt format cache: if the engine
/// claims `\begin{document}` is missing even though our input contains it (the
/// classic symptom of a broken cached format), drop the format cache and retry.
pub(crate) fn compile_with_tectonic(tex: &str, input_root: Option<&Path>) -> std::result::Result<Vec<u8>, TexFailure> {
    match run_tectonic_session(tex, input_root) {
        Ok(pdf) => Ok(pdf),
        Err(failure)
            if tex.contains("\\begin{document}")
                && failure.message.contains("Missing \\begin{document}") =>
        {
            clear_tectonic_format_cache();
            run_tectonic_session(tex, input_root)
        }
        Err(failure) => Err(failure),
    }
}

/// Compile `tex` to PDF bytes, capturing the TeX log on failure. Runs the
/// Tectonic driver directly (vs. latex_to_pdf) so we can attach a capturing
/// status backend. Honours TECTONIC_CACHE_DIR set at startup.
pub(crate) fn run_tectonic_session(tex: &str, input_root: Option<&Path>) -> std::result::Result<Vec<u8>, TexFailure> {
    use tectonic::config::PersistentConfig;
    use tectonic::driver::{OutputFormat, ProcessingSessionBuilder};

    let mut status = CapturingStatus::default();
    let log_handle = status.log.clone();
    let messages_handle = status.messages.clone();
    let read_log = |h: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&h.lock()).into_owned();

    let config = match PersistentConfig::open(false) {
        Ok(c) => c,
        Err(e) => {
            return Err(TexFailure {
                message: format!("Tectonic config error: {e}"),
                log: String::new(),
            })
        }
    };
    let bundle = match config.default_bundle(false) {
        Ok(b) => b,
        Err(e) => {
            return Err(TexFailure {
                message: format!("Could not load the LaTeX support bundle: {e}"),
                log: read_log(&log_handle),
            })
        }
    };
    let format_cache_path = match config.format_cache_path() {
        Ok(p) => p,
        Err(e) => {
            return Err(TexFailure {
                message: format!("Tectonic format cache error: {e}"),
                log: read_log(&log_handle),
            })
        }
    };

    let mut sb = ProcessingSessionBuilder::default();
    sb.bundle(bundle)
        .primary_input_buffer(tex.as_bytes())
        .tex_input_name("texput.tex")
        .format_name("latex")
        .format_cache_path(format_cache_path)
        .keep_logs(false)
        .keep_intermediates(false)
        .print_stdout(false)
        .output_format(OutputFormat::Pdf)
        .do_not_write_output_files();
	if let Some(root) = input_root {
		sb.filesystem_root(root);
	}

    let mut sess = match sb.create(&mut status) {
        Ok(s) => s,
        Err(e) => {
            return Err(TexFailure {
                message: format!("{e}"),
                log: read_log(&log_handle),
            })
        }
    };
    if let Err(e) = sess.run(&mut status) {
        // Prefer the engine's first reported error line over the generic wrapper.
        let message = messages_handle
            .lock()
            .iter()
            .find(|m| !m.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("{e}"));
        return Err(TexFailure {
            message,
            log: read_log(&log_handle),
        });
    }

    let mut files = sess.into_file_data();
    match files.remove("texput.pdf") {
        Some(file) => Ok(file.data),
        None => Err(TexFailure {
            message: "LaTeX reported success but produced no PDF.".into(),
            log: read_log(&log_handle),
        }),
    }
}

/// Parse a TeX error log into editor diagnostics. The transform line map avoids
/// shifting diagnostics that occur before generated content and handles removed
/// duplicate package declarations.
pub(crate) fn parse_tex_log(log: &str, line_map: &[usize]) -> Vec<serde_json::Value> {
    let mut diagnostics = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut current_msg: Option<String> = None;

    let map_line = |n: usize| -> usize { line_map.get(n.saturating_sub(1)).copied().unwrap_or(0) };
    let leading_number = |s: &str| -> Option<usize> {
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<usize>().ok()
    };

    for raw_line in log.lines() {
        let line = raw_line.trim_end();
        if let Some(rest) = line.strip_prefix("! ") {
            current_msg = Some(rest.trim_end_matches('.').trim().to_string());
        }
        let mut push = |line_no: usize, msg: String| {
            let editor_line = map_line(line_no);
            if seen.insert((editor_line, msg.clone())) {
                diagnostics.push(serde_json::json!({
                    "line": editor_line,
                    "message": msg,
                    "severity": "error",
                }));
            }
        };
        if let Some(num) = line.strip_prefix("l.").and_then(leading_number) {
            let msg = current_msg
                .take()
                .unwrap_or_else(|| "LaTeX error".to_string());
            push(num, msg);
        } else if let Some(idx) = line.find("on input line ") {
            if let Some(num) = leading_number(&line[idx + "on input line ".len()..]) {
                let msg = current_msg
                    .clone()
                    .unwrap_or_else(|| line.trim().trim_start_matches('!').trim().to_string());
                push(num, msg);
            }
        }
    }
    diagnostics
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TectonicCacheStatus {
    pub warmed: bool,
    pub size_bytes: u64,
}

/// Total size of every file under `path` (recursive). Missing dir ⇒ 0.
pub(crate) fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => total += dir_size(&entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// Upper bound on the complete on-disk KV snapshot cache. A slot file holds a
/// full context window (~0.5-1 GB each at 32k), so an unbounded cache would eat
/// the disk; the LRU eviction keeps only the most recently used notes.
pub(crate) const SLOT_CACHE_BUDGET_BYTES: u64 = 8 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveSlotCache {
    pub(crate) note_id: String,
    pub(crate) filename: String,
    pub(crate) identity: String,
}

/// Delete the oldest `*.slot` snapshots (with their paired `.slot.json`
/// manifests) until `slot_dir` fits under `budget_bytes`. LRU order is file
/// mtime: every save and restore refreshes it.
pub(crate) fn enforce_slot_cache_budget(slot_dir: &Path, budget_bytes: u64) {
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut total: u64 = 0;
    for entry in walkdir::WalkDir::new(slot_dir).into_iter().flatten() {
        let path = entry.path().to_path_buf();
        if path.extension().and_then(|e| e.to_str()) != Some("slot") {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else { continue };
        total += meta.len();
        entries.push((
            meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            path,
        ));
    }
    if total <= budget_bytes {
        return;
    }
    entries.sort_by_key(|(modified, _)| *modified);
    for (_, path) in entries {
        if total <= budget_bytes {
            break;
        }
        if let Ok(meta) = fs::metadata(&path) {
            total = total.saturating_sub(meta.len());
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        log::info!("evicting llama slot cache entry {}", path.display());
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_file_name(format!("{name}.json")));
    }
}

/// The exact system/tool prefix the synthetic warm-up renders — the provenance
/// identity a post-turn slot save must record. Retrieval-backed chat uses the
/// same short tool-free profile as the document-answer fast path; ordinary chat
/// keeps the fixed read-only schema set for cache reuse on tool requests.
/// Render the viewer-section block exactly as the ask path embeds it in the
/// system message (state.rs note_body_excerpt). The scan and the ask path must
/// produce byte-identical text or the saved KV snapshot identity mismatches
/// and every ask falls back to a synchronous prime.
pub(crate) fn section_excerpt(section: &crate::models::ActiveSection) -> String {
    let label = section.label.as_deref().unwrap_or("active section");
    // Viewer callbacks do not all normalize extracted text identically. Use
    // one canonical form for both the prompt and the cache key so harmless
    // trailing whitespace cannot turn a prepared page into a cache miss.
    let content: String = section.content.trim().chars().take(80_000).collect();
    format!(
        "[ACTIVE VIEWER SECTION — {label}]\nSection key: {}\n{content}\n[/ACTIVE VIEWER SECTION]",
        section.key
    )
}

/// Ask llama-server itself to render the stable section system message without
/// a user turn. The resulting prompt ends at the system-message boundary, so
/// the real Chat or Write request can append its tool block, mode policy,
/// history, and user content after the restored KV prefix. Keeping this
/// boundary before the mode-specific system message is what lets one section
/// cache serve both profiles.
pub(crate) async fn section_prime_body(
    client: &reqwest::Client,
    config: &llama_server::ResolvedLlamaConfig,
    common_system: &str,
    template_kwargs: &Option<String>,
) -> Result<serde_json::Value> {
    let mut body = serde_json::json!({
        "messages": [
            { "role": "system", "content": common_system },
            // An explicit empty assistant message asks llama-server for the
            // completed system boundary instead of appending its generation
            // prompt. This is important: a user sentinel would save a user
            // header that is not present when a Write tool block follows.
            { "role": "assistant", "content": "" },
        ],
    });
    if let Some(raw) = template_kwargs.as_deref().and_then(|value| {
        serde_json::from_str::<serde_json::Value>(value).ok()
    }) {
        body["chat_template_kwargs"] = raw;
    }
    let url = format!("{}/apply-template", config.base_url());
    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let prompt = response["prompt"]
        .as_str()
        .ok_or_else(|| anyhow!("llama-server /apply-template omitted prompt"))?;
    if prompt.trim().is_empty() || !prompt.contains(common_system) {
        return Err(anyhow!(
            "llama-server produced no stable section-prefix boundary"
        ));
    }
    Ok(serde_json::json!({
        "prompt": prompt,
        "n_predict": 0,
        "cache_prompt": true,
        "id_slot": 0,
    }))
}

pub(crate) fn warmup_prefix(
    note_title: &str,
    excerpt: &str,
    cells: Option<&str>,
    interaction_mode: &str,
    doc_type: &str,
    supports_tools: bool,
    oversized: bool,
    verbose_tool_schemas: bool,
) -> (String, Vec<serde_json::Value>) {
    let direct_document_profile = interaction_mode == "chat" && (oversized || !supports_tools);
    let preamble = if interaction_mode == "write" {
        crate::agent::TARGETED_WRITE_PREAMBLE
    } else if direct_document_profile {
        crate::agent::DIRECT_CHAT_PREAMBLE
    } else {
        crate::agent::MYELIN_PREAMBLE
    };
    let context = if interaction_mode == "write" {
        assemble_targeted_write_context(note_title, excerpt, cells)
    } else {
        assemble_note_context(note_title, excerpt, cells)
    };
    let system = format!("{preamble}\n\n{context}");
    let tools = if direct_document_profile {
        Vec::new()
    } else if interaction_mode == "write" {
        crate::agent::targeted_write_tools(doc_type)
    } else {
        crate::agent::interaction_mode_tools(interaction_mode, oversized)
    };
    let tools = crate::agent::compact_tool_specs_for_profile(tools, verbose_tool_schemas);
    (system, tools)
}

pub(crate) fn describe_completion_error(error: &CompletionError) -> String {
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

pub(crate) fn describe_prompt_error(error: &PromptError) -> String {
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
