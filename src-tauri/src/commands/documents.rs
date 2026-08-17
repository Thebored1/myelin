use crate::models::{AppSnapshot, NoteDocument, SearchResponse};
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub(crate) async fn create_note(
    state: State<'_, AppState>,
    title: String,
    source_pdf: Option<String>,
    extension: Option<String>,
    notebook: Option<String>,
) -> Result<NoteDocument, String> {
    state
        .create_note(title, source_pdf, extension, notebook)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn create_notebook(state: State<'_, AppState>, name: String) -> Result<Vec<String>, String> {
    state
        .create_notebook(name)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn list_notebooks(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state.list_notebooks().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn load_note(state: State<'_, AppState>, note_id: String) -> Result<NoteDocument, String> {
    state
        .load_note(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn save_note(
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
pub(crate) async fn delete_note(state: State<'_, AppState>, note_id: String) -> Result<AppSnapshot, String> {
    state
        .delete_note(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn duplicate_note(
    state: State<'_, AppState>,
    note_id: String,
) -> Result<NoteDocument, String> {
    state
        .duplicate_note(note_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn move_note(
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
pub(crate) async fn reorder_note(
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
pub(crate) async fn search_notes(state: State<'_, AppState>, query: String) -> Result<SearchResponse, String> {
    state
        .search_notes(query)
        .await
        .map_err(|error| error.to_string())
}
