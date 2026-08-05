//! Versioned, file-managed AI configuration.
//!
//! This module is deliberately independent from the llama-server process
//! manager. It owns the user-facing configuration contract and provides a
//! compatibility projection for the existing launcher while that launcher is
//! migrated incrementally.

use anyhow::{anyhow, bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = "ai-config.json";
pub const APPLIED_FILE_NAME: &str = "ai-config.applied.json";
pub const SCHEMA_FILE_NAME: &str = "ai-config.schema.json";
pub const VALIDATIONS_FILE_NAME: &str = "ai-runtime-validations.json";
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigFile {
    pub version: u32,
    pub active_profile: String,
    #[serde(default)]
    pub profiles: BTreeMap<String, AiLaunchProfile>,
    #[serde(default)]
    pub runtimes: BTreeMap<String, RuntimeDefinition>,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default)]
    pub tools: ToolingConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiLaunchProfile {
    pub model_path: PathBuf,
    pub runtime: String,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub inference: InferenceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self { Self { host: "127.0.0.1".into(), port: 39281 } }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InferenceConfig {
    pub context_size: u32,
    pub gpu_layers: Option<i32>,
    pub threads: Option<u32>,
    pub backend: String,
    pub gpu_device: Option<String>,
    pub temperature: f32,
    pub top_p: f32,
    pub chat_format: Option<String>,
    pub thinking: bool,
    pub auto_offload: bool,
    pub max_turns: u32,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self { context_size: 24096, gpu_layers: None, threads: None, backend: "auto".into(),
            gpu_device: None, temperature: 0.0, top_p: 0.95, chat_format: None,
            thinking: false, auto_offload: true, max_turns: 4, extra_args: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum RuntimeSource {
    Bundled { runtime: BuiltinRuntime },
    Path { executable: PathBuf },
    Download { url: String, sha256: String, archive_format: RuntimeArchiveFormat, binary_path: Option<PathBuf> },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinRuntime { Stock, Bee }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeArchiveFormat { Zip, TarGz, Raw }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDefinition {
    pub display_name: String,
    pub source: RuntimeSource,
    #[serde(default = "default_cache_reuse")]
    pub cache_reuse_tokens: u32,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

fn default_cache_reuse() -> u32 { 256 }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CacheConfig { pub max_bytes: u64 }
impl Default for CacheConfig { fn default() -> Self { Self { max_bytes: 8 * 1024 * 1024 * 1024 } } }

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    pub enabled: bool,
    pub model_path: Option<PathBuf>,
    pub runtime: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub context_size: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalConfig { pub searxng_url: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolingConfig { pub deterministic: bool, pub gating: bool }
impl Default for ToolingConfig { fn default() -> Self { Self { deterministic: true, gating: false } } }

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub port: Option<u16>, pub bin_path: Option<String>, pub tool_mode: Option<String>,
    pub strict: Option<bool>, pub prompt_tools: Option<bool>, pub call_only: Option<bool>,
    pub no_think: Option<bool>, pub narrow: Option<bool>, pub slm: Option<bool>,
    pub friendly_results: Option<bool>, pub max_calls: Option<usize>, pub total_max: Option<usize>,
    pub tool_timeout_secs: Option<u64>, pub generation_timeout_secs: Option<u64>,
    pub tool_subset: Option<String>, pub base_url_override: Option<String>,
    pub tool_choice: Option<String>, pub template_kwargs: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigError { pub path: String, pub category: String, pub message: String }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigStatus {
    pub config_path: String,
    pub schema_path: String,
    pub candidate_hash: Option<String>,
    pub applied_hash: Option<String>,
    pub has_unapplied_changes: bool,
    pub validation_state: String,
    pub active_profile: Option<String>,
    pub runtime_id: Option<String>,
    pub model_path: Option<String>,
    pub ai_available: bool,
    pub errors: Vec<AiConfigError>,
}

pub fn config_path(app_data: &Path) -> PathBuf { app_data.join(CONFIG_FILE_NAME) }
pub fn applied_path(app_data: &Path) -> PathBuf { app_data.join(APPLIED_FILE_NAME) }

pub fn default_config() -> AiConfigFile {
    AiConfigFile { version: CONFIG_VERSION, active_profile: "default".into(), profiles: BTreeMap::new(),
        runtimes: BTreeMap::new(), cache: CacheConfig::default(), embedding: EmbeddingConfig::default(),
        retrieval: RetrievalConfig::default(), tools: ToolingConfig::default(), agent: AgentConfig::default() }
}

pub fn validate(config: &AiConfigFile) -> Vec<AiConfigError> {
    let mut errors = Vec::new();
    if config.version != CONFIG_VERSION { errors.push(err("/version", "schema", format!("unsupported version {}", config.version))); }
    if !config.profiles.contains_key(&config.active_profile) {
        errors.push(err("/activeProfile", "semantic", "active profile does not exist"));
    }
    for (id, profile) in &config.profiles {
        validate_id(id, "/profiles", &mut errors);
        if !profile.model_path.is_absolute() { errors.push(err(&format!("/profiles/{id}/modelPath"), "semantic", "model path must be absolute")); }
        if !(1024..=65535).contains(&profile.server.port) { errors.push(err(&format!("/profiles/{id}/server/port"), "semantic", "port must be between 1024 and 65535")); }
        if profile.inference.context_size == 0 { errors.push(err(&format!("/profiles/{id}/inference/contextSize"), "semantic", "context size must be positive")); }
        if !profile.inference.temperature.is_finite() || profile.inference.temperature < 0.0 { errors.push(err(&format!("/profiles/{id}/inference/temperature"), "semantic", "temperature must be finite and non-negative")); }
        if !profile.inference.top_p.is_finite() || !(0.0..=1.0).contains(&profile.inference.top_p) { errors.push(err(&format!("/profiles/{id}/inference/topP"), "semantic", "topP must be between 0 and 1")); }
        if !config.runtimes.contains_key(&profile.runtime) && !matches!(profile.runtime.as_str(), "stock" | "bee") { errors.push(err(&format!("/profiles/{id}/runtime"), "semantic", "runtime does not exist")); }
    }
    for (id, runtime) in &config.runtimes { validate_id(id, "/runtimes", &mut errors); validate_runtime(id, runtime, &mut errors); }
    if config.cache.max_bytes == 0 { errors.push(err("/cache/maxBytes", "semantic", "cache budget must be positive")); }
    errors
}

fn validate_id(id: &str, root: &str, errors: &mut Vec<AiConfigError>) {
    let valid = !id.is_empty() && id.len() <= 64 && id.chars().enumerate().all(|(i, c)| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' ) && i > 0 || i == 0 && c.is_ascii_alphanumeric());
    if !valid { errors.push(err(&format!("{root}/{id}"), "schema", "id must match [A-Za-z0-9][A-Za-z0-9._-]{0,63}")); }
}

fn validate_runtime(id: &str, runtime: &RuntimeDefinition, errors: &mut Vec<AiConfigError>) {
    if !(1..=4096).contains(&runtime.cache_reuse_tokens) { errors.push(err(&format!("/runtimes/{id}/cacheReuseTokens"), "semantic", "must be between 1 and 4096")); }
    for key in runtime.environment.keys() { if matches!(key.as_str(), "PATH" | "HOME" | "LD_LIBRARY_PATH" | "DYLD_LIBRARY_PATH") { errors.push(err(&format!("/runtimes/{id}/environment/{key}"), "semantic", "environment key is managed by Myelin")); } }
    if let RuntimeSource::Download { url, sha256, binary_path, .. } = &runtime.source {
        if !url.starts_with("https://") { errors.push(err(&format!("/runtimes/{id}/source/url"), "semantic", "download URL must use HTTPS")); }
        if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) { errors.push(err(&format!("/runtimes/{id}/source/sha256"), "semantic", "SHA-256 must be 64 hexadecimal characters")); }
        if binary_path.as_ref().is_some_and(|p| p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir))) { errors.push(err(&format!("/runtimes/{id}/source/binaryPath"), "semantic", "binary path must remain inside the archive")); }
    }
}

fn err(path: &str, category: &str, message: impl Into<String>) -> AiConfigError { AiConfigError { path: path.into(), category: category.into(), message: message.into() } }

pub fn load(app_data: &Path) -> Result<AiConfigFile> {
    let path = config_path(app_data);
    if !path.exists() { return Ok(default_config()); }
    let text = fs::read_to_string(&path).with_context(|| format!("failed reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("invalid AI config JSON at {}", path.display()))
}

pub fn write_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow!("configuration has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path).with_context(|| format!("failed replacing {}", path.display()))?;
    Ok(())
}

pub fn canonical_hash(config: &AiConfigFile) -> Result<String> {
    let bytes = serde_json::to_vec(config)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn ensure_schema(app_data: &Path) -> Result<()> {
    let schema = schemars::schema_for!(AiConfigFile);
    write_atomic(&app_data.join(SCHEMA_FILE_NAME), &schema)
}

pub fn ensure_file(app_data: &Path) -> Result<bool> {
    ensure_schema(app_data)?;
    let path = config_path(app_data);
    if path.exists() { return Ok(false); }
    let config = migrate_legacy(app_data).unwrap_or_else(|_| default_config());
    write_atomic(&path, &config)?;
    // Bootstrap an applied snapshot only when the migrated candidate is
    // structurally valid. Runtime protocol validation still happens before a
    // custom executable is used by the launcher.
    if validate(&config).is_empty() {
        write_atomic(&applied_path(app_data), &config)?;
    }
    Ok(true)
}

fn migrate_legacy(app_data: &Path) -> Result<AiConfigFile> {
    let legacy_path = app_data.join("llama-server.json");
    if !legacy_path.exists() { return Ok(default_config()); }
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&legacy_path)?)?;
    let get = |name: &str| value.get(name).cloned().filter(|v| !v.is_null());
    let engine = get("inferenceEngine").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "llama_cpp".into());
    let runtime_id = if engine == "beellama" { "bee" } else { "stock" };
    let model = get("modelPath").and_then(|v| v.as_str().map(PathBuf::from)).unwrap_or_else(|| app_data.join("model.gguf"));
    let mut config = default_config();
    config.runtimes.insert("stock".into(), RuntimeDefinition { display_name: "Stock llama.cpp".into(), source: RuntimeSource::Bundled { runtime: BuiltinRuntime::Stock }, cache_reuse_tokens: 256, extra_args: Vec::new(), environment: BTreeMap::new() });
    config.runtimes.insert("bee".into(), RuntimeDefinition { display_name: "BeeLlama".into(), source: RuntimeSource::Bundled { runtime: BuiltinRuntime::Bee }, cache_reuse_tokens: 64, extra_args: Vec::new(), environment: BTreeMap::new() });
    let mut inference = InferenceConfig::default();
    if let Some(v) = get("contextSize").and_then(|v| v.as_u64()) { inference.context_size = v as u32; }
    if let Some(v) = get("gpuLayers").and_then(|v| v.as_i64()) { inference.gpu_layers = Some(v as i32); }
    if let Some(v) = get("threads").and_then(|v| v.as_u64()) { inference.threads = Some(v as u32); }
    if let Some(v) = get("temperature").and_then(|v| v.as_f64()) { inference.temperature = v as f32; }
    if let Some(v) = get("topP").and_then(|v| v.as_f64()) { inference.top_p = v as f32; }
    if let Some(v) = get("thinking").and_then(|v| v.as_bool()) { inference.thinking = v; }
    if let Some(v) = get("autoOffload").and_then(|v| v.as_bool()) { inference.auto_offload = v; }
    if let Some(v) = get("maxTurns").and_then(|v| v.as_u64()) { inference.max_turns = v as u32; }
    if let Some(v) = get("extraArgs").and_then(|v| v.as_array().cloned()) { inference.extra_args = v.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(); }
    let server = ServerConfig { host: get("host").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "127.0.0.1".into()), port: get("port").and_then(|v| v.as_u64()).unwrap_or(39281) as u16 };
    config.profiles.insert("default".into(), AiLaunchProfile { model_path: model, runtime: runtime_id.into(), server, inference });
    Ok(config)
}

pub fn status(app_data: &Path) -> AiConfigStatus {
    let config = load(app_data).ok();
    let errors = config.as_ref().map(validate).unwrap_or_else(|| vec![err("/", "syntax", "unable to parse ai-config.json")]);
    let candidate_hash = config.as_ref().and_then(|c| canonical_hash(c).ok());
    let applied = fs::read_to_string(applied_path(app_data)).ok().and_then(|s| serde_json::from_str::<AiConfigFile>(&s).ok());
    let applied_hash = applied.as_ref().and_then(|c| canonical_hash(c).ok());
    let profile = config.as_ref().and_then(|c| c.profiles.get(&c.active_profile));
    AiConfigStatus {
        config_path: config_path(app_data).display().to_string(),
        schema_path: app_data.join(SCHEMA_FILE_NAME).display().to_string(),
        has_unapplied_changes: candidate_hash.is_some() && candidate_hash != applied_hash,
        candidate_hash, applied_hash,
        validation_state: if config.is_none() { "invalid".into() } else if errors.is_empty() { "valid".into() } else { "invalid".into() },
        active_profile: config.as_ref().map(|c| c.active_profile.clone()),
        runtime_id: profile.map(|p| p.runtime.clone()),
        model_path: profile.map(|p| p.model_path.display().to_string()),
        ai_available: errors.is_empty() && profile.is_some(), errors,
    }
}

pub fn require_valid(config: &AiConfigFile) -> Result<()> {
    let errors = validate(config);
    if errors.is_empty() { Ok(()) } else { bail!("AI configuration is invalid: {}", errors.iter().map(|e| format!("{}: {}", e.path, e.message)).collect::<Vec<_>>().join("; ")) }
}
