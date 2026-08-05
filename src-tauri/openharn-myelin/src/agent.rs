//! The openharn harness loop, made async and streaming for the Myelin sidecar.
//!
//! Same reliability behaviour as `openharn/src/agent.rs::run` — tool-call text
//! recovery, context-fit, per-turn + total call limits, exact-repeat circuit
//! breaker, optional strict grammar / prompt-tools — but instead of executing
//! tools locally it emits a `Tool` event and BLOCKS until Myelin posts the real
//! result back (Myelin runs the actual note-store / RAG / web tools against its
//! own AppState). Assistant text and the live `write_note` body are streamed out
//! as they are generated.

use crate::harness;
use crate::server::Pending;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

const HISTORY_BUDGET: usize = 16_000;

fn normalize_lfm_tool_arguments(messages: &mut [Value], model: &str) {
    if !model.to_ascii_lowercase().contains("lfm2") {
        return;
    }
    for message in messages {
        let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for call in calls {
            let Some(arguments) = call
                .get_mut("function")
                .and_then(|function| function.get_mut("arguments"))
            else {
                continue;
            };
            let Some(raw) = arguments.as_str() else {
                continue;
            };
            if let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(raw) {
                *arguments = parsed;
            }
        }
    }
}

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
    /// prefill. Managed llama-server instances already support reasoning-off
    /// mode, so their host disables this to keep the serialized prompt stable;
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

/// A successful mutating tool call is terminal for this request. Small models
/// often treat the tool result as a prompt to rewrite the same note again; that
/// is both unnecessary and dangerous because the host clears the open-note
/// context as soon as the request completes.
fn is_terminal_mutation(name: &str, result: &str) -> bool {
    if !matches!(
        name,
        "write_note"
            | "append_note"
            | "prepend_note"
            | "replace_in_note"
            | "insert_after_line"
            | "delete_in_note"
            | "format_note"
            | "edit_notebook"
    ) {
        return false;
    }
    let result = result.trim_start().to_ascii_lowercase();
    result.contains("successfully updated") || result.starts_with("notebook cell updated")
}

fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "write_note"
            | "append_note"
            | "prepend_note"
            | "replace_in_note"
            | "insert_after_line"
            | "delete_in_note"
            | "format_note"
            | "edit_notebook"
    )
}

/// Emit the exact message array sent to the model. This deliberately excludes
/// transport credentials; it is a local diagnostic requested by the user and is
/// persisted with the note's debug trace by the Myelin host.
async fn emit_model_prompt(tx: &mpsc::Sender<Out>, stage: &str, body: &Value) {
    let messages = body.get("messages").cloned().unwrap_or(Value::Null);
    let rendered = serde_json::to_string_pretty(&messages).unwrap_or_else(|_| messages.to_string());
    let chars = rendered.chars().count();
    let approx_tokens = (chars + 3) / 4;
    let _ = tx
        .send(Out::Debug {
            kind: "model_prompt".into(),
            message: format!("{stage}\nprompt_chars={chars} approx_tokens={approx_tokens}\n{rendered}"),
        })
        .await;
}

fn upstream_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(2)
            .connect_timeout(Duration::from_secs(10))
            // This is an idle-read timeout, not a total request deadline. A
            // large prompt can legitimately take longer than this before its
            // first token, while a stalled stream still gets released.
            .read_timeout(Duration::from_secs(120))
            .build()
            .expect("valid upstream HTTP client")
    })
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
        }
    }
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

const INTENT_ROUTING_RULES: &str = "Classify the latest user message by calling the classify_intent tool exactly once. TOOL means the user asks for any operation to be performed. Every instruction, command, or action request is TOOL; unless another target is named, it is work on the open note. Operations include creating, writing, adding, editing, revising, formatting, deleting, reading, finding, searching, fetching, browsing, looking up, and researching. CHAT means the user only wants a direct answer, explanation, capability description, greeting, thanks, small talk, opinion, or general knowledge without an operation. Questions are CHAT unless they explicitly ask you to perform an operation. Examples: \"write this on the note\", \"add a poem\", and \"search my notes for Rust\" are TOOL; \"hey\", \"what can you do?\", \"what does this poem mean?\", and \"what is Rust?\" are CHAT.";

