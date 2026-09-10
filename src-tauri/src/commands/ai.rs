use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub(crate) fn cancel_ai(state: State<'_, AppState>) {
    state.request_ai_cancel();
}

#[tauri::command]
pub(crate) async fn ask_ai_stream(
    state: State<'_, AppState>,
    note_id: String,
    question: String,
    request_id: String,
    selection: Option<crate::agent::SelectionArg>,
    doc_type: Option<String>,
    interaction_mode: Option<String>,
    active_section: Option<crate::models::ActiveSection>,
) -> Result<(), String> {
    state
        .ask_ai_stream(
            note_id,
            question,
            request_id,
            selection,
            doc_type,
            interaction_mode,
            active_section,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn save_chat_history(
    state: State<'_, AppState>,
    note_id: String,
    chat_history: Vec<crate::models::ChatMessage>,
) -> Result<(), String> {
    state
        .save_chat_history(note_id, chat_history)
        .await
        .map_err(|error| error.to_string())
}

/// Drop the in-memory model conversation for a note. The next AI turn rebuilds
/// its context from the authoritative persisted chat history.
#[tauri::command]
pub(crate) fn clear_ai_conversation(state: State<'_, AppState>, note_id: String) {
    if let Err(error) = state.clear_conversation(&note_id) {
        log::warn!("could not clear AI conversation: {error}");
    }
}

#[tauri::command]
pub(crate) async fn get_note_history(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<Vec<crate::git_history::GitCommit>, String> {
    state
        .get_note_history(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn get_note_version(
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
pub(crate) async fn compile_latex(
    state: State<'_, AppState>,
    note_id: String,
    source: Option<String>,
) -> Result<tauri::ipc::Response, String> {
    // Raw bytes over IPC (see read_pdf_binary) — the compiled PDF can be large.
    state
        .compile_latex(note_id, source)
        .await
        .map(tauri::ipc::Response::new)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn import_latex_asset(
    state: State<'_, AppState>,
    note_id: String,
    source_path: String,
) -> Result<String, String> {
    state
        .import_latex_asset(note_id, source_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn tectonic_cache_status(state: State<'_, AppState>) -> crate::state::TectonicCacheStatus {
    state.tectonic_cache_status()
}

#[tauri::command]
pub(crate) async fn prewarm_tectonic(state: State<'_, AppState>) -> Result<(), String> {
    state.prewarm_tectonic().await.map_err(|e| e.to_string())
}
