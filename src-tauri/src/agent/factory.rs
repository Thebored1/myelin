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
pub fn build_myelin_agent(
    state: AppState,
    base_url: &str,
    model_name: &str,
    preamble: &str,
    temperature: f64,
    max_turns: usize,
) -> rig_core::agent::Agent<impl rig_core::completion::CompletionModel> {
    let client = rig_core::providers::openai::Client::builder()
        .api_key("sk-fake")
        .base_url(base_url)
        .build()
        .expect("Failed to initialize rig client")
        .completions_api();
    let model = client.completion_model(model_name);
    rig_core::agent::AgentBuilder::new(model)
        .preamble(preamble)
        // Low temperature keeps the model decisive and on-task instead of
        // rambling or asking the same clarifying question repeatedly.
        .temperature(temperature)
        .default_max_turns(max_turns)
        .tool(ReadNoteTool {
            state: state.clone(),
        })
        .tool(WriteNoteTool {
            state: state.clone(),
        })
        .tool(AppendNoteTool {
            state: state.clone(),
        })
        .tool(PrependNoteTool {
            state: state.clone(),
        })
        .tool(ReplaceInNoteTool {
            state: state.clone(),
        })
        .tool(InsertAfterLineTool {
            state: state.clone(),
        })
        .tool(DeleteInNoteTool {
            state: state.clone(),
        })
        .tool(FormatNoteTool {
            state: state.clone(),
        })
        .tool(FetchWebPageTool {
            state: state.clone(),
        })
        .tool(WebSearchTool {
            state: state.clone(),
        })
        .tool(SearchDocumentsTool {
            state: state.clone(),
        })
        .tool(FindInNoteTool {
            state: state.clone(),
        })
        .tool(SearchNotesTool {
            state: state.clone(),
        })
        .tool(EditNotebookTool { state })
        .build()
}

