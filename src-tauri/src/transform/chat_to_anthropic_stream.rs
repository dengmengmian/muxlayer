//! Chat Completions SSE → Anthropic Messages SSE 增量转换器。
//!
//! 场景：client 走 /v1/messages（Anthropic 协议）但 provider 只支持 OpenAI
//! Chat Completions（没配 anthropic_base_url），且 client 要 `stream:true`。
//!
//! 之前的 fallback 是"非流式上游 + 合成 SSE"（详见 [`crate::protocol::
//! anthropic_messages::synthesize_sse_events`]），首字延迟 = 上游完整生成
//! 耗时。这个模块实现**真流式**：边收上游 chat chunk 边转换成 Anthropic
//! SSE 事件发给 client，首字延迟 = 上游首字延迟。
//!
//! 状态机要点：
//! - 第一个 chunk 到达时 emit `message_start`（input_tokens 通常只能留 0，最后
//!   usage 到达时由 message_delta 携带 input/output/cache 全部 usage 字段）
//! - 文本 delta → 必要时 emit `content_block_start{type:text}`、再 emit
//!   `content_block_delta{type:text_delta}`
//! - reasoning_content delta（DeepSeek-thinking / MiMo / o1 风格）→ 同上但
//!   block type 是 thinking、delta type 是 thinking_delta
//! - tool_calls delta：每个 upstream tool_call_index 映射到独立的 Anthropic
//!   content_block index；first delta 携带 id + name 触发
//!   `content_block_start{type:tool_use}`，后续 arguments 增量 emit
//!   `content_block_delta{type:input_json_delta, partial_json}`
//! - finish_reason 到达 → 关闭所有 open 的 content_block、emit `message_delta`
//!   带 stop_reason 映射、emit `message_stop`

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::protocol::chat_completions::ChatCompletionChunk;

/// 一个已经打开（emit 过 content_block_start、未关闭）的 Anthropic content block。
/// 块种类靠它在哪个字段持有来区分（text_block / thinking_block / tool_blocks），
/// 不另开 enum。
struct OpenBlock {
    /// Anthropic content 数组里的 index——与 chat tool_call_index 不同体系。
    anthropic_idx: usize,
}

/// 上游 tool_call delta 的归属键。标准上游带 index；部分上游并行调用只带 id
/// 不带 index，此时按 id 区分，避免全部并进同一个 block。
#[derive(Clone, PartialEq, Eq, Hash)]
enum ToolKey {
    Index(i64),
    Id(String),
}

pub struct ChatToAnthropicStream {
    message_id: String,
    model: String,
    /// 是否已经 emit message_start——第一个 upstream chunk 到达时触发一次。
    started: bool,
    /// 当前打开的 text block（如果有）。Chat 流的 text content 全部 collapse
    /// 进同一个 Anthropic text content_block。
    text_block: Option<OpenBlock>,
    /// 当前打开的 thinking block（如果有）。
    thinking_block: Option<OpenBlock>,
    /// 上游 tool_call_index → 我方 OpenBlock 的映射。Anthropic content block
    /// 序号必须连续、按打开顺序递增；upstream tool_call_index 也是连续的但
    /// 单独编号体系，需要这层映射。
    tool_blocks: HashMap<ToolKey, OpenBlock>,
    /// 最近一次出现的 tool_call 键——既无 index 也无 id 的续传增量归到这里。
    last_tool_key: Option<ToolKey>,
    /// 上游 tool_call id → 已打开块的键。首块带 index、后续只带 id 的增量靠它
    /// 续到同一个块，避免同一个 id 打开第二个 tool_use。
    id_to_key: HashMap<String, ToolKey>,
    /// 下一个可分配的 Anthropic content_block index。
    next_anthropic_idx: usize,
    /// 终块带的 finish_reason；用来映射 Anthropic stop_reason。
    finish_reason: Option<String>,
    /// 上游最终 usage（含 include_usage 时）。
    final_usage: Option<Value>,
    /// 是否已经 emit message_stop——防御幂等性。
    stopped: bool,
}

impl ChatToAnthropicStream {
    pub fn new(model: impl Into<String>) -> Self {
        // 生成稳定的 message id，与 from_chat_response 同形态（"msg_" + uuid 前缀）。
        let message_id = format!(
            "msg_{}",
            &uuid::Uuid::new_v4().to_string().replace('-', "")[..12]
        );
        Self {
            message_id,
            model: model.into(),
            started: false,
            text_block: None,
            thinking_block: None,
            tool_blocks: HashMap::new(),
            last_tool_key: None,
            id_to_key: HashMap::new(),
            next_anthropic_idx: 0,
            finish_reason: None,
            final_usage: None,
            stopped: false,
        }
    }

