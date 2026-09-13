use super::*;

pub(super) fn build_chat_non_stream_responses_response(
    resp_id: &str,
    model: &str,
    req: &ResponsesRequest,
    chat_resp: &ChatCompletionResponse,
    output: Vec<Value>,
) -> Value {
    let finish_reason = chat_resp
        .choices
        .as_ref()
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.finish_reason.as_deref());
    let incomplete = matches!(finish_reason, Some("length") | Some("max_tokens"));

    let reasoning = json!({
        "effort": req
            .reasoning
            .as_ref()
            .and_then(|r| r.get("effort"))
            .cloned()
            .unwrap_or(Value::Null),
        "summary": req
            .reasoning
            .as_ref()
            .and_then(|r| r.get("summary"))
            .cloned()
            .unwrap_or(Value::Null),
    });

    let text = req
        .text
        .as_ref()
        .and_then(|t| t.get("format"))
        .map(|format| json!({ "format": format }))
        .unwrap_or_else(|| json!({ "format": { "type": "text" } }));

    json!({
        "id": resp_id,
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": if incomplete { "incomplete" } else { "completed" },
        "model": model,
        "output": output,
        "usage": map_chat_usage_to_responses(chat_resp.usage.as_ref()),
        "parallel_tool_calls": req.parallel_tool_calls.unwrap_or(true),
        "tool_choice": req.tool_choice.clone().unwrap_or_else(|| json!("auto")),
        "reasoning": reasoning,
        "text": text,
        "incomplete_details": if incomplete {
            json!({ "reason": "max_output_tokens" })
        } else {
            Value::Null
        },
        "error": Value::Null,
        "metadata": req.metadata.clone().unwrap_or(Value::Null),
        "previous_response_id": req.previous_response_id.clone().map(Value::String).unwrap_or(Value::Null),
        "instructions": req.instructions.clone().map(Value::String).unwrap_or(Value::Null),
        "temperature": req.temperature.map(Value::from).unwrap_or(Value::Null),
        "top_p": req.top_p.map(Value::from).unwrap_or(Value::Null),
        "max_output_tokens": req.max_output_tokens.map(Value::from).unwrap_or(Value::Null),
        "tools": req.tools.clone().map(Value::Array).unwrap_or_else(|| json!([])),
        "truncation": "disabled",
    })
}

fn map_chat_usage_to_responses(usage: Option<&Value>) -> Value {
    let Some(usage) = usage else {
        return Value::Null;
    };

    let input_tokens = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(input_tokens + output_tokens);

    let mut out = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
    });

    if let Some(cached) = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"))
        .and_then(|d| d.get("cached_tokens"))
        .cloned()
    {
        out["input_tokens_details"] = json!({ "cached_tokens": cached });
    }
    if let Some(reasoning) = usage
        .get("completion_tokens_details")
        .or_else(|| usage.get("output_tokens_details"))
        .and_then(|d| d.get("reasoning_tokens"))
        .cloned()
    {
        out["output_tokens_details"] = json!({ "reasoning_tokens": reasoning });
    }

    out
}

