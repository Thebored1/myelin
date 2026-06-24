pub mod git_history;
mod embeddings;
mod gguf;
mod llama_server;
mod model_profiles;
mod models;
mod state;
mod stream_chat;
mod tool_capability;
mod web_search;

use models::{AppSnapshot, NoteDocument, ProviderStatus, SearchResponse};
use state::AppState;
use tauri::{Emitter, Manager, State};

#[tauri::command]
async fn bootstrap(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    state.bootstrap().await.map_err(|error| error.to_string())
}

#[tauri::command]
fn set_require_tool_approval(state: State<'_, AppState>, require: bool) {
    state.set_require_tool_approval(require);
}

#[tauri::command]
fn resolve_tool_approval(state: State<'_, AppState>, id: String, approved: bool) {
    state.resolve_tool_approval(&id, approved);
}

#[tauri::command]
async fn set_workspace(
    state: State<'_, AppState>,
    workspace_path: String,
) -> Result<AppSnapshot, String> {
    state
        .set_workspace(workspace_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_llama_model_path(
    state: State<'_, AppState>,
    model_path: String,
) -> Result<(), String> {
    state
        .set_llama_model_path(model_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_llama_executable_path(
    state: State<'_, AppState>,
    executable_path: String,
) -> Result<(), String> {
    state
        .set_llama_executable_path(executable_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_llama_advanced_config(
    state: State<'_, AppState>,
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
) -> Result<(), String> {
    state
        .set_llama_advanced_config(
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
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_llama_devices(
    state: State<'_, AppState>,
    backend: String,
) -> Result<Vec<crate::llama_server::DeviceInfo>, String> {
    Ok(state.list_llama_devices(backend))
}

#[tauri::command]
async fn downloadable_backends(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.downloadable_backends())
}

#[tauri::command]
async fn download_llama_backend(
    state: State<'_, AppState>,
    backend: String,
) -> Result<(), String> {
    state
        .download_llama_backend(backend)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn create_note(
    state: State<'_, AppState>,
    title: String,
    source_pdf: Option<String>,
    extension: Option<String>,
) -> Result<NoteDocument, String> {
    state
        .create_note(title, source_pdf, extension)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn load_note(state: State<'_, AppState>, note_id: String) -> Result<NoteDocument, String> {
    state
        .load_note(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn save_note(
    state: State<'_, AppState>,
    note_id: String,
    title: String,
    tags: Vec<String>,
    body: String,
    source_pdf: Option<String>,
    annotations: Option<Vec<crate::models::PdfAnnotation>>,
) -> Result<NoteDocument, String> {
    state
        .save_note(note_id, title, tags, body, source_pdf, annotations)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn delete_note(state: State<'_, AppState>, note_id: String) -> Result<AppSnapshot, String> {
    state
        .delete_note(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn duplicate_note(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<NoteDocument, String> {
    state
        .duplicate_note(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn move_note(
    state: State<'_, AppState>,
    note_id: String,
    target_folder: String,
) -> Result<NoteDocument, String> {
    state
        .move_note(note_id, target_folder)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn reorder_note(
    state: State<'_, AppState>,
    note_id: String,
    direction: String,
) -> Result<AppSnapshot, String> {
    state
        .reorder_note(note_id, direction)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn search_notes(state: State<'_, AppState>, query: String) -> Result<SearchResponse, String> {
    state
        .search_notes(query)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_provider_status(state: State<'_, AppState>) -> Result<ProviderStatus, String> {
    state
        .provider_status()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn rebuild_index(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    state
        .rebuild_index()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn import_pdf_file(
    state: State<'_, AppState>,
    file_path: String,
) -> Result<NoteDocument, String> {
    state
        .import_pdf_file(file_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn save_pdf_annotations(
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
async fn get_snapshot(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    Ok(state.snapshot())
}

/// Warm the llama-server when a note is opened so the first chat is instant.
#[tauri::command]
async fn warm_llama_server(state: State<'_, AppState>) -> Result<(), String> {
    state.warm_llama_server().await.map_err(|error| error.to_string())
}

/// Stop the llama-server when the open note is closed, freeing RAM/VRAM.
#[tauri::command]
async fn stop_llama_server(state: State<'_, AppState>) -> Result<(), String> {
    state.stop_llama_server().await;
    Ok(())
}

/// Current SearXNG base URL for web search (empty = DuckDuckGo fallback).
#[tauri::command]
async fn get_searxng_url(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.searxng_url())
}

/// Set (or clear) the SearXNG base URL for web search.
#[tauri::command]
async fn set_searxng_url(state: State<'_, AppState>, url: Option<String>) -> Result<(), String> {
    state.set_searxng_url(url).map_err(|error| error.to_string())
}

/// Current embedding model GGUF path (empty = embeddings/RAG disabled).
#[tauri::command]
async fn get_embed_model_path(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.embed_model_path())
}

/// Set (or clear) the embedding model GGUF path.
#[tauri::command]
async fn set_embed_model_path(state: State<'_, AppState>, path: Option<String>) -> Result<(), String> {
    state.set_embed_model_path(path).map_err(|error| error.to_string())
}

#[tauri::command]
async fn extract_from_paste(
    _state: State<'_, AppState>,
    _note_id: String,
    _paste_content: String,
) -> Result<String, String> {
    Ok("Mocked extracted content from paste...".into())
}

#[tauri::command]
async fn get_all_note_documents(state: State<'_, AppState>) -> Result<Vec<NoteDocument>, String> {
    Ok(state.get_all_note_documents())
}

#[tauri::command]
async fn read_pdf_binary(state: State<'_, AppState>, note_id: String) -> Result<Vec<u8>, String> {
    state
        .read_pdf_binary(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn summarise_note(state: State<'_, AppState>, note_id: String) -> Result<String, String> {
    state
        .summarise_note(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ask_ai(
    state: State<'_, AppState>,
    note_id: String,
    question: String,
) -> Result<String, String> {
    state
        .ask_ai(note_id, question)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ask_ai_stream(
    state: State<'_, AppState>,
    note_id: String,
    question: String,
    request_id: String,
) -> Result<(), String> {
    state
        .ask_ai_stream(note_id, question, request_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn save_chat_history(
    state: State<'_, AppState>,
    note_id: String,
    chat_history: Vec<crate::models::ChatMessage>,
) -> Result<(), String> {
    state
        .save_chat_history(note_id, chat_history)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_note_history(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<Vec<crate::git_history::GitCommit>, String> {
    state
        .get_note_history(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_note_version(
    state: State<'_, AppState>,
    note_id: String,
    commit_hash: String,
) -> Result<String, String> {
    state
        .get_note_version(note_id, commit_hash)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn compile_latex(state: State<'_, AppState>, note_id: String) -> Result<Vec<u8>, String> {
    state
        .compile_latex(note_id)
        .await
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_state = AppState::new(app.handle().clone())?;
            app.manage(app_state);

            app.handle().plugin(tauri_plugin_dialog::init())?;

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        // Silence noisy dependency spans (e.g. lancedb's "load"
                        // tracing spans) while keeping our own INFO logs.
                        .level_for("tracing::span", log::LevelFilter::Warn)
                        .level_for("lance", log::LevelFilter::Warn)
                        .level_for("lance_table", log::LevelFilter::Warn)
                        .level_for("lance_core", log::LevelFilter::Warn)
                        .level_for("lance_io", log::LevelFilter::Warn)
                        .build(),
                )?;
            }

            app.emit("app://ready", "ready")?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            set_workspace,
            set_llama_model_path,
            set_llama_executable_path,
            set_llama_advanced_config,
            list_llama_devices,
            downloadable_backends,
            download_llama_backend,
            create_note,
            load_note,
            save_note,
            delete_note,
            duplicate_note,
            move_note,
            reorder_note,
            search_notes,
            get_provider_status,
            rebuild_index,
            get_snapshot,
            warm_llama_server,
            stop_llama_server,
            get_searxng_url,
            set_searxng_url,
            get_embed_model_path,
            set_embed_model_path,
            get_all_note_documents,
            extract_from_paste,
            read_pdf_binary,
            summarise_note,
            ask_ai,
            ask_ai_stream,
            save_chat_history,
            get_note_history,
            get_note_version,
            import_pdf_file,
            save_pdf_annotations,
            compile_latex,
            set_require_tool_approval,
            resolve_tool_approval
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
pub mod agent;
