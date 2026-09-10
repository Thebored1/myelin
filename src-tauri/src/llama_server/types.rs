use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Child;
use std::thread;

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
    pub(super) fn dir_name(self) -> Option<&'static str> {
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
        matches!(
            self,
            GpuBackend::Cuda | GpuBackend::Vulkan | GpuBackend::Metal
        )
    }
}

/// Whether an explicitly selected GPU backend should keep MoE experts in CPU
/// RAM for this backend attempt. `auto` and CPU fallbacks intentionally do not
/// enable this policy: they should preserve the normal adaptive placement.
pub(super) fn gpu_moe_policy_active(
    config: &ResolvedLlamaConfig,
    candidate: &BackendCandidate,
) -> bool {
    candidate.backend.is_gpu() && config.backend_preference == candidate.backend.label()
}

/// User-supplied tensor placement always takes precedence over the automatic
/// MoE policy. Accept both `--override-tensor VALUE` and `--override-tensor=VALUE`
/// (and the short `-ot` spelling).
pub(super) fn has_tensor_override(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "-ot"
            || arg == "--override-tensor"
            || arg.starts_with("-ot=")
            || arg.starts_with("--override-tensor=")
    })
}

/// One launchable llama-server binary plus the backend it provides.
#[derive(Debug, Clone)]
pub struct BackendCandidate {
    pub backend: GpuBackend,
    pub executable_path: PathBuf,
    /// "beellama" for the experimental fork, otherwise "llama_cpp".
    pub engine: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceLlamaConfig {
    /// Inference engine: "llama_cpp" (default) or experimental "beellama".
    pub inference_engine: Option<String>,
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
    /// Optional cross-encoder reranker GGUF. It is served by a dedicated stock
    /// llama-server and never shares the embedding runtime.
    pub reranker_model_path: Option<String>,
    /// Deterministic correctness tools: regex format_note, find_in_note word
    /// search, and the destructive-write guard. Default ON — they make tool use
    /// more reliable. None → on.
    pub deterministic_tools: Option<bool>,
    /// Per-message tool gating: offer the model only the tools its message
    /// warrants, via keyword intent heuristics. **Default OFF** — the model gets
    /// the full toolset every turn and decides for itself (model-agnostic, the
    /// standard agent approach). Opt-in only for sub-2B models that misfire on
    /// tools they shouldn't touch; the heuristics are brittle and can withhold a
    /// valid tool (e.g. block a web search the model would have run). None → off.
    pub tool_gating: Option<bool>,
    /// Persist llama-server slot snapshots between note opens/restarts.
    pub prompt_cache: Option<bool>,
    /// Global hotkey that opens the quick-capture window (e.g. "Ctrl+Space").
    /// None → the default ("Ctrl+Space").
    pub quick_capture_shortcut: Option<String>,
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
    pub inference_engine: String,
    pub executable_path: PathBuf,
    /// Stable identity of the executable that produced section KV snapshots.
    /// This prevents a custom fork from sharing the historical llama_cpp path.
    #[serde(default)]
    pub runtime_fingerprint: String,
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
    /// Compatibility hint for models that may benefit from prompt-tools + strict
    /// grammar (e.g. LFM2 at low quants). Informational only; user settings
    /// choose the actual strategy.
    #[serde(default)]
    pub prefers_prompt_tools: Option<bool>,
    /// Keep parameter descriptions in model-facing schemas. Off by default;
    /// enable in a model profile only when a weak model needs the duplication.
    #[serde(default)]
    pub verbose_tool_schemas: bool,
    /// Optional profile suggestion for `tool_choice` in native FC mode. The
    /// sidecar uses the explicit Openharn setting instead of applying this silently.
    #[serde(default)]
    pub tool_choice: Option<String>,
    /// Optional profile suggestion for chat-template kwargs. The sidecar uses
    /// explicit Openharn settings instead of applying this silently.
    #[serde(default)]
    pub template_kwargs: Option<String>,
    /// Deterministic correctness tools (format_note / find_in_note / write
    /// guard). Default true; on a more capable model they can be disabled.
    #[serde(default = "default_true")]
    pub deterministic_tools: bool,
    /// Per-message tool gating (offer only the tools a message warrants).
    /// Default OFF — the model gets the full toolset every turn (model-agnostic).
    #[serde(default = "default_false")]
    pub tool_gating: bool,
    /// Persist the single inference slot under app data.
    #[serde(default = "default_true")]
    pub prompt_cache: bool,
    /// App-managed directory passed to llama-server's slot persistence API.
    #[serde(skip)]
    pub slot_save_path: PathBuf,
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
        self.inference_engine == other.inference_engine
            && self.runtime_fingerprint == other.runtime_fingerprint
            && self.executable_path == other.executable_path
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
            && self.prompt_cache == other.prompt_cache
    }

    /// A selected experimental/GPU candidate may legitimately fall back to a
    /// later candidate. Treat that running candidate as stable until settings
    /// change instead of retrying the failed preferred binary on every turn.
    pub fn accepts_running(&self, running: &Self) -> bool {
        let is_candidate = self.candidates.iter().any(|candidate| {
            candidate.executable_path == running.executable_path
                && candidate.engine == running.inference_engine
        });
        if !is_candidate {
            return false;
        }
        let mut expected = self.clone();
        expected.executable_path = running.executable_path.clone();
        expected.inference_engine = running.inference_engine.clone();
        expected.backend = running.backend.clone();
        expected.slot_save_path = running.slot_save_path.clone();
        expected.matches_runtime(running)
    }
}

pub struct ManagedLlamaServer {
    pub config: ResolvedLlamaConfig,
    pub child: Child,
    /// The backend that actually loaded, detected from the server's startup log.
    pub active_backend: GpuBackend,
    pub active_engine: String,
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
    pub(super) _stderr_reader: Option<thread::JoinHandle<()>>,
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
    pub installed_bee_backends: Vec<String>,
}
