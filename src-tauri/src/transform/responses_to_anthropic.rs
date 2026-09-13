use serde_json::{json, Value};

use crate::errors::AppError;
use crate::protocol::openai_responses::ResponsesRequest;

/// Convert a Responses API request into a Claude Messages API request body.
/// `auto_cache`: if true, inject cache_control breakpoints for prompt caching.
pub fn convert(req: &ResponsesRequest, model: &str, auto_cache: bool) -> Result<Value, AppError> {
    // 1. System prompt (separate field in Claude, NOT in messages)
    let mut system_blocks: Vec<Value> = Vec::new();
    let system_text = req.instructions.as_ref().or(req.system.as_ref());
    if let Some(text) = system_text {
        if !text.is_empty() {
            system_blocks.push(json!({"type": "text", "text": text}));
        }
    }

    // 2. Replay history from previous_response_id
    let mut history_messages: Vec<Value> = Vec::new();
    if let Some(ref prev_id) = req.previous_response_id {
        if let Some(history) = crate::gateway::session_store::get_history(prev_id) {
            for msg in &history {
                let role = match msg.role.as_str() {
                    "system" | "developer" => {
                        // Fold system messages into system blocks
                        if let Some(text) = msg.content.as_ref().and_then(|c| c.as_str()) {
                            if !text.is_empty() {
                                system_blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                        continue;
                    }
                    "tool" => {
                        // Tool messages become user messages with tool_result
                        let content = msg.content.as_ref().and_then(|c| c.as_str()).unwrap_or("");
                        let tool_use_id = crate::transform::tool_calls::sanitize_call_id(
                            msg.tool_call_id.as_deref().unwrap_or(""),
                        );
                        history_messages.push(json!({
                            "role": "user",
                            "content": [{"type": "tool_result", "tool_use_id": tool_use_id.as_ref(), "content": content}]
                        }));
                        continue;
                    }
                    "assistant" => "assistant",
                    _ => "user",
                };

                let mut content_blocks: Vec<Value> = Vec::new();

                // Text content
                if let Some(ref c) = msg.content {
                    let text = c.as_str().unwrap_or("");
                    if !text.is_empty() {
                        content_blocks.push(json!({"type": "text", "text": text}));
                    }
                }

                // Tool calls → tool_use blocks
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        let input: Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                        let tu_id = crate::transform::tool_calls::sanitize_call_id(&tc.id);
                        content_blocks.push(json!({
                            "type": "tool_use",
                            "id": tu_id.as_ref(),
                            "name": tc.function.name,
                            "input": input
                        }));
                    }
                }

                if !content_blocks.is_empty() {
                    history_messages.push(json!({"role": role, "content": content_blocks}));
                }
            }
        }
    }

    // 3. Convert input items to Claude messages
    // tool_result 里的图片只发给 claude 模型：Anthropic 路径也服务 MiMo 等第三方
    // 兼容上游，这里拿不到 provider 能力矩阵，按模型 id 保守判断。
    let tool_result_images = model.to_ascii_lowercase().contains("claude");
    let input_messages = convert_input(&req.input, &mut system_blocks, tool_result_images)?;

    // 4. Combine history + input messages
    let mut all_messages = history_messages;
    all_messages.extend(input_messages);

    // Remove any existing system blocks from history that duplicate instructions
    // (instructions take priority, same as responses_to_chat)

    // 5. Enforce strict user/assistant alternation
    all_messages = merge_consecutive_role_messages(all_messages);

    // 6. Convert tools + tool_choice。"none" 语义是"禁用工具"——Anthropic
    //    没有 type=none，正确做法是**根本不发 tools 字段**而非映射成 auto
    //    （映射成 auto 模型仍可能选用工具，违背 client 明确意图）。
    let suppress_tools = matches!(
        req.tool_choice.as_ref().and_then(|v| v.as_str()),
        Some("none")
    );
    let tools = if suppress_tools {
        None
    } else {
        req.tools.as_ref().map(|t| convert_tools(t))
    };

    // 7. Convert tool_choice + 注入 disable_parallel_tool_use（如果 client
    //    设了 parallel_tool_calls: false）。Anthropic 把"禁用并行"放在
    //    tool_choice 对象里，不是顶层字段；client 没显式给 tool_choice 时
    //    我们补一个 auto 让它有地方挂这个开关。
    //    "none" 已通过 suppress_tools 处理，这里跳过 tool_choice 设置。
    let mut tool_choice = if suppress_tools {
        None
    } else {
        req.tool_choice.as_ref().map(convert_tool_choice)
    };
    if req.parallel_tool_calls == Some(false) && !suppress_tools {
        let mut tc = tool_choice.unwrap_or_else(|| json!({"type": "auto"}));
        if let Some(obj) = tc.as_object_mut() {
            obj.insert("disable_parallel_tool_use".to_string(), json!(true));
        }
        tool_choice = Some(tc);
    }

    // 8. max_tokens (required by Claude)
    let max_tokens = req.max_output_tokens.unwrap_or(8192);

    // 9. Thinking configuration:claude 系支持就开(质量优先),haiku/强制
    //    工具调用/显式 off 除外。详见 convert_thinking。
    let forced_tool_choice = tool_choice
        .as_ref()
        .and_then(|tc| tc.get("type"))
        .and_then(|t| t.as_str())
        .is_some_and(|t| t == "any" || t == "tool");
    let thinking = convert_thinking(&req.reasoning, model, max_tokens, forced_tool_choice);
    let thinking_on = thinking.is_some();

    // 10. Build request body
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
    });

    if !system_blocks.is_empty() {
        body["system"] = json!(system_blocks);
    }
    body["messages"] = json!(all_messages);

    if let Some(tools) = tools {
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
    }
    if let Some(tc) = tool_choice {
        body["tool_choice"] = tc;
    }
    if let Some(thinking) = thinking {
        body["thinking"] = thinking;
    }
    if let Some(stream) = req.stream {
        body["stream"] = json!(stream);
    }
    // thinking 开启时 Anthropic 要求 temperature 只能省略或为 1、top_p 受限
    // (须 ≥0.95)——带上自定义采样参数直接 400。开思考时一律省略,走 API 默认。
    if !thinking_on {
        if let Some(temp) = req.temperature {
            // Anthropic temperature 上限 1.0；OpenAI 上限 2.0。透传 1.5 会被
            // Anthropic 400 (`temperature: Input should be less than or equal to 1`)。
            // clamp 是悄悄修正而非报错——用户感知不到差异（温度 1.5 vs 1.0 对
            // 模型输出多样性差异不大），却避免了一次请求失败。
            body["temperature"] = json!(temp.clamp(0.0, 1.0));
        }
        if let Some(top_p) = req.top_p {
            // 双方都是 [0, 1]，理论上无需 clamp；防御性 clamp 一次免得 client
            // 传 1.5 之类的非法值漏到上游。
            body["top_p"] = json!(top_p.clamp(0.0, 1.0));
        }
    }
    if let Some(ref stop) = req.stop {
        body["stop_sequences"] = stop.clone();
    }

    // 11. Inject cache_control for prompt caching (if enabled)
    if auto_cache {
        inject_cache_control(&mut body);
    }

    Ok(body)
}