/// Ask the same model/session to classify the turn via a virtual tool call. This
/// deliberately uses the live system/note history rather than a standalone
/// completion, preserving the main conversation's KV-cache prefix in the single
/// llama-server slot. The virtual call is consumed locally and never reaches the
/// host or user-visible tool list.
/// Whole-note replacement can safely preview from an empty buffer. Append and
/// insertion requests cannot: until the model finishes declaring their mode, a
/// replacement preview would temporarily hide the existing note.
fn request_allows_replace_preview(user_text: &str) -> bool {
    let text = user_text.to_ascii_lowercase();
    !text.starts_with("add ")
        && ![
            "append",
            " below",
            "add below",
            "add to",
            "insert",
            "add a section",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
}

fn should_stream_note_preview(user_text: &str, selection_scoped: bool) -> bool {
    selection_scoped || request_allows_replace_preview(user_text)
}

async fn detect_intent_in_session(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    model: &str,
    history: &[Value],
    tx: &mpsc::Sender<Out>,
    cancel: &watch::Receiver<bool>,
) -> bool {
    let route_schemas = json!([{
        "type": "function",
        "function": {
            "name": "classify_intent",
            "description": "Classify the latest user message using the routing rules in the system prompt.",
            "parameters": {
                "type": "object",
                "properties": {
                    "intent": {
                        "type": "string",
                        "enum": ["CHAT", "TOOL"],
                        "description": "CHAT for a direct answer; TOOL for an operation."
                    }
                },
                "required": ["intent"]
            }
        }
    }]);

    let mut route_history = history.to_vec();
    if let Some(system) = route_history
        .iter_mut()
        .find(|message| message["role"].as_str() == Some("system"))
    {
        let base = system["content"].as_str().unwrap_or("");
        system["content"] = json!(format!("{base}\n\n{INTENT_ROUTING_RULES}"));
    } else {
        route_history.insert(
            0,
            json!({ "role": "system", "content": INTENT_ROUTING_RULES }),
        );
    }
    let wire = harness::flatten_for_prompt_tools(&route_history, &route_schemas);
    let body = json!({
        "model": model,
        "messages": wire,
        "temperature": 0.0,
        "max_tokens": 32,
        "stream": true,
        "stream_options": { "include_usage": true },
        "cache_prompt": true,
        "id_slot": 0,
        "grammar": harness::tool_grammar(&route_schemas, "call"),
    });
    let _ = tx
        .send(Out::Debug {
            kind: "intent_prompt".into(),
            message: format!("In-session classify_intent tool\n{INTENT_ROUTING_RULES}"),
        })
        .await;
    emit_model_prompt(tx, "INTENT virtual-tool request messages:", &body).await;

    let mut request = client.post(url).json(&body);
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let (content, mut calls, _) = match request.send().await {
        Ok(response) if response.status().is_success() => {
            match stream_upstream(response, tx, false, true, false, &route_schemas, cancel, 120).await {
                Ok(result) => result,
                Err(_) => (String::new(), Vec::new(), false),
            }
        }
        _ => (String::new(), Vec::new(), false),
    };
    if calls.is_empty() {
        calls = harness::parse_text_tool_calls(&content, &route_schemas).unwrap_or_default();
    }
    let intent = calls
        .first()
        .and_then(|call| call["function"]["arguments"].as_str())
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .and_then(|arguments| arguments["intent"].as_str().map(str::to_owned))
        .map(|intent| intent.to_ascii_uppercase());
    // Default to TOOL on ambiguity: losing a genuine note-write is worse than
    // a greeting occasionally entering the tool loop.
    let is_tool = intent.as_deref() != Some("CHAT");
    let raw = intent.unwrap_or_else(|| "(no valid tool call; defaulting to TOOL)".into());
    let _ = tx
        .send(Out::Debug {
            kind: "intent_result".into(),
            message: format!(
                "virtual tool result: {raw}; decision: {}",
                if is_tool { "TOOL" } else { "CHAT" }
            ),
        })
        .await;
    is_tool
}

/// Drive one user request to completion, streaming events on `tx` and requesting
/// tool execution from Myelin via the `pending` registry. `cancel` is the
/// request's cancellation flag: the host posts to `/v1/cancel` to flip it, which
/// aborts the upstream stream and any pending tool wait. On cancel the loop
/// emits a final `done` carrying the partial history so already-executed tool
/// calls are not lost from the conversation.
pub async fn run_loop(
    req: ChatRequest,
    tx: mpsc::Sender<Out>,
    pending: Pending,
    cancel: watch::Receiver<bool>,
) {
    let request_id = req
        .request_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let model = req.model.clone().unwrap_or_else(|| "myelin".to_string());
    let temperature = req.temperature.unwrap_or(0.2);
    // Targeted writes must return a bounded insertion/replacement tool payload.
    // Reusing the normal 4096-token chat budget lets a model that misses the
    // tool protocol spend the whole turn reasoning or reproducing the note,
    // leaving the editor waiting for a terminal tool call.
    let max_tokens = req
        .max_tokens
        .unwrap_or(4096)
        .min(if req.options.targeted_write { 1024 } else { 4096 });
    let max_turns = req.max_turns.unwrap_or(8).max(1);
    let opts = req.options.clone();
    let _ = tx
        .send(Out::Debug {
            kind: "session".into(),
            message: format!(
                "slot_id={} epoch={} pass_kind={}",
                req.session.slot_id, req.session.epoch, req.session.pass_kind
            ),
        })
        .await;

    let url = format!("{}/chat/completions", req.base_url.trim_end_matches('/'));
    // One pooled client is shared by all requests to llama-server. Cloning a
    // reqwest Client is cheap, but using one process-wide instance lets the
    // connection pool survive across chat turns as well as tool-loop turns.
    let client = upstream_client();

    let schemas = if req.tools.is_array() {
        req.tools.clone()
    } else {
        json!([])
    };

    // `narrow` (OPENHARN_NARROW) is a read-only preset: strict + prompt-tools +
    // restricted to non-mutating tools. Mirrors openharn's env-driven preset.
    let narrow = opts.narrow;

    // Effective tool schemas after applying the narrow / subset restrictions.
    let mut effective_schemas = schemas.clone();
    if let Some(arr) = effective_schemas.as_array_mut() {
        // Read-only subset for narrow mode: keep navigation/search tools, drop
        // the mutating ones (write_note / format_note / edit_notebook).
        if narrow {
            arr.retain(|t| {
                let name = t["function"]["name"].as_str().unwrap_or("");
                !matches!(name, "write_note" | "append_note" | "prepend_note" | "replace_in_note" | "insert_after_line" | "delete_in_note" | "format_note" | "edit_notebook")
            });
        }
        // Explicit named subset (OPENHARN_TOOLS): keep only listed tools.
        if !opts.tool_subset.is_empty() {
            let wanted: std::collections::HashSet<String> =
                opts.tool_subset.iter().cloned().collect();
            arr.retain(|t| {
                let name = t["function"]["name"].as_str().unwrap_or("");
                wanted.contains(name)
            });
        }
    }
    let has_tools = effective_schemas
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    let mut history: Vec<Value> = req.messages.clone();

    // Per-request policy (from openharn paper, Table 1 / derive_policy):
    // Decompose the user's request into sub-operations to decide the generation
    // path. This is the key reliability ladder — each mode is used where it is
    // strongest:
    //   plan_len == 0  → no matching tool → abstain immediately (NO_TOOL)
    //   plan_len <= 1  → native FC (model's best mode for one-shot calls, ~80%)
    //   plan_len > 1   → prompt-tools + strict grammar (multi-call recovery)
    let user_text = history
        .iter()
        .rev()
        .find(|m| m["role"].as_str() == Some("user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("");
    let lexical_plan_len = if has_tools {
        harness::harness_decompose(user_text, &effective_schemas).len()
    } else {
        0
    };
    // The host may already have classified this turn deterministically from
    // the live editor state and interaction mode.  In that case lexical
    // decomposition is still useful for choosing the multi-call recovery
    // path, but it must not veto a valid operation merely because compact
    // schemas (or natural language such as "rewrite the introduction") have
    // no keyword overlap with a tool name/description.
    //
    // Keep the fallback at one: the host guarantees that an operation is
    // actionable, while only lexical evidence should request multi-call
    // planning.
    let plan_len = if has_tools
        && opts.intent_is_tool == Some(true)
        && lexical_plan_len == 0
    {
        1
    } else {
        lexical_plan_len
    };

    // strict grammar implies prompt-tools (text-form calls); mirror openharn.
    // Per-request policy: only use prompt-tools + strict for multi-call requests
    // (plan_len > 1). Single-call requests use native FC, which scores ~80% vs
    // 29.5% for forced prompt-tools (paper Table 1).
    //
    // Exception: when the model profile says `prefers_prompt_tools` (e.g. LFM2
    // at Q2_K_XL), native FC is unreliable — the model emits empty/incomplete
    // arguments that llama-server rejects with "could not decode tool: unexpected
    // end of JSON input". In that case, force prompt-tools + strict for ALL
    // tool-bearing requests, not just multi-call ones.
    let stream_note_preview = should_stream_note_preview(user_text, opts.selection_scoped);
    let targeted_native = opts.targeted_write
        && has_tools
        && opts.intent_is_tool == Some(true);
    let mut strict = opts.strict
        || narrow
        || (!opts.native_first && !targeted_native
            && (plan_len > 1 || (opts.prefers_prompt_tools && plan_len <= 1)));
    let mut prompt_tools = !targeted_native && (strict || opts.prompt_tools);
    let mut no_think = opts.no_think && !strict;
    // Call-only keeps tool requests structured until the authoritative write
    // result completes the operation.
    let mut call_only = opts.call_only;
    let mut write_completed = false;
    let mut chat_lookup_completed = false;

    let mut seen_calls: HashSet<String> = HashSet::new();
    let mut budget = HISTORY_BUDGET;
    let mut repeats = 0usize;
    let mut total_calls = 0usize;
    let mut no_tools = !has_tools;
    let mut last_tool: Option<String> = None;
    let mut new_messages: Vec<Value> = Vec::new();


    // Model-based TOOL/CHAT classification (friendly_results mode).
    // Classify the latest user turn before entering the tool loop.
    // CHAT = skip tools, answer directly.
    // TOOL = run tool loop, then generate a friendly summary of results.
    // Independent of prompt_tools now — the relevance gate is a separate
    // concern from the generation path.
    let friendly = opts.friendly_results;
    let intent_is_tool = if friendly {
        if let Some(classified) = opts.intent_is_tool {
            classified
        } else {
            detect_intent_in_session(
                &client,
                &url,
                req.api_key.as_deref(),
                &model,
                &history,
                &tx,
                &cancel,
            )
            .await
        }
    } else {
        true
    };
    if *cancel.borrow() {
        let _ = tx
            .send(Out::Done {
                messages: history.clone(),
                new_messages: Vec::new(),
                last_tool: None,
            })
            .await;
        return;
    }

    // FAST PATH: if the model classified this as TOOL but decomposition found
    // no matching tool, abstain. A CHAT classification must fall through to the
    // prose path below even when the planner finds no tool match (e.g. "gg").
    if has_tools && plan_len == 0 && (!friendly || intent_is_tool) {
        let _ = tx
            .send(Out::Done {
                messages: history.clone(),
                new_messages: Vec::new(),
                last_tool: None,
            })
            .await;
        return;
    }

    // CHAT intent: skip the tool loop, answer directly in prose.
    if friendly && !intent_is_tool {
        // The tool loop trims before every wire build; the CHAT path must do the
        // same so an oversized history (long note + conversation) degrades by
        // dropping oldest turns instead of hard-failing with a context error.
        let mut wire = history.clone();
        harness::fit_context(&mut wire, budget);
        let mut body = json!({
            "model": model,
            "messages": wire,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
            "cache_prompt": true,
            "id_slot": req.session.slot_id,
        });
        if has_tools {
            body["tools"] = effective_schemas.clone();
            body["tool_choice"] = json!("none");
        }
        if no_think && opts.no_think_prefill {
            if let Some(arr) = body["messages"].as_array_mut() {
                arr.push(json!({ "role": "assistant", "content": "<think></think>" }));
            }
        }
        if let Some(kw) = &opts.template_kwargs {
            if let Ok(v) = serde_json::from_str::<Value>(kw) {
                body["chat_template_kwargs"] = v;
            }
        }
        emit_model_prompt(&tx, "CHAT request messages:", &body).await;
        let dispatched_at = Instant::now();
        let resp = match client
            .post(&url)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = tx
                    .send(Out::Error(format!("chat request failed: {e}")))
                    .await;
                return;
            }
        };
        let _ = tx.send(Out::Debug { kind: "response_headers".into(), message: format!("elapsed_ms={} status={}", dispatched_at.elapsed().as_millis(), resp.status()) }).await;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            let _ = tx
                .send(Out::Error(format!("upstream HTTP {status}: {txt}")))
                .await;
            return;
        }
        let (content, _, _) = match stream_upstream_with_timeout(
            resp,
            &tx,
            no_think,
            false,
            false,
            &effective_schemas,
            &cancel,
            opts.generation_timeout_secs,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                if *cancel.borrow() {
                    let _ = tx
                        .send(Out::Done {
                            messages: history.clone(),
                            new_messages,
                            last_tool: None,
                        })
                        .await;
                    return;
                }
                let _ = tx.send(Out::Error(e)).await;
                return;
            }
        };
        let mut h = history.clone();
        h.push(json!({ "role": "assistant", "content": content }));
        let _ = tx
            .send(Out::Done {
                messages: h,
                new_messages: vec![json!({ "role": "assistant", "content": content })],
                last_tool: None,
            })
            .await;
        return;
    }

    for _turn in 0..max_turns {
        if *cancel.borrow() {
            let _ = tx
                .send(Out::Done {
                    messages: history.clone(),
                    new_messages: new_messages.clone(),
                    last_tool: last_tool.clone(),
                })
                .await;
            return;
        }
        harness::fit_context(&mut history, budget);

        let mut wire = if prompt_tools && has_tools {
            harness::flatten_for_prompt_tools(&history, &effective_schemas)
        } else {
            history.clone()
        };
        normalize_lfm_tool_arguments(&mut wire, &model);
        if no_think && opts.no_think_prefill {
            wire.push(json!({ "role": "assistant", "content": "<think></think>" }));
        }
        let mut body = json!({
            "model": model,
            "messages": wire,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
            "cache_prompt": true,
            "id_slot": req.session.slot_id,
        });
        if chat_lookup_completed && !prompt_tools {
            // Keep the identical native schema rendering on the retrieval
            // follow-up. Removing `tools` here changes the fixed prefix and
            // throws away llama.cpp's useful KV cache.
            body["tools"] = effective_schemas.clone();
            body["tool_choice"] = json!("none");
        } else if no_tools {
            // no tools available — text only
        } else if prompt_tools {
            if strict {
                // Call-only grammar for multi-call requests (forces the model
                // to output a call array, not prose). For single-call requests
                // the model uses native FC, so this path is only reached for
                // plan_len > 1 (or explicit call_only override).
                let grammar_root = if !write_completed
                    && (plan_len > 1 || (call_only && friendly && intent_is_tool))
                {
                    "call"
                } else {
                    "call | text"
                };
                body["grammar"] = json!(harness::tool_grammar(&effective_schemas, grammar_root));
            }
        } else {
            body["tools"] = effective_schemas.clone();
            // OPENHARN_TOOL_CHOICE: force tool_choice=required to grammar-force
            // a well-formed call in the model's OWN native format (llama.cpp
            // derives the grammar from the model's chat template). This is the
            // strict-grammar idea without the format transplant: the model keeps
            // its native (multi-call-capable) call syntax but physically cannot
            // emit a malformed one. Rescues quant-degraded native FC.
            //
            // When call_only is active (host says this MUST be a tool call),
            // always force "required" so the model cannot write chat prose
            // (e.g. after a strict-grammar retry where prompt_tools was turned
            // off because the model failed to produce valid JSON under GBNF).
            let choice: Value = if call_only {
                json!("required")
            } else {
                match &opts.tool_choice {
                    Some(c) if c == "required" || c == "none" || c == "auto" => {
                        json!(c)
                    }
                    Some(name) => {
                        json!({
                            "type": "function",
                            "function": { "name": name }
                        })
                    }
                    None => json!("auto"),
                }
            };
            body["tool_choice"] = choice;
        }
        // OPENHARN_TEMPLATE_KWARGS: apply to ALL requests (tool and chat paths)
        // so thinking models skip the think phase when told to. The canonical use
        // is `{"enable_thinking":false}` — templates that support the switch
        // render a CLOSED think block; templates that don't simply ignore it.
        if let Some(kw) = &opts.template_kwargs {
            if let Ok(v) = serde_json::from_str::<Value>(kw) {
                body["chat_template_kwargs"] = v;
            }
        }

        // Show the exact system/history/user prompt after prompt-tools flattening
        // and before sending it to the model.
        let wire_mode = if prompt_tools {
            if opts.native_first { "prompt-recovery" } else { "prompt-explicit" }
        } else {
            "native"
        };
        let _ = tx
            .send(Out::Debug {
                kind: "wire_mode".into(),
                message: wire_mode.into(),
            })
            .await;
        emit_model_prompt(&tx, "AGENT request messages:", &body).await;

        // POST to llama-server, retrying with backoff. The host app starts the
        // server just before calling us, so the first attempt can land while it's
        // still binding — give it a few chances before surfacing an error.
        let resp = {
            let mut attempt = 0u32;
            const MAX_ATTEMPTS: u32 = 6;
            let dispatched_at = Instant::now();
            loop {
                let mut rq = client
                    .post(&url)
                    .json(&body);
                if let Some(k) = &req.api_key {
                    if !k.is_empty() {
                        rq = rq.bearer_auth(k);
                    }
                }
                match rq.send().await {
                    Ok(r) => {
                        let _ = tx.send(Out::Debug { kind: "response_headers".into(), message: format!("elapsed_ms={} status={}", dispatched_at.elapsed().as_millis(), r.status()) }).await;
                        break Some(r)
                    },
                    Err(e) => {
                        attempt += 1;
                        if attempt >= MAX_ATTEMPTS {
                            let _ = tx
                                .send(Out::Error(format!(
                                    "request to llama-server failed after {attempt} attempts: {e}. \
                                     Check the model server is running and reachable at {url}."
                                )))
                                .await;
                            break None;
                        }
                        tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                    }
                }
            }
        };
        let Some(resp) = resp else { return };

        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            // Context overflow: shrink budget and retry this turn instead of dying.
            // If the budget cannot shrink the prompt any further (e.g. the note
            // body in the system message alone exceeds the context), retrying is
            // futile — the retried prompt would be byte-identical. Fail fast
            // rather than burning repeated round-trips.
            if status.as_u16() == 400 && txt.contains("context") && budget > 4_000 {
                budget /= 2;
                let trim_shrank = harness::fit_context(&mut history, budget);
                if !trim_shrank {
                    let _ = tx
                        .send(Out::Error(format!(
                            "upstream HTTP {status}: {txt} (prompt too large to fit the context; shorten the note or conversation and retry)"
                        )))
                        .await;
                    return;
                }
                continue;
            }
            // Tool decoding error: llama-server rejected the model's native FC
            // output (e.g. empty/incomplete arguments → "unexpected end of JSON
            // input"). Retry with prompt-tools + strict grammar, which constrains
            // the model to a flat, schema-valid call format that Q2 models handle
            // reliably. This is the same recovery the native-empty fallback uses,
            // but triggered by an HTTP error rather than an empty response.
            if !prompt_tools
                && (txt.contains("could not decode tool") || txt.contains("unexpected end of JSON"))
            {
                prompt_tools = true;
                strict = true;
                no_think = false;
                continue;
            }
            let _ = tx
                .send(Out::Error(format!("upstream HTTP {status}: {txt}")))
                .await;
            return;
        }

        // Any prompt-tools TOOL request carries the note body as ordinary
        // content. Suppress that JSON from the chat bubble and expose its real
        // content deltas to the note preview instead.
        let suppress_text_call = prompt_tools && friendly && intent_is_tool;
        let (mut content, mut tool_calls, streamed_incomplete_candidate) =
            match stream_upstream_with_timeout(
                resp,
                &tx,
                no_think,
                suppress_text_call,
                stream_note_preview,
                &effective_schemas,
                &cancel,
                opts.generation_timeout_secs,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    if *cancel.borrow() {
                        // Cancelled: persist the partial history (executed tool
                        // calls and their results) so the turn is not lost.
                        let _ = tx
                            .send(Out::Done {
                                messages: history.clone(),
                                new_messages: new_messages.clone(),
                                last_tool: last_tool.clone(),
                            })
                            .await;
                        return;
                    }
                    // Tool decoding error in the stream: llama-server emitted an
                    // error mid-stream (e.g. "could not decode tool: unexpected end
                    // of JSON input"). Retry with prompt-tools + strict grammar.
                    if !prompt_tools
                        && (e.contains("could not decode tool")
                            || e.contains("unexpected end of JSON"))
                    {
                        prompt_tools = true;
                        strict = true;
                        no_think = false;
                        continue;
                    }
                    if stream_note_preview {
                        let _ = tx.send(Out::NoteCancel).await;
                    }
                    let _ = tx.send(Out::Error(e)).await;
                    return;
                }
            };

        // Recover a tool call the server left as plain text.
        if tool_calls.is_empty() && !no_tools {
            if let Some(parsed) = harness::parse_text_tool_calls(&content, &effective_schemas) {
                tool_calls = parsed;
                content.clear();
            }
        }

        // Never turn an explicit operation into a hidden chat completion. The
        // host suppresses chat output for this mode, so a no-tool completion
        // would otherwise look like a successful operation that did nothing.
        if call_only && tool_calls.is_empty() {
            if (opts.native_first || targeted_native) && !prompt_tools {
                // Native-first operation: retry once with the strict flattened
                // format when native FC returned prose, an empty response, or
                // malformed output without a tool call.
                prompt_tools = true;
                strict = true;
                no_think = false;
                continue;
            }
            if stream_note_preview {
                let _ = tx.send(Out::NoteCancel).await;
            }
            let message = if streamed_incomplete_candidate {
                "Generation ended before write_note completed, likely because the context window or output limit was exhausted. Live preview reverted; no changes were saved."
            } else {
                "Operation did not produce a tool call; no changes were made."
            };
            let _ = tx
                .send(Out::Error(message.to_string()))
                .await;
            return;
        }

        // Native-empty fallback: retry only when native FC returns genuinely
        // nothing while tools are still enabled. A normal prose completion such
        // as "Done" must end the turn; retrying it with a forced tool grammar
        // would turn that completion into another write/search loop.
        if !no_tools
            && !prompt_tools
            && tool_calls.is_empty()
            && content.trim().is_empty()
            && has_tools
            && plan_len <= 1
        {
            let fb_wire = harness::flatten_for_prompt_tools(&history, &effective_schemas);
            let mut fb_body = json!({
                "model": model,
                "messages": fb_wire,
                "temperature": temperature,
                "max_tokens": max_tokens,
                "stream": true,
                "stream_options": { "include_usage": true },
                "cache_prompt": true,
                "id_slot": req.session.slot_id,
                "grammar": json!(harness::tool_grammar(&effective_schemas, "call")),
            });
            if let Some(kw) = &opts.template_kwargs {
                if let Ok(v) = serde_json::from_str::<Value>(kw) {
                    fb_body["chat_template_kwargs"] = v;
                }
            }
            emit_model_prompt(&tx, "PROMPT-TOOLS fallback request messages:", &fb_body).await;
            let fb_resp = {
                let mut attempt = 0u32;
                const MAX_ATTEMPTS: u32 = 6;
                let dispatched_at = Instant::now();
                loop {
                    let mut rq = client
                        .post(&url)
                        .json(&fb_body);
                    if let Some(k) = &req.api_key {
                        if !k.is_empty() {
                            rq = rq.bearer_auth(k);
                        }
                    }
                    match rq.send().await {
                        Ok(r) => {
                            let _ = tx.send(Out::Debug { kind: "response_headers".into(), message: format!("elapsed_ms={} status={}", dispatched_at.elapsed().as_millis(), r.status()) }).await;
                            break Some(r)
                        },
                        Err(_e) => {
                            attempt += 1;
                            if attempt >= MAX_ATTEMPTS {
                                break None;
                            }
                            tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                        }
                    }
                }
            };
            if let Some(fb_resp) = fb_resp {
                if fb_resp.status().is_success() {
                    let (fb_content, fb_calls, _) =
                        match stream_upstream_with_timeout(
                            fb_resp,
                            &tx,
                            false,
                            true,
                            stream_note_preview,
                            &effective_schemas,
                            &cancel,
                            opts.generation_timeout_secs,
                        )
                        .await
                        {
                            Ok(v) => v,
                            Err(_) => (String::new(), Vec::new(), false),
                        };
                    if !fb_calls.is_empty() {
                        tool_calls = fb_calls;
                        content = fb_content;
                    } else if !fb_content.is_empty() {
                        if let Some(parsed) =
                            harness::parse_text_tool_calls(&fb_content, &effective_schemas)
                        {
                            if !parsed.is_empty() {
                                tool_calls = parsed;
                                content = String::new();
                            }
                        }
                    }
                }
            }
        }

        if let Err(message) = validate_generated_tool_calls(&tool_calls) {
            if stream_note_preview {
                let _ = tx.send(Out::NoteCancel).await;
            }
            let _ = tx.send(Out::Error(message)).await;
            return;
        }

        // Per-turn grounding: dispatch only the first max_calls.
        let per_turn_excess = if !no_tools && tool_calls.len() > opts.max_calls {
            let excess = tool_calls.len() - opts.max_calls;
            tool_calls.truncate(opts.max_calls);
            Some(excess)
        } else {
            None
        };

        // Record the assistant turn.
        let mut assistant = json!({ "role": "assistant" });
        assistant["content"] = if content.is_empty() {
            Value::Null
        } else {
            json!(content)
        };
        if !tool_calls.is_empty() {
            assistant["tool_calls"] = json!(tool_calls);
        }
        history.push(assistant.clone());
        new_messages.push(assistant);

        if tool_calls.is_empty() {
            let _ = tx
                .send(Out::Done {
                    messages: history.clone(),
                    new_messages: new_messages.clone(),
                    last_tool: last_tool.clone(),
                })
                .await;
            return;
        }

        // Execute each tool via Myelin (HTTP callback) and feed results back.
        for tc in &tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let args_raw = tc["function"]["arguments"]
                .as_str()
                .unwrap_or("{}")
                .to_string();
            let args_val: Value = serde_json::from_str(&args_raw).unwrap_or_else(|_| json!({}));

            let result = if !seen_calls.insert(format!("{name}:{args_val}")) {
                repeats += 1;
                "You already made this exact tool call and saw its result. Repeating it will not change anything. Take a DIFFERENT action, or answer the user with what you know (including telling them something was not found).".to_string()
            } else {
                last_tool = Some(name.clone());
                dispatch_tool(
                    &tx,
                    &pending,
                    &request_id,
                    &id,
                    &name,
                    &args_raw,
                    opts.tool_timeout_secs,
                    &cancel,
                )
                .await
            };

            let cap = if opts.slm {
                harness::TOOL_RESULT_CAP / 3
            } else {
                harness::TOOL_RESULT_CAP
            };
            // A successful mutation is an authoritative completed side effect.
            // Do not ask a weak model for a follow-up turn: it can hallucinate
            // or repeat the whole note instead of acknowledging completion.
            if is_terminal_mutation(&name, &result) {
                write_completed = true;
            }
            let capped = harness::cap_result_with(result.clone(), cap);
            let _ = tx
                .send(Out::ToolResult {
                    id: id.clone(),
                    name: name.clone(),
                    result: capped.clone(),
                })
                .await;

            let tool_message = json!({
                "role": "tool",
                "tool_call_id": id,
                "content": capped,
            });
            history.push(tool_message.clone());
            new_messages.push(tool_message);

            // Operation mode must not retry a failed mutation. The failed call
            // may have been based on a stale selection, and another generation
            // can stream an invalid rewrite into the editor.
            if opts.call_only && is_mutating_tool(&name) && !write_completed {
                let _ = tx.send(Out::Error(result)).await;
                return;
            }

            // max_calls is normally one, but stop even if a malformed model
            // response contained additional calls after a successful mutation.
            if write_completed {
                break;
            }
        }
        total_calls += tool_calls.len();

        if write_completed {
            // The tool result is the source of truth. Finish locally with a
            // concise acknowledgement rather than allowing a second generation
            // that could repeat or alter the newly written note.
            let confirmation = "Done".to_string();
            let _ = tx.send(Out::ChatChunk(confirmation.clone())).await;
            let confirmation_message = json!({ "role": "assistant", "content": confirmation });
            history.push(confirmation_message.clone());
            new_messages.push(confirmation_message);
            let _ = tx
                .send(Out::Done {
                    messages: history.clone(),
                    new_messages: new_messages.clone(),
                    last_tool: last_tool.clone(),
                })
                .await;
            return;
        }

        if opts.chat_mode && !chat_lookup_completed {
            // A Chat-mode lookup is only evidence gathering. Preserve the
            // schema and prompt mode for cache identity, but force the native
            // follow-up to prose with tool_choice:none.
            chat_lookup_completed = true;
            strict = false;
            call_only = false;
            continue;
        }

        if repeats >= 3 {
            let _ = tx
                .send(Out::Done {
                    messages: history.clone(),
                    new_messages: new_messages.clone(),
                    last_tool: last_tool.clone(),
                })
                .await;
            return;
        }

        if let Some(excess) = per_turn_excess {
            let guidance = json!({"role": "user", "content": format!(
                "You made too many tool calls this turn; only the first {} ran and {} were discarded. Make at most {} tool call(s) per turn and wait for the results.",
                opts.max_calls, excess, opts.max_calls
            )});
            history.push(guidance.clone());
            new_messages.push(guidance);
            continue;
        }

        if total_calls >= opts.total_max {
            no_tools = true;
            let guidance = json!({"role": "user", "content":
                "You have used your tool budget. STOP calling tools and answer the user with what you now know (including if something was not found)."
            });
            history.push(guidance.clone());
            new_messages.push(guidance);
            continue;
        }
    }

    // Hit max turns — return whatever we have so Myelin can close the turn.
    let _ = tx
        .send(Out::Done {
            messages: history,
            new_messages,
            last_tool,
        })
        .await;
}

