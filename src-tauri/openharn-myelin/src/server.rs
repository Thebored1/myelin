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

use crate::protocol;
use crate::protocol::{ChatRequest, Out};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

pub const PROTOCOL_VERSION: u32 = protocol::VERSION;

/// Registry of tool calls awaiting a result, keyed by `"{request_id}:{call_id}"`.
pub type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

/// Per-request cancellation flags, keyed by request id. A request is cancelled
/// by sending `true` on its watch channel; the loop observes the change.
pub type Cancels = Arc<Mutex<HashMap<String, watch::Sender<bool>>>>;

#[derive(Clone)]
pub struct AppState {
    pub pending: Pending,
    pub cancels: Cancels,
    pub token: Option<String>,
}

pub fn router() -> Router {
    router_with_token(std::env::var("OPENHARN_MYELIN_TOKEN").ok())
}

pub fn router_with_token(token: Option<String>) -> Router {
    let state = AppState {
        pending: Arc::new(Mutex::new(HashMap::new())),
        cancels: Arc::new(Mutex::new(HashMap::new())),
        token,
    };
    Router::new()
        .route("/health", get(health))
        .route("/v1/chat/stream", post(chat_stream))
        .route("/v1/tool-result", post(tool_result))
        .route("/v1/cancel", post(cancel))
        .with_state(state)
}

fn authorized(headers: &HeaderMap, state: &AppState) -> bool {
    let Some(expected) = state.token.as_deref() else {
        return true;
    };
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.strip_prefix("Bearer ") == Some(expected))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": "sidecar authorization required" })),
    )
        .into_response()
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
    headers: HeaderMap,
    Json(body): Json<ToolResultBody>,
) -> Response {
    if !authorized(&headers, &state) {
        return unauthorized();
    }
    let key = format!("{}:{}", body.request_id, body.tool_call_id);
    let sender = state.pending.lock().await.remove(&key);
    match sender {
        Some(tx) => {
            let _ = tx.send(body.result);
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "no pending tool call for that id" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct CancelBody {
    request_id: String,
}

async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CancelBody>,
) -> Response {
    if !authorized(&headers, &state) {
        return unauthorized();
    }
    if let Some(tx) = state.cancels.lock().await.get(&body.request_id) {
        let _ = tx.send(true);
    }
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn chat_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut req): Json<ChatRequest>,
) -> Response {
    if !authorized(&headers, &state) {
        return unauthorized();
    }
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
    state
        .cancels
        .lock()
        .await
        .insert(request_id.clone(), cancel_tx);
    let cancels = state.cancels.clone();
    tokio::spawn(async move {
        crate::runner_entry::run(req, tx, pending, cancel_rx).await;
        cancels.lock().await.remove(&request_id);
    });

    let stream = ReceiverStream::new(rx).map(|out| Ok::<Event, Infallible>(to_event(out)));
    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

fn to_event(out: Out) -> Event {
    let (name, data): (&str, Value) = match out {
        Out::ChatChunk(delta) => (protocol::CHAT_CHUNK, json!({ "delta": delta })),
        Out::NoteStart => (protocol::NOTE_START, json!({})),
        Out::NoteDelta(delta) => (protocol::NOTE_DELTA, json!({ "delta": delta })),
        Out::NoteCancel => (protocol::NOTE_CANCEL, json!({})),
        Out::Tool {
            id,
            name,
            arguments,
        } => (
            protocol::TOOL,
            json!({ "id": id, "name": name, "arguments": arguments }),
        ),
        Out::ToolResult { id, name, result } => (
            protocol::TOOL_RESULT,
            json!({ "id": id, "name": name, "result": result }),
        ),
        Out::Done {
            messages,
            new_messages,
            last_tool,
        } => (
            protocol::DONE,
            json!({ "messages": messages, "new_messages": new_messages, "last_tool": last_tool }),
        ),
        Out::Error(message) => (protocol::ERROR, json!({ "message": message })),
        Out::Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            evaluated_tokens,
            cache_reuse_ratio,
        } => (
            protocol::USAGE,
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
        Out::Debug { kind, message } => {
            (protocol::DEBUG, json!({ "kind": kind, "message": message }))
        }
    };
    Event::default().event(name).data(data.to_string())
}

#[cfg(test)]
mod tests {
    use super::{authorized, AppState};
    use axum::http::{header::AUTHORIZATION, HeaderMap, HeaderValue};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn state(token: Option<&str>) -> AppState {
        AppState {
            pending: Arc::new(Mutex::new(HashMap::new())),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            token: token.map(str::to_owned),
        }
    }

    #[test]
    fn token_auth_is_required_only_when_configured() {
        let open = state(None);
        assert!(authorized(&HeaderMap::new(), &open));

        let protected = state(Some("secret"));
        assert!(!authorized(&HeaderMap::new(), &protected));
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer wrong"));
        assert!(!authorized(&headers, &protected));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        assert!(authorized(&headers, &protected));
    }
}
