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

const CONFIG_FILE_NAME: &str = "llama-server.json";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 39281;
const STARTUP_ATTEMPTS: usize = 60;
const STARTUP_DELAY_MS: u64 = 500;
/// How many lines of llama-server stderr we retain to detect the active backend.
const STDERR_CAPTURE_LINES: usize = 200;

/// A compute backend for llama.cpp. The desktop strategy is CUDA-tiered:
/// detect an NVIDIA GPU and prefer CUDA, otherwise fall back to a portable
/// GPU backend (Vulkan on Windows/Linux, Metal on macOS), and finally CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuBackend {
    Cuda,
    Vulkan,
    Metal,
    Cpu,
    /// A user-supplied executable whose backend we don't manage.
    Custom,
}

impl GpuBackend {
    /// Subdirectory under `<app_data>/bin` where this backend's binaries live.
    fn dir_name(self) -> Option<&'static str> {
        match self {
            GpuBackend::Cuda => Some("cuda"),
            GpuBackend::Vulkan => Some("vulkan"),
            GpuBackend::Metal => Some("metal"),
            GpuBackend::Cpu => Some("cpu"),
            GpuBackend::Custom => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GpuBackend::Cuda => "cuda",
            GpuBackend::Vulkan => "vulkan",
            GpuBackend::Metal => "metal",
            GpuBackend::Cpu => "cpu",
            GpuBackend::Custom => "custom",
        }
    }

    /// Whether this backend offloads work to a GPU.
    pub fn is_gpu(self) -> bool {
        matches!(self, GpuBackend::Cuda | GpuBackend::Vulkan | GpuBackend::Metal)
    }
}

/// One launchable llama-server binary plus the backend it provides.
#[derive(Debug, Clone)]
pub struct BackendCandidate {
    pub backend: GpuBackend,
    pub executable_path: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceLlamaConfig {
    pub executable_path: Option<String>,
    pub model_path: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub context_size: Option<u32>,
    pub gpu_layers: Option<i32>,
    pub threads: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub chat_format: Option<String>,
    pub extra_args: Vec<String>,
    /// Compute backend preference: "auto", "cuda", "vulkan", "metal", or "cpu".
    pub backend_preference: Option<String>,
    /// Optional specific GPU device id (e.g. "Vulkan0", "CUDA0") to pin to.
    /// Empty/None means let the backend choose. Lets users pick the iGPU.
    pub gpu_device: Option<String>,
    /// Whether the model is allowed to "think"/reason. None defaults to off
    /// (faster, no hidden reasoning tokens). Universal across models via the
    /// llama-server `--reasoning on|off` flag.
    pub thinking: Option<bool>,
    /// Adaptive offload: when true (default), the launcher manages
    /// --n-gpu-layers / --ctx-size / --no-kv-offload / --flash-attn so the model
    /// uses available VRAM, keeps KV in RAM, holds a large context, and retries
    /// with reduced settings on failure. When false, the manual gpu_layers /
    /// context_size are used verbatim.
    pub auto_offload: Option<bool>,
    /// Max agent tool-calling turns before forcing a final answer.
    pub max_turns: Option<u32>,
    /// Optional SearXNG instance base URL for web search (privacy-first). When
    /// empty/None the agent falls back to the no-key DuckDuckGo endpoint.
    pub searxng_url: Option<String>,
    /// Optional path to the embedding model GGUF (e.g. nomic-embed-text). When
    /// set, the app runs a second llama-server in embedding mode for RAG.
    pub embed_model_path: Option<String>,
    /// Deterministic correctness tools: regex format_note, find_in_note word
    /// search, and the destructive-write guard. Default ON — they make tool use
    /// more reliable. None → on.
    pub deterministic_tools: Option<bool>,
    /// Whether the model supports tool calling. None → probe at first use.
    #[serde(default)]
    pub supports_tools: Option<bool>,
    /// Per-message tool gating: offer the model only the tools its message
    /// warrants, via keyword intent heuristics. **Default OFF** — the model gets
    /// the full toolset every turn and decides for itself (model-agnostic, the
    /// standard agent approach). Opt-in only for sub-2B models that misfire on
    /// tools they shouldn't touch; the heuristics are brittle and can withhold a
    /// valid tool (e.g. block a web search the model would have run). None → off.
    pub tool_gating: Option<bool>,
    /// Global hotkey that opens the quick-capture window (e.g. "Ctrl+Space").
    /// None → the default ("Ctrl+Space").
    pub quick_capture_shortcut: Option<String>,
    /// Provider kind: "local" (default, runs llama-server) or "openai"
    /// (bring-your-own-key, uses a remote OpenAI-compatible API).
    #[serde(default)]
    pub provider_kind: Option<String>,
    /// Base URL for the OpenAI-compatible API (e.g. "https://api.openai.com/v1").
    /// Only used when provider_kind is "openai".
    #[serde(default)]
    pub openai_base_url: Option<String>,
    /// API key for the OpenAI-compatible API.
    /// Only used when provider_kind is "openai".
    #[serde(default)]
    pub openai_key: Option<String>,
    /// Model name for the OpenAI-compatible API (e.g. "gpt-4o-mini").
    /// Only used when provider_kind is "openai".
    #[serde(default)]
    pub openai_model: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLlamaConfig {
    pub executable_path: PathBuf,
    pub model_path: PathBuf,
    pub host: String,
    pub port: u16,
    pub context_size: u32,
    pub gpu_layers: Option<i32>,
    pub threads: Option<u32>,
    pub temperature: f32,
    pub top_p: f32,
    pub chat_format: Option<String>,
    pub extra_args: Vec<String>,
    /// The backend of the primary (preferred) candidate, for display before launch.
    #[serde(default)]
    pub backend: Option<String>,
    /// Compute backend preference: "auto", "cuda", "vulkan", "metal", or "cpu".
    #[serde(default)]
    pub backend_preference: String,
    /// Specific GPU device id to pin to ("Vulkan0", "CUDA0", …), if any.
    #[serde(default)]
    pub gpu_device: Option<String>,
    /// Whether the model may think/reason (false = faster, no hidden tokens).
    #[serde(default)]
    pub thinking: bool,
    /// Adaptive offload management (default true).
    #[serde(default)]
    pub auto_offload: bool,
    /// Max agent tool-calling turns before forcing a final answer.
    #[serde(default)]
    pub max_turns: u32,
    /// Chat-template override from the model profile: builtin id "lfm2" or a
    /// file path. None → use the model's embedded template via --jinja.
    #[serde(default)]
    pub chat_template_override: Option<String>,
    /// Model role from the profile registry: "chat" (default) or "embed".
    #[serde(default)]
    pub model_role: String,
    /// Whether the model reliably does tool-calling, from the profile. None =
    /// unknown → the capability probe decides (and caches) at first use.
    #[serde(default)]
    pub supports_tools: Option<bool>,
    /// Deterministic correctness tools (format_note / find_in_note / write
    /// guard). Default true; on a more capable model they can be disabled.
    #[serde(default = "default_true")]
    pub deterministic_tools: bool,
    /// Per-message tool gating (offer only the tools a message warrants).
    /// Default OFF — the model gets the full toolset every turn (model-agnostic).
    #[serde(default = "default_false")]
    pub tool_gating: bool,
    /// Ordered list of binaries to try (best first). Not serialized to the UI.
    #[serde(skip)]
    pub candidates: Vec<BackendCandidate>,
}

/// Build a synthetic `ResolvedLlamaConfig` for the OpenAI-compatible BYOK provider.
/// Used instead of `resolve_config` when `provider_kind` is "openai".
pub fn build_openai_resolved(app_data_dir: &Path) -> Result<ResolvedLlamaConfig> {
    let (base, _key, model) = openai_config(app_data_dir)
        .ok_or_else(|| anyhow!("OpenAI provider not fully configured"))?;
    let app_config = load_config(app_data_dir).unwrap_or_default();
    // Strip trailing /v1 so base_url() → /v1/chat/completions works correctly.
    let base = base.trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base).to_string();
    // The base URL goes in `host` and the model name in `model_path` as a
    // workaround so base_url() and model_name() return the right values.
    Ok(ResolvedLlamaConfig {
        executable_path: PathBuf::new(),
        model_path: PathBuf::from(&model),
        host: base,
        port: 0,
        context_size: app_config.context_size.unwrap_or(4096),
        gpu_layers: None,
        threads: None,
        temperature: app_config.temperature.unwrap_or(0.7),
        top_p: app_config.top_p.unwrap_or(0.95),
        chat_format: None,
        extra_args: Vec::new(),
        backend: None,
        backend_preference: "auto".into(),
        gpu_device: None,
        thinking: false,
        auto_offload: false,
        max_turns: app_config.max_turns.filter(|&n| n > 0).unwrap_or(4),
        chat_template_override: None,
        model_role: "chat".into(),
        supports_tools: Some(true),
        deterministic_tools: app_config.deterministic_tools.unwrap_or(true),
        tool_gating: app_config.tool_gating.unwrap_or(false),
        candidates: Vec::new(),
    })
}

impl ResolvedLlamaConfig {
    pub fn base_url(&self) -> String {
        if self.port == 0 && !self.host.is_empty() {
            // Synthetic BYOK config: return the stored base URL directly.
            // For openai configs, `model_path` holds the model name as a workaround.
            return self.host.clone();
        }
        format!("http://{}:{}", self.host, self.port)
    }

