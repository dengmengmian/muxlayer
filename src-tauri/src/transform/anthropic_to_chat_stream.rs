//! Anthropic Messages SSE → Chat Completions SSE 增量转换器。
//!
//! 场景：client 走 /v1/chat/completions（Chat 协议）但 provider 只支持 Anthropic
//! Messages（仅配 anthropic_base_url），且 client 要 `stream:true`。
//!
//! 与 [`crate::transform::chat_to_anthropic_stream`] 完全反向对称：
//!
//! | Anthropic SSE 事件 | 输出 Chat chunk |
//! | --- | --- |
//! | `message_start` | 首块 `{role:"assistant"}` delta，记录 id/model |
//! | `content_block_start{type:text}` | no-op（Chat 没有 block 概念） |
//! | `content_block_start{type:thinking}` | no-op |
//! | `content_block_start{type:tool_use, id, name}` | `tool_calls[{index, id, type:"function", function:{name, arguments:""}}]` |
//! | `content_block_delta{type:text_delta, text}` | `delta:{content: text}` |
//! | `content_block_delta{type:thinking_delta, thinking}` | `delta:{reasoning_content: text}` |
//! | `content_block_delta{type:input_json_delta, partial_json}` | `tool_calls[{index, function:{arguments: partial_json}}]` |
//! | `content_block_stop` | no-op |
//! | `message_delta{stop_reason, usage:{output_tokens}}` | 记录 finish_reason + usage（等 message_stop 一并 emit） |
//! | `message_stop` | 终块带 finish_reason + usage，再 `data: [DONE]` |
//! | `ping` / 未知事件 | no-op |

use std::collections::HashMap;

use serde_json::{json, Value};

/// 已经在 Chat 流里见过的 tool_call_index（Anthropic content_block_idx → Chat tool_call 序号）。
struct ToolBlockMapping {
    chat_tool_idx: i64,
}

pub struct AnthropicToChatStream {
    chat_id: String,
    model: String,
    created: i64,
    started: bool,
    /// Anthropic content_block index → 我方 Chat tool_call 序号。
    tool_blocks: HashMap<usize, ToolBlockMapping>,
    /// 下一个可分配的 Chat tool_call 序号（连续递增）。
    next_tool_idx: i64,
    /// 从 message_delta 取的 stop_reason，待 message_stop 时映射 finish_reason。
    stop_reason: Option<String>,
    /// 从 message_start / message_delta 合并出来的 Anthropic usage（后到的字段覆盖先到的），
    /// 终块统一经 anthropic_to_chat::remap_usage 映射，与非流式路径保持一致（含 cache 字段）。
    usage: serde_json::Map<String, Value>,
    /// 防止 finalize / message_stop 重复 emit。
    stopped: bool,
}

impl AnthropicToChatStream {
    pub fn new(model: impl Into<String>) -> Self {
        let chat_id = format!(
            "chatcmpl_{}",
            &uuid::Uuid::new_v4().to_string().replace('-', "")[..12]
        );
        Self {
            chat_id,
            model: model.into(),
            created: chrono::Utc::now().timestamp(),
            started: false,
            tool_blocks: HashMap::new(),
            next_tool_idx: 0,
            stop_reason: None,
            usage: serde_json::Map::new(),
            stopped: false,
        }
    }

    /// 消费一个 Anthropic SSE 事件，返回这一步该写给 client 的 Chat SSE 字符串列表。
    pub fn process_event(&mut self, event_type: &str, data: &Value) -> Vec<String> {
        match event_type {
            "message_start" => self.on_message_start(data),
            "content_block_start" => self.on_content_block_start(data),
            "content_block_delta" => self.on_content_block_delta(data),
            "content_block_stop" => Vec::new(),
            "message_delta" => self.on_message_delta(data),
            "message_stop" => self.on_message_stop(),
            // ping / 未知事件忽略
            _ => Vec::new(),
        }
    }

    /// 在上游流结束时调用：补 message_stop（如果还没 emit）+ `data: [DONE]`。
    /// 重复调用是 no-op。
    pub fn finalize(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        if !self.stopped {
            events.extend(self.on_message_stop());
        }
        events.push("data: [DONE]\n\n".to_string());
        events
    }