/// Ask Myelin to run a tool: emit a `Tool` event, register a oneshot keyed by
/// request+call id, and await the result Myelin posts to `/v1/tool-result`.
/// Aborts the wait immediately when the request is cancelled.
async fn dispatch_tool(
    tx: &mpsc::Sender<Out>,
    pending: &Pending,
    request_id: &str,
    call_id: &str,
    name: &str,
    args_raw: &str,
    timeout_secs: u64,
    cancel: &watch::Receiver<bool>,
) -> String {
    let key = format!("{request_id}:{call_id}");
    let (otx, orx) = oneshot::channel::<String>();
    pending.lock().await.insert(key.clone(), otx);

    let _ = tx
        .send(Out::Tool {
            id: call_id.to_string(),
            name: name.to_string(),
            arguments: args_raw.to_string(),
        })
        .await;

    let mut cancel = cancel.clone();
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(timeout_secs), orx) => match result {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                pending.lock().await.remove(&key);
                format!("Tool '{name}' failed: the host closed the result channel.")
            }
            Err(_) => {
                pending.lock().await.remove(&key);
                format!("Tool '{name}' timed out after {timeout_secs}s.")
            }
        },
        _ = cancel.changed() => {
            pending.lock().await.remove(&key);
            format!("Tool '{name}' was cancelled before it completed.")
        }
    }
}



