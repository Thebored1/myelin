//! openharn-myelin — the openharn agent harness served as a sidecar for the
//! Myelin desktop app.
//!
//! Myelin's Rust backend spawns this binary, points it at the local
//! `llama-server`, and drives it over HTTP: it POSTs the conversation + Myelin's
//! own tool schemas to `/v1/chat/stream`, streams back assistant text and the
//! live note body, and — crucially — runs Myelin's REAL tools itself (note
//! store, RAG, web) when the loop emits a `tool` event, posting each result to
//! `/v1/tool-result`. This keeps openharn's reliability loop (tool-call text
//! recovery, context-fit, circuit breaker, strict grammar, prompt-tools) while
//! the tools stay in Myelin's process where AppState lives.
//!
//! Listen port: --port N, or OPENHARN_MYELIN_PORT, default 8091.

mod agent;
mod harness;
mod server;

use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let mut port: u16 = std::env::var("OPENHARN_MYELIN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8091);

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if let Some(p) = args.get(i + 1).and_then(|v| v.parse().ok()) {
                    port = p;
                }
                i += 2;
            }
            other => {
                if let Some(p) = other.strip_prefix("--port=").and_then(|v| v.parse().ok()) {
                    port = p;
                }
                i += 1;
            }
        }
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[openharn-myelin] failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    // A single line on stdout so the host (Myelin) can confirm readiness.
    println!("openharn-myelin listening on http://{addr}");
    println!("  POST /v1/chat/stream   POST /v1/tool-result   GET /health");

    if let Err(e) = axum::serve(listener, server::router()).await {
        eprintln!("[openharn-myelin] server error: {e}");
        std::process::exit(1);
    }
}