    fn on_message_start(&mut self, data: &Value) -> Vec<String> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        // 优先用上游 message_start 携带的 id / model；缺省保留构造时的值。
        if let Some(msg) = data.get("message") {
            if let Some(id) = msg.get("id").and_then(|i| i.as_str()) {
                self.chat_id = id.to_string();
            }
            if let Some(m) = msg.get("model").and_then(|m| m.as_str()) {
                self.model = m.to_string();
            }
            self.merge_usage(msg.get("usage"));
        }

        vec![self.emit_chunk(json!({"role": "assistant", "content": ""}), None)]
    }

    fn on_content_block_start(&mut self, data: &Value) -> Vec<String> {
        let idx = data.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
        let block = match data.get("content_block") {
            Some(b) => b,
            None => return Vec::new(),
        };
        let bt = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match bt {
            "tool_use" => {
                // 分配新的 chat tool_call_index、emit 首块 tool_calls delta 带 id + name + ""
                let chat_tool_idx = self.alloc_tool_idx();
                self.tool_blocks
                    .insert(idx, ToolBlockMapping { chat_tool_idx });
                let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                vec![self.emit_chunk(
                    json!({
                        "tool_calls": [{
                            "index": chat_tool_idx,
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": ""},
                        }],
                    }),
                    None,
                )]
            }
            // text / thinking 块：Chat 无对应"开始"语义，等 delta 到再 emit。
            _ => Vec::new(),
        }
    }

    fn on_content_block_delta(&mut self, data: &Value) -> Vec<String> {
        let idx = data.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
        let delta = match data.get("delta") {
            Some(d) => d,
            None => return Vec::new(),
        };
        let dt = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match dt {
            "text_delta" => {
                let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text.is_empty() {
                    return Vec::new();
                }
                vec![self.emit_chunk(json!({"content": text}), None)]
            }
            "thinking_delta" => {
                let text = delta.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                if text.is_empty() {
                    return Vec::new();
                }
                vec![self.emit_chunk(json!({"reasoning_content": text}), None)]
            }
            "input_json_delta" => {
                let partial = delta
                    .get("partial_json")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                if partial.is_empty() {
                    return Vec::new();
                }
                let mapping = match self.tool_blocks.get(&idx) {
                    Some(m) => m,
                    None => return Vec::new(),
                };
                vec![self.emit_chunk(
                    json!({
                        "tool_calls": [{
                            "index": mapping.chat_tool_idx,
                            "function": {"arguments": partial},
                        }],
                    }),
                    None,
                )]
            }
            // signature_delta / 未知类型忽略
            _ => Vec::new(),
        }
    }

    fn on_message_delta(&mut self, data: &Value) -> Vec<String> {
        if let Some(delta) = data.get("delta") {
            if let Some(sr) = delta.get("stop_reason").and_then(|s| s.as_str()) {
                if !sr.is_empty() {
                    self.stop_reason = Some(sr.to_string());
                }
            }
        }
        self.merge_usage(data.get("usage"));
        Vec::new()
    }

    /// 合并一段 Anthropic usage：只收数值字段，后到的覆盖先到的
    /// （message_delta 的 output_tokens 是累计值）。部分兼容上游在 message_delta
    /// 里把已在 message_start 给过的字段填 0，0 不覆盖已有值。
    fn merge_usage(&mut self, usage: Option<&Value>) {
        if let Some(Value::Object(u)) = usage {
            for (k, v) in u {
                let zero_over_existing = v.as_i64() == Some(0) && self.usage.contains_key(k);
                if v.is_number() && !zero_over_existing {
                    self.usage.insert(k.clone(), v.clone());
                }
            }
        }
    }

    fn on_message_stop(&mut self) -> Vec<String> {
        if self.stopped {
            return Vec::new();
        }
        self.stopped = true;

        // 兜底：没收到 message_start（罕见上游异常）也要 emit 一个角色块给 client。
        let mut events: Vec<String> = Vec::new();
        if !self.started {
            self.started = true;
            events.push(self.emit_chunk(json!({"role": "assistant", "content": ""}), None));
        }

        let finish_reason = map_stop_reason(self.stop_reason.as_deref());
        // 终块：delta 留空，finish_reason 设值，usage 同包带过去。
        // include_usage 形态：终块的 choices[].delta 为空、choices[].finish_reason 有值，
        // 整 chunk 顶层带 usage。
        let usage = crate::transform::anthropic_to_chat::remap_usage(Some(&Value::Object(
            self.usage.clone(),
        )));
        events.push(self.emit_chunk(json!({}), Some((finish_reason, usage))));

        events
    }

    fn alloc_tool_idx(&mut self) -> i64 {
        let i = self.next_tool_idx;
        self.next_tool_idx += 1;
        i
    }

    /// 拼一个 Chat SSE chunk 字符串。`delta` 是 choices[0].delta 的内容；
    /// `tail` 是 (finish_reason, usage)——终块才传，普通 chunk 传 None。
    fn emit_chunk(&self, delta: Value, tail: Option<(&'static str, Value)>) -> String {
        let mut choice = json!({
            "index": 0,
            "delta": delta,
            "finish_reason": Value::Null,
        });
        let mut chunk = json!({
            "id": self.chat_id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [choice.clone()],
        });
        if let Some((fr, usage)) = tail {
            choice["finish_reason"] = json!(fr);
            chunk["choices"] = json!([choice]);
            chunk["usage"] = usage;
        }
        format!("data: {chunk}\n\n")
    }
}