/// Inject `cache_control: {type: "ephemeral"}` at Anthropic's recommended breakpoints.
///
/// 3 breakpoints (out of max 4):
/// 1. Last system block
/// 2. Last tool definition
/// 3. Last assistant message's last non-thinking block
///
/// Anthropic 全请求上限 4 个断点。转换路径的 body 是全新的(0 个已有断点);
/// pass-through 的 body 来自 Claude Code,可能已带自己的断点——先计数,
/// 只用剩余预算,已标记的位置跳过不覆盖(保留 client 设置的 ttl)。
pub(crate) fn inject_cache_control(body: &mut Value) {
    let marker = json!({"type": "ephemeral"});
    let mut budget = 4usize.saturating_sub(count_cache_controls(body));

    // 1. System: last block
    if budget > 0 {
        if let Some(last) = body
            .get_mut("system")
            .and_then(|s| s.as_array_mut())
            .and_then(|s| s.last_mut())
        {
            if last.get("cache_control").is_none() {
                last["cache_control"] = marker.clone();
                budget -= 1;
            }
        }
    }

    // 2. Tools: last item
    if budget > 0 {
        if let Some(last) = body
            .get_mut("tools")
            .and_then(|t| t.as_array_mut())
            .and_then(|t| t.last_mut())
        {
            if last.get("cache_control").is_none() {
                last["cache_control"] = marker.clone();
                budget -= 1;
            }
        }
    }

    // 3. Messages: last assistant message's last non-thinking block
    if budget == 0 {
        return;
    }
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        // Reverse iterate to find the last assistant message
        for msg in messages.iter_mut().rev() {
            if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
                continue;
            }
            if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
                // Find last non-thinking block
                for block in content.iter_mut().rev() {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if block_type != "thinking" && block_type != "redacted_thinking" {
                        if block.get("cache_control").is_none() {
                            block["cache_control"] = marker;
                        }
                        return;
                    }
                }
            }
            break; // Only process the last assistant message
        }
    }
}

/// 统计 body 里已有的 cache_control 断点数(system / tools / messages 全算)。
fn count_cache_controls(body: &Value) -> usize {
    let mut n = 0;
    for key in ["system", "tools"] {
        if let Some(arr) = body.get(key).and_then(|v| v.as_array()) {
            n += arr
                .iter()
                .filter(|b| b.get("cache_control").is_some())
                .count();
        }
    }
    if let Some(msgs) = body.get("messages").and_then(|v| v.as_array()) {
        for m in msgs {
            if let Some(content) = m.get("content").and_then(|c| c.as_array()) {
                n += content
                    .iter()
                    .filter(|b| b.get("cache_control").is_some())
                    .count();
            }
        }
    }
    n
}

/// Convert the Responses API `input` field to Claude messages.
fn convert_input(
    input: &Value,
    system_blocks: &mut Vec<Value>,
    tool_result_images: bool,
) -> Result<Vec<Value>, AppError> {
    match input {
        Value::String(s) => Ok(vec![
            json!({"role": "user", "content": [{"type": "text", "text": s}]}),
        ]),
        Value::Array(items) => convert_input_array(items, system_blocks, tool_result_images),
        Value::Object(_) => {
            let blocks = extract_content_blocks(Some(input));
            if blocks.is_empty() {
                Ok(vec![
                    json!({"role": "user", "content": [{"type": "text", "text": ""}]}),
                ])
            } else {
                Ok(vec![json!({"role": "user", "content": blocks})])
            }
        }
        _ => Ok(vec![
            json!({"role": "user", "content": [{"type": "text", "text": input.to_string()}]}),
        ]),
    }
}

