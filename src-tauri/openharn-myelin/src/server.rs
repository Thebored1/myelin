//! OpenAI-adjacent HTTP server for the Myelin sidecar.
//!
//!   POST /v1/chat/stream   run the openharn loop; stream SSE events. On a tool
//!                          call the loop BLOCKS and emits a `tool` event; the
//!                          caller (Myelin) runs the real tool and posts the
//!                          result back to unblock it.
//!   POST /v1/tool-result   deliver `{request_id, tool_call_id, result}` to a
//!                          waiting tool call.
//!   POST /v1/cancel        abort a running request: the loop stops streaming
//!                          upstream, aborts any pending tool wait, and emits a
//!                          final `done` so the host can persist the partial
//!                          turn (including already-executed tool calls).
//!   GET  /health           liveness probe.
//!
//! SSE event names: chat_chunk, note_start, note_delta, note_cancel, tool,
//! tool_result, done, error. Each `data:` payload is a JSON object.

use crate::agent::{self, ChatRequest, Out};
use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, Sse},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

pub const PROTOCOL_VERSION: u32 = 4;

/// Registry of tool calls awaiting a result, keyed by `"{request_id}:{call_id}"`.
pub type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

/// Per-request cancellation flags, keyed by request id. A request is cancelled
/// by sending `true` on its watch channel; the loop observes the change.
pub type Cancels = Arc<Mutex<HashMap<String, watch::Sender<bool>>>>;

#[derive(Clone)]
pub struct AppState {
    pub pending: Pending,
    pub cancels: Cancels,
}

pub fn router() -> Router {
    let state = AppState {
        pending: Arc::new(Mutex::new(HashMap::new())),
        cancels: Arc::new(Mutex::new(HashMap::new())),
    };
    Router::new()
        .route("/health", get(health))
        .route("/v1/chat/stream", post(chat_stream))
        .route("/v1/tool-result", post(tool_result))
        .route("/v1/cancel", post(cancel))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "protocol_version": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Deserialize)]
struct ToolResultBody {
    request_id: String,
    tool_call_id: String,
    #[serde(default)]
    result: String,
}

async fn tool_result(
    State(state): State<AppState>,
    Json(body): Json<ToolResultBody>,
) -> impl IntoResponse {
    let key = format!("{}:{}", body.request_id, body.tool_call_id);
    let sender = state.pending.lock().await.remove(&key);
    match sender {
        Some(tx) => {
            let _ = tx.send(body.result);
            (StatusCode::OK, Json(json!({ "ok": true })))
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "no pending tool call for that id" })),
        ),
    }
}

#[derive(Deserialize)]
struct CancelBody {
    request_id: String,
}

async fn cancel(
    State(state): State<AppState>,
    Json(body): Json<CancelBody>,
) -> impl IntoResponse {
    if let Some(tx) = state.cancels.lock().await.get(&body.request_id) {
        let _ = tx.send(true);
    }
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn chat_stream(
    State(state): State<AppState>,
    Json(mut req): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Buffer generously: the loop can outpace the client briefly (e.g. during a
    // fast note stream) without blocking token generation.
    let (tx, rx) = mpsc::channel::<Out>(256);
    let pending = state.pending.clone();

    // The host cancels by request id, so pin the id here and hand the request
    // down with it set (run_loop also defaults, as a safety net).
    let request_id = req
        .request_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    req.request_id = Some(request_id.clone());

    let (cancel_tx, cancel_rx) = watch::channel(false);
    state.cancels.lock().await.insert(request_id.clone(), cancel_tx);
    let cancels = state.cancels.clone();
    tokio::spawn(async move {
        agent::run_loop(req, tx, pending, cancel_rx).await;
        cancels.lock().await.remove(&request_id);
    });

    let stream = ReceiverStream::new(rx).map(|out| Ok(to_event(out)));
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

fn to_event(out: Out) -> Event {
    let (name, data): (&str, Value) = match out {
        Out::ChatChunk(delta) => ("chat_chunk", json!({ "delta": delta })),
        Out::NoteStart => ("note_start", json!({})),
        Out::NoteDelta(delta) => ("note_delta", json!({ "delta": delta })),
        Out::NoteCancel => ("note_cancel", json!({})),
        Out::Tool {
            id,
            name,
            arguments,
        } => (
            "tool",
            json!({ "id": id, "name": name, "arguments": arguments }),
        ),
        Out::ToolResult { id, name, result } => (
            "tool_result",
            json!({ "id": id, "name": name, "result": result }),
        ),
        Out::Done {
            messages,
            new_messages,
            last_tool,
        } => (
            "done",
            json!({ "messages": messages, "new_messages": new_messages, "last_tool": last_tool }),
        ),
        Out::Error(message) => ("error", json!({ "message": message })),
        Out::Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            evaluated_tokens,
            cache_reuse_ratio,
        } => (
            "usage",
            json!({
                "slot_id": 0,
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": total_tokens,
                "cached_tokens": cached_tokens,
                "evaluated_tokens": evaluated_tokens,
                "cache_reuse_ratio": cache_reuse_ratio
            }),
        ),
        Out::Debug { kind, message } => ("debug", json!({ "kind": kind, "message": message })),
    };
    Event::default().event(name).data(data.to_string())
}
