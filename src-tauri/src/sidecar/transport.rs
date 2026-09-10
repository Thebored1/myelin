use super::SIDECAR_PROTOCOL_VERSION;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

pub(super) fn http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::new)
}

pub(super) async fn compatible_health(base: &str, token: Option<&str>) -> bool {
    let mut request = http_client()
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(1));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let Ok(response) = request.send().await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<Value>()
        .await
        .ok()
        .and_then(|health| health["protocol_version"].as_u64())
        == Some(SIDECAR_PROTOCOL_VERSION)
}
