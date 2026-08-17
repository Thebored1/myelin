use super::*;
use crate::ai_turn::ToolTurnContext;
use crate::state::AppState;
use rig_core::client::CompletionClient;
pub fn build_myelin_agent(
    state: AppState,
    base_url: &str,
    model_name: &str,
    preamble: &str,
    temperature: f64,
    max_turns: usize,
) -> rig_core::agent::Agent<impl rig_core::completion::CompletionModel> {
    let turn = ToolTurnContext::standalone(state);
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
            turn: turn.clone(),
        })
        .tool(WriteNoteTool {
            turn: turn.clone(),
        })
        .tool(AppendNoteTool {
            turn: turn.clone(),
        })
        .tool(PrependNoteTool {
            turn: turn.clone(),
        })
        .tool(ReplaceInNoteTool {
            turn: turn.clone(),
        })
        .tool(InsertAfterLineTool {
            turn: turn.clone(),
        })
        .tool(DeleteInNoteTool {
            turn: turn.clone(),
        })
        .tool(FormatNoteTool {
            turn: turn.clone(),
        })
        .tool(FetchWebPageTool {
            turn: turn.clone(),
        })
        .tool(WebSearchTool {
            turn: turn.clone(),
        })
        .tool(SearchDocumentsTool {
            turn: turn.clone(),
        })
        .tool(FindInNoteTool {
            turn: turn.clone(),
        })
        .tool(SearchNotesTool {
            turn: turn.clone(),
        })
        .tool(EditNotebookTool { turn })
        .build()
}
