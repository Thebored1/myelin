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
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const HISTORY_BUDGET: usize = 16_000;

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
    /// Prime a closed `<think>` turn so the model emits no reasoning
    /// (OPENHARN_NO_THINK). Can't combine with strict's grammar.
    #[serde(default)]
    pub no_think: bool,
    /// Per-turn circuit-breaker limit on tool calls (OPENHARN_MAX_CALLS).
    #[serde(default = "default_max_calls")]
    pub max_calls: usize,
    /// Total tool calls across all turns before tools are removed (OPENHARN_TOTAL_MAX).
    #[serde(default = "default_total_max")]
    pub total_max: usize,
    /// Seconds to wait for a tool result from the host before failing the call.
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: u64,
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
}

fn default_max_calls() -> usize {
    1
}
fn default_total_max() -> usize {
    5
}
fn default_tool_timeout() -> u64 {
    300
}

fn upstream_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(2)
            .connect_timeout(Duration::from_secs(10))
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
            max_calls: default_max_calls(),
            total_max: default_total_max(),
            tool_timeout_secs: default_tool_timeout(),
            narrow: false,
            tool_subset: Vec::new(),
            slm: false,
            tool_choice: None,
            template_kwargs: None,
            friendly_results: false,
            call_only: false,
            intent_is_tool: None,
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
        last_tool: Option<String>,
    },
    Error(String),
    /// Token usage from llama-server's `include_usage` stream option.
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    },
}

/// Classify the user's latest message as `TOOL` or `CHAT` using the model
/// (1 word, cheap and fast). Returns `true` when the intent is `TOOL`.
/// On any error or empty reply we default to `TOOL` (safer: losing a genuine
/// tool request is worse than a greeting occasionally entering the tool loop).
async fn detect_intent(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    model: &str,
    user: &str,
) -> bool {
    let prompt = format!(
        "You are a note-taking assistant. Classify the user's request with exactly one word: TOOL or CHAT.\n\
         CHAT = greeting, thanks, small talk.\n\
         TOOL = anything involving writing, editing, searching, reading, formatting, or fetching notes or documents.\n\n\
         User: {user}\n\n\
         Classification:"
    );
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.0,
        "max_tokens": 4,
        "stream": false,
    });
    let mut req = client.post(url).json(&body);
    if let Some(k) = api_key {
        if !k.is_empty() {
            req = req.bearer_auth(k);
        }
    }
    let text = match req.send().await {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => v["choices"]
                .get(0)
                .and_then(|c| c["message"]["content"].as_str())
                .map(|s| s.trim().to_uppercase())
                .unwrap_or_default(),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    };
    // Default to TOOL on ambiguity: losing a genuine note-write is worse than
    // a greeting occasionally calling a tool. Only an explicit "CHAT" is treated
    // as conversation.
    if text.is_empty() {
        return true;
    }
    if text.contains("TOOL") {
        true
    } else if text.contains("CHAT") {
        false
    } else {
        true
    }
}