fn convert_input_array(
    items: &[Value],
    system_blocks: &mut Vec<Value>,
    tool_result_images: bool,
) -> Result<Vec<Value>, AppError> {
    if !items.is_empty() && items.iter().all(is_content_part) {
        let blocks = extract_content_blocks(Some(&Value::Array(items.to_vec())));
        if blocks.is_empty() {
            return Ok(vec![]);
        }
        return Ok(vec![json!({"role": "user", "content": blocks})]);
    }

    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool_uses: Vec<Value> = Vec::new();
    // 从 reasoning 项里解码出来的 thinking 块，等到下一个 assistant 输出
    // （message:assistant / function_call）时 prepend 到 content 数组——
    // Anthropic 要求 thinking 块必须在同一条 assistant message 里、且
    // 出现在 text/tool_use 之前。
    let mut pending_thinking_blocks: Vec<Value> = Vec::new();

    for item in items {
        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match item_type {
            "message" => {
                // Flush pending tool calls
                flush_tool_uses(
                    &mut messages,
                    &mut pending_tool_uses,
                    &mut pending_thinking_blocks,
                );

                let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");

                match role {
                    "system" | "developer" => {
                        let text = extract_text(item.get("content"));
                        if !text.is_empty() {
                            system_blocks.push(json!({"type": "text", "text": text}));
                        }
                    }
                    "assistant" => {
                        // thinking 块必须在 text/tool_use 之前——先 drain。
                        let mut content_blocks: Vec<Value> =
                            std::mem::take(&mut pending_thinking_blocks);
                        let text = extract_text(item.get("content"));
                        if !text.is_empty() {
                            content_blocks.push(json!({"type": "text", "text": text}));
                        }

                        // Check for embedded tool_calls in content array
                        if let Some(Value::Array(parts)) = item.get("content") {
                            for part in parts {
                                if part.get("type").and_then(|t| t.as_str()) == Some("tool_call") {
                                    let raw_id =
                                        part.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                    let id = crate::transform::tool_calls::sanitize_call_id(raw_id);
                                    let name = part
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let args = part
                                        .get("arguments")
                                        .map(|a| {
                                            if a.is_string() {
                                                a.as_str().unwrap().to_string()
                                            } else {
                                                a.to_string()
                                            }
                                        })
                                        .unwrap_or_default();
                                    let input: Value =
                                        serde_json::from_str(&args).unwrap_or(json!({}));
                                    content_blocks.push(json!({
                                        "type": "tool_use", "id": id.as_ref(), "name": name, "input": input
                                    }));
                                }
                            }
                        }

                        if !content_blocks.is_empty() {
                            messages.push(json!({"role": "assistant", "content": content_blocks}));
                        }
                    }
                    _ => {
                        // user — preserve images
                        let blocks = extract_content_blocks(item.get("content"));
                        if !blocks.is_empty() {
                            messages.push(json!({"role": "user", "content": blocks}));
                        }
                    }
                }
            }
            "custom_tool_call" => {
                let raw_call_id = item
                    .get("call_id")
                    .and_then(|c| c.as_str())
                    .unwrap_or("call_unknown");
                let call_id = crate::transform::tool_calls::sanitize_call_id(raw_call_id);
                let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
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
                pending_tool_uses.push(json!({
                    "type": "tool_use",
                    "id": call_id.as_ref(),
                    "name": name,
                    "input": {"input": input}
                }));
            }
            "function_call" => {
                let raw_call_id = item
                    .get("call_id")
                    .and_then(|c| c.as_str())
                    .unwrap_or("call_unknown");
                let call_id = crate::transform::tool_calls::sanitize_call_id(raw_call_id);
                let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let arguments = item
                    .get("arguments")
                    .map(|a| {
                        if a.is_string() {
                            a.as_str().unwrap().to_string()
                        } else {
                            a.to_string()
                        }
                    })
                    .unwrap_or_default();
                let input: Value = serde_json::from_str(&arguments).unwrap_or(json!({}));

                pending_tool_uses.push(json!({
                    "type": "tool_use",
                    "id": call_id.as_ref(),
                    "name": name,
                    "input": input
                }));
            }
            "function_call_output" | "custom_tool_call_output" => {
                // Flush pending tool calls first
                flush_tool_uses(
                    &mut messages,
                    &mut pending_tool_uses,
                    &mut pending_thinking_blocks,
                );

                let call_id = item.get("call_id").and_then(|c| c.as_str());
                if call_id.is_none() || call_id == Some("") {
                    return Err(AppError::new(
                        crate::errors::codes::FUNCTION_CALL_OUTPUT_ID_MISSING,
                        "function_call_output is missing call_id",
                    ).with_suggestion("Each function_call_output must have a call_id matching a previous function_call"));
                }

                // tool_result.content 支持 text + image 块：目标模型支持视觉（claude）时
                // ContentPart 数组转成 Anthropic 块（图片保留为 base64 / url source）；
                // 否则与 Chat 路径一样降级成 "[image omitted]" 文本。无法识别出任何块时
                // 退回字符串形态。
                let output = match item.get("output") {
                    Some(Value::String(s)) => json!(s),
                    Some(o @ Value::Array(_)) if !tool_result_images => {
                        json!(crate::transform::responses_to_chat::flatten_tool_output(o))
                    }
                    Some(o @ Value::Array(_)) => {
                        let blocks = extract_content_blocks(Some(o));
                        if blocks.is_empty() {
                            json!(crate::transform::responses_to_chat::flatten_tool_output(o))
                        } else {
                            json!(blocks)
                        }
                    }
                    Some(o) => json!(o.to_string()),
                    None => json!(""),
                };

                let sanitized = crate::transform::tool_calls::sanitize_call_id(call_id.unwrap());
                messages.push(json!({
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": sanitized.as_ref(), "content": output}]
                }));
            }
            "reasoning" => {
                // 上一轮 Anthropic 响应的 thinking 块——解码 encrypted_content
                // 拿回签名链。等下一个 assistant 输出（message:assistant /
                // function_call）出现时 prepend 进 content。如果直到本数组
                // 结束都没遇到 assistant 输出，pending_thinking_blocks 会被
                // 丢弃，对 Anthropic 无影响（孤立的 thinking 块没意义）。
                if let Some(s) = item.get("encrypted_content").and_then(|v| v.as_str()) {
                    let blocks =
                        crate::transform::thinking_blocks::decode_from_encrypted_content(s);
                    let anth =
                        crate::transform::thinking_blocks::to_anthropic_content_blocks(&blocks);
                    pending_thinking_blocks.extend(anth);
                }
            }
            "compaction" | "context_compaction" | "compaction_summary" => {
                flush_tool_uses(
                    &mut messages,
                    &mut pending_tool_uses,
                    &mut pending_thinking_blocks,
                );
                let summary = item
                    .get("summary")
                    .or(item.get("content"))
                    .map(|v| extract_text(Some(v)))
                    .unwrap_or_else(|| "[context compacted]".to_string());
                messages
                    .push(json!({"role": "user", "content": [{"type": "text", "text": summary}]}));
            }
            _ => {
                // Unknown item: try to extract as message if it has role/content
                if let Some(role) = item.get("role").and_then(|r| r.as_str()) {
                    flush_tool_uses(
                        &mut messages,
                        &mut pending_tool_uses,
                        &mut pending_thinking_blocks,
                    );
                    let mapped_role = if role == "assistant" {
                        "assistant"
                    } else {
                        "user"
                    };
                    let text = extract_text(item.get("content"));
                    if !text.is_empty() {
                        messages.push(json!({"role": mapped_role, "content": [{"type": "text", "text": text}]}));
                    }
                }
            }
        }
    }

    flush_tool_uses(
        &mut messages,
        &mut pending_tool_uses,
        &mut pending_thinking_blocks,
    );
    Ok(messages)
}

fn flush_tool_uses(
    messages: &mut Vec<Value>,
    pending: &mut Vec<Value>,
    pending_thinking_blocks: &mut Vec<Value>,
) {
    // 仅当有 tool_use 待 flush 时才生成 assistant message。如果只剩孤立的
    // thinking_blocks（前面有 reasoning 但后面没遇到 assistant 输出就来了
    // user 消息），就保留给下个 assistant 输出消耗；要是直到数组末尾都没人
    // 接，会被静默丢弃——孤立 thinking 对 Anthropic 无意义，丢比硬塞强。
    if pending.is_empty() {
        return;
    }
    // thinking 块必须在 tool_use 之前——Anthropic 的 content 数组顺序敏感。
    let mut content: Vec<Value> = std::mem::take(pending_thinking_blocks);
    content.extend(std::mem::take(pending));
    messages.push(json!({"role": "assistant", "content": content}));
}

fn extract_text(content: Option<&Value>) -> String {
    match content {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()).map(String::from))
            .collect::<Vec<_>>()
            .join(""),
        Some(Value::Object(obj)) => obj
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        Some(other) => other.to_string(),
    }
}

fn is_content_part(part: &Value) -> bool {
    matches!(
        part.get("type").and_then(|t| t.as_str()),
        Some("input_text" | "output_text" | "text" | "input_image" | "image_url")
    )
}

