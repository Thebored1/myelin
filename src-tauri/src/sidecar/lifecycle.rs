//! Sidecar process lifecycle and binary resolution.

use super::transport::compatible_health;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tauri::Manager;

pub const SIDECAR_NAME: &str = "openharn-myelin";
pub const DEFAULT_PORT: u16 = 8091;
pub const SIDECAR_PROTOCOL_VERSION: u64 = 4;

/// How long to keep draining the sidecar's SSE stream after posting a cancel
/// request: it needs a moment to emit its final `done` (with the partial turn)
/// before the connection can be dropped.
pub const CANCEL_DRAIN_SECS: u64 = 15;

/// Reuse loopback connections for health checks and sidecar requests. The
/// sidecar is long-lived, so constructing a new client for every chat turn
/// needlessly throws away the connection pool.
/// A running sidecar process plus the HTTP base we talk to it on.
pub struct ManagedSidecar {
    pub base: String,
    pub token: String,
    _child: Child,
}

impl Drop for ManagedSidecar {
    fn drop(&mut self) {
        let _ = self._child.kill();
    }
}

/// Full target triple for the current platform, used to locate the Tauri
/// externalBin build (`<name>-<triple>`).
fn target_triple() -> &'static str {
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "windows"),
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "macos")
    )))]
    {
        "unknown"
    }
}

/// Resolve the sidecar binary path. Precedence:
///   1. `OPENHARN_MYELIN_BIN` env var (absolute path).
///   2. `<resource_dir>/bin/<name>-<target-triple>` (Tauri externalBin).
///   3. `<resource_dir>/bin/<name>` (already-suffixed / dev bundle).
fn resolve_sidecar_bin(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("OPENHARN_MYELIN_BIN") {
        if !explicit.trim().is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    let dir = resource_dir?;
    let triple = target_triple();
    let with_ext = if cfg!(target_os = "windows") {
        format!("{SIDECAR_NAME}.exe")
    } else {
        SIDECAR_NAME.to_string()
    };
    // Tauri maps `resources/bin` -> `<resource_dir>/bin` at build time, but in
    // `tauri dev` the source tree (`<resource_dir>/resources/bin`) is used, so
    // probe both locations (mirrors how llama_server resolves its binaries).
    let mut roots: Vec<PathBuf> = vec![dir.join("bin"), dir.join("resources").join("bin")];
    roots.retain(|r| r.is_dir());
    let mut candidates: Vec<PathBuf> = Vec::new();
    if triple != "unknown" {
        let suffixed = if cfg!(target_os = "windows") {
            format!("{SIDECAR_NAME}-{triple}.exe")
        } else {
            format!("{SIDECAR_NAME}-{triple}")
        };
        for r in &roots {
            candidates.push(r.join(&suffixed));
        }
    }
    for r in &roots {
        candidates.push(r.join(&with_ext));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Spawn the sidecar if it isn't already running, wait for it to report healthy,
/// and return its HTTP base URL. Idempotent: re-checks the running process and
/// relaunches if it died.
pub async fn ensure_sidecar(state: &AppState) -> Result<String> {
    let mut guard = state.inner.ai.sidecar.lock().await;

    if let Some(sc) = guard.as_ref() {
        if compatible_health(&sc.base, Some(&sc.token)).await {
            return Ok(sc.base.clone());
        }
        // Process died — drop the stale handle (kill is best-effort).
        *guard = None;
    }

    let oh = state.openharn_settings();
    let resource_dir = state.handle.path().resource_dir().ok();

    // Binary resolution: explicit path in settings > OPENHARN_MYELIN_BIN env >
    // bundled/resource resolution.
    let bin = oh
        .bin_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| {
            std::env::var("OPENHARN_MYELIN_BIN")
                .ok()
                .map(|p| PathBuf::from(p))
                .filter(|p| p.exists())
        })
        .or_else(|| resolve_sidecar_bin(resource_dir.as_deref()))
        .ok_or_else(|| {
            anyhow!(
                "openharn-myelin sidecar binary not found. Build it (npm run build:sidecar) and \
             place it under the app's bin dir, or set its path in Settings > Agent (openharn)."
            )
        })?;

    let port = oh
        .port
        .filter(|&p| p != 0)
        .or_else(|| {
            std::env::var("OPENHARN_MYELIN_PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok())
        })
        .unwrap_or(DEFAULT_PORT);
    let base = format!("http://127.0.0.1:{port}");
    let token = uuid::Uuid::new_v4().to_string();

    log::info!("[sidecar] launching {bin:?} on {base}");
    let mut command = Command::new(&bin);
    command.arg("--port").arg(port.to_string());
    command.env("OPENHARN_MYELIN_TOKEN", &token);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    // Kill the sidecar if the app dies (mirrors llama-server's PR_SET_PDEATHSIG).
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to start sidecar at {bin:?}"))?;

    let managed = ManagedSidecar {
        base: base.clone(),
        token: token.clone(),
        _child: child,
    };

    // Wait for readiness (the sidecar prints a readiness line, but we just poll
    // /health so we don't depend on stdout capture).
    let mut ready = false;
    for _ in 0..50 {
        if compatible_health(&base, Some(&token)).await {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        return Err(anyhow!(
            "openharn-myelin sidecar at {base} is unavailable or incompatible \
             (required protocol {SIDECAR_PROTOCOL_VERSION}). Rebuild it with \
             `npm run build:sidecar:debug`."
        ));
    }

    *guard = Some(managed);
    Ok(base)
}