    /// 消费一个解析好的 Chat Completions chunk，返回这一步应该立即写给 client
    /// 的 Anthropic SSE 事件列表。
    pub fn process_chunk(&mut self, chunk: &ChatCompletionChunk) -> Vec<String> {
        let mut events: Vec<String> = Vec::new();

        // 第一次拿到 chunk 时 emit message_start。input_tokens 用 chunk 里的
        // usage（若 include_usage 在首块就送）或者 0；output_tokens 起步 1。
        if !self.started {
            self.started = true;
            let input_tokens = chunk
                .usage
                .as_ref()
                .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("input_tokens")))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            events.push(sse_event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": self.message_id,
                        "type": "message",
                        "role": "assistant",
                        "model": self.model,
                        "content": [],
                        "stop_reason": Value::Null,
                        "stop_sequence": Value::Null,
                        "usage": {
                            "input_tokens": input_tokens,
                            "output_tokens": 1
                        }
                    }
                }),
            ));
        }

        // 终块（OpenAI 习惯）携带 usage 但 choices 为空或没 delta。先取下来。
        if let Some(u) = &chunk.usage {
            self.final_usage = Some(u.clone());
        }

        let Some(choices) = &chunk.choices else {
            return events;
        };

        for choice in choices {
            // finish_reason 通常只在终块。先存起来；真正 emit message_delta 是
            // 在 finalize 时统一做（避免某些上游分多次发 finish_reason 时重复 emit）。
            if let Some(fr) = &choice.finish_reason {
                if !fr.is_empty() {
                    self.finish_reason = Some(fr.clone());
                }
            }

            let Some(delta) = &choice.delta else { continue };

            // reasoning_content → thinking block。Anthropic 顺序约束：thinking
            // 必须在 text/tool_use 之前。如果 text 已经打开了 reasoning 才来，
            // 仍然按到达顺序 emit——客户端容忍乱序、且这只是 fallback 路径。
            if let Some(rc) = &delta.reasoning_content {
                if !rc.is_empty() {
                    events.extend(self.ensure_thinking_block());
                    if let Some(b) = &self.thinking_block {
                        events.push(sse_event(
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": b.anthropic_idx,
                                "delta": {"type": "thinking_delta", "thinking": rc}
                            }),
                        ));
                    }
                }
            }
            // reasoning_details 数组（o3/o4 native 风格）—— 取 text 字段拼到 thinking。
            if let Some(details) = &delta.reasoning_details {
                for d in details {
                    if let Some(text) = d.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            events.extend(self.ensure_thinking_block());
                            if let Some(b) = &self.thinking_block {
                                events.push(sse_event(
                                    "content_block_delta",
                                    json!({
                                        "type": "content_block_delta",
                                        "index": b.anthropic_idx,
                                        "delta": {"type": "thinking_delta", "thinking": text}
                                    }),
                                ));
                            }
                        }
                    }
                }
            }

            // text content → text block
            if let Some(text) = &delta.content {
                if !text.is_empty() {
                    events.extend(self.ensure_text_block());
                    if let Some(b) = &self.text_block {
                        events.push(sse_event(
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": b.anthropic_idx,
                                "delta": {"type": "text_delta", "text": text}
                            }),
                        ));
                    }
                }
            }

            // tool_calls deltas
            if let Some(tcs) = &delta.tool_calls {
                for tc in tcs {
                    let func = tc.function.as_ref();
                    let name = func.and_then(|f| f.name.as_deref()).unwrap_or("");
                    let id = tc.id.as_deref().unwrap_or("");
                    // index 优先；缺 index 时按 id 找已打开的块（可能是按 index 打开的），
                    // 找不到再以 id 为键；两者都没有的续传增量归最近一个调用。
                    let tc_key = match (tc.index, id) {
                        (Some(i), _) => ToolKey::Index(i),
                        (None, id) if !id.is_empty() => self
                            .id_to_key
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| ToolKey::Id(id.to_string())),
                        (None, _) => self.last_tool_key.clone().unwrap_or(ToolKey::Index(0)),
                    };
                    if !id.is_empty() {
                        self.id_to_key
                            .entry(id.to_string())
                            .or_insert_with(|| tc_key.clone());
                    }
                    self.last_tool_key = Some(tc_key.clone());

                    // 第一次见这个键时打开 content_block。OpenAI 习惯：
                    // 首个 delta 携带 id + function.name + arguments=""；后续
                    // delta 仅携带 arguments 增量。
                    let need_open = !self.tool_blocks.contains_key(&tc_key);
                    if need_open {
                        let anthropic_idx = self.alloc_idx();
                        let sanitized_id = crate::transform::tool_calls::sanitize_call_id(id);
                        let sanitized_name = crate::transform::tool_calls::sanitize_tool_name(name);
                        events.push(sse_event(
                            "content_block_start",
                            json!({
                                "type": "content_block_start",
                                "index": anthropic_idx,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": sanitized_id.as_ref(),
                                    "name": sanitized_name.as_ref(),
                                    "input": {}
                                }
                            }),
                        ));
                        self.tool_blocks
                            .insert(tc_key.clone(), OpenBlock { anthropic_idx });
                    }

                    // arguments 增量 → input_json_delta
                    if let Some(args) = func.and_then(|f| f.arguments.as_deref()) {
                        if !args.is_empty() {
                            let anthropic_idx = self.tool_blocks[&tc_key].anthropic_idx;
                            events.push(sse_event(
                                "content_block_delta",
                                json!({
                                    "type": "content_block_delta",
                                    "index": anthropic_idx,
                                    "delta": {"type": "input_json_delta", "partial_json": args}
                                }),
                            ));
                        }
                    }
                }
            }
        }

        events
    }

    /// 在上游流结束（[DONE] / connection close）时调用，emit 收尾事件：
    /// 关闭所有 open 的 content_block、message_delta（stop_reason + usage）、
    /// message_stop。重复调用是 no-op（stopped 标记守护）。
    pub fn finalize(&mut self) -> Vec<String> {
        if self.stopped {
            return Vec::new();
        }
        self.stopped = true;

        let mut events: Vec<String> = Vec::new();

        // 没收到任何 chunk 就直接 finalize（上游 0 字节就关）：补 message_start。
        if !self.started {
            self.started = true;
            events.push(sse_event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": self.message_id,
                        "type": "message",
                        "role": "assistant",
                        "model": self.model,
                        "content": [],
                        "stop_reason": Value::Null,
                        "stop_sequence": Value::Null,
                        "usage": {"input_tokens": 0, "output_tokens": 1}
                    }
                }),
            ));
        }

        // 关闭所有 open block（顺序：thinking → text → tool_uses）
        if let Some(b) = self.thinking_block.take() {
            events.push(sse_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": b.anthropic_idx}),
            ));
        }
        if let Some(b) = self.text_block.take() {
            events.push(sse_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": b.anthropic_idx}),
            ));
        }
        // tool_blocks 按 anthropic_idx 升序关闭，行为可预测
        let mut tool_blocks: Vec<OpenBlock> = self.tool_blocks.drain().map(|(_, b)| b).collect();
        tool_blocks.sort_by_key(|b| b.anthropic_idx);
        for b in tool_blocks {
            events.push(sse_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": b.anthropic_idx}),
            ));
        }

        // message_delta：stop_reason 映射 + 终 usage。message_start 发出时上游
        // usage 通常还没到（input_tokens 只能填 0），所以终块 usage 在这里带上
        // input_tokens / cache_read_input_tokens 等全部字段，与非流式
        // from_chat_response 的映射一致。
        let stop_reason = map_finish_reason(self.finish_reason.as_deref());
        let usage = match self.final_usage.as_ref() {
            Some(u) => crate::protocol::anthropic_messages::remap_usage_to_anthropic(Some(u)),
            None => json!({"output_tokens": 0}),
        };
        events.push(sse_event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
                "usage": usage
            }),
        ));

        // message_stop
        events.push(sse_event("message_stop", json!({"type": "message_stop"})));

        events
    }

    fn alloc_idx(&mut self) -> usize {
        let i = self.next_anthropic_idx;
        self.next_anthropic_idx += 1;
        i
    }

    fn ensure_text_block(&mut self) -> Vec<String> {
        if self.text_block.is_some() {
            return Vec::new();
        }
        let idx = self.alloc_idx();
        self.text_block = Some(OpenBlock { anthropic_idx: idx });
        vec![sse_event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": {"type": "text", "text": ""}
            }),
        )]
    }

    fn ensure_thinking_block(&mut self) -> Vec<String> {
        if self.thinking_block.is_some() {
            return Vec::new();
        }
        let idx = self.alloc_idx();
        self.thinking_block = Some(OpenBlock { anthropic_idx: idx });
        vec![sse_event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": {"type": "thinking", "thinking": ""}
            }),
        )]
    }
}

