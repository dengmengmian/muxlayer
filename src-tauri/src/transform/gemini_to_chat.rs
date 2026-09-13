use serde_json::{json, Value};

use crate::errors::AppError;
use crate::protocol::chat_completions::{
    ChatCompletionsRequest, ChatMessage, ToolCall, ToolCallFunction,
};

/// Convert a Gemini API request body to Chat Completions format.
pub fn convert(gemini_body: &Value, model: &str) -> Result<ChatCompletionsRequest, AppError> {
    let mut messages: Vec<ChatMessage> = Vec::new();

    // 1. System instruction → system message
    if let Some(si) = gemini_body.get("systemInstruction") {
        let text = extract_parts_text(si.get("parts"));
        if !text.is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: Some(Value::String(text)),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }
    }

    // 2. Contents → messages
    // 配对规则（Gemini CLI 实测行为）：
    // - functionCall 带 id（我们输出时写入的上游 id，CLI 原样回显）→ 直接用；
    //   不带 id 时生成 `call_{序号}_{name}`，序号全局递增，截断到 64 字符也不撞。
    // - functionResponse 的 id 命中待应答调用 → 用它；CLI 自己生成的 callId
    //   （如 `shell-1700000000-ab12`）不命中 → 取同名最早未应答的调用；
    //   都没有（历史被截断）→ 用显式 id 或生成新 id。
    // - 所有 id 统一过 sanitize_call_id，两侧对称。
    let mut pending_calls: Vec<(String, String)> = Vec::new(); // (name, id)，按出现顺序
    let mut generated = 0usize;
    let mut gen_id = |name: &str| {
        let id = format!("call_{generated}_{name}");
        generated += 1;
        crate::transform::tool_calls::sanitize_call_id(&id).into_owned()
    };
    if let Some(contents) = gemini_body.get("contents").and_then(|c| c.as_array()) {
        for content in contents {
            let role = content
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            let chat_role = if role == "model" { "assistant" } else { "user" };

            let parts = content.get("parts").and_then(|p| p.as_array());
            if parts.is_none() {
                continue;
            }
            let parts = parts.unwrap();

            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut function_responses: Vec<(String, String, String)> = Vec::new();

            for part in parts {
                // Text part
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    text_parts.push(text.to_string());
                }

                // Function call (model calling a tool)
                if let Some(fc) = part.get("functionCall") {
                    let name = fc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = fc
                        .get("args")
                        .map(|a| a.to_string())
                        .unwrap_or("{}".to_string());
                    let id = match fc.get("id").and_then(|i| i.as_str()) {
                        Some(explicit) if !explicit.is_empty() => {
                            crate::transform::tool_calls::sanitize_call_id(explicit).into_owned()
                        }
                        _ => gen_id(&name),
                    };
                    pending_calls.push((name.clone(), id.clone()));
                    tool_calls.push(ToolCall {
                        id,
                        call_type: "function".to_string(),
                        function: ToolCallFunction {
                            name,
                            arguments: args,
                        },
                    });
                }

                // Function response (tool result)
                if let Some(fr) = part.get("functionResponse") {
                    let name = fr
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let response = fr
                        .get("response")
                        .map(|r| {
                            if r.is_string() {
                                r.as_str().unwrap().to_string()
                            } else {
                                r.to_string()
                            }
                        })
                        .unwrap_or_default();
                    let explicit = fr
                        .get("id")
                        .and_then(|i| i.as_str())
                        .filter(|i| !i.is_empty())
                        .map(|i| crate::transform::tool_calls::sanitize_call_id(i).into_owned());
                    let matched = explicit
                        .as_ref()
                        .and_then(|e| pending_calls.iter().position(|(_, id)| id == e))
                        .or_else(|| pending_calls.iter().position(|(n, _)| *n == name));
                    let id = match (matched, explicit) {
                        (Some(pos), _) => pending_calls.remove(pos).1,
                        (None, Some(explicit)) => explicit,
                        (None, None) => gen_id(&name),
                    };
                    function_responses.push((id, name, response));
                }
            }

            // Emit function responses as tool messages
            if !function_responses.is_empty() {
                for (id, name, response) in function_responses {
                    messages.push(ChatMessage {
                        role: "tool".to_string(),
                        content: Some(Value::String(response)),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: Some(id),
                        name: Some(name),
                    });
                }
                continue;
            }

            // Emit as regular message
            let text = text_parts.join("");
            messages.push(ChatMessage {
                role: chat_role.to_string(),
                content: if text.is_empty() {
                    None
                } else {
                    Some(Value::String(text))
                },
                reasoning_content: None,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id: None,
                name: None,
            });
        }
    }

    // 3. Tools → Chat Completions tools
    let tools = gemini_body
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools_arr| {
            let mut result = Vec::new();
            for tool in tools_arr {
                if let Some(decls) = tool.get("functionDeclarations").and_then(|d| d.as_array()) {
                    for decl in decls {
                        let name = decl.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        let desc = decl
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("");
                        let params = decl
                            .get("parameters")
                            .cloned()
                            .unwrap_or(json!({"type": "object"}));
                        result.push(json!({
                            "type": "function",
                            "function": {"name": name, "description": desc, "parameters": params}
                        }));
                    }
                }
            }
            result
        })
        .filter(|t| !t.is_empty());

    // 4. Generation config
    let gen = gemini_body.get("generationConfig");
    let temperature = gen
        .and_then(|g| g.get("temperature"))
        .and_then(|v| v.as_f64());
    let top_p = gen.and_then(|g| g.get("topP")).and_then(|v| v.as_f64());
    let max_tokens = gen
        .and_then(|g| g.get("maxOutputTokens"))
        .and_then(|v| v.as_i64());
    let stop = gen.and_then(|g| g.get("stopSequences")).cloned();

    // 5. Stream options
    let stream_options = Some(json!({"include_usage": true}));

    Ok(ChatCompletionsRequest {
        model: model.to_string(),
        messages,
        tools,
        tool_choice: None,
        stream: true, // Gemini CLI typically streams
        temperature,
        top_p,
        max_tokens,
        max_completion_tokens: max_tokens, // 同步透传新字段（C 修复）
        thinking: None,
        stream_options,
        response_format: None,
        reasoning_effort: None,
        seed: None,
        stop,
        frequency_penalty: None,
        presence_penalty: None,
        parallel_tool_calls: None,
        diagnostic_events: Vec::new(),
    })
}