    pub fn model_name(&self) -> String {
        if self.port == 0 && !self.host.is_empty() {
            // Synthetic BYOK config: model name is stored as filename in model_path.
            return self.model_path.to_string_lossy().to_string();
        }
        self.model_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("local-model")
            .to_string()
    }

    pub fn matches_runtime(&self, other: &Self) -> bool {
        self.executable_path == other.executable_path
            && self.model_path == other.model_path
            && self.host == other.host
            && self.port == other.port
            && self.context_size == other.context_size
            && self.gpu_layers == other.gpu_layers
            && self.threads == other.threads
            && self.chat_format == other.chat_format
            && self.extra_args == other.extra_args
            // Changing the device preference (e.g. GPU → CPU) must relaunch the
            // server even when the same binary is selected, since it changes the
            // effective --n-gpu-layers; likewise for pinning a specific GPU.
            && self.backend_preference == other.backend_preference
            && self.gpu_device == other.gpu_device
            && self.thinking == other.thinking
            && self.auto_offload == other.auto_offload
    }
}

pub struct ManagedLlamaServer {
    pub config: ResolvedLlamaConfig,
    pub child: Child,
    /// The backend that actually loaded, detected from the server's startup log.
    pub active_backend: GpuBackend,
    /// True when a GPU backend was requested for this launch.
    pub requested_gpu: bool,
    /// True when the model is actually running on a GPU.
    pub gpu_offloaded: bool,
    /// The context window (tokens) the server actually launched with — the
    /// adaptive offloader may have set it well above the configured value, or
    /// degraded it. Used to budget how much of a note fits in the prompt.
    pub ctx_size: u32,
    /// Drains the server's stderr for the process lifetime so its pipe never
    /// fills and stalls generation. Detaches; exits on child EOF.
    _stderr_reader: Option<thread::JoinHandle<()>>,
}

impl Drop for ManagedLlamaServer {
    fn drop(&mut self) {
        // Never leave the spawned llama-server running when this handle goes away
        // (restart, or app teardown). Best-effort; the app-exit path kills it too.
        let _ = self.child.kill();
    }
}

#[derive(Debug, Clone)]
pub struct LlamaProviderInfo {
    pub resolved: Option<ResolvedLlamaConfig>,
    pub config: WorkspaceLlamaConfig,
    pub healthy: bool,
    pub detail: String,
    /// The backend we would prefer for this machine (before launch).
    pub selected_backend: Option<String>,
    /// Whether an NVIDIA GPU was detected on this machine.
    pub nvidia_detected: bool,
    /// Whether GPU acceleration is usable on this machine at all.
    pub gpu_available: bool,
    /// GPU adapter names detected on this machine (for display).
    pub gpus: Vec<String>,
    /// Backend builds actually installed ("cuda"/"vulkan"/"metal"/"cpu").
    pub installed_backends: Vec<String>,
}

/// The release of llama.cpp the app targets — must match the bundled builds so
/// downloaded backends are ABI-compatible with each other.
pub const LLAMA_RELEASE_TAG: &str = "b9585";

/// Directory holding binaries bundled with the installer (resource dir / bin),
/// set once at startup. Used as a low-priority tiering root so a shipped app
/// finds its CPU/Vulkan builds with zero setup.
static RESOURCE_BIN_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn set_resource_bin_dir(dir: Option<PathBuf>) {
    let _ = RESOURCE_BIN_DIR.set(dir);
}

fn resource_bin_dir() -> Option<PathBuf> {
    RESOURCE_BIN_DIR.get().cloned().flatten()
}

/// Detect a usable NVIDIA GPU by probing `nvidia-smi`. Cached for the process
/// lifetime — driver availability does not change while the app runs.
pub fn detect_nvidia() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        Command::new("nvidia-smi")
            .arg("-L")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).contains("GPU"))
            .unwrap_or(false)
    })
}

/// Enumerate GPU adapter names on this machine. Returns `(names, probed)` where
/// `probed` is true when the OS query actually ran (so an empty list means
/// "definitely no GPU" rather than "couldn't tell"). Cached for the process.
pub fn detect_gpus() -> (Vec<String>, bool) {
    static CACHE: OnceLock<(Vec<String>, bool)> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if cfg!(target_os = "macos") {
                // Every supported Mac has a Metal-capable GPU.
                return (vec!["Apple GPU".to_string()], true);
            }
            if cfg!(target_os = "windows") {
                let out = Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        "(Get-CimInstance Win32_VideoController).Name -join \"`n\"",
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output();
                if let Ok(out) = out {
                    if out.status.success() {
                        let names: Vec<String> = String::from_utf8_lossy(&out.stdout)
                            .lines()
                            .map(|l| l.trim().to_string())
                            .filter(|l| !l.is_empty())
                            .collect();
                        return (names, true);
                    }
                }
                return (Vec::new(), false);
            }
            // Linux/other: best-effort via lspci.
            let out = Command::new("lspci")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output();
            if let Ok(out) = out {
                if out.status.success() {
                    let names: Vec<String> = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .filter(|l| {
                            let l = l.to_lowercase();
                            l.contains("vga") || l.contains("3d controller") || l.contains("display")
                        })
                        .map(|l| l.trim().to_string())
                        .collect();
                    return (names, true);
                }
            }
            (Vec::new(), false)
        })
        .clone()
}

/// Whether GPU acceleration is usable on this machine. Lenient on probe
/// failure (assume available) so we never wrongly block a real GPU.
pub fn gpu_available() -> bool {
    if cfg!(target_os = "macos") || detect_nvidia() {
        return true;
    }
    let (gpus, probed) = detect_gpus();
    !probed || !gpus.is_empty()
}

/// Backend subfolders that actually contain a binary, across the tiering roots.
pub fn installed_backends(app_data_dir: &Path, workspace_config: &WorkspaceLlamaConfig) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for root in tiering_roots(app_data_dir, workspace_config) {
        for backend in [GpuBackend::Cuda, GpuBackend::Vulkan, GpuBackend::Metal, GpuBackend::Cpu] {
            if let Some(dir) = backend.dir_name() {
                let label = backend.label().to_string();
                if root.join(dir).join(executable_name()).is_file() && !found.contains(&label) {
                    found.push(label);
                }
            }
        }
    }
    found
}

/// A compute device exposed by a backend, e.g. id "Vulkan0", name
/// "Intel(R) UHD Graphics".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub backend: String,
}

/// Run `llama-server --list-devices` for a given backend's binary and parse the
/// devices it exposes. Lets the UI offer a "use the iGPU" choice. Returns an
/// empty list if the backend isn't installed or the probe fails.
pub fn list_devices(app_data_dir: &Path, backend_label: &str) -> Vec<DeviceInfo> {
    let workspace_config = load_config(app_data_dir).unwrap_or_default();
    match backend_binary(app_data_dir, &workspace_config, backend_label) {
        Some(exe) => list_devices_on(&exe, backend_label),
        None => Vec::new(),
    }
}

/// Run `<exe> --list-devices` and parse the devices it exposes.
fn list_devices_on(exe: &Path, backend_label: &str) -> Vec<DeviceInfo> {
    let mut cmd = Command::new(exe);
    cmd.arg("--list-devices")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_library_path(&mut cmd, exe);

    let Ok(output) = cmd.output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();
    for line in text.lines() {
        // Lines look like: "  Vulkan0: Intel(R) UHD Graphics (8025 MiB, ...)"
        let trimmed = line.trim();
        let Some((id, rest)) = trimmed.split_once(':') else {
            continue;
        };
        // Device ids are a backend prefix + index, no spaces.
        if id.is_empty() || id.contains(char::is_whitespace) {
            continue;
        }
        let name = rest
            .trim()
            .rsplit_once(" (")
            .map(|(n, _)| n)
            .unwrap_or_else(|| rest.trim())
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        devices.push(DeviceInfo {
            id: id.to_string(),
            name,
            backend: backend_label.to_string(),
        });
    }
    devices
}

