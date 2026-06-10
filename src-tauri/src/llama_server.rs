use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const CONFIG_FILE_PATH: &str = ".myelin/llama-server.json";
const DEFAULT_HOST: &str = "127.0.0.1";
const STARTUP_ATTEMPTS: usize = 60;
const STARTUP_DELAY_MS: u64 = 500;

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
    }
}

pub struct ManagedLlamaServer {
    pub config: ResolvedLlamaConfig,
    pub child: Child,
}

#[derive(Debug, Clone)]
pub struct LlamaProviderInfo {
    pub resolved: Option<ResolvedLlamaConfig>,
    pub config: WorkspaceLlamaConfig,
    pub healthy: bool,
    pub detail: String,
}


pub fn inspect_provider(workspace: &Path) -> Result<LlamaProviderInfo> {
    let workspace_config = load_workspace_config(workspace).unwrap_or_default();
    match resolve_config(workspace) {
        Ok(config) => Ok(LlamaProviderInfo {
            detail: format!(
                "Ready to use {} with model {}.",
                config.executable_path.display(),
                config.model_path.display()
            ),
            resolved: Some(config),
            config: workspace_config,
            healthy: true,
        }),
        Err(error) => Ok(LlamaProviderInfo {
            detail: error.to_string(),
            resolved: None,
            config: workspace_config,
            healthy: false,
        }),
    }
}

pub fn resolve_config(workspace: &Path) -> Result<ResolvedLlamaConfig> {
    let workspace_config = load_workspace_config(workspace)?;
    let host = workspace_config
        .host
        .clone()
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    let port = workspace_config.port.unwrap_or_else(find_free_port);
    let executable_path = resolve_executable_path(workspace, &workspace_config)?;
    let model_path = resolve_model_path(workspace, &workspace_config)?;

    Ok(ResolvedLlamaConfig {
        executable_path,
        model_path,
        host,
        port,
        context_size: workspace_config.context_size.unwrap_or(4096),
        gpu_layers: workspace_config.gpu_layers,
        threads: workspace_config.threads,
        temperature: workspace_config.temperature.unwrap_or(0.2),
        top_p: workspace_config.top_p.unwrap_or(0.95),
        chat_format: workspace_config.chat_format.clone(),
        extra_args: workspace_config.extra_args.clone(),
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
    let mut command = Command::new(&config.executable_path);
    command
        .arg("--host")
        .arg(&config.host)
        .arg("--port")
        .arg(config.port.to_string())
        .arg("--model")
        .arg(&config.model_path)
        .arg("--ctx-size")
        .arg(config.context_size.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(gpu_layers) = config.gpu_layers {
        command.arg("--n-gpu-layers").arg(gpu_layers.to_string());
    }

    if let Some(threads) = config.threads {
        command.arg("--threads").arg(threads.to_string());
    }

    if let Some(chat_format) = &config.chat_format {
        command.arg("--chat-template").arg(chat_format);
    }

    command.args(&config.extra_args);

    let child = command.spawn().with_context(|| {
        format!(
            "failed to start llama-server at {}",
            config.executable_path.display()
        )
    })?;

    for _ in 0..STARTUP_ATTEMPTS {
        if health_check(client, config).await {
            return Ok(ManagedLlamaServer {
                config: config.clone(),
                child,
            });
        }
        thread::sleep(Duration::from_millis(STARTUP_DELAY_MS));
    }

    let mut child = child;
    let _ = child.kill();
    let _ = child.wait();
    bail!("llama-server started but never became healthy")
}

pub async fn stop_server(server: &mut ManagedLlamaServer) {
    let _ = server.child.kill();
    let _ = server.child.wait();
}


fn load_workspace_config(workspace: &Path) -> Result<WorkspaceLlamaConfig> {
    let path = workspace.join(CONFIG_FILE_PATH);
    if !path.exists() {
        return Ok(WorkspaceLlamaConfig::default());
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn set_workspace_model_path(workspace: &Path, model_path: String) -> Result<()> {
    let mut config = load_workspace_config(workspace).unwrap_or_default();
    config.model_path = Some(model_path);
    
    let path = workspace.join(CONFIG_FILE_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let raw = serde_json::to_string_pretty(&config)?;
    fs::write(&path, raw)?;
    Ok(())
}

pub fn set_workspace_executable_path(workspace: &Path, executable_path: String) -> Result<()> {
    let mut config = load_workspace_config(workspace).unwrap_or_default();
    config.executable_path = Some(executable_path);
    
    let path = workspace.join(CONFIG_FILE_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let raw = serde_json::to_string_pretty(&config)?;
    fs::write(&path, raw)?;
    Ok(())
}

fn resolve_executable_path(
    workspace: &Path,
    workspace_config: &WorkspaceLlamaConfig,
) -> Result<PathBuf> {
    if let Ok(path) = env::var("MYELIN_LLAMA_SERVER_PATH") {
        return validate_existing_file(resolve_input_path(workspace, &path), "llama-server");
    }

    if let Some(path) = &workspace_config.executable_path {
        return validate_existing_file(resolve_input_path(workspace, path), "llama-server");
    }

    let workspace_candidates = [
        workspace
            .join(".myelin")
            .join("bin")
            .join(executable_name()),
        workspace.join("bin").join(executable_name()),
    ];
    for candidate in workspace_candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Some(path) = find_on_path(executable_name()) {
        return Ok(path);
    }

    bail!(
        "llama-server not found. Set MYELIN_LLAMA_SERVER_PATH, add executablePath to {}, or put {} on PATH.",
        CONFIG_FILE_PATH,
        executable_name()
    )
}

fn resolve_model_path(
    workspace: &Path,
    workspace_config: &WorkspaceLlamaConfig,
) -> Result<PathBuf> {
    if let Ok(path) = env::var("MYELIN_LLAMA_MODEL_PATH") {
        return validate_existing_model(resolve_input_path(workspace, &path));
    }

    if let Some(path) = &workspace_config.model_path {
        return validate_existing_model(resolve_input_path(workspace, path));
    }

    let models = discover_gguf_models(workspace)?;
    match models.as_slice() {
        [] => bail!(
            "no .gguf model found. Set MYELIN_LLAMA_MODEL_PATH, add modelPath to {}, or place a single .gguf file in the workspace.",
            CONFIG_FILE_PATH
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
                    "multiple .gguf models found. Add modelPath to {} to choose one explicitly.",
                    CONFIG_FILE_PATH
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

fn find_free_port() -> u16 {
    TcpListener::bind((DEFAULT_HOST, 0))
        .ok()
        .and_then(|listener| listener.local_addr().ok().map(|address| address.port()))
        .unwrap_or(39281)
}