/// Extract content blocks for Claude Messages API, preserving images.
/// Returns Vec of content blocks (text + image blocks).
fn extract_content_blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        None => vec![],
        Some(Value::String(s)) => {
            if s.is_empty() {
                vec![]
            } else {
                vec![json!({"type": "text", "text": s})]
            }
        }
        Some(Value::Array(arr)) => {
            let mut blocks = Vec::new();
            for part in arr {
                let pt = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match pt {
                    "input_text" | "output_text" | "text" => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                    }
                    "input_image" => {
                        // Convert to Claude image block format
                        if let Some(url) =
                            part.get("image_url").and_then(|u| u.as_str()).or_else(|| {
                                part.get("image_url")
                                    .and_then(|u| u.get("url"))
                                    .and_then(|u| u.as_str())
                            })
                        {
                            if let Some(b64_data) = url.strip_prefix("data:") {
                                // Parse data URI: data:image/png;base64,<data>
                                if let Some((media_info, data)) = b64_data.split_once(',') {
                                    let media_type = media_info.replace(";base64", "");
                                    blocks.push(json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": media_type,
                                            "data": data
                                        }
                                    }));
                                }
                            } else {
                                // URL-based image
                                blocks.push(json!({
                                    "type": "image",
                                    "source": {"type": "url", "url": url}
                                }));
                            }
                        }
                    }
                    "image_url" => {
                        // OpenAI Chat Completions format image_url
                        if let Some(url) = part
                            .get("image_url")
                            .and_then(|u| u.get("url"))
                            .and_then(|u| u.as_str())
                        {
                            if let Some(b64_data) = url.strip_prefix("data:") {
                                if let Some((media_info, data)) = b64_data.split_once(',') {
                                    let media_type = media_info.replace(";base64", "");
                                    blocks.push(json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": media_type,
                                            "data": data
                                        }
                                    }));
                                }
                            } else {
                                blocks.push(json!({
                                    "type": "image",
                                    "source": {"type": "url", "url": url}
                                }));
                            }
                        }
                    }
                    "tool_call" => {} // handled separately
                    _ => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                    }
                }
            }
            blocks
        }
        Some(Value::Object(obj)) => {
            if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
                vec![json!({"type": "text", "text": text})]
            } else {
                vec![]
            }
        }
        Some(_) => vec![],
    }
}

/// Merge consecutive messages with the same role.
/// Claude requires strict user/assistant alternation.
fn merge_consecutive_role_messages(messages: Vec<Value>) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        if let Some(last) = result.last_mut() {
            let last_role = last.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let msg_role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");

            if last_role == msg_role {
                // Merge content arrays
                if let (Some(last_content), Some(msg_content)) = (
                    last.get_mut("content").and_then(|c| c.as_array_mut()),
                    msg.get("content").and_then(|c| c.as_array()),
                ) {
                    last_content.extend(msg_content.iter().cloned());
                    continue;
                }
            }
        }
        result.push(msg);
    }

    result
}

/// Convert Responses API tools to Claude tool definitions.
///
/// 复用 Chat 路径的 `tool_calls::convert_tools`（覆盖 function A/B 结构、custom、
/// local_shell、namespace 的 tools/children 递归展开；tool_search 除外），再把
/// `{type:function, function:{name, description, parameters}}` 映射成
/// Anthropic 的 `{name, description, input_schema}`。只剪 null 值 property，
/// 不做 DeepSeek 式清洗——Anthropic 原生支持 additionalProperties 等字段。
fn convert_tools(tools: &[Value]) -> Vec<Value> {
    // 不提供 tool_search：Anthropic 路径不转换 tool_search_call / tool_search_output
    // 历史项，模型调用后拿不到结果会反复调用。
    let tools: Vec<Value> = tools
        .iter()
        .filter(|t| t.get("type").and_then(|v| v.as_str()) != Some("tool_search"))
        .cloned()
        .collect();
    let result = crate::transform::tool_calls::convert_tools(&tools, false)
        .into_iter()
        .filter_map(|tool| {
            // provider_type 为空时 convert_tools 只产出 function 形态；其它 builtin 不支持，跳过
            let func = tool.get("function")?;
            let raw_name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let name = crate::transform::tool_calls::sanitize_tool_name(raw_name);
            let desc = func
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            // Anthropic input_schema 必须是 type:object；缺省 / 空对象补齐。
            // null 值 property 对 Anthropic 也非法，剪掉（不做 DeepSeek 式剥除）。
            let input_schema = match func.get("parameters") {
                Some(p) if p.as_object().is_some_and(|o| !o.is_empty()) => {
                    let mut p = p.clone();
                    crate::transform::schema_cleaner::prune_null_properties(&mut p);
                    p
                }
                _ => json!({"type": "object", "properties": {}}),
            };
            Some(json!({
                "name": name.as_ref(),
                "description": desc,
                "input_schema": input_schema
            }))
        })
        .collect();

    crate::transform::tool_calls::dedupe_tools_by_name(result)
}

/// Convert Responses API tool_choice to Claude format.
fn convert_tool_choice(tc: &Value) -> Value {
    match tc {
        Value::String(s) => match s.as_str() {
            "auto" | "none" => json!({"type": "auto"}),
            "required" | "any" => json!({"type": "any"}),
            _ => json!({"type": "auto"}),
        },
        Value::Object(obj) => {
            // {name: "X"} or {function: {name: "X"}}
            let name = obj.get("name").and_then(|n| n.as_str()).or_else(|| {
                obj.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
            });
            if let Some(name) = name {
                json!({"type": "tool", "name": name})
            } else {
                json!({"type": "auto"})
            }
        }
        _ => json!({"type": "auto"}),
    }
}

/// Convert reasoning effort to Claude thinking configuration.
/// Anthropic 要求 thinking budget 最低 1024。
const MIN_THINKING_BUDGET: i64 = 1024;
/// 开思考时至少给正文留的 token 数,避免 budget 吃满 max_tokens 导致正文被截断。
const MIN_OUTPUT_RESERVE: i64 = 1024;

/// effort 字符串 → thinking budget 档位。未知 / auto 返回 None。
fn effort_to_thinking_budget(effort: &str) -> Option<i64> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "low" | "minimal" => Some(4096),
        "medium" => Some(8192),
        "high" => Some(16384),
        "xhigh" | "max" | "highest" => Some(32768),
        _ => None,
    }
}

/// 把 budget 夹进 [1024, max_tokens - MIN_OUTPUT_RESERVE];装不下最低 budget 返回 None(不开思考)。
fn clamp_thinking_budget(budget: i64, max_tokens: i64) -> Option<i64> {
    let budget = budget.min(max_tokens - MIN_OUTPUT_RESERVE);
    (budget >= MIN_THINKING_BUDGET).then_some(budget)
}