/// Pick the integrated GPU's device id from a device list (for the power-saving
/// Vulkan path on machines that also have a discrete GPU). Matches common iGPU
/// names; returns None if none look integrated.
/// Heuristic: does this GPU name look like an integrated GPU (shares system RAM)?
fn is_integrated_gpu_name(name: &str) -> bool {
    const HINTS: [&str; 9] = [
        "uhd", "iris", "integrated", "radeon graphics", "hd graphics", "renoir",
        "cezanne", "rembrandt", "phoenix",
    ];
    let n = name.to_lowercase();
    HINTS.iter().any(|h| n.contains(h))
}

/// The integrated GPU's device id (for power-saving), if any.
fn integrated_device_id(devices: &[DeviceInfo]) -> Option<String> {
    devices
        .iter()
        .find(|d| is_integrated_gpu_name(&d.name))
        .map(|d| d.id.clone())
}

/// The first DISCRETE GPU's device id, when more than one GPU is present — so a
/// hybrid laptop uses the fast dGPU instead of Vulkan's default device 0 (often
/// the iGPU). `None` on a single-GPU machine: there's no choice to make.
fn discrete_device_id(devices: &[DeviceInfo]) -> Option<String> {
    if devices.len() < 2 {
        return None;
    }
    devices
        .iter()
        .find(|d| !is_integrated_gpu_name(&d.name))
        .map(|d| d.id.clone())
}

/// Locate the llama-server binary for a specific backend ("cuda"/"vulkan"/…),
/// searching the tiering roots' `<backend>/` subfolders.
fn backend_binary(
    app_data_dir: &Path,
    workspace_config: &WorkspaceLlamaConfig,
    backend_label: &str,
) -> Option<PathBuf> {
    for root in tiering_roots(app_data_dir, workspace_config) {
        let exe = root.join(backend_label).join(executable_name());
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

/// Release asset file names to download for a backend on the current OS.
/// Empty when the backend isn't downloadable here (e.g. CUDA on Linux, or any
/// GPU backend on macOS where Metal ships in the default build).
pub fn assets_for_backend(backend: &str) -> Vec<String> {
    let tag = LLAMA_RELEASE_TAG;
    if cfg!(target_os = "windows") {
        match backend {
            // CUDA needs the runtime DLLs (cudart) alongside the binaries.
            "cuda" => vec![
                "cudart-llama-bin-win-cuda-12.4-x64.zip".to_string(),
                format!("llama-{tag}-bin-win-cuda-12.4-x64.zip"),
            ],
            "vulkan" => vec![format!("llama-{tag}-bin-win-vulkan-x64.zip")],
            "cpu" => vec![format!("llama-{tag}-bin-win-cpu-x64.zip")],
            _ => vec![],
        }
    } else if cfg!(target_os = "linux") {
        match backend {
            "vulkan" => vec![format!("llama-{tag}-bin-ubuntu-vulkan-x64.tar.gz")],
            "cpu" => vec![format!("llama-{tag}-bin-ubuntu-x64.tar.gz")],
            // No prebuilt CUDA tarball for Linux in this release; use Vulkan.
            _ => vec![],
        }
    } else {
        vec![]
    }
}

/// Backends that can be downloaded on demand for this OS.
pub fn downloadable_backends() -> Vec<String> {
    ["cuda", "vulkan", "cpu"]
        .iter()
        .filter(|b| !assets_for_backend(b).is_empty())
        .map(|b| b.to_string())
        .collect()
}

pub fn download_url(asset: &str) -> String {
    format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{}/{}",
        LLAMA_RELEASE_TAG, asset
    )
}

/// Extract a downloaded archive using the platform's standard tool.
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_lowercase();

    let status = if name.ends_with(".zip") {
        if cfg!(target_os = "windows") {
            Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command"])
                .arg(format!(
                    "Expand-Archive -LiteralPath \"{}\" -DestinationPath \"{}\" -Force",
                    archive.display(),
                    dest.display()
                ))
                .status()
        } else {
            Command::new("unzip")
                .arg("-o")
                .arg(archive)
                .arg("-d")
                .arg(dest)
                .status()
        }
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
    } else {
        bail!("unsupported archive format: {name}");
    };

    let status =
        status.with_context(|| format!("failed to run extractor for {}", archive.display()))?;
    if !status.success() {
        bail!("extraction failed for {}", archive.display());
    }
    Ok(())
}

/// Find the llama-server binary inside an extracted archive and copy its whole
/// directory into `backend_dir` (the .dll/.so siblings must come along).
pub fn install_backend_from_staging(staging: &Path, backend_dir: &Path) -> Result<()> {
    let exe_name = executable_name();
    let server = walkdir::WalkDir::new(staging)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().is_file() && e.file_name().to_string_lossy() == exe_name)
        .map(|e| e.path().to_path_buf())
        .ok_or_else(|| anyhow!("no {} found in the downloaded archive", exe_name))?;
    let src_dir = server
        .parent()
        .ok_or_else(|| anyhow!("invalid archive layout"))?;

    fs::create_dir_all(backend_dir)?;
    for entry in fs::read_dir(src_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let dest = backend_dir.join(entry.file_name());
            fs::copy(entry.path(), &dest)?;
            #[cfg(unix)]
            if entry.file_name().to_string_lossy() == exe_name {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&dest)?.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&dest, perms);
            }
        }
    }
    // Recreate the .so soname symlinks the file-only copy above dropped.
    ensure_sonames(backend_dir);
    Ok(())
}

/// Directories that may hold per-backend subfolders, highest priority first.
fn tiering_roots(app_data_dir: &Path, workspace_config: &WorkspaceLlamaConfig) -> Vec<PathBuf> {
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

/// Normalize a user backend preference to "auto" | "gpu" | "cuda" | "vulkan" |
/// "metal" | "cpu". "gpu" means "prefer any GPU backend" (see `desired_backends`).
fn normalize_preference(raw: Option<&str>) -> String {
    match raw.map(|p| p.trim().to_lowercase()).as_deref() {
        Some("cpu") => "cpu".into(),
        Some("gpu") => "gpu".into(),
        Some("cuda") => "cuda".into(),
        Some("vulkan") => "vulkan".into(),
        Some("metal") => "metal".into(),
        _ => "auto".into(),
    }
}

/// Backend order for the current OS, hardware, and preference. A specific
/// preference forces that backend first; CPU is appended as a safety net so the
/// app never fails to launch — except when the user explicitly forces CPU.
fn desired_backends(preference: &str) -> Vec<GpuBackend> {
    match preference {
        "cpu" => vec![GpuBackend::Cpu],
        "cuda" => vec![GpuBackend::Cuda, GpuBackend::Cpu],
        "vulkan" => vec![GpuBackend::Vulkan, GpuBackend::Cpu],
        "metal" => vec![GpuBackend::Metal, GpuBackend::Cpu],
        // Generic "GPU": try every GPU backend in order (the matching subfolder
        // only exists for installed backends, so absent ones are skipped) before
        // CPU. Does NOT depend on detect_nvidia(), which can fail inside the GUI
        // process and silently strand the model on CPU.
        "gpu" => vec![
            GpuBackend::Cuda,
            GpuBackend::Vulkan,
            GpuBackend::Metal,
            GpuBackend::Cpu,
        ],
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
        }),
    }
}

pub fn resolve_config(app_data_dir: &Path) -> Result<ResolvedLlamaConfig> {
    let app_config = load_config(app_data_dir)?;
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
    let model_filename = model_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let profile = crate::model_profiles::resolve(app_data_dir, gguf.as_ref(), model_filename);

    Ok(ResolvedLlamaConfig {
        executable_path: primary.executable_path.clone(),
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
        deterministic_tools: app_config.deterministic_tools.unwrap_or(true),
        // Default OFF: the model gets the full toolset every turn (model-agnostic).
        // Gating is opt-in only for sub-2B models that misfire on tools.
        tool_gating: app_config.tool_gating.unwrap_or(false),
        candidates,
    })
}

