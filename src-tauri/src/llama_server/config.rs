use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use sha2::{Digest, Sha256};
use super::*;
pub(super) fn tiering_roots(app_data_dir: &Path, workspace_config: &WorkspaceLlamaConfig) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(raw) = &workspace_config.executable_path {
        let exe = resolve_input_path(app_data_dir, raw);
        if let Some(parent) = exe.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    // User-managed binaries take priority over bundled ones, so a downloaded
    // CUDA build wins over the shipped Vulkan/CPU build.
    roots.push(app_data_dir.join("bin"));
    if let Some(bundled) = resource_bin_dir() {
        roots.push(bundled);
    }
    roots
}

/// Normalize a user backend preference to `auto`, `cuda`, `vulkan`, `metal`,
/// or `cpu`. The former generic `gpu` value is accepted only as a compatibility
/// alias and becomes `auto`; new configuration should name the concrete backend.
fn normalize_preference(raw: Option<&str>) -> String {
    match raw.map(|p| p.trim().to_lowercase()).as_deref() {
        Some("cpu") => "cpu".into(),
        Some("cuda") => "cuda".into(),
        Some("vulkan") => "vulkan".into(),
        Some("metal") => "metal".into(),
        _ => "auto".into(),
    }
}

pub fn normalize_engine(raw: Option<&str>) -> String {
    match raw.map(|value| value.trim().to_ascii_lowercase()).as_deref() {
        Some("beellama") => "beellama".into(),
        _ => "llama_cpp".into(),
    }
}

/// Backend order for the current OS, hardware, and preference. A specific
/// preference forces that backend first; CPU is appended as a safety net so the
/// app never fails to launch — except when the user explicitly forces CPU.
pub(super) fn desired_backends(preference: &str) -> Vec<GpuBackend> {
    match preference {
        "cpu" => vec![GpuBackend::Cpu],
        "cuda" => vec![GpuBackend::Cuda, GpuBackend::Cpu],
        "vulkan" => vec![GpuBackend::Vulkan, GpuBackend::Cpu],
        "metal" => vec![GpuBackend::Metal, GpuBackend::Cpu],
        // "auto": best for the detected hardware.
        _ => {
            if cfg!(target_os = "macos") {
                vec![GpuBackend::Metal, GpuBackend::Cpu]
            } else if detect_nvidia() {
                vec![GpuBackend::Cuda, GpuBackend::Vulkan, GpuBackend::Cpu]
            } else {
                // Vulkan covers AMD/Intel/NVIDIA and degrades to CPU on its own.
                vec![GpuBackend::Vulkan, GpuBackend::Cpu]
            }
        }
    }
}

pub fn inspect_provider(app_data_dir: &Path) -> Result<LlamaProviderInfo> {
    let app_config = load_config(app_data_dir).unwrap_or_default();
    let nvidia_detected = detect_nvidia();
    let gpu_available = gpu_available();
    let (gpus, _) = detect_gpus();
    let installed_backends = installed_backends(app_data_dir, &app_config);
    let installed_bee_backends = installed_bee_backends(app_data_dir);
    match resolve_config(app_data_dir) {
        Ok(config) => Ok(LlamaProviderInfo {
            detail: format!(
                "Ready to use {} ({} backend) with model {}.",
                config.executable_path.display(),
                config.backend.clone().unwrap_or_else(|| "cpu".into()),
                config.model_path.display()
            ),
            selected_backend: config.backend.clone(),
            resolved: Some(config),
            config: app_config,
            healthy: true,
            nvidia_detected,
            gpu_available,
            gpus,
            installed_backends,
            installed_bee_backends,
        }),
        Err(error) => Ok(LlamaProviderInfo {
            detail: error.to_string(),
            resolved: None,
            config: app_config,
            healthy: false,
            selected_backend: None,
            nvidia_detected,
            gpu_available,
            gpus,
            installed_backends,
            installed_bee_backends,
        }),
    }
}

