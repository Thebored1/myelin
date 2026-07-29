use crate::llama_server::{self, ManagedLlamaServer};
use crate::models::{
    AppSnapshot, Backlink, ChatTool, IndexState, LibraryFacets, NoteDocument, NoteSummary,
    ProviderStatus, SearchResponse, SearchResult, Task,
};
use crate::sidecar::ManagedSidecar;
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
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{async_runtime::Mutex as AsyncMutex, AppHandle, Emitter, Manager};
use uuid::Uuid;

// nomic-embed-text v1.5 width. Notes use real embeddings when an embed model is
// configured (semantic search), else a same-width lexical hashed fallback.
const EMBEDDING_DIM: i32 = 768;
const INDEX_DIR_NAME: &str = "index";
// Fast-chat profile: retain only the latest user/assistant pair when rebuilding
// context after restart. Live tool turns are also capped below.
const MAX_CHAT_HISTORY_MESSAGES_IN_PROMPT: usize = 2;
const MAX_LIVE_CONVERSATION_CHARS: usize = 8_000;
const LARGE_SUMMARY_CHUNK_WORDS: usize = 2_400;
const LARGE_SUMMARY_OVERLAP_WORDS: usize = 120;
const SUMMARY_REDUCTION_WORDS: usize = 5_000;
const SETTINGS_FILE_NAME: &str = "settings.json";
const TABLE_NAME: &str = "notes";
const NOTE_INGEST_MANIFEST: &str = "note-ingestion.json";
const NOTE_CHUNKER_VERSION: &str = "words-320-overlap-50-v1";
// Tectonic downloads its LaTeX support bundle (~50 MB on first use) on demand.
// We pin that package cache to a directory we own under app data so it lands in
// a known place we can measure, pre-warm from Settings, and report on.
const TECTONIC_CACHE_DIR_NAME: &str = "tectonic-cache";
const TECTONIC_WARMED_MARKER: &str = ".myelin_warmed";

// Preamble used to wrap bare .tex notes that lack their own \documentclass. Kept
// deliberately broad so typical documents (math, figures, tables, links, colour,
// sensible margins) compile without the user hand-rolling a preamble. The prewarm
// stub uses the SAME preamble so "Download now" caches exactly these packages.
const DEFAULT_TEX_PREAMBLE: &str = "\\documentclass[11pt]{article}\n\
     \\usepackage[margin=1in]{geometry}\n\
     \\usepackage{amsmath,amssymb,amsfonts,mathtools}\n\
     \\usepackage{graphicx}\n\
     \\usepackage{booktabs}\n\
     \\usepackage{enumitem}\n\
     \\usepackage{xcolor}\n\
     \\usepackage{hyperref}\n\
     \\begin{document}";

/// Wrap bare LaTeX body text (no `\documentclass`) in the default preamble.
fn wrap_bare_latex(body: &str) -> String {
    format!("{DEFAULT_TEX_PREAMBLE}\n{body}\n\\end{{document}}")
}

/// Faithful test entrypoint mirroring [`AppState::compile_latex`]'s transform
/// (frontmatter strip → preamble wrap / package injection → compile) for a raw
/// note file. Used by the `texcheck` diagnostic bin. Returns PDF bytes or the
/// first-line error message.
pub fn compile_tex_source(raw: &str) -> std::result::Result<Vec<u8>, String> {
    let body = split_frontmatter(raw).1;
    if body.trim().is_empty() {
        return Err("This note is empty — add some LaTeX before compiling.".to_string());
    }
    let final_tex = if !body.contains("\\documentclass") {
        wrap_bare_latex(&body)
    } else {
        ensure_packages(&body).0
    };
    compile_with_tectonic(&final_tex).map_err(|f| f.message)
}

// Packages commonly used in notes that we make sure are available even when the
// note brings its own (often thin) preamble — e.g. AI/template notes that use
// \mathbb but only load amsmath. geometry/inputenc are intentionally excluded:
// they change layout, and a note with its own preamble may set them itself.
const ENSURE_PACKAGES: &[&str] = &[
    "amsmath",
    "amssymb",
    "amsfonts",
    "mathtools",
    "graphicx",
    "booktabs",
    "enumitem",
    "xcolor",
    "hyperref",
];

/// For a document that has its own `\documentclass`, inject `\usepackage{…}` lines
/// for any [`ENSURE_PACKAGES`] not already referenced, right after the
/// `\documentclass` line. Returns the new source and how many lines were inserted
/// (so TeX error lines can be mapped back to the editor). Skipping already-present
/// packages avoids LaTeX "option clash" errors.
fn ensure_packages(src: &str) -> (String, usize) {
    let missing: Vec<&str> = ENSURE_PACKAGES
        .iter()
        .copied()
        .filter(|pkg| !src.contains(pkg))
        .collect();
    if missing.is_empty() {
        return (src.to_string(), 0);
    }
    let Some(dc) = src.find("\\documentclass") else {
        return (src.to_string(), 0);
    };
    // Insert after the end of the \documentclass line.
    let insert_at = src[dc..]
        .find('\n')
        .map(|i| dc + i + 1)
        .unwrap_or(src.len());
    let injected: String = missing
        .iter()
        .map(|pkg| format!("\\usepackage{{{pkg}}}\n"))
        .collect();
    let mut out = String::with_capacity(src.len() + injected.len());
    out.push_str(&src[..insert_at]);
    out.push_str(&injected);
    out.push_str(&src[insert_at..]);
    (out, missing.len())
}

/// A failed Tectonic run: the engine's high-level message plus the raw TeX log
/// (which carries the `l.NN` line markers we parse into editor diagnostics).
struct TexFailure {
    message: String,
    log: String,
}

