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
        Some("lfm25") => match lfm25_template_path() {
            Ok(p) => {
                log::info!("using corrected LFM2.5 chat template: {}", p.display());
                Some(p)
            }
            Err(e) => {
                log::warn!("failed to write LFM2.5 chat template: {e}");
                None
            }
        },
        Some(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => None,
    };

    let mut last_error: Option<String> = None;
    for candidate in &config.candidates {
        for plan in launch_plans(config, candidate, gguf.as_ref()) {
            log::info!(
                "starting llama-server {} ({}) ngl={} ctx={}",
                candidate.executable_path.display(),
                candidate.backend.label(),
                plan.ngl,
                plan.ctx
            );
            match try_start_candidate(
                client,
                config,
                candidate,
                &plan,
                chat_template_file.as_deref(),
            )
            .await
            {
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
    let slot_save_path = config
        .slot_save_path
        .parent()
        .map(|parent| parent.join(&candidate.engine))
        .unwrap_or_else(|| config.slot_save_path.clone());

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
    if config.prompt_cache {
        fs::create_dir_all(&slot_save_path).with_context(|| {
            format!("failed to create slot cache {}", slot_save_path.display())
        })?;
        command
            .arg("--slot-save-path")
            .arg(&slot_save_path)
            // `--slots` is a boolean endpoint switch in llama-server, not a
            // numeric slot count. The slot count is controlled by --parallel.
            .arg("--slots")
            // Reuse stable prompt chunks across successive chat renderings.
            // llama.cpp defaults this to 0 (disabled), even with cache_prompt.
            .arg("--cache-reuse")
            // BeeLlama's recurrent cache needs chunk reuse, but the minimal
            // direct-chat system prefix is commonly well below 256 tokens.
            // Stock llama.cpp keeps the more conservative granularity.
            .arg(cache_reuse_tokens(&candidate.engine));
    }

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
                log::info!(
                    "vulkan: pinning device {id} (pref={})",
                    config.backend_preference
                );
                command.arg("--device").arg(id);
            }
        }
    }

    let threads = config.threads.unwrap_or_else(default_inference_threads);
    command.arg("--threads").arg(threads.to_string());

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

    if gpu_moe_policy_active(config, candidate) {
        if has_tensor_override(&config.extra_args) {
            log::info!(
                "GPU MoE policy: user tensor override supplied; automatic expert CPU placement skipped"
            );
        } else {
            log::info!(
                "GPU MoE policy active for {}: keeping MoE expert tensors in CPU RAM",
                candidate.backend.label()
            );
            command
                .arg("--override-tensor")
                .arg(GPU_MOE_TENSOR_OVERRIDE);
        }
    }

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

    let startup_started = std::time::Instant::now();
    loop {
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
            running.inference_engine = candidate.engine.clone();
            running.slot_save_path = slot_save_path.clone();

            return Ok(ManagedLlamaServer {
                config: running,
                child,
                active_backend,
                active_engine: candidate.engine.clone(),
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

        if has_fatal_startup_error(&captured.lock().unwrap()) {
            break;
        }

        if startup_started.elapsed() >= Duration::from_secs(STARTUP_TIMEOUT_SECS) {
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
    log::warn!(
        "llama-server {} did not become healthy after {:.1}s",
        candidate.executable_path.display(),
        startup_started.elapsed().as_secs_f32()
    );
    bail!(
        "started but never became healthy within {}s. {tail}",
        STARTUP_TIMEOUT_SECS
    )
}

/// Fatal initialization messages mean retrying the same plan cannot help. Keep
/// this deliberately narrow: ordinary warnings and model-loading progress must
/// not cause a healthy slow launch to be abandoned.
pub(super) fn has_fatal_startup_error(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("unknown argument")
            || lower.contains("unrecognized option")
            || lower.contains("invalid argument")
            || lower.contains("cannot load model")
            || lower.contains("failed to load model")
            || lower.contains("ggml_assert")
            || lower.contains("out of memory")
            || lower.contains("cuda error")
            || lower.contains("vulkan error")
            || lower.contains("metal error")
    })
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
    #[cfg(unix)]
    unsafe {
        libc::kill(server.child.id() as i32, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = server.child.kill();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if matches!(server.child.try_wait(), Ok(Some(_))) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    log::warn!("llama-server did not exit within 2s of graceful termination; killing it");
    let _ = server.child.kill();
    let _ = server.child.wait();
}