pub fn resolve_config(app_data_dir: &Path) -> Result<ResolvedLlamaConfig> {
    let app_config = load_active_config(app_data_dir)?;
    let host = app_config
        .host
        .clone()
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    let port = app_config.port.unwrap_or(DEFAULT_PORT);
    let preference = normalize_preference(app_config.backend_preference.as_deref());
    let candidates = resolve_candidates(app_data_dir, &app_config, &preference)?;
    let primary = candidates
        .first()
        .ok_or_else(|| anyhow!("no llama-server binary could be resolved"))?;
    let model_path = resolve_model_path(app_data_dir, &app_config)?;

    // Resolve the model profile (GGUF auto ← bundled ← user) for chat-template
    // overrides, role, and recommended sampling. Cheap header read of the GGUF.
    let gguf = crate::gguf::read_gguf_info(&model_path).ok();
    let model_filename = model_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let profile = crate::model_profiles::resolve(app_data_dir, gguf.as_ref(), model_filename);

    let runtime_fingerprint = runtime_fingerprint(&primary.executable_path, &primary.engine)?;
    let model_fingerprint = model_fingerprint(&model_path);
    Ok(ResolvedLlamaConfig {
        inference_engine: primary.engine.clone(),
        executable_path: primary.executable_path.clone(),
        runtime_fingerprint: runtime_fingerprint.clone(),
        model_path,
        host,
        port,
        context_size: app_config.context_size.unwrap_or(4096),
        gpu_layers: app_config.gpu_layers,
        threads: app_config.threads,
        // User config wins; otherwise the profile's recommended value; else default.
        temperature: app_config
            .temperature
            .or(profile.temperature)
            .unwrap_or(0.2),
        top_p: app_config.top_p.or(profile.top_p).unwrap_or(0.95),
        chat_format: app_config.chat_format.clone(),
        extra_args: app_config.extra_args.clone(),
        backend: Some(primary.backend.label().to_string()),
        backend_preference: preference,
        gpu_device: app_config
            .gpu_device
            .clone()
            .filter(|d| !d.trim().is_empty()),
        thinking: app_config.thinking.unwrap_or(false),
        auto_offload: app_config.auto_offload.unwrap_or(true),
        max_turns: app_config.max_turns.filter(|&n| n > 0).unwrap_or(4),
        chat_template_override: profile.chat_template.clone(),
        model_role: match profile.role {
            crate::model_profiles::ModelRole::Embed => "embed".to_string(),
            crate::model_profiles::ModelRole::Chat => "chat".to_string(),
        },
        supports_tools: profile.supports_tools,
        prefers_prompt_tools: profile.prefers_prompt_tools,
        verbose_tool_schemas: profile.verbose_tool_schemas,
        tool_choice: profile.tool_choice.clone(),
        template_kwargs: profile.template_kwargs.clone(),
        deterministic_tools: app_config.deterministic_tools.unwrap_or(true),
        // Default OFF: the model gets the full toolset every turn (model-agnostic).
        // Gating is opt-in only for sub-2B models that misfire on tools.
        tool_gating: app_config.tool_gating.unwrap_or(false),
        prompt_cache: app_config.prompt_cache.unwrap_or(true),
        slot_save_path: app_data_dir
            .join("llama-cache")
            .join("slots")
            .join(&runtime_fingerprint)
            .join(model_fingerprint),
        candidates,
    })
}

/// Project the applied schema-backed profile into the legacy launcher shape.
/// Keeping this projection local lets the existing adaptive backend selection
/// remain stable while all new runtime identities come from the applied file.
fn load_active_config(app_data_dir: &Path) -> Result<WorkspaceLlamaConfig> {
    let ai_path = crate::ai_config::applied_path(app_data_dir);
    if ai_path.exists() {
        let config = crate::ai_config::load_applied(app_data_dir)?
            .ok_or_else(|| anyhow!("applied AI configuration disappeared while loading"))?;
        crate::ai_config::require_valid(&config)?;
        let profile = config.profiles.get(&config.active_profile).ok_or_else(|| anyhow!("active AI profile is missing"))?;
        let mut legacy = WorkspaceLlamaConfig::default();
        legacy.model_path = Some(profile.model_path.to_string_lossy().into_owned());
        legacy.host = Some(profile.server.host.clone());
        legacy.port = Some(profile.server.port);
        legacy.context_size = Some(profile.inference.context_size);
        legacy.gpu_layers = profile.inference.gpu_layers;
        legacy.threads = profile.inference.threads;
        legacy.temperature = Some(profile.inference.temperature);
        legacy.top_p = Some(profile.inference.top_p);
        legacy.backend_preference = Some(profile.inference.backend.to_string());
        legacy.gpu_device = profile.inference.gpu_device.clone();
        legacy.thinking = Some(profile.inference.thinking);
        legacy.auto_offload = Some(profile.inference.auto_offload);
        legacy.max_turns = Some(profile.inference.max_turns);
        legacy.chat_format = profile.inference.chat_format.clone();
        legacy.extra_args = profile.inference.extra_args.clone();
        match config.runtimes.get(&profile.runtime).map(|r| &r.source) {
            Some(crate::ai_config::RuntimeSource::Bundled { runtime: crate::ai_config::BuiltinRuntime::Bee }) => legacy.inference_engine = Some("beellama".into()),
            Some(crate::ai_config::RuntimeSource::Path { executable }) => legacy.executable_path = Some(executable.to_string_lossy().into_owned()),
            Some(crate::ai_config::RuntimeSource::Download { binary_path, .. }) => {
                let installed = app_data_dir.join("bin").join("runtimes").join(&profile.runtime);
                let binary = binary_path.as_ref().map(|p| installed.join(p)).unwrap_or(installed.join(executable_name()));
                legacy.executable_path = Some(binary.to_string_lossy().into_owned());
            }
            _ => {}
        }
        legacy.deterministic_tools = Some(config.tools.deterministic);
        legacy.tool_gating = Some(config.tools.gating);
        legacy.searxng_url = config.retrieval.searxng_url.clone();
        legacy.prompt_cache = Some(true);
        return Ok(legacy);
    }
    load_config(app_data_dir)
}

