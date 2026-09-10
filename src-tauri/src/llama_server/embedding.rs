use super::*;
use crate::embeddings::EmbeddingPooling;
use anyhow::{bail, Context, Result};
use reqwest::Client;
use std::env;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
pub struct ManagedEmbedServer {
    pub child: Child,
    pub port: u16,
    pub model_path: PathBuf,
    pub executable_path: PathBuf,
    pub contract: Option<crate::embeddings::EmbeddingModelContract>,
    _stderr_reader: Option<thread::JoinHandle<()>>,
}

impl Drop for ManagedEmbedServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Spawn the configured embedding server (currently GTE-small) and wait until
/// it's healthy.
pub async fn start_embed_server(
    client: &Client,
    executable: &Path,
    model_path: &Path,
    host: &str,
    port: u16,
    context_tokens: usize,
    pooling: Option<EmbeddingPooling>,
) -> Result<ManagedEmbedServer> {
    let mut command = Command::new(executable);
    command
        .arg("--host")
        .arg(host)
        .arg("--port")
        .arg(port.to_string())
        .arg("--model")
        .arg(model_path)
        .arg("--embedding")
        .arg("--ctx-size")
        .arg(context_tokens.to_string())
        .arg("--batch-size")
        .arg("1024")
        .arg("--ubatch-size")
        .arg("1024")
        .arg("--parallel")
        .arg("1")
        .arg("--no-warmup")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(pooling) = pooling {
        command.arg("--pooling").arg(pooling.as_server_arg());
    }
    apply_library_path(&mut command, executable);
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn embedding server: {}", executable.display()))?;

    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let reader_handle = child.stderr.take().map(|stderr| {
        let captured = Arc::clone(&captured);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut guard = captured.lock().unwrap();
                if guard.len() == STDERR_CAPTURE_LINES {
                    guard.remove(0);
                }
                guard.push(line);
            }
        })
    });

    let base = format!("http://{host}:{port}");
    for _ in 0..80 {
        if client
            .get(format!("{base}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            log::info!(
                "embedding server ready on port {port}: {} with {}",
                executable.display(),
                model_path.display()
            );
            return Ok(ManagedEmbedServer {
                child,
                port,
                model_path: model_path.to_path_buf(),
                executable_path: executable.to_path_buf(),
                contract: None,
                _stderr_reader: reader_handle,
            });
        }
        if let Ok(Some(status)) = child.try_wait() {
            let detail = captured.lock().unwrap().join(" | ");
            bail!(
                "embedding server {} exited early ({status}) while loading {}{}",
                executable.display(),
                model_path.display(),
                (!detail.is_empty())
                    .then(|| format!(": {detail}"))
                    .unwrap_or_default()
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = child.kill();
    let detail = captured.lock().unwrap().join(" | ");
    bail!(
        "embedding server {} did not become healthy while loading {}{}",
        executable.display(),
        model_path.display(),
        (!detail.is_empty())
            .then(|| format!(": {detail}"))
            .unwrap_or_default()
    )
}

pub async fn stop_embed_server(server: &mut ManagedEmbedServer) {
    let _ = server.child.kill();
    let _ = server.child.wait();
}

/// Embeddings deliberately use stock llama.cpp even when chat uses BeeLlama:
/// the Bee fork is optimized for LFM inference and crashes while loading BERT
/// embedding architectures such as Nomic Embed.
pub fn resolve_embedding_executable(app_data_dir: &Path) -> Result<PathBuf> {
    if let Ok(raw) = env::var("MYELIN_EMBED_SERVER_PATH") {
        return validate_existing_file(
            resolve_input_path(app_data_dir, &raw),
            "embedding llama-server",
        );
    }
    let installed = app_data_dir.join("bin").join("cpu").join(executable_name());
    if installed.is_file() {
        return Ok(installed);
    }
    if let Some(resource) = resource_bin_dir()
        .map(|root| root.join("cpu").join(executable_name()))
        .filter(|path| path.is_file())
    {
        return Ok(resource);
    }
    bail!(
        "stock CPU embedding server is not installed (expected {})",
        installed.display()
    )
}