pub async fn health_check(client: &Client, config: &ResolvedLlamaConfig) -> bool {
    client
        .get(format!("{}/health", config.base_url()))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

/// Default context the adaptive offloader aims to hold on every machine,
/// clamped down only if RAM can't fit the KV cache for it.
const AUTO_CTX_TARGET: u32 = 32768;

/// Available system RAM in bytes (cross-platform, via sysinfo).
fn available_ram_bytes() -> u64 {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.available_memory()
}

/// What a GPU can hold for model weights, and whether that memory is shared RAM.
pub struct GpuBudget {
    /// Bytes available for offloaded weight layers — VRAM for a discrete card,
    /// GTT / shared system memory for an integrated GPU.
    pub bytes: u64,
    /// True for an integrated GPU: its weight memory IS system RAM, so it
    /// competes with the KV cache (kept in RAM via --no-kv-offload).
    pub integrated: bool,
}

/// Probe the GPU's weight-offload budget, best-effort and cross-platform.
/// `None` when undeterminable → the planner requests full offload and the launch
/// ladder backs off. Generic across vendors; no per-machine assumptions.
pub fn probe_gpu_budget() -> Option<GpuBudget> {
    // AMD on Linux exposes VRAM (and GTT) via sysfs.
    #[cfg(target_os = "linux")]
    {
        let read_u64 = |path: String| -> Option<u64> {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
        };
        for n in 0..4u8 {
            let base = format!("/sys/class/drm/card{n}/device");
            if let Some(vram_total) = read_u64(format!("{base}/mem_info_vram_total")) {
                let vram_free = vram_total
                    .saturating_sub(read_u64(format!("{base}/mem_info_vram_used")).unwrap_or(0));
                // A small dedicated-VRAM carveout means an integrated GPU (APU):
                // the model lives in GTT/shared RAM. A discrete card (large VRAM)
                // keeps weights in real VRAM, separate from the KV cache.
                const INTEGRATED_VRAM_MAX: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB
                if vram_total <= INTEGRATED_VRAM_MAX {
                    let gtt_free = read_u64(format!("{base}/mem_info_gtt_total"))
                        .unwrap_or(0)
                        .saturating_sub(read_u64(format!("{base}/mem_info_gtt_used")).unwrap_or(0));
                    return Some(GpuBudget { bytes: vram_free + gtt_free, integrated: true });
                }
                return Some(GpuBudget { bytes: vram_free, integrated: false });
            }
        }
    }
    // NVIDIA via nvidia-smi — discrete, separate VRAM (Linux + Windows).
    if let Some(v) = nvidia_smi_free_vram() {
        return Some(GpuBudget { bytes: v, integrated: false });
    }
    // Windows AMD/Intel/iGPU via DXGI.
    #[cfg(target_os = "windows")]
    {
        return dxgi_gpu_budget();
    }
    #[allow(unreachable_code)]
    None
}

/// Bytes the GPU can hold for weights (size only — see [`probe_gpu_budget`] for
/// the integrated flag). Kept for callers that just need the number (logging).
pub fn free_device_local_vram() -> Option<u64> {
    probe_gpu_budget().map(|b| b.bytes)
}

/// DXGI adapter memory for the best GPU (Windows AMD/Intel, including iGPUs). A
/// discrete GPU reports `DedicatedVideoMemory`; an integrated GPU runs from
/// shared system memory, so its budget is dedicated + shared. Best-effort.
#[cfg(target_os = "windows")]
fn dxgi_gpu_budget() -> Option<GpuBudget> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
    };
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut discrete: u64 = 0;
        let mut integrated: u64 = 0;
        let mut i = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let desc = match adapter.GetDesc1() {
                Ok(d) => d,
                Err(_) => continue,
            };
            // Skip the software/WARP adapter.
            if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let end = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end]);
            let dedicated = desc.DedicatedVideoMemory as u64;
            let shared = desc.SharedSystemMemory as u64;
            if is_integrated_gpu_name(&name) {
                integrated = integrated.max(dedicated.saturating_add(shared));
            } else if dedicated > 0 {
                discrete = discrete.max(dedicated);
            }
        }
        // Prefer the discrete GPU (real VRAM); else the integrated budget.
        if discrete > 0 {
            Some(GpuBudget { bytes: discrete, integrated: false })
        } else if integrated > 0 {
            Some(GpuBudget { bytes: integrated, integrated: true })
        } else {
            None
        }
    }
}

/// Free VRAM (bytes) of the first NVIDIA GPU via nvidia-smi, cross-platform.
fn nvidia_smi_free_vram() -> Option<u64> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    first
        .parse::<u64>()
        .ok()
        .map(|mib| mib.saturating_mul(1024 * 1024))
}

/// How many of `n_layers` layers fit in `free_vram` given the model's on-disk
/// weight size. KV stays in system RAM (--no-kv-offload), so only the weights
/// need VRAM; 0.85 leaves headroom for compute/activation buffers. Returns the
/// first-launch n-gpu-layers — full offload when it all fits, a partial GPU/CPU
/// split when it doesn't, 0 when essentially nothing fits. The degrade ladder
/// still covers a too-optimistic estimate. 999 ("all") when geometry is unknown.
fn fit_ngl(free_vram: u64, model_weight_bytes: u64, n_layers: u64) -> i32 {
    if n_layers == 0 || model_weight_bytes == 0 {
        return 999;
    }
    let per_layer = (model_weight_bytes / n_layers).max(1);
    let budget = (free_vram as f64 * 0.85) as u64;
    (budget / per_layer).min(n_layers) as i32
}

/// OS + app headroom kept free of model/KV allocations on an integrated GPU.
const IGPU_RAM_RESERVE: u64 = 1536 * 1024 * 1024; // 1.5 GiB

/// Weight-offload budget for an integrated GPU. Its offloaded weights (GTT) and
/// the KV cache (--no-kv-offload) both live in system RAM, so **full context
/// wins first**: reserve the KV cache (sized for the chosen context) plus OS
/// headroom, then offer the remaining RAM for weights — capped by what the iGPU
/// can actually address (its GTT budget). This maximises offload without
/// over-committing RAM and OOM-ing, while keeping the full context window.
fn integrated_weight_budget(available_ram: u64, kv_bytes: u64, gtt_budget: u64, reserve: u64) -> u64 {
    available_ram
        .saturating_sub(kv_bytes)
        .saturating_sub(reserve)
        .min(gtt_budget)
}

/// Largest context whose f16 KV cache fits in ~75% of available RAM (KV lives in
/// system RAM via --no-kv-offload), bounded by the model's trained context.
/// The launch ladder degrades ctx on a failed start, so this can be optimistic;
/// a too-conservative value here needlessly starves context (a 1B model on a
/// machine momentarily low on free RAM was collapsing to ~5K tokens, which the
/// note + tool schemas + any fetched page would then overflow). A 4096 floor
/// keeps a usable window; the ladder still backs off if the allocation fails.
fn ram_safe_ctx(requested: u32, gguf: Option<&crate::gguf::GgufInfo>) -> u32 {
    let model_max = gguf
        .and_then(|g| g.context_length)
        .map(|c| c.min(u32::MAX as u64) as u32)
        .unwrap_or(u32::MAX);
    let ceil = requested.min(model_max);
    // Transformer KV grows with every layer → clamp context so the f16 KV cache
    // fits in ~75% of available RAM (it lives in system RAM via --no-kv-offload).
    // Recurrent/hybrid archs (Mamba/RWKV/Granite-h/LFM2) keep a small FIXED state
    // — kv_bytes_per_token is None — so RAM isn't the limit; use the trained ctx
    // (bounded by the requested target). 4096 floor keeps a usable window; the
    // launch ladder still backs off if a too-optimistic allocation fails.
    match gguf.and_then(|g| g.kv_bytes_per_token()).filter(|&k| k > 0) {
        Some(kv_per_tok) => {
            // The adaptive path stores KV as q8_0 (~half of f16), so the same RAM
            // holds ~2x the context — budget against the quantized size so we can
            // actually reach the 32k target on 8 GB-class machines.
            let kv_q8 = (kv_per_tok / 2).max(1);
            let budget = (available_ram_bytes() as f64 * 0.75) as u64;
            let max_ctx = (budget / kv_q8).clamp(512, u32::MAX as u64) as u32;
            ceil.min(max_ctx).max(4096)
        }
        None => ceil.max(4096),
    }
}

/// One launch attempt's parameters.
#[derive(Debug, Clone)]
struct LaunchPlan {
    ngl: i32,
    ctx: u32,
    no_kv_offload: bool,
    flash_attn: bool,
    ubatch: Option<u32>,
    /// KV cache quantization (`--cache-type-k/v`), e.g. "q8_0" to halve the KV so
    /// a 32k context fits in RAM. None = f16 (also the fallback if a model/backend
    /// rejects quantized KV). Requires flash attention.
    cache_type: Option<&'static str>,
    /// Pass `--jinja` (use the model's embedded chat template; needed for tool
    /// calling). Off for models without an embedded template, or as a final
    /// fallback so the server still starts.
    jinja: bool,
}

