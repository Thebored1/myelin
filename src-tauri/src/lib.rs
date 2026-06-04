mod models;
mod state;

use models::{AppSnapshot, NoteDocument, ProviderStatus, SearchResponse};
use state::AppState;
use tauri::{Emitter, Manager, State};

#[tauri::command]
async fn bootstrap(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    state.bootstrap().await.map_err(|error| error.to_string())
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
async fn create_note(state: State<'_, AppState>, title: String) -> Result<NoteDocument, String> {
    state
        .create_note(title)
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
) -> Result<NoteDocument, String> {
    state
        .save_note(note_id, title, tags, body)
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
                        .build(),
                )?;
            }

            app.emit("app://ready", "ready")?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            set_workspace,
            create_note,
            load_note,
            save_note,
            delete_note,
            duplicate_note,
            move_note,
            reorder_note,
            search_notes,
            get_provider_status,
            rebuild_index
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
