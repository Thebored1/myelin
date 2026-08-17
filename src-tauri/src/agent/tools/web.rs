use super::*;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::Deserialize;
use tauri::Emitter;
#[derive(Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    query: String,
    #[serde(default)]
    count: Option<u32>,
}

#[derive(Clone)]
pub struct WebSearchTool {
    pub turn: ToolTurnContext,
}

impl Tool for WebSearchTool {
    const NAME: &'static str = "web_search";

    type Error = ToolError;
    type Args = WebSearchArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) = tool_contract("web_search").expect("web_search contract");
        ToolDefinition {
            name: "web_search".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let count = args.count.unwrap_or(5).clamp(1, 10) as usize;
        self.turn.record_tool("Web Search", args.query.clone());
        let _ = self.turn.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Web Search", "details": args.query.clone() }),
        );
        let searxng = self.turn.state.searxng_url();
        match crate::web_search::web_search(&args.query, count, searxng.as_deref()).await {
            Ok(results) => Ok(crate::web_search::format_results(&args.query, &results)),
            Err(e) => Ok(format!("Web search failed: {e}")),
        }
    }
}

#[derive(Clone)]
pub struct SearchNotesTool {
    pub turn: ToolTurnContext,
}

impl Tool for SearchNotesTool {
    const NAME: &'static str = "search_notes";

    type Error = ToolError;
    type Args = SearchNotesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (_, description, params) =
            tool_contract("search_notes").expect("search_notes contract");
        ToolDefinition {
            name: "search_notes".to_string(),
            description: description.to_string(),
            parameters: params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.turn.record_tool("Search Notes", args.query.clone());
        let _ = self.turn.state.handle.emit(
            "ai://chat_tool",
            serde_json::json!({ "tool": "Search Notes", "details": args.query }),
        );
        let results = self
            .turn.state
            .search_notes(args.query)
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
        let mut output = String::new();
        for r in results.results.into_iter().take(5) {
            output.push_str(&format!(
                "ID: {} | Title: {}\nSnippet: {}\n\n",
                r.note.id, r.note.title, r.note.excerpt
            ));
        }
        if output.is_empty() {
            Ok("No results found.".to_string())
        } else {
            Ok(output)
        }
    }
}

pub fn normalize_web_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL is required.".to_string());
    }

    let url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.contains(char::is_whitespace)
    {
        return Err(format!("Invalid web URL: {raw}"));
    }

    Ok(url)
}

pub fn html_to_text(raw: &str) -> String {
    let document = scraper::Html::parse_document(raw);
    let preferred = scraper::Selector::parse("article, main, [role=main], .content, #content")
        .expect("static selector");
    let excluded =
        scraper::Selector::parse("script, style, noscript, nav, footer, form, aside, [hidden]")
            .expect("static selector");
    let block = scraper::Selector::parse("h1, h2, h3, h4, h5, h6, p, li, pre, blockquote, tr")
        .expect("static selector");
    let root = document.select(&preferred).next();
    let roots = root.into_iter().collect::<Vec<_>>();
    let roots: Vec<_> = if roots.is_empty() {
        vec![document.root_element()]
    } else {
        roots
    };
    let mut output = Vec::new();
    for root in roots {
        for element in root.select(&block) {
            if element.select(&excluded).next().is_some() {
                continue;
            }
            let text = element.text().collect::<Vec<_>>().join(" ");
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !text.is_empty() {
                output.push(text);
            }
        }
    }
    if output.is_empty() {
        output.extend(
            document
                .root_element()
                .text()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string),
        );
    }
    output.join("\n\n")
}
