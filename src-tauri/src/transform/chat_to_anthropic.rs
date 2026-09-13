use serde_json::{json, Value};

use crate::errors::AppError;
use crate::protocol::chat_completions::ChatCompletionsRequest;

/// Convert a Chat Completions request into an Anthropic Messages request body.
///
/// 关键差异：
/// - `system` role 消息从 messages 拆到顶层 `system` 字段
/// - `tools` 形态 `{type:"function", function:{name, description, parameters}}` → `{name, description, input_schema}`
/// - `tool_choice` 反向映射（参考 protocol::anthropic_messages::tool_choice_to_chat）
/// - `reasoning_effort` 字符串 → `thinking.budget_tokens` 数值
/// - `messages[role=="tool"]` → `{role:"user", content:[{type:"tool_result", ...}]}`
/// - 图像 `{type:"image_url", image_url:{url:"data:..."}}` → `{type:"image", source:{type:"base64", ...}}`
/// - 图像 `http(s)://...` → `{type:"image", source:{type:"url", url}}`（Anthropic 官方支持）
/// - 其它 URL 形态：插入 text 提示，避免静默丢图
/// - `max_tokens` Anthropic 必填，缺省给 8192
/// - `temperature` Anthropic 上限 1.0，超过 clamp
pub fn convert(req: &ChatCompletionsRequest) -> Result<Value, AppError> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    for msg in &req.messages {
        match msg.role.as_str() {
            "system" | "developer" => {
                if let Some(text) = extract_string_content(msg.content.as_ref()) {
                    if !text.is_empty() {
                        system_parts.push(text);
                    }
                }
            }
            "user" => {
                let blocks = user_content_to_blocks(msg.content.as_ref());
                if !blocks.is_empty() {
                    messages.push(json!({"role": "user", "content": blocks}));
                }
            }
            "assistant" => {
                let mut content_blocks: Vec<Value> = Vec::new();
                if let Some(text) = extract_string_content(msg.content.as_ref()) {
                    if !text.is_empty() {
                        content_blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        let input: Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                        content_blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input,
                        }));
                    }
                }
                if !content_blocks.is_empty() {
                    messages.push(json!({"role": "assistant", "content": content_blocks}));
                }
            }
            "tool" => {
                // Chat 的 tool 消息 → Anthropic 的 user 消息 + tool_result 块
                let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
                let content = extract_string_content(msg.content.as_ref()).unwrap_or_default();
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content,
                    }],
                }));
            }
            _ => {
                // 未知 role，按 user 处理但不丢
                let blocks = user_content_to_blocks(msg.content.as_ref());
                if !blocks.is_empty() {
                    messages.push(json!({"role": "user", "content": blocks}));
                }
            }
        }
    }

    // Anthropic 严格 user/assistant 交替；连续同 role 合并
    let messages = merge_consecutive_same_role(messages);

    // Anthropic max_tokens 必填；Chat 没给时用 8192（与 responses_to_anthropic 一致）
    let max_tokens = req.max_tokens.unwrap_or(8192);
    let mut body = json!({
        "model": req.model,
        "max_tokens": max_tokens,
        "messages": messages,
    });

    if !system_parts.is_empty() {
        body["system"] = json!(system_parts.join("\n\n"));
    }

    // tools：Chat 形态 → Anthropic 形态
    if let Some(ref tools) = req.tools {
        let converted = convert_tools(tools);
        if !converted.is_empty() {
            body["tools"] = json!(converted);
        }
    }

    // tool_choice：Chat → Anthropic
    if let Some(ref tc) = req.tool_choice {
        if let Some(converted) = convert_tool_choice(tc) {
            body["tool_choice"] = converted;
        }
    }

    // reasoning_effort → thinking：复用 responses_to_anthropic 的模型族规则
    // （adaptive / haiku 不开 / 强制工具不开 / budget clamp）；Chat 路径只在显式
    // reasoning_effort 时开思考，不因模型族强开。
    let forced_tool_choice = body
        .get("tool_choice")
        .and_then(|tc| tc.get("type"))
        .and_then(|t| t.as_str())
        .is_some_and(|t| t == "any" || t == "tool");
    let thinking = req.reasoning_effort.as_deref().and_then(|effort| {
        crate::transform::responses_to_anthropic::convert_thinking(
            &Some(json!({"effort": effort})),
            &req.model,
            max_tokens,
            forced_tool_choice,
        )
    });
    let thinking_on = thinking.is_some();
    if let Some(thinking) = thinking {
        body["thinking"] = thinking;
    }

    body["stream"] = json!(req.stream);
    // thinking 开启时 Anthropic 不接受自定义 temperature / top_p（400），与
    // responses_to_anthropic 一致：开思考时一律省略。
    if !thinking_on {
        if let Some(temp) = req.temperature {
            // Anthropic temperature 上限 1.0；OpenAI 上限 2.0。clamp 静默修正。
            body["temperature"] = json!(temp.clamp(0.0, 1.0));
        }
        if let Some(top_p) = req.top_p {
            body["top_p"] = json!(top_p.clamp(0.0, 1.0));
        }
    }
    if let Some(ref stop) = req.stop {
        body["stop_sequences"] = stop.clone();
    }

    Ok(body)
}