/// 按目标模型决定 thinking 配置。质量优先:**claude 系支持就开**(参考
/// cc-switch thinking_optimizer),显式 `effort: none/off` 是唯一逃生口。
/// 同时守住三个 Anthropic 硬约束防 400:
/// 1. haiku 不支持 thinking → 一律不带;
/// 2. thinking 与强制工具调用(tool_choice: any/tool)不兼容 → 不带;
/// 3. budget_tokens 必须 ∈ [1024, max_tokens) → clamp 并给正文留余量,装不下就不开。
/// 形态:opus-4.6+ / sonnet-4.6 用 adaptive;其他 claude 用 enabled+budget,
/// 未指定 effort 时 budget 顶满,只给正文留 MIN_OUTPUT_RESERVE。
/// 非 claude 的 anthropic 兼容上游(MiMo 等)思考方言各异,只跟随显式 effort。
pub(crate) fn convert_thinking(
    reasoning: &Option<Value>,
    model: &str,
    max_tokens: i64,
    forced_tool_choice: bool,
) -> Option<Value> {
    let m = model.to_ascii_lowercase().replace('.', "-");
    if m.contains("haiku") || forced_tool_choice {
        return None;
    }

    let effort = reasoning
        .as_ref()
        .and_then(|r| r.get("effort"))
        .and_then(|e| e.as_str())
        .map(|s| s.trim().to_ascii_lowercase());
    if matches!(effort.as_deref(), Some("none") | Some("off")) {
        return None;
    }
    let effort_budget = effort.as_deref().and_then(effort_to_thinking_budget);

    if ["opus-4-8", "opus-4-7", "opus-4-6", "sonnet-4-6"]
        .iter()
        .any(|fam| m.contains(fam))
    {
        return Some(json!({"type": "adaptive"}));
    }

    // budget 形态:claude 未指定 effort 时强开并顶满(clamp 会给正文留余量);
    // 非 claude 只跟随显式 effort。
    let budget = match effort_budget {
        Some(b) => b,
        None if m.contains("claude") => max_tokens,
        None => return None,
    };
    let budget = clamp_thinking_budget(budget, max_tokens)?;
    Some(json!({"type": "enabled", "budget_tokens": budget}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_req(input: Value) -> ResponsesRequest {
        ResponsesRequest {
            model: Some("claude-3-5-sonnet".to_string()),
            input,
            instructions: None,
            system: None,
            previous_response_id: None,
            tools: None,
            tool_choice: None,
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            ..Default::default()
        }
    }

    #[test]
    fn reasoning_item_with_signature_prepends_thinking_block_to_function_call() {
        // 模拟：上一轮 Anthropic 返回了 reasoning（带签名）+ tool_call，
        // client 把它们回传，AgentGate 必须把签名块 prepend 到 assistant
        // 的 content 数组里，否则 Anthropic 拒（thinking_required_signature）。
        let blocks = vec![crate::transform::thinking_blocks::ThinkingBlock {
            kind: "thinking".into(),
            text: "Let me think...".into(),
            signature: "sig_abc".into(),
            data: String::new(),
        }];
        let encrypted =
            crate::transform::thinking_blocks::encode_for_encrypted_content(&blocks).unwrap();
        let req = make_req(json!([
            {"type": "message", "role": "user", "content": "hello"},
            {"type": "reasoning", "id": "rs_1", "encrypted_content": encrypted},
            {"type": "function_call", "call_id": "c1", "name": "search", "arguments": "{}"}
        ]));
        let mut system_blocks: Vec<Value> = vec![];
        let messages =
            convert_input_array(req.input.as_array().unwrap(), &mut system_blocks, true).unwrap();
        // 倒数第二条应该是 assistant，含 thinking + tool_use 两个块
        let assistant = messages
            .iter()
            .rev()
            .find(|m| m["role"] == "assistant")
            .unwrap();
        let content = assistant["content"].as_array().unwrap();
        assert_eq!(
            content.len(),
            2,
            "expected thinking + tool_use, got {content:?}"
        );
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "sig_abc");
        assert_eq!(content[1]["type"], "tool_use");
    }

    #[test]
    fn reasoning_item_prepends_thinking_block_to_assistant_message() {
        let blocks = vec![crate::transform::thinking_blocks::ThinkingBlock {
            kind: "thinking".into(),
            text: "step by step".into(),
            signature: "sig1".into(),
            data: String::new(),
        }];
        let encrypted =
            crate::transform::thinking_blocks::encode_for_encrypted_content(&blocks).unwrap();
        let req = make_req(json!([
            {"type": "reasoning", "id": "rs_1", "encrypted_content": encrypted},
            {"type": "message", "role": "assistant", "content": "the answer is 42"}
        ]));
        let mut system_blocks: Vec<Value> = vec![];
        let messages =
            convert_input_array(req.input.as_array().unwrap(), &mut system_blocks, true).unwrap();
        let assistant = messages
            .iter()
            .rev()
            .find(|m| m["role"] == "assistant")
            .unwrap();
        let content = assistant["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[1]["type"], "text");
    }

    #[test]
    fn reasoning_item_without_signature_is_dropped() {
        // 旧 encrypted_content 是纯文本，解码出空块数组——不应该 emit
        // 无签名 thinking（Anthropic 会 400）。
        let req = make_req(json!([
            {"type": "reasoning", "id": "rs_1", "encrypted_content": "Just a plain text summary"},
            {"type": "function_call", "call_id": "c1", "name": "x", "arguments": "{}"}
        ]));
        let mut system_blocks: Vec<Value> = vec![];
        let messages =
            convert_input_array(req.input.as_array().unwrap(), &mut system_blocks, true).unwrap();
        let assistant = messages
            .iter()
            .rev()
            .find(|m| m["role"] == "assistant")
            .unwrap();
        let content = assistant["content"].as_array().unwrap();
        // 只有 tool_use，没有 thinking
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
    }

    #[test]
    fn test_convert_simple_string_input() {
        let req = make_req(json!("hello"));
        let result = convert(&req, "claude-3-5-sonnet", false).unwrap();
        assert_eq!(result["model"], "claude-3-5-sonnet");
        assert_eq!(result["max_tokens"], 8192);
        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"][0]["type"], "text");
        assert_eq!(msgs[0]["content"][0]["text"], "hello");
    }

    #[test]
    fn test_convert_with_instructions() {
        let mut req = make_req(json!("hello"));
        req.instructions = Some("Be helpful".to_string());
        let result = convert(&req, "claude-3-5-sonnet", false).unwrap();
        let sys = result["system"].as_array().unwrap();
        assert_eq!(sys.len(), 1);
        assert_eq!(sys[0]["text"], "Be helpful");
    }

    #[test]
    fn test_convert_function_call_to_tool_use() {
        let req = make_req(json!([
            {"type": "function_call", "call_id": "call_1", "name": "search", "arguments": "{\"q\":\"hi\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "result"}
        ]));
        let result = convert(&req, "claude-3-5-sonnet", false).unwrap();
        let msgs = result["messages"].as_array().unwrap();
        // assistant with tool_use, then user with tool_result
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[0]["content"][0]["id"], "call_1");
        assert_eq!(msgs[0]["content"][0]["name"], "search");
        assert_eq!(msgs[0]["content"][0]["input"]["q"], "hi");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[1]["content"][0]["tool_use_id"], "call_1");
    }

    #[test]
    fn test_convert_tool_choice() {
        assert_eq!(convert_tool_choice(&json!("auto")), json!({"type": "auto"}));
        assert_eq!(
            convert_tool_choice(&json!("required")),
            json!({"type": "any"})
        );
        assert_eq!(
            convert_tool_choice(&json!({"name": "search"})),
            json!({"type": "tool", "name": "search"})
        );
    }

    #[test]
    fn convert_tools_flattens_functions_namespace_exec() {
        let tools = vec![json!({
            "type": "namespace",
            "name": "functions",
            "tools": [
                {"type": "custom", "name": "exec", "description": "run js"},
                {"type": "function", "name": "wait", "parameters": {"type": "object"}}
            ]
        })];
        let result = convert_tools(&tools);
        let names: Vec<&str> = result.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, vec!["exec", "wait"]);
    }

    #[test]
    fn test_convert_tools_parameters_to_input_schema() {
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "search",
                "description": "Search the web",
                "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}
            }
        })];
        let result = convert_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "search");
        assert!(result[0]["input_schema"].is_object());
        assert!(result[0].get("parameters").is_none());
    }

    #[test]
    fn test_convert_tools_dedupes_duplicate_names() {
        let tools = vec![
            json!({"type": "function", "function": {"name": "search", "description": "A", "parameters": {"type": "object"}}}),
            json!({"type": "function", "function": {"name": "search", "description": "B", "parameters": {"type": "object"}}}),
        ];
        let result = convert_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["description"], "A");
    }

    #[test]
    fn test_convert_max_tokens_default() {
        let req = make_req(json!("hi"));
        let result = convert(&req, "claude-3-5-sonnet", false).unwrap();
        assert_eq!(result["max_tokens"], 8192);
    }

    #[test]
    fn test_convert_max_tokens_custom() {
        let mut req = make_req(json!("hi"));
        req.max_output_tokens = Some(4096);
        let result = convert(&req, "claude-3-5-sonnet", false).unwrap();
        assert_eq!(result["max_tokens"], 4096);
    }

    #[test]
    fn test_convert_thinking() {
        let m = "claude-3-5-sonnet";
        let mt = 64000;
        assert_eq!(
            convert_thinking(&Some(json!({"effort": "low"})), m, mt, false),
            Some(json!({"type": "enabled", "budget_tokens": 4096}))
        );
        assert_eq!(
            convert_thinking(&Some(json!({"effort": "medium"})), m, mt, false),
            Some(json!({"type": "enabled", "budget_tokens": 8192}))
        );
        assert_eq!(
            convert_thinking(&Some(json!({"effort": "high"})), m, mt, false),
            Some(json!({"type": "enabled", "budget_tokens": 16384}))
        );
        // 显式 none/off 是唯一逃生口
        assert_eq!(
            convert_thinking(&Some(json!({"effort": "none"})), m, mt, false),
            None
        );
        // 质量优先:未指定 / auto → claude 系强开并顶满,只给正文留 1024
        assert_eq!(
            convert_thinking(&None, m, mt, false),
            Some(json!({"type": "enabled", "budget_tokens": 62976}))
        );
        assert_eq!(
            convert_thinking(&Some(json!({"effort": "auto"})), m, mt, false),
            Some(json!({"type": "enabled", "budget_tokens": 62976}))
        );
    }

    #[test]
    fn test_thinking_default_budget_leaves_room_for_output() {
        // 未指定 effort 时旧逻辑 budget = max_tokens-1,正文只剩 1 个 token。
        for mt in [8192_i64, 64000] {
            let t = convert_thinking(&None, "claude-3-5-sonnet", mt, false).unwrap();
            let budget = t["budget_tokens"].as_i64().unwrap();
            assert!(
                mt - budget >= 1024,
                "max_tokens={mt} budget={budget} 必须给正文留 >=1024 token"
            );
        }
    }

    #[test]
    fn test_convert_tools_skips_tool_search_and_keeps_namespace_children() {
        // tool_search_call / tool_search_output 历史项在 Anthropic 路径不转换,
        // 提供 tool_search 会让模型反复调用却拿不到结果 → 不提供。
        // namespace 需读 children 并递归(曾只读 tools)。
        let tools = vec![
            json!({"type": "tool_search", "description": "find tools"}),
            json!({
                "type": "namespace",
                "name": "mcp",
                "children": [
                    {"type": "function", "name": "read", "parameters": {"type": "object"}},
                    {"type": "namespace", "name": "inner", "tools": [
                        {"type": "function", "name": "deep", "parameters": {"type": "object"}}
                    ]}
                ]
            }),
        ];
        let result = convert_tools(&tools);
        let names: Vec<&str> = result.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, vec!["mcp__read", "inner__deep"]);
        for t in &result {
            assert_eq!(t["input_schema"]["type"], "object", "tool={t}");
            assert!(t.get("type").is_none() && t.get("function").is_none());
        }
    }

    #[test]
    fn test_convert_tools_does_not_strip_schema_on_anthropic_path() {
        // schema cleaner 是 DeepSeek quirk,Anthropic 原生支持 additionalProperties。
        let tools = vec![json!({
            "type": "function",
            "name": "put",
            "parameters": {"type": "object", "properties": {"k": {"type": "string"}}, "additionalProperties": false}
        })];
        let result = convert_tools(&tools);
        assert_eq!(
            result[0]["input_schema"]["additionalProperties"],
            json!(false)
        );
    }

    #[test]
    fn test_convert_tools_prunes_null_properties_on_anthropic_path() {
        // null 值 property 对官方 Anthropic 也非法:删掉并同步摘 required,
        // 但不做 DeepSeek 的 additionalProperties 剥除。
        let tools = vec![json!({
            "type": "function",
            "name": "put",
            "parameters": {
                "type": "object",
                "properties": {
                    "k": {"type": "string"},
                    "gone": null,
                    "nested": {"type": "object", "properties": {"x": null, "y": {"type": "number"}}, "required": ["x", "y"]}
                },
                "required": ["k", "gone"],
                "additionalProperties": false
            }
        })];
        let result = convert_tools(&tools);
        let schema = &result[0]["input_schema"];
        assert!(
            schema["properties"].get("gone").is_none(),
            "schema={schema}"
        );
        assert_eq!(schema["required"], json!(["k"]));
        assert!(schema["properties"]["nested"]["properties"]
            .get("x")
            .is_none());
        assert_eq!(schema["properties"]["nested"]["required"], json!(["y"]));
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn test_function_call_output_images_omitted_for_non_claude_models() {
        // Anthropic 兼容的第三方上游(MiMo 等)不一定支持视觉:非 claude 模型保持
        // "[image omitted]" 文本降级,不发 image 块。
        let req = make_req(json!([
            {"type": "function_call", "call_id": "c1", "name": "screenshot", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": [
                {"type": "input_text", "text": "shot"},
                {"type": "input_image", "image_url": "data:image/png;base64,QUJD"}
            ]}
        ]));
        let body = convert(&req, "mimo-v2.5-pro", false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        let tool_result = &msgs.last().unwrap()["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        let content = tool_result["content"].as_str().expect("string content");
        assert!(content.starts_with("shot"), "content={content}");
        assert!(content.contains("image"), "content={content}");
        assert!(!content.contains("QUJD"), "content={content}");
    }

    #[test]
    fn test_function_call_output_images_become_tool_result_image_blocks() {
        // tool_result.content 支持 image 块,Anthropic 路径不应降级成 "[image omitted]"。
        let req = make_req(json!([
            {"type": "function_call", "call_id": "c1", "name": "screenshot", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": [
                {"type": "input_text", "text": "shot"},
                {"type": "input_image", "image_url": "data:image/png;base64,QUJD"},
                {"type": "input_image", "image_url": "https://example.com/a.png"}
            ]}
        ]));
        let body = convert(&req, "claude-3-5-sonnet", false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        let tool_result = &msgs.last().unwrap()["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        assert_eq!(
            tool_result["content"],
            json!([
                {"type": "text", "text": "shot"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}},
                {"type": "image", "source": {"type": "url", "url": "https://example.com/a.png"}}
            ])
        );
    }

    #[test]
    fn test_thinking_stripped_for_haiku() {
        // Haiku 不支持 thinking,带上会 400 —— 客户端要思考也得剥。
        assert_eq!(
            convert_thinking(
                &Some(json!({"effort": "high"})),
                "claude-haiku-4-5",
                64000,
                false
            ),
            None
        );
        // 强开也不适用于 haiku
        assert_eq!(
            convert_thinking(&None, "claude-haiku-4-5", 64000, false),
            None
        );
    }

    #[test]
    fn test_thinking_adaptive_for_new_models() {
        // opus-4.6+/sonnet-4.6 用 adaptive 形态(参考 cc-switch thinking_optimizer
        // 的模型分发),不再发 legacy budget;未指定 effort 同样强开。
        for m in ["claude-opus-4-8", "claude-opus-4.6", "claude-sonnet-4-6"] {
            assert_eq!(
                convert_thinking(&Some(json!({"effort": "high"})), m, 64000, false),
                Some(json!({"type": "adaptive"})),
                "{m} 应使用 adaptive thinking"
            );
            assert_eq!(
                convert_thinking(&None, m, 64000, false),
                Some(json!({"type": "adaptive"})),
                "{m} 未指定 effort 也应强开 adaptive"
            );
        }
        // 老模型保持 legacy budget 形态
        assert_eq!(
            convert_thinking(
                &Some(json!({"effort": "high"})),
                "claude-sonnet-4-5",
                64000,
                false
            ),
            Some(json!({"type": "enabled", "budget_tokens": 16384}))
        );
    }

    #[test]
    fn test_thinking_budget_clamped_below_max_tokens() {
        // Anthropic 要求 budget_tokens < max_tokens,否则 400。
        // 此前 effort=high 固定给 16384,而 max_tokens 默认 8192,本就有潜在 400。
        assert_eq!(
            convert_thinking(
                &Some(json!({"effort": "high"})),
                "claude-3-5-sonnet",
                8192,
                false
            ),
            Some(json!({"type": "enabled", "budget_tokens": 7168}))
        );
        // max_tokens 装不下最低 1024 budget → 不开
        assert_eq!(
            convert_thinking(&None, "claude-3-5-sonnet", 1000, false),
            None
        );
    }

    #[test]
    fn test_thinking_skipped_when_tool_choice_forced() {
        // thinking 与强制工具调用(tool_choice: any/tool)不兼容,带上 400。
        assert_eq!(
            convert_thinking(
                &Some(json!({"effort": "high"})),
                "claude-3-5-sonnet",
                64000,
                true
            ),
            None
        );
        assert_eq!(
            convert_thinking(&None, "claude-opus-4-8", 64000, true),
            None
        );
    }

    #[test]
    fn test_thinking_not_forced_for_non_claude_models() {
        // anthropic 兼容上游(MiMo / DeepSeek 等)思考方言各异,只跟随显式
        // effort,不强开。
        assert_eq!(convert_thinking(&None, "mimo-v2.5-pro", 64000, false), None);
        assert_eq!(
            convert_thinking(
                &Some(json!({"effort": "high"})),
                "mimo-v2.5-pro",
                64000,
                false
            ),
            Some(json!({"type": "enabled", "budget_tokens": 16384}))
        );
    }

    #[test]
    fn test_temperature_dropped_when_thinking_on() {
        // thinking 开启时 temperature 只能省略或为 1,带 0.7 会 400。
        let mut req = make_req(json!([{"type": "message", "role": "user", "content": "hi"}]));
        req.temperature = Some(0.7);
        req.top_p = Some(0.9);
        req.max_output_tokens = Some(32000);
        let body = convert(&req, "claude-sonnet-4-5", false).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(
            body.get("temperature").is_none(),
            "开思考时不得带 temperature"
        );
        assert!(body.get("top_p").is_none(), "开思考时不得带 top_p");
        // haiku 不开思考 → 采样参数照常透传
        let body = convert(&req, "claude-haiku-4-5", false).unwrap();
        assert!(body.get("thinking").is_none());
        assert_eq!(body["temperature"], json!(0.7));
    }

    #[test]
    fn test_merge_consecutive_user_messages() {
        let messages = vec![
            json!({"role": "user", "content": [{"type": "text", "text": "a"}]}),
            json!({"role": "user", "content": [{"type": "text", "text": "b"}]}),
        ];
        let merged = merge_consecutive_role_messages(messages);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_system_role_message_folded_into_system() {
        let req = make_req(json!([
            {"type": "message", "role": "system", "content": "sys prompt"},
            {"type": "message", "role": "user", "content": "hello"}
        ]));
        let result = convert(&req, "claude-3-5-sonnet", false).unwrap();
        let sys = result["system"].as_array().unwrap();
        assert_eq!(sys[0]["text"], "sys prompt");
        // Messages should only have user, no system
        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn test_missing_call_id_errors() {
        let req = make_req(json!([
            {"type": "function_call_output", "call_id": "", "output": "result"}
        ]));
        assert!(convert(&req, "claude-3-5-sonnet", false).is_err());
    }

    // ── extract_content_blocks image tests ──

    #[test]
    fn test_extract_content_blocks_text_only() {
        let content = json!([{"type": "input_text", "text": "hello"}]);
        let blocks = extract_content_blocks(Some(&content));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "hello");
    }

    #[test]
    fn test_extract_content_blocks_with_base64_image() {
        let content = json!([
            {"type": "input_text", "text": "describe"},
            {"type": "input_image", "image_url": "data:image/png;base64,abc123"}
        ]);
        let blocks = extract_content_blocks(Some(&content));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "base64");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["data"], "abc123");
    }

    #[test]
    fn test_convert_initial_top_level_content_parts_preserves_image() {
        let mut system_blocks = Vec::new();
        let items = json!([
            {"type": "input_text", "text": "describe this"},
            {"type": "input_image", "image_url": {"url": "data:image/png;base64,abc123"}}
        ]);
        let messages =
            convert_input_array(items.as_array().unwrap(), &mut system_blocks, true).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content[0], json!({"type": "text", "text": "describe this"}));
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "abc123");
    }

    #[test]
    fn test_extract_content_blocks_with_url_image() {
        let content = json!([
            {"type": "input_image", "image_url": "https://example.com/photo.jpg"}
        ]);
        let blocks = extract_content_blocks(Some(&content));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["type"], "url");
        assert_eq!(blocks[0]["source"]["url"], "https://example.com/photo.jpg");
    }

    #[test]
    fn test_extract_content_blocks_image_url_type() {
        let content = json!([
            {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,xyz"}}
        ]);
        let blocks = extract_content_blocks(Some(&content));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(blocks[0]["source"]["data"], "xyz");
    }

    #[test]
    fn test_extract_content_blocks_string_input() {
        let blocks = extract_content_blocks(Some(&json!("hello")));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["text"], "hello");
    }

    #[test]
    fn test_extract_content_blocks_none() {
        let blocks = extract_content_blocks(None);
        assert!(blocks.is_empty());
    }

    // ── cache_control injection tests ──

    #[test]
    fn test_inject_respects_anthropic_four_breakpoint_budget() {
        // pass-through 场景:Claude Code 自己已带 4 个断点(Anthropic 硬上限),
        // 再注入会超限 400。已满时必须零注入。
        let marked = json!({"type": "ephemeral"});
        let mut body = json!({
            "system": [{"type": "text", "text": "s", "cache_control": marked}],
            "tools": [{"name": "t", "input_schema": {}, "cache_control": marked}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "u1", "cache_control": marked},
                    {"type": "text", "text": "u2", "cache_control": marked}
                ]},
                {"role": "assistant", "content": [{"type": "text", "text": "a"}]}
            ]
        });
        inject_cache_control(&mut body);
        assert!(
            body["messages"][1]["content"][0]
                .get("cache_control")
                .is_none(),
            "预算用尽不得再注入"
        );
    }

    #[test]
    fn test_inject_uses_remaining_budget_and_skips_marked() {
        // 已有 3 个断点(system/tools/user),剩 1 个预算 → 只给 assistant 补 1 个,
        // 已标记的位置跳过不重写。
        let marked = json!({"type": "ephemeral", "ttl": "1h"});
        let mut body = json!({
            "system": [{"type": "text", "text": "s", "cache_control": marked}],
            "tools": [{"name": "t", "input_schema": {}, "cache_control": marked}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "u", "cache_control": marked}
                ]},
                {"role": "assistant", "content": [{"type": "text", "text": "a"}]}
            ]
        });
        inject_cache_control(&mut body);
        assert_eq!(
            body["messages"][1]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        // 已标记的 system 不被覆盖(ttl 保留)
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
    }

    #[test]
    fn test_inject_cache_control_system_and_tools() {
        let mut body = json!({
            "system": [{"type": "text", "text": "Be helpful"}],
            "tools": [
                {"name": "search", "input_schema": {}},
                {"name": "read", "input_schema": {}}
            ],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });
        inject_cache_control(&mut body);
        // System last block has cache_control
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        // Tools last item has cache_control
        assert!(body["tools"][0].get("cache_control").is_none());
        assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_inject_cache_control_assistant_message() {
        let mut body = json!({
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "q1"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "a1"}]},
                {"role": "user", "content": [{"type": "text", "text": "q2"}]}
            ]
        });
        inject_cache_control(&mut body);
        // Last assistant message's text block gets cache_control
        assert_eq!(
            body["messages"][1]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        // User messages don't get cache_control
        assert!(body["messages"][2]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn test_inject_cache_control_skips_thinking_blocks() {
        let mut body = json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "q"}]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "answer"},
                    {"type": "thinking", "thinking": "..."},
                ]}
            ]
        });
        inject_cache_control(&mut body);
        // Should mark the text block, not the thinking block
        assert_eq!(
            body["messages"][1]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(body["messages"][1]["content"][1]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn test_inject_cache_control_no_assistant_messages() {
        let mut body = json!({
            "system": [{"type": "text", "text": "sys"}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "first message"}]}]
        });
        inject_cache_control(&mut body);
        // System still gets marked, no assistant to mark
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn test_convert_with_auto_cache_enabled() {
        let req = make_req(json!([
            {"type": "message", "role": "user", "content": "q1"},
            {"type": "message", "role": "assistant", "content": "a1"},
            {"type": "message", "role": "user", "content": "q2"}
        ]));
        let result = convert(&req, "claude-3-5-sonnet", true).unwrap();
        // With auto_cache=true, last assistant message should have cache_control
        let msgs = result["messages"].as_array().unwrap();
        let asst = &msgs[1];
        assert_eq!(asst["content"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_convert_with_auto_cache_disabled() {
        let req = make_req(json!([
            {"type": "message", "role": "user", "content": "q1"},
            {"type": "message", "role": "assistant", "content": "a1"},
            {"type": "message", "role": "user", "content": "q2"}
        ]));
        let result = convert(&req, "claude-3-5-sonnet", false).unwrap();
        // With auto_cache=false, no cache_control anywhere
        let msgs = result["messages"].as_array().unwrap();
        assert!(msgs[1]["content"][0].get("cache_control").is_none());
    }
}