pub(super) async fn handle_non_stream_response(
    state: GatewayState,
    config: ProviderConfig,
    mut chat_req: crate::protocol::chat_completions::ChatCompletionsRequest,
    req: ResponsesRequest,
    request_id: String,
    raw_request: String,
    mut converted_request: String,
    model: String,
    start: Instant,
    client_type: String,
    session_id: Option<String>,
    provider_id: String,
) -> Result<Response, GatewayError> {
    let result = adapter::send_non_stream(&state.http_client, &config, &mut chat_req).await;

    match result {
        Ok(upstream_json) => {
            converted_request = serde_json::to_string(&chat_req).unwrap_or_default();
            let resp_id = format!("resp_{}", &request_id[4..]);
            let tool_call_resolution =
                crate::transform::tool_calls::build_tool_call_resolution_map(&raw_request);

            // Parse upstream response
            let chat_resp: ChatCompletionResponse = serde_json::from_value(upstream_json.clone())
                .unwrap_or(ChatCompletionResponse {
                    id: None,
                    choices: None,
                    usage: None,
                });

            // Convert to Responses format
            let mut output = Vec::new();
            let mut tool_calls_json = String::new();

            if let Some(choices) = &chat_resp.choices {
                if choices.is_empty() {
                    // Empty choices — emit a placeholder message so Codex doesn't hang
                    let msg_id = format!("msg_{}", resp_id.replace("resp_", ""));
                    output.push(json!({
                        "id": msg_id, "type": "message", "status": "completed",
                        "role": "assistant", "content": [{"type": "output_text", "text": ""}]
                    }));
                }
                for choice in choices {
                    if let Some(msg) = &choice.message {
                        // 剥离模型模仿 Codex commentary channel 输出的字面
                        // <commentary> 标签（与 sse.rs 流式路径对称）。剥离后
                        // 的文本同时作为 reasoning_store 的 key——Codex 下轮
                        // 回传的就是剥离后的文本，key 必须一致才能命中。
                        let text_content =
                            crate::transform::responses_to_chat::strip_commentary_tags(
                                &msg.content.clone().unwrap_or_default(),
                            );

                        // Store reasoning_content for future multi-turn requests
                        if let Some(ref rc) = msg.reasoning_content {
                            if !rc.is_empty() {
                                let tc_ids: Vec<String> = msg
                                    .tool_calls
                                    .as_ref()
                                    .map(|tcs| tcs.iter().map(|tc| tc.id.clone()).collect())
                                    .unwrap_or_default();
                                crate::transform::reasoning_store::store(
                                    &model,
                                    &text_content,
                                    rc,
                                    &tc_ids,
                                );
                            }
                        }

                        // Text content
                        if !text_content.is_empty() {
                            let msg_id = format!("msg_{}", resp_id.replace("resp_", ""));
                            // Pull web-search annotations from the raw upstream message
                            // (ChatCompletionResponse struct doesn't model them; the
                            // shape is provider-defined and we pass through verbatim).
                            let annotations = upstream_json
                                .get("choices")
                                .and_then(|c| c.as_array())
                                .and_then(|arr| arr.first())
                                .and_then(|c| c.get("message"))
                                .and_then(|m| m.get("annotations"))
                                .and_then(|a| a.as_array())
                                .map(|anns| {
                                    crate::protocol::responses_events::normalize_annotations(anns)
                                })
                                .unwrap_or_default();
                            let mut item = json!({
                                "id": msg_id,
                                "type": "message",
                                "status": "completed",
                                "role": "assistant",
                                "content": [{
                                    "type": "output_text",
                                    "text": &text_content,
                                    "annotations": annotations,
                                }]
                            });
                            if let Some(ref rc) = msg.reasoning_content {
                                if !rc.is_empty() {
                                    item["reasoning_content"] = json!(rc);
                                }
                            }
                            output.push(item);
                        }

                        // Tool calls
                        if let Some(ref tcs) = msg.tool_calls {
                            // #5 修复：非流式响应路径也对 arguments 做 JSON 合法性
                            // salvage（与 sse.rs 流式路径对称）。上游偶尔在非流式
                            // 模式下回半截 JSON args（finish_reason="length" 或自身
                            // 截断），原样塞给客户端 → 下轮 history 带病。
                            let finish = choice.finish_reason.as_deref();
                            for tc in tcs {
                                let mut item = crate::transform::tool_calls::responses_tool_call_item_from_chat_name(
                                    &format!("fc_{}", tc.id),
                                    &tc.id,
                                    &tc.function.name,
                                    &tc.function.arguments,
                                    finish,
                                    &tool_call_resolution,
                                );
                                if let Some(ref rc) = msg.reasoning_content {
                                    if !rc.is_empty() {
                                        item["reasoning_content"] = json!(rc);
                                    }
                                }
                                output.push(item);
                            }
                            tool_calls_json = serde_json::to_string(tcs).unwrap_or_default();
                        }
                    }
                }
            }

            let responses_resp = build_chat_non_stream_responses_response(
                &resp_id, &model, &req, &chat_resp, output,
            );
            let latency = start.elapsed().as_millis() as i64;

            // Store session for previous_response_id support
            {
                let mut asst_msgs = Vec::new();
                if let Some(choices) = &chat_resp.choices {
                    for choice in choices {
                        if let Some(msg) = &choice.message {
                            asst_msgs.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: msg
                                    .content
                                    .as_ref()
                                    .map(|c| serde_json::Value::String(c.clone())),
                                reasoning_content: msg.reasoning_content.clone(),
                                tool_calls: msg.tool_calls.clone(),
                                tool_call_id: None,
                                name: None,
                            });
                        }
                    }
                }
                crate::gateway::session_store::store_turn(
                    &resp_id,
                    chat_req.messages.clone(),
                    asst_msgs,
                    chat_resp
                        .choices
                        .as_ref()
                        .and_then(|c| c.first())
                        .and_then(|c| c.message.as_ref())
                        .and_then(|m| m.reasoning_content.clone()),
                );
            }

            // Extract token usage from upstream
            let (in_tok, out_tok) = crate::gateway::usage::extract_chat(&upstream_json);
            let (cache_w, cache_r) = chat_resp
                .usage
                .as_ref()
                .map(crate::storage::request_logs::extract_cache_tokens)
                .unwrap_or((None, None));

            // Record session affinity if the upstream reported cache hits.
            // Skipped silently when no session_id (short prompts) or usage is absent.
            if let Some(ref sid) = session_id {
                if let Some(usage) = chat_resp.usage.as_ref() {
                    crate::gateway::session_affinity::record_if_cache_hit(sid, &provider_id, usage);
                }
            }

            // Log success
            let trace = trace_with_degradation_events(
                json!({ "response_id": &resp_id, "stream": false }),
                &chat_req.diagnostic_events,
            );
            log_request_success(
                &state.db,
                &client_type,
                "/v1/responses",
                &request_id,
                &raw_request,
                &converted_request,
                &serde_json::to_string(&upstream_json).unwrap_or_default(),
                &serde_json::to_string(&responses_resp).unwrap_or_default(),
                if tool_calls_json.is_empty() {
                    None
                } else {
                    Some(&tool_calls_json)
                },
                &config.name,
                &model,
                200,
                latency,
                Some(&trace),
                crate::gateway::usage::TokenUsage {
                    input: in_tok,
                    output: out_tok,
                    cache_write: cache_w,
                    cache_read: cache_r,
                    input_semantics: crate::gateway::usage::InputCacheSemantics::IncludesCacheRead,
                },
            );

            Ok(Json(responses_resp).into_response())
        }
        Err(err) => {
            let latency = start.elapsed().as_millis() as i64;
            let status = if err.code == "PROVIDER_API_KEY_MISSING" {
                401
            } else if err.code.starts_with("UPSTREAM") {
                502
            } else {
                500
            };

            log_request_error_full(
                &state.db,
                &client_type,
                "/v1/responses",
                &request_id,
                &raw_request,
                &converted_request,
                &config.name,
                &model,
                &err,
                status,
                latency,
            );

            Err(GatewayError(err))
        }
    }
}

