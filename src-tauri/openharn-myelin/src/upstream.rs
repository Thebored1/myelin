//! Upstream HTTP client and model-request diagnostics.

use crate::protocol::Out;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;

pub(super) fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(2)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
            .build()
            .expect("valid upstream HTTP client")
    })
}

pub(super) async fn emit_prompt(tx: &mpsc::Sender<Out>, stage: &str, body: &Value) {
    let messages = body.get("messages").cloned().unwrap_or(Value::Null);
    let rendered = serde_json::to_string_pretty(&messages).unwrap_or_else(|_| messages.to_string());
    let chars = rendered.chars().count();
    let approx_tokens = (chars + 3) / 4;
    let _ = tx
        .send(Out::Debug {
            kind: "model_prompt".into(),
            message: format!(
                "{stage}\nprompt_chars={chars} approx_tokens={approx_tokens}"
            ),
        })
        .await;
}

pub(super) fn request_body(
    model: &str,
    messages: Value,
    temperature: f64,
    max_tokens: u32,
    external: bool,
    slot_id: i32,
) -> Value {
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !external {
        body["cache_prompt"] = serde_json::json!(true);
        body["id_slot"] = serde_json::json!(slot_id);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::{emit_prompt, request_body};
    use crate::protocol::Out;
    use serde_json::json;
    use tokio::sync::mpsc;

    #[test]
    fn external_requests_omit_llama_specific_cache_fields() {
        let body = request_body("external", serde_json::json!([]), 0.2, 128, true, 7);
        assert!(body.get("cache_prompt").is_none());
        assert!(body.get("id_slot").is_none());
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[tokio::test]
    async fn prompt_diagnostics_expose_shape_but_not_note_content() {
        let (tx, mut rx) = mpsc::channel(1);
        emit_prompt(
            &tx,
            "stage",
            &json!({"messages": [{"role": "user", "content": "private note body"}]}),
        )
        .await;
        let Some(Out::Debug { message, .. }) = rx.recv().await else {
            panic!("expected prompt diagnostics")
        };
        assert!(message.contains("prompt_chars="));
        assert!(!message.contains("private note body"));
    }
}
