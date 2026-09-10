use crate::models::AppSnapshot;
use crate::state::{AppState, BackgroundSettings};
use tauri::State;

#[tauri::command]
pub(crate) fn get_background_settings(state: State<'_, AppState>) -> BackgroundSettings {
    state.background_settings()
}

#[tauri::command]
pub(crate) fn set_start_with_system(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|e| e.to_string())?;
    state
        .set_background_settings(BackgroundSettings {
            start_with_system: enabled,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn bootstrap(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    state.bootstrap().await.map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_appearance_settings(
    state: State<'_, AppState>,
) -> Result<crate::models::AppearanceSettings, String> {
    state
        .get_appearance_settings()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn save_color_theme(
    state: State<'_, AppState>,
    theme: crate::models::ColorTheme,
) -> Result<crate::models::AppearanceSettings, String> {
    state
        .save_color_theme(theme)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn delete_color_theme(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::models::AppearanceSettings, String> {
    state
        .delete_color_theme(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn set_active_color_theme(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::models::AppearanceSettings, String> {
    state
        .set_active_color_theme(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_ai_config_status(state: State<'_, AppState>) -> crate::ai_config::AiConfigStatus {
    state.ai_config_status()
}

#[tauri::command]
pub(crate) async fn validate_ai_config(
    state: State<'_, AppState>,
) -> Result<crate::ai_config::AiConfigStatus, String> {
    state.validate_ai_config().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn apply_ai_config(
    state: State<'_, AppState>,
    candidate_hash: String,
) -> Result<crate::ai_config::AiConfigStatus, String> {
    state
        .apply_ai_config(&candidate_hash)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn open_ai_config_file(state: State<'_, AppState>) -> Result<String, String> {
    state.ensure_ai_config().map_err(|e| e.to_string())?;
    let path = state.ai_config_status().config_path;
    Ok(path)
}

#[tauri::command]
pub(crate) fn read_ai_config(state: State<'_, AppState>) -> Result<String, String> {
    state.ensure_ai_config().map_err(|e| e.to_string())?;
    std::fs::read_to_string(state.ai_config_status().config_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn save_ai_config(state: State<'_, AppState>, contents: String) -> Result<(), String> {
    state.ensure_ai_config().map_err(|e| e.to_string())?;
    let config: crate::ai_config::AiConfigFile =
        serde_json::from_str(&contents).map_err(|e| format!("invalid JSON: {e}"))?;
    crate::ai_config::write_atomic(
        std::path::Path::new(&state.ai_config_status().config_path),
        &config,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn install_ai_runtime(state: State<'_, AppState>, runtime_id: String) -> Result<(), String> {
    state
        .install_ai_runtime(&runtime_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn set_require_tool_approval(state: State<'_, AppState>, require: bool) {
    state.set_require_tool_approval(require);
}

#[tauri::command]
pub(crate) async fn set_deterministic_tools(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    state
        .set_deterministic_tools(enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn set_tool_gating(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    state
        .set_tool_gating(enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn set_prompt_cache(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    state
        .set_prompt_cache(enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn llama_cache_status(state: State<'_, AppState>) -> crate::state::LlamaCacheStatus {
    state.llama_cache_status()
}

#[tauri::command]
pub(crate) async fn clear_llama_cache(state: State<'_, AppState>) -> Result<(), String> {
    state.clear_llama_cache().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn resolve_tool_approval(state: State<'_, AppState>, id: String, approved: bool) {
    state.resolve_tool_approval(&id, approved);
}

#[tauri::command]
pub(crate) async fn set_workspace(
    state: State<'_, AppState>,
    workspace_path: String,
) -> Result<AppSnapshot, String> {
    state
        .set_workspace(workspace_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn set_llama_model_path(
    state: State<'_, AppState>,
    model_path: String,
) -> Result<(), String> {
    state
        .set_llama_model_path(model_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn set_llama_executable_path(
    state: State<'_, AppState>,
    executable_path: String,
) -> Result<(), String> {
    state
        .set_llama_executable_path(executable_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn set_llama_advanced_config(
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
pub(crate) async fn list_llama_devices(
    state: State<'_, AppState>,
    backend: String,
) -> Result<Vec<crate::llama_server::DeviceInfo>, String> {
    Ok(state.list_llama_devices(backend))
}

#[tauri::command]
pub(crate) async fn get_openharn_settings(
    state: State<'_, AppState>,
) -> Result<crate::state::OpenharnSettings, String> {
    Ok(state.openharn_settings())
}

#[tauri::command]
pub(crate) async fn set_openharn_settings(
    state: State<'_, AppState>,
    settings: crate::state::OpenharnSettings,
) -> Result<(), String> {
    state
        .set_openharn_settings(settings)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn downloadable_backends(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.downloadable_backends())
}

#[tauri::command]
pub(crate) async fn downloadable_bee_backends(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.downloadable_bee_backends())
}

#[tauri::command]
pub(crate) async fn download_llama_backend(state: State<'_, AppState>, backend: String) -> Result<(), String> {
    state
        .download_llama_backend(backend)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn download_bee_backend(state: State<'_, AppState>, backend: String) -> Result<(), String> {
    state
        .download_bee_backend(backend)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn set_inference_engine(state: State<'_, AppState>, engine: String) -> Result<(), String> {
    state
        .set_inference_engine(engine)
        .await
        .map_err(|error| error.to_string())
}