/// Anthropic stop_reason → Chat finish_reason（与 `map_finish_reason` 反向对称）。
fn map_stop_reason(sr: Option<&str>) -> &'static str {
    match sr {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        Some("refusal") => "content_filter",
        Some("end_turn") | Some("stop_sequence") => "stop",
        _ => "stop",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_start_emits_role_chunk() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let events = s.process_event(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": "msg_abc",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-3-sonnet",
                    "content": [],
                    "usage": {"input_tokens": 100, "output_tokens": 1},
                }
            }),
        );
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("\"role\":\"assistant\""));
        assert!(events[0].contains("\"id\":\"msg_abc\""));
        assert!(events[0].contains("\"model\":\"claude-3-sonnet\""));
    }

    #[test]
    fn text_delta_emits_content_chunk() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let _ = s.process_event(
            "content_block_start",
            &json!({
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
        );
        let events = s.process_event(
            "content_block_delta",
            &json!({
                "index": 0,
                "delta": {"type": "text_delta", "text": "Hello"}
            }),
        );
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("\"content\":\"Hello\""));
    }

    #[test]
    fn thinking_delta_emits_reasoning_content() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let _ = s.process_event(
            "content_block_start",
            &json!({
                "index": 0,
                "content_block": {"type": "thinking", "thinking": ""}
            }),
        );
        let events = s.process_event(
            "content_block_delta",
            &json!({
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Hmm..."}
            }),
        );
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("\"reasoning_content\":\"Hmm...\""));
    }

    #[test]
    fn tool_use_block_start_emits_initial_tool_call_chunk() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let events = s.process_event(
            "content_block_start",
            &json!({
                "index": 1,
                "content_block": {"type": "tool_use", "id": "tu1", "name": "search", "input": {}}
            }),
        );
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("\"tool_calls\""));
        assert!(events[0].contains("\"index\":0"));
        assert!(events[0].contains("\"id\":\"tu1\""));
        assert!(events[0].contains("\"name\":\"search\""));
        assert!(events[0].contains("\"arguments\":\"\""));
    }

    #[test]
    fn input_json_delta_emits_arguments_chunk() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let _ = s.process_event(
            "content_block_start",
            &json!({
                "index": 0,
                "content_block": {"type": "tool_use", "id": "tu1", "name": "search", "input": {}}
            }),
        );
        let events = s.process_event(
            "content_block_delta",
            &json!({
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"q\":\"r"}
            }),
        );
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("\"arguments\":\"{\\\"q\\\":\\\"r\""));
        assert!(events[0].contains("\"index\":0"));
    }

    #[test]
    fn parallel_tool_use_blocks_get_independent_chat_indices() {
        // Anthropic block 0=text, block 1=tool_use(a), block 2=tool_use(b)
        // → Chat tool_calls index 0=a, index 1=b
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let _ = s.process_event(
            "content_block_start",
            &json!({
                "index": 0, "content_block": {"type": "text", "text": ""}
            }),
        );
        let events_a = s.process_event(
            "content_block_start",
            &json!({
                "index": 1,
                "content_block": {"type": "tool_use", "id": "tu_a", "name": "alpha"}
            }),
        );
        let events_b = s.process_event(
            "content_block_start",
            &json!({
                "index": 2,
                "content_block": {"type": "tool_use", "id": "tu_b", "name": "beta"}
            }),
        );
        assert!(events_a[0].contains("\"index\":0"));
        assert!(events_a[0].contains("\"id\":\"tu_a\""));
        assert!(events_b[0].contains("\"index\":1"));
        assert!(events_b[0].contains("\"id\":\"tu_b\""));
    }

    #[test]
    fn message_stop_emits_finish_chunk_and_done() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event(
            "message_start",
            &json!({"message": {"usage": {"input_tokens": 100}}}),
        );
        let _ = s.process_event(
            "content_block_delta",
            &json!({
                "index": 0, "delta": {"type": "text_delta", "text": "hi"}
            }),
        );
        let _ = s.process_event(
            "message_delta",
            &json!({
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 5}
            }),
        );
        let stop_events = s.process_event("message_stop", &json!({"type": "message_stop"}));
        let final_events = s.finalize();
        let joined = stop_events.join("") + &final_events.join("");
        assert!(joined.contains("\"finish_reason\":\"stop\""));
        assert!(joined.contains("\"prompt_tokens\":100"));
        assert!(joined.contains("\"completion_tokens\":5"));
        assert!(joined.contains("\"total_tokens\":105"));
        assert!(joined.contains("data: [DONE]"));
    }

    #[test]
    fn final_usage_keeps_cache_tokens_like_non_stream() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event(
            "message_start",
            &json!({"message": {"usage": {
                "input_tokens": 10,
                "cache_read_input_tokens": 7,
                "cache_creation_input_tokens": 3,
                "output_tokens": 1
            }}}),
        );
        let _ = s.process_event(
            "message_delta",
            &json!({"delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}),
        );
        let events = s.process_event("message_stop", &json!({}));
        let chunk: Value =
            serde_json::from_str(events.last().unwrap().trim().trim_start_matches("data: "))
                .unwrap();
        let usage = &chunk["usage"];
        assert_eq!(usage["prompt_tokens"], 10);
        assert_eq!(usage["completion_tokens"], 5);
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 7);
        assert_eq!(usage["cache_read_input_tokens"], 7);
        assert_eq!(usage["cache_creation_input_tokens"], 3);
    }

    #[test]
    fn stop_reason_max_tokens_maps_to_length() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let _ = s.process_event(
            "message_delta",
            &json!({
                "delta": {"stop_reason": "max_tokens"}
            }),
        );
        let events = s.process_event("message_stop", &json!({}));
        let joined = events.join("");
        assert!(joined.contains("\"finish_reason\":\"length\""));
    }

    #[test]
    fn stop_reason_tool_use_maps_to_tool_calls() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let _ = s.process_event(
            "message_delta",
            &json!({
                "delta": {"stop_reason": "tool_use"}
            }),
        );
        let events = s.process_event("message_stop", &json!({}));
        let joined = events.join("");
        assert!(joined.contains("\"finish_reason\":\"tool_calls\""));
    }

    #[test]
    fn ping_event_is_noop() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let events = s.process_event("ping", &json!({"type": "ping"}));
        assert!(events.is_empty());
    }

    #[test]
    fn finalize_is_idempotent() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let _ = s.process_event("message_stop", &json!({}));
        let first = s.finalize();
        let second = s.finalize();
        // first 包含 [DONE]，second 也包含但不应再 emit chunk
        assert!(first.iter().any(|e| e.contains("[DONE]")));
        assert!(second.iter().all(|e| e.contains("[DONE]")));
        // 关键：finalize 后再 message_stop 是 no-op（stopped 标记守护）
        let after = s.process_event("message_stop", &json!({}));
        assert!(after.is_empty());
    }

    #[test]
    fn finalize_without_message_stop_still_emits_done() {
        // 上游异常断开，message_stop 没到——finalize 也要给 client 合法收尾
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let events = s.finalize();
        let joined = events.join("");
        assert!(joined.contains("\"finish_reason\""));
        assert!(joined.contains("data: [DONE]"));
    }

    #[test]
    fn content_block_stop_is_noop() {
        let mut s = AnthropicToChatStream::new("claude-3");
        let _ = s.process_event("message_start", &json!({"message": {}}));
        let events = s.process_event("content_block_stop", &json!({"index": 0}));
        assert!(events.is_empty());
    }
}
