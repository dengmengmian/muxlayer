//! 主转换入口与请求组装:Responses 请求 → ChatCompletionsRequest,
//! 含 MCP advisory 注入与同角色消息合并。

use serde_json::Value;

use super::effort::apply_effort_overrides;
use super::input::convert_input;
use crate::errors::AppError;
use crate::protocol::chat_completions::{
    CapabilityDegradationEvent, ChatCompletionsRequest, ChatMessage,
};
use crate::protocol::openai_responses::ResponsesRequest;
use crate::transform::providers::ProviderTransform;
use crate::transform::tool_calls;

pub fn convert_with_provider(
    req: &ResponsesRequest,
    model: &str,
    provider: &dyn ProviderTransform,
) -> Result<ChatCompletionsRequest, AppError> {
    convert_with_provider_matrix(req, model, provider, &Default::default())
}

/// Same as convert_with_provider but consults a per-model capability matrix
/// when emitting provider-builtin tools (e.g. MiMo's web_search). Production
/// gateway uses this so users can opt out of capabilities per model by
/// editing the matrix; tests can use the simpler 3-arg form.
pub fn convert_with_provider_matrix(
    req: &ResponsesRequest,
    model: &str,
    provider: &dyn ProviderTransform,
    matrix: &std::collections::HashMap<String, Vec<String>>,
) -> Result<ChatCompletionsRequest, AppError> {
    let mut messages = Vec::new();
    let mut diagnostic_events = Vec::new();

    // 0. Replay history from previous_response_id (session store)
    if let Some(ref prev_id) = req.previous_response_id {
        if let Some(history) = crate::gateway::session_store::get_history(prev_id) {
            messages.extend(history);
        }
    }

    // 1. instructions / system -> system message (only if not already present from history)
    let system_text = req.instructions.as_ref().or(req.system.as_ref());
    if let Some(text) = system_text {
        if !text.is_empty() {
            // Remove any existing system message from history to avoid duplication
            messages.retain(|m| m.role != "system");
            messages.insert(
                0,
                ChatMessage {
                    role: "system".to_string(),
                    content: Some(Value::String(text.clone())),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            );
        }
    }

    // 2. Convert input
    let input_messages = convert_input(&req.input, model, &mut diagnostic_events)?;
    messages.extend(input_messages);

    // 3. Convert tools (provider + matrix aware: Kimi $web_search builtin,
    //    MiMo web_search builtin gated by per-model capability matrix)
    let converted_tools = req
        .tools
        .as_ref()
        .map(|t| {
            tool_calls::convert_tools_with_matrix(
                t,
                provider.clean_schemas(),
                provider.provider_type(),
                model,
                matrix,
            )
        })
        .filter(|t| !t.is_empty());
    inject_mcp_advisory_if_needed(&mut messages, req.tools.as_deref(), &mut diagnostic_events);

    // 4. Convert tool_choice
    let tool_choice = req
        .tool_choice
        .as_ref()
        .map(tool_calls::convert_tool_choice);

    // 5. Provider-specific message processing (e.g. DeepSeek: fix order, strip images, inject reasoning)
    messages = provider.process_messages(messages)?;

    // 5.5 通用剥图兜底:能力矩阵显式声明该模型无 vision 时剥除图片块(留提示文本)。
    //     候选层 vision 过滤在"全部候选都不支持"时会放行原始顺序(failover.rs),
    //     这里是防 400 的最后一道闸。矩阵无该模型条目则不动(与 web_search 门控
    //     同语义);DeepSeek/MiMo 在各自 transform 已剥过,到这里天然幂等。
    let lacks_vision = matrix
        .get(tool_calls::model_base(model))
        .is_some_and(|caps| {
            !caps
                .iter()
                .any(|c| c == crate::providers::capabilities::CAP_VISION)
        });
    if lacks_vision {
        diagnostic_events.extend(
            crate::transform::degradation::strip_image_parts_with_notice(
                &mut messages,
                provider.provider_type(),
                provider.provider_type(),
                model,
                "To analyze images, switch to a vision-capable provider/model and re-send the request.",
            ),
        );
    }

    // 6. Merge consecutive messages of the same role
    //    (some providers reject user→user or assistant→assistant sequences)
    messages = merge_consecutive_messages(messages);

    // 7. Sanitize tool call arguments (invalid JSON -> "{}")，与入站 function_call
    //    共用 salvage_tool_arguments（覆盖 session history / 内嵌 tool_call 等来源）。
    for msg in &mut messages {
        if let Some(ref mut tcs) = msg.tool_calls {
            for tc in tcs {
                tc.function.arguments = tool_calls::salvage_tool_arguments(
                    &tc.function.arguments,
                    &tc.function.name,
                    &tc.id,
                    None,
                );
            }
        }
    }

    // Auto-inject stream_options for usage capture
    let stream_options = if req.stream.unwrap_or(false) {
        Some(serde_json::json!({"include_usage": true}))
    } else {
        None
    };

    // Convert reasoning.effort → reasoning_effort
    // "智能"=auto → None (provider default), "超高"=xhigh → "high"
    let reasoning_effort = req
        .reasoning
        .as_ref()
        .and_then(|r| r.get("effort"))
        .and_then(|e| e.as_str())
        .and_then(|e| provider.map_reasoning_effort(e));

    // A/扩展修复：两层 effort 兜底，按激进程度从弱到强：
    //
    // 1. AGENTGATE_FORCE_HIGH_EFFORT_PROVIDERS（fill）—— 仅当客户端没传时补 high。
    //    仅当客户端没传时补 high，不覆盖客户端意图。
    // 2. AGENTGATE_EFFORT_FLOOR_PROVIDERS（floor）—— 客户端传 low/medium 也强制升 high。
    //    覆盖客户端意图，最激进，针对 "Codex 显式传 medium 但模型仍偏好短回复" 场景。
    //
    // 两个 env 独立，可同时配置不同 provider_type 列表。
    let reasoning_effort = apply_effort_overrides(provider.provider_type(), reasoning_effort);

    // Convert text.format → response_format
    let response_format = req
        .text
        .as_ref()
        .and_then(|t| t.get("format"))
        .and_then(|f| {
            let fmt_type = f.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match fmt_type {
                "json_object" => Some(serde_json::json!({"type": "json_object"})),
                "json_schema" => Some(f.clone()),
                _ => None,
            }
        });

    let mut chat_req = ChatCompletionsRequest {
        model: model.to_string(),
        messages,
        tools: converted_tools,
        tool_choice,
        stream: req.stream.unwrap_or(false),
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens: req.max_output_tokens,
        // C 修复：同时填 max_completion_tokens（新标准）。MiMo / DeepSeek-thinking
        // 等都认这个；老 provider 不识别会被忽略，无副作用。两者同时填覆盖最广。
        max_completion_tokens: req.max_output_tokens,
        thinking: None,
        stream_options,
        response_format,
        reasoning_effort,
        seed: req.seed.clone(),
        stop: req.stop.clone(),
        frequency_penalty: req.frequency_penalty,
        presence_penalty: req.presence_penalty,
        parallel_tool_calls: req.parallel_tool_calls,
        diagnostic_events,
    };

    // 8. Provider-specific finalization (thinking, reasoning_effort, response_format overrides)
    let tools_clone = chat_req.tools.clone();
    provider.finalize_request(&mut chat_req, &tools_clone);

    Ok(chat_req)
}

fn inject_mcp_advisory_if_needed(
    messages: &mut Vec<ChatMessage>,
    tools: Option<&[Value]>,
    diagnostic_events: &mut Vec<CapabilityDegradationEvent>,
) {
    let Some(tools) = tools else {
        return;
    };
    let labels = collect_dropped_mcp_labels(tools);
    if labels.is_empty() {
        return;
    }
    diagnostic_events.push(crate::transform::degradation::mcp_advisory_event(&labels));
    let note = build_mcp_advisory_note(&labels);
    let insert_at = messages.iter().take_while(|m| m.role == "system").count();
    messages.insert(
        insert_at,
        ChatMessage {
            role: "system".to_string(),
            content: Some(Value::String(note)),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    );
}

fn collect_dropped_mcp_labels(tools: &[Value]) -> Vec<String> {
    let mut labels = Vec::new();
    for tool in tools {
        if tool.get("type").and_then(|t| t.as_str()) != Some("mcp") {
            continue;
        }
        let label = tool
            .get("server_label")
            .or_else(|| tool.get("connector_id"))
            .or_else(|| tool.get("server_url"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("unnamed MCP tool");
        labels.push(label.to_string());
    }
    labels.sort();
    labels.dedup();
    labels
}

fn build_mcp_advisory_note(labels: &[String]) -> String {
    let list = labels
        .iter()
        .map(|l| format!("\"{l}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut note = format!(
        "MuxLayer note: the user has OpenAI Responses MCP connector tool(s) enabled ({list}), \
         but this request is being converted to Chat Completions for the upstream provider. \
         That upstream does not implement OpenAI's MCP runtime, so these MCP connector tools are not callable here. \
         Do not pretend to call them. If the user asks for one of those connectors, explain that it is unavailable through this converted route and use an available shell/function alternative only if one is actually present."
    );

    let hints = connector_cli_hints(labels);
    if !hints.is_empty() {
        note.push_str(" Suggested command-line alternatives: ");
        note.push_str(&hints.join("; "));
        note.push('.');
    }
    note
}

fn connector_cli_hints(labels: &[String]) -> Vec<String> {
    let mut hints = Vec::new();
    for label in labels {
        let lower = label.to_ascii_lowercase();
        let hint = if lower.contains("github") {
            Some("GitHub: gh")
        } else if lower.contains("gmail")
            || lower.contains("google_drive")
            || lower.contains("google drive")
            || lower.contains("google-docs")
            || lower.contains("google docs")
        {
            Some("Google/Gmail/Drive: rclone or Google's official CLI tools")
        } else if lower.contains("dropbox") {
            Some("Dropbox: rclone or dropbox CLI")
        } else if lower.contains("canva") || lower.contains("heygen") {
            Some("Canva/HeyGen: provider REST API via curl when the user supplies credentials")
        } else {
            None
        };
        if let Some(hint) = hint {
            if !hints.iter().any(|h| h == hint) {
                hints.push(hint.to_string());
            }
        }
    }
    hints
}

/// 把一条消息的 content 规范成 Chat 多模态 parts 列表。字符串转成单个 text 块,
/// 数组原样保留,空/缺失返回空列表。用于合并含图片的同角色消息时不丢内容。
fn content_to_parts(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(s)) if !s.is_empty() => {
            vec![serde_json::json!({"type": "text", "text": s})]
        }
        Some(Value::Array(arr)) => arr.clone(),
        _ => Vec::new(),
    }
}

/// Merge consecutive messages of the same role (user+user, assistant+assistant).
/// Some providers reject consecutive same-role messages.
pub(super) fn merge_consecutive_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut result: Vec<ChatMessage> = Vec::new();

    for msg in messages {
        if let Some(last) = result.last_mut() {
            // Only merge if same role, no tool_calls, no tool_call_id
            if last.role == msg.role
                && last.tool_calls.is_none()
                && msg.tool_calls.is_none()
                && last.tool_call_id.is_none()
                && msg.tool_call_id.is_none()
                && (msg.role == "user" || msg.role == "system")
            {
                let last_is_str = last
                    .content
                    .as_ref()
                    .is_none_or(|c| c.is_string() || c.is_null());
                let new_is_str = msg
                    .content
                    .as_ref()
                    .is_none_or(|c| c.is_string() || c.is_null());
                if last_is_str && new_is_str {
                    // 两条都是纯文本:沿用 \n\n 拼接。
                    let last_text = last.content.as_ref().and_then(|c| c.as_str()).unwrap_or("");
                    let new_text = msg.content.as_ref().and_then(|c| c.as_str()).unwrap_or("");
                    if !new_text.is_empty() {
                        let merged = if last_text.is_empty() {
                            new_text.to_string()
                        } else {
                            format!("{last_text}\n\n{new_text}")
                        };
                        last.content = Some(Value::String(merged));
                    }
                } else {
                    // 任一条是多模态数组(图片等):合并成 parts 数组,绝不丢内容。
                    // as_str() 对数组返回 None,旧逻辑会把整条消息连同图片和文字
                    // 一起吞掉,所以这里必须按 parts 合并。
                    let mut parts = content_to_parts(last.content.as_ref());
                    parts.extend(content_to_parts(msg.content.as_ref()));
                    last.content = Some(Value::Array(parts));
                }
                continue;
            }
        }
        result.push(msg);
    }

    result
}
