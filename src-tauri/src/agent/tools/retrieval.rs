use super::*;
use crate::state::AppState;
use futures_util::StreamExt;
use rig_core::client::CompletionClient;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;
#[derive(Deserialize, JsonSchema)]
pub struct SearchNotesArgs {
    pub(super) query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct FetchWebPageArgs {
    url: String,
}

#[derive(Clone)]
pub struct FetchWebPageTool {
    pub state: AppState,
}

impl Tool for FetchWebPageTool {
    const NAME: &'static str = "fetch_web_page";

    type Error = ToolError;
    type Args = FetchWebPageArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("fetch_web_page").expect("fetch_web_page contract");
        ToolDefinition {
            name: "fetch_web_page".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let url = normalize_web_url(&args.url).map_err(|message| ToolError { message })?;
        // SSRF guard: never fetch loopback/private/link-local addresses (the
        // user's own machine or LAN), checked again on every redirect hop.
        let resolved_addr = crate::web_search::resolve_public_url(&url)
            .await
            .map_err(|message| ToolError { message })?;
        self.state.record_chat_tool("Fetch Web Page", url.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Fetch Web Page", "details": url }),
        );

        // Pin the initial hostname to the already-validated public address and
        // disable redirects. Following redirects would require an async DNS
        // validation step for every hop; refusing them keeps the guarantee
        // between validation and connection establishment explicit.
        let mut client_builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(6))
            .timeout(std::time::Duration::from_secs(20))
            .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(addr) = resolved_addr {
            if let Some(host) = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_string)) {
                client_builder = client_builder.resolve(&host, addr);
            }
        }
        let client = client_builder.build()
            .map_err(|e| ToolError {
                message: format!("Failed to build web client: {e}"),
            })?;

        let response = client
            .get(&url)
            .header(
                reqwest::header::USER_AGENT,
                "Myelin/0.1 local notes web fetcher",
            )
            .send()
            .await
            .map_err(|e| ToolError {
                message: format!("Failed to fetch {url}: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(ToolError {
                message: format!("Failed to fetch {url}: HTTP {status}"),
            });
        }

        // Cap the raw body read so a huge page cannot fill memory before the
        // 6,000-character text excerpt is extracted below.
        let mut body_bytes: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while body_bytes.len() < WEB_BODY_CAP {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    let take = (WEB_BODY_CAP - body_bytes.len()).min(chunk.len());
                    body_bytes.extend_from_slice(&chunk[..take]);
                }
                Some(Err(e)) => {
                    return Err(ToolError {
                        message: format!("Failed to read response from {url}: {e}"),
                    })
                }
                None => break,
            }
        }
        let body = String::from_utf8_lossy(&body_bytes);
        let text = html_to_text(&body);
        if text.trim().is_empty() {
            Ok(format!("Fetched {url}, but no readable text was found."))
        } else {
            Ok(text.chars().take(WEB_FETCH_LIMIT).collect())
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchDocumentsArgs {
    query: String,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    doc_id: Option<String>,
}

#[derive(Clone)]
pub struct SearchDocumentsTool {
    pub state: AppState,
}

impl Tool for SearchDocumentsTool {
    const NAME: &'static str = "search_documents";

    type Error = ToolError;
    type Args = SearchDocumentsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("search_documents").expect("search_documents contract");
        ToolDefinition {
            name: "search_documents".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let k = args.count.unwrap_or(5).clamp(1, 10) as usize;
        let scope = self.state.current_attachment_scope();
        if scope.is_empty() {
            return Ok("Document search is unavailable because no document is authorized for the active AI workspace.".to_string());
        }
        let scoped_ids = match args.doc_id {
            Some(id) if scope.contains(&id) => vec![id],
            Some(_) => {
                return Ok("That document is outside the active note and attachment scope.".to_string())
            }
            None => scope,
        };
        self.state
            .record_chat_tool("Search Documents", args.query.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Search Documents", "details": args.query.clone() }),
        );
        match self
            .state
            .retrieve_chunks_scoped(&args.query, k, Some(&scoped_ids))
            .await
        {
            Ok(chunks) if !chunks.is_empty() => {
                let mut out = format!("Passages from your documents for \"{}\":\n\n", args.query);
                for (i, c) in chunks.iter().enumerate() {
                    out.push_str(&format!("{}. [{}]\n{}\n\n", i + 1, c.source, c.text.trim()));
                }
                Ok(out)
            }
            Ok(_) => Ok("No relevant passages found in your documents.".to_string()),
            Err(e) => Ok(format!("Document search failed: {e}")),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct FindInNoteArgs {
    query: String,
}

#[derive(Clone)]
pub struct FindInNoteTool {
    pub state: AppState,
}

impl Tool for FindInNoteTool {
    const NAME: &'static str = "find_in_note";

    type Error = ToolError;
    type Args = FindInNoteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("find_in_note").expect("find_in_note contract");
        ToolDefinition {
            name: "find_in_note".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let q = args.query.trim().to_string();
        self.state.record_chat_tool("Find in Note", q.clone());
        let _ = self.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Find in Note", "details": q.clone() }),
        );
        if q.is_empty() {
            return Ok("No search term was given.".to_string());
        }
        let body = self.state.open_note_body().unwrap_or_default();
        // Whole-word (boundary) match, not substring: "fix" must not hit
        // "prefix" and "add" must not hit "address".
        let pattern = format!(r"(?i)\b{}\b", regex::escape(&q));
        let count = regex::Regex::new(&pattern)
            .map(|re| re.find_iter(&body).count())
            .unwrap_or_else(|_| 0);
        if count == 0 {
            Ok(format!(
                "The text \"{q}\" does NOT appear in the open note."
            ))
        } else {
            Ok(format!(
                "Yes — \"{q}\" appears {count} time(s) in the open note."
            ))
        }
    }
}
