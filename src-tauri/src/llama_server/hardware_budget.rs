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
pub(super) const AUTO_CTX_TARGET: u32 = 32768;

/// Available system RAM in bytes (cross-platform, via sysinfo).
pub(super) fn available_ram_bytes() -> u64 {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.available_memory()
}

/// llama.cpp's fallback thread count is deliberately conservative. For a
/// desktop inference server, use every physical core unless the user supplied
/// an explicit value; SMT siblings rarely improve prompt evaluation enough to
/// justify becoming the default.
pub(crate) fn default_inference_threads() -> u32 {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.physical_core_count()
        .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
        .unwrap_or(1)
        .max(1) as u32
}

pub(super) fn cache_reuse_tokens(engine: &str) -> &'static str {
    if engine == "beellama" {
        "64"
    } else {
        "256"
    }
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
                    return Some(GpuBudget {
                        bytes: vram_free + gtt_free,
                        integrated: true,
                    });
                }
                return Some(GpuBudget {
                    bytes: vram_free,
                    integrated: false,
                });
            }
        }
    }
    // NVIDIA via nvidia-smi — discrete, separate VRAM (Linux + Windows).
    if let Some(v) = nvidia_smi_free_vram() {
        return Some(GpuBudget {
            bytes: v,
            integrated: false,
        });
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
            Some(GpuBudget {
                bytes: discrete,
                integrated: false,
            })
        } else if integrated > 0 {
            Some(GpuBudget {
                bytes: integrated,
                integrated: true,
            })
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
pub(super) fn fit_ngl(free_vram: u64, model_weight_bytes: u64, n_layers: u64) -> i32 {
    if n_layers == 0 || model_weight_bytes == 0 {
        return 999;
    }
    let per_layer = (model_weight_bytes / n_layers).max(1);
    let budget = (free_vram as f64 * 0.85) as u64;
    (budget / per_layer).min(n_layers) as i32
}

/// OS + app headroom kept free of model/KV allocations on an integrated GPU.
pub(super) const IGPU_RAM_RESERVE: u64 = 1536 * 1024 * 1024; // 1.5 GiB

/// Weight-offload budget for an integrated GPU. Its offloaded weights (GTT) and
/// the KV cache (--no-kv-offload) both live in system RAM, so **full context
/// wins first**: reserve the KV cache (sized for the chosen context) plus OS
/// headroom, then offer the remaining RAM for weights — capped by what the iGPU
/// can actually address (its GTT budget). This maximises offload without
/// over-committing RAM and OOM-ing, while keeping the full context window.
pub(super) fn integrated_weight_budget(
    available_ram: u64,
    kv_bytes: u64,
    gtt_budget: u64,
    reserve: u64,
) -> u64 {
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
pub(super) fn ram_safe_ctx(
    requested: u32,
    gguf: Option<&crate::gguf::GgufInfo>,
    quantized_kv: bool,
) -> u32 {
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
            // GPU-backed adaptive plans store KV as q8_0 (~half of f16). The
            // measured CPU profile keeps faster f16 KV, so budget its full size.
            let budgeted_kv = if quantized_kv {
                (kv_per_tok / 2).max(1)
            } else {
                kv_per_tok
            };
            let budget = (available_ram_bytes() as f64 * 0.75) as u64;
            let max_ctx = (budget / budgeted_kv).clamp(512, u32::MAX as u64) as u32;
            ceil.min(max_ctx).max(4096)
        }
        None => ceil.max(4096),
    }
}
