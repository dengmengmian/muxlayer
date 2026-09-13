//! input items / 消息提取:把 Responses 的 input(string/array/object)转成
//! ChatMessage 列表,含 function_call 配对、reasoning 回填、内容与工具输出展平。

use serde_json::Value;

use crate::errors::AppError;
use crate::protocol::chat_completions::{
    CapabilityDegradationEvent, ChatMessage, ToolCall, ToolCallFunction,
};
use crate::transform::reasoning_store;

/// `model` 是目标上游模型，用作 reasoning_store 的分区键（与响应侧 store 一致）。
pub(super) fn convert_input(
    input: &Value,
    model: &str,
    diagnostic_events: &mut Vec<CapabilityDegradationEvent>,
) -> Result<Vec<ChatMessage>, AppError> {
    match input {
        Value::String(s) => Ok(vec![msg("user", Value::String(s.clone()))]),
        Value::Array(items) => convert_input_array(items, model, diagnostic_events),
        Value::Object(_) => {
            let content = extract_content(Some(input));
            Ok(vec![msg("user", content)])
        }
        _ => Ok(vec![msg("user", Value::String(input.to_string()))]),
    }
}

pub(super) fn convert_input_array(
    items: &[Value],
    model: &str,
    diagnostic_events: &mut Vec<CapabilityDegradationEvent>,
) -> Result<Vec<ChatMessage>, AppError> {
    if !items.is_empty() && items.iter().all(is_content_part) {
        return Ok(vec![msg(
            "user",
            extract_content(Some(&Value::Array(items.to_vec()))),
        )]);
    }

    let mut messages = Vec::new();
    let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
    let mut pending_reasoning: Option<String> = None;

    for item in items {
        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match item_type {
            "message" => {
                flush_tool_calls(
                    &mut messages,
                    &mut pending_tool_calls,
                    &mut pending_reasoning,
                    model,
                );

                let role = map_role(item.get("role").and_then(|r| r.as_str()).unwrap_or("user"));

                // Check for embedded tool_calls in content array (Codex multi-turn history format)
                let mut embedded_text = String::new();
                let mut embedded_tool_calls: Vec<ToolCall> = Vec::new();
                let mut has_embedded_tool_calls = false;
                if let Some(Value::Array(parts)) = item.get("content") {
                    for part in parts {
                        let pt = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match pt {
                            "input_text" | "output_text" | "text" => {
                                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                                    if !embedded_text.is_empty() {
                                        embedded_text.push('\n');
                                    }
                                    embedded_text.push_str(t);
                                }
                            }
                            "tool_call" => {
                                has_embedded_tool_calls = true;
                                embedded_tool_calls.push(ToolCall {
                                    id: part
                                        .get("id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    call_type: "function".to_string(),
                                    function: ToolCallFunction {
                                        name: part
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        arguments: part
                                            .get("arguments")
                                            .map(|a| {
                                                if a.is_string() {
                                                    a.as_str().unwrap().to_string()
                                                } else {
                                                    a.to_string()
                                                }
                                            })
                                            .unwrap_or_default(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                }

                let content = if has_embedded_tool_calls {
                    Value::String(embedded_text)
                } else {
                    extract_content(item.get("content"))
                };

                // reasoning_content: from item itself, or pending, or look up from store
                let reasoning = if role == "assistant" {
                    item.get("reasoning_content")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .filter(|s| !s.is_empty())
                        .or_else(|| pending_reasoning.take())
                        .or_else(|| {
                            // Look up from reasoning store by content hash
                            let text = extract_content(item.get("content"));
                            let text_str = text.as_str().unwrap_or("");
                            reasoning_store::lookup_by_content(model, text_str)
                        })
                } else {
                    None
                };

                messages.push(ChatMessage {
                    role,
                    content: Some(content),
                    reasoning_content: reasoning,
                    tool_calls: if embedded_tool_calls.is_empty() {
                        None
                    } else {
                        Some(embedded_tool_calls)
                    },
                    tool_call_id: None,
                    name: None,
                });
            }
            "custom_tool_call" => {
                if let Some(rc) = item.get("reasoning_content").and_then(|v| v.as_str()) {
                    if !rc.is_empty() && pending_reasoning.is_none() {
                        pending_reasoning = Some(rc.to_string());
                    }
                }

                let raw_call_id = item
                    .get("call_id")
                    .and_then(|c| c.as_str())
                    .unwrap_or("call_unknown");
                let call_id =
                    crate::transform::tool_calls::sanitize_call_id(raw_call_id).into_owned();
                let name = item
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = item
                    .get("input")
                    .map(|v| {
                        if let Some(s) = v.as_str() {
                            s.to_string()
                        } else {
                            v.to_string()
                        }
                    })
                    .unwrap_or_default();
                pending_tool_calls.push(ToolCall {
                    id: call_id,
                    call_type: "function".to_string(),
                    function: ToolCallFunction {
                        name,
                        arguments: crate::transform::tool_calls::custom_tool_arguments_from_input(
                            &input,
                        ),
                    },
                });
            }
            "function_call" => {
                // Capture reasoning_content from function_call items (DeepSeek thinking mode)
                if let Some(rc) = item.get("reasoning_content").and_then(|v| v.as_str()) {
                    if !rc.is_empty() && pending_reasoning.is_none() {
                        pending_reasoning = Some(rc.to_string());
                    }
                }

                let raw_call_id = item
                    .get("call_id")
                    .and_then(|c| c.as_str())
                    .unwrap_or("call_unknown");
                let call_id =
                    crate::transform::tool_calls::sanitize_call_id(raw_call_id).into_owned();
                let name = item
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let raw_arguments = item
                    .get("arguments")
                    .map(|a| {
                        if a.is_string() {
                            a.as_str().unwrap().to_string()
                        } else {
                            a.to_string()
                        }
                    })
                    .unwrap_or_default();
                // #4 修复：入站 history function_call.arguments 校验 JSON 合法性。
                // 客户端历史里可能带上一轮被截断的半截 args，原样发上游 → 严格
                // provider 400 "unexpected end of data"。salvage 成 {} 让对话能继续。
                // 与 sse.rs 出站方向对称。
                let arguments = crate::transform::tool_calls::salvage_tool_arguments(
                    &raw_arguments,
                    &name,
                    &call_id,
                    None,
                );

                pending_tool_calls.push(ToolCall {
                    id: call_id,
                    call_type: "function".to_string(),
                    function: ToolCallFunction { name, arguments },
                });
            }
            "function_call_output" | "custom_tool_call_output" => {
                // Flush pending tool calls before adding tool response
                flush_tool_calls(
                    &mut messages,
                    &mut pending_tool_calls,
                    &mut pending_reasoning,
                    model,
                );

                let call_id = item.get("call_id").and_then(|c| c.as_str());

                if call_id.is_none() || call_id == Some("") {
                    return Err(AppError::new(
                        crate::errors::codes::FUNCTION_CALL_OUTPUT_ID_MISSING,
                        "function_call_output is missing call_id",
                    ).with_suggestion("Each function_call_output must have a call_id matching a previous function_call"));
                }

                let raw_output = item
                    .get("output")
                    .map(|o| flatten_tool_output_with_events(o, diagnostic_events))
                    .unwrap_or_default();
                let output = Value::String(raw_output);

                let sanitized_id =
                    crate::transform::tool_calls::sanitize_call_id(call_id.unwrap()).into_owned();
                messages.push(ChatMessage {
                    role: "tool".to_string(),
                    content: Some(output),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: Some(sanitized_id),
                    name: None,
                });
            }
            "compaction" | "context_compaction" | "compaction_summary" => {
                // Codex auto-compact: convert summary to user message.
                // 取 `summary` / `content` 文本;无法还原时兜底占位。
                flush_tool_calls(
                    &mut messages,
                    &mut pending_tool_calls,
                    &mut pending_reasoning,
                    model,
                );
                let summary = item
                    .get("summary")
                    .or(item.get("content"))
                    .map(|v| extract_content(Some(v)))
                    .unwrap_or(Value::String("[context compacted]".to_string()));
                messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: Some(summary),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });
            }
            "reasoning" => {
                // Reasoning round-trip priority:
                //   1. `encrypted_content` — full uncondensed trace pinned by the
                //      gateway on the previous response. Codex echoes it back
                //      verbatim, so this survives even when summary[] is truncated.
                //      Critical for MiMo / DeepSeek thinking-mode multi-turn tool
                //      calls (upstream 400s if the assistant turn that carried
                //      tool_calls is missing its reasoning_content).
                //   2. `content` — legacy field used by some clients.
                //   3. `summary[].text` — Codex's short summary, lossy fallback.
                if let Some(rc) = item.get("encrypted_content").and_then(|v| v.as_str()) {
                    if !rc.is_empty() {
                        pending_reasoning = Some(rc.to_string());
                    }
                }
                if pending_reasoning.is_none() {
                    if let Some(rc) = item.get("content").and_then(|v| v.as_str()) {
                        if !rc.is_empty() {
                            pending_reasoning = Some(rc.to_string());
                        }
                    }
                }
                if pending_reasoning.is_none() {
                    if let Some(rc) = item.get("summary").and_then(|v| {
                        if v.is_string() {
                            v.as_str().map(String::from)
                        } else if v.is_array() {
                            let texts: Vec<String> = v
                                .as_array()
                                .unwrap()
                                .iter()
                                .filter_map(|p| {
                                    p.get("text").and_then(|t| t.as_str()).map(String::from)
                                })
                                .collect();
                            if texts.is_empty() {
                                None
                            } else {
                                Some(texts.join(""))
                            }
                        } else {
                            None
                        }
                    }) {
                        if !rc.is_empty() {
                            pending_reasoning = Some(rc);
                        }
                    }
                }
            }
            _ => {
                // Unknown item: try to extract as message if it has role/content
                if let Some(role) = item.get("role").and_then(|r| r.as_str()) {
                    flush_tool_calls(
                        &mut messages,
                        &mut pending_tool_calls,
                        &mut pending_reasoning,
                        model,
                    );
                    let content = extract_content(item.get("content"));
                    messages.push(ChatMessage {
                        role: map_role(role),
                        content: Some(content),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
                // else: silently skip
            }
        }
    }

    // Flush remaining pending tool calls
    flush_tool_calls(
        &mut messages,
        &mut pending_tool_calls,
        &mut pending_reasoning,
        model,
    );

    Ok(messages)
}

fn flush_tool_calls(
    messages: &mut Vec<ChatMessage>,
    pending: &mut Vec<ToolCall>,
    reasoning: &mut Option<String>,
    model: &str,
) {
    if pending.is_empty() {
        return;
    }
    // Try to find reasoning from store by tool_call_id if not already available
    let rc = reasoning.take().or_else(|| {
        for tc in pending.iter() {
            if let Some(r) = reasoning_store::lookup_by_tool_call_id(model, &tc.id) {
                return Some(r);
            }
        }
        None
    });
    // Codex 把"assistant 说一句"和紧随的 function_call 作为**两个独立 item** 下发,
    // 直译会变成两条连续 assistant 消息(一条纯文本、一条纯 tool_calls)。这在 Chat
    // Completions 里不标准——标准是一条 assistant 同时带 content + tool_calls。部分模型
    // (实测 MiMo)在这种拆开的历史下会"看不懂上一轮的动作链",大概率只回一句计划就
    // stop、不接着调工具(实测:拆开 1/10 调工具,合并 8/10)。所以:若上一条正是
    // assistant 纯文本(有 content、无 tool_calls),把本批 tool_calls 合并进去成一条。
    if let Some(last) = messages.last_mut() {
        if last.role == "assistant"
            && last.tool_calls.is_none()
            && last.content.as_ref().is_some_and(|c| !c.is_null())
        {
            last.tool_calls = Some(std::mem::take(pending));
            if last.reasoning_content.is_none() {
                last.reasoning_content = rc;
            }
            return;
        }
    }
    messages.push(ChatMessage {
        role: "assistant".to_string(),
        content: None,
        reasoning_content: rc,
        tool_calls: Some(std::mem::take(pending)),
        tool_call_id: None,
        name: None,
    });
}

/// Flatten tool output to a string.
/// Chat Completions tool role only accepts `content: string`, but Codex Responses API
/// may send `output` as a ContentPart array (e.g. when a tool returns images + text).
/// We extract text parts, drop images with a placeholder notice.
pub fn flatten_tool_output(output: &Value) -> String {
    let mut events = Vec::new();
    flatten_tool_output_with_events(output, &mut events)
}

pub(super) fn flatten_tool_output_with_events(
    output: &Value,
    diagnostic_events: &mut Vec<CapabilityDegradationEvent>,
) -> String {
    if let Some(s) = output.as_str() {
        return s.to_string();
    }
    if let Some(parts) = output.as_array() {
        let mut chunks = Vec::new();
        let mut dropped_images = 0u32;
        for part in parts {
            let pt = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match pt {
                "input_text" | "output_text" | "text" => {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            chunks.push(text.to_string());
                        }
                    }
                }
                "input_image" | "image_url" => {
                    dropped_images += 1;
                }
                _ => {}
            }
        }
        if dropped_images > 0 {
            // Chat Completions 协议本身不支持 tool 角色消息含图——这是协议限制
            // 不是 AgentGate 的 bug。但应用层应该知道：tool 返回的截图/图表
            // 等视觉信息丢了，模型只能看到"image omitted"占位符。warn 一行
            // 留给排查"为什么模型没看到我的图"。
            eprintln!(
                "[transform] {dropped_images} image attachment(s) dropped from tool output \
                 (Chat Completions protocol does not support images in tool messages)"
            );
            diagnostic_events.push(
                crate::transform::degradation::tool_output_image_omitted_event(
                    dropped_images as usize,
                ),
            );
            chunks.push(
                crate::transform::degradation::tool_output_image_omitted_notice(
                    dropped_images as usize,
                ),
            );
        }
        if chunks.is_empty() {
            return output.to_string();
        }
        return chunks.join("");
    }
    output.to_string()
}

pub(super) fn msg(role: &str, content: Value) -> ChatMessage {
    ChatMessage {
        role: role.to_string(),
        content: Some(content),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

pub(super) fn map_role(role: &str) -> String {
    match role {
        "developer" => "system".to_string(),
        other => other.to_string(),
    }
}

fn is_content_part(part: &Value) -> bool {
    matches!(
        part.get("type").and_then(|t| t.as_str()),
        Some("input_text" | "output_text" | "text" | "input_image" | "image_url")
    )
}

pub(super) fn extract_content(content: Option<&Value>) -> Value {
    match content {
        None => Value::String(String::new()),
        Some(Value::String(s)) => Value::String(s.clone()),
        Some(Value::Array(arr)) => {
            let mut parts_out: Vec<Value> = Vec::new();
            let mut has_image = false;

            for part in arr {
                let pt = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match pt {
                    "input_text" | "output_text" | "text" => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            parts_out.push(serde_json::json!({"type": "text", "text": text}));
                        }
                    }
                    "input_image" => {
                        // Convert Responses API input_image to Chat Completions image_url format
                        has_image = true;
                        // Responses 协议 detail 字段在 input_image 顶层（"auto"/"low"/"high"）；
                        // Chat 协议把它塞在 image_url 对象里。两种位置都收，保留进出参一致。
                        let detail = part.get("detail").and_then(|d| d.as_str()).or_else(|| {
                            part.get("image_url")
                                .and_then(|u| u.get("detail"))
                                .and_then(|d| d.as_str())
                        });
                        let url = part.get("image_url").and_then(|u| u.as_str()).or_else(|| {
                            part.get("image_url")
                                .and_then(|u| u.get("url"))
                                .and_then(|u| u.as_str())
                        });
                        if let Some(url) = url {
                            let mut image_url = serde_json::json!({"url": url});
                            if let Some(d) = detail {
                                image_url["detail"] = serde_json::json!(d);
                            }
                            parts_out.push(serde_json::json!({
                                "type": "image_url",
                                "image_url": image_url,
                            }));
                        }
                    }
                    "image_url" => {
                        // Already in Chat Completions format, pass through
                        has_image = true;
                        parts_out.push(part.clone());
                    }
                    _ => {
                        // Fallback: try "text" field
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            parts_out.push(serde_json::json!({"type": "text", "text": text}));
                        }
                    }
                }
            }

            if parts_out.is_empty() {
                Value::String(serde_json::to_string(arr).unwrap_or_default())
            } else if has_image {
                // Return multipart content array to preserve images
                Value::Array(parts_out)
            } else {
                // Text-only: join into a single string for compatibility
                let text = parts_out
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("");
                Value::String(text)
            }
        }
        Some(Value::Object(obj)) => {
            if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
                Value::String(text.to_string())
            } else {
                Value::String(serde_json::to_string(obj).unwrap_or_default())
            }
        }
        Some(other) => Value::String(other.to_string()),
    }
}
