pub mod ai_config;
pub mod ai_turn;
mod embeddings;
mod gguf;
pub mod git_history;
pub mod llama_server;
mod model_profiles;
mod models;
mod note_prompt;
mod notebook;
mod ocr;
mod persistence;
mod rag;
mod rag_copy;
pub mod retrieval_eval;
pub mod retrieval_pipeline;
mod sidecar;
pub mod state;
mod stream_chat;
mod tool_capability;
mod wayland_shortcut;
mod web_search;
mod commands;

use state::AppState;
use commands::*;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};

const STARTUP_WARMUP_ATTEMPTS: usize = 5;
const STARTUP_WARMUP_RETRY_DELAY_SECS: u64 = 2;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_state = AppState::new(app.handle().clone())?;
            let background_launch = std::env::args().any(|arg| arg == "--background");

            // Warm the llama-server in the background so the first chat doesn't
            // pay the cold-start cost of loading the model into memory. Clone
            // before the move into manage(). Best-effort — if it fails the first
            // chat will start the server normally.
            //
            // Spawn from a std thread with its own Tokio runtime because the
            // Tauri setup closure may not have a Tokio context active (depends
            // on the backend and how tauri initialises the main thread).
            let warmup_state = app_state.clone();
            std::thread::spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        log::warn!("startup warm-up: failed to build runtime: {e}");
                        return;
                    }
                };
                rt.block_on(async move {
                    for attempt in 1..=STARTUP_WARMUP_ATTEMPTS {
                        match warmup_state.warm_llama_server().await {
                            Ok(()) => {
                                log::info!("startup warm-up ready (attempt {attempt})");
                                return;
                            }
                            Err(error) if attempt < STARTUP_WARMUP_ATTEMPTS => {
                                log::warn!(
                                    "startup warm-up attempt {attempt}/{STARTUP_WARMUP_ATTEMPTS} failed: {error}; retrying"
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(
                                    STARTUP_WARMUP_RETRY_DELAY_SECS,
                                ))
                                .await;
                            }
                            Err(error) => {
                                log::warn!(
                                    "startup warm-up failed after {STARTUP_WARMUP_ATTEMPTS} attempts; first chat will retry: {error}"
                                );
                            }
                        }
                    }
                });
            });

            app.manage(app_state);
            app.handle().plugin(tauri_plugin_autostart::Builder::new().args(["--background"]).build())?;
            app.handle().plugin(tauri_plugin_dialog::init())?;

            // Set the generated app icon explicitly on the native window. Linux
            // window managers otherwise may fall back to Tauri's development icon.
            let app_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))?;
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/internal-tray.png"))?;
            if let Some(window) = app.get_webview_window("main") {
                window.set_icon(app_icon.clone())?;
            }

            let show = MenuItem::with_id(app, "show", "Show Myelin", true, None::<&str>)?;
            let quick = MenuItem::with_id(app, "quick", "Quick Capture", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quick, &quit])?;
            TrayIconBuilder::new().menu(&menu).tooltip("Myelin")
                .icon(tray_icon)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => { if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); } }
                    "quick" => toggle_quick_window(app),
                    "quit" => quit_app(app),
                    _ => {}
                }).build(app)?;
            if background_launch { if let Some(w) = app.get_webview_window("main") { let _ = w.hide(); } }

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
                        // zbus logs every D-Bus frame at INFO and floods the log.
                        .level_for("zbus", log::LevelFilter::Warn)
                        .build(),
                )?;
            }

            // Quick-capture global shortcut: register the plugin with a handler that
            // toggles the capture window, then register the user's configured combo.
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(|app, _shortcut, event| {
                        if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                            toggle_quick_window(app);
                        }
                    })
                    .build(),
            )?;
            {
                let sc = app.state::<AppState>().quick_shortcut();
                // Linux/Wayland blocks apps from grabbing global keys, so the native
                // plugin can't fire there — go through the xdg-desktop-portal
                // GlobalShortcuts interface instead. Everywhere else, the plugin works.
                #[cfg(target_os = "linux")]
                // The host portal resolves this value against the desktop entry. Tauri's
                // Linux bundle installs `myelin.desktop` (product name), while the Tauri
                // runtime identifier is `com.paper.myelin`; passing the latter makes the
                // portal reject the registration with "App info not found".
                wayland_shortcut::spawn(
                    app.handle().clone(),
                    sc,
                    app.config()
                        .product_name
                        .clone()
                        .unwrap_or_else(|| "myelin".to_string()),
                );
                #[cfg(not(target_os = "linux"))]
                {
                    use tauri_plugin_global_shortcut::GlobalShortcutExt;
                    if let Ok(parsed) = sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
                        let _ = app.global_shortcut().register(parsed);
                    }
                }
            }

            app.emit("app://ready", "ready")?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            get_appearance_settings,
            save_color_theme,
            delete_color_theme,
            set_active_color_theme,
            get_ai_config_status,
            validate_ai_config,
            apply_ai_config,
            open_ai_config_file,
            read_ai_config,
            save_ai_config,
            install_ai_runtime,
            set_workspace,
            set_llama_model_path,
            set_llama_executable_path,
            set_llama_advanced_config,
            list_llama_devices,
            downloadable_backends,
            downloadable_bee_backends,
            download_llama_backend,
            download_bee_backend,
            set_inference_engine,
            create_note,
            create_notebook,
            list_notebooks,
            load_note,
            save_note,
            delete_note,
            duplicate_note,
            move_note,
            reorder_note,
            search_notes,
            get_ocr_status,
            set_ocr_settings,
            ocr_pdf_page,
            get_provider_status,
            rebuild_index,
            get_snapshot,
            warm_llama_server,
            stop_llama_server,
            cache_note_sections,
            get_searxng_url,
            set_searxng_url,
            get_embed_model_path,
            set_embed_model_path,
            get_reranker_model_status,
            set_reranker_model_path,
            list_built_in_models,
            download_built_in_model,
            ingest_document,
            ensure_document_ingested,
            delete_document,
            list_model_profiles,
            get_all_note_documents,
            read_pdf_binary,
            save_source_annotations,
            ask_ai_stream,
            cancel_ai,
            save_chat_history,
            clear_ai_conversation,
            get_note_history,
            get_note_version,
            import_pdf_file,
            clone_pdf_for_attachment,
            save_pdf_annotations,
            compile_latex,
            import_latex_asset,
            tectonic_cache_status,
            prewarm_tectonic,
            get_quick_shortcut,
            set_quick_shortcut,
            list_tasks,
            save_task,
            delete_task,
            set_require_tool_approval,
            set_deterministic_tools,
            set_tool_gating,
            set_prompt_cache,
            llama_cache_status,
            clear_llama_cache,
            resolve_tool_approval,
            get_openharn_settings,
            set_openharn_settings,
            get_background_settings,
            set_start_with_system
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) && (window.label() == "main" || window.label() == "quick") {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); }
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
pub mod agent;
