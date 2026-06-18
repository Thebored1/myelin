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
    /// Max agent tool-calling turns before forcing a final answer.
    pub max_turns: Option<u32>,
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
    /// Max agent tool-calling turns before forcing a final answer.
    #[serde(default)]
    pub max_turns: u32,
    /// Ordered list of binaries to try (best first). Not serialized to the UI.
    #[serde(skip)]
    pub candidates: Vec<BackendCandidate>,
}

impl ResolvedLlamaConfig {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    pub fn model_name(&self) -> String {
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
    /// Drains the server's stderr for the process lifetime so its pipe never
    /// fills and stalls generation. Detaches; exits on child EOF.
    _stderr_reader: Option<thread::JoinHandle<()>>,
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
    let exe = match backend_binary(app_data_dir, &workspace_config, backend_label) {
        Some(exe) => exe,
        None => return Vec::new(),
    };

    let mut cmd = Command::new(&exe);
    cmd.arg("--list-devices")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_library_path(&mut cmd, &exe);
    let output = cmd.output();

    let Ok(output) = output else {
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

/// Normalize a user backend preference to "auto" | "cuda" | "vulkan" |
/// "metal" | "cpu". Legacy "gpu" maps to "auto".
fn normalize_preference(raw: Option<&str>) -> String {
    match raw.map(|p| p.trim().to_lowercase()).as_deref() {
        Some("cpu") => "cpu".into(),
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

    Ok(ResolvedLlamaConfig {
        executable_path: primary.executable_path.clone(),
        model_path,
        host,
        port,
        context_size: app_config.context_size.unwrap_or(4096),
        gpu_layers: app_config.gpu_layers,
        threads: app_config.threads,
        temperature: app_config.temperature.unwrap_or(0.2),
        top_p: app_config.top_p.unwrap_or(0.95),
        chat_format: app_config.chat_format.clone(),
        extra_args: app_config.extra_args.clone(),
        backend: Some(primary.backend.label().to_string()),
        backend_preference: preference,
        gpu_device: app_config
            .gpu_device
            .clone()
            .filter(|d| !d.trim().is_empty()),
        max_turns: app_config.max_turns.filter(|&n| n > 0).unwrap_or(4),
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

pub async fn start_server(
    client: &Client,
    config: &ResolvedLlamaConfig,
) -> Result<ManagedLlamaServer> {
    let mut last_error: Option<String> = None;

    for candidate in &config.candidates {
        match try_start_candidate(client, config, candidate).await {
            Ok(server) => return Ok(server),
            Err(error) => {
                log::warn!(
                    "llama-server candidate {} ({}) failed to start: {error}",
                    candidate.executable_path.display(),
                    candidate.backend.label()
                );
                last_error = Some(error.to_string());
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
) -> Result<ManagedLlamaServer> {
    // Force 0 layers when the user picked CPU, or for the explicit `cpu/`
    // backend, so the server never probes for a device. GPU backends and
    // unknown/custom binaries (which may themselves be GPU builds) honour the
    // configured gpu_layers. requested_gpu reflects whether we asked for offload.
    let gpu_layers = if config.backend_preference == "cpu" {
        0
    } else {
        match candidate.backend {
            GpuBackend::Cpu => 0,
            _ => config.gpu_layers.unwrap_or(999),
        }
    };
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
        .arg(config.context_size.to_string())
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

    // Pin a specific GPU (e.g. the iGPU to save battery) when requested and the
    // device id belongs to this candidate's backend. Guarded by the prefix so a
    // stale "CUDA0" is never passed to a Vulkan launch.
    if candidate.backend.is_gpu() && gpu_layers > 0 {
        if let Some(device) = config.gpu_device.as_deref().filter(|d| !d.is_empty()) {
            let prefix = candidate.backend.label(); // "cuda" | "vulkan" | "metal"
            if device.to_lowercase().starts_with(prefix) {
                command.arg("--device").arg(device);
            }
        }
    }

    if let Some(threads) = config.threads {
        command.arg("--threads").arg(threads.to_string());
    }

    if let Some(chat_format) = &config.chat_format {
        command.arg("--chat-template").arg(chat_format);
    }

    command.args(&config.extra_args);
    apply_library_path(&mut command, &candidate.executable_path);

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
            let active_backend = detect_active_backend(&log_lines, candidate.backend);
            let gpu_offloaded = active_backend.is_gpu();

            let mut running = config.clone();
            running.executable_path = candidate.executable_path.clone();
            running.backend = Some(active_backend.label().to_string());

            return Ok(ManagedLlamaServer {
                config: running,
                child,
                active_backend,
                requested_gpu,
                gpu_offloaded,
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
        // Backend registration lines, e.g. "loaded CUDA backend from ...".
        if lower.contains("loaded cuda backend") {
            detected = Some(GpuBackend::Cuda);
        } else if lower.contains("loaded vulkan backend") {
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

fn config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(CONFIG_FILE_NAME)
}

fn load_config(app_data_dir: &Path) -> Result<WorkspaceLlamaConfig> {
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
