use anyhow::{bail, Context, Result};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::{apply_library_path, resolve_embedding_executable};

pub struct ManagedRerankerServer {
    pub child: Child,
    pub model_path: PathBuf,
    pub executable_path: PathBuf,
    pub context_tokens: usize,
}

impl Drop for ManagedRerankerServer {
    fn drop(&mut self) { let _ = self.child.kill(); }
}

/// Rerankers use rank pooling and must never share the embedding server: rank
/// pooling changes the output from dense vectors to a scalar relevance score.
pub async fn start_reranker_server(
    client: &Client, executable: &Path, model_path: &Path, host: &str, port: u16, context_tokens: usize,
) -> Result<ManagedRerankerServer> {
    let mut command = Command::new(executable);
    command.arg("--host").arg(host).arg("--port").arg(port.to_string())
        .arg("--model").arg(model_path).arg("--reranking")
        .arg("--ctx-size").arg(context_tokens.clamp(128, 8192).to_string())
        .arg("--batch-size").arg("2048").arg("--ubatch-size").arg(context_tokens.min(2048).to_string())
        .arg("--parallel").arg("1").arg("--no-warmup").stdout(Stdio::null()).stderr(Stdio::null());
    apply_library_path(&mut command, executable);
    #[cfg(target_os = "linux")]
    unsafe { use std::os::unix::process::CommandExt; command.pre_exec(|| { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL); Ok(()) }); }
    let mut child = command.spawn().with_context(|| format!("failed to spawn reranker server: {}", executable.display()))?;
    let base = format!("http://{host}:{port}");
    for _ in 0..80 {
        if client.get(format!("{base}/health")).send().await.map(|r| r.status().is_success()).unwrap_or(false) {
            return Ok(ManagedRerankerServer { child, model_path: model_path.to_path_buf(), executable_path: executable.to_path_buf(), context_tokens });
        }
        if let Ok(Some(status)) = child.try_wait() { bail!("reranker server exited early ({status}) while loading {}", model_path.display()); }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = child.kill();
    bail!("reranker server did not become healthy while loading {}", model_path.display())
}

pub async fn stop_reranker_server(server: &mut ManagedRerankerServer) { let _ = server.child.kill(); let _ = server.child.wait(); }

/// Reranking is validated and served by the same stock llama.cpp executable
/// family as embeddings. It never uses the chat/BeeLlama executable.
pub fn resolve_reranker_executable(app_data_dir: &Path) -> Result<PathBuf> { resolve_embedding_executable(app_data_dir) }
