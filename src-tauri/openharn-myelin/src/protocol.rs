//! Wire-level names kept separate from sidecar execution policy.

pub(crate) const VERSION: u32 = 4;
pub(crate) const CHAT_CHUNK: &str = "chat_chunk";
pub(crate) const NOTE_START: &str = "note_start";
pub(crate) const NOTE_DELTA: &str = "note_delta";
pub(crate) const NOTE_CANCEL: &str = "note_cancel";
pub(crate) const TOOL: &str = "tool";
pub(crate) const TOOL_RESULT: &str = "tool_result";
pub(crate) const DONE: &str = "done";
pub(crate) const ERROR: &str = "error";
pub(crate) const USAGE: &str = "usage";
pub(crate) const DEBUG: &str = "debug";

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Clone)]
pub struct Options {
    /// Constrain tool calls with a GBNF grammar (OPENHARN_STRICT_TOOLS). Most
    /// reliable for models that emit malformed JSON. Implies prompt_tools.
    #[serde(default)]
    pub strict: bool,
    /// Describe tools in the prompt and omit the native `tools` field
    /// (OPENHARN_PROMPT_TOOLS) — for servers without native function calling.
    #[serde(default)]
    pub prompt_tools: bool,
    /// Strip reasoning output (and, when enabled below, prime a closed think
    /// turn) so the model returns only its answer. Can't combine with strict.
    #[serde(default)]
    pub no_think: bool,
    /// Whether `no_think` should also inject the legacy closed-think assistant
    /// prefill. Some managed model templates still enter a thinking turn even
    /// with reasoning disabled, so the host enables this for ordinary Chat;
    /// default true preserves the standalone sidecar's prior behavior.
    #[serde(default = "default_true")]
    pub no_think_prefill: bool,
    /// Per-turn circuit-breaker limit on tool calls (OPENHARN_MAX_CALLS).
    #[serde(default = "default_max_calls")]
    pub max_calls: usize,
    /// Total tool calls across all turns before tools are removed (OPENHARN_TOTAL_MAX).
    #[serde(default = "default_total_max")]
    pub total_max: usize,
    /// Seconds to wait for a tool result from the host before failing the call.
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: u64,
    /// Maximum idle time between upstream llama-server stream chunks.
    #[serde(default = "default_generation_timeout")]
    pub generation_timeout_secs: u64,
    /// Read-only navigation preset (OPENHARN_NARROW): strict + prompt-tools +
    /// restrict to read/search tools only. Forces a safe, non-mutating agent.
    #[serde(default)]
    pub narrow: bool,
    /// Restrict to a named subset of tools (OPENHARN_TOOLS, comma-separated).
    /// Tools not present in the request's schema are ignored.
    #[serde(default)]
    pub tool_subset: Vec<String>,
    /// Compact structured-observation harness (OPENHARN_SLM): smaller, tighter
    /// tool-result caps so weak models don't drown in observation text.
    #[serde(default)]
    pub slm: bool,
    /// Force `tool_choice` in native FC mode: "auto" (default), "required"
    /// (llama.cpp grammar-forces a call in the model's OWN native format),
    /// "none", or a specific tool name. Works only when NOT using prompt_tools
    /// (native tool-calling must be active). From openharn DSGoal research:
    /// tool_choice=required + enable_thinking:false recovers ~71% of quant
    /// degradation (MiniCPM-V Q4_0: 47.5% -> 72.5%).
    #[serde(default)]
    pub tool_choice: Option<String>,
    /// Raw JSON forwarded as `chat_template_kwargs` into the model's chat
    /// template (a llama.cpp passthrough). The canonical use is
    /// `{"enable_thinking":false}` — templates that support the switch render
    /// a CLOSED think block so generation starts at the answer/call; templates
    /// that don't simply ignore it. Pairs with tool_choice=required: a thinking
    /// model otherwise burns its budget reasoning under the forced call grammar
    /// and returns nothing.
    #[serde(default)]
    pub template_kwargs: Option<String>,
    /// Model-based TOOL/CHAT classification: the sidecar asks the model to
    /// classify the user's latest turn as "TOOL" or "CHAT" (1 word). A CHAT
    /// turn (greetings, questions) skips tools and answers directly; a TOOL
    /// turn enters the tool loop with the full toolset and the model's native
    /// FC format (with tool_choice=required set by the host for weak models).
    /// Requires `prompt_tools`. Set automatically by the host for
    /// `prefersPromptTools` models.
    #[serde(default)]
    pub friendly_results: bool,
    /// When `friendly_results` classifies a turn as TOOL and `strict` is on,
    /// force the call-only grammar (root ::= call, no text alternative) so a
    /// weak model MUST output a tool call instead of answering in prose.
    #[serde(default)]
    pub call_only: bool,
    /// Pre-computed TOOL/CHAT result supplied by the host. When absent, the
    /// sidecar runs model-based intent detection.
    #[serde(default)]
    pub intent_is_tool: Option<bool>,
    /// The user selected Chat mode. Read-only lookup tools may be used, but once
    /// one returns the next turn must be a prose answer rather than another tool
    /// planning pass.
    #[serde(default)]
    pub chat_mode: bool,
    /// True when the host has an armed editor selection. Selection-scoped edits
    /// must not be shown as speculative whole-note previews.
    #[serde(default)]
    pub selection_scoped: bool,
    /// Model-profile hint: the model's native FC is unreliable at low quants
    /// and benefits from prompt-tools + strict grammar even for single-call
    /// requests (e.g. LFM2 at Q2_K_XL). When true, the per-request policy
    /// uses prompt-tools + strict grammar for ALL tool-bearing requests
    /// (not just multi-call ones).
    #[serde(default)]
    pub prefers_prompt_tools: bool,
    /// Host requests should try the native tool format first and use strict
    /// prompt-tools only as a recovery path.
    #[serde(default)]
    pub native_first: bool,
    /// Focused Write has exactly one target-compatible tool. Prefer native
    /// function calling even when the user's general setting is prompt-tools;
    /// prompt-tools remains the bounded fallback if native decoding fails.
    #[serde(default)]
    pub targeted_write: bool,
    /// Use a plain OpenAI-compatible endpoint. Do not send llama.cpp-specific
    /// slot or prompt-cache fields when this is enabled.
    #[serde(default)]
    pub external: bool,
}