/// StatusBackend that captures the TeX error log instead of printing it. The
/// one-shot `tectonic::latex_to_pdf` uses a Noop backend that throws this away,
/// so we drive the session ourselves to get line-level diagnostics.
#[derive(Default, Clone)]
struct CapturingStatus {
    log: Arc<Mutex<Vec<u8>>>,
    messages: Arc<Mutex<Vec<String>>>,
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
fn clear_tectonic_format_cache() {
    if let Ok(dir) = std::env::var("TECTONIC_CACHE_DIR") {
        let _ = fs::remove_dir_all(Path::new(&dir).join("formats"));
    }
}

/// Compile `tex` to PDF bytes. Self-heals a corrupt format cache: if the engine
/// claims `\begin{document}` is missing even though our input contains it (the
/// classic symptom of a broken cached format), drop the format cache and retry.
fn compile_with_tectonic(tex: &str) -> std::result::Result<Vec<u8>, TexFailure> {
    match run_tectonic_session(tex) {
        Ok(pdf) => Ok(pdf),
        Err(failure)
            if tex.contains("\\begin{document}")
                && failure.message.contains("Missing \\begin{document}") =>
        {
            clear_tectonic_format_cache();
            run_tectonic_session(tex)
        }
        Err(failure) => Err(failure),
    }
}

/// Compile `tex` to PDF bytes, capturing the TeX log on failure. Runs the
/// Tectonic driver directly (vs. latex_to_pdf) so we can attach a capturing
/// status backend. Honours TECTONIC_CACHE_DIR set at startup.
fn run_tectonic_session(tex: &str) -> std::result::Result<Vec<u8>, TexFailure> {
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

/// Parse a TeX error log into editor diagnostics. TeX reports the offending line
/// as `l.NN`; LaTeX package errors as `... on input line NN`. Line numbers are in
/// the compiled document's coordinates, so subtract `body_line_offset` (the
/// preamble length we prepended for bare notes) to map back to the editor.
fn parse_tex_log(log: &str, body_line_offset: usize) -> Vec<serde_json::Value> {
    let mut diagnostics = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut current_msg: Option<String> = None;

    let map_line = |n: usize| -> usize { n.saturating_sub(body_line_offset).max(1) };
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
fn dir_size(path: &Path) -> u64 {
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
    pub(crate) inner: Arc<InnerState>,
}

pub(crate) struct InnerState {
    app_data_dir: PathBuf,
    runtime: RwLock<RuntimeState>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    index_lock: AsyncMutex<()>,
    // Serialises Tectonic runs: concurrent compiles share one format-cache dir and
    // would corrupt it if they built the format at the same time.
    tectonic_lock: AsyncMutex<()>,
    llama_server: AsyncMutex<Option<ManagedLlamaServer>>,
    /// Serializes complete AI startup across boot, note-open, and first chat.
    ai_pipeline_lock: AsyncMutex<()>,
    ai_pipeline_ready: std::sync::atomic::AtomicBool,
    embed_server: AsyncMutex<Option<crate::llama_server::ManagedEmbedServer>>,
    /// The openharn-myelin agent sidecar: a long-lived `openharn-myelin` process
    /// that runs the agent loop and calls back to Myelin for tool execution.
    pub(crate) sidecar: AsyncMutex<Option<ManagedSidecar>>,
    /// Chat tools read shared per-turn context (open note, selection, question),
    /// so concurrent requests would overwrite or clear each other's target.
    chat_lock: AsyncMutex<()>,
    /// Live mirror of the persisted openharn sidecar settings, refreshed on save.
    openharn_settings: Mutex<OpenharnSettings>,
    background_settings: Mutex<BackgroundSettings>,
    llama_client: Client,
    chat_tools: Mutex<Vec<ChatTool>>,
    latest_chat_question: Mutex<Option<String>>,
    /// The editor text selection the user armed for the current chat turn, if any.
    /// Read by the write_note tool to scope an edit to just that span.
    current_selection: Mutex<Option<crate::agent::SelectionArg>>,
    /// The working-doc type of the open document this turn: "md" | "tex" | "ipynb".
    /// Steers the prompt (LaTeX/notebook vs Markdown) and notebook-aware tools.
    current_doc_type: Mutex<Option<String>>,
    current_note_id: Mutex<Option<String>>,
    cancel_ai: std::sync::atomic::AtomicBool,
    cancel_notify: tokio::sync::Notify,
    require_tool_approval: std::sync::atomic::AtomicBool,
    /// Runtime mirror of config.deterministic_tools, refreshed each chat turn, so
    /// tools (e.g. the write guard) can read it without re-resolving the config.
    deterministic_tools: std::sync::atomic::AtomicBool,
    /// Runtime mirror of config.tool_gating (per-message tool gating), refreshed
    /// each chat turn alongside `deterministic_tools`.
    tool_gating: std::sync::atomic::AtomicBool,
    /// Trusted execution policy for the current serialized chat turn. The model
    /// sees a stable tool schema for prompt-cache reuse; these flags enforce the
    /// mode-specific mutation boundary when a tool call reaches Rust.
    chat_mode: std::sync::atomic::AtomicBool,
    append_only: std::sync::atomic::AtomicBool,
    placement_edit: std::sync::atomic::AtomicBool,
    oversized_doc: std::sync::atomic::AtomicBool,
    tools_supported: std::sync::atomic::AtomicBool,
    note_ingest_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    note_ingest_manifest_lock: AsyncMutex<()>,
    /// At most one note-prefix warm-up should be consuming llama-server at once.
    /// The key is the exact request prefix; a newer prefix supersedes an older one.
    prompt_warmup: Mutex<Option<(u64, tokio::task::JoinHandle<()>)>>,
    pending_approvals: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// Per-note live conversation as the REAL message array (system-less): user
    /// turns, assistant turns with tool_calls, and the tool RESULTS. The frontend's
    /// chat_history keeps only text replies, so this is what lets the model keep
    /// coherent context across turns — search/fetch results stay as real `tool`
    /// messages instead of being flattened to a vague summary and lost. Re-sent each
    /// turn so llama-server reuses the cached prefix (KV cache). Keyed by note id.
    conversations: Mutex<HashMap<String, Vec<serde_json::Value>>>,
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
    #[serde(default, skip_serializing_if = "OpenharnSettings::is_default")]
    openharn: OpenharnSettings,
    #[serde(default, skip_serializing_if = "BackgroundSettings::is_default")]
    pub background: BackgroundSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct NoteIngestionManifest {
    entries: HashMap<String, NoteIngestionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NoteIngestionEntry {
    body_hash: String,
    chunker: String,
    embedding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct BackgroundSettings { pub start_with_system: bool }
impl BackgroundSettings { fn is_default(&self) -> bool { !self.start_with_system } }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaCacheStatus {
    pub enabled: bool,
    pub size_bytes: u64,
}

/// Configuration for the openharn-myelin agent sidecar. Persisted in
/// settings.json and surfaced in the Settings UI. Every field is optional /
/// overridable; defaults (see `OpenharnSettings::default`) are used when unset.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct OpenharnSettings {
    /// Sidecar listen port (Myelin -> sidecar). None = default (8091).
    pub port: Option<u16>,
    /// Explicit path to the openharn-myelin binary; overrides bundled/resource
    /// resolution (same as the OPENHARN_MYELIN_BIN env var).
    pub bin_path: Option<String>,
    /// Harness tuning forwarded to the sidecar's `/v1/chat/stream` `options`:
    /// `strict` enables the GBNF grammar + text-form tool calls; `prompt_tools`
    /// forces text-form tool calls without the grammar; `no_think` strips a
    /// model's `<think>` block; `narrow` is the read-only preset (strict +
    /// prompt-tools + only non-mutating tools); `slm` tightens tool-result caps
    /// for weak models; the rest bound the tool-call budget.
    /// Tool-call strategy: "auto" (Openharn chooses per request), "native",
    /// or "prompt" (text-form calls). This is user-controlled; model profiles
    /// must not silently override it.
    #[serde(default = "default_tool_mode")]
    pub tool_mode: String,
    /// Use the strict grammar when prompt-tools are selected.
    pub strict: bool,
    pub prompt_tools: bool,
    /// Force a tool call in prompt-tools mode instead of allowing prose.
    pub call_only: bool,
    pub no_think: bool,
    pub narrow: bool,
    pub slm: bool,
    /// Enable the relevance gate (model-based TOOL/CHAT classification) before
    /// entering the tool loop. Recovers the irrelevance category. Off by
    /// default; the per-request policy handles abstention via harness_decompose
    /// (plan_len==0 → NO_TOOL).
    #[serde(default)]
    pub friendly_results: bool,
    pub max_calls: Option<usize>,
    pub total_max: Option<usize>,
    pub tool_timeout_secs: Option<u64>,
    /// Maximum seconds allowed for one llama-server generation.
    pub generation_timeout_secs: Option<u64>,
    /// Restrict the agent to a named subset of tools (comma-separated function
    /// names, e.g. "write_note,web_search"). Blank = all tools Myelin offers.
    pub tool_subset: Option<String>,
    /// Override the llama-server base URL the sidecar calls
    /// (e.g. "http://127.0.0.1:39281/v1"). None = derived from the resolved
    /// llama config (config.base_url() + "/v1").
    pub base_url: Option<String>,
    /// Force `tool_choice` in native FC mode: "auto" (default), "required"
    /// (server grammar-forces a call in the model's own format — rescues
    /// quant-degraded native FC), "none", or a specific tool name.
    /// From openharn DSGoal: tool_choice=required + enable_thinking:false
    /// recovers ~71% of quant-gap on MiniCPM-V Q4_0 (47.5% -> 72.5%).
    #[serde(default)]
    pub tool_choice: Option<String>,
    /// Raw JSON forwarded as `chat_template_kwargs` into llama-server's request
    /// body. The canonical use is `{"enable_thinking":false}` — disables
    /// chain-of-thought on thinking models (a no-op on templates without the
    /// switch). Pairs with tool_choice=required to prevent think-budget deaths.
    #[serde(default)]
    pub template_kwargs: Option<String>,
}

fn default_tool_mode() -> String {
    "auto".to_string()
}

impl OpenharnSettings {
    fn is_default(&self) -> bool {
        *self == OpenharnSettings::default()
    }
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
        let openharn_settings = settings.openharn.clone();
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
                tectonic_lock: AsyncMutex::new(()),
                llama_server: AsyncMutex::new(None),
                ai_pipeline_lock: AsyncMutex::new(()),
                ai_pipeline_ready: std::sync::atomic::AtomicBool::new(false),
                embed_server: AsyncMutex::new(None),
                sidecar: AsyncMutex::new(None),
                chat_lock: AsyncMutex::new(()),
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
                cancel_ai: std::sync::atomic::AtomicBool::new(false),
                cancel_notify: tokio::sync::Notify::new(),
                require_tool_approval: std::sync::atomic::AtomicBool::new(false),
                deterministic_tools: std::sync::atomic::AtomicBool::new(true),
                tool_gating: std::sync::atomic::AtomicBool::new(false),
                chat_mode: std::sync::atomic::AtomicBool::new(false),
                append_only: std::sync::atomic::AtomicBool::new(false),
                placement_edit: std::sync::atomic::AtomicBool::new(false),
                oversized_doc: std::sync::atomic::AtomicBool::new(false),
                tools_supported: std::sync::atomic::AtomicBool::new(true),
                note_ingest_locks: Mutex::new(HashMap::new()),
                note_ingest_manifest_lock: AsyncMutex::new(()),
                prompt_warmup: Mutex::new(None),
                pending_approvals: Mutex::new(HashMap::new()),
                conversations: Mutex::new(HashMap::new()),
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

    /// The user's current chat message (for intent checks during tool calls).
    pub fn latest_chat_question(&self) -> String {
        self.inner
            .latest_chat_question
            .lock()
            .clone()
            .unwrap_or_default()
    }

    pub fn set_current_selection(&self, selection: Option<crate::agent::SelectionArg>) {
        *self.inner.current_selection.lock() = selection;
    }

    pub fn current_selection(&self) -> Option<crate::agent::SelectionArg> {
        self.inner.current_selection.lock().clone()
    }

    pub fn set_current_doc_type(&self, doc_type: Option<String>) {
        *self.inner.current_doc_type.lock() = doc_type;
    }

    pub fn current_doc_type(&self) -> String {
        self.inner
            .current_doc_type
            .lock()
            .clone()
            .unwrap_or_else(|| "md".to_string())
    }

    pub fn request_ai_cancel(&self) {
        self.inner
            .cancel_ai
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.cancel_notify.notify_waiters();
    }

    pub fn ai_cancel_requested(&self) -> bool {
        self.inner
            .cancel_ai
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) async fn wait_for_ai_cancel(&self) {
        self.inner.cancel_notify.notified().await;
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

    /// The live conversation (real message array) for a note — empty if none yet.
    pub fn conversation(&self, note_id: &str) -> Vec<serde_json::Value> {
        self.inner
            .conversations
            .lock()
            .get(note_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Replace a note's live conversation after a turn (already trimmed by caller).
    pub fn save_conversation(&self, note_id: &str, msgs: Vec<serde_json::Value>) {
        self.inner
            .conversations
            .lock()
            .insert(note_id.to_string(), msgs);
    }

    /// Forget a note's live conversation (e.g. when the user clears chat).
    pub fn clear_conversation(&self, note_id: &str) {
        self.inner.conversations.lock().remove(note_id);
    }

    fn note_by_id(&self, id: &str) -> Option<NoteDocument> {
        self.inner
            .runtime
            .read()
            .notes
            .get(id)
            .map(|note| note.document.clone())
    }

    /// Body of the note currently open in the editor (for the find_in_note tool).
    pub fn open_note_body(&self) -> Option<String> {
        self.current_note_id()
            .and_then(|id| self.note_by_id(&id))
            .map(|doc| doc.body)
    }

    /// Resolve the note a chat tool should act on: always prefer the note that
    /// is currently open in the editor, regardless of the title the model
    /// passed (a model can get the title wrong). Fall back to an exact
    /// title match only when no note is open.
    pub fn resolve_chat_target_note(&self, title: &str) -> Option<NoteDocument> {
        if let Some(id) = self.current_note_id() {
            if let Some(doc) = self.note_by_id(&id) {
                return Some(doc);
            }
        }
        self.find_note_by_exact_title(title)
    }

    pub fn is_tool_approval_required(&self) -> bool {
        self.inner
            .require_tool_approval
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_require_tool_approval(&self, require: bool) {
        self.inner
            .require_tool_approval
            .store(require, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn deterministic_tools_enabled(&self) -> bool {
        self.inner
            .deterministic_tools
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_deterministic_tools_runtime(&self, enabled: bool) {
        self.inner
            .deterministic_tools
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn tool_gating_enabled(&self) -> bool {
        self.inner
            .tool_gating
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_tool_gating_runtime(&self, enabled: bool) {
        self.inner
            .tool_gating
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_turn_tool_policy(
        &self,
        chat_mode: bool,
        append_only: bool,
        placement_edit: bool,
        oversized_doc: bool,
        tools_supported: bool,
    ) {
        use std::sync::atomic::Ordering;
        self.inner.chat_mode.store(chat_mode, Ordering::SeqCst);
        self.inner.append_only.store(append_only, Ordering::SeqCst);
        self.inner.placement_edit.store(placement_edit, Ordering::SeqCst);
        self.inner.oversized_doc.store(oversized_doc, Ordering::SeqCst);
        self.inner.tools_supported.store(tools_supported, Ordering::SeqCst);
    }

    pub fn authorize_tool_call(&self, name: &str) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        authorize_tool_policy(
            name,
            self.inner.chat_mode.load(Ordering::SeqCst),
            self.inner.append_only.load(Ordering::SeqCst),
            self.inner.placement_edit.load(Ordering::SeqCst),
            self.current_selection().is_some(),
            self.current_doc_type() == "ipynb",
            self.inner.oversized_doc.load(Ordering::SeqCst),
            self.inner.tools_supported.load(Ordering::SeqCst),
        )
    }

    pub fn oversized_doc_active(&self) -> bool {
        self.inner
            .oversized_doc
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The openharn sidecar settings (port, binary override, harness tuning).
    pub fn openharn_settings(&self) -> OpenharnSettings {
        self.inner.openharn_settings.lock().clone()
    }

    /// Persist new openharn settings and update the live mirror. Drops any
    /// running sidecar so the next chat respawns it with the new port / binary /
    /// harness tuning (the running process keeps the old launch args).
    pub fn set_openharn_settings(&self, settings: OpenharnSettings) -> Result<()> {
        self.invalidate_ai_pipeline();
        let mut all = load_settings(&self.inner.app_data_dir)?;
        all.openharn = settings.clone();
        save_settings(&self.inner.app_data_dir, &all)?;
        *self.inner.openharn_settings.lock() = settings;
        if let Ok(mut guard) = self.inner.sidecar.try_lock() {
            *guard = None;
        }
        Ok(())
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
            {
                let mut runtime = self.inner.runtime.write();
                runtime.index_state.is_indexing = true;
                runtime.index_state.backend = "scanning".to_string();
            }
            // Startup only needs enough data to open notes. Keep the expensive
            // embedding and LanceDB work off the first-paint path; reindex emits a
            // status event as soon as parsed notes are available.
            let state = self.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = state.reindex_workspace(workspace).await {
                    log::error!("startup workspace index failed: {error}");
                }
            });
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

    pub async fn set_llama_model_path(&self, model_path: String) -> Result<()> {
        self.invalidate_ai_pipeline();
        crate::llama_server::set_model_path(&self.inner.app_data_dir, model_path)?;
        Ok(())
    }

    pub async fn set_llama_executable_path(&self, executable_path: String) -> Result<()> {
        self.invalidate_ai_pipeline();
        crate::llama_server::set_executable_path(&self.inner.app_data_dir, executable_path)?;
        Ok(())
    }

    pub async fn set_deterministic_tools(&self, enabled: bool) -> Result<()> {
        crate::llama_server::set_deterministic_tools(&self.inner.app_data_dir, enabled)?;
        self.set_deterministic_tools_runtime(enabled);
        Ok(())
    }

    pub async fn set_tool_gating(&self, enabled: bool) -> Result<()> {
        crate::llama_server::set_tool_gating(&self.inner.app_data_dir, enabled)?;
        self.set_tool_gating_runtime(enabled);
        Ok(())
    }

    pub async fn set_prompt_cache(&self, enabled: bool) -> Result<()> {
        crate::llama_server::set_prompt_cache(&self.inner.app_data_dir, enabled)?;
        self.stop_llama_server().await;
        Ok(())
    }

    pub async fn set_inference_engine(&self, engine: String) -> Result<()> {
        crate::llama_server::set_inference_engine(&self.inner.app_data_dir, engine)?;
        self.stop_llama_server().await;
        Ok(())
    }

    pub fn llama_cache_status(&self) -> LlamaCacheStatus {
        LlamaCacheStatus {
            enabled: crate::llama_server::resolve_config(&self.inner.app_data_dir)
                .map(|c| c.prompt_cache)
                .unwrap_or(true),
            size_bytes: dir_size(&self.inner.app_data_dir.join("llama-cache")),
        }
    }

    pub async fn clear_llama_cache(&self) -> Result<()> {
        self.stop_llama_server().await;
        let path = self.inner.app_data_dir.join("llama-cache");
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
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
        self.invalidate_ai_pipeline();
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

    fn invalidate_ai_pipeline(&self) {
        self.inner
            .ai_pipeline_ready
            .store(false, std::sync::atomic::Ordering::Release);
    }

    fn ai_pipeline_ready(&self) -> bool {
        self.inner
            .ai_pipeline_ready
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn list_llama_devices(&self, backend: String) -> Vec<crate::llama_server::DeviceInfo> {
        crate::llama_server::list_devices(&self.inner.app_data_dir, &backend)
    }

    pub fn downloadable_backends(&self) -> Vec<String> {
        crate::llama_server::downloadable_backends()
    }

    pub fn downloadable_bee_backends(&self) -> Vec<String> {
        crate::llama_server::downloadable_bee_backends()
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
        self.download_backend(backend, false).await
    }

    pub async fn download_bee_backend(&self, backend: String) -> Result<()> {
        self.download_backend(backend, true).await
    }

    async fn download_backend(&self, backend: String, bee: bool) -> Result<()> {
        use futures_util::StreamExt;
        use sha2::{Digest, Sha256};

        let assets = if bee {
            crate::llama_server::bee_assets_for_backend(&backend)
        } else {
            crate::llama_server::assets_for_backend(&backend)
        };
        if assets.is_empty() {
            anyhow::bail!("No downloadable {backend} build is available for this platform.");
        }

        let bin_root = if bee {
            self.inner.app_data_dir.join("bin").join("bee")
        } else {
            self.inner.app_data_dir.join("bin")
        };
        let backend_dir = bin_root.join(&backend);
        let staging = bin_root.join(format!(".staging-{backend}"));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;

        // Backend archives are hundreds of MB and can take minutes. The shared
        // `llama_client` has a 120s TOTAL timeout tuned for chat/health requests,
        // which aborts a large download mid-body — surfacing as the misleading
        // "error decoding response body". Use a dedicated client with a per-read
        // idle timeout (catches stalled/dead connections) but NO overall cap, so a
        // slow-but-progressing download isn't killed.
        let download_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(120))
            .user_agent("Myelin")
            .build()
            .unwrap_or_else(|_| self.inner.llama_client.clone());

        let result: Result<()> = async {
            let total_assets = assets.len() as f64;
            for (i, asset) in assets.iter().enumerate() {
                let url = if bee {
                    crate::llama_server::bee_download_url(asset)
                } else {
                    crate::llama_server::download_url(asset)
                };
                self.emit_download(&backend, "downloading", (i as f64 / total_assets) * 100.0,
                    &format!("Downloading {} ({}/{})", asset, i + 1, assets.len()));

                let resp = download_client.get(&url).send().await
                    .with_context(|| format!("failed to download {asset}"))?;
                if !resp.status().is_success() {
                    anyhow::bail!("download failed for {asset}: HTTP {}", resp.status());
                }
                let total = resp.content_length().unwrap_or(0);
                let archive_path = staging.join(asset);
                let mut file = fs::File::create(&archive_path)?;
                let mut downloaded: u64 = 0;
                let mut last_pct: i32 = -1;
                let mut digest = Sha256::new();
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.with_context(|| {
                        format!("download stream interrupted for {asset} (network stalled or connection dropped)")
                    })?;
                    std::io::Write::write_all(&mut file, &chunk)?;
                    digest.update(&chunk);
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
                if bee {
                    let expected = crate::llama_server::bee_asset_sha256(asset)
                        .ok_or_else(|| anyhow!("No pinned checksum for {asset}"))?;
                    let actual = format!("{:x}", digest.finalize());
                    if actual != expected {
                        anyhow::bail!(
                            "Checksum mismatch for {asset}: expected {expected}, got {actual}"
                        );
                    }
                }

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
                self.emit_download(
                    &backend,
                    "done",
                    100.0,
                    &format!(
                        "{} {backend} backend installed",
                        if bee { "BeeLlama" } else { "llama.cpp" }
                    ),
                );
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
        notebook: Option<String>,
    ) -> Result<NoteDocument> {
        let workspace = self.require_workspace()?;
        let now = timestamp_now();
        let id = Uuid::new_v4().to_string();

        let unique_title = self.ensure_unique_title(&title, None);
        let safe_slug = slugify(&unique_title);
        let ext = extension.unwrap_or_else(|| "md".to_string());
        let file_name = format!("{safe_slug}--{}.{ext}", &id[..8]);
        // When a notebook (folder) is given, create the note inside it.
        let target_dir = match notebook {
            Some(name) if !name.trim().is_empty() && !name.trim().eq_ignore_ascii_case("root") => {
                let safe = sanitize_relative_folder(&name)?;
                let dir = workspace.join(folder_to_relative_path(&safe));
                fs::create_dir_all(&dir)
                    .with_context(|| format!("failed to open notebook {}", dir.display()))?;
                dir
            }
            _ => workspace.clone(),
        };
        let path = unique_note_path(&target_dir, &file_name);
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

        let vector = self
            .note_embedding(
                &format!(
                    "{}\n{}\n{}",
                    document.title,
                    document.tags.join(" "),
                    document.body
                ),
                false,
            )
            .await;

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

    /// Create a notebook — a top-level folder in the workspace that holds notes
    /// of any kind. Returns the updated list of notebooks.
    pub fn create_notebook(&self, name: String) -> Result<Vec<String>> {
        let workspace = self.require_workspace()?;
        let safe = sanitize_relative_folder(&name)?;
        if safe == "Root" {
            return Err(anyhow!("notebook name cannot be empty"));
        }
        let dir = workspace.join(folder_to_relative_path(&safe));
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create notebook {}", dir.display()))?;
        self.list_notebooks()
    }

    /// List notebooks: the top-level folders in the workspace (filesystem is the
    /// source of truth), excluding hidden/ignored dirs. Includes empty ones.
    pub fn list_notebooks(&self) -> Result<Vec<String>> {
        let workspace = self.require_workspace()?;
        let mut names = Vec::new();
        if let Ok(entries) = fs::read_dir(&workspace) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.')
                    || name == "node_modules"
                    || name == "target"
                    || name == "dist"
                    || name == "build"
                    || name == "tasks"
                {
                    continue;
                }
                names.push(name);
            }
        }
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        Ok(names)
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
        let prompt_changed = existing.document.body != body
            || existing.document.title != title;

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
        if prompt_changed {
            let slot = self.inner.app_data_dir.join("llama-cache").join("slots")
                .join(Self::slot_filename(&note_id));
            let _ = fs::remove_file(&slot);
            let _ = fs::remove_file(slot.with_file_name(format!("{}.json", slot.file_name().unwrap_or_default().to_string_lossy())));
        }

        let vector = self
            .note_embedding(
                &format!(
                    "{}\n{}\n{}",
                    updated.title,
                    updated.tags.join(" "),
                    updated.body
                ),
                false,
            )
            .await;

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
        // Version history is useful, but it must never make a successfully
        // persisted Markdown edit look like a failed note write. A workspace may
        // have an inaccessible Git index or an unsupported file type; keep the
        // note and report history failure only in the application log.
        if let Err(error) = crate::git_history::commit_changes(
            &workspace,
            &format!("Update note: {}", updated.title),
        ) {
            log::warn!("saved note but could not create Git history entry: {error}");
        }

        let state = self.clone();
        let workspace_clone = workspace.clone();
        tauri::async_runtime::spawn(async move {
            let _ = state.reindex_workspace(workspace_clone).await;
        });
        let ingest_state = self.clone();
        let ingest_note = updated.clone();
        tauri::async_runtime::spawn(async move {
            let known = ingest_state.note_has_ingestion_entry(&ingest_note.id);
            let ctx = ingest_state
                .running_ctx_size()
                .await
                .or_else(|| {
                    crate::llama_server::resolve_config(&ingest_state.inner.app_data_dir)
                        .ok()
                        .map(|config| config.context_size)
                })
                .unwrap_or(4096) as usize;
            let oversized = crate::note_prompt::NotePromptShape::build(
                &ingest_note.body,
                &ingest_note.relative_path,
                ctx,
            )
            .oversized;
            if known || oversized {
                if let Err(error) = ingest_state.ensure_oversized_note_ingested(&ingest_note).await {
                    log::warn!("background oversized-note ingestion failed: {error}");
                }
            }
        });

        Ok(updated)
    }

    pub async fn delete_note(&self, note_id: String) -> Result<AppSnapshot> {
        let workspace = self.require_workspace()?;
        let slot_dir = self.inner.app_data_dir.join("llama-cache").join("slots");
        let slot_name = Self::slot_filename(&note_id);
        let _ = fs::remove_file(slot_dir.join(&slot_name));
        let _ = fs::remove_file(slot_dir.join(format!("{slot_name}.json")));
        let path = {
            let runtime = self.inner.runtime.read();
            runtime
                .notes
                .get(&note_id)
                .map(|note| workspace.join(&note.document.relative_path))
                .ok_or_else(|| anyhow!("note not found"))?
        };

        fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;

        // Delete the note's sidecars too — the chat session and annotations are
        // keyed by note id and would otherwise orphan in the workspace data dir
        // (stale sessions lingering after the note is gone).
        let data_dir = self.workspace_data_dir(&workspace);
        let _ = fs::remove_file(data_dir.join("chats").join(format!("{note_id}.chat.json")));
        let _ = fs::remove_file(data_dir.join("chats").join(format!("{note_id}.chat.tmp")));
        let _ = fs::remove_file(
            data_dir
                .join("annotations")
                .join(format!("{note_id}.annotations.json")),
        );
        // Drop any RAG chunks ingested for this note from the document store.
        let _ = self.delete_document(&note_id).await;
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let mut ingestion = self.load_note_ingestion_manifest();
            if ingestion.entries.remove(&note_id).is_some() {
                let _ = self.save_note_ingestion_manifest(&ingestion);
            }
        }

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

        let query_vector = self.note_embedding(trimmed, true).await;
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
        let mut active_engine = info
            .resolved
            .as_ref()
            .map(|config| config.inference_engine.clone());
        let configured_engine =
            crate::llama_server::normalize_engine(info.config.inference_engine.as_deref());
        let healthy = if let Some(config) = &info.resolved {
            let server = self.inner.llama_server.lock().await;
            if let Some(server) = server.as_ref() {
                active_backend = Some(server.active_backend.label().to_string());
                active_engine = Some(server.active_engine.clone());
                if config.accepts_running(&server.config) {
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
            active_provider: if active_engine.as_deref() == Some("beellama") {
                "BeeLlama".into()
            } else {
                "llama.cpp".into()
            },
            available_providers: vec!["llama.cpp".into(), "BeeLlama".into()],
            healthy,
            ready: healthy && self.ai_pipeline_ready(),
            detail: info.detail,
            config: Some(info.config),
            resolved: info.resolved,
            active_backend,
            configured_engine,
            active_engine,
            nvidia_detected: info.nvidia_detected,
            gpu_available: info.gpu_available,
            gpus: info.gpus,
            installed_backends: info.installed_backends,
            installed_bee_backends: info.installed_bee_backends,
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

    /// Summarise a note of arbitrary size without putting the whole source in a
    /// single model context. Each source chunk is summarised independently, then
    /// those summaries are recursively reduced until one final summary remains.
    pub async fn summarise_large_note(&self, note_id: String) -> Result<String> {
        let note = self.load_note(note_id.clone()).await?;
        if note.body.trim().is_empty() {
            return Ok("The note is empty.".to_string());
        }

        self.inner
            .cancel_ai
            .store(false, std::sync::atomic::Ordering::Release);
        let chunks = crate::embeddings::chunk_text(
            &note.body,
            LARGE_SUMMARY_CHUNK_WORDS,
            LARGE_SUMMARY_OVERLAP_WORDS,
        );
        let total = chunks.len();
        self.emit_summary_progress(&note_id, "chunking", 0, total, "Preparing source");

        let mut summaries = Vec::with_capacity(total);
        for (index, chunk) in chunks.iter().enumerate() {
            self.ensure_summary_not_cancelled()?;
            let prompt = format!(
                "Summarise this source section faithfully and densely. Preserve important facts, decisions, numbers, names, and conclusions. Do not invent information. Keep the section label in your response.\n\nSource: {}\nSection {}/{}\n\n{}",
                note.title,
                index + 1,
                total,
                chunk.text
            );
            let summary = self
                .run_llama_prompt(
                    "You are the first stage of a hierarchical document summarizer. Produce a factual section summary that another model can combine later. Keep it under 600 words.",
                    &prompt,
                )
                .await?;
            summaries.push(format!("[Section {}/{}]\n{}", index + 1, total, summary.trim()));
            self.emit_summary_progress(
                &note_id,
                "summarizing",
                index + 1,
                total,
                &format!("Summarized section {}/{}", index + 1, total),
            );
        }

        let original_total = total;
        while summaries.len() > 1 {
            self.ensure_summary_not_cancelled()?;
            let mut reduced = Vec::new();
            let mut cursor = 0;
            while cursor < summaries.len() {
                self.ensure_summary_not_cancelled()?;
                let mut group = Vec::new();
                let mut words = 0;
                while cursor < summaries.len() {
                    let next_words = summaries[cursor].split_whitespace().count();
                    if !group.is_empty() && words + next_words > SUMMARY_REDUCTION_WORDS {
                        break;
                    }
                    words += next_words;
                    group.push(summaries[cursor].as_str());
                    cursor += 1;
                }
                let prompt = format!(
                    "Combine the following section summaries into one faithful summary. Preserve all major facts and conclusions, remove repetition, and retain useful section/source labels. Do not mention the summarization process.\n\nDocument: {}\n\n{}",
                    note.title,
                    group.join("\n\n")
                );
                let summary = self
                    .run_llama_prompt(
                        "You are a higher-level document summarizer. Synthesize the supplied summaries without adding facts that are not present. Keep the result under 900 words.",
                        &prompt,
                    )
                    .await?;
                reduced.push(summary.trim().to_string());
                self.emit_summary_progress(
                    &note_id,
                    "combining",
                    reduced.len(),
                    summaries.len(),
                    "Combining section summaries",
                );
            }
            summaries = reduced;
        }

        self.ensure_summary_not_cancelled()?;
        self.emit_summary_progress(
            &note_id,
            "complete",
            original_total,
            original_total,
            "Summary complete",
        );
        Ok(summaries.pop().unwrap_or_default())
    }

    fn ensure_summary_not_cancelled(&self) -> Result<()> {
        if self.ai_cancel_requested() {
            Err(anyhow!("Summary cancelled"))
        } else {
            Ok(())
        }
    }

    fn emit_summary_progress(
        &self,
        note_id: &str,
        phase: &str,
        completed: usize,
        total: usize,
        message: &str,
    ) {
        let _ = self.handle.emit(
            "ai://summary_progress",
            serde_json::json!({
                "noteId": note_id,
                "phase": phase,
                "completed": completed,
                "total": total,
                "message": message,
            }),
        );
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
        selection: Option<crate::agent::SelectionArg>,
        doc_type: Option<String>,
        interaction_mode: Option<String>,
    ) -> Result<()> {
        // The active note/question/selection live in shared AppState because the
        // tools are invoked asynchronously. If an older turn still owns the
        // guard, cancel it and let this new request take over instead of showing
        // a zero-second "another request is running" error to the user.
        let _chat_guard = match self.inner.chat_lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.request_ai_cancel();
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    self.inner.chat_lock.lock(),
                )
                .await
                .map_err(|_| anyhow!("The previous AI request did not stop within 10 seconds."))?
            }
        };
        // Chat and edit are the current frontend paths. Operation and auto stay
        // accepted for API compatibility and a future composer-mode toggle.
        let interaction_mode = match interaction_mode.as_deref() {
            None | Some("auto") => "auto",
            Some("chat") => "chat",
            Some("operation") => "operation",
            Some("edit") => "edit",
            Some(mode) => return Err(anyhow!("unknown AI interaction mode: {mode}")),
        };
        self.reset_chat_tools();
        self.inner
            .cancel_ai
            .store(false, std::sync::atomic::Ordering::Release);
        self.set_latest_chat_question(question.clone());
        // A normal selection carries text; an isolated editor insertion carries
        // an explicitly marked zero-length cursor target.
        let selection = selection.filter(|s| s.cursor || !s.text.trim().is_empty());
        self.set_current_selection(selection.clone());
        let doc_type = doc_type.unwrap_or_else(|| "md".to_string());
        self.set_current_doc_type(Some(doc_type.clone()));
        self.set_current_note_id(note_id.clone());
        let result: Result<()> = async {
            let note = self.load_note(note_id).await?;

            // Relative placement has no deterministic anchor unless the editor
            // supplied an armed selection. Refuse before model/tool execution so
            // a weak model cannot silently append to the wrong location.
            if crate::agent::placement_request_intent(&question) && selection.is_none() {
                let message = "Where should I place it? Select the anchor text in the note, or provide an exact heading, line, or marker.";
                let _ = self.handle.emit("ai://chat_chunk", serde_json::json!({
                    "requestId": request_id, "delta": message
                }));
                return Ok(());
            }

            // A synthetic note-prefix request must never sit ahead of a real
            // user turn on llama-server's single inference slot.
            self.cancel_prompt_warmup().await;
            let config = self.ensure_ai_pipeline_ready().await?;
            self.cancel_prompt_warmup().await;
            // Fast-chat profile: let the model choose ordinary tools instead of
            // routing through deterministic format/find assists. Rust still
            // executes the selected tools and enforces write safety.
            let deterministic_tools = false;
            self.set_deterministic_tools_runtime(deterministic_tools);
            self.set_tool_gating_runtime(config.tool_gating);
            // Budget the note to ~half the context window the server ACTUALLY
            // launched with (auto-offload may run far above the configured value),
            // leaving room for the system prompt, tools, chat history, and the
            // reply. ~4 chars/token → a 32K-token context holds ~65K chars of note,
            // far past the old flat 24K cap.
            let ctx_tokens = self
                .running_ctx_size()
                .await
                .unwrap_or(config.context_size) as usize;
            let prompt_shape = crate::note_prompt::NotePromptShape::build(
                &note.body,
                &note.relative_path,
                ctx_tokens,
            );
            let oversized_ready = if prompt_shape.oversized {
                self.ensure_oversized_note_ingested(&note).await.is_ok()
            } else {
                false
            };
            let note_body_excerpt = if prompt_shape.oversized && !oversized_ready {
                let limit = ctx_tokens.saturating_mul(2).clamp(4_000, 400_000);
                let head: String = note.body.chars().take(limit).collect();
                format!("{head}\n…[note truncated because full-note indexing failed]")
            } else {
                prompt_shape.body.clone()
            };
            // Give the model the note's CURRENT content as editable text. The old
            // "reference only — do NOT copy" framing (plus a 400-char cap) meant it
            // could neither see nor feel allowed to modify existing content, so it
            // could only write fresh, never edit/format/shorten/delete.
            let isolated_edit = interaction_mode == "edit";
            let append_only = !isolated_edit && crate::agent::append_request_intent(&question);
            let placement = crate::agent::placement_request_intent(&question);
            let has_selection = selection.is_some();
            // For a notebook, present readable CELLS (not the raw JSON body) so the
            // model edits via edit_notebook instead of trying to rewrite JSON.
            let notebook_cells = if doc_type == "ipynb" {
                crate::notebook::present(&note.body)
            } else {
                None
            };
            let context = assemble_note_context(&note.title, &note_body_excerpt, notebook_cells.as_deref());
            let stable_context = context.clone();
            let mut turn_instructions = String::new();
            if append_only && !has_selection {
                turn_instructions.push_str(
                    "\n\nAPPEND-ONLY TURN: The current note (if shown) is reference material only. \
                     Add ONLY the new requested text; never reproduce, quote, or regenerate any \
                     existing line. Call append_note with only the new paragraph in content."
                );
            }
            // The open document isn't always Markdown — tell the model so it edits
            // in the right language instead of defaulting to Markdown headings/lists.
            if doc_type == "tex" {
                turn_instructions.push_str(
                    "\n\nIMPORTANT: This open document is a LaTeX (.tex) source file, NOT Markdown. \
                     Write and edit it using LaTeX syntax only — e.g. \\section{...}, \\subsection{...}, \
                     \\textbf{...}, \\emph{...}, \\begin{itemize}\\item ...\\end{itemize}, $...$ for math, \
                     \\begin{equation}...\\end{equation}. Do NOT use Markdown (#, **, -). Preserve the \
                     document's preamble and \\begin{document}/\\end{document} structure.",
                );
            }
            // If the user armed an editor selection, show the model EXACTLY what is
            // selected and scope the request to it. The deterministic write path
            // (selection_scoped_plan) enforces "selection only" regardless, but the
            // model still needs to see the selected text to rewrite it well.
            if let Some(sel) = &selection {
                if sel.cursor {
                    turn_instructions.push_str(
                        "\n\nThe editor supplied an exact CURSOR target. Generate only the new text to insert there and call write_note with that text as content. Never reproduce existing note text.",
                    );
                } else {
                    turn_instructions.push_str(&format!(
                        "\n\nThe user has SELECTED the following part of the note. This request applies to the SELECTION ONLY — leave every character outside it unchanged:\n\"\"\"\n{}\n\"\"\"\n\nUse write_note for this selection and send ONLY replacement content. For removal, send an empty `content`. Do not reproduce the whole note or call a mutation tool that targets text outside this selection.",
                        sel.text
                    ));
                    if placement {
                        turn_instructions.push_str(" For a below/after/under/beneath request, use insert_after_line with marker set to the selected text and insert immediately after its line.");
                    }
                }
            }
            // The note context is always embedded in the system message so that
            // cache_prompt sees the same note-scoped prefix across chat and
            // operation turns. Per-turn routing details are appended to the user
            // message below and do not change this shared prefix.

            // Keep routing/intent detection for execution policy, but use one
            // stable model-facing schema list on every supported-model turn.
            let recent_user_msgs: Vec<&str> = note
                .chat_history
                .iter()
                .filter(|m| m.role == "user" && m.error != Some(true))
                .map(|m| m.content.as_str())
                .collect();
            let edit_thread = crate::agent::in_edit_thread(&recent_user_msgs);
            // Capability gate: skip tools entirely for a model whose template
            // can't do tool calls (profile-known or probed once + cached), so it
            // works as a chat-only model instead of erroring every turn.
            let model_id = config.model_path.to_string_lossy().to_string();
            let supports_tools = crate::tool_capability::supports_tools(
                &self.inner.llama_client,
                &config.base_url(),
                &self.inner.app_data_dir,
                &config.model_path,
                &model_id,
                config.supports_tools,
            )
            .await;
            // `oversized_ready` is shared by the wire schema and execution policy:
            // successful ingestion enables scoped retrieval and the oversized
            // mutation gate. On failure, ensure_oversized_note_ingested emits
            // failure progress, the prompt uses its truncated fallback, and this
            // gate stays off because scoped retrieval is unavailable.
            self.set_turn_tool_policy(
                interaction_mode == "chat",
                append_only && !has_selection,
                placement && has_selection,
                oversized_ready,
                supports_tools,
            );

            // Stream directly against llama-server (not through rig) so the note
            // content can be surfaced token-by-token as it is generated. See
            // `stream_chat`.
            // Build the send array as REAL messages: system preamble + the note's
            // live conversation (prior user/assistant turns AND tool results) + this
            // turn's user message. Re-sent each turn so llama-server reuses the
            // cached prefix (KV cache); the retained tool messages are what let a
            // later "write what you found" actually have the search results.
            let nid = self.current_note_id().unwrap_or_default();
            let mut convo = if isolated_edit { Vec::new() } else { self.conversation(&nid) };
            if convo.is_empty() && !isolated_edit {
                // First turn this session: seed from the saved text history (no tool
                // results — those were never persisted) so we don't lose continuity.
                convo = chat_history_to_messages(&note.chat_history);
            }
            // Embed the note context in the system message so cache_prompt
            // reuses the KV prefix across requests (the note stays constant
            // between turns when no tool modifies it). Only the short question
            // at the end needs re-evaluation each turn.
            let mode_instruction = match interaction_mode {
                "chat" => "CHAT TURN POLICY: The open note identified above is the default target for every read or question. Answer questions about it directly; do not call search_notes or read_note unless the user explicitly asks about another note, their notes/workspace, or names a different note. Use web/document retrieval only when explicitly requested. Never modify the note for this turn.",
                "operation" => "OPERATION TURN POLICY: Perform the user's requested operation using the appropriate tool. The open note identified above is the default target.",
                "edit" => "EDITOR ACTION: Perform exactly this isolated edit with write_note. Return only the replacement or insertion content in the tool call; never reproduce text outside the target.",
                _ => "AUTO TURN POLICY: Decide whether the user needs a direct answer or an operation.",
            };
            let turn = crate::ai_turn::AiTurnBuilder::build(crate::ai_turn::AiTurnInput {
                mode: interaction_mode,
                note_title: &note.title,
                system_context: &stable_context,
                conversation: &convo,
                question: &question,
                mode_policy: mode_instruction,
                turn_instructions: &turn_instructions,
                has_open_note: true,
                edit_thread,
                oversized: oversized_ready,
                supports_tools,
                verbose_tool_schemas: config.verbose_tool_schemas,
            });
            let intent_is_tool = Some(turn.intent_is_tool);
            let messages = turn.messages;
            let tools = turn.tools;
            let user_content = crate::ai_turn::render_user_content(
                &note.title,
                mode_instruction,
                &turn_instructions,
                &question,
            );

            let tool_names: Vec<String> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect();
            log::info!(
                "[ask_ai_stream] mode={} tools_offered={} gating={} deterministic={} edit_thread={} supports_tools={}",
                interaction_mode, tool_names.join(","), config.tool_gating && interaction_mode != "operation", deterministic_tools, edit_thread, supports_tools
            );
            let tool_mode = self.openharn_settings().tool_mode;
            let _ = self.handle.emit(
                "ai://debug_event",
                serde_json::json!({
                    "kind": "config",
                    "msg": format!(
                        "mode={}, tools: {}, gate={}, determ={}, edit={}, tools_supported={}, tool_mode={}",
                        interaction_mode,
                        tool_names.join(", "),
                        config.tool_gating && interaction_mode != "operation",
                        deterministic_tools,
                        edit_thread,
                        supports_tools,
                        tool_mode,
                    ),
                    "requestId": request_id,
                }),
            );

            // Deterministic TOOL/CHAT intent from the same routing predicates
            // used to select the tool schemas. This avoids the expensive model
            // classifier (which cost ~8s in the trace) while still recognizing
            // note edits, searches, lookups, and document retrieval.
            let final_messages = crate::sidecar::run_chat(
                self,
                &config,
                messages,
                tools,
                &request_id,
                &nid,
                intent_is_tool,
                interaction_mode == "chat",
                interaction_mode == "operation" || isolated_edit,
                selection.is_some(),
            )
            .await?;

            // Persist the exact user message sent on the wire. Byte identity is
            // required for cache_prompt to reuse prior conversation/tool turns.
            // The UI transcript separately retains the undecorated question.
            if !isolated_edit {
                convo.push(serde_json::json!({ "role": "user", "content": user_content }));
                convo.extend(final_messages.iter().cloned());
                let convo = trim_conversation(convo, MAX_LIVE_CONVERSATION_CHARS);
                self.save_conversation(&nid, convo);
            }

            if turn_contains_note_mutation(&final_messages) {
                let state = self.clone();
                let note_id = nid.clone();
                let warm_mode = interaction_mode.to_string();
                tokio::spawn(async move {
                    if let Err(error) = state
                        .warm_llama_server_for_note(Some(note_id), Some(warm_mode))
                        .await
                    {
                        log::debug!("post-write prompt-cache warm-up skipped: {error}");
                    }
                });
            }

            Ok(())
        }
        .await;

        self.clear_latest_chat_question();
        self.clear_current_note_id();
        // Release the per-turn guard before notifying the UI. Retry/undo waits
        // for chat_done, so keeping the guard through this emit creates a small
        // but real race with the replacement request.
        drop(_chat_guard);

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

        let dest = if src.starts_with(&workspace) {
            src.clone()
        } else {
            let file_name = src
                .file_name()
                .ok_or_else(|| anyhow!("invalid file path: no filename"))?;
            // Place the import inside the given notebook (folder), else workspace root.
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
            let dest = target_dir.join(file_name);
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

    fn tectonic_cache_dir(&self) -> PathBuf {
        self.inner.app_data_dir.join(TECTONIC_CACHE_DIR_NAME)
    }

    fn tectonic_warmed_marker(&self) -> PathBuf {
        self.tectonic_cache_dir().join(TECTONIC_WARMED_MARKER)
    }

    fn is_tectonic_warmed(&self) -> bool {
        self.tectonic_warmed_marker().exists()
    }

    fn mark_tectonic_warmed(&self) {
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
    async fn run_tectonic(&self, tex: String, body_line_offset: usize) -> Result<Vec<u8>> {
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

        let result =
            tauri::async_runtime::spawn_blocking(move || compile_with_tectonic(&tex)).await;

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
                    "diagnostics": parse_tex_log(&failure.log, body_line_offset),
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

    pub async fn compile_latex(&self, note_id: String) -> Result<Vec<u8>> {
        let workspace = self.require_workspace()?;
        let path = {
            let runtime = self.inner.runtime.read();
            let note = runtime
                .notes
                .get(&note_id)
                .ok_or_else(|| anyhow!("note not found"))?;
            workspace.join(&note.document.relative_path)
        };
        let raw = fs::read_to_string(&path)?;
        // The .tex file on disk carries YAML frontmatter (id/title/tags/…). Strip
        // it before compiling — otherwise that metadata block is text BEFORE
        // \documentclass and LaTeX fails with "Missing \begin{document}" at line 1.
        let tex_content = split_frontmatter(&raw).1;
        if tex_content.trim().is_empty() {
            return Err(anyhow!(
                "This note is empty — add some LaTeX before compiling."
            ));
        }

        // Bare notes get the full default preamble prepended; notes with their own
        // \documentclass get any missing common packages injected. Either way the
        // returned offset is how many lines we added before the body, so TeX error
        // lines map back to what the editor shows.
        let (final_tex, offset) = if !tex_content.contains("\\documentclass") {
            (
                wrap_bare_latex(&tex_content),
                DEFAULT_TEX_PREAMBLE.lines().count(),
            )
        } else {
            ensure_packages(&tex_content)
        };

        self.run_tectonic(final_tex, offset).await
    }

    /// Pre-download Tectonic's support bundle by compiling a tiny stub document,
    /// so users can warm the cache from Settings instead of paying the first-run
    /// fetch when they hit "Compile to PDF".
    pub async fn prewarm_tectonic(&self) -> Result<()> {
        let stub = wrap_bare_latex("Myelin LaTeX warm-up: $E = mc^2$, \\textbf{ready}.");
        self.run_tectonic(stub, 0).await.map(|_| ())
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

    // ── Tasks ──
    // Each task is a self-contained JSON file (file-per-item: portable, separately
    // copyable, Drive-syncable). Default location is `<workspace>/tasks/<id>.json`;
    // a task assigned to a notebook lives at `<workspace>/<notebook>/tasks/<id>.json`.
    // The note indexer ignores them (not a note extension).

    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let workspace = self.require_workspace()?;
        let mut tasks = Vec::new();
        for entry in walkdir::WalkDir::new(&workspace)
            .into_iter()
            .filter_entry(|e| !is_hidden_or_ignored(e))
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !is_task_file(path) {
                continue;
            }
            let Ok(raw) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(mut task) = serde_json::from_str::<Task>(&raw) else {
                continue;
            };
            // The file's location is the source of truth for the notebook (a task
            // file copied into another notebook folder belongs to that notebook).
            task.notebook = notebook_from_task_path(&workspace, path);
            tasks.push(task);
        }
        tasks.sort_by(|a, b| match (a.position, b.position) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.created_at.cmp(&b.created_at),
        });
        Ok(tasks)
    }

    /// Create (empty id) or update a task, writing its JSON file. Moving it between
    /// notebooks (changing `notebook`) relocates the file. Returns the saved task.
    pub fn save_task(&self, mut task: Task) -> Result<Task> {
        let workspace = self.require_workspace()?;
        task.notebook = task
            .notebook
            .map(|s| s.trim().replace('\\', "/"))
            .filter(|s| !s.is_empty());
        if let Some(nb) = &task.notebook {
            validate_relative_dir(nb)?;
        }
        if task.id.trim().is_empty() {
            task.id = Uuid::new_v4().to_string();
        }
        validate_task_id(&task.id)?;
        let now = Utc::now().to_rfc3339();
        if task.created_at.trim().is_empty() {
            task.created_at = now.clone();
        }
        task.updated_at = now;
        for sub in task.subtasks.iter_mut() {
            if sub.id.trim().is_empty() {
                sub.id = Uuid::new_v4().to_string();
            }
        }
        let dir = task_dir_for(&workspace, task.notebook.as_deref());
        fs::create_dir_all(&dir)?;
        let target = dir.join(format!("{}.json", task.id));
        // Drop any existing file for this id elsewhere (handles notebook moves).
        remove_task_files(&workspace, &task.id, Some(target.as_path()));
        let tmp = dir.join(format!("{}.tmp", task.id));
        fs::write(&tmp, serde_json::to_string_pretty(&task)?)?;
        fs::rename(&tmp, &target)?;
        let _ = self.handle.emit("tasks://changed", ());
        Ok(task)
    }

    pub fn delete_task(&self, id: String) -> Result<()> {
        let workspace = self.require_workspace()?;
        validate_task_id(&id)?;
        remove_task_files(&workspace, &id, None);
        let _ = self.handle.emit("tasks://changed", ());
        Ok(())
    }

    /// Synchronously kill the spawned llama + embed server child processes. Safe to
    /// call from a window-close handler — uses try_lock so it never awaits/blocks.
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

    pub fn background_settings(&self) -> BackgroundSettings { self.inner.background_settings.lock().clone() }

    pub fn set_background_settings(&self, settings: BackgroundSettings) -> Result<()> {
        let mut persisted = load_settings(&self.inner.app_data_dir)?;
        persisted.background = settings.clone();
        save_settings(&self.inner.app_data_dir, &persisted)?;
        *self.inner.background_settings.lock() = settings;
        Ok(())
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

        // Upgrade the hashed placeholder vectors to real embeddings (one batch)
        // when an embed model is configured — semantic note search.
        self.reembed_notes(&mut notes).await;

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

        let table = rebuild_lancedb(&self.index_dir(), &notes).await?;

        {
            let mut runtime = self.inner.runtime.write();
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
        let mut settings = load_settings(&self.inner.app_data_dir)?;
        settings.workspace_path = runtime
            .workspace_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        settings.custom_note_order = runtime.custom_note_order.clone();
        save_settings(&self.inner.app_data_dir, &settings)?;
        Ok(())
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

    /// Context window (tokens) the running llama-server launched with, if any.
    async fn running_ctx_size(&self) -> Option<u32> {
        self.inner
            .llama_server
            .lock()
            .await
            .as_ref()
            .map(|s| s.ctx_size)
    }

    /// The configured SearXNG base URL for web search (None → DuckDuckGo).
    pub fn searxng_url(&self) -> Option<String> {
        crate::llama_server::searxng_url(&self.inner.app_data_dir)
    }

    /// Set (or clear, when empty) the SearXNG base URL for web search.
    pub fn set_searxng_url(&self, url: Option<String>) -> Result<()> {
        crate::llama_server::set_searxng_url(&self.inner.app_data_dir, url)
    }

    /// The configured embedding model GGUF path (None → embeddings disabled).
    pub fn quick_shortcut(&self) -> String {
        crate::llama_server::quick_capture_shortcut(&self.inner.app_data_dir)
    }

    pub fn set_quick_shortcut(&self, shortcut: String) -> Result<()> {
        crate::llama_server::set_quick_capture_shortcut(&self.inner.app_data_dir, shortcut)
    }

    pub fn embed_model_path(&self) -> Option<String> {
        crate::llama_server::embed_model_path(&self.inner.app_data_dir)
    }

    /// All known model profiles (bundled + user) for the compatibility list.
    pub fn list_model_profiles(&self) -> Vec<crate::model_profiles::ModelProfile> {
        crate::model_profiles::all_profiles(&self.inner.app_data_dir)
    }

    /// Set (or clear, when empty) the embedding model GGUF path.
    pub fn set_embed_model_path(&self, path: Option<String>) -> Result<()> {
        crate::llama_server::set_embed_model_path(&self.inner.app_data_dir, path)
    }

    /// Start (or keep) the llama-server warm for the note now open in the editor,
    /// so the first message is instant and subsequent ones reuse the warm slot.
    /// Called on note open. Best-effort: a failure just means the first chat pays
    /// the cold start it already handled before.
    pub async fn warm_llama_server(&self) -> Result<()> {
        self.warm_llama_server_for_note(None, None).await
    }

    pub async fn warm_llama_server_for_note(
        &self,
        note_id: Option<String>,
        interaction_mode: Option<String>,
    ) -> Result<()> {
        let configured = self.ensure_ai_pipeline_ready().await?;
        // Fingerprint and address the server that actually won candidate
        // selection. The configured preference may differ from the running
        // binary/backend after startup fallback.
        let (config, ctx_tokens) = {
            let server = self.inner.llama_server.lock().await;
            match server.as_ref() {
                Some(server) => (server.config.clone(), server.ctx_size as usize),
                None => (configured.clone(), configured.context_size as usize),
            }
        };
        let Some(note_id) = note_id else { return Ok(()); };
        let note = self.load_note(note_id.clone()).await?;
        let prompt_shape = crate::note_prompt::NotePromptShape::build(
            &note.body,
            &note.relative_path,
            ctx_tokens,
        );
        let oversized_ready = if prompt_shape.oversized {
            self.ensure_oversized_note_ingested(&note).await.is_ok()
        } else {
            false
        };
        let excerpt = if prompt_shape.oversized && !oversized_ready {
            let limit = ctx_tokens.saturating_mul(2).clamp(4_000, 400_000);
            let head: String = note.body.chars().take(limit).collect();
            format!("{head}\n…[note truncated because full-note indexing failed]")
        } else {
            prompt_shape.body
        };
        let interaction_mode = match interaction_mode.as_deref() {
            Some("operation") => "operation",
            Some("edit") => "edit",
            _ => "chat",
        };
        let doc_type = note.relative_path.to_ascii_lowercase();
        let cells = if doc_type.ends_with(".ipynb") {
            crate::notebook::present(&note.body)
        } else {
            None
        };
        let warm_preamble = if interaction_mode == "chat" {
            crate::agent::DIRECT_CHAT_PREAMBLE
        } else {
            crate::agent::MYELIN_PREAMBLE
        };
        let system = format!(
            "{warm_preamble}\n\n{}",
            assemble_note_context(&note.title, &excerpt, cells.as_deref())
        );
        let template_kwargs = self.openharn_settings().template_kwargs;
        // The synthetic note-open warm-up targets the overwhelmingly common
        // direct Chat path. Retrieval schemas are question-specific and must
        // not be paid on every empty/direct turn merely to warm the note prefix.
        let warm_specs = if interaction_mode == "chat" {
            Vec::new()
        } else {
            crate::agent::interaction_mode_tools(interaction_mode, oversized_ready)
        };
        let tools = crate::agent::compact_tool_specs_for_profile(
            warm_specs,
            config.verbose_tool_schemas,
        );
        let tools_json = serde_json::to_string(&tools).unwrap_or_default();
        let identity = Self::slot_identity(
            &config,
            ctx_tokens as u32,
            interaction_mode,
            &system,
            &tools_json,
            &template_kwargs,
        );
        // Never enqueue synthetic inference while a real chat owns the turn.
        if self.inner.chat_lock.try_lock().is_err() {
            return Ok(());
        }
        if config.prompt_cache {
            self.restore_note_slot(&config, &note_id, &identity).await;
        }
        self.spawn_note_cache_warmup(
            &config,
            note_id,
            interaction_mode.to_string(),
            system,
            tools,
            template_kwargs,
            identity,
        );
        Ok(())
    }

    fn slot_filename(note_id: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        note_id.hash(&mut hasher);
        format!("note-{:016x}.slot", hasher.finish())
    }

    fn slot_manifest_path(config: &llama_server::ResolvedLlamaConfig, note_id: &str) -> PathBuf {
        config
            .slot_save_path
            .join(format!("{}.json", Self::slot_filename(note_id)))
    }

    /// Everything that determines whether a saved KV snapshot is still valid:
    /// the server binary (stock llama.cpp vs. a patched build), the model
    /// identity, the chat template, the running context size, launch-affecting
    /// flags, and the exact model-facing payload. A snapshot whose identity
    /// record does not match must be erased, never restored — a foreign KV is
    /// silent corruption of an editing assistant.
    fn slot_identity(
        config: &llama_server::ResolvedLlamaConfig,
        running_ctx: u32,
        interaction_mode: &str,
        system: &str,
        tools_json: &str,
        template_kwargs: &Option<String>,
    ) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        config.inference_engine.hash(&mut hasher);
        config.executable_path.hash(&mut hasher);
        let exe_meta = std::fs::metadata(&config.executable_path).ok();
        exe_meta.as_ref().map(|m| m.len()).unwrap_or(0).hash(&mut hasher);
        exe_meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .hash(&mut hasher);
        config.model_path.hash(&mut hasher);
        let model_meta = std::fs::metadata(&config.model_path).ok();
        model_meta.as_ref().map(|m| m.len()).unwrap_or(0).hash(&mut hasher);
        model_meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .hash(&mut hasher);
        running_ctx.hash(&mut hasher);
        config.backend.hash(&mut hasher);
        config.gpu_layers.hash(&mut hasher);
        config.threads.hash(&mut hasher);
        config.gpu_device.hash(&mut hasher);
        config.chat_format.hash(&mut hasher);
        config.chat_template_override.hash(&mut hasher);
        match config.chat_template_override.as_deref() {
            Some("lfm2") => include_str!("../templates/lfm2.jinja").hash(&mut hasher),
            Some("lfm25") => include_str!("../templates/lfm25.jinja").hash(&mut hasher),
            Some(path) if !path.trim().is_empty() => {
                std::fs::read(path).unwrap_or_default().hash(&mut hasher)
            }
            _ => {}
        }
        config.extra_args.hash(&mut hasher);
        interaction_mode.hash(&mut hasher);
        system.hash(&mut hasher);
        tools_json.hash(&mut hasher);
        template_kwargs.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    async fn restore_note_slot(
        &self,
        config: &llama_server::ResolvedLlamaConfig,
        note_id: &str,
        identity: &str,
    ) {
        let filename = Self::slot_filename(note_id);
        let slot_path = config.slot_save_path.join(&filename);
        if !slot_path.exists() {
            return;
        }
        // Never restore a snapshot without a matching provenance record: a
        // changed backend build, model, template, context size, or schema set
        // makes the saved KV foreign to this note's next request.
        let recorded = std::fs::read_to_string(Self::slot_manifest_path(config, note_id))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v["identity"].as_str().map(str::to_string));
        if recorded.as_deref() != Some(identity) {
            log::info!("discarding stale llama slot for {note_id} (identity mismatch)");
            let _ = fs::remove_file(&slot_path);
            let _ = fs::remove_file(Self::slot_manifest_path(config, note_id));
            return;
        }
        let url = format!("{}/slots/0?action=restore", config.base_url());
        match self.inner.llama_client.post(url).json(&serde_json::json!({"filename": filename})).send().await {
            Ok(response) if response.status().is_success() => {
                log::info!("restored persistent llama slot for note {note_id}");
            }
            Ok(response) => {
                log::warn!("slot restore failed for {note_id}: {}", response.status());
                let _ = fs::remove_file(&slot_path);
                let _ = fs::remove_file(Self::slot_manifest_path(config, note_id));
            }
            Err(error) => log::warn!("slot restore request failed for {note_id}: {error}"),
        }
    }

    /// Prepare every process/check that would otherwise delay the first model
    /// request. The lock makes startup and a quickly submitted chat share the
    /// same work instead of launching duplicate capability probes.
    async fn ensure_ai_pipeline_ready(&self) -> Result<llama_server::ResolvedLlamaConfig> {
        let _pipeline_guard = self.inner.ai_pipeline_lock.lock().await;
        if !self.ai_pipeline_ready() {
            let _ = self.handle.emit(
                "ai://llama_warmup",
                serde_json::json!({ "status": "started" }),
            );
        }

        let result: Result<llama_server::ResolvedLlamaConfig> = async {
            let config = llama_server::resolve_config(&self.inner.app_data_dir)?;
            self.ensure_llama_server(&config).await?;

            // Sidecar startup is independent of the one-time model capability
            // probe, so overlap them after llama-server becomes healthy.
            let model_id = config.model_path.to_string_lossy().to_string();
            let llama_base = config.base_url();
            let sidecar = crate::sidecar::ensure_sidecar(self);
            let capability = crate::tool_capability::supports_tools(
                &self.inner.llama_client,
                &llama_base,
                &self.inner.app_data_dir,
                &config.model_path,
                &model_id,
                config.supports_tools,
            );
            let (sidecar_result, _) = tokio::join!(sidecar, capability);
            sidecar_result?;
            Ok(config)
        }
        .await;

        match result {
            Ok(config) => {
                let announce_ready = !self.ai_pipeline_ready();
                self.inner
                    .ai_pipeline_ready
                    .store(true, std::sync::atomic::Ordering::Release);
                if announce_ready {
                    let _ = self.handle.emit(
                        "ai://llama_warmup",
                        serde_json::json!({ "status": "ready" }),
                    );
                }
                Ok(config)
            }
            Err(error) => {
                self.invalidate_ai_pipeline();
                let _ = self.handle.emit(
                    "ai://llama_warmup",
                    serde_json::json!({ "status": "failed", "message": error.to_string() }),
                );
                Err(error)
            }
        }
    }

    async fn cancel_prompt_warmup(&self) {
        let handle = self
            .inner
            .prompt_warmup
            .lock()
            .take()
            .map(|(_, handle)| handle);
        if let Some(handle) = handle {
            if !handle.is_finished() {
                handle.abort();
                let _ = handle.await;
                log::debug!("preempted note prompt-cache warm-up for user chat");
            }
        }
    }

    fn spawn_note_cache_warmup(
        &self,
        config: &llama_server::ResolvedLlamaConfig,
        note_id: String,
        interaction_mode: String,
        system: String,
        tools: Vec<serde_json::Value>,
        template_kwargs: Option<String>,
        identity: String,
    ) {
        use std::hash::{Hash, Hasher};
        let client = self.inner.llama_client.clone();
        let url = format!("{}/v1/chat/completions", config.base_url());
        let model = config.model_name();
        let slot_url = format!("{}/slots/0?action=save", config.base_url());
        let slot_filename = Self::slot_filename(&note_id);
        let slot_dir = config.slot_save_path.clone();
        let manifest = serde_json::json!({ "identity": identity }).to_string();
        let persist = config.prompt_cache;
        let parsed_template_kwargs = template_kwargs
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        model.hash(&mut hasher);
        system.hash(&mut hasher);
        interaction_mode.hash(&mut hasher);
        serde_json::to_string(&tools)
            .unwrap_or_default()
            .hash(&mut hasher);
        template_kwargs.hash(&mut hasher);
        let key = hasher.finish();
        let mut warmup = self.inner.prompt_warmup.lock();
        if let Some((existing_key, handle)) = warmup.as_ref() {
            if *existing_key == key && !handle.is_finished() {
                return;
            }
            if !handle.is_finished() {
                handle.abort();
            }
        }
        let handle = tokio::spawn(async move {
            let mut body = serde_json::json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": " " }
                ],
                "max_tokens": 1,
                "temperature": 0.0,
                "cache_prompt": true,
            });
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
                body["tool_choice"] = serde_json::json!("none");
            }
            if let Some(kwargs) = parsed_template_kwargs {
                body["chat_template_kwargs"] = kwargs;
            }
            match client.post(&url).json(&body).send().await {
                Ok(response) if response.status().is_success() => {
                    let usage = response
                        .json::<serde_json::Value>()
                        .await
                        .ok()
                        .and_then(|value| value.get("usage").cloned());
                    log::info!("llama note prompt-cache warm-up complete; usage={usage:?}");
                    if persist {
                        match client.post(slot_url).json(&serde_json::json!({"filename": slot_filename})).send().await {
                            Ok(saved) if saved.status().is_success() => {
                                log::info!("saved persistent llama slot for note {note_id}");
                                // Record provenance alongside the snapshot; the
                                // restore path erases snapshots whose identity
                                // no longer matches the current binaries/payload.
                                let manifest_path = slot_dir.join(format!("{slot_filename}.json"));
                                let _ = std::fs::write(&manifest_path, manifest);
                            }
                            Ok(saved) => log::warn!("slot save failed for {note_id}: {}", saved.status()),
                            Err(error) => log::warn!("slot save request failed for {note_id}: {error}"),
                        }
                    }
                },
                Ok(response) => log::warn!("llama note prompt-cache warm-up returned {}", response.status()),
                Err(error) => log::warn!("llama note prompt-cache warm-up failed: {error}"),
            }
        });
        *warmup = Some((key, handle));
    }

    /// Stop the llama-server (and the embedding server), releasing RAM/VRAM.
    /// Called when the open note is closed — nothing to infer for. The next note
    /// open warms it again. Idempotent.
    pub async fn stop_llama_server(&self) {
        self.invalidate_ai_pipeline();
        self.cancel_prompt_warmup().await;
        let mut guard = self.inner.llama_server.lock().await;
        if let Some(mut server) = guard.take() {
            llama_server::stop_server(&mut server).await;
            log::info!("llama-server stopped (note closed)");
        }
        drop(guard);
        let mut embed = self.inner.embed_server.lock().await;
        if let Some(mut server) = embed.take() {
            llama_server::stop_embed_server(&mut server).await;
        }
    }

    /// Ensure the embedding server is running for the configured embed model and
    /// return its base URL. The embed server runs alongside the chat server on
    /// chat_port + 1. Errors if no embedding model is configured.
    async fn ensure_embed_server(&self) -> Result<String> {
        let model = llama_server::embed_model_path(&self.inner.app_data_dir).ok_or_else(|| {
            anyhow::anyhow!("no embedding model configured (set one in Settings)")
        })?;
        let model_path = std::path::PathBuf::from(&model);
        let config = llama_server::resolve_config(&self.inner.app_data_dir)?;
        let host = config.host.clone();
        let port = config.port.saturating_add(1);
        let base = format!("http://{host}:{port}");

        let mut guard = self.inner.embed_server.lock().await;
        if let Some(server) = guard.as_ref() {
            let healthy = self
                .inner
                .llama_client
                .get(format!("{base}/health"))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if server.model_path == model_path && healthy {
                return Ok(base);
            }
            if let Some(mut old) = guard.take() {
                llama_server::stop_embed_server(&mut old).await;
            }
        }
        let server = llama_server::start_embed_server(
            &self.inner.llama_client,
            &config.executable_path,
            &model_path,
            &host,
            port,
        )
        .await?;
        *guard = Some(server);
        Ok(base)
    }

    /// Embed a batch of texts via the local embedding server (starting it if
    /// needed). `is_query` selects the nomic query vs document task prefix.
    pub async fn embed_texts(&self, texts: &[String], is_query: bool) -> Result<Vec<Vec<f32>>> {
        let base = self.ensure_embed_server().await?;
        crate::embeddings::embed(
            &self.inner.llama_client,
            &base,
            "nomic-embed",
            texts,
            is_query,
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    /// LanceDB dir for the document RAG store (separate from the notes index so
    /// re-indexing notes never wipes ingested documents).
    fn rag_dir(&self) -> PathBuf {
        self.inner.app_data_dir.join("rag-index")
    }

    fn note_ingestion_manifest_path(&self) -> PathBuf {
        self.rag_dir().join(NOTE_INGEST_MANIFEST)
    }

    fn load_note_ingestion_manifest(&self) -> NoteIngestionManifest {
        fs::read_to_string(self.note_ingestion_manifest_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_note_ingestion_manifest(&self, manifest: &NoteIngestionManifest) -> Result<()> {
        fs::create_dir_all(self.rag_dir())?;
        let path = self.note_ingestion_manifest_path();
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, serde_json::to_vec_pretty(manifest)?)?;
        fs::rename(temp, path)?;
        Ok(())
    }

    fn embedding_fingerprint(&self) -> String {
        let Some(path) = crate::llama_server::embed_model_path(&self.inner.app_data_dir) else {
            return "hashed-768-v1".to_string();
        };
        let metadata = fs::metadata(&path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("nomic:{path}:{size}:{modified}")
    }

    fn note_ingestion_entry(&self, body: &str) -> NoteIngestionEntry {
        let digest = Sha256::digest(body.as_bytes());
        NoteIngestionEntry {
            body_hash: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            chunker: NOTE_CHUNKER_VERSION.to_string(),
            embedding: self.embedding_fingerprint(),
        }
    }

    async fn ensure_oversized_note_ingested(&self, note: &NoteDocument) -> Result<usize> {
        let lock = {
            let mut locks = self.inner.note_ingest_locks.lock();
            Arc::clone(
                locks
                    .entry(note.id.clone())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let _guard = lock.lock().await;
        let expected = self.note_ingestion_entry(&note.body);
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let manifest_matches = self
                .load_note_ingestion_manifest()
                .entries
                .get(&note.id)
                == Some(&expected);
            if manifest_matches
                && crate::rag::contains_document(&self.rag_dir(), &note.id)
                    .await
                    .unwrap_or(false)
            {
                return Ok(0);
            }
        }

        let _ = self.handle.emit(
            "ai://indexing_progress",
            serde_json::json!({
                "noteId": note.id,
                "status": "started",
                "message": "Indexing the complete oversized note…"
            }),
        );
        let chunks = match self
            .ingest_document(&note.id, &note.title, &note.body, false)
            .await
        {
            Ok(chunks) => chunks,
            Err(error) => {
                let _ = self.handle.emit(
                    "ai://indexing_progress",
                    serde_json::json!({
                        "noteId": note.id,
                        "status": "failed",
                        "message": error.to_string()
                    }),
                );
                return Err(error);
            }
        };
        {
            let _manifest_guard = self.inner.note_ingest_manifest_lock.lock().await;
            let mut manifest = self.load_note_ingestion_manifest();
            manifest.entries.insert(note.id.clone(), expected);
            self.save_note_ingestion_manifest(&manifest)?;
        }
        let _ = self.handle.emit(
            "ai://indexing_progress",
            serde_json::json!({
                "noteId": note.id,
                "status": "done",
                "chunks": chunks
            }),
        );
        Ok(chunks)
    }

    fn note_has_ingestion_entry(&self, note_id: &str) -> bool {
        self.load_note_ingestion_manifest()
            .entries
            .contains_key(note_id)
    }

    /// Ingest a document into the RAG store: chunk → embed → store. Re-ingesting
    /// the same doc_id replaces its chunks. `contextual` (for the working doc /
    /// "deep index") embeds each chunk with a one-sentence LLM context that
    /// situates it in the document, while STORING the clean chunk text. Plain
    /// (sources) skips that for speed. Returns the number of chunks stored.
    pub async fn ingest_document(
        &self,
        doc_id: &str,
        source: &str,
        text: &str,
        contextual: bool,
    ) -> Result<usize> {
        let chunks = crate::embeddings::chunk_text(text, 320, 50);
        if chunks.is_empty() {
            crate::rag::upsert_document(&self.rag_dir(), doc_id, Vec::new()).await?;
            return Ok(0);
        }

        // Contextual: one LLM summary of the doc, prepended to each chunk's
        // EMBED text (not its stored text) so the vector carries document context.
        let prefix = if contextual {
            let excerpt: String = text.chars().take(3000).collect();
            match self
                .run_llama_prompt(
                    "You write a single short sentence situating a document, for search context.",
                    &format!(
                        "Document:\n{excerpt}\n\nIn ONE sentence, say what this document is about (for retrieval context). Reply with only the sentence."
                    ),
                )
                .await
            {
                Ok(s) => format!("[Context: {}] ", s.trim()),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        };

        let embed_input: Vec<String> = chunks
            .iter()
            .map(|c| format!("{prefix}{}", c.text))
            .collect();
        let vectors = if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            self.embed_texts(&embed_input, false).await?
        } else {
            embed_input
                .iter()
                .map(|text| hashed_embedding(text))
                .collect()
        };
        let docs: Vec<crate::rag::DocChunk> = chunks
            .iter()
            .zip(vectors)
            .map(|(c, v)| crate::rag::DocChunk {
                doc_id: doc_id.to_string(),
                source: source.to_string(),
                chunk_index: c.index as i32,
                text: c.text.clone(),
                vector: v,
            })
            .collect();
        let n = docs.len();
        crate::rag::upsert_document(&self.rag_dir(), doc_id, docs).await?;
        Ok(n)
    }

    /// Remove a document's chunks from the RAG store.
    pub async fn delete_document(&self, doc_id: &str) -> Result<()> {
        crate::rag::upsert_document(&self.rag_dir(), doc_id, Vec::new()).await
    }

    /// Embedding for a note / query: real nomic vectors when an embed model is
    /// configured (semantic search), else the lexical hashed fallback. Always
    /// EMBEDDING_DIM-wide so both paths are interchangeable.
    async fn note_embedding(&self, text: &str, is_query: bool) -> Vec<f32> {
        if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            let input: String = text.chars().take(4000).collect();
            if let Ok(mut v) = self.embed_texts(&[input], is_query).await {
                if let Some(vec) = v.pop() {
                    if vec.len() == EMBEDDING_DIM as usize {
                        return vec;
                    }
                }
            }
        }
        hashed_embedding(text)
    }

    /// Re-embed a batch of notes with real nomic vectors when an embed model is
    /// configured (one batched call); otherwise leaves their hashed vectors.
    async fn reembed_notes(&self, notes: &mut [IndexedNote]) {
        if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_none() {
            return;
        }
        let inputs: Vec<String> = notes
            .iter()
            .map(|n| {
                let body: String = n.document.body.chars().take(4000).collect();
                format!("{}\n{}", n.document.title, body)
            })
            .collect();
        if let Ok(vectors) = self.embed_texts(&inputs, false).await {
            if vectors.len() == notes.len() {
                for (n, v) in notes.iter_mut().zip(vectors) {
                    if v.len() == EMBEDDING_DIM as usize {
                        n.vector = v;
                    }
                }
            }
        }
    }

    /// Retrieve the top-K document chunks most relevant to a query.
    pub async fn retrieve_chunks(
        &self,
        query: &str,
        k: usize,
        doc_id: Option<&str>,
    ) -> Result<Vec<crate::rag::RetrievedChunk>> {
        let qvec = if crate::llama_server::embed_model_path(&self.inner.app_data_dir).is_some() {
            self.embed_texts(&[query.to_string()], true)
                .await?
                .into_iter()
                .next()
                .unwrap_or_default()
        } else {
            hashed_embedding(query)
        };
        if qvec.is_empty() {
            return Ok(Vec::new());
        }
        crate::rag::search_hybrid(&self.rag_dir(), qvec, query, k, doc_id).await
    }

    async fn ensure_llama_server(&self, config: &llama_server::ResolvedLlamaConfig) -> Result<()> {
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

/// Seed a live conversation from the frontend's saved chat history on the first
/// turn of a session. Only text turns survive (tool results were never persisted),
/// but it keeps continuity after an app restart instead of starting blank.
fn chat_history_to_messages(chat_history: &[crate::models::ChatMessage]) -> Vec<serde_json::Value> {
    chat_history
        .iter()
        .filter(|m| m.error != Some(true) && m.is_streaming != Some(true))
        .filter(|m| !m.content.trim().is_empty())
        .filter(|m| m.role == "user" || m.role == "assistant")
        .rev()
        .take(MAX_CHAT_HISTORY_MESSAGES_IN_PROMPT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect()
}

fn assemble_note_context(title: &str, body_excerpt: &str, notebook_cells: Option<&str>) -> String {
    let mut context = format!("The note currently open is titled \"{title}\".");
    if let Some(cells) = notebook_cells {
        context.push_str(&format!("\n\n{cells}"));
    } else if body_excerpt.trim().is_empty() {
        context.push_str("\n\nThe note's CURRENT content is empty.");
    } else {
        context.push_str(&format!(
            "\n\nHere is the note's CURRENT content. When the user asks you to edit, change, format, fix, clean up, rewrite, shorten, expand, reorder, or remove part of the note, treat this as the text to modify — reproduce the parts that stay, apply the change, and pass the full result to write_note. (When you are only answering a question, use it as reference and do not echo it back verbatim.)\n--- CURRENT NOTE ---\n{body_excerpt}\n--- END CURRENT NOTE ---"
        ));
    }
    context
}

#[cfg(test)]
fn assemble_user_content(
    note_title: &str,
    mode_instruction: &str,
    turn_instructions: &str,
    question: &str,
) -> String {
    format!(
        "OPEN NOTE TITLE: {note_title:?}\n\n\
         INTERNAL TURN POLICY (not note metadata):\n{mode_instruction}\n\n\
         {turn_instructions}\n\n\
         USER REQUEST:\n{question}"
    )
}

pub(crate) fn is_note_mutation_tool(name: &str) -> bool {
    matches!(
        name,
        "write_note"
            | "append_note"
            | "prepend_note"
            | "replace_in_note"
            | "insert_after_line"
            | "delete_in_note"
            | "format_note"
            | "edit_notebook"
    )
}

fn authorize_tool_policy(
    name: &str,
    chat_mode: bool,
    append_only: bool,
    placement_edit: bool,
    has_selection: bool,
    is_notebook: bool,
    oversized_doc: bool,
    tools_supported: bool,
) -> Result<(), String> {
    if !tools_supported {
        return Err("Tool execution is disabled for this model.".to_string());
    }
    if !is_note_mutation_tool(name) {
        return Ok(());
    }
    if chat_mode {
        return Err("Note mutations are disabled in Chat mode.".to_string());
    }
    if oversized_doc && matches!(name, "write_note" | "format_note") {
        return Err(
            "This note exceeds the model context; retrieve the relevant region with \
             search_documents, then use replace_in_note, insert_after_line, delete_in_note, \
             append_note, or prepend_note."
                .to_string(),
        );
    }
    if is_notebook && name != "edit_notebook" {
        return Err("Notebook documents may only be mutated with edit_notebook.".to_string());
    }
    if !is_notebook && name == "edit_notebook" {
        return Err("edit_notebook is only available for notebook documents.".to_string());
    }
    if has_selection {
        let allowed = if placement_edit {
            name == "insert_after_line"
        } else {
            name == "write_note"
        };
        if !allowed {
            return Err("This mutation is outside the armed selection.".to_string());
        }
    } else if append_only && name != "append_note" {
        return Err("This is an append-only turn; only append_note is permitted.".to_string());
    }
    Ok(())
}

fn turn_contains_note_mutation(messages: &[serde_json::Value]) -> bool {
    let mutation_ids: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .filter(|call| {
            call["function"]["name"]
                .as_str()
                .is_some_and(is_note_mutation_tool)
        })
        .filter_map(|call| call["id"].as_str())
        .collect();
    messages.iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"]
                .as_str()
                .is_some_and(|id| mutation_ids.contains(id))
            && message["content"].as_str().is_some_and(|content| {
                content.starts_with("Note successfully updated with ID:")
                    || content.starts_with("Notebook updated (cell ")
            })
    })
}

/// Keep the most recent whole turns of a live conversation under a rough char
/// budget. A "turn" starts at a `user` message and includes the assistant/tool
/// messages that follow it, so trimming never orphans a tool result from its
/// assistant tool_call (which llama-server would reject).
fn trim_conversation(msgs: Vec<serde_json::Value>, max_chars: usize) -> Vec<serde_json::Value> {
    let mut groups: Vec<Vec<serde_json::Value>> = Vec::new();
    for m in msgs {
        if m["role"] == "user" || groups.is_empty() {
            groups.push(vec![m]);
        } else {
            groups.last_mut().unwrap().push(m);
        }
    }
    let cost = |m: &serde_json::Value| -> usize {
        let c = m["content"].as_str().map(|s| s.len()).unwrap_or(0);
        let a = m["tool_calls"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|t| {
                        t["function"]["arguments"]
                            .as_str()
                            .map(|s| s.len())
                            .unwrap_or(0)
                    })
                    .sum::<usize>()
            })
            .unwrap_or(0);
        c + a
    };
    let mut kept: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut total = 0usize;
    for g in groups.into_iter().rev() {
        let g_cost: usize = g.iter().map(&cost).sum();
        if !kept.is_empty() && total + g_cost > max_chars {
            break;
        }
        total += g_cost;
        kept.push(g);
    }
    kept.reverse();
    kept.into_iter().flatten().collect()
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

/// Directory holding a task's file for a given notebook (None = workspace root).
fn task_dir_for(workspace: &Path, notebook: Option<&str>) -> PathBuf {
    match notebook.map(str::trim).filter(|s| !s.is_empty()) {
        Some(nb) => workspace.join(nb).join("tasks"),
        None => workspace.join("tasks"),
    }
}

/// True for a `.../tasks/<name>.json` file (a task file we own).
fn is_task_file(path: &Path) -> bool {
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
fn notebook_from_task_path(workspace: &Path, path: &Path) -> Option<String> {
    let holder = path.parent()?.parent()?; // the folder that contains the `tasks` dir
    let rel = relative_to_workspace(workspace, holder);
    if rel.is_empty() || rel == "." {
        None
    } else {
        Some(rel)
    }
}

/// Delete every `<id>.json` task file across the workspace except `keep`.
fn remove_task_files(workspace: &Path, id: &str, keep: Option<&Path>) {
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

fn validate_task_id(id: &str) -> Result<()> {
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
fn validate_relative_dir(dir: &str) -> Result<()> {
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
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assemble_note_context, assemble_user_content, authorize_tool_policy,
        chat_history_to_messages, hashed_embedding, slugify, split_frontmatter, tokenize,
    };
    use crate::models::ChatMessage;

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

    #[test]
    fn restored_chat_history_contains_only_text_turns() {
        let history = vec![
            ChatMessage {
                role: "user".into(),
                content: "note A question".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".into(),
                content: "note A answer".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "tool".into(),
                content: "should not be restored".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".into(),
                content: "failed turn".into(),
                error: Some(true),
                ..Default::default()
            },
        ];

        let messages = chat_history_to_messages(&history);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["content"], "note A question");
        assert_eq!(messages[1]["content"], "note A answer");
    }

    #[test]
    fn note_context_always_contains_body_in_the_shared_shape() {
        let context = assemble_note_context("Today", "body text", None);
        assert!(context.contains("Today"));
        assert!(context.contains("body text"));
        assert!(context.contains("--- CURRENT NOTE ---"));
    }

    #[test]
    fn decorated_wire_user_content_is_stable_for_persistence() {
        let sent = assemble_user_content(
            "Project Aurora",
            "CHAT TURN POLICY: Never modify the note.",
            "TURN RULE",
            "What changed?",
        );
        let persisted = serde_json::json!({ "role": "user", "content": sent.clone() });
        assert_eq!(persisted["content"], sent);
        assert!(sent.starts_with("OPEN NOTE TITLE: \"Project Aurora\""));
        assert!(sent.contains("INTERNAL TURN POLICY (not note metadata):"));
        assert!(sent.contains("CHAT TURN POLICY:"));
        assert!(!sent.contains("COMPOSER MODE"));
        assert!(sent.ends_with("USER REQUEST:\nWhat changed?"));
    }

    #[test]
    fn empty_note_context_keeps_the_real_title() {
        let context = assemble_note_context("Untitled idea", "", None);
        assert!(context.contains("titled \"Untitled idea\""));
        assert!(context.contains("CURRENT content is empty"));
    }

    #[test]
    fn runtime_policy_blocks_unsafe_mutations() {
        assert!(authorize_tool_policy("write_note", true, false, false, false, false, false, true).is_err());
        assert!(authorize_tool_policy("write_note", false, true, false, false, false, false, true).is_err());
        assert!(authorize_tool_policy("append_note", false, true, false, false, false, false, true).is_ok());
        assert!(authorize_tool_policy("append_note", false, false, false, true, false, false, true).is_err());
        assert!(authorize_tool_policy("write_note", false, false, false, true, false, false, true).is_ok());
        assert!(authorize_tool_policy("insert_after_line", false, false, true, true, false, false, true).is_ok());
        assert!(authorize_tool_policy("write_note", false, false, false, false, true, false, true).is_err());
        assert!(authorize_tool_policy("edit_notebook", false, false, false, false, true, false, true).is_ok());
        assert!(authorize_tool_policy("read_note", false, false, false, false, false, false, false).is_err());
        let blocked = authorize_tool_policy("write_note", false, false, false, false, false, true, true)
            .unwrap_err();
        assert!(blocked.contains("exceeds the model context"));
        assert!(authorize_tool_policy("format_note", false, false, false, false, false, true, true).is_err());
        assert!(authorize_tool_policy("replace_in_note", false, false, false, false, false, true, true).is_ok());
    }
}
