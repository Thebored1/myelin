pub(crate) use crate::llama_server::{self, ManagedLlamaServer};
pub(crate) use crate::models::{
    AppSnapshot, Backlink, ChatTool, IndexState, LibraryFacets, NoteDocument, NoteSummary,
    ProviderStatus, SearchResponse, SearchResult, StorageIssue, Task,
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
use super::latex_support::*;

#[derive(Clone)]
pub struct AppState {
    pub handle: AppHandle,
	pub(crate) inner: Arc<InnerState>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingIndexRequest {
    pub(crate) generation: u64,
    pub(crate) workspace: PathBuf,
    pub(crate) debounce: bool,
}

#[derive(Debug)]
pub(crate) struct IndexCompletion {
    pub(crate) generation: u64,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct IndexScheduler {
    pub(crate) next_generation: u64,
    pub(crate) next_worker_id: u64,
    pub(crate) pending: Option<PendingIndexRequest>,
    pub(crate) in_flight: Option<PendingIndexRequest>,
    pub(crate) worker_running: bool,
    pub(crate) active_worker_id: Option<u64>,
    pub(crate) last_completion: Option<IndexCompletion>,
}

#[derive(Debug)]
pub(crate) struct IndexRequestReceipt {
    pub(crate) generation: u64,
    pub(crate) spawn_worker: bool,
    pub(crate) worker_id: Option<u64>,
}

impl IndexScheduler {
	pub(crate) fn request(
        &mut self,
        workspace: PathBuf,
        debounce: bool,
    ) -> IndexRequestReceipt {
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        if let Some(pending) = self.pending.as_mut() {
            pending.generation = generation;
            pending.workspace = workspace;
            // Once any caller asks for an immediate pass, later filesystem
            // noise must not turn that batch back into a delayed one.
            pending.debounce &= debounce;
        } else {
            self.pending = Some(PendingIndexRequest {
                generation,
                workspace,
                debounce,
            });
        }

        let spawn_worker = !self.worker_running;
        let worker_id = if spawn_worker {
            self.worker_running = true;
            self.next_worker_id = self.next_worker_id.saturating_add(1);
            self.active_worker_id = Some(self.next_worker_id);
            self.active_worker_id
        } else {
            None
        };

        IndexRequestReceipt {
            generation,
            spawn_worker,
            worker_id,
        }
    }

	pub(crate) fn take_pending(&mut self) -> Option<PendingIndexRequest> {
        let request = self.pending.take()?;
        self.in_flight = Some(request.clone());
        Some(request)
    }

	pub(crate) fn finish_pass(&mut self, generation: u64, error: Option<String>) -> bool {
        self.in_flight = None;
        self.last_completion = Some(IndexCompletion { generation, error });
        let has_pending_rerun = self.pending.is_some();
        if !has_pending_rerun {
            self.worker_running = false;
            self.active_worker_id = None;
        }
        has_pending_rerun
    }

	pub(crate) fn completion_for(&self, generation: u64) -> Option<Result<(), String>> {
        let completion = self.last_completion.as_ref()?;
        if completion.generation < generation {
            return None;
        }
        Some(match &completion.error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        })
    }

	pub(crate) fn abort_worker(&mut self, worker_id: u64, message: &str) -> bool {
        if !self.worker_running || self.active_worker_id != Some(worker_id) {
            return false;
        }
        self.worker_running = false;
        self.active_worker_id = None;
        // A cancelled worker may have already claimed the only dirty request.
        // Put it back unless a newer pending full scan already covers it.
        if self.pending.is_none() {
            self.pending = self.in_flight.take();
        } else {
            self.in_flight = None;
        }
        self.last_completion = Some(IndexCompletion {
            generation: self.next_generation,
            error: Some(message.to_string()),
        });
        true
    }
}

pub(crate) struct InnerState {
    pub(crate) app_data_dir: PathBuf,
    pub(crate) persistence_lock: Mutex<()>,
    pub(crate) runtime: RwLock<RuntimeState>,
    pub(crate) watcher: Mutex<Option<RecommendedWatcher>>,
    pub(crate) index_lock: AsyncMutex<()>,
    pub(crate) index_scheduler: Mutex<IndexScheduler>,
    pub(crate) index_completion: tokio::sync::Notify,
    // Serialises Tectonic runs: concurrent compiles share one format-cache dir and
    // would corrupt it if they built the format at the same time.
    pub(crate) tectonic_lock: AsyncMutex<()>,
    pub(crate) llama_server: AsyncMutex<Option<ManagedLlamaServer>>,
    /// Serializes complete AI startup across boot, note-open, and first chat.
    pub(crate) ai_pipeline_lock: AsyncMutex<()>,
    pub(crate) ai_pipeline_ready: std::sync::atomic::AtomicBool,
    pub(crate) embed_server: AsyncMutex<Option<crate::llama_server::ManagedEmbedServer>>,
    /// The openharn-myelin agent sidecar: a long-lived `openharn-myelin` process
    /// that runs the agent loop and calls back to Myelin for tool execution.
	pub(crate) sidecar: AsyncMutex<Option<ManagedSidecar>>,
    /// Chat tools read shared per-turn context (open note, selection, question),
    /// so concurrent requests would overwrite or clear each other's target.
    pub(crate) chat_lock: AsyncMutex<()>,
    /// Serializes every operation that reads or mutates llama-server slot 0.
    /// The page pre-cache, prompt warm-up, real chat, and slot save/restore APIs
    /// all target the same slot; overlapping them makes the in-memory resident
    /// page marker disagree with the server's actual KV state.
    pub(crate) llama_slot_lock: AsyncMutex<()>,
    /// A real user turn temporarily preempts background section priming. The
    /// scan remains alive and resumes at its current profile afterward.
    pub(crate) section_cache_preempt: std::sync::atomic::AtomicBool,
    pub(crate) section_cache_resume: tokio::sync::Notify,
    /// Live mirror of the persisted openharn sidecar settings, refreshed on save.
    pub(crate) openharn_settings: Mutex<OpenharnSettings>,
    pub(crate) background_settings: Mutex<BackgroundSettings>,
    pub(crate) llama_client: Client,
    pub(crate) chat_tools: Mutex<Vec<ChatTool>>,
    pub(crate) latest_chat_question: Mutex<Option<String>>,
    /// The editor text selection the user armed for the current chat turn, if any.
    /// Read by the write_note tool to scope an edit to just that span.
    pub(crate) current_selection: Mutex<Option<crate::agent::SelectionArg>>,
    /// The working-doc type of the open document this turn: "md" | "tex" | "ipynb".
    /// Steers the prompt (LaTeX/notebook vs Markdown) and notebook-aware tools.
    pub(crate) current_doc_type: Mutex<Option<String>>,
    pub(crate) current_note_id: Mutex<Option<String>>,
    pub(crate) cancel_ai: std::sync::atomic::AtomicBool,
    pub(crate) cancel_notify: tokio::sync::Notify,
    pub(crate) require_tool_approval: std::sync::atomic::AtomicBool,
    /// Runtime mirror of config.deterministic_tools, refreshed each chat turn, so
    /// tools (e.g. the write guard) can read it without re-resolving the config.
    pub(crate) deterministic_tools: std::sync::atomic::AtomicBool,
    /// Runtime mirror of config.tool_gating (per-message tool gating), refreshed
    /// each chat turn alongside `deterministic_tools`.
    pub(crate) tool_gating: std::sync::atomic::AtomicBool,
    /// Trusted execution policy for the current serialized chat turn. The model
    /// sees a stable tool schema for prompt-cache reuse; these flags enforce the
    /// mode-specific mutation boundary when a tool call reaches Rust.
    pub(crate) targeted_write: std::sync::atomic::AtomicBool,
    pub(crate) chat_mode: std::sync::atomic::AtomicBool,
    pub(crate) append_only: std::sync::atomic::AtomicBool,
    pub(crate) placement_edit: std::sync::atomic::AtomicBool,
    pub(crate) oversized_doc: std::sync::atomic::AtomicBool,
    pub(crate) tools_supported: std::sync::atomic::AtomicBool,
    pub(crate) note_ingest_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    pub(crate) note_ingest_manifest_lock: AsyncMutex<()>,
    /// Persistent cache for normalized question embeddings. Document vectors
    /// live in LanceDB; this avoids repeating the embedding-server call for
    /// repeated retrieval queries.
    pub(crate) query_embedding_cache: Mutex<Option<QueryEmbeddingCacheFile>>,
    /// At most one note-prefix warm-up should be consuming llama-server at once.
    /// The key is the exact request prefix; a newer prefix supersedes an older one.
    pub(crate) prompt_warmup: Mutex<Option<(u64, tokio::task::JoinHandle<()>)>>,
    /// (note_id, identity) of the most recent successful slot snapshot. Slot 0
    /// still holds that snapshot's KV when the app quits, so the quit path can
    /// re-persist it before llama-server dies; the next boot restores it.
    pub(crate) last_slot_save: Mutex<Option<(String, String)>>,
    /// Prefix currently resident in slot 0. A section is restored only when
    /// this identity changes; restoring on every turn would throw away the
    /// in-RAM history and defeat the point of the cache.
    pub(crate) active_slot_cache: Mutex<Option<ActiveSlotCache>>,
    /// The whole-document section pre-cache scan (one prime+save per section).
    /// Runs independently of the prompt warm-up: it must NOT be aborted by a
    /// chat turn (finish_prompt_warmup), only by note close / server stop.
    pub(crate) section_cache: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) pending_approvals: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// Per-note live conversation as the REAL message array (system-less): user
    /// turns, assistant turns with tool_calls, and the tool RESULTS. The frontend's
    /// chat_history keeps only text replies, so this is what lets the model keep
    /// coherent context across turns — search/fetch results stay as real `tool`
    /// messages instead of being flattened to a vague summary and lost. Re-sent each
    /// turn so llama-server reuses the cached prefix (KV cache). Keyed by note id.
    pub(crate) conversations: Mutex<HashMap<String, Vec<serde_json::Value>>>,
}

/// Makes cancellation or an unexpected worker exit observable to generation
/// waiters instead of leaving them asleep forever. Pending dirty work remains in
/// the scheduler so a later request can restart it.
pub(crate) struct IndexWorkerGuard {
    pub(crate) inner: Arc<InnerState>,
    pub(crate) worker_id: u64,
    pub(crate) armed: bool,
}

impl IndexWorkerGuard {
	pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for IndexWorkerGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let aborted = self
            .inner
            .index_scheduler
            .lock()
            .abort_worker(
                self.worker_id,
                "index worker stopped before completing its queued work",
            );
        if aborted {
            self.inner.index_completion.notify_waiters();
        }
    }
}

