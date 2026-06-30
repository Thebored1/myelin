//! One-time per-model probe: does this model's chat template actually handle
//! tool calls? The profile registry answers it for known models; for unknown
//! ones we fire a single tiny tools request, cache the verdict by a cheap file
//! fingerprint, and fall back to chat-only when the template can't do tools
//! (rather than erroring every turn — the failure we saw with broken templates).

use reqwest::Client;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

const CACHE_FILE: &str = "tool-capability.json";

/// Cheap, content-free fingerprint: path + size + mtime. Changes if the file is
/// swapped/re-quantized, which correctly forces a re-probe.
pub fn fingerprint(model_path: &Path) -> String {
    let meta = std::fs::metadata(model_path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}|{}|{}", model_path.display(), size, mtime)
}

fn load(app_data_dir: &Path) -> HashMap<String, bool> {
    std::fs::read_to_string(app_data_dir.join(CACHE_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn store(app_data_dir: &Path, fp: &str, supported: bool) {
    let mut map = load(app_data_dir);
    map.insert(fp.to_string(), supported);
    if let Ok(raw) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(app_data_dir.join(CACHE_FILE), raw);
    }
}

/// Fire one tiny tools request; `true` if the server accepts tools without
/// erroring (a broken template returns HTTP 500 — exactly what this guards).
async fn probe(client: &Client, base_url: &str, model: &str) -> bool {
    let body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Call the ping tool." }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "ping",
                "description": "Replies pong.",
                "parameters": { "type": "object", "properties": {} }
            }
        }],
        "tool_choice": "auto",
        "max_tokens": 32,
        "stream": false
    });
    match client
        .post(format!("{base_url}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Effective tool support for the current model: the profile's answer when
/// known, else the cached probe result, else probe once and cache it.
pub async fn supports_tools(
    client: &Client,
    base_url: &str,
    app_data_dir: &Path,
    model_path: &Path,
    model: &str,
    profile_says: Option<bool>,
) -> bool {
    if let Some(known) = profile_says {
        return known;
    }
    let fp = fingerprint(model_path);
    if let Some(cached) = load(app_data_dir).get(&fp).copied() {
        return cached;
    }
    let result = probe(client, base_url, model).await;
    store(app_data_dir, &fp, result);
    log::info!("tool-capability probe for {model}: supports_tools={result}");
    result
}
