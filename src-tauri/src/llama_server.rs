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
fn integrated_device_id(devices: &[DeviceInfo]) -> Option<String> {
    const HINTS: [&str; 9] = [
        "uhd", "iris", "integrated", "radeon graphics", "hd graphics", "renoir",
        "cezanne", "rembrandt", "phoenix",
    ];
    devices
        .iter()
        .find(|d| {
            let n = d.name.to_lowercase();
            HINTS.iter().any(|h| n.contains(h))
        })
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

/// Normalize a user backend preference to "auto" | "cuda" | "vulkan" |
/// "metal" | "cpu". Legacy "gpu" maps to "auto".
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
        thinking: app_config.thinking.unwrap_or(false),
        auto_offload: app_config.auto_offload.unwrap_or(true),
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

/// Free device-local VRAM in bytes (true fast VRAM, not GTT), best-effort and
/// cross-platform. `None` when undeterminable — we then just request full
/// offload and let it spill to GTT / retry on failure. Informational for now.
pub fn free_device_local_vram() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        // AMD exposes true device-local VRAM via sysfs (total/used).
        for n in 0..4u8 {
            let base = format!("/sys/class/drm/card{n}/device");
            if let Ok(total) = std::fs::read_to_string(format!("{base}/mem_info_vram_total")) {
                if let Ok(t) = total.trim().parse::<u64>() {
                    let used = std::fs::read_to_string(format!("{base}/mem_info_vram_used"))
                        .ok()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .unwrap_or(0);
                    return Some(t.saturating_sub(used));
                }
            }
        }
        // NVIDIA via nvidia-smi (MiB).
        if let Ok(out) = Command::new("nvidia-smi")
            .args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if out.status.success() {
                if let Some(first) = String::from_utf8_lossy(&out.stdout).lines().next() {
                    if let Ok(mib) = first.trim().parse::<u64>() {
                        return Some(mib.saturating_mul(1024 * 1024));
                    }
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Windows/macOS: full-offload + retry handles fit; DXGI/Metal probing
        // can be added later as a starting-ngl optimizer.
        None
    }
}

/// Largest context whose f16 KV cache fits in ~60% of available RAM (KV lives in
/// system RAM via --no-kv-offload), bounded by the model's trained context.
fn ram_safe_ctx(requested: u32, gguf: Option<&crate::gguf::GgufInfo>) -> u32 {
    let kv_per_tok = match gguf.and_then(|g| g.kv_bytes_per_token()).filter(|&k| k > 0) {
        Some(k) => k,
        None => return requested, // unknown geometry → trust the request
    };
    let budget = (available_ram_bytes() as f64 * 0.6) as u64;
    let max_ctx = (budget / kv_per_tok).clamp(512, u32::MAX as u64) as u32;
    let model_max = gguf
        .and_then(|g| g.context_length)
        .map(|c| c.min(u32::MAX as u64) as u32)
        .unwrap_or(u32::MAX);
    requested.min(max_ctx).min(model_max).max(512)
}

/// One launch attempt's parameters.
#[derive(Debug, Clone)]
struct LaunchPlan {
    ngl: i32,
    ctx: u32,
    no_kv_offload: bool,
    flash_attn: bool,
    ubatch: Option<u32>,
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

    if !config.auto_offload || !is_gpu {
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
    let half_layers = gguf
        .and_then(|g| g.n_layers)
        .map(|n| (n / 2).max(1) as i32)
        .unwrap_or(16);

    let mut plans = vec![
        LaunchPlan { ngl: 999, ctx: base_ctx, no_kv_offload: true, flash_attn: true, ubatch: Some(256), jinja },
        LaunchPlan { ngl: 999, ctx: (base_ctx / 2).max(2048), no_kv_offload: true, flash_attn: true, ubatch: Some(256), jinja },
        LaunchPlan { ngl: half_layers, ctx: (base_ctx / 2).max(2048), no_kv_offload: true, flash_attn: true, ubatch: Some(128), jinja },
    ];
    // Last resort: start without --jinja so a model with a missing/broken
    // template still runs (chat-only; tools may not work).
    if jinja {
        plans.push(LaunchPlan { jinja: false, ngl: 999, ctx: (base_ctx / 2).max(2048), no_kv_offload: true, flash_attn: true, ubatch: Some(256) });
    }
    plans
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

    let mut last_error: Option<String> = None;
    for candidate in &config.candidates {
        for plan in launch_plans(config, candidate, gguf.as_ref()) {
            match try_start_candidate(client, config, candidate, &plan).await {
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
        } else if candidate.backend == GpuBackend::Vulkan && config.backend_preference == "vulkan" {
            // Power-saving "Vulkan" mode: prefer the integrated GPU when the
            // machine also has a discrete one.
            let devices = list_devices_on(&candidate.executable_path, "vulkan");
            if let Some(id) = integrated_device_id(&devices) {
                log::info!("vulkan power-saving: pinning integrated device {id}");
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
    // --jinja to a template-less model fails to start.
    if plan.jinja {
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