/// 把 ChatMessage.content 拍成纯字符串。content 可能是：
/// - Value::String → 直接返回
/// - Value::Array (multimodal) → 拼接所有 text/input_text 块
/// - None → None
fn extract_string_content(content: Option<&Value>) -> Option<String> {
    let c = content?;
    match c {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => {
            let text = arr
                .iter()
                .filter_map(|p| {
                    let t = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if t == "text" || t == "input_text" {
                        p.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

/// Chat user content → Anthropic content blocks。
///
/// 处理 multimodal：Chat 的 `image_url:{url:"data:<mt>;base64,<data>"}` → Anthropic
/// `{type:"image", source:{type:"base64", media_type, data}}`。
fn user_content_to_blocks(content: Option<&Value>) -> Vec<Value> {
    let Some(c) = content else { return Vec::new() };
    match c {
        Value::String(s) => {
            if s.is_empty() {
                Vec::new()
            } else {
                vec![json!({"type": "text", "text": s})]
            }
        }
        Value::Array(arr) => {
            let mut out: Vec<Value> = Vec::new();
            for part in arr {
                let pt = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match pt {
                    "text" | "input_text" => {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            if !t.is_empty() {
                                out.push(json!({"type": "text", "text": t}));
                            }
                        }
                    }
                    "image_url" => {
                        let url = part
                            .get("image_url")
                            .and_then(|u| u.get("url"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        if let Some(block) = image_url_to_anthropic_block(url) {
                            out.push(block);
                        } else if !url.is_empty() {
                            // 无法转换时显式提示，禁止静默丢图。
                            out.push(json!({
                                "type": "text",
                                "text": format!(
                                    "[Note: image omitted — MuxLayer could not convert this image URL for Anthropic. Use a data: base64 URL or http(s) URL. Original: {}]",
                                    truncate_for_notice(url, 120)
                                ),
                            }));
                        }
                    }
                    _ => {
                        // 未知 part：text 字段如果有就保留
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            if !t.is_empty() {
                                out.push(json!({"type": "text", "text": t}));
                            }
                        }
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Chat `image_url` → Anthropic image block。
/// - `data:<media>;base64,<data>` → base64 source
/// - `http://` / `https://` → url source（Anthropic 官方支持）
fn image_url_to_anthropic_block(url: &str) -> Option<Value> {
    if let Some((media_type, data)) = parse_data_url(url) {
        return Some(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
            },
        }));
    }
    if is_http_url(url) {
        return Some(json!({
            "type": "image",
            "source": {
                "type": "url",
                "url": url,
            },
        }));
    }
    None
}

fn is_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn truncate_for_notice(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

/// `data:<media_type>;base64,<data>` 解析。
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (header, data) = rest.split_once(',')?;
    let media_type = header.split(';').next()?;
    if media_type.is_empty() {
        return None;
    }
    Some((media_type.to_string(), data.to_string()))
}

/// 合并相邻同 role 的消息，保证 user/assistant 交替。
fn merge_consecutive_same_role(messages: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    for msg in messages {
        let role = msg
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(last) = out.last_mut() {
            let last_role = last.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if last_role == role {
                // 合并 content 数组（原地 extend，不整组 clone）
                if let (Some(la), Some(na)) = (
                    last.get_mut("content").and_then(|c| c.as_array_mut()),
                    msg.get("content").and_then(|c| c.as_array()),
                ) {
                    la.extend(na.iter().cloned());
                    continue;
                }
            }
        }
        out.push(msg);
    }
    out
}

/// Chat tools (`{type:"function", function:{name, description, parameters}}`)
/// → Anthropic tools (`{name, description, input_schema}`).
fn convert_tools(tools: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(tools.len());
    for tool in tools {
        // Chat 形态：嵌在 function 子对象里
        let function = tool.get("function").unwrap_or(tool);
        let name = function.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let desc = function
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let input_schema = function
            .get("parameters")
            .cloned()
            .unwrap_or(json!({"type": "object"}));
        out.push(json!({
            "name": name,
            "description": desc,
            "input_schema": input_schema,
        }));
    }
    crate::transform::tool_calls::dedupe_tools_by_name(out)
}

/// Chat tool_choice → Anthropic tool_choice。
///
/// 映射（`tool_choice_to_chat` 的反向）：
/// - `"auto"`                          → `{type:"auto"}`
/// - `"required"`                      → `{type:"any"}`
/// - `"none"`                          → None（Anthropic 没 `type:"none"`，直接不发 tools 也行；
///                                         这里返回 None 让顶层决定是否发 tool_choice 字段）
/// - `{type:"function", function:{name:"X"}}` → `{type:"tool", name:"X"}`
fn convert_tool_choice(tc: &Value) -> Option<Value> {
    if let Some(s) = tc.as_str() {
        return match s {
            "auto" => Some(json!({"type": "auto"})),
            "required" => Some(json!({"type": "any"})),
            "none" => None,
            _ => None,
        };
    }
    let obj = tc.as_object()?;
    let kind = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match kind {
        "function" => {
            let name = obj
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if name.is_empty() {
                None
            } else {
                Some(json!({"type": "tool", "name": name}))
            }
        }
        // 已经是 Anthropic 形态原样透传
        "auto" | "any" | "tool" => Some(tc.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::chat_completions::{ChatMessage, ToolCall, ToolCallFunction};
    use serde_json::json;

    fn base_req() -> ChatCompletionsRequest {
        ChatCompletionsRequest {
            model: "claude-3".into(),
            messages: vec![],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            thinking: None,
            stream_options: None,
            response_format: None,
            reasoning_effort: None,
            seed: None,
            stop: None,
            frequency_penalty: None,
            presence_penalty: None,
            parallel_tool_calls: None,
            diagnostic_events: Vec::new(),
        }
    }

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            content: Some(json!(text)),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    #[test]
    fn system_messages_promoted_to_top_level() {
        let mut req = base_req();
        req.messages = vec![
            ChatMessage {
                role: "system".into(),
                content: Some(json!("You are helpful")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            user_msg("hi"),
        ];
        let body = convert(&req).unwrap();
        assert_eq!(body["system"], json!("You are helpful"));
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn multiple_system_messages_concatenated() {
        let mut req = base_req();
        req.messages = vec![
            ChatMessage {
                role: "system".into(),
                content: Some(json!("A")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "system".into(),
                content: Some(json!("B")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            user_msg("hi"),
        ];
        let body = convert(&req).unwrap();
        assert_eq!(body["system"], json!("A\n\nB"));
    }

    #[test]
    fn assistant_with_tool_calls_emits_tool_use_blocks() {
        let mut req = base_req();
        req.messages = vec![
            user_msg("search please"),
            ChatMessage {
                role: "assistant".into(),
                content: Some(json!("On it")),
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "tc1".into(),
                    call_type: "function".into(),
                    function: ToolCallFunction {
                        name: "search".into(),
                        arguments: r#"{"q":"x"}"#.into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            },
        ];
        let body = convert(&req).unwrap();
        let asst = &body["messages"][1];
        assert_eq!(asst["role"], "assistant");
        let blocks = asst["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "On it");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "tc1");
        assert_eq!(blocks[1]["name"], "search");
        assert_eq!(blocks[1]["input"], json!({"q": "x"}));
    }

    #[test]
    fn tool_message_becomes_user_tool_result() {
        let mut req = base_req();
        req.messages = vec![ChatMessage {
            role: "tool".into(),
            content: Some(json!("result data")),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("tc1".into()),
            name: None,
        }];
        let body = convert(&req).unwrap();
        let msg = &body["messages"][0];
        assert_eq!(msg["role"], "user");
        assert_eq!(msg["content"][0]["type"], "tool_result");
        assert_eq!(msg["content"][0]["tool_use_id"], "tc1");
        assert_eq!(msg["content"][0]["content"], "result data");
    }

    #[test]
    fn image_url_data_uri_converted_to_anthropic_image() {
        let mut req = base_req();
        req.messages = vec![ChatMessage {
            role: "user".into(),
            content: Some(json!([
                {"type": "text", "text": "what is this?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,abc123"}}
            ])),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        let body = convert(&req).unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "base64");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["data"], "abc123");
    }

    #[test]
    fn image_url_https_converted_to_anthropic_url_source() {
        let mut req = base_req();
        req.messages = vec![ChatMessage {
            role: "user".into(),
            content: Some(json!([
                {"type": "text", "text": "describe"},
                {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}}
            ])),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        let body = convert(&req).unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "url");
        assert_eq!(blocks[1]["source"]["url"], "https://example.com/a.png");
    }

    #[test]
    fn image_url_unsupported_scheme_emits_notice_not_silent_drop() {
        let mut req = base_req();
        req.messages = vec![ChatMessage {
            role: "user".into(),
            content: Some(json!([
                {"type": "text", "text": "see"},
                {"type": "image_url", "image_url": {"url": "file:///tmp/x.png"}}
            ])),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        let body = convert(&req).unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["type"], "text");
        let notice = blocks[1]["text"].as_str().unwrap();
        assert!(notice.contains("image omitted"), "notice={notice}");
        assert!(notice.contains("file:///tmp/x.png"), "notice={notice}");
    }

    #[test]
    fn tools_function_shape_to_anthropic_input_schema() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        req.tools = Some(vec![json!({
            "type": "function",
            "function": {
                "name": "search",
                "description": "Search the web",
                "parameters": {"type": "object", "properties": {"q": {"type": "string"}}},
            }
        })]);
        let body = convert(&req).unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "search");
        assert_eq!(tools[0]["description"], "Search the web");
        assert_eq!(
            tools[0]["input_schema"]["properties"]["q"]["type"],
            "string"
        );
        // 不应残留 type/function 包裹层
        assert!(tools[0].get("type").is_none());
    }

    #[test]
    fn tool_choice_maps_all_variants() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];

        req.tool_choice = Some(json!("auto"));
        assert_eq!(
            convert(&req).unwrap()["tool_choice"],
            json!({"type": "auto"})
        );

        req.tool_choice = Some(json!("required"));
        assert_eq!(
            convert(&req).unwrap()["tool_choice"],
            json!({"type": "any"})
        );

        req.tool_choice = Some(json!("none"));
        // none → 不发 tool_choice 字段
        assert!(convert(&req).unwrap().get("tool_choice").is_none());

        req.tool_choice = Some(json!({"type": "function", "function": {"name": "search"}}));
        assert_eq!(
            convert(&req).unwrap()["tool_choice"],
            json!({"type": "tool", "name": "search"})
        );
    }

    #[test]
    fn reasoning_effort_budget_below_default_max_tokens_and_strips_sampling() {
        // 默认 max_tokens=8192 时 medium/high 的 budget 不能 >= max_tokens（Anthropic 400）；
        // 开思考时 temperature/top_p 也必须省略（同样 400）。
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        req.temperature = Some(0.5);
        req.top_p = Some(0.9);
        for effort in ["low", "medium", "high"] {
            req.reasoning_effort = Some(effort.into());
            let body = convert(&req).unwrap();
            let budget = body["thinking"]["budget_tokens"].as_i64().unwrap();
            let max_tokens = body["max_tokens"].as_i64().unwrap();
            assert!(
                budget < max_tokens,
                "{effort}: budget {budget} must be < max_tokens {max_tokens}"
            );
            assert!(body.get("temperature").is_none(), "{effort}: temperature");
            assert!(body.get("top_p").is_none(), "{effort}: top_p");
        }
    }

    #[test]
    fn reasoning_effort_maps_to_thinking_budget() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        req.max_tokens = Some(64000);

        req.reasoning_effort = Some("low".into());
        assert_eq!(
            convert(&req).unwrap()["thinking"],
            json!({"type": "enabled", "budget_tokens": 4096})
        );

        req.reasoning_effort = Some("medium".into());
        assert_eq!(
            convert(&req).unwrap()["thinking"],
            json!({"type": "enabled", "budget_tokens": 8192})
        );

        req.reasoning_effort = Some("high".into());
        assert_eq!(
            convert(&req).unwrap()["thinking"],
            json!({"type": "enabled", "budget_tokens": 16384})
        );
    }

    #[test]
    fn reasoning_effort_follows_model_family_rules() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];

        // opus-4-7 只接受 adaptive,enabled+budget 会 400
        req.model = "claude-opus-4-7".into();
        req.reasoning_effort = Some("xhigh".into());
        assert_eq!(
            convert(&req).unwrap()["thinking"],
            json!({"type": "adaptive"})
        );

        // haiku 不支持 thinking
        req.model = "claude-haiku-4-5".into();
        req.reasoning_effort = Some("high".into());
        assert!(convert(&req).unwrap().get("thinking").is_none());

        // 老 sonnet 走 enabled+budget,且 budget < 默认 max_tokens
        req.model = "claude-3-5-sonnet".into();
        req.reasoning_effort = Some("medium".into());
        let body = convert(&req).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(
            body["thinking"]["budget_tokens"].as_i64().unwrap()
                < body["max_tokens"].as_i64().unwrap()
        );
    }

    #[test]
    fn reasoning_effort_with_forced_tool_choice_disables_thinking() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        req.model = "claude-3-5-sonnet".into();
        req.reasoning_effort = Some("high".into());
        req.tool_choice = Some(json!({"type": "function", "function": {"name": "search"}}));
        assert!(convert(&req).unwrap().get("thinking").is_none());
        req.tool_choice = Some(json!("required"));
        assert!(convert(&req).unwrap().get("thinking").is_none());
    }

    #[test]
    fn no_reasoning_effort_keeps_thinking_off() {
        // Chat 路径不因模型族强开思考,只跟随显式 reasoning_effort
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        req.model = "claude-sonnet-4-5".into();
        assert!(convert(&req).unwrap().get("thinking").is_none());
    }

    #[test]
    fn temperature_clamped_to_anthropic_range() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        req.temperature = Some(1.5);
        let body = convert(&req).unwrap();
        assert_eq!(body["temperature"], json!(1.0));
    }

    #[test]
    fn max_tokens_defaults_when_missing() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        let body = convert(&req).unwrap();
        assert_eq!(body["max_tokens"], json!(8192));
    }

    #[test]
    fn stop_renamed_to_stop_sequences() {
        let mut req = base_req();
        req.messages = vec![user_msg("hi")];
        req.stop = Some(json!(["STOP"]));
        let body = convert(&req).unwrap();
        assert_eq!(body["stop_sequences"], json!(["STOP"]));
    }

    #[test]
    fn consecutive_same_role_messages_merged() {
        let mut req = base_req();
        req.messages = vec![user_msg("part 1"), user_msg("part 2")];
        let body = convert(&req).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        let blocks = messages[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], "part 1");
        assert_eq!(blocks[1]["text"], "part 2");
    }

    #[test]
    fn empty_messages_still_produces_valid_body() {
        let req = base_req();
        let body = convert(&req).unwrap();
        assert_eq!(body["model"], "claude-3");
        assert_eq!(body["max_tokens"], json!(8192));
        assert_eq!(body["messages"], json!([]));
    }
}