/// Read the upstream SSE stream: forward assistant text as `ChatChunk`, stream
/// native function-call `write_note` arguments or prompt-tools content as
/// `NoteStart`/`NoteDelta` when they arrive incrementally, and assemble the
/// tool-call deltas. No text is fabricated if the upstream server batches SSE
/// chunks.
async fn stream_upstream_with_timeout(
    resp: reqwest::Response,
    tx: &mpsc::Sender<Out>,
    no_think: bool,
    suppress_text_call: bool,
    stream_note_preview: bool,
    schemas: &Value,
    cancel: &watch::Receiver<bool>,
    timeout_secs: u64,
) -> Result<(String, Vec<Value>, bool), String> {
    stream_upstream(
        resp,
        tx,
        no_think,
        suppress_text_call,
        stream_note_preview,
        schemas,
        cancel,
        timeout_secs,
    )
    .await
}

async fn stream_upstream(
    resp: reqwest::Response,
    tx: &mpsc::Sender<Out>,
    no_think: bool,
    suppress_text_call: bool,
    stream_note_preview: bool,
    schemas: &Value,
    cancel: &watch::Receiver<bool>,
    idle_timeout_secs: u64,
) -> Result<(String, Vec<Value>, bool), String> {
    let mut content = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    // Live note-streaming state.
    let mut note_streaming = false;
    let mut note_emitted = String::new();
    let mut note_cancelled = false;
    // Once a note-mutating tool starts, stop echoing prose to chat (it duplicates
    // the note in the editor).
    let mut suppress_prose = false;

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let stream_started = Instant::now();
    let mut first_delta = false;
    let mut cancel = cancel.clone();

    loop {
        // Cancellation aborts mid-generation: drop the upstream connection so
        // the llama slot frees immediately.
        if *cancel.borrow() {
            return Err("cancelled by user".to_string());
        }
        let chunk = tokio::time::timeout(
            Duration::from_secs(idle_timeout_secs.max(1)),
            async {
                tokio::select! {
                    chunk = stream.next() => Ok(chunk),
                    _ = cancel.changed() => Err("cancelled by user"),
                }
            },
        )
        .await
        .map_err(|_| {
            format!(
                "upstream stream idle for {idle_timeout_secs}s while waiting for llama-server output"
            )
        })??;
        let Some(chunk) = chunk else { break };
        let bytes = chunk.map_err(|e| {
            if e.is_timeout() {
                "upstream stream stalled while waiting for llama-server output".to_string()
            } else {
                format!("upstream stream error: {e}")
            }
        })?;
        buf.extend_from_slice(&bytes);

        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim();
            let data = match line.strip_prefix("data:") {
                Some(d) => d.trim(),
                None => continue,
            };
            if data == "[DONE]" {
                buf.clear();
                return finish(content, tool_calls, no_think, note_streaming);
            }
            let chunk_json: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Detect error events in the streaming response. llama-server may
            // emit an error mid-stream when it fails to decode a tool call
            // (e.g. "could not decode tool: unexpected end of JSON input").
            // Previously these were silently skipped (no "choices" key),
            // causing the model to appear to return nothing and triggering
            // the native-empty fallback — which adds a wasted round-trip.
            // Surface the error so the caller can retry with prompt-tools.
            if let Some(err) = chunk_json.get("error") {
                let msg = err.as_str().unwrap_or("unknown error");
                return Err(format!("completion parsing error: {msg}"));
            }
            // llama-server emits usage on the final chunk when include_usage is set.
            // Always forward the event even when values are zero so the frontend can
            // distinguish "no usage data yet" from "usage is zero".
            if let Some(usage) = chunk_json.get("usage") {
                let pt = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                let ct = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
                let tt = usage["total_tokens"].as_u64().unwrap_or(0) as u32;
                let cached = usage["prompt_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .or_else(|| chunk_json["timings"]["cache_n"].as_u64())
                    .unwrap_or(0) as u32;
                let evaluated = pt.saturating_sub(cached);
                let ratio = if pt == 0 {
                    0.0
                } else {
                    cached as f64 / pt as f64
                };
                let _ = tx
                    .send(Out::Usage {
                        prompt_tokens: pt,
                        completion_tokens: ct,
                        total_tokens: tt,
                        cached_tokens: cached,
                        evaluated_tokens: evaluated,
                        cache_reuse_ratio: ratio,
                    })
                    .await;
            }
            let choice = match chunk_json["choices"].get(0) {
                Some(c) => c,
                None => continue,
            };
            let delta = &choice["delta"];

            if let Some(t) = delta["content"].as_str() {
                if !t.is_empty() {
                    if !first_delta {
                        first_delta = true;
                        let _ = tx.send(Out::Debug {
                            kind: "first_model_delta".into(),
                            message: format!("elapsed_ms={}", stream_started.elapsed().as_millis()),
                        }).await;
                    }
                    content.push_str(t);
                    if !suppress_prose && !suppress_text_call {
                        let _ = tx.send(Out::ChatChunk(t.to_string())).await;
                    }

                    // Prompt-tools calls arrive as ordinary content rather than
                    // OpenAI `tool_calls` deltas. Extract the decoded content
                    // string from each upstream delta and stream only that real
                    // generated text into the editor. If the server batches its
                    // SSE output, the UI can only display that batch when it
                    // arrives; it must not invent text ahead of the model.
                    if stream_note_preview && suppress_text_call && content.contains("write_note") && !note_cancelled {
                        // Do not classify a partial `find` field as an edit:
                        // weak models can emit transient/incomplete fields while
                        // the replacement body is still streaming. Only an
                        // explicit append or a fully valid JSON object can cancel
                        // the replace preview.
                        let partial_mode = harness::partial_field(&content, "mode");
                        let parsed = serde_json::from_str::<Value>(&content).ok();
                        let is_append = partial_mode.as_deref() == Some("append");
                        let complete_find = parsed
                            .as_ref()
                            .and_then(|v| v.get("find"))
                            .and_then(Value::as_str)
                            .map(|f| !f.trim().is_empty())
                            .unwrap_or(false);
                        let is_replace = !is_append && !complete_find;
                        if !is_replace {
                            if note_streaming {
                                let _ = tx.send(Out::NoteCancel).await;
                                note_streaming = false;
                            }
                            note_cancelled = true;
                        } else if let Some(c) = harness::extract_partial_content(&content) {
                            if !c.is_empty() && !note_streaming {
                                let _ = tx.send(Out::NoteStart).await;
                                note_streaming = true;
                            }
                            if c.len() > note_emitted.len() && c.starts_with(&note_emitted) {
                                let new_part = c[note_emitted.len()..].to_string();
                                if !new_part.is_empty() {
                                    let _ = tx.send(Out::NoteDelta(new_part)).await;
                                }
                                note_emitted = c;
                            }
                        }
                    }

                    // Some chat templates append a closing tool wrapper but never
                    // emit the upstream [DONE] event. Once that wrapper arrives,
                    // the call is terminal: execute it immediately if it parses,
                    // otherwise fail now so the speculative note is reverted
                    // instead of leaving the UI timer running until timeout.
                    if suppress_text_call
                        && [
                            "</tool_call>",
                            "<|tool_call_end|>",
                            "<|eot_id|>",
                            "<|end_of_text|>",
                        ]
                            .iter()
                            .any(|marker| content.contains(marker))
                    {
                        if harness::parse_text_tool_calls(&content, schemas).is_some() {
                            return finish(content, tool_calls, no_think, note_streaming);
                        }
                        if note_streaming {
                            let _ = tx.send(Out::NoteCancel).await;
                        }
                        return Err(
                            "Generation ended with an incomplete write_note call. Live preview reverted; no changes were saved."
                                .to_string(),
                        );
                    }

                }
            }

            if let Some(tcs) = delta["tool_calls"].as_array() {
                if !first_delta && !tcs.is_empty() {
                    first_delta = true;
                    let _ = tx.send(Out::Debug {
                        kind: "first_model_delta".into(),
                        message: format!("elapsed_ms={}", stream_started.elapsed().as_millis()),
                    }).await;
                }
                for tc in tcs {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    while tool_calls.len() <= idx {
                        tool_calls.push(json!({"id":"","type":"function","function":{"name":"","arguments":""}}));
                    }
                    let slot = &mut tool_calls[idx];
                    if let Some(id) = tc["id"].as_str() {
                        if !id.is_empty() {
                            slot["id"] = json!(id);
                        }
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        if !name.is_empty() {
                            slot["function"]["name"] = json!(name);
                            if matches!(name, "write_note" | "append_note" | "prepend_note" | "replace_in_note" | "insert_after_line" | "delete_in_note" | "format_note" | "edit_notebook") {
                                suppress_prose = true;
                            }
                        }
                    }
                    if let Some(a) = tc["function"]["arguments"].as_str() {
                        let prev = slot["function"]["arguments"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        slot["function"]["arguments"] = json!(prev + a);
                    }

                    // Live-stream write_note's whole-body content into the editor.
                    let slot_name = slot["function"]["name"].as_str().unwrap_or("").to_string();
                    let slot_args = slot["function"]["arguments"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    if stream_note_preview && slot_name == "write_note" && !note_cancelled {
                        // Keep streaming until an explicit append or a complete
                        // JSON object confirms a targeted edit. Partial `find`
                        // fields are not reliable enough to cancel the preview.
                        let partial_mode = harness::partial_field(&slot_args, "mode");
                        let parsed = serde_json::from_str::<Value>(&slot_args).ok();
                        let is_append = partial_mode.as_deref() == Some("append");
                        let complete_find = parsed
                            .as_ref()
                            .and_then(|v| v.get("find"))
                            .and_then(Value::as_str)
                            .map(|f| !f.trim().is_empty())
                            .unwrap_or(false);
                        let is_replace = !is_append && !complete_find;
                        if !is_replace {
                            if note_streaming {
                                let _ = tx.send(Out::NoteCancel).await;
                                note_streaming = false;
                            }
                            note_cancelled = true;
                        } else if let Some(c) = harness::extract_partial_content(&slot_args) {
                            if !note_streaming {
                                let _ = tx.send(Out::NoteStart).await;
                                note_streaming = true;
                            }
                            if c.len() > note_emitted.len() && c.starts_with(&note_emitted) {
                                let new_part = c[note_emitted.len()..].to_string();
                                let _ = tx.send(Out::NoteDelta(new_part)).await;
                                note_emitted = c;
                            }
                        }
                    }
                }
            }
        }
    }
    finish(content, tool_calls, no_think, note_streaming)
}

fn finish(
    content: String,
    mut tool_calls: Vec<Value>,
    no_think: bool,
    note_streaming: bool,
) -> Result<(String, Vec<Value>, bool), String> {
    tool_calls.retain(|t| !t["function"]["name"].as_str().unwrap_or("").is_empty());
    validate_generated_tool_calls(&tool_calls)?;
    let content = if no_think {
        harness::strip_think(&content)
    } else {
        content
    };
    Ok((content, tool_calls, note_streaming))
}

fn validate_generated_tool_calls(tool_calls: &[Value]) -> Result<(), String> {
    for call in tool_calls {
        if call["function"]["name"].as_str() != Some("write_note") {
            continue;
        }
        let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
        let content = serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|value| value["content"].as_str().map(str::to_owned));
        if content
            .as_deref()
            .is_some_and(harness::note_content_has_protocol_residue)
        {
            return Err(
                "Generation mixed tool protocol text into the note. Live preview reverted; no changes were saved."
                    .to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        is_mutating_tool, is_terminal_mutation, normalize_lfm_tool_arguments,
        should_stream_note_preview,
    };
    use serde_json::json;

    #[test]
    fn lfm_history_uses_mapping_arguments_for_native_template_rendering() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "type": "function",
                "function": {
                    "name": "write_note",
                    "arguments": "{\"content\":\"hello\",\"mode\":\"replace\"}"
                }
            }]
        })];
        normalize_lfm_tool_arguments(&mut messages, "LFM2-2.6B-Tool-Q4.gguf");
        assert_eq!(
            messages[0]["tool_calls"][0]["function"]["arguments"]["content"],
            "hello"
        );
    }

    #[test]
    fn non_lfm_history_keeps_openai_argument_strings() {
        let mut messages = vec![json!({
            "tool_calls": [{
                "function": {"name": "write_note", "arguments": "{\"content\":\"hello\"}"}
            }]
        })];
        normalize_lfm_tool_arguments(&mut messages, "qwen.gguf");
        assert!(messages[0]["tool_calls"][0]["function"]["arguments"].is_string());
    }

    #[test]
    fn successful_note_write_is_terminal() {
        assert!(is_terminal_mutation(
            "write_note",
            "Note successfully updated with ID: 90dc6"
        ));
    }

    #[test]
    fn failed_or_readonly_tool_result_is_not_terminal() {
        assert!(!is_terminal_mutation(
            "write_note",
            "No note is currently open to write to."
        ));
        assert!(!is_terminal_mutation("read_note", "Note successfully updated"));
    }

    #[test]
    fn mutating_tools_are_identified_for_operation_abort() {
        assert!(is_mutating_tool("write_note"));
        assert!(is_mutating_tool("replace_in_note"));
        assert!(!is_mutating_tool("search_notes"));
    }

    #[test]
    fn other_successful_mutations_are_terminal() {
        assert!(is_terminal_mutation("format_note", " Note successfully updated."));
        assert!(is_terminal_mutation("edit_notebook", "Notebook cell updated."));
        assert!(is_terminal_mutation(
            "append_note",
            "Note successfully updated with ID: 42"
        ));
        assert!(is_terminal_mutation(
            "prepend_note",
            "Note successfully updated"
        ));
        assert!(is_terminal_mutation(
            "replace_in_note",
            "Note successfully updated with ID: 1"
        ));
        assert!(is_terminal_mutation(
            "insert_after_line",
            "Note successfully updated with ID: 1"
        ));
        assert!(is_terminal_mutation(
            "delete_in_note",
            "Note successfully updated with ID: 1"
        ));
    }

    #[test]
    fn selection_scoped_writes_always_enable_safe_preview_streaming() {
        assert!(should_stream_note_preview("insert text here", true));
        assert!(should_stream_note_preview("add below the selection", true));
    }

}
