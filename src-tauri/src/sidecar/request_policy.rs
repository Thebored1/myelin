//! Host-side request policy for the sidecar boundary.

use crate::llama_server::ResolvedLlamaConfig;
use crate::state::OpenharnSettings;
use serde_json::{json, Value};

pub(super) struct RequestPolicy {
    pub(super) llama_base: String,
    pub(super) model: String,
    pub(super) api_key: Option<String>,
    pub(super) options: Value,
    pub(super) max_output_tokens: u32,
    pub(super) pass_kind: &'static str,
    pub(super) tool_mode: &'static str,
}

/// Build the complete sidecar policy in one place so routing, model settings,
/// and external-endpoint behavior cannot drift between call sites.
pub(super) fn build(
    settings: &OpenharnSettings,
    config: &ResolvedLlamaConfig,
    intent_is_tool: Option<bool>,
    chat_mode: bool,
    suppress_chat_output: bool,
    selection_scoped: bool,
    targeted_write: bool,
) -> RequestPolicy {
    let external = settings.external_ready();
    let llama_base = if external {
        settings
            .external_base_url
            .clone()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string()
    } else {
        settings
            .base_url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| format!("{}/v1", config.base_url()))
    };
    let model = if external {
        settings.external_model.clone().unwrap_or_default()
    } else {
        config.model_name()
    };
    let api_key = if external {
        settings.external_api_key.clone()
    } else {
        None
    };

    let tool_mode = match settings.tool_mode.trim().to_ascii_lowercase().as_str() {
        "native" => "native",
        "prompt" | "prompt_tools" => "prompt",
        _ => "auto",
    };
    let explicit_prompt = tool_mode == "prompt";
    let use_prompt_tools = explicit_prompt;
    let use_strict = explicit_prompt && (settings.strict || suppress_chat_output);
    let call_only = suppress_chat_output
        || (explicit_prompt && settings.call_only && intent_is_tool == Some(true));

    let mut options = json!({
        "strict": use_strict,
        "prompt_tools": use_prompt_tools,
        "prefers_prompt_tools": false,
        "call_only": call_only,
        "friendly_results": intent_is_tool.is_some(),
        "no_think": settings.no_think || chat_mode,
        "no_think_prefill": chat_mode || (config.thinking && settings.no_think),
        "narrow": false,
        "slm": false,
        "chat_mode": chat_mode,
        "selection_scoped": selection_scoped,
        "targeted_write": targeted_write,
        "native_first": !explicit_prompt,
        "external": external,
    });
    if let Some(is_tool) = intent_is_tool {
        options["intent_is_tool"] = json!(is_tool);
    }
    options["max_calls"] = json!(if intent_is_tool == Some(true) {
        1
    } else {
        settings
            .max_calls
            .unwrap_or(if tool_mode == "prompt" { 3 } else { 1 })
    });
    if let Some(value) = settings.total_max {
        options["total_max"] = json!(value);
    }
    if let Some(value) = settings.tool_timeout_secs {
        options["tool_timeout_secs"] = json!(value);
    }
    if let Some(value) = settings.generation_timeout_secs {
        options["generation_timeout_secs"] = json!(value);
    }
    if let Some(value) = settings
        .tool_choice
        .as_ref()
        .filter(|v| !v.trim().is_empty())
    {
        options["tool_choice"] = json!(value.trim());
    }
    if let Some(value) = settings
        .template_kwargs
        .as_ref()
        .filter(|v| !v.trim().is_empty())
    {
        options["template_kwargs"] = json!(value);
    }

    RequestPolicy {
        llama_base,
        model,
        api_key,
        options,
        max_output_tokens: if chat_mode && intent_is_tool != Some(true) {
            768
        } else {
            4096
        },
        pass_kind: if intent_is_tool == Some(true) {
            "tool selection"
        } else {
            "direct answer"
        },
        tool_mode,
    }
}