fn sse_event(event_type: &str, data: Value) -> String {
    format!("event: {event_type}\ndata: {data}\n\n")
}

/// Chat finish_reason → Anthropic stop_reason 映射。与
/// [`crate::protocol::anthropic_messages::from_chat_response`] 保持一致。
fn map_finish_reason(fr: Option<&str>) -> &'static str {
    match fr {
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        Some("content_filter") => "refusal",
        Some("stop") => "end_turn",
        _ => "end_turn",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ChatCompletionChunk {
        serde_json::from_str(s).expect("test chunk parses")
    }

    #[test]
    fn emits_message_start_on_first_chunk() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let chunk = parse(
            r#"{"id":"x","choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
        );
        let events = s.process_chunk(&chunk);
        assert!(events[0].contains("event: message_start"));
        assert!(events[0].contains("\"output_tokens\":1"));
    }

    #[test]
    fn text_content_emits_text_block_lifecycle() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
        ));
        let events = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"content":"Hello"}}]}"#,
        ));
        let joined = events.join("");
        assert!(joined.contains("event: content_block_start"));
        assert!(joined.contains("\"type\":\"text\""));
        assert!(joined.contains("event: content_block_delta"));
        assert!(joined.contains("\"text_delta\""));
        assert!(joined.contains("\"text\":\"Hello\""));
    }

    #[test]
    fn subsequent_text_chunks_only_emit_delta() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"content":"a"}}]}"#,
        ));
        let events = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"content":"b"}}]}"#,
        ));
        // 仅 1 个 delta，不再 emit content_block_start
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("content_block_delta"));
        assert!(events[0].contains("\"text\":\"b\""));
    }

    #[test]
    fn reasoning_content_emits_thinking_block() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
        ));
        let events = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"think..."}}]}"#,
        ));
        let joined = events.join("");
        assert!(joined.contains("\"type\":\"thinking\""));
        assert!(joined.contains("\"thinking_delta\""));
        assert!(joined.contains("\"thinking\":\"think...\""));
    }

    #[test]
    fn tool_call_first_delta_opens_block_with_id_and_name() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
        ));
        let events = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x","function":{"name":"search","arguments":""}}]}}]}"#,
        ));
        let joined = events.join("");
        assert!(joined.contains("content_block_start"));
        assert!(joined.contains("\"type\":\"tool_use\""));
        assert!(joined.contains("\"id\":\"call_x\""));
        assert!(joined.contains("\"name\":\"search\""));
    }

    #[test]
    fn tool_call_argument_deltas_emit_input_json_delta() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
        ));
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x","function":{"name":"search","arguments":""}}]}}]}"#,
        ));
        let events = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q"}}]}}]}"#,
        ));
        let joined = events.join("");
        assert!(joined.contains("\"input_json_delta\""));
        // arguments 增量原样塞 partial_json（注意是 Chat 流自己分块的形态）
        assert!(joined.contains("\"partial_json\":\"{\\\"q\""));
    }

    #[test]
    fn finalize_closes_open_blocks_and_emits_terminal_events() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"hi"}}]}"#,
        ));
        let _ = s.process_chunk(&parse(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"completion_tokens":5}}"#));
        let events = s.finalize();
        let joined = events.join("");
        assert!(joined.contains("content_block_stop"));
        assert!(joined.contains("event: message_delta"));
        assert!(joined.contains("\"stop_reason\":\"end_turn\""));
        assert!(joined.contains("\"output_tokens\":5"));
        assert!(joined.contains("event: message_stop"));
    }

    #[test]
    fn finish_reason_length_maps_to_max_tokens() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"content":"trunc"}}]}"#,
        ));
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#,
        ));
        let events = s.finalize();
        let joined = events.join("");
        assert!(joined.contains("\"stop_reason\":\"max_tokens\""));
    }

    #[test]
    fn parallel_tool_calls_get_distinct_anthropic_indices() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
        ));
        let events1 = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"a","arguments":""}}]}}]}"#,
        ));
        let events2 = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"c2","function":{"name":"b","arguments":""}}]}}]}"#,
        ));
        let joined1 = events1.join("");
        let joined2 = events2.join("");
        // 两个 tool_call_index → 两个独立 Anthropic block_start
        assert!(joined1.contains("\"index\":0"));
        assert!(joined2.contains("\"index\":1"));
        assert!(joined2.contains("\"id\":\"c2\""));
    }

    #[test]
    fn final_usage_carries_input_and_cache_read_tokens() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"content":"hi"}}]}"#,
        ));
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        ));
        let _ = s.process_chunk(&parse(
            r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":30}}}"#,
        ));
        let events = s.finalize();
        let delta = events
            .iter()
            .find(|e| e.starts_with("event: message_delta"))
            .expect("message_delta");
        let data: Value =
            serde_json::from_str(delta.lines().nth(1).unwrap().trim_start_matches("data: "))
                .unwrap();
        assert_eq!(data["usage"]["output_tokens"], 5);
        // OpenAI 语义 prompt_tokens(100) 已含 cached(30);Anthropic 的 input_tokens
        // 不含 cache_read,Claude Code 会把两者相加,必须扣掉避免 2× 上下文。
        assert_eq!(data["usage"]["input_tokens"], 70);
        assert_eq!(data["usage"]["cache_read_input_tokens"], 30);
    }

    #[test]
    fn tool_call_deltas_without_index_are_keyed_by_id() {
        // 部分上游并行 tool_call 不带 index 只带 id,不能全部并进 block 0。
        let mut s = ChatToAnthropicStream::new("claude-3");
        let mut events = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"id":"a","function":{"name":"f","arguments":"{\"x\":1}"}}]}}]}"#,
        ));
        events.extend(s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"id":"b","function":{"name":"g","arguments":""}}]}}]}"#,
        )));
        // 后续无 id 无 index 的参数增量归属最近打开的块
        events.extend(s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"function":{"arguments":"{\"y\":2}"}}]}}]}"#,
        )));
        let parsed: Vec<Value> = events
            .iter()
            .filter_map(|e| e.lines().nth(1))
            .map(|d| serde_json::from_str(d.trim_start_matches("data: ")).unwrap())
            .collect();
        let starts: Vec<(i64, String)> = parsed
            .iter()
            .filter(|v| {
                v["type"] == "content_block_start" && v["content_block"]["type"] == "tool_use"
            })
            .map(|v| {
                (
                    v["index"].as_i64().unwrap(),
                    v["content_block"]["id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(starts, vec![(0, "a".to_string()), (1, "b".to_string())]);
        let deltas: Vec<(i64, String)> = parsed
            .iter()
            .filter(|v| v["delta"]["type"] == "input_json_delta")
            .map(|v| {
                (
                    v["index"].as_i64().unwrap(),
                    v["delta"]["partial_json"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            deltas,
            vec![(0, r#"{"x":1}"#.to_string()), (1, r#"{"y":2}"#.to_string())]
        );
    }

    #[test]
    fn id_only_deltas_attach_to_block_opened_by_index() {
        // 首块带 index + id,后续块只带 id 不带 index:必须续到同一个 tool_use 块,
        // 不能再用同一个 id 打开第二个块。
        let mut s = ChatToAnthropicStream::new("claude-3");
        let mut events = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"f","arguments":""}}]}}]}"#,
        ));
        events.extend(s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"id":"a","function":{"arguments":"{\"x\":1}"}}]}}]}"#,
        )));
        let parsed: Vec<Value> = events
            .iter()
            .filter_map(|e| e.lines().nth(1))
            .map(|d| serde_json::from_str(d.trim_start_matches("data: ")).unwrap())
            .collect();
        let starts = parsed
            .iter()
            .filter(|v| {
                v["type"] == "content_block_start" && v["content_block"]["type"] == "tool_use"
            })
            .count();
        assert_eq!(starts, 1);
        let deltas: Vec<(i64, String)> = parsed
            .iter()
            .filter(|v| v["delta"]["type"] == "input_json_delta")
            .map(|v| {
                (
                    v["index"].as_i64().unwrap(),
                    v["delta"]["partial_json"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(deltas, vec![(0, r#"{"x":1}"#.to_string())]);
    }

    #[test]
    fn finalize_is_idempotent() {
        let mut s = ChatToAnthropicStream::new("claude-3");
        let _ = s.process_chunk(&parse(
            r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
        ));
        let first = s.finalize();
        let second = s.finalize();
        assert!(!first.is_empty());
        assert!(second.is_empty(), "second finalize must be no-op");
    }

    #[test]
    fn finalize_without_chunks_still_emits_message_start_and_stop() {
        // 罕见但要防御：上游一字节都没送就关闭，要 emit 合法的 Anthropic 流。
        let mut s = ChatToAnthropicStream::new("claude-3");
        let events = s.finalize();
        let joined = events.join("");
        assert!(joined.contains("message_start"));
        assert!(joined.contains("message_stop"));
    }

    #[test]
    fn message_id_format_matches_anthropic_convention() {
        let s = ChatToAnthropicStream::new("claude-3");
        assert!(s.message_id.starts_with("msg_"));
        assert_eq!(s.message_id.len(), "msg_".len() + 12);
    }
}
