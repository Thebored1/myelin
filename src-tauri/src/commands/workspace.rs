use crate::models::{AppSnapshot, NoteDocument, ProviderStatus};
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub(crate) fn get_ocr_status(state: State<'_, AppState>) -> crate::ocr::OcrStatus { state.ocr_status() }

#[tauri::command]
pub(crate) fn set_ocr_settings(state: State<'_, AppState>, settings: crate::ocr::OcrSettings) -> Result<crate::ocr::OcrStatus, String> {
    state.set_ocr_settings(settings).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn ocr_pdf_page(state: State<'_, AppState>, png: Vec<u8>, page_number: u32, language: Option<String>) -> Result<crate::ocr::OcrPageResult, String> {
    let mut settings = state.ocr_settings();
    if !settings.auto_low_text_pages { return Err("Automatic OCR is disabled in settings.".into()); }
    if let Some(language) = language { settings.language = language; }
    crate::ocr::recognize_png(&png, page_number, &settings)
}

#[tauri::command]
pub(crate) async fn get_provider_status(state: State<'_, AppState>) -> Result<ProviderStatus, String> {
    state
        .provider_status()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn rebuild_index(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    state
        .rebuild_index()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn import_pdf_file(
    state: State<'_, AppState>,
    file_path: String,
    notebook: Option<String>,
) -> Result<NoteDocument, String> {
    state
        .import_pdf_file(file_path, notebook)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn clone_pdf_for_attachment(
    state: State<'_, AppState>,
    note_id: String,
    notebook: Option<String>,
) -> Result<NoteDocument, String> {
    state
        .clone_pdf_for_attachment(note_id, notebook)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn save_pdf_annotations(
    state: State<'_, AppState>,
    note_id: String,
    annotations: Vec<crate::models::PdfAnnotation>,
) -> Result<(), String> {
    state
        .save_pdf_annotations(note_id, annotations)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn get_snapshot(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    Ok(state.snapshot())
}

/// Warm the llama-server when a note is opened so the first chat is instant.
#[tauri::command]
pub(crate) async fn warm_llama_server(
    state: State<'_, AppState>,
    note_id: Option<String>,
    interaction_mode: Option<String>,
    active_section: Option<crate::models::ActiveSection>,
) -> Result<(), String> {
    state
        .warm_llama_server_for_note(note_id, interaction_mode, active_section)
        .await
        .map_err(|error| error.to_string())
}

/// Stop the llama-server when the open note is closed, freeing RAM/VRAM.
#[tauri::command]
pub(crate) async fn stop_llama_server(state: State<'_, AppState>) -> Result<(), String> {
    state.stop_llama_server().await;
    Ok(())
}

/// Eagerly evaluate and persist one slot snapshot per document section (in the
/// background, with progress events) so asks restore the active section's KV
/// instead of evaluating it inline.
#[tauri::command]
pub(crate) async fn cache_note_sections(
    state: State<'_, AppState>,
    note_id: String,
    sections: Vec<crate::models::ActiveSection>,
    active_section_key: Option<String>,
    interaction_mode: Option<String>,
) -> Result<(), String> {
    state
        .cache_note_sections(note_id, sections, active_section_key, interaction_mode)
        .await
        .map_err(|error| error.to_string())
}

/// Current SearXNG base URL for web search (empty = DuckDuckGo fallback).
#[tauri::command]
pub(crate) async fn get_searxng_url(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.searxng_url())
}

/// Set (or clear) the SearXNG base URL for web search.
#[tauri::command]
pub(crate) async fn set_searxng_url(state: State<'_, AppState>, url: Option<String>) -> Result<(), String> {
    state
        .set_searxng_url(url)
        .map_err(|error| error.to_string())
}

/// Current embedding model GGUF path (empty = embeddings/RAG disabled).
#[tauri::command]
pub(crate) async fn get_embed_model_path(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.embed_model_path())
}

/// Set (or clear) the embedding model GGUF path.
#[tauri::command]
pub(crate) async fn set_embed_model_path(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<crate::embeddings::EmbeddingModelContract, String> {
    state
        .set_embed_model_path(path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn get_reranker_model_status(state: State<'_, AppState>) -> Result<crate::state::RerankerModelStatus, String> {
    Ok(state.get_reranker_model_status().await)
}

#[tauri::command]
pub(crate) async fn set_reranker_model_path(state: State<'_, AppState>, path: Option<String>) -> Result<crate::state::RerankerModelStatus, String> {
    state.set_reranker_model_path(path).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn list_built_in_models(state: State<'_, AppState>) -> Vec<crate::state::BuiltInModelInfo> { state.built_in_models() }

#[tauri::command]
pub(crate) async fn download_built_in_model(state: State<'_, AppState>, id: String) -> Result<String, String> { state.download_built_in_model(&id).await.map_err(|error| error.to_string()) }

/// Ingest a document into the RAG store (chunk → embed → store). `contextual`
/// for the working doc / deep index; plain for bulk sources. Returns chunk count.
#[tauri::command]
pub(crate) async fn ingest_document(
    state: State<'_, AppState>,
    doc_id: String,
    source: String,
    text: String,
    contextual: Option<bool>,
    format_hint: Option<crate::embeddings::DocumentFormat>,
) -> Result<usize, String> {
    state
        .ingest_document(&doc_id, &source, &text, contextual.unwrap_or(false), format_hint)
        .await
        .map_err(|error| error.to_string())
}

/// Index extracted note/PDF text unless the same content and embedding model
/// are already represented in the persistent RAG store.
#[tauri::command]
pub(crate) async fn ensure_document_ingested(
    state: State<'_, AppState>,
    doc_id: String,
    source: String,
    text: String,
    format_hint: Option<crate::embeddings::DocumentFormat>,
) -> Result<crate::state::DocumentIngestionResult, String> {
    state
        .ensure_document_ingested(&doc_id, &source, &text, format_hint)
        .await
        .map_err(|error| error.to_string())
}

/// Remove a document's chunks from the RAG store.
#[tauri::command]
pub(crate) async fn delete_document(state: State<'_, AppState>, doc_id: String) -> Result<(), String> {
    state
        .delete_document(&doc_id)
        .await
        .map_err(|error| error.to_string())
}

/// All known model profiles (bundled + user) for the compatibility list.
#[tauri::command]
pub(crate) async fn list_model_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<crate::model_profiles::ModelProfile>, String> {
    Ok(state.list_model_profiles())
}

#[tauri::command]
pub(crate) async fn get_all_note_documents(state: State<'_, AppState>) -> Result<Vec<NoteDocument>, String> {
    Ok(state.get_all_note_documents())
}

#[tauri::command]
pub(crate) async fn read_pdf_binary(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<tauri::ipc::Response, String> {
    // Return raw bytes over IPC (ArrayBuffer on the JS side) instead of a JSON
    // number[] — a PDF as JSON integers is several times its real size to encode.
    state
        .read_pdf_binary(note_id)
        .await
        .map(tauri::ipc::Response::new)
        .map_err(|error| error.to_string())
}
