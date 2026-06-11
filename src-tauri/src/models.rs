use serde::{Deserialize, Serialize};

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
