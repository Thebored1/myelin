//! Tool-loop state machine.

use crate::agent::{
    detect_intent_in_session, dispatch_tool, normalize_lfm_tool_arguments,
    stream_upstream_with_timeout, validate_generated_tool_calls,
};
use crate::harness;
use crate::policy::{is_mutating_tool, is_terminal_mutation};
use crate::protocol::{ChatRequest, Out};
use crate::runner_policy::{
    effective_schemas, generation_policy, max_tokens, request_schemas, HISTORY_BUDGET,
};
use crate::server::Pending;
use crate::upstream::{client as upstream_client, emit_prompt as emit_model_prompt, request_body};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

pub(crate) async fn run_loop(
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
    let max_tokens = max_tokens(
        req.max_tokens.unwrap_or(4096),
        req.options.chat_mode,
        req.options.intent_is_tool,
        req.options.targeted_write,
    );
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

    let schemas = request_schemas(&req.tools);

    // `narrow` and explicit tool subsets are applied in one policy boundary.
    let effective_schemas = effective_schemas(&schemas, &opts);
    let has_tools = effective_schemas
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    let mut history: Vec<Value> = req.messages.clone();

    let user_text = history
        .iter()
        .rev()
        .find(|m| m["role"].as_str() == Some("user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("");
    let policy = generation_policy(user_text, &effective_schemas, &opts);
    let plan_len = policy.plan_len;
    let stream_note_preview = policy.stream_note_preview;
    let targeted_native = policy.targeted_native;
    let mut strict = policy.strict;
    let mut prompt_tools = policy.prompt_tools;
    let mut no_think = policy.no_think;
    // Call-only keeps tool requests structured until the authoritative write
    // result completes the operation.
    let mut call_only = policy.call_only;
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
        let mut body = request_body(
            &model,
            Value::Array(wire),
            temperature,
            max_tokens,
            opts.external,
            req.session.slot_id,
        );
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
        let resp = match client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = tx
                    .send(Out::Error(format!("chat request failed: {e}")))
                    .await;
                return;
            }
        };
        let _ = tx
            .send(Out::Debug {
                kind: "response_headers".into(),
                message: format!(
                    "elapsed_ms={} status={}",
                    dispatched_at.elapsed().as_millis(),
                    resp.status()
                ),
            })
            .await;
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
        if content.trim().is_empty() {
            let _ = tx
                .send(Out::Error(
                    "The model used its response budget without producing an answer. Please try again."
                        .to_string(),
                ))
                .await;
            return;
        }
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
        let mut body = request_body(
            &model,
            Value::Array(wire),
            temperature,
            max_tokens,
            opts.external,
            req.session.slot_id,
        );
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
            if opts.native_first {
                "prompt-recovery"
            } else {
                "prompt-explicit"
            }
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

        let resp = post_with_retry(&client, &url, req.api_key.as_deref(), &body, &tx, true).await;
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
            let _ = tx.send(Out::Error(message.to_string())).await;
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
                "grammar": json!(harness::tool_grammar(&effective_schemas, "call")),
            });
            if !opts.external {
                fb_body["cache_prompt"] = json!(true);
                fb_body["id_slot"] = json!(req.session.slot_id);
            }
            if let Some(kw) = &opts.template_kwargs {
                if let Ok(v) = serde_json::from_str::<Value>(kw) {
                    fb_body["chat_template_kwargs"] = v;
                }
            }
            emit_model_prompt(&tx, "PROMPT-TOOLS fallback request messages:", &fb_body).await;
            let fb_resp =
                post_with_retry(&client, &url, req.api_key.as_deref(), &fb_body, &tx, false).await;
            if let Some(fb_resp) = fb_resp {
                if fb_resp.status().is_success() {
                    let (fb_content, fb_calls, _) = match stream_upstream_with_timeout(
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

/// POST to llama-server, retrying with backoff. The host app starts the
/// server just before calling us, so the first attempt can land while it's
/// still binding — give it a few chances before surfacing an error. When
/// `error_on_exhausted` is set, emitting the user-facing error here is the
/// caller's whole failure path; otherwise the caller treats `None` as
/// "fallback unavailable" and keeps its primary result.
async fn post_with_retry(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    body: &Value,
    tx: &mpsc::Sender<Out>,
    error_on_exhausted: bool,
) -> Option<reqwest::Response> {
    let mut attempt = 0u32;
    const MAX_ATTEMPTS: u32 = 6;
    let dispatched_at = Instant::now();
    loop {
        let mut rq = client.post(url).json(body);
        if let Some(k) = api_key {
            if !k.is_empty() {
                rq = rq.bearer_auth(k);
            }
        }
        match rq.send().await {
            Ok(r) => {
                let _ = tx
                    .send(Out::Debug {
                        kind: "response_headers".into(),
                        message: format!(
                            "elapsed_ms={} status={}",
                            dispatched_at.elapsed().as_millis(),
                            r.status()
                        ),
                    })
                    .await;
                return Some(r);
            }
            Err(e) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    if error_on_exhausted {
                        let _ = tx
                            .send(Out::Error(format!(
                                "request to llama-server failed after {attempt} attempts: {e}. \
                                 Check the model server is running and reachable at {url}."
                            )))
                            .await;
                    }
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
}
