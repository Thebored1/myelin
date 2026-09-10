use super::*;
use std::path::PathBuf;

/// One launch attempt's parameters.
#[derive(Debug, Clone)]
pub(super) struct LaunchPlan {
    pub(super) ngl: i32,
    pub(super) ctx: u32,
    pub(super) no_kv_offload: bool,
    pub(super) flash_attn: bool,
    pub(super) ubatch: Option<u32>,
    /// KV cache quantization (`--cache-type-k/v`), e.g. "q8_0" to halve the KV so
    /// a 32k context fits in RAM. None = f16 (also the fallback if a model/backend
    /// rejects quantized KV). Requires flash attention.
    pub(super) cache_type: Option<&'static str>,
    /// Pass `--jinja` (use the model's embedded chat template; needed for tool
    /// calling). Off for models without an embedded template, or as a final
    /// fallback so the server still starts.
    pub(super) jinja: bool,
}

/// Ordered launch attempts for a candidate: in auto mode on a GPU, take all the
/// VRAM (spill to GTT), keep KV in RAM, hold a big context, and degrade on
/// failure. Otherwise a single attempt with the configured values.
pub(super) fn launch_plans(
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
            config.gpu_layers.unwrap_or(if is_gpu { 999 } else { 0 })
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
            plans.push(LaunchPlan {
                jinja: false,
                ..plans[0].clone()
            });
        }
        return plans;
    }

    let target = AUTO_CTX_TARGET.max(config.context_size);
    let base_ctx = ram_safe_ctx(target, gguf, !forced_cpu);
    let n_layers = gguf.and_then(|g| g.n_layers);
    let half_layers = n_layers.map(|n| (n / 2).max(1) as i32).unwrap_or(16);

    // Smart starting offload: when we can probe VRAM and know the model's weight
    // size + layer count, compute exactly how many layers fit (the rest run on
    // CPU — the iGPU/CPU split). When VRAM is unknown, request full offload (999)
    // and let the ladder degrade. This makes the FIRST launch land instead of
    // burning a failed full-offload attempt on small-VRAM machines.
    let model_bytes = std::fs::metadata(&config.model_path)
        .map(|m| m.len())
        .unwrap_or(0);
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
                    integrated_weight_budget(
                        available_ram_bytes(),
                        kv_bytes,
                        mem.bytes,
                        IGPU_RAM_RESERVE,
                    )
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
    // target fits in RAM on GPU-backed systems. Only for transformer KV;
    // recurrent/hybrid archs keep a tiny fixed state (no KV to quantize).
    let cache_type: Option<&'static str> =
        gguf.and_then(|g| g.kv_bytes_per_token()).map(|_| "q8_0");

    // CPU inference has no VRAM buffers to protect. Inheriting the GPU ladder's
    // small ubatch and forced flash-attention severely throttles prompt
    // evaluation, while its fallback attempts cannot improve a CPU launch.
    if forced_cpu {
        let mut plans = vec![LaunchPlan {
            ngl: 0,
            ctx: base_ctx,
            no_kv_offload: false,
            flash_attn: false,
            ubatch: Some(512),
            // The local CPU sweep found f16 KV ~12–14% faster than q8_0 for
            // both prompt evaluation and generation.
            cache_type: None,
            jinja,
        }];
        if jinja {
            plans.push(LaunchPlan {
                jinja: false,
                ..plans[0].clone()
            });
        }
        return plans;
    }

    let mut plans = vec![
        LaunchPlan {
            ngl: primary_ngl,
            ctx: base_ctx,
            no_kv_offload: true,
            flash_attn: true,
            ubatch: Some(256),
            cache_type,
            jinja,
        },
        LaunchPlan {
            ngl: primary_ngl,
            ctx: (base_ctx / 2).max(2048),
            no_kv_offload: true,
            flash_attn: true,
            ubatch: Some(256),
            cache_type,
            jinja,
        },
        LaunchPlan {
            ngl: half_layers,
            ctx: (base_ctx / 2).max(2048),
            no_kv_offload: true,
            flash_attn: true,
            ubatch: Some(128),
            cache_type,
            jinja,
        },
    ];
    // If a model/backend rejects quantized KV (rare), retry once with f16 KV at a
    // reduced context before falling back further.
    if cache_type.is_some() {
        plans.push(LaunchPlan {
            ngl: half_layers,
            ctx: (base_ctx / 2).max(2048),
            no_kv_offload: true,
            flash_attn: true,
            ubatch: Some(128),
            cache_type: None,
            jinja,
        });
    }
    // Last resort: start without --jinja so a model with a missing/broken
    // template still runs (chat-only; tools may not work).
    if jinja {
        plans.push(LaunchPlan {
            jinja: false,
            ngl: half_layers,
            ctx: (base_ctx / 2).max(2048),
            no_kv_offload: true,
            flash_attn: true,
            ubatch: Some(256),
            cache_type: None,
        });
    }
    plans
}

/// Corrected LFM2.5 chat template. The upstream template breaks multi-turn tool
/// calling (ignores the `tool_calls` field and renders `content=None` as the
/// literal "null"); this fixed version reconstructs tool calls in LFM's native
/// format. Applied via `--chat-template-file` for `lfm2` models. See
/// https://huggingface.co/LiquidAI/LFM2.5-1.2B-Instruct/discussions/12
const LFM2_CHAT_TEMPLATE: &str = include_str!("../../templates/lfm2.jinja");
const LFM25_CHAT_TEMPLATE: &str = include_str!("../../templates/lfm25.jinja");

/// Write the corrected LFM2 template to a temp file and return its path.
pub(super) fn lfm2_template_path() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join("myelin-lfm2-chat-template.jinja");
    std::fs::write(&path, LFM2_CHAT_TEMPLATE)?;
    Ok(path)
}

pub(super) fn lfm25_template_path() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join("myelin-lfm25-chat-template.jinja");
    std::fs::write(&path, LFM25_CHAT_TEMPLATE)?;
    Ok(path)
}