#[derive(Default)]
pub(crate) struct RuntimeState {
    pub(crate) workspace_path: Option<PathBuf>,
    pub(crate) notes: HashMap<String, IndexedNote>,
    pub(crate) custom_note_order: Vec<String>,
    pub(crate) index_state: IndexState,
    pub(crate) storage_issues: Vec<StorageIssue>,
}

#[derive(Clone)]
pub(crate) struct IndexedNote {
    pub(crate) document: NoteDocument,
    pub(crate) vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct PersistedSettings {
    pub(crate) workspace_path: Option<String>,
    pub(crate) custom_note_order: Vec<String>,
    #[serde(default, skip_serializing_if = "OpenharnSettings::is_default")]
    pub(crate) openharn: OpenharnSettings,
    #[serde(default, skip_serializing_if = "BackgroundSettings::is_default")]
    pub background: BackgroundSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct NoteIngestionManifest {
    pub(crate) entries: HashMap<String, NoteIngestionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct NoteIngestionEntry {
    pub(crate) body_hash: String,
    pub(crate) chunker: String,
    pub(crate) embedding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct QueryEmbeddingCacheFile {
    pub(crate) embedding: String,
    pub(crate) entries: HashMap<String, Vec<f32>>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentIngestionResult {
    pub status: String,
    pub chunks: usize,
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
    /// Optional plain OpenAI-compatible endpoint for simple Chat/Write turns.
    /// When enabled, the sidecar omits llama.cpp slot and prompt-cache fields.
    #[serde(default)]
    pub external_enabled: bool,
    #[serde(default)]
    pub external_base_url: Option<String>,
    #[serde(default)]
    pub external_model: Option<String>,
    /// Stored locally in settings.json and never included in debug events.
    #[serde(default)]
    pub external_api_key: Option<String>,
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

pub(crate) fn default_tool_mode() -> String {
    "auto".to_string()
}

impl OpenharnSettings {
    pub fn external_ready(&self) -> bool {
        self.external_enabled
            && self
                .external_base_url
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .external_model
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }

    fn is_default(&self) -> bool {
        *self == OpenharnSettings::default()
    }
}

/// Project the applied schema-backed agent settings into the live sidecar
/// settings. Legacy external-endpoint fields are intentionally preserved: they
/// are still stored in settings.json and are not part of the managed runtime
/// configuration schema.
pub(crate) fn project_ai_agent_settings(
    legacy: &OpenharnSettings,
    agent: &crate::ai_config::AgentConfig,
) -> OpenharnSettings {
    let mut settings = legacy.clone();
    if let Some(value) = agent.port { settings.port = Some(value); }
    if let Some(value) = agent.bin_path.clone() { settings.bin_path = Some(value); }
    if let Some(value) = agent.tool_mode.clone() { settings.tool_mode = value; }
    if let Some(value) = agent.strict { settings.strict = value; }
    if let Some(value) = agent.prompt_tools { settings.prompt_tools = value; }
    if let Some(value) = agent.call_only { settings.call_only = value; }
    if let Some(value) = agent.no_think { settings.no_think = value; }
    if let Some(value) = agent.narrow { settings.narrow = value; }
    if let Some(value) = agent.slm { settings.slm = value; }
    if let Some(value) = agent.friendly_results { settings.friendly_results = value; }
    if let Some(value) = agent.max_calls { settings.max_calls = Some(value); }
    if let Some(value) = agent.total_max { settings.total_max = Some(value); }
    if let Some(value) = agent.tool_timeout_secs { settings.tool_timeout_secs = Some(value); }
    if let Some(value) = agent.generation_timeout_secs { settings.generation_timeout_secs = Some(value); }
    if let Some(value) = agent.tool_subset.clone() { settings.tool_subset = Some(value); }
    if let Some(value) = agent.base_url_override.clone() { settings.base_url = Some(value); }
    if let Some(value) = agent.tool_choice.clone() { settings.tool_choice = Some(value); }
    if let Some(value) = agent.template_kwargs.clone() { settings.template_kwargs = Some(value); }
    settings
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct Frontmatter {
    pub(crate) id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) source_pdf: Option<String>,
}

/// Metadata for formats whose native grammar cannot contain YAML frontmatter.
/// The path is included for diagnostics and to make an accidental hash collision
/// fail closed when the sidecar is read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DocumentMetadataSidecar {
    #[serde(default = "metadata_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(flatten)]
    pub(crate) metadata: Frontmatter,
    pub(crate) relative_path: String,
}

fn metadata_schema_version() -> u32 { 2 }

pub(crate) type NativeMetadataSidecar = DocumentMetadataSidecar;