pub(super) async fn handle_stream_response(
    state: GatewayState,
    config: ProviderConfig,
    mut chat_req: crate::protocol::chat_completions::ChatCompletionsRequest,
    request_id: String,
    raw_request: String,
    mut converted_request: String,
    model: String,
    start: Instant,
    client_type: String,
    session_id: Option<String>,
    provider_id: String,
) -> Result<Response, GatewayError> {
    let mut degraded_bootstrap_web_search = false;
    let upstream_resp = loop {
        let upstream_resp = adapter::send_stream(&state.http_client, &config, &mut chat_req).await;
        match upstream_resp {
            Ok(response) => {
                // Bootstrap-validate the upstream stream: read the leading window
                // before any byte reaches the client so HTTP-200-with-error-frame
                // failures become a clean Err. MiMo can report paid web_search
                // plugin failures here, so degrade before opening the client stream.
                match crate::gateway::sse_bootstrap::bootstrap_detect(response).await {
                    Ok(boot) => break Ok(boot),
                    Err(e)
                        if !degraded_bootstrap_web_search
                            && adapter::is_mimo_web_search_disabled_error(&e)
                            && adapter::strip_mimo_web_search_tool(&mut chat_req) =>
                    {
                        adapter::remember_mimo_web_search_disabled(&config);
                        let degraded_model = chat_req.model.clone();
                        chat_req.diagnostic_events.push(
                            crate::transform::degradation::web_search_degraded_event(
                                &config.provider_type,
                                Some(degraded_model.as_str()),
                                "stream_bootstrap_web_search_disabled",
                            ),
                        );
                        converted_request = serde_json::to_string(&chat_req).unwrap_or_default();
                        degraded_bootstrap_web_search = true;
                        tracing::warn!(
                            provider = %config.name,
                            "MiMo stream reported Web Search Plugin disabled in bootstrap; stripped web_search and retrying once"
                        );
                        continue;
                    }
                    Err(e) => break Err(e),
                }
            }
            Err(err) => break Err(err),
        }
    };

    match upstream_resp {
        Ok(boot) => {
            converted_request = serde_json::to_string(&chat_req).unwrap_or_default();
            let resp_id = format!("resp_{}", &request_id[4..]);
            let (tx, rx) = mpsc::channel::<String>(256);

            let db = state.db.clone();
            let provider_name = config.name.clone();
            let model_clone = model.clone();
            let req_id = request_id.clone();
            let raw_req = raw_request.clone();
            let conv_req = converted_request.clone();
            let sent_messages = chat_req.messages.clone();
            let diagnostic_events = chat_req.diagnostic_events.clone();
            let sa_session = session_id.clone();
            let sa_provider = provider_id.clone();

            // Spawn task to process upstream SSE and send converted events
            tokio::spawn(async move {
                let mut acc = SseAccumulator::new(resp_id, model_clone.clone());
                acc.tool_call_resolution =
                    crate::transform::tool_calls::build_tool_call_resolution_map(&raw_req);

                let result = crate::gateway::sse::process_upstream_stream_inner(
                    boot,
                    tx.clone(),
                    &mut acc,
                    true,
                    true,
                )
                .await;

                // Record session affinity when the upstream confirmed a cache
                // hit (acc.usage was normalized to the Responses-shape during
                // stream processing — input_tokens_details.cached_tokens etc.).
                if let (Some(sid), Some(usage)) = (sa_session.as_ref(), acc.usage.as_ref()) {
                    crate::gateway::session_affinity::record_if_cache_hit(sid, &sa_provider, usage);
                }

                let latency = start.elapsed().as_millis() as i64;
                let tc_list = acc.tool_calls_list();
                let tool_calls_json = if tc_list.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(&tc_list.iter().map(|tc| {
                        json!({"id": tc.id, "name": tc.name, "arguments": tc.arguments})
                    }).collect::<Vec<_>>()).unwrap_or_default())
                };

                match result {
                    Ok(()) => {
                        // Store session for previous_response_id
                        {
                            type CM = crate::protocol::chat_completions::ChatMessage;
                            type TC = crate::protocol::chat_completions::ToolCall;
                            type Tcf = crate::protocol::chat_completions::ToolCallFunction;
                            let mut asst_msgs: Vec<CM> = vec![];
                            let rc_opt = if acc.reasoning_content.is_empty() {
                                None
                            } else {
                                Some(acc.reasoning_content.clone())
                            };
                            let tcs_opt = if tc_list.is_empty() {
                                None
                            } else {
                                Some(
                                    tc_list
                                        .iter()
                                        .map(|tc| TC {
                                            id: tc.id.clone(),
                                            call_type: "function".to_string(),
                                            function: Tcf {
                                                name: tc.name.clone(),
                                                arguments: tc.arguments.clone(),
                                            },
                                        })
                                        .collect(),
                                )
                            };
                            asst_msgs.push(CM {
                                role: "assistant".to_string(),
                                content: if acc.full_text.is_empty() {
                                    None
                                } else {
                                    Some(serde_json::Value::String(acc.full_text.clone()))
                                },
                                reasoning_content: rc_opt.clone(),
                                tool_calls: tcs_opt,
                                tool_call_id: None,
                                name: None,
                            });
                            let rc = if acc.reasoning_content.is_empty() {
                                None
                            } else {
                                Some(acc.reasoning_content.clone())
                            };
                            crate::gateway::session_store::store_turn(
                                &acc.response_id,
                                sent_messages,
                                asst_msgs,
                                rc,
                            );
                        }

                        // Bug #9 修复：trace 加 finish_reason / reasoning_tokens /
                        // truncated 字段，让 `agentgate logs` 能直接看出截断原因
                        // （而不是猜是 max_tokens 还是 AgentGate 自己挂了）。
                        let reasoning_tokens = acc
                            .usage
                            .as_ref()
                            .and_then(|u| u.get("output_tokens_details"))
                            .and_then(|d| d.get("reasoning_tokens"))
                            .and_then(|v| v.as_i64());
                        let truncated = matches!(
                            acc.finish_reason.as_deref(),
                            Some("length") | Some("max_tokens")
                        );
                        let trace = trace_with_degradation_events(
                            serde_json::json!({
                                "response_id": &acc.response_id,
                                "stream": true,
                                "text_len": acc.full_text.len(),
                                "tool_calls_count": tc_list.len(),
                                "reasoning_len": acc.reasoning_content.len(),
                                "finish_reason": acc.finish_reason.as_deref(),
                                "reasoning_tokens": reasoning_tokens,
                                "truncated": truncated,
                            }),
                            &diagnostic_events,
                        );
                        // Extract tokens from SSE usage
                        let (in_tok, out_tok) = acc
                            .usage
                            .as_ref()
                            .map(|u| {
                                (
                                    u.get("input_tokens").and_then(|v| v.as_i64()),
                                    u.get("output_tokens").and_then(|v| v.as_i64()),
                                )
                            })
                            .unwrap_or((None, None));
                        let (cache_w, cache_r) = acc
                            .usage
                            .as_ref()
                            .map(crate::storage::request_logs::extract_cache_tokens)
                            .unwrap_or((None, None));

                        log_request_success(
                            &db,
                            &client_type,
                            "/v1/responses",
                            &req_id,
                            &raw_req,
                            &conv_req,
                            "",
                            &truncate_str(&acc.full_text, 10000),
                            tool_calls_json.as_deref(),
                            &provider_name,
                            &model_clone,
                            200,
                            latency,
                            Some(&trace),
                            crate::gateway::usage::TokenUsage {
                                input: in_tok,
                                output: out_tok,
                                cache_write: cache_w,
                                cache_read: cache_r,
                                input_semantics:
                                    crate::gateway::usage::InputCacheSemantics::IncludesCacheRead,
                            },
                        );
                    }
                    Err(err_msg) => {
                        // 客户端断开记 499 + client disconnected,上游失败记 502。
                        let err = stream_task_error(&err_msg);
                        log_request_error_full(
                            &db,
                            &client_type,
                            "/v1/responses",
                            &req_id,
                            &raw_req,
                            &conv_req,
                            &provider_name,
                            &model_clone,
                            &err,
                            stream_error_status(&err),
                            latency,
                        );
                    }
                }
            });

            // Return SSE stream response
            let stream = ReceiverStream::new(rx);
            let body = Body::from_stream(tokio_stream::StreamExt::map(stream, |s| {
                Ok::<_, std::convert::Infallible>(s)
            }));

            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
                .body(body)
                .unwrap())
        }
        Err(err) => {
            let latency = start.elapsed().as_millis() as i64;
            let status = if err.code.starts_with("UPSTREAM") {
                502
            } else {
                500
            };
            log_request_error_full(
                &state.db,
                &client_type,
                "/v1/responses",
                &request_id,
                &raw_request,
                &converted_request,
                &config.name,
                &model,
                &err,
                status,
                latency,
            );
            Err(GatewayError(err))
        }
    }
}
