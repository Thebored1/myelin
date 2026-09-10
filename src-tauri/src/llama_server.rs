const CONFIG_FILE_NAME: &str = "llama-server.json";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 39281;
const STARTUP_TIMEOUT_SECS: u64 = 75;
const STARTUP_DELAY_MS: u64 = 500;
const STDERR_CAPTURE_LINES: usize = 200;
const GPU_MOE_TENSOR_OVERRIDE: &str = ".*ffn_.*_exps.*=CPU";

pub const BEELLAMA_RELEASE_TAG: &str = "v0.4.1";
pub const LLAMA_RELEASE_TAG: &str = "b9585";

mod assets;
mod config;
mod discovery;
mod embedding;
mod hardware;
mod hardware_budget;
mod launch_plan;
mod process;
mod reranker;
mod types;

#[cfg(test)]
mod tests;

pub use assets::{
    assets_for_backend, bee_asset_sha256, bee_assets_for_backend, bee_download_url, download_url,
    downloadable_backends, downloadable_bee_backends, extract_archive,
    install_backend_from_staging,
};
pub use config::{
    embed_model_path, inspect_provider, normalize_engine, quick_capture_shortcut,
    reranker_model_path, resolve_config, searxng_url, set_advanced_config, set_deterministic_tools,
    set_embed_model_path, set_executable_path, set_inference_engine, set_model_path,
    set_prompt_cache, set_quick_capture_shortcut, set_reranker_model_path, set_searxng_url,
    set_tool_gating,
};
pub use embedding::ManagedEmbedServer;
pub use embedding::{resolve_embedding_executable, start_embed_server, stop_embed_server};
pub use hardware::DeviceInfo;
pub use hardware::{
    detect_gpus, detect_nvidia, gpu_available, installed_backends, installed_bee_backends,
    list_devices, set_resource_bin_dir,
};
pub use hardware_budget::{free_device_local_vram, health_check, probe_gpu_budget, GpuBudget};
pub use process::{start_server, stop_server};
pub use reranker::{
    resolve_reranker_executable, start_reranker_server, stop_reranker_server, ManagedRerankerServer,
};
pub use types::{
    BackendCandidate, GpuBackend, LlamaProviderInfo, ManagedLlamaServer, ResolvedLlamaConfig,
    WorkspaceLlamaConfig,
};

use config::{desired_backends, load_config, tiering_roots};
use discovery::{
    apply_library_path, ensure_sonames, executable_name, resolve_candidates, resolve_input_path,
    resolve_model_path, validate_existing_file,
};
use hardware::{discrete_device_id, integrated_device_id, list_devices_on, resource_bin_dir};
pub(crate) use hardware_budget::default_inference_threads;
use hardware_budget::{
    available_ram_bytes, cache_reuse_tokens, fit_ngl, integrated_weight_budget, ram_safe_ctx,
    AUTO_CTX_TARGET, IGPU_RAM_RESERVE,
};
use launch_plan::{launch_plans, lfm25_template_path, lfm2_template_path, LaunchPlan};
use types::{gpu_moe_policy_active, has_tensor_override};
