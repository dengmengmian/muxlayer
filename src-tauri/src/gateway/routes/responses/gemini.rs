use super::*;

// ── Gemini API handlers ──────────────────────────────────────

pub(super) async fn handle_gemini_non_stream_response(
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
    // Gemini's usage object doesn't currently expose a prompt-cache hit
    // counter, so the affinity record below is effectively a no-op today;
    // the params are wired for parity and forward-compat (if Gemini API
    // adds it later, we'll start writing affinity without further plumbing).
    let _ = (&session_id, &provider_id);
    let result = adapter::send_gemini_non_stream(&state.http_client, &config, &body, &model).await;

    match result {
        Ok(upstream_json) => {
            let resp_id = format!("resp_{}", &request_id[4..]);
            let mut output = Vec::new();
            let tool_call_resolution =
                crate::transform::tool_calls::build_tool_call_resolution_map(&raw_request);

            // Parse Gemini response: candidates[0].content.parts[]
            if let Some(candidate) = upstream_json
                .get("candidates")
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
            {
                let msg_id = format!("msg_{}", resp_id.replace("resp_", ""));
                let mut text_parts = Vec::new();

                if let Some(parts) = candidate
                    .get("content")
                    .and_then(|c| c.get("parts"))
                    .and_then(|p| p.as_array())
                {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            text_parts.push(text.to_string());
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let args = fc
                                .get("args")
                                .map(|a| a.to_string())
                                .unwrap_or("{}".to_string());
                            let call_id = format!("call_gemini_{}", output.len());
                            output.push(
                                crate::transform::tool_calls::responses_tool_call_item_from_chat_name(
                                    &format!("fc_{call_id}"),
                                    &call_id,
                                    name,
                                    &args,
                                    None,
                                    &tool_call_resolution,
                                ),
                            );
                        }
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
            let (in_tok, out_tok) = crate::gateway::usage::extract_gemini(&upstream_json);
            if let Some(ref sid) = session_id {
                if let Some(usage) = upstream_json.get("usageMetadata") {
                    crate::gateway::session_affinity::record_if_cache_hit(sid, &provider_id, usage);
                }
            }
            let trace =
                json!({"response_id": &resp_id, "stream": false, "protocol": "gemini"}).to_string();
            log_request_success(
                &state.db,
                &client_type,
                "/v1/responses",
                &request_id,
                &raw_request,
                &converted_request,
                &serde_json::to_string(&upstream_json).unwrap_or_default(),
                &serde_json::to_string(&responses_resp).unwrap_or_default(),
                None,
                &config.name,
                &model,
                200,
                latency,
                Some(&trace),
                crate::gateway::usage::TokenUsage {
                    input: in_tok,
                    output: out_tok,
                    cache_write: None,
                    cache_read: None,
                    input_semantics: crate::gateway::usage::InputCacheSemantics::IncludesCacheRead,
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

pub(super) async fn handle_gemini_stream_response(
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
    // See note in `handle_gemini_non_stream_response` — params wired for parity.
    let _ = (&session_id, &provider_id);
    let upstream_resp =
        adapter::send_gemini_stream(&state.http_client, &config, &body, &model).await;

    match upstream_resp {
        Ok(response) => {
            // Bootstrap-validate the stream before committing to forwarding.
            // Gemini occasionally returns 200 then immediately emits an error
            // JSON in the first SSE frame (e.g. quota / safety blocks); the
            // scan catches those and routes them through the standard
            // failover path instead of letting the client see a broken stream.
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

            tokio::spawn(async move {
                let mut acc = GeminiSseAccumulator::new(resp_id, model_clone.clone());
                acc.tool_call_resolution =
                    crate::transform::tool_calls::build_tool_call_resolution_map(&raw_req);
                let result =
                    crate::gateway::sse_gemini::process_gemini_stream(boot, tx, &mut acc).await;

                let latency = start.elapsed().as_millis() as i64;
                match result {
                    Ok(()) => {
                        let trace = json!({
                            "response_id": &acc.response_id, "stream": true, "protocol": "gemini",
                            "text_len": acc.full_text.len(), "tool_calls_count": acc.tool_calls.len(),
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
                                cache_write: None,
                                cache_read: None,
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