/// Ordered launch attempts for a candidate: in auto mode on a GPU, take all the
/// VRAM (spill to GTT), keep KV in RAM, hold a big context, and degrade on
/// failure. Otherwise a single attempt with the configured values.
fn launch_plans(
    config: &ResolvedLlamaConfig,
    candidate: &BackendCandidate,
    gguf: Option<&crate::gguf::GgufInfo>,
) -> Vec<LaunchPlan> {
    let forced_cpu = config.backend_preference == "cpu" || candidate.backend == GpuBackend::Cpu;
    let is_gpu = !forced_cpu
        && (candidate.backend.is_gpu()
            || (candidate.backend == GpuBackend::Custom && config.gpu_layers.unwrap_or(999) > 0));

    // Only pass --jinja when the model embeds a chat template, otherwise the
    // server fails to start. Unknown (gguf unreadable) → assume yes (most
    // instruct models have one); a no-jinja fallback below covers the rest.
    let jinja = gguf.map(|g| g.has_chat_template).unwrap_or(true);

    // Advanced/manual: adaptive offload off → use the configured context + GPU
    // layers verbatim, no automatic KV/ctx management (the user is in control).
    if !config.auto_offload {
        let ngl = if forced_cpu {
            0
        } else {
            config
                .gpu_layers
                .unwrap_or(if is_gpu { 999 } else { 0 })
        };
        let mut plans = vec![LaunchPlan {
            ngl,
            ctx: config.context_size,
            no_kv_offload: false,
            flash_attn: false,
            ubatch: None,
            cache_type: None,
            jinja,
        }];
        // If we tried with --jinja, add a no-jinja fallback so the server still
        // starts for models with a broken/unsupported template (chat works,
        // tool calling may not).
        if jinja {
            plans.push(LaunchPlan { jinja: false, ..plans[0].clone() });
        }
        return plans;
    }

    let target = AUTO_CTX_TARGET.max(config.context_size);
    let base_ctx = ram_safe_ctx(target, gguf);
    let n_layers = gguf.and_then(|g| g.n_layers);
    let half_layers = n_layers.map(|n| (n / 2).max(1) as i32).unwrap_or(16);

    // Smart starting offload: when we can probe VRAM and know the model's weight
    // size + layer count, compute exactly how many layers fit (the rest run on
    // CPU — the iGPU/CPU split). When VRAM is unknown, request full offload (999)
    // and let the ladder degrade. This makes the FIRST launch land instead of
    // burning a failed full-offload attempt on small-VRAM machines.
    let model_bytes = std::fs::metadata(&config.model_path).map(|m| m.len()).unwrap_or(0);
    // KV-cache bytes at the chosen context (0 for recurrent/hybrid archs, which
    // keep a small fixed state — see kv_bytes_per_token).
    let kv_bytes = gguf
        .and_then(|g| g.kv_bytes_per_token())
        .unwrap_or(0)
        .saturating_mul(base_ctx as u64);
    let primary_ngl = if forced_cpu {
        0
    } else {
        match (probe_gpu_budget(), n_layers) {
        (Some(mem), Some(layers)) if model_bytes > 0 => {
            let weight_budget = if mem.integrated {
                // iGPU: weight memory (GTT) and the KV cache (RAM) are the same
                // physical RAM — keep the full context, then offload the rest.
                integrated_weight_budget(available_ram_bytes(), kv_bytes, mem.bytes, IGPU_RAM_RESERVE)
            } else {
                // Discrete: VRAM holds weights; the KV cache lives in separate RAM.
                mem.bytes
            };
            // A model whose weights fit the GPU's addressable budget is FULLY
            // offloaded: a partial GPU/CPU split is both unnecessary and crashes
            // llama.cpp's scheduler (GGML_ASSERT n_inputs < GGML_SCHED_MAX_SPLIT_INPUTS)
            // on some backends (notably Vulkan/RADV iGPUs). The KV cache stays in
            // RAM via --no-kv-offload, so it does NOT compete for this budget —
            // only the weights need to fit. Fall back to a computed split only
            // when the model genuinely can't fit the GPU.
            let ngl = if model_bytes <= (mem.bytes as f64 * 0.9) as u64 {
                layers as i32
            } else {
                fit_ngl(weight_budget, model_bytes, layers)
            };
            log::info!(
                "offload plan: gpu {} MiB ({}), model {} MiB/{} layers, ctx {} (kv {} MiB) -> ngl={}",
                mem.bytes / 1_048_576,
                if mem.integrated { "integrated" } else { "discrete" },
                model_bytes / 1_048_576,
                layers,
                base_ctx,
                kv_bytes / 1_048_576,
                ngl
            );
            ngl
        }
            _ => 999,
        }
    };

    // Quantize the KV cache to q8_0 (near-lossless, ~half of f16) so the 32k
    // target fits in RAM on all systems — CPU included. Only for transformer KV;
    // recurrent/hybrid archs keep a tiny fixed state (no KV to quantize).
    let cache_type: Option<&'static str> = gguf
        .and_then(|g| g.kv_bytes_per_token())
        .map(|_| "q8_0");

    let mut plans = vec![
        LaunchPlan { ngl: primary_ngl, ctx: base_ctx, no_kv_offload: true, flash_attn: true, ubatch: Some(256), cache_type, jinja },
        LaunchPlan { ngl: primary_ngl, ctx: (base_ctx / 2).max(2048), no_kv_offload: true, flash_attn: true, ubatch: Some(256), cache_type, jinja },
        LaunchPlan { ngl: half_layers, ctx: (base_ctx / 2).max(2048), no_kv_offload: true, flash_attn: true, ubatch: Some(128), cache_type, jinja },
    ];
    // If a model/backend rejects quantized KV (rare), retry once with f16 KV at a
    // reduced context before falling back further.
    if cache_type.is_some() {
        plans.push(LaunchPlan { ngl: half_layers, ctx: (base_ctx / 2).max(2048), no_kv_offload: true, flash_attn: true, ubatch: Some(128), cache_type: None, jinja });
    }
    // Last resort: start without --jinja so a model with a missing/broken
    // template still runs (chat-only; tools may not work).
    if jinja {
        plans.push(LaunchPlan { jinja: false, ngl: half_layers, ctx: (base_ctx / 2).max(2048), no_kv_offload: true, flash_attn: true, ubatch: Some(256), cache_type: None });
    }
    plans
}

/// Corrected LFM2.5 chat template. The upstream template breaks multi-turn tool
/// calling (ignores the `tool_calls` field and renders `content=None` as the
/// literal "null"); this fixed version reconstructs tool calls in LFM's native
/// format. Applied via `--chat-template-file` for `lfm2` models. See
/// https://huggingface.co/LiquidAI/LFM2.5-1.2B-Instruct/discussions/12
const LFM2_CHAT_TEMPLATE: &str = include_str!("../templates/lfm2.jinja");

/// Write the corrected LFM2 template to a temp file and return its path.
fn lfm2_template_path() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join("myelin-lfm2-chat-template.jinja");
    std::fs::write(&path, LFM2_CHAT_TEMPLATE)?;
    Ok(path)
}

