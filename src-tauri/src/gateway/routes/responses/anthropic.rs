use super::*;

// ── Anthropic (Claude Messages API) handlers ──────────────────

pub(super) async fn handle_anthropic_non_stream_response(
    state: GatewayState,
    config: ProviderConfig,
    body: serde_json::Value,
    request_id: String,
    raw_request: String,
    converted_request: String,
    model: String,
    start: Instant,
    client_type: String,
    session_id: Option<String>,
    provider_id: String,
) -> Result<Response, GatewayError> {
    let result = adapter::send_anthropic_non_stream(&state.http_client, &config, &body).await;

    match result {
        Ok(upstream_json) => {
            let resp_id = format!("resp_{}", &request_id[4..]);

            // Parse Claude response: {content: [...], stop_reason, usage}
            let mut output = Vec::new();
            let tool_calls_json = String::new();

            if let Some(content) = upstream_json.get("content").and_then(|c| c.as_array()) {
                let msg_id = format!("msg_{}", resp_id.replace("resp_", ""));
                let mut text_parts = Vec::new();
                let tool_call_resolution =
                    crate::transform::tool_calls::build_tool_call_resolution_map(&raw_request);

                for block in content {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                text_parts.push(text.to_string());
                            }
                        }
                        "tool_use" => {
                            let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let empty_input = json!({});
                            let input = block.get("input").unwrap_or(&empty_input);
                            let arguments =
                                serde_json::to_string(input).unwrap_or("{}".to_string());
                            output.push(
                                crate::transform::tool_calls::responses_tool_call_item_from_chat_name(
                                    &format!("fc_{id}"),
                                    id,
                                    name,
                                    &arguments,
                                    None,
                                    &tool_call_resolution,
                                ),
                            );
                        }
                        _ => {}
                    }
                }

                if !text_parts.is_empty() {
                    let full_text = text_parts.join("");
                    output.insert(
                        0,
                        json!({
                            "id": msg_id,
                            "type": "message",
                            "status": "completed",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": full_text}]
                        }),
                    );
                }
            }

            let responses_resp = json!({
                "id": resp_id,
                "object": "response",
                "created_at": chrono::Utc::now().timestamp(),
                "status": "completed",
                "model": model,
                "output": output
            });
            let latency = start.elapsed().as_millis() as i64;
            let (in_tok, out_tok) = crate::gateway::usage::extract_anthropic(&upstream_json);
            let (cache_w, cache_r) = upstream_json
                .get("usage")
                .map(crate::storage::request_logs::extract_cache_tokens)
                .unwrap_or((None, None));

            // Record session affinity on Anthropic cache_read_input_tokens hit.
            if let Some(ref sid) = session_id {
                if let Some(usage) = upstream_json.get("usage") {
                    crate::gateway::session_affinity::record_if_cache_hit(sid, &provider_id, usage);
                }
            }

            let trace =
                json!({"response_id": &resp_id, "stream": false, "protocol": "anthropic_messages"})
                    .to_string();
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
                    input_semantics: crate::gateway::usage::InputCacheSemantics::ExcludesCache,
                },
            );

            Ok(Json(responses_resp).into_response())
        }
        Err(err) => {
            let latency = start.elapsed().as_millis() as i64;
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
                502,
                latency,
            );
            Err(GatewayError(err))
        }
    }
}

pub(super) async fn handle_anthropic_stream_response(
    state: GatewayState,
    config: ProviderConfig,
    body: serde_json::Value,
    request_id: String,
    raw_request: String,
    converted_request: String,
    model: String,
    start: Instant,
    client_type: String,
    session_id: Option<String>,
    provider_id: String,
) -> Result<Response, GatewayError> {
    let upstream_resp = adapter::send_anthropic_stream(&state.http_client, &config, &body).await;

    match upstream_resp {
        Ok(response) => {
            // Bootstrap-validate before committing to streaming so HTTP-200-
            // with-error-frame failures (Anthropic overload / rate-limit
            // events) become a clean Err that triggers failover.
            let boot = match crate::gateway::sse_bootstrap::bootstrap_detect(response).await {
                Ok(b) => b,
                Err(e) => return Err(GatewayError(e)),
            };

            let resp_id = format!("resp_{}", &request_id[4..]);
            let (tx, rx) = mpsc::channel::<String>(256);

            let db = state.db.clone();
            let provider_name = config.name.clone();
            let model_clone = model.clone();
            let req_id = request_id.clone();
            let raw_req = raw_request.clone();
            let conv_req = converted_request.clone();
            let sa_session = session_id.clone();
            let sa_provider = provider_id.clone();

            tokio::spawn(async move {
                let mut acc = AnthropicSseAccumulator::new(resp_id, model_clone.clone());
                acc.tool_call_resolution =
                    crate::transform::tool_calls::build_tool_call_resolution_map(&raw_req);
                let result =
                    crate::gateway::sse_anthropic::process_anthropic_stream(boot, tx, &mut acc)
                        .await;

                let latency = start.elapsed().as_millis() as i64;
                let tc_list = acc.tool_calls_list();

                if let (Some(sid), Some(usage)) = (sa_session.as_ref(), acc.usage.as_ref()) {
                    crate::gateway::session_affinity::record_if_cache_hit(sid, &sa_provider, usage);
                }

                match result {
                    Ok(()) => {
                        // Bug #9 修复：anthropic stream 路径也加观测字段保持一致。
                        // stop_reason 是 Anthropic 自己术语（end_turn/max_tokens/...）。
                        let truncated = matches!(
                            acc.stop_reason.as_deref(),
                            Some("max_tokens") | Some("length")
                        );
                        let trace = json!({
                            "response_id": &acc.response_id, "stream": true, "protocol": "anthropic_messages",
                            "text_len": acc.full_text.len(), "tool_calls_count": tc_list.len(),
                            "reasoning_len": acc.reasoning_content.len(),
                            "stop_reason": acc.stop_reason.as_deref(),
                            "truncated": truncated,
                        }).to_string();
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
                            None,
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
                                    crate::gateway::usage::InputCacheSemantics::ExcludesCache,
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
                502,
                latency,
            );
            Err(GatewayError(err))
        }
    }
}