fn extract_parts_text(parts: Option<&Value>) -> String {
    parts
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Convert a Chat Completions response to Gemini response format.
pub fn response_to_gemini(chat_resp: &Value, model: &str) -> Value {
    let mut parts: Vec<Value> = Vec::new();

    if let Some(choices) = chat_resp.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            if let Some(msg) = choice.get("message").or(choice.get("delta")) {
                if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                }
                if let Some(tcs) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        let args_str = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("{}");
                        let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                        let mut call = json!({"name": name, "args": args});
                        // 带上上游 id：Gemini CLI 会把它回显到下一轮的 functionCall /
                        // functionResponse 两侧，请求转换据此精确配对。
                        if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                            if !id.is_empty() {
                                call["id"] = json!(id);
                            }
                        }
                        parts.push(json!({"functionCall": call}));
                    }
                }
            }
        }
    }

    let finish_reason = chat_resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .map(|r| match r {
            "stop" => "STOP",
            "length" => "MAX_TOKENS",
            "tool_calls" => "STOP",
            _ => "STOP",
        })
        .unwrap_or("STOP");

    let usage = chat_resp.get("usage");

    let mut resp = json!({
        "candidates": [{
            "content": {"role": "model", "parts": parts},
            "finishReason": finish_reason
        }],
        "modelVersion": model
    });

    if let Some(u) = usage {
        resp["usageMetadata"] = json!({
            "promptTokenCount": u.get("prompt_tokens").or(u.get("input_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
            "candidatesTokenCount": u.get("completion_tokens").or(u.get("output_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
            "totalTokenCount": u.get("total_tokens").and_then(|v| v.as_i64()).unwrap_or(0)
        });
    }

    resp
}

/// Chat Completions SSE → Gemini SSE 的有状态转换器。
///
/// Chat 流的 tool_call 是增量的：首块带 name（arguments 常为 ""），后续块只带
/// arguments 片段。Gemini 的 functionCall 必须一次给出完整 args，所以按 index
/// 缓冲 name + arguments，等 finish_reason 到达时一次性输出；上游没发
/// finish_reason 就断流时由 [`GeminiStreamState::finish`] 兜底输出。
#[derive(Default)]
pub struct GeminiStreamState {
    /// 按首次出现顺序缓冲的 tool_call。
    tool_calls: Vec<StreamToolCall>,
}

#[derive(Default)]
struct StreamToolCall {
    index: Option<i64>,
    /// 上游 tool_call id，输出到 functionCall.id 供 Gemini CLI 回显配对。
    id: Option<String>,
    name: String,
    arguments: String,
}

impl GeminiStreamState {
    /// 消费一个 Chat chunk，返回应立即写给 client 的 Gemini SSE 帧（可能没有）。
    pub fn chunk_to_gemini(&mut self, chunk: &Value) -> Option<String> {
        let choices = chunk.get("choices")?.as_array()?;
        let choice = choices.first()?;
        let delta = choice.get("delta");

        let mut parts: Vec<Value> = Vec::new();

        if let Some(text) = delta
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
        {
            if !text.is_empty() {
                parts.push(json!({"text": text}));
            }
        }

        if let Some(tcs) = delta
            .and_then(|d| d.get("tool_calls"))
            .and_then(|t| t.as_array())
        {
            for tc in tcs {
                let Some(func) = tc.get("function") else {
                    continue;
                };
                let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let index = tc.get("index").and_then(|i| i.as_i64());
                let id = tc
                    .get("id")
                    .and_then(|i| i.as_str())
                    .filter(|i| !i.is_empty());
                // 归属：index 优先；缺 index 按 id（首块带 index、后续只带 id 也能续上）；
                // 两者都没有时，带 name 视为新调用，否则续到最近一个调用上。
                let pos = index
                    .and_then(|i| self.tool_calls.iter().position(|c| c.index == Some(i)))
                    .or_else(|| {
                        id.and_then(|id| {
                            self.tool_calls
                                .iter()
                                .position(|c| c.id.as_deref() == Some(id))
                        })
                    })
                    .or_else(|| {
                        if index.is_none() && id.is_none() && name.is_empty() {
                            self.tool_calls.len().checked_sub(1)
                        } else {
                            None
                        }
                    });
                let entry = match pos {
                    Some(p) => &mut self.tool_calls[p],
                    None => {
                        self.tool_calls.push(StreamToolCall::default());
                        self.tool_calls.last_mut().expect("just pushed")
                    }
                };
                if entry.index.is_none() {
                    entry.index = index;
                }
                if entry.id.is_none() {
                    entry.id = id.map(String::from);
                }
                if !name.is_empty() {
                    entry.name = name.to_string();
                }
                if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                    entry.arguments.push_str(args);
                }
            }
        }

        let finish = choice.get("finish_reason").and_then(|f| f.as_str());
        if finish.is_some() {
            parts.extend(self.drain_function_calls());
        }
        let usage = chunk.get("usage");
        if parts.is_empty() && finish.is_none() && usage.is_none() {
            return None;
        }

        let mut candidate = json!({});
        if !parts.is_empty() {
            candidate["content"] = json!({"role": "model", "parts": parts});
        }
        if let Some(f) = finish {
            let gemini_reason = match f {
                "length" => "MAX_TOKENS",
                _ => "STOP",
            };
            candidate["finishReason"] = json!(gemini_reason);
        }
        let mut resp = json!({"candidates": [candidate]});
        if let Some(u) = usage {
            resp["usageMetadata"] = json!({
                "promptTokenCount": u.get("prompt_tokens").or(u.get("input_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
                "candidatesTokenCount": u.get("completion_tokens").or(u.get("output_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
                "totalTokenCount": u.get("total_tokens").and_then(|v| v.as_i64()).unwrap_or(0)
            });
        }
        Some(format!("data: {}\n\n", resp))
    }

    /// 上游流结束时调用：输出仍在缓冲中的 functionCall（没收到 finish_reason 的情况）。
    pub fn finish(&mut self) -> Option<String> {
        let parts = self.drain_function_calls();
        if parts.is_empty() {
            return None;
        }
        let resp = json!({"candidates": [{"content": {"role": "model", "parts": parts}}]});
        Some(format!("data: {}\n\n", resp))
    }

    fn drain_function_calls(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.tool_calls)
            .into_iter()
            .filter_map(|c| {
                if c.name.is_empty() {
                    tracing::warn!(
                        index = ?c.index,
                        "dropping streamed tool_call without function name (Gemini functionCall requires name)"
                    );
                    return None;
                }
                let args_val = if c.arguments.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&c.arguments).unwrap_or_else(|e| {
                        tracing::warn!(
                            tool = %c.name,
                            error = %e,
                            "streamed tool_call arguments not valid JSON; sent as {{}}"
                        );
                        json!({})
                    })
                };
                let mut call = json!({"name": c.name, "args": args_val});
                if let Some(id) = c.id {
                    call["id"] = json!(id);
                }
                Some(json!({"functionCall": call}))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_simple() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}]
        });
        let result = convert(&body, "gemini-2.5-flash").unwrap();
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].role, "user");
        assert_eq!(
            result.messages[0].content,
            Some(Value::String("hello".to_string()))
        );
    }

    #[test]
    fn test_convert_system_instruction() {
        let body = json!({
            "systemInstruction": {"parts": [{"text": "Be helpful"}]},
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}]
        });
        let result = convert(&body, "test").unwrap();
        assert_eq!(result.messages[0].role, "system");
        assert_eq!(
            result.messages[0].content,
            Some(Value::String("Be helpful".to_string()))
        );
        assert_eq!(result.messages[1].role, "user");
    }

    #[test]
    fn test_convert_model_to_assistant() {
        let body = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "q"}]},
                {"role": "model", "parts": [{"text": "a"}]}
            ]
        });
        let result = convert(&body, "test").unwrap();
        assert_eq!(result.messages[1].role, "assistant");
    }

    #[test]
    fn test_convert_function_call() {
        let body = json!({
            "contents": [
                {"role": "model", "parts": [{"functionCall": {"name": "search", "args": {"q": "hi"}}}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "search", "response": {"result": "found"}}}]}
            ]
        });
        let result = convert(&body, "test").unwrap();
        assert_eq!(result.messages[0].role, "assistant");
        assert!(result.messages[0].tool_calls.is_some());
        assert_eq!(result.messages[1].role, "tool");
    }

    #[test]
    fn test_convert_function_call_ids_pair_with_responses() {
        // 无 id 的 functionCall / functionResponse 两侧必须生成同一套 id,
        // 同名并行调用也不能撞 id。
        let body = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "list twice"}]},
                {"role": "model", "parts": [
                    {"functionCall": {"name": "shell", "args": {"cmd": "ls a"}}},
                    {"functionCall": {"name": "shell", "args": {"cmd": "ls b"}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "shell", "response": {"out": "a"}}},
                    {"functionResponse": {"name": "shell", "response": {"out": "b"}}}
                ]},
                {"role": "model", "parts": [{"functionCall": {"name": "shell", "args": {"cmd": "ls c"}}}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "shell", "response": {"out": "c"}}}]}
            ]
        });
        let result = convert(&body, "test").unwrap();
        let call_ids: Vec<String> = result
            .messages
            .iter()
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
            .map(|tc| tc.id.clone())
            .collect();
        let response_ids: Vec<String> = result
            .messages
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_call_id.clone())
            .collect();
        assert_eq!(call_ids.len(), 3);
        assert_eq!(call_ids, response_ids);
        let unique: std::collections::HashSet<&String> = call_ids.iter().collect();
        assert_eq!(unique.len(), 3, "ids must be unique: {call_ids:?}");
    }

    fn call_and_response_ids(result: &ChatCompletionsRequest) -> (Vec<String>, Vec<String>) {
        let call_ids = result
            .messages
            .iter()
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
            .map(|tc| tc.id.clone())
            .collect();
        let response_ids = result
            .messages
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_call_id.clone())
            .collect();
        (call_ids, response_ids)
    }

    #[test]
    fn test_gemini_cli_echoes_our_function_call_id_on_both_sides() {
        // 我们输出的 functionCall 带 id → Gemini CLI 两侧回显同一个 id → 原样配对。
        let resp = response_to_gemini(
            &json!({"choices": [{"message": {"tool_calls": [
                {"id": "call_abc", "type": "function", "function": {"name": "shell", "arguments": "{}"}}
            ]}, "finish_reason": "tool_calls"}]}),
            "m",
        );
        let fc = resp["candidates"][0]["content"]["parts"][0]["functionCall"].clone();
        assert_eq!(fc["id"], "call_abc");
        let body = json!({
            "contents": [
                {"role": "model", "parts": [{"functionCall": fc}]},
                {"role": "user", "parts": [{"functionResponse": {"id": "call_abc", "name": "shell", "response": {"out": "x"}}}]}
            ]
        });
        let (calls, responses) = call_and_response_ids(&convert(&body, "m").unwrap());
        assert_eq!(calls, vec!["call_abc".to_string()]);
        assert_eq!(responses, calls);
    }

    #[test]
    fn test_gemini_cli_generated_response_id_pairs_with_idless_call() {
        // functionCall 没 id,Gemini CLI 自己生成 callId 只写进 functionResponse.id
        // → 不匹配任何待应答调用时按同名最早未应答调用配对。
        let body = json!({
            "contents": [
                {"role": "model", "parts": [{"functionCall": {"name": "shell", "args": {}}}]},
                {"role": "user", "parts": [{"functionResponse": {"id": "shell-1700000000-ab12", "name": "shell", "response": {"out": "x"}}}]}
            ]
        });
        let (calls, responses) = call_and_response_ids(&convert(&body, "m").unwrap());
        assert_eq!(calls.len(), 1);
        assert_eq!(responses, calls);
    }

    #[test]
    fn test_gemini_cli_two_same_name_calls_pair_in_order() {
        let body = json!({
            "contents": [
                {"role": "model", "parts": [
                    {"functionCall": {"name": "shell", "args": {"cmd": "a"}}},
                    {"functionCall": {"name": "shell", "args": {"cmd": "b"}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"id": "shell-1-aa", "name": "shell", "response": {"out": "a"}}},
                    {"functionResponse": {"id": "shell-2-bb", "name": "shell", "response": {"out": "b"}}}
                ]}
            ]
        });
        let (calls, responses) = call_and_response_ids(&convert(&body, "m").unwrap());
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0], calls[1]);
        assert_eq!(responses, calls);
    }

    #[test]
    fn test_generated_call_id_is_valid_for_dotted_or_colon_names() {
        let body = json!({
            "contents": [
                {"role": "model", "parts": [{"functionCall": {"name": "mcp.server:tool", "args": {}}}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "mcp.server:tool", "response": {}}}]}
            ]
        });
        let (calls, responses) = call_and_response_ids(&convert(&body, "m").unwrap());
        assert_eq!(responses, calls);
        assert!(
            calls[0]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "invalid id: {}",
            calls[0]
        );
    }

    #[test]
    fn test_stream_function_call_carries_upstream_id() {
        let mut state = GeminiStreamState::default();
        let chunks = [
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "shell", "arguments": "{}"}}]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ];
        let frames: Vec<String> = chunks
            .iter()
            .filter_map(|c| state.chunk_to_gemini(c))
            .collect();
        let calls = collect_function_calls(&frames);
        assert_eq!(
            calls,
            vec![json!({"id": "call_1", "name": "shell", "args": {}})]
        );
    }

    #[test]
    fn test_stream_indexless_deltas_repeating_name_keyed_by_id() {
        // 不带 index、每块都重复 name 的上游:按 id 归并,不能拆成多个残缺调用。
        let mut state = GeminiStreamState::default();
        let chunks = [
            json!({"choices": [{"delta": {"tool_calls": [{"id": "c1", "function": {"name": "shell", "arguments": "{\"cmd\":"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"id": "c1", "function": {"name": "shell", "arguments": "\"ls\"}"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"id": "c2", "function": {"name": "shell", "arguments": "{}"}}]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ];
        let frames: Vec<String> = chunks
            .iter()
            .filter_map(|c| state.chunk_to_gemini(c))
            .collect();
        let calls = collect_function_calls(&frames);
        assert_eq!(
            calls,
            vec![
                json!({"id": "c1", "name": "shell", "args": {"cmd": "ls"}}),
                json!({"id": "c2", "name": "shell", "args": {}})
            ]
        );
    }

    fn collect_function_calls(frames: &[String]) -> Vec<Value> {
        frames
            .iter()
            .filter_map(|f| f.strip_prefix("data: "))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .flat_map(|v| {
                v["candidates"][0]["content"]["parts"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|p| p.get("functionCall").cloned())
            .collect()
    }

    #[test]
    fn test_stream_tool_call_arguments_accumulated_until_finish() {
        let mut state = GeminiStreamState::default();
        let chunks = [
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "shell", "arguments": ""}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{\"cmd\":"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"ls\"}"}}]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ];
        let mut frames: Vec<String> = chunks
            .iter()
            .filter_map(|c| state.chunk_to_gemini(c))
            .collect();
        frames.extend(state.finish());
        let calls = collect_function_calls(&frames);
        assert_eq!(
            calls,
            vec![json!({"id": "c1", "name": "shell", "args": {"cmd": "ls"}})]
        );
    }

    #[test]
    fn test_stream_tool_calls_flushed_at_stream_end_without_finish_reason() {
        let mut state = GeminiStreamState::default();
        let chunks = [
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"name": "a", "arguments": "{\"x\":1}"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 1, "function": {"name": "b", "arguments": "{}"}}]}}]}),
        ];
        let mut frames: Vec<String> = chunks
            .iter()
            .filter_map(|c| state.chunk_to_gemini(c))
            .collect();
        frames.extend(state.finish());
        let calls = collect_function_calls(&frames);
        assert_eq!(
            calls,
            vec![
                json!({"name": "a", "args": {"x": 1}}),
                json!({"name": "b", "args": {}})
            ]
        );
    }

    #[test]
    fn test_convert_tools() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "tools": [{"functionDeclarations": [{"name": "search", "description": "Search", "parameters": {"type": "object"}}]}]
        });
        let result = convert(&body, "test").unwrap();
        assert!(result.tools.is_some());
        assert_eq!(result.tools.unwrap()[0]["function"]["name"], "search");
    }

    #[test]
    fn test_response_to_gemini() {
        let chat_resp = json!({
            "choices": [{"message": {"content": "hello"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let result = response_to_gemini(&chat_resp, "gemini-2.5-flash");
        assert_eq!(
            result["candidates"][0]["content"]["parts"][0]["text"],
            "hello"
        );
        assert_eq!(result["candidates"][0]["finishReason"], "STOP");
        assert_eq!(result["usageMetadata"]["promptTokenCount"], 10);
    }
}