fn runtime_fingerprint(path: &Path, engine: &str) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| format!("failed to inspect runtime {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(engine.as_bytes());
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(metadata.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos().to_le_bytes()).unwrap_or_default());
    // Hash the executable when practical. This runs at configuration/startup
    // boundaries, never on every request, and gives custom forks isolated KV.
    if metadata.len() <= 512 * 1024 * 1024 {
        hasher.update(fs::read(path).with_context(|| format!("failed to fingerprint runtime {}", path.display()))?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn model_fingerprint(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    if let Ok(metadata) = fs::metadata(path) {
        hasher.update(metadata.len().to_le_bytes());
        hasher.update(metadata.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos().to_le_bytes()).unwrap_or_default());
    }
    format!("{:x}", hasher.finalize())
}

fn config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(CONFIG_FILE_NAME)
}

fn update_config(
    app_data_dir: &Path,
    update: impl FnOnce(&mut WorkspaceLlamaConfig) -> Result<()>,
) -> Result<()> {
    let mut config = load_config(app_data_dir)?;
    update(&mut config)?;
    if config.context_size == Some(0) || config.max_turns == Some(0) {
        bail!("llama configuration contains an invalid zero-sized runtime setting");
    }
    let path = config_path(app_data_dir);
    crate::persistence::atomic_write_json(&path, &config)
        .with_context(|| format!("failed to persist llama configuration at {}", path.display()))
}

pub(super) fn load_config(app_data_dir: &Path) -> Result<WorkspaceLlamaConfig> {
    let path = config_path(app_data_dir);
    if !path.exists() {
        return Ok(WorkspaceLlamaConfig::default());
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn set_model_path(app_data_dir: &Path, model_path: String) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.model_path = Some(model_path);
        Ok(())
    })
}

/// The configured SearXNG base URL for web search, if set and non-empty.
pub fn searxng_url(app_data_dir: &Path) -> Option<String> {
    load_config(app_data_dir)
        .ok()
        .and_then(|c| c.searxng_url)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Set (or clear, when empty) the SearXNG base URL for web search.
pub fn set_searxng_url(app_data_dir: &Path, url: Option<String>) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.searxng_url = url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Ok(())
    })
}

/// Path to the configured embedding model GGUF, if set and non-empty.
pub fn embed_model_path(app_data_dir: &Path) -> Option<String> {
    load_config(app_data_dir)
        .ok()
        .and_then(|c| c.embed_model_path)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Configured quick-capture global shortcut, defaulting to "Ctrl+Space".
pub fn quick_capture_shortcut(app_data_dir: &Path) -> String {
    load_config(app_data_dir)
        .ok()
        .and_then(|c| c.quick_capture_shortcut)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Ctrl+Space".to_string())
}

/// Set the quick-capture global shortcut string.
pub fn set_quick_capture_shortcut(app_data_dir: &Path, shortcut: String) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.quick_capture_shortcut = Some(shortcut.trim().to_string()).filter(|s| !s.is_empty());
        Ok(())
    })
}

/// Set (or clear, when empty) the embedding model GGUF path.
pub fn set_embed_model_path(app_data_dir: &Path, path: Option<String>) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.embed_model_path = path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Ok(())
    })
}

pub fn set_deterministic_tools(app_data_dir: &Path, enabled: bool) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.deterministic_tools = Some(enabled);
        Ok(())
    })
}

pub fn set_tool_gating(app_data_dir: &Path, enabled: bool) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.tool_gating = Some(enabled);
        Ok(())
    })
}

pub fn set_prompt_cache(app_data_dir: &Path, enabled: bool) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.prompt_cache = Some(enabled);
        Ok(())
    })
}

pub fn set_inference_engine(app_data_dir: &Path, engine: String) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.inference_engine = Some(normalize_engine(Some(&engine)));
        Ok(())
    })
}

pub fn set_executable_path(app_data_dir: &Path, executable_path: String) -> Result<()> {
    update_config(app_data_dir, |config| {
        config.executable_path = Some(executable_path);
        Ok(())
    })
}

pub fn set_advanced_config(
    app_data_dir: &Path,
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
) -> Result<()> {
    update_config(app_data_dir, |config| {
        if let Some(cs) = context_size { config.context_size = Some(cs); }
        if let Some(gl) = gpu_layers { config.gpu_layers = Some(gl); }
        if let Some(t) = threads { config.threads = Some(t); }
        if let Some(temp) = temperature { config.temperature = Some(temp); }
        if let Some(tp) = top_p { config.top_p = Some(tp); }
        if let Some(ea) = extra_args { config.extra_args = ea; }
        if let Some(bp) = backend_preference { config.backend_preference = Some(normalize_preference(Some(&bp))); }
        if let Some(dev) = gpu_device {
            config.gpu_device = if dev.trim().is_empty() { None } else { Some(dev) };
        }
        if let Some(t) = thinking { config.thinking = Some(t); }
        if let Some(ao) = auto_offload { config.auto_offload = Some(ao); }
        if let Some(mt) = max_turns { config.max_turns = Some(mt.clamp(1, 12)); }
        Ok(())
    })
}
