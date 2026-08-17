use crate::models;
use crate::state::AppState;
use tauri::{Emitter, Manager, State};

pub(crate) fn quit_app(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister_all();
    // Persist the last slot snapshot before the server dies (bounded to 3s), so
    // the next boot restores the last conversation's KV prefix per note. Then
    // kill the child processes and exit.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        app.state::<AppState>().save_active_slot_before_quit().await;
        app.state::<AppState>().shutdown_servers_sync();
        app.exit(0);
    });
}

pub(crate) fn toggle_quick_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("quick") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.center();
            let _ = win.show();
            let _ = win.set_focus();
            let _ = win.emit("quick://focus", ());
        }
        return;
    }
    // First press: build the window (on the main thread, required by GTK/Linux).
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        use tauri::{WebviewUrl, WebviewWindowBuilder};
        let _ = WebviewWindowBuilder::new(&app2, "quick", WebviewUrl::App("quick".into()))
            .title("Quick Capture")
            .inner_size(660.0, 600.0)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .center()
            .focused(true)
            .build();
    });
}

#[tauri::command]
pub(crate) fn get_quick_shortcut(state: State<'_, AppState>) -> String {
    state.quick_shortcut()
}

#[tauri::command]
pub(crate) fn set_quick_shortcut(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    shortcut: String,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let parsed: tauri_plugin_global_shortcut::Shortcut = shortcut
        .parse()
        .map_err(|_| format!("Invalid shortcut: {shortcut}"))?;
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    gs.register(parsed).map_err(|e| e.to_string())?;
    state
        .set_quick_shortcut(shortcut)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) fn list_tasks(state: State<'_, AppState>) -> Result<Vec<models::Task>, String> {
    state.list_tasks().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn save_task(state: State<'_, AppState>, task: models::Task) -> Result<models::Task, String> {
    state.save_task(task).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn delete_task(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.delete_task(id).map_err(|e| e.to_string())
}
