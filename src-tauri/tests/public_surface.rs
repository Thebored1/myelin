#![allow(unused_imports)]

use myelin_lib::agent::{
    append_request_intent, build_myelin_agent, chat_tool_intent, compact_tool_specs_for_profile,
    detect_format_op, FetchWebPageArgs, FetchWebPageTool, FindInNoteArgs, FindInNoteTool,
    FormatNoteArgs, FormatNoteTool, InsertAfterLineArgs, InsertAfterLineTool,
    ReadNoteArgs, ReadNoteTool, SearchDocumentsArgs, SearchDocumentsTool, SearchNotesArgs,
    SearchNotesTool, SelectionArg, TARGETED_WRITE_PREAMBLE, DIRECT_CHAT_PREAMBLE,
    DeleteInNoteArgs, DeleteInNoteTool, EditNotebookArgs, EditNotebookTool, MYELIN_PREAMBLE,
    AppendNoteArgs, AppendNoteTool, PrependNoteArgs, PrependNoteTool, ReplaceInNoteArgs,
    ReplaceInNoteTool, ToolError, WebSearchArgs, WebSearchTool, WriteNoteArgs, WriteNoteTool,
    html_to_text, in_edit_thread, interaction_mode_tools, is_pure_note_write_request,
    is_small_talk, locate_selection, note_write_intent, normalize_web_url, placement_request_intent,
    select_chat_tools, select_tools, select_tools_cfg, selection_insert_after_plan,
    selection_scoped_plan, targeted_write_tools, tool_intent, tool_specs, wants_clear,
    wants_documents, wants_fetch, wants_find, wants_other_notes, wants_partial_removal,
    wants_search,
};
use myelin_lib::llama_server::{
    assets_for_backend, bee_asset_sha256, bee_assets_for_backend, bee_download_url,
    detect_gpus, detect_nvidia, downloadable_backends,
    downloadable_bee_backends, download_url, embed_model_path, extract_archive, free_device_local_vram,
    gpu_available, health_check, installed_backends, installed_bee_backends,
    inspect_provider, install_backend_from_staging, list_devices, normalize_engine,
    probe_gpu_budget, resolve_config, resolve_embedding_executable, set_advanced_config,
    set_deterministic_tools, set_embed_model_path, set_executable_path, set_inference_engine,
    set_model_path, set_prompt_cache, set_quick_capture_shortcut, set_resource_bin_dir,
    set_searxng_url, set_tool_gating, start_embed_server, start_server, stop_embed_server,
    stop_server, BackendCandidate, DeviceInfo, GpuBackend, GpuBudget, LlamaProviderInfo,
    ManagedEmbedServer, ManagedLlamaServer, ResolvedLlamaConfig, WorkspaceLlamaConfig,
    BEELLAMA_RELEASE_TAG, LLAMA_RELEASE_TAG,
};
use myelin_lib::state::{
    compile_tex_source, AppState, BackgroundSettings, DocumentIngestionResult, LlamaCacheStatus,
    OpenharnSettings, TectonicCacheStatus,
};
use serde_json::json;

#[test]
fn public_paths_remain_available() {
    let _ = (
        MYELIN_PREAMBLE,
        DIRECT_CHAT_PREAMBLE,
        TARGETED_WRITE_PREAMBLE,
        BEELLAMA_RELEASE_TAG,
        LLAMA_RELEASE_TAG,
    );
}

#[test]
fn persisted_defaults_keep_their_wire_shapes() {
    assert_eq!(
        serde_json::to_value(BackgroundSettings::default()).unwrap(),
        json!({"startWithSystem": false})
    );
    assert_eq!(
        serde_json::to_value(myelin_lib::llama_server::WorkspaceLlamaConfig::default()).unwrap(),
        json!({
            "inferenceEngine": null,
            "executablePath": null,
            "modelPath": null,
            "host": null,
            "port": null,
            "contextSize": null,
            "gpuLayers": null,
            "threads": null,
            "temperature": null,
            "topP": null,
            "chatFormat": null,
            "extraArgs": [],
            "backendPreference": null,
            "gpuDevice": null,
            "thinking": null,
            "autoOffload": null,
            "maxTurns": null,
            "searxngUrl": null,
            "embedModelPath": null,
            "deterministicTools": null,
            "toolGating": null,
            "promptCache": null,
            "quickCaptureShortcut": null
        })
    );
    let openharn = serde_json::to_value(OpenharnSettings::default()).unwrap();
    assert_eq!(openharn["tool_mode"], "");
    assert_eq!(openharn["strict"], false);
    assert_eq!(openharn["external_enabled"], false);
    assert_eq!(openharn["external_api_key"], json!(null));
}

#[test]
fn tool_schema_order_is_stable() {
    let names = tool_specs()
        .into_iter()
        .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "read_note", "write_note", "append_note", "prepend_note",
            "replace_in_note", "insert_after_line", "delete_in_note", "format_note",
            "fetch_web_page", "web_search", "search_documents", "find_in_note",
            "search_notes", "edit_notebook"
        ]
    );
}