pub async fn start_server(
    client: &Client,
    config: &ResolvedLlamaConfig,
) -> Result<ManagedLlamaServer> {
    let gguf = crate::gguf::read_gguf_info(&config.model_path).ok();
    if let Some(g) = &gguf {
        log::info!(
            "model: arch={:?} layers={:?} kv/token={:?}B ctx_train={:?}",
            g.architecture,
            g.n_layers,
            g.kv_bytes_per_token(),
            g.context_length
        );
    }
    if let Some(v) = free_device_local_vram() {
        log::info!("free device-local VRAM ~= {} MiB", v / 1_048_576);
    }

    // Chat-template override from the model profile (data-driven, no per-model
    // code): the builtin id "lfm2" writes the corrected LFM2 template (its
    // embedded one breaks multi-turn tool calling); any other value is a path to
    // a user-supplied template file. None → the model's embedded template via
    // --jinja, unchanged.
    let chat_template_file = match config.chat_template_override.as_deref() {
        Some("lfm2") => match lfm2_template_path() {
            Ok(p) => {
                log::info!("using corrected LFM2 chat template: {}", p.display());
                Some(p)
            }
            Err(e) => {
                log::warn!("failed to write LFM2 chat template: {e}");
                None
            }
        },
        Some(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => None,
    };

    let mut last_error: Option<String> = None;
    for candidate in &config.candidates {
        for plan in launch_plans(config, candidate, gguf.as_ref()) {
            match try_start_candidate(client, config, candidate, &plan, chat_template_file.as_deref()).await {
                Ok(server) => return Ok(server),
                Err(error) => {
                    log::warn!(
                        "llama-server {} ({}) ngl={} ctx={} failed: {error}",
                        candidate.executable_path.display(),
                        candidate.backend.label(),
                        plan.ngl,
                        plan.ctx
                    );
                    last_error = Some(error.to_string());
                }
            }
        }
    }

    bail!(
        "no llama-server backend could be started. Last error: {}",
        last_error.unwrap_or_else(|| "unknown".into())
    )
}

async fn try_start_candidate(
    client: &Client,
    config: &ResolvedLlamaConfig,
    candidate: &BackendCandidate,
    plan: &LaunchPlan,
    chat_template_file: Option<&Path>,
) -> Result<ManagedLlamaServer> {
    let gpu_layers = plan.ngl;
    let requested_gpu = gpu_layers > 0;

    let mut command = Command::new(&candidate.executable_path);
    command
        .arg("--host")
        .arg(&config.host)
        .arg("--port")
        .arg(config.port.to_string())
        .arg("--model")
        .arg(&config.model_path)
        .arg("--ctx-size")
        .arg(plan.ctx.to_string())
        .arg("--n-gpu-layers")
        .arg(gpu_layers.to_string())
        // Single slot: this is a single-user desktop app. Multiple slots split
        // the context window and scatter requests across cold slots, defeating
        // the prompt-prefix KV cache (the system + tool-schema prefix is large
        // and constant). One slot keeps the full ctx and reuses that prefix on
        // every request. Placed before extra_args so a user can override.
        .arg("--parallel")
        .arg("1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    // Adaptive offload: keep the KV cache in system RAM so a big context fits on
    // any VRAM, and use flash attention + a small ubatch to bound GPU buffers.
    if plan.no_kv_offload {
        command.arg("--no-kv-offload");
    }
    if plan.flash_attn {
        command.arg("--flash-attn").arg("on");
    }
    // Quantized KV cache (requires flash attention, set above) — halves the KV so
    // a 32k context fits in RAM. Falls back to f16 via the launch ladder if a
    // backend rejects it.
    if let Some(ct) = plan.cache_type {
        command
            .arg("--cache-type-k")
            .arg(ct)
            .arg("--cache-type-v")
            .arg(ct);
    }
    if let Some(ub) = plan.ubatch {
        command.arg("--ubatch-size").arg(ub.to_string());
    }

    // Pin a specific GPU (e.g. the iGPU to save battery) when requested and the
    // device id belongs to this candidate's backend. Guarded by the prefix so a
    // stale "CUDA0" is never passed to a Vulkan launch.
    if candidate.backend.is_gpu() && gpu_layers > 0 {
        if let Some(device) = config.gpu_device.as_deref().filter(|d| !d.is_empty()) {
            let prefix = candidate.backend.label(); // "cuda" | "vulkan" | "metal"
            if device.to_lowercase().starts_with(prefix) {
                command.arg("--device").arg(device);
            }
        } else if candidate.backend == GpuBackend::Vulkan {
            // Vulkan enumerates ALL GPUs and defaults to device 0 — on a hybrid
            // laptop that's usually the iGPU. Pick deliberately: the integrated
            // GPU in power-saving "vulkan" mode, otherwise the DISCRETE GPU for
            // performance (auto/gpu). No-op on single-GPU machines.
            let devices = list_devices_on(&candidate.executable_path, "vulkan");
            let pick = if config.backend_preference == "vulkan" {
                integrated_device_id(&devices)
            } else {
                discrete_device_id(&devices)
            };
            if let Some(id) = pick {
                log::info!("vulkan: pinning device {id} (pref={})", config.backend_preference);
                command.arg("--device").arg(id);
            }
        }
    }

    if let Some(threads) = config.threads {
        command.arg("--threads").arg(threads.to_string());
    }

    if let Some(chat_format) = &config.chat_format {
        command.arg("--chat-template").arg(chat_format);
    }

    // Use the model's embedded Jinja chat template (needed for correct tool
    // calling, e.g. LFM2). Only when the model actually has one — passing
    // --jinja to a template-less model fails to start. For LFM2 we override with
    // the corrected template file (the embedded one breaks multi-turn tools).
    if plan.jinja {
        if let Some(tpl) = chat_template_file {
            if config.chat_format.is_none() {
                command.arg("--chat-template-file").arg(tpl);
            }
        }
        command.arg("--jinja");
    }

    // Universal thinking/reasoning switch (model-agnostic via the chat template).
    command
        .arg("--reasoning")
        .arg(if config.thinking { "on" } else { "off" });

    command.args(&config.extra_args);
    // Self-heal: ensure the backend dir has its .so soname symlinks (covers
    // manually-installed or older downloads that dropped them).
    if let Some(dir) = candidate.executable_path.parent() {
        ensure_sonames(dir);
    }
    apply_library_path(&mut command, &candidate.executable_path);

    // Tie the server's lifetime to ours on Linux: if the app exits (e.g. a
    // `tauri dev` rebuild/restart), the kernel kills the server too. Otherwise
    // the orphaned server keeps holding the port and the next app instance
    // reuses it — running stale flags/prompt.
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }

    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to start llama-server at {}",
            candidate.executable_path.display()
        )
    })?;

    // Drain and capture stderr so the pipe never blocks the server, and so we
    // can read which compute backend actually loaded.
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let reader_handle = child.stderr.take().map(|stderr| {
        let captured = Arc::clone(&captured);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let mut guard = captured.lock().unwrap();
                if guard.len() < STDERR_CAPTURE_LINES {
                    guard.push(line);
                }
                // Keep looping past the cap to keep draining the pipe.
            }
        })
    });

    for _ in 0..STARTUP_ATTEMPTS {
        if health_check(client, config).await {
            let log_lines = captured.lock().unwrap().clone();
            // Trust the launch: a GPU candidate that came up healthy with ngl>0
            // IS offloading — llama.cpp aborts rather than silently running GPU
            // layers on the CPU, so a "CPU" verdict from the (build-dependent)
            // stderr strings would be a false negative. The stderr scan only
            // refines the label when we didn't request GPU layers ourselves.
            let gpu_offloaded = requested_gpu && candidate.backend.is_gpu();
            let active_backend = if gpu_offloaded {
                candidate.backend
            } else {
                detect_active_backend(&log_lines, candidate.backend)
            };

            let mut running = config.clone();
            running.executable_path = candidate.executable_path.clone();
            running.backend = Some(active_backend.label().to_string());

            return Ok(ManagedLlamaServer {
                config: running,
                child,
                active_backend,
                requested_gpu,
                gpu_offloaded,
                ctx_size: plan.ctx,
                _stderr_reader: reader_handle,
            });
        }

        // If the process already exited, stop waiting and let the caller try
        // the next backend.
        if let Ok(Some(_status)) = child.try_wait() {
            break;
        }

        thread::sleep(Duration::from_millis(STARTUP_DELAY_MS));
    }

    let _ = child.kill();
    let _ = child.wait();
    let tail = captured
        .lock()
        .unwrap()
        .iter()
        .rev()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ");
    bail!("started but never became healthy. {tail}")
}

/// Inspect llama.cpp startup log lines to determine which backend actually
/// loaded the model. Falls back to the requested backend if undetermined.
fn detect_active_backend(lines: &[String], requested: GpuBackend) -> GpuBackend {
    let mut saw_gpu_device = false;
    let mut detected: Option<GpuBackend> = None;

    for line in lines {
        let lower = line.to_lowercase();
        // Backend registration / device-init lines. llama.cpp varies by build —
        // CUDA: "loaded CUDA backend" / "ggml_cuda"; Vulkan: "loaded Vulkan
        // backend" / "ggml_vulkan: Found N Vulkan devices".
        if lower.contains("loaded cuda backend") || lower.contains("ggml_cuda") {
            detected = Some(GpuBackend::Cuda);
        } else if lower.contains("loaded vulkan backend")
            || lower.contains("ggml_vulkan")
            || lower.contains("vulkan devices")
        {
            detected = Some(GpuBackend::Vulkan);
        } else if lower.contains("loaded metal backend") || lower.contains("ggml_metal_init") {
            detected = Some(GpuBackend::Metal);
        }
        // Device assignment lines confirm the model is really on the GPU,
        // e.g. "using device CUDA0" / "offloaded 29/29 layers to GPU".
        if lower.contains("offloaded") && lower.contains("to gpu") {
            saw_gpu_device = true;
        }
        if lower.contains("using device") && (lower.contains("cuda") || lower.contains("vulkan")) {
            saw_gpu_device = true;
        }
    }

    match detected {
        // A GPU backend registered but nothing was offloaded → effectively CPU.
        Some(gpu) if gpu.is_gpu() => {
            if saw_gpu_device || requested.is_gpu() {
                gpu
            } else {
                GpuBackend::Cpu
            }
        }
        Some(other) => other,
        None => {
            // No backend line captured. Trust the request only if it was CPU;
            // otherwise we genuinely don't know, so report CPU conservatively.
            if requested == GpuBackend::Cpu {
                GpuBackend::Cpu
            } else if saw_gpu_device {
                requested
            } else {
                GpuBackend::Cpu
            }
        }
    }
}

