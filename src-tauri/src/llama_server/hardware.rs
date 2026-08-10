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
/// Directory holding binaries bundled with the installer (resource dir / bin),
/// set once at startup. Used as a low-priority tiering root so a shipped app
/// finds its CPU/Vulkan builds with zero setup.
static RESOURCE_BIN_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn set_resource_bin_dir(dir: Option<PathBuf>) {
    let _ = RESOURCE_BIN_DIR.set(dir);
}

pub(super) fn resource_bin_dir() -> Option<PathBuf> {
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
                            l.contains("vga")
                                || l.contains("3d controller")
                                || l.contains("display")
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
pub fn installed_backends(
    app_data_dir: &Path,
    workspace_config: &WorkspaceLlamaConfig,
) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for root in tiering_roots(app_data_dir, workspace_config) {
        for backend in [
            GpuBackend::Cuda,
            GpuBackend::Vulkan,
            GpuBackend::Metal,
            GpuBackend::Cpu,
        ] {
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

pub fn installed_bee_backends(app_data_dir: &Path) -> Vec<String> {
    ["cuda", "vulkan", "metal", "cpu"]
        .iter()
        .filter(|backend| bee_backend_binary(app_data_dir, backend).is_some())
        .map(|backend| backend.to_string())
        .collect()
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
pub(super) fn list_devices_on(exe: &Path, backend_label: &str) -> Vec<DeviceInfo> {
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
        "uhd",
        "iris",
        "integrated",
        "radeon graphics",
        "hd graphics",
        "renoir",
        "cezanne",
        "rembrandt",
        "phoenix",
    ];
    let n = name.to_lowercase();
    HINTS.iter().any(|h| n.contains(h))
}

/// The integrated GPU's device id (for power-saving), if any.
pub(super) fn integrated_device_id(devices: &[DeviceInfo]) -> Option<String> {
    devices
        .iter()
        .find(|d| is_integrated_gpu_name(&d.name))
        .map(|d| d.id.clone())
}

/// The first DISCRETE GPU's device id, when more than one GPU is present — so a
/// hybrid laptop uses the fast dGPU instead of Vulkan's default device 0 (often
/// the iGPU). `None` on a single-GPU machine: there's no choice to make.
pub(super) fn discrete_device_id(devices: &[DeviceInfo]) -> Option<String> {
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

fn bee_backend_binary(app_data_dir: &Path, backend_label: &str) -> Option<PathBuf> {
    let candidate = app_data_dir
        .join("bin")
        .join("bee")
        .join(backend_label)
        .join(executable_name());
    candidate.is_file().then_some(candidate)
}