fn default_max_calls() -> usize {
    1
}
fn default_true() -> bool {
    true
}
fn default_total_max() -> usize {
    5
}
fn default_tool_timeout() -> u64 {
    300
}
fn default_generation_timeout() -> u64 {
    120
}

#[derive(Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub request_id: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_turns: Option<usize>,
    pub messages: Vec<Value>,
    #[serde(default)]
    pub tools: Value,
    #[serde(default)]
    pub options: Options,
    #[serde(default)]
    pub session: SessionMetadata,
}

#[derive(Deserialize, Clone)]
pub struct SessionMetadata {
    #[serde(default)]
    pub slot_id: i32,
    #[serde(default)]
    pub epoch: u64,
    #[serde(default = "default_pass_kind")]
    pub pass_kind: String,
}

fn default_pass_kind() -> String {
    "direct answer".into()
}

impl Default for SessionMetadata {
    fn default() -> Self {
        Self {
            slot_id: 0,
            epoch: 0,
            pass_kind: default_pass_kind(),
        }
    }
}

/// Everything the loop can emit to the client (Myelin), mapped 1:1 to SSE events.
#[derive(Clone)]
pub enum Out {
    ChatChunk(String),
    NoteStart,
    NoteDelta(String),
    NoteCancel,
    Tool {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        id: String,
        name: String,
        result: String,
    },
    Done {
        messages: Vec<Value>,
        new_messages: Vec<Value>,
        last_tool: Option<String>,
    },
    Error(String),
    /// Token usage from llama-server's `include_usage` stream option.
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        cached_tokens: u32,
        evaluated_tokens: u32,
        cache_reuse_ratio: f64,
    },
    Debug {
        kind: String,
        message: String,
    },
}

impl Default for Options {
    fn default() -> Self {
        Self {
            strict: false,
            prompt_tools: false,
            no_think: false,
            no_think_prefill: true,
            max_calls: default_max_calls(),
            total_max: default_total_max(),
            tool_timeout_secs: default_tool_timeout(),
            generation_timeout_secs: default_generation_timeout(),
            narrow: false,
            tool_subset: Vec::new(),
            slm: false,
            tool_choice: None,
            template_kwargs: None,
            friendly_results: false,
            call_only: false,
            intent_is_tool: None,
            chat_mode: false,
            selection_scoped: false,
            prefers_prompt_tools: false,
            native_first: false,
            targeted_write: false,
            external: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_and_event_names_are_stable() {
        assert_eq!(VERSION, 4);
        assert_eq!(CHAT_CHUNK, "chat_chunk");
        assert_eq!(DONE, "done");
        assert_eq!(DEBUG, "debug");
    }
}