pub async fn stop_server(server: &mut ManagedLlamaServer) {
    let _ = server.child.kill();
    let _ = server.child.wait();
}

/// A second llama-server running in embedding mode (for RAG). Lighter than the
/// chat server: CPU, mean pooling, small context — it doesn't compete with the
/// chat model for VRAM.
pub struct ManagedEmbedServer {
    pub child: Child,
    pub port: u16,
    pub model_path: PathBuf,
}

impl Drop for ManagedEmbedServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Spawn the embedding server (nomic-embed etc.) and wait until it's healthy.
pub async fn start_embed_server(
    client: &Client,
    executable: &Path,
    model_path: &Path,
    host: &str,
    port: u16,
) -> Result<ManagedEmbedServer> {
    let mut child = Command::new(executable)
        .arg("--host")
        .arg(host)
        .arg("--port")
        .arg(port.to_string())
        .arg("--model")
        .arg(model_path)
        .arg("--embedding")
        .arg("--pooling")
        .arg("mean")
        .arg("--ctx-size")
        .arg("2048")
        .arg("--no-warmup")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to spawn embedding server: {}", executable.display()))?;

    let base = format!("http://{host}:{port}");
    for _ in 0..80 {
        if client
            .get(format!("{base}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            log::info!("embedding server ready on port {port} ({})", model_path.display());
            return Ok(ManagedEmbedServer {
                child,
                port,
                model_path: model_path.to_path_buf(),
            });
        }
        if let Ok(Some(status)) = child.try_wait() {
            bail!("embedding server exited early ({status})");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = child.kill();
    bail!("embedding server did not become healthy")
}

pub async fn stop_embed_server(server: &mut ManagedEmbedServer) {
    let _ = server.child.kill();
    let _ = server.child.wait();
}

fn config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(CONFIG_FILE_NAME)
}

pub fn load_config(app_data_dir: &Path) -> Result<WorkspaceLlamaConfig> {
    let path = config_path(app_data_dir);
    if !path.exists() {
        return Ok(WorkspaceLlamaConfig::default());
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn set_model_path(app_data_dir: &Path, model_path: String) -> Result<()> {
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.model_path = Some(model_path);

    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let raw = serde_json::to_string_pretty(&config)?;
    fs::write(&path, raw)?;
    Ok(())
}

/// Provider kind from config: "local" (default) or "openai".
pub fn provider_kind(app_data_dir: &Path) -> String {
    load_config(app_data_dir)
        .ok()
        .and_then(|c| c.provider_kind)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

/// OpenAI-compatible API config (base URL, key, model) when using BYOK.
pub fn openai_config(app_data_dir: &Path) -> Option<(String, String, String)> {
    let cfg = load_config(app_data_dir).ok()?;
    let base = cfg.openai_base_url.as_ref()?;
    let key = cfg.openai_key.as_ref()?;
    let model = cfg.openai_model.as_ref()?;
    if base.is_empty() || key.is_empty() || model.is_empty() {
        return None;
    }
    Some((base.clone(), key.clone(), model.clone()))
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
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.searxng_url = url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
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
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.quick_capture_shortcut = Some(shortcut.trim().to_string()).filter(|s| !s.is_empty());
    let cfg_path = config_path(app_data_dir);
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&cfg_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

/// Set (or clear, when empty) the embedding model GGUF path.
pub fn set_embed_model_path(app_data_dir: &Path, path: Option<String>) -> Result<()> {
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.embed_model_path = path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let cfg_path = config_path(app_data_dir);
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&cfg_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

pub fn set_deterministic_tools(app_data_dir: &Path, enabled: bool) -> Result<()> {
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.deterministic_tools = Some(enabled);

    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(&config)?;
    fs::write(&path, raw)?;
    Ok(())
}

pub fn set_tool_gating(app_data_dir: &Path, enabled: bool) -> Result<()> {
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.tool_gating = Some(enabled);

    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(&config)?;
    fs::write(&path, raw)?;
    Ok(())
}

pub fn set_provider(app_data_dir: &Path, kind: &str) -> Result<()> {
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.provider_kind = Some(kind.to_string());
    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

pub fn set_openai_config(
    app_data_dir: &Path,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
) -> Result<()> {
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.openai_base_url = base_url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    config.openai_key = api_key.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    config.openai_model = model.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

pub fn set_executable_path(app_data_dir: &Path, executable_path: String) -> Result<()> {
    let mut config = load_config(app_data_dir).unwrap_or_default();
    config.executable_path = Some(executable_path);

    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let raw = serde_json::to_string_pretty(&config)?;
    fs::write(&path, raw)?;
    Ok(())
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
    let mut config = load_config(app_data_dir).unwrap_or_default();
    if let Some(cs) = context_size {
        config.context_size = Some(cs);
    }
    if let Some(gl) = gpu_layers {
        config.gpu_layers = Some(gl);
    }
    if let Some(t) = threads {
        config.threads = Some(t);
    }
    if let Some(temp) = temperature {
        config.temperature = Some(temp);
    }
    if let Some(tp) = top_p {
        config.top_p = Some(tp);
    }
    if let Some(ea) = extra_args {
        config.extra_args = ea;
    }
    if let Some(bp) = backend_preference {
        config.backend_preference = Some(normalize_preference(Some(&bp)));
    }
    if let Some(dev) = gpu_device {
        // Empty string clears the pin (back to automatic device choice).
        config.gpu_device = if dev.trim().is_empty() { None } else { Some(dev) };
    }
    if let Some(t) = thinking {
        config.thinking = Some(t);
    }
    if let Some(ao) = auto_offload {
        config.auto_offload = Some(ao);
    }
    if let Some(mt) = max_turns {
        config.max_turns = Some(mt.clamp(1, 12));
    }

    let path = config_path(app_data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let raw = serde_json::to_string_pretty(&config)?;
    fs::write(&path, raw)?;
    Ok(())
}

/// Resolve the provider config: for the local provider this resolves the binary
/// and model path; for the OpenAI (BYOK) provider it builds a synthetic config
/// from the stored base URL, key, and model name.
pub fn resolve_provider_config(app_data_dir: &Path) -> Result<ResolvedLlamaConfig> {
    if provider_kind(app_data_dir) == "openai" {
        return build_openai_resolved(app_data_dir);
    }
    resolve_config(app_data_dir)
}

/// Build the ordered list of llama-server binaries to try, best backend first.
///
/// A "root" is a directory that may contain per-backend subfolders
/// (`<root>/cuda/`, `<root>/vulkan/`, `<root>/metal/`, `<root>/cpu/`) plus a
/// flat `<root>/llama-server` used as a CPU fallback. Roots, in order:
/// 1. The directory of a configured `executablePath` (so users can drop a
///    `cuda/` folder beside their existing binary and auto-upgrade).
/// 2. `<app_data>/bin`.
///
/// `MYELIN_LLAMA_SERVER_PATH` remains a hard single override for power users.
fn resolve_candidates(
    app_data_dir: &Path,
    workspace_config: &WorkspaceLlamaConfig,
    preference: &str,
) -> Result<Vec<BackendCandidate>> {
    if let Ok(path) = env::var("MYELIN_LLAMA_SERVER_PATH") {
        let exe = validate_existing_file(resolve_input_path(app_data_dir, &path), "llama-server")?;
        return Ok(vec![BackendCandidate {
            backend: GpuBackend::Custom,
            executable_path: exe,
        }]);
    }

    let mut candidates: Vec<BackendCandidate> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let push = |candidates: &mut Vec<BackendCandidate>,
                seen: &mut Vec<PathBuf>,
                backend: GpuBackend,
                exe: PathBuf| {
        if exe.is_file() && !seen.contains(&exe) {
            seen.push(exe.clone());
            candidates.push(BackendCandidate {
                backend,
                executable_path: exe,
            });
        }
    };

    // Collect tiering roots in priority order.
    let roots = tiering_roots(app_data_dir, workspace_config);
    let configured_exe = workspace_config
        .executable_path
        .as_ref()
        .map(|raw| resolve_input_path(app_data_dir, raw));

    // GPU/CPU backend subfolders, per detected hardware, across all roots.
    for backend in desired_backends(preference) {
        if let Some(dir) = backend.dir_name() {
            for root in &roots {
                push(
                    &mut candidates,
                    &mut seen,
                    backend,
                    root.join(dir).join(executable_name()),
                );
            }
        }
    }

    // Flat fallbacks: the configured binary, each root's flat binary, then
    // PATH. These are `Custom` (unknown) rather than `Cpu` — a flat extraction
    // may itself be a GPU build, so it keeps the configured gpu_layers and the
    // real backend is detected from the startup log.
    if let Some(exe) = configured_exe {
        push(&mut candidates, &mut seen, GpuBackend::Custom, exe);
    }
    for root in &roots {
        push(
            &mut candidates,
            &mut seen,
            GpuBackend::Custom,
            root.join(executable_name()),
        );
    }
    if let Some(path) = find_on_path(executable_name()) {
        push(&mut candidates, &mut seen, GpuBackend::Custom, path);
    }

    if candidates.is_empty() {
        bail!(
            "llama-server not found. Set MYELIN_LLAMA_SERVER_PATH, add executablePath in app settings, install a backend under {}, or put {} on PATH.",
            app_data_dir.join("bin").display(),
            executable_name()
        );
    }

    Ok(candidates)
}

fn resolve_model_path(
    app_data_dir: &Path,
    workspace_config: &WorkspaceLlamaConfig,
) -> Result<PathBuf> {
    if let Ok(path) = env::var("MYELIN_LLAMA_MODEL_PATH") {
        return validate_existing_model(resolve_input_path(app_data_dir, &path));
    }

    if let Some(path) = &workspace_config.model_path {
        return validate_existing_model(resolve_input_path(app_data_dir, path));
    }

    let models = discover_gguf_models(app_data_dir)?;
    match models.as_slice() {
        [] => bail!(
            "no .gguf model found. Set MYELIN_LLAMA_MODEL_PATH, choose a model in Settings, or place a single .gguf file in the app data directory."
        ),
        [model] => Ok(model.clone()),
        _ => {
            let preferred = models
                .iter()
                .find(|path| {
                    path.components().any(|component| {
                        component.as_os_str().to_string_lossy().eq_ignore_ascii_case("models")
                    })
                })
                .cloned();

            preferred.ok_or_else(|| {
                anyhow!(
                    "multiple .gguf models found. Choose one explicitly in Settings."
                )
            })
        }
    }
}

fn discover_gguf_models(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut models = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                ".git" | "node_modules" | "target" | "build" | "dist"
            )
        })
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("gguf"))
                .unwrap_or(false)
        {
            models.push(entry.path().to_path_buf());
        }
    }

    models.sort();
    Ok(models)
}

fn validate_existing_file(path: PathBuf, label: &str) -> Result<PathBuf> {
    if path.is_file() {
        Ok(path)
    } else {
        bail!("{} path does not exist: {}", label, path.display())
    }
}

fn validate_existing_model(path: PathBuf) -> Result<PathBuf> {
    if !path.is_file() {
        bail!("model path does not exist: {}", path.display());
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("gguf"))
        .unwrap_or(false)
    {
        bail!("model must be a .gguf file: {}", path.display());
    }
    Ok(path)
}

fn resolve_input_path(workspace: &Path, raw: &str) -> PathBuf {
    let input = PathBuf::from(raw);
    if input.is_absolute() {
        input
    } else {
        workspace.join(input)
    }
}

fn executable_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// On Linux/macOS the dynamic loader does not search the executable's own
/// directory, so a relocatable llama-server can't find its sibling
/// `libggml*.so` / `libllama.so` and fails to launch (which surfaces as an
/// unreachable server / rig completion error). Prepend the binary's directory
/// to the child's library search path. No-op on Windows, where DLLs load from
/// the executable's directory automatically.
fn apply_library_path(command: &mut Command, executable: &Path) {
    #[cfg(not(target_os = "windows"))]
    if let Some(dir) = executable.parent() {
        let var = if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut paths = vec![dir.to_path_buf()];
        if let Some(existing) = env::var_os(var) {
            paths.extend(env::split_paths(&existing));
        }
        if let Ok(joined) = env::join_paths(paths) {
            command.env(var, joined);
        }
    }
    #[cfg(target_os = "windows")]
    let _ = (command, executable);
}

/// llama.cpp's Linux/macOS archives ship versioned libraries
/// (`libllama.so.0.0.9585`) alongside soname symlinks (`libllama.so.0`) that the
/// binary actually needs. Copying only regular files drops those symlinks, so
/// recreate them: for every `lib*.so.<major>...` real file, ensure a
/// `lib*.so.<major>` symlink exists. Idempotent and best-effort. No-op on
/// Windows (DLLs aren't versioned this way).
fn ensure_sonames(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(idx) = name.find(".so.") else {
                continue;
            };
            let major: String = name[idx + 4..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if major.is_empty() {
                continue;
            }
            let soname = format!("{}.so.{}", &name[..idx], major);
            if soname == name {
                continue; // already the bare soname
            }
            let link = dir.join(&soname);
            if !link.exists() {
                let _ = symlink(&name, &link); // relative target, best-effort
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn find_on_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for directory in env::split_paths(&path_var) {
        let candidate = directory.join(binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::fit_ngl;

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    #[test]
    fn picks_discrete_gpu_on_hybrid_keeps_igpu_alone() {
        use super::{discrete_device_id, integrated_device_id, DeviceInfo};
        let dev = |id: &str, name: &str| DeviceInfo {
            id: id.into(),
            name: name.into(),
            backend: "vulkan".into(),
        };
        // Hybrid laptop: prefer the discrete NVIDIA, identify the Intel iGPU.
        let hybrid = vec![
            dev("Vulkan0", "Intel(R) UHD Graphics"),
            dev("Vulkan1", "NVIDIA GeForce RTX 2050"),
        ];
        assert_eq!(discrete_device_id(&hybrid).as_deref(), Some("Vulkan1"));
        assert_eq!(integrated_device_id(&hybrid).as_deref(), Some("Vulkan0"));
        // Single iGPU (the Linux box): no discrete choice — use the only device.
        let single = vec![dev("Vulkan0", "AMD Radeon Graphics (RADV RENOIR)")];
        assert_eq!(discrete_device_id(&single), None);
        assert_eq!(integrated_device_id(&single).as_deref(), Some("Vulkan0"));
    }

    #[test]
    fn integrated_budget_reserves_kv_then_caps_at_gtt() {
        use super::integrated_weight_budget;
        let g = GIB;
        // 16 GiB RAM, 2 GiB KV, 1.5 GiB reserve, 7 GiB GTT → RAM-KV-reserve=12.5,
        // capped at GTT → 7 GiB.
        assert_eq!(integrated_weight_budget(16 * g, 2 * g, 7 * g, 3 * g / 2), 7 * g);
        // Low RAM dominates: 6 GiB RAM, 3 GiB KV, 1.5 reserve → 1.5 GiB (< 7 GTT).
        assert_eq!(integrated_weight_budget(6 * g, 3 * g, 7 * g, 3 * g / 2), 3 * g / 2);
        // Over-committed RAM → 0 (fall to CPU), never underflows.
        assert_eq!(integrated_weight_budget(2 * g, 3 * g, 7 * g, 3 * g / 2), 0);
    }

    #[test]
    fn fit_ngl_full_when_it_fits() {
        // 4 GiB free, 856 MiB model, 40 layers → all fit.
        assert_eq!(fit_ngl(4 * GIB, 856 * MIB, 40), 40);
    }

    #[test]
    fn fit_ngl_partial_split() {
        // 1 GiB free, 2 GiB model, 40 layers → a partial GPU/CPU split.
        let ngl = fit_ngl(GIB, 2 * GIB, 40);
        assert!(ngl > 0 && ngl < 40, "expected partial split, got {ngl}");
    }

    #[test]
    fn fit_ngl_zero_when_nothing_fits() {
        assert_eq!(fit_ngl(64 * MIB, 4 * GIB, 40), 0);
    }

    #[test]
    fn fit_ngl_unknown_geometry_requests_all() {
        assert_eq!(fit_ngl(GIB, 0, 40), 999);
        assert_eq!(fit_ngl(GIB, 1024, 0), 999);
    }
}