/// Drive one user request to completion, streaming events on `tx` and requesting
/// tool execution from Myelin via the `pending` registry.
pub async fn run_loop(req: ChatRequest, tx: mpsc::Sender<Out>, pending: Pending) {
    let request_id = req
        .request_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let model = req.model.clone().unwrap_or_else(|| "myelin".to_string());
    let temperature = req.temperature.unwrap_or(0.2);
    let max_tokens = req.max_tokens.unwrap_or(4096);
    let max_turns = req.max_turns.unwrap_or(8).max(1);
    let opts = req.options.clone();

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
                !matches!(name, "write_note" | "format_note" | "edit_notebook")
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
    let plan_len = if has_tools {
        harness::harness_decompose(user_text, &effective_schemas).len()
    } else {
        0
    };

    // strict grammar implies prompt-tools (text-form calls); mirror openharn.
    // Per-request policy: only use prompt-tools + strict for multi-call requests
    // (plan_len > 1). Single-call requests use native FC, which scores ~80% vs
    // 29.5% for forced prompt-tools (paper Table 1).
    let strict = opts.strict || narrow || plan_len > 1;
    let prompt_tools = strict || opts.prompt_tools;
    let no_think = opts.no_think && !strict;

    let mut seen_calls: HashSet<String> = HashSet::new();
    let mut budget = HISTORY_BUDGET;
    let mut repeats = 0usize;
    let mut total_calls = 0usize;
    let mut no_tools = !has_tools;
    let mut last_tool: Option<String> = None;

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
            let user_text = history
                .iter()
                .rev()
                .find(|m| m["role"].as_str() == Some("user"))
                .and_then(|m| m["content"].as_str())
                .unwrap_or("")
                .to_string();
            detect_intent(&client, &url, req.api_key.as_deref(), &model, &user_text).await
        }
    } else {
        true
    };

    // FAST PATH: plan_len==0 means harness_decompose found NO matching tool —
    // this is definitely irrelevant. Skip the gate LLM call entirely and
    // abstain immediately. This is cheaper AND more accurate than the gate
    // (the gate still gets ~15% of irrelevance cases wrong due to the weak
    // model's YES/NO quality). Note: if friendly mode classified this as CHAT,
    // we already returned above — so reaching here means TOOL intent, and
    // plan_len==0 is a definitive no-match.
    if has_tools && plan_len == 0 {
        let _ = tx
            .send(Out::Done {
                messages: history.clone(),
                last_tool: None,
            })
            .await;
        return;
    }

    // CHAT intent: skip the tool loop, answer directly in prose.
    if friendly && !intent_is_tool {
        let wire = history.clone();
        let mut body = json!({
            "model": model,
            "messages": wire,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
            "cache_prompt": true,
        });
        if no_think {
            if let Some(arr) = body["messages"].as_array_mut() {
                arr.push(json!({ "role": "assistant", "content": "<think></think>" }));
            }
        }
        if let Some(kw) = &opts.template_kwargs {
            if let Ok(v) = serde_json::from_str::<Value>(kw) {
                body["chat_template_kwargs"] = v;
            }
        }
        let resp = match client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = tx
                    .send(Out::Error(format!("chat request failed: {e}")))
                    .await;
                return;
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            let _ = tx
                .send(Out::Error(format!("upstream HTTP {status}: {txt}")))
                .await;
            return;
        }
        let (content, _) = match stream_upstream(resp, &tx, no_think).await {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Out::Error(e)).await;
                return;
            }
        };
        let mut h = history.clone();
        h.push(json!({ "role": "assistant", "content": content }));
        let _ = tx
            .send(Out::Done {
                messages: h,
                last_tool: None,
            })
            .await;
        return;
    }

    for _turn in 0..max_turns {
        harness::fit_context(&mut history, budget);

        let mut wire = if prompt_tools && has_tools {
            harness::flatten_for_prompt_tools(&history, &effective_schemas)
        } else {
            history.clone()
        };
        if no_think {
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
        });
        if no_tools {
            // no tools available — text only
        } else if prompt_tools {
            if strict {
                // Call-only grammar for multi-call requests (forces the model
                // to output a call array, not prose). For single-call requests
                // the model uses native FC, so this path is only reached for
                // plan_len > 1 (or explicit call_only override).
                let grammar_root = if plan_len > 1 || (opts.call_only && friendly && intent_is_tool)
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
            let choice: Value = match &opts.tool_choice {
                Some(c) if c == "required" || c == "none" || c == "auto" => {
                    json!(c)
                }
                Some(name) => {
                    // Specific tool name: {'type':'function','function':{'name':tool}}
                    json!({
                        "type": "function",
                        "function": { "name": name }
                    })
                }
                None => json!("auto"),
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

        // POST to llama-server, retrying with backoff. The host app starts the
        // server just before calling us, so the first attempt can land while it's
        // still binding — give it a few chances before surfacing an error.
        let resp = {
            let mut attempt = 0u32;
            const MAX_ATTEMPTS: u32 = 6;
            loop {
                let mut rq = client.post(&url).json(&body);
                if let Some(k) = &req.api_key {
                    if !k.is_empty() {
                        rq = rq.bearer_auth(k);
                    }
                }
                match rq.send().await {
                    Ok(r) => break Some(r),
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
            if status.as_u16() == 400 && txt.contains("context") && budget > 4_000 {
                budget /= 2;
                continue;
            }
            let _ = tx
                .send(Out::Error(format!("upstream HTTP {status}: {txt}")))
                .await;
            return;
        }

        let (mut content, mut tool_calls) = match stream_upstream(resp, &tx, no_think).await {
            Ok(v) => v,
            Err(e) => {
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

        // Native-empty fallback: when native FC returns NOTHING, retry with
        // prompt-tools + strict grammar. Recovers text-gen failures without
        // forcing strict on every case. Per openharn paper: this is the
        // "native-empty fallback" that bridges the gap between native FC
        // (good for single calls) and prompt-tools (needed for multi-call).
        if !prompt_tools && tool_calls.is_empty() && has_tools && plan_len <= 1 {
            let fb_wire = harness::flatten_for_prompt_tools(&history, &effective_schemas);
            let mut fb_body = json!({
                "model": model,
                "messages": fb_wire,
                "temperature": temperature,
                "max_tokens": max_tokens,
                "stream": true,
                "stream_options": { "include_usage": true },
                "cache_prompt": true,
                "grammar": json!(harness::tool_grammar(&effective_schemas, "call")),
            });
            if let Some(kw) = &opts.template_kwargs {
                if let Ok(v) = serde_json::from_str::<Value>(kw) {
                    fb_body["chat_template_kwargs"] = v;
                }
            }
            let fb_resp = {
                let mut attempt = 0u32;
                const MAX_ATTEMPTS: u32 = 6;
                loop {
                    let mut rq = client.post(&url).json(&fb_body);
                    if let Some(k) = &req.api_key {
                        if !k.is_empty() {
                            rq = rq.bearer_auth(k);
                        }
                    }
                    match rq.send().await {
                        Ok(r) => break Some(r),
                        Err(e) => {
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
                    let (fb_content, fb_calls) = match stream_upstream(fb_resp, &tx, false).await {
                        Ok(v) => v,
                        Err(_) => (String::new(), Vec::new()),
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
        history.push(assistant);

        if tool_calls.is_empty() {
            let _ = tx
                .send(Out::Done {
                    messages: history.clone(),
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
                )
                .await
            };

            let cap = if opts.slm {
                harness::TOOL_RESULT_CAP / 3
            } else {
                harness::TOOL_RESULT_CAP
            };
            let capped = harness::cap_result_with(result.clone(), cap);
            let _ = tx
                .send(Out::ToolResult {
                    id: id.clone(),
                    name: name.clone(),
                    result: capped.clone(),
                })
                .await;

            history.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": capped,
            }));
        }
        total_calls += tool_calls.len();

        if repeats >= 3 {
            let _ = tx
                .send(Out::Done {
                    messages: history.clone(),
                    last_tool: last_tool.clone(),
                })
                .await;
            return;
        }

        if let Some(excess) = per_turn_excess {
            history.push(json!({"role": "user", "content": format!(
                "You made too many tool calls this turn; only the first {} ran and {} were discarded. Make at most {} tool call(s) per turn and wait for the results.",
                opts.max_calls, excess, opts.max_calls
            )}));
            continue;
        }

        if total_calls >= opts.total_max {
            no_tools = true;
            history.push(json!({"role": "user", "content":
                "You have used your tool budget. STOP calling tools and answer the user with what you now know (including if something was not found)."
            }));
            continue;
        }
    }

    // Hit max turns — return whatever we have so Myelin can close the turn.
    let _ = tx
        .send(Out::Done {
            messages: history,
            last_tool,
        })
        .await;
}

/// Ask Myelin to run a tool: emit a `Tool` event, register a oneshot keyed by
/// request+call id, and await the result Myelin posts to `/v1/tool-result`.
async fn dispatch_tool(
    tx: &mpsc::Sender<Out>,
    pending: &Pending,
    request_id: &str,
    call_id: &str,
    name: &str,
    args_raw: &str,
    timeout_secs: u64,
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

    match tokio::time::timeout(Duration::from_secs(timeout_secs), orx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            pending.lock().await.remove(&key);
            format!("Tool '{name}' failed: the host closed the result channel.")
        }
        Err(_) => {
            pending.lock().await.remove(&key);
            format!("Tool '{name}' timed out after {timeout_secs}s.")
        }
    }
}

/// Read the upstream SSE stream: forward assistant text as `ChatChunk`, stream a
/// live `write_note` body as `NoteStart`/`NoteDelta`/`NoteCancel`, and assemble
/// the (chunked) tool-call deltas. Mirrors Myelin's stream_chat SSE parser.
async fn stream_upstream(
    resp: reqwest::Response,
    tx: &mpsc::Sender<Out>,
    no_think: bool,
) -> Result<(String, Vec<Value>), String> {
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

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("stream error: {e}"))?;
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
                return finish(content, tool_calls, no_think);
            }
            let chunk_json: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // llama-server emits usage on the final chunk when include_usage is set.
            // Always forward the event even when values are zero so the frontend can
            // distinguish "no usage data yet" from "usage is zero".
            if let Some(usage) = chunk_json.get("usage") {
                let pt = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                let ct = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
                let tt = usage["total_tokens"].as_u64().unwrap_or(0) as u32;
                let _ = tx
                    .send(Out::Usage {
                        prompt_tokens: pt,
                        completion_tokens: ct,
                        total_tokens: tt,
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
                    content.push_str(t);
                    if !suppress_prose {
                        let _ = tx.send(Out::ChatChunk(t.to_string())).await;
                    }
                }
            }

            if let Some(tcs) = delta["tool_calls"].as_array() {
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
                            if matches!(name, "write_note" | "format_note" | "edit_notebook") {
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
                    if slot_name == "write_note" && !note_cancelled {
                        let mode = harness::partial_field(&slot_args, "mode");
                        let find = harness::partial_field(&slot_args, "find");
                        let m = mode.as_deref().unwrap_or("");
                        let is_append = m == "append";
                        let explicit_replace = m == "replace";
                        let has_find = find.map(|f| !f.trim().is_empty()).unwrap_or(false);
                        let snippet = has_find && !explicit_replace && !is_append;
                        let is_replace = !is_append && !snippet;
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
    finish(content, tool_calls, no_think)
}

fn finish(
    content: String,
    mut tool_calls: Vec<Value>,
    no_think: bool,
) -> Result<(String, Vec<Value>), String> {
    tool_calls.retain(|t| !t["function"]["name"].as_str().unwrap_or("").is_empty());
    let content = if no_think {
        harness::strip_think(&content)
    } else {
        content
    };
    Ok((content, tool_calls))
}
