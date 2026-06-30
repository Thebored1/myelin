use serde::{Deserialize, Serialize};

/// A single bullet-point subtask. Persisted inside its parent [`Task`] file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Subtask {
    pub id: String,
    pub text: String,
    pub done: bool,
}

/// A task. Each task is stored as its own self-contained JSON file under
/// `<workspace>/tasks/<id>.json` (file-per-item: portable, separately copyable).
/// `#[serde(default)]` lets the frontend send partial objects (e.g. just a title).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// Free-form details / sub-paragraph (markdown allowed). Empty = none.
    pub details: String,
    /// Notebook (workspace folder) this task belongs to, e.g. "Work" or
    /// "Work/ProjectA". None = the default top-level task list (not in a notebook).
    /// Determines the on-disk location: `<workspace>/<notebook>/tasks/<id>.json`
    /// (or `<workspace>/tasks/<id>.json` when None).
    pub notebook: Option<String>,
    /// Deadline as an ISO-8601 string — date only ("2026-07-01") or date+time
    /// ("2026-07-01T14:30"). None = no deadline.
    pub due: Option<String>,
    pub done: bool,
    /// Ordered bullet-point subtasks.
    pub subtasks: Vec<Subtask>,
    /// Manual sort position (ascending). Frontend sets it for drag-reorder.
    pub position: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatTool {
    pub name: String,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ChatTool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<NoteSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NoteSnapshot {
    pub note_body: String,
    pub draft_title: String,
    pub draft_tags: String,
    pub chat_length: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PdfAnnotation {
    pub id: String,
    pub page: i32,
    #[serde(rename = "type")]
    pub type_: String,
    pub points: Option<Vec<(f64, f64)>>,
    pub rect: Option<(f64, f64, f64, f64)>,
    pub rects: Option<Vec<(f64, f64, f64, f64)>>,
    pub color: String,
    pub stroke_width: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Backlink {
    pub source_id: String,
    pub source_title: String,
    pub target_block: Option<String>,
    pub context_excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub workspace_path: Option<String>,
    pub notes: Vec<NoteSummary>,
    pub custom_note_order: Vec<String>,
    pub library_facets: LibraryFacets,
    pub provider_status: ProviderStatus,
    pub index_state: IndexState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IndexState {
    pub is_indexing: bool,
    pub last_indexed_at: Option<String>,
    pub note_count: usize,
    pub backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub active_provider: String,
    pub available_providers: Vec<String>,
    pub healthy: bool,
    pub detail: String,
    pub config: Option<crate::llama_server::WorkspaceLlamaConfig>,
    pub resolved: Option<crate::llama_server::ResolvedLlamaConfig>,
    /// Backend actually running ("cuda"/"vulkan"/"metal"/"cpu"), or the
    /// preferred backend when no server is running yet.
    #[serde(default)]
    pub active_backend: Option<String>,
    /// Whether an NVIDIA GPU was detected on this machine.
    #[serde(default)]
    pub nvidia_detected: bool,
    /// Whether GPU acceleration is usable on this machine at all.
    #[serde(default)]
    pub gpu_available: bool,
    /// GPU adapter names detected on this machine (for display).
    #[serde(default)]
    pub gpus: Vec<String>,
    /// Backend builds installed ("cuda"/"vulkan"/"metal"/"cpu").
    #[serde(default)]
    pub installed_backends: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub folder: String,
    pub excerpt: String,
    pub relative_path: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub source_pdf: Option<String>,
    #[serde(default)]
    pub backlinks: Vec<Backlink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFacets {
    pub folders: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NoteDocument {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub body: String,
    pub relative_path: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub source_pdf: Option<String>,
    #[serde(default)]
    pub backlinks: Vec<Backlink>,
    #[serde(default)]
    pub annotations: Vec<PdfAnnotation>,
    #[serde(default)]
    pub chat_history: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub note: NoteSummary,
    pub score: f32,
    pub reason: String,
}
