use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::protocol::chat_completions::ChatCompletionChunk;
use crate::protocol::responses_events as ev;
use crate::transform::reasoning_store;
use crate::transform::responses_to_chat::{CommentaryStripper, ThinkSplitter};

const MAX_EVENTS_LOG_SIZE: usize = 1_000_000; // 1MB

/// 流式转换任务因客户端断开而提前结束时返回的错误文本。路由据此把请求日志
/// 记成 client disconnected,而不是上游失败。
pub const CLIENT_DISCONNECTED: &str = "client disconnected";

/// 单条 Responses 流的事件发送端:用本流自己的计数器给每个事件盖
/// `sequence_number`(之前是进程级全局计数器,并发流会互相重置 / 交错),
/// 并能判断客户端是否已断开。
#[derive(Clone)]
pub(crate) struct EventTx {
    tx: mpsc::Sender<String>,
    sequence: Arc<AtomicU64>,
}

impl EventTx {
    pub(crate) fn new(tx: mpsc::Sender<String>, sequence: Arc<AtomicU64>) -> Self {
        Self { tx, sequence }
    }

    /// 发送一个事件。客户端已断开时发送失败(返回 false),读循环通过
    /// [`EventTx::is_closed`] 停止继续读上游。
    pub(crate) async fn send(&self, event: &str) -> bool {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.tx.send(ev::with_sequence(event, seq)).await.is_ok()
    }

    /// 客户端(接收端)是否已经断开。
    pub(crate) fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}
/// Accumulated tool call from streaming deltas.
#[derive(Debug, Clone)]
pub struct AccumulatedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    /// Whether we have already emitted the output_item.added event for this call.
    emitted_added: bool,
    /// Track the last arguments length so we can compute per-delta output.
    last_args_len: usize,
    /// Responses 协议 output_index——added 时分配（用 `acc.next_output_index`），
    /// delta / done 一律复用这个值。**绝不能再用 `idx + 1` 算**：当流里先有
    /// reasoning（占了 next_output_index）再来 tool_call 时，两种算法值不同——
    /// Codex 看到 added 与 delta/done 的 output_index 对不上，直接判流断开。
    output_index: usize,
}

/// State accumulated during SSE stream processing.
pub struct SseAccumulator {
    pub response_id: String,
    pub msg_item_id: String,
    pub model: String,
    pub full_text: String,
    pub tool_calls: BTreeMap<usize, AccumulatedToolCall>,
    pub reasoning_content: String,
    pub usage: Option<serde_json::Value>,
    /// Web-search citations / annotations collected from delta.annotations
    /// as they stream in. Embedded into the final output_item.done message
    /// so the client sees the references.
    pub annotations: Vec<serde_json::Value>,
    pub events_log: String,
    events_size: usize,
    next_output_index: usize,
    msg_output_index: Option<usize>,
    text_content_started: bool,
    /// Output index reserved for the reasoning item when the first reasoning
    /// chunk arrives. `None` if no reasoning has streamed yet — set lazily so
    /// non-thinking responses don't emit a spurious empty reasoning item.
    reasoning_output_index: Option<usize>,
    /// Cached reasoning item id matching `output_item_added_reasoning`; used
    /// by subsequent deltas + the finalize close-out.
    reasoning_item_id: String,
    /// Last `choice.finish_reason` seen from upstream chunks. None until the
    /// terminal chunk arrives (Chat Completions emits finish_reason only on
    /// the last choice). Drives Responses `status`/`incomplete_details` mapping.
    pub finish_reason: Option<String>,
    /// Inline `<think>` 切分器（跨 chunk carry 半截标签）。
    /// 无 inline-think 上游也可安全用——content 透明透传。
    think_splitter: ThinkSplitter,
    /// `<commentary>` 标签剥离器。Codex prompt 让模型走 commentary channel，
    /// 第三方模型在正文里模仿输出字面标签——删标签保正文。无标签上游透传。
    commentary_stripper: CommentaryStripper,
    /// Streaming 期间累积的 finalized output items（reasoning / message /
    /// function_call 各自 done 时的 final JSON）。最后塞进
    /// `response.completed` envelope.output 字段。
    pub output_items: Vec<Value>,
    pub tool_call_resolution: crate::transform::tool_calls::ToolCallResolutionMap,
    /// 本流的事件序号(从 0 开始,每条流独立)。
    pub(crate) sequence: Arc<AtomicU64>,
}

impl SseAccumulator {
    pub fn new(response_id: String, model: String) -> Self {
        let msg_item_id = format!("msg_{}", response_id.replace("resp_", ""));
        let reasoning_item_id = format!("rs_{}", response_id.replace("resp_", ""));
        Self {
            response_id,
            msg_item_id,
            model,
            full_text: String::new(),
            tool_calls: BTreeMap::new(),
            reasoning_content: String::new(),
            usage: None,
            annotations: Vec::new(),
            events_log: String::new(),
            events_size: 0,
            next_output_index: 0,
            msg_output_index: None,
            text_content_started: false,
            reasoning_output_index: None,
            reasoning_item_id,
            finish_reason: None,
            think_splitter: ThinkSplitter::new(),
            commentary_stripper: CommentaryStripper::new(),
            output_items: Vec::new(),
            tool_call_resolution: Default::default(),
            sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    fn log_event(&mut self, event: &str) {
        let remaining = MAX_EVENTS_LOG_SIZE.saturating_sub(self.events_size);
        if remaining > 0 {
            // 必须按字符边界截:上限切在中文中间时字节切片会 panic,转换任务直接死掉。
            let slice = crate::gateway::stream_utf8::truncate_at_char_boundary(event, remaining);
            self.events_log.push_str(slice);
            self.events_log.push('\n');
            self.events_size += slice.len() + 1;
        }
    }

    pub fn tool_calls_list(&self) -> Vec<AccumulatedToolCall> {
        self.tool_calls.values().cloned().collect()
    }
}

/// Process upstream Chat Completions SSE and emit Responses API SSE events.
///
/// The caller is expected to have already run `sse_bootstrap::bootstrap_detect`
/// on the upstream response so any HTTP-200-with-error-frame failure mode has
/// been turned into a clean Err that triggers failover before we commit to
/// streaming. The `Bootstrap.prefix` carries whatever bytes the bootstrap
/// scan already consumed — we replay those first, then drain the live stream.
pub async fn process_upstream_stream(
    boot: crate::gateway::sse_bootstrap::Bootstrap,
    tx: mpsc::Sender<String>,
    acc: &mut SseAccumulator,
) -> Result<(), String> {
    process_upstream_stream_inner(boot, tx, acc, true, true).await
}

/// 与 [`process_upstream_stream`] 相同，但允许调用方控制是否发 `response.created` /
/// `response.in_progress` / `response.completed` 这几个**流级别**事件。
pub async fn process_upstream_stream_inner(
    boot: crate::gateway::sse_bootstrap::Bootstrap,
    tx: mpsc::Sender<String>,
    acc: &mut SseAccumulator,
    emit_open: bool,
    emit_completed: bool,
) -> Result<(), String> {
    let tx = EventTx::new(tx, acc.sequence.clone());
    if emit_open {
        // 1. response.created + in_progress
        send(&tx, &ev::response_created(&acc.response_id, &acc.model)).await;
        send(&tx, &ev::response_in_progress(&acc.response_id, &acc.model)).await;
    }

    // NOTE: output_item.added is deferred until first text delta to avoid
    // emitting a spurious empty message item for tool-call-only responses.

    let mut stream = boot.stream;
    let mut utf8_pending: Vec<u8> = Vec::new();
    let mut buffer = String::new();
    crate::gateway::stream_utf8::append_utf8_safe(&mut buffer, &mut utf8_pending, &boot.prefix);
    let mut has_text = false;
    let mut has_tool_calls = false;
    let mut message_item_emitted = false;

    // Drain the prefix buffer before we ever poll the live stream — preserves
    // any complete SSE frames that bootstrap already pulled.
    loop {
        // First, parse out any complete lines already buffered.
        for line in crate::gateway::sse_buffer::take_complete_lines(&mut buffer) {
            match dispatch_line(
                &line,
                &tx,
                acc,
                &mut has_text,
                &mut has_tool_calls,
                &mut message_item_emitted,
                emit_completed,
            )
            .await
            {
                LineOutcome::Continue => {}
                LineOutcome::Done(result) => return result,
            }
        }

        // 客户端已断开:停止读上游,不再继续烧 token。
        if tx.is_closed() {
            return Err(CLIENT_DISCONNECTED.to_string());
        }

        // Then pull more from upstream.
        let chunk_bytes = match stream.next().await {
            Some(Ok(b)) => b,
            Some(Err(e)) => {
                let msg = crate::gateway::sse_bootstrap::describe_stream_error(&e);
                send(
                    &tx,
                    &ev::response_failed(&acc.response_id, &acc.model, &msg),
                )
                .await;
                return Err(msg);
            }
            None => {
                // 流自然结束（上游关连接、未发 [DONE]）。一律走 finalize
                // → response.completed。注意 `has_text || has_tool_calls` 守卫
                // 已经移除：纯 reasoning 响应（o1 / DeepSeek-R1 / MiMo 在某些
                // prompt 下 final content 为空、全部内容在 reasoning_content）
                // 之前被错判 failed，触发 Codex"流没处理完就断开"。
                return finalize(tx, acc, has_text, has_tool_calls, emit_completed).await;
            }
        };

        crate::gateway::stream_utf8::append_utf8_safe(&mut buffer, &mut utf8_pending, &chunk_bytes);
    }
}

enum LineOutcome {
    Continue,
    Done(Result<(), String>),
}

/// Per-line SSE dispatch — shared between the prefix replay and the live
/// stream so a frame straddling the boundary is handled identically.
async fn dispatch_line(
    line: &str,
    tx: &EventTx,
    acc: &mut SseAccumulator,
    has_text: &mut bool,
    has_tool_calls: &mut bool,
    message_item_emitted: &mut bool,
    emit_completed: bool,
) -> LineOutcome {
    if line.is_empty() || line.starts_with(':') {
        return LineOutcome::Continue;
    }

    let Some(data) = line.strip_prefix("data:").map(|d| d.trim()) else {
        return LineOutcome::Continue;
    };

    if data == "[DONE]" {
        return LineOutcome::Done(
            finalize(tx.clone(), acc, *has_text, *has_tool_calls, emit_completed).await,
        );
    }

    acc.log_event(data);

    let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) else {
        return LineOutcome::Continue;
    };

    if let Some(ref u) = chunk.usage {
        acc.usage = Some(normalize_usage(u));
    }

    let Some(choices) = &chunk.choices else {
        return LineOutcome::Continue;
    };

    process_choices(
        choices,
        tx,
        acc,
        has_text,
        has_tool_calls,
        message_item_emitted,
    )
    .await;

    LineOutcome::Continue
}

async fn process_choices(
    choices: &[crate::protocol::chat_completions::ChunkChoice],
    tx: &EventTx,
    acc: &mut SseAccumulator,
    has_text: &mut bool,
    has_tool_calls: &mut bool,
    message_item_emitted: &mut bool,
) {
    for choice in choices {
        // finish_reason 通常只在终块（delta 为空）出现。先抓后处理 delta，
        // 这样即使 delta 为空也能记录 stop 原因供 finalize 映射 status。
        if let Some(ref fr) = choice.finish_reason {
            if !fr.is_empty() {
                acc.finish_reason = Some(fr.clone());
            }
        }
        let Some(delta) = &choice.delta else { continue };

        // ── Text content (with stateful <think> tag splitting) ──
        // ThinkSplitter 跨 chunk carry 半截标签（旧的无状态 split_think_tags
        // 在 chunk 边界恰好切到 `<thi` 或 `</th` 时会把残留泄进 visible 文本）。
        if let Some(ref content) = delta.content {
            if !content.is_empty() {
                let (text, thinking) = acc.think_splitter.process_chunk(content);
                if let Some(ref tk) = thinking {
                    // reasoning 走自己的 reasoning_item，绝不开 message item
                    // (Bug #5 修复：旧代码这里误发 output_item_added_message，
                    //  reasoning 自己有 stream_reasoning_delta 内部处理 added)
                    if acc.reasoning_content.is_empty() {
                        acc.reasoning_content.push_str("**Thinking**\n\n");
                    }
                    acc.reasoning_content.push_str(tk);
                    stream_reasoning_delta(tx, acc, tk).await;
                }
                // Codex prompt 让模型走 commentary channel，第三方模型只能在
                // 正文里模仿输出字面 <commentary> 标签——删标签保正文。
                let text = acc.commentary_stripper.process_chunk(&text);
                if !text.is_empty() {
                    if !*message_item_emitted {
                        start_message_item(tx, acc).await;
                        *message_item_emitted = true;
                    }
                    *has_text = true;
                    acc.full_text.push_str(&text);
                    let oi = acc.msg_output_index.unwrap_or(0);
                    send(tx, &ev::output_text_delta(&acc.msg_item_id, oi, 0, &text)).await;
                }
            }
        }

        // ── Reasoning content（独立字段，DeepSeek-R1 / o1 / GLM-Z1 用）──
        // Bug #5 修复：删掉了"先 emit message added"那段冗余/错误代码——reasoning
        // 完全独立于 message item，自己有 reasoning_item_id 由 stream_reasoning_delta
        // 内部 emit `output_item_added_reasoning`。原代码误发 message added 导致纯
        // reasoning 响应时 Codex 收到孤儿 message item.added 没有对应 done。
        if let Some(ref rc) = delta.reasoning_content {
            if !rc.is_empty() {
                if acc.reasoning_content.is_empty() {
                    acc.reasoning_content.push_str("**Thinking**\n\n");
                }
                acc.reasoning_content.push_str(rc);
                stream_reasoning_delta(tx, acc, rc).await;
            }
        }

        // ── reasoning_details array (o3/o4 native) ──
        if let Some(ref details) = delta.reasoning_details {
            for detail in details {
                if let Some(text) = detail.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        if acc.reasoning_content.is_empty() {
                            acc.reasoning_content.push_str("**Thinking**\n\n");
                        }
                        acc.reasoning_content.push_str(text);
                        stream_reasoning_delta(tx, acc, text).await;
                    }
                }
            }
        }

        // ── Web-search annotations / citations ──
        // MiMo emits these on the first streaming chunk; OpenAI's
        // search-preview models emit per chunk. Forward each as an
        // `output_text.annotation.added` event so the client can
        // surface citations in real time, and accumulate them for the
        // final output_item.done message.
        if let Some(ref anns) = delta.annotations {
            if !anns.is_empty() && !*message_item_emitted {
                start_message_item(tx, acc).await;
                *message_item_emitted = true;
            }
            for ann in anns {
                let ann = ev::normalize_annotation(ann);
                let annotation_index = acc.annotations.len();
                let oi = acc.msg_output_index.unwrap_or(0);
                send(
                    tx,
                    &ev::output_text_annotation_added(
                        &acc.msg_item_id,
                        oi,
                        0,
                        annotation_index,
                        &ann,
                    ),
                )
                .await;
                acc.annotations.push(ann);
            }
        }

        // ── Legacy delta.function_call → synthetic tool_call ──
        if let Some(ref fc) = delta.function_call {
            *has_tool_calls = true;
            let idx = 0usize;
            if !acc.tool_calls.contains_key(&idx) {
                acc.tool_calls.insert(
                    idx,
                    AccumulatedToolCall {
                        id: format!("call_legacy_{}", acc.response_id.replace("resp_", "")),
                        name: String::new(),
                        arguments: String::new(),
                        emitted_added: false,
                        last_args_len: 0,
                        output_index: 0,
                    },
                );
            }
            let tc = acc.tool_calls.get_mut(&idx).unwrap();
            if let Some(name) = fc.get("name").and_then(|n| n.as_str()) {
                tc.name.push_str(name);
            }
            if let Some(args) = fc.get("arguments").and_then(|a| a.as_str()) {
                tc.arguments.push_str(args);
            }
            if !tc.emitted_added && !tc.name.is_empty() {
                let item_id = format!("fc_{}", tc.id);
                let oi = acc.next_output_index;
                acc.next_output_index += 1;
                tc.output_index = oi;
                // #1 namespace 还原
                send_tool_call_added(
                    tx,
                    &acc.tool_call_resolution,
                    &item_id,
                    oi,
                    &tc.id,
                    &tc.name,
                )
                .await;
                tc.emitted_added = true;
            }
            if tc.emitted_added && tc.arguments.len() > tc.last_args_len {
                let delta_args = &tc.arguments[tc.last_args_len..];
                let item_id = format!("fc_{}", tc.id);
                send_tool_call_arguments_delta(
                    tx,
                    &acc.tool_call_resolution,
                    &item_id,
                    tc.output_index,
                    &tc.name,
                    delta_args,
                )
                .await;
                tc.last_args_len = tc.arguments.len();
            }
        }

        // ── Tool calls (streaming deltas) ──
        if let Some(ref tcs) = delta.tool_calls {
            *has_tool_calls = true;
            for tc_delta in tcs {
                let idx = tc_delta.index.unwrap_or(0) as usize;

                let tc = acc
                    .tool_calls
                    .entry(idx)
                    .or_insert_with(|| AccumulatedToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                        emitted_added: false,
                        last_args_len: 0,
                        output_index: 0,
                    });

                if let Some(ref id) = tc_delta.id {
                    if tc.id.is_empty() {
                        tc.id = crate::transform::tool_calls::sanitize_call_id(id).into_owned();
                    }
                }

                if let Some(ref func) = tc_delta.function {
                    if let Some(ref name) = func.name {
                        tc.name.push_str(name);
                    }
                    if let Some(ref args) = func.arguments {
                        tc.arguments.push_str(args);
                    }
                }

                if tc.id.is_empty() {
                    tc.id = format!("call_{}_{}", acc.response_id.replace("resp_", ""), idx);
                }

                // Bug #6 修复：首个 chunk 看到这个 tool_call idx 就 emit added，
                // 不再等 name 非空。某些上游（罕见但有）首块只发 id，name 后到——
                // 旧版 gate 会一直不发 added，后续 arguments 也被 gate 掉，整个
                // 调用静默丢失。name 后到时不重发 added，
                // （openToolCall 用 name ?? ""）。
                if !tc.emitted_added {
                    let item_id = format!("fc_{}", tc.id);
                    let oi = acc.next_output_index;
                    acc.next_output_index += 1;
                    tc.output_index = oi;
                    send_tool_call_added(
                        tx,
                        &acc.tool_call_resolution,
                        &item_id,
                        oi,
                        &tc.id,
                        &tc.name,
                    )
                    .await;
                    tc.emitted_added = true;
                }

                if tc.emitted_added && tc.arguments.len() > tc.last_args_len {
                    let delta_args = &tc.arguments[tc.last_args_len..];
                    let item_id = format!("fc_{}", tc.id);
                    send_tool_call_arguments_delta(
                        tx,
                        &acc.tool_call_resolution,
                        &item_id,
                        tc.output_index,
                        &tc.name,
                        delta_args,
                    )
                    .await;
                    tc.last_args_len = tc.arguments.len();
                }
            }
        }
    }
}

async fn finalize(
    tx: EventTx,
    acc: &mut SseAccumulator,
    has_text: bool,
    has_tool_calls: bool,
    emit_completed: bool,
) -> Result<(), String> {
    // Flush ThinkSplitter carry：上游 stream 末尾如果残留半截 `<thi` 这类标签，
    // 按当前 in_think 状态 emit 出去（in_think → reasoning，否则按字面文本进 message）。
    let (flush_text, flush_reasoning) = acc.think_splitter.flush();
    if let Some(tk) = flush_reasoning {
        if acc.reasoning_content.is_empty() {
            acc.reasoning_content.push_str("**Thinking**\n\n");
        }
        acc.reasoning_content.push_str(&tk);
        stream_reasoning_delta(&tx, acc, &tk).await;
    }
    // flush_text 极少数情况（chunk 末尾的 `<thi` 等假阳性 carry），按 text 流出去。
    // commentary stripper 同样可能有 carry 残留，一并 flush。
    let mut flush_text = acc.commentary_stripper.process_chunk(&flush_text);
    flush_text.push_str(&acc.commentary_stripper.flush());
    let extra_text = if !flush_text.is_empty() {
        acc.full_text.push_str(&flush_text);
        Some(flush_text)
    } else {
        None
    };
    let had_flush_text = extra_text.is_some();

    let rc_owned = if acc.reasoning_content.is_empty() {
        None
    } else {
        Some(acc.reasoning_content.clone())
    };
    let rc = rc_owned.as_deref();

    // Store reasoning for future requests (in-memory LRU, process-local —
    // survives within the same process for the same conversation thread).
    if !acc.reasoning_content.is_empty() {
        let tc_ids: Vec<String> = acc.tool_calls.values().map(|tc| tc.id.clone()).collect();
        reasoning_store::store(&acc.model, &acc.full_text, &acc.reasoning_content, &tc_ids);
    }

    // Pin reasoning into a dedicated `reasoning` output_item with
    // `encrypted_content`. Codex round-trips this verbatim, so the trace
    // survives process restarts and multi-turn tool calls preserve their
    // reasoning_content (required by MiMo / DeepSeek thinking-mode upstream).
    if !acc.reasoning_content.is_empty() {
        // Reuse the oi reserved during streaming if delta events were emitted;
        // otherwise (non-streaming reasoning paths, providers that surface
        // reasoning only in the final chunk) allocate fresh — keeps
        // back-compat with consumers that don't subscribe to delta events.
        //
        // Bug #8 修复：fallback 路径用独立 alloc（先 ++ 占位），不再与已经
        // 被 tool_calls 抢过的 next_output_index 重叠——之前裸 unwrap_or 会
        // 让 reasoning 与第一个 tool_call 拿到同一个 oi，Codex 报 item 冲突。
        let oi = match acc.reasoning_output_index {
            Some(oi) => oi,
            None => {
                let oi = acc.next_output_index;
                acc.next_output_index += 1;
                oi
            }
        };
        if acc.reasoning_output_index.is_some() {
            // Streamed: close out the summary text before the item.done.
            send(
                &tx,
                &ev::reasoning_summary_text_done(
                    &acc.reasoning_item_id,
                    oi,
                    0,
                    &acc.reasoning_content,
                ),
            )
            .await;
            send(
                &tx,
                &ev::reasoning_summary_part_done(
                    &acc.reasoning_item_id,
                    oi,
                    0,
                    &acc.reasoning_content,
                ),
            )
            .await;
        }
        send(
            &tx,
            &ev::output_item_done_reasoning(&acc.reasoning_item_id, oi, &acc.reasoning_content),
        )
        .await;
        // 累积进 envelope.output（Bug #4）
        acc.output_items.push(json!({
            "id": acc.reasoning_item_id,
            "type": "reasoning",
            "status": "completed",
            "summary": [{"type": "summary_text", "text": &acc.reasoning_content}],
            "encrypted_content": &acc.reasoning_content,
        }));
    }

    // Close text content if any
    if acc.msg_output_index.is_some() || has_text || had_flush_text {
        // 如果只是 flush_text（无之前的 text delta）需要补 added 事件，但 has_text=true
        // 时之前已经发过 output_item.added + content_part.added。flush_text 单独场景
        // 极罕见（splitter 末尾误判半截标签）——优先保持流可以正常 complete，宁可
        // text 略冗余，不重发 added。
        let oi = start_message_item(&tx, acc).await;
        if let Some(extra) = extra_text {
            send(&tx, &ev::output_text_delta(&acc.msg_item_id, oi, 0, &extra)).await;
        }
        send(
            &tx,
            &ev::output_text_done(&acc.msg_item_id, oi, 0, &acc.full_text),
        )
        .await;
        send(
            &tx,
            &ev::content_part_done_with_annotations(
                &acc.msg_item_id,
                oi,
                0,
                &acc.full_text,
                &acc.annotations,
            ),
        )
        .await;
        send(
            &tx,
            &ev::output_item_done_message_with_annotations(
                &acc.msg_item_id,
                oi,
                &acc.full_text,
                rc,
                &acc.annotations,
            ),
        )
        .await;
        // 累积 message item 进 envelope.output（Bug #4）
        let mut msg_item = json!({
            "id": acc.msg_item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": &acc.full_text, "annotations": &acc.annotations}],
        });
        if let Some(r) = rc {
            msg_item["reasoning_content"] = json!(r);
        }
        acc.output_items.push(msg_item);
    }
    // Tool-call-only: no message item was emitted, no need to close it

    // Close tool calls. 用 tc.output_index（不是 idx + 1）+ JSON salvage + namespace 还原。
    if has_tool_calls {
        for tc in acc.tool_calls.values() {
            let item_id = format!("fc_{}", tc.id);
            let oi = tc.output_index;
            let item = send_tool_call_done(
                &tx,
                &acc.tool_call_resolution,
                &item_id,
                oi,
                &tc.id,
                &tc.name,
                &tc.arguments,
                rc,
                acc.finish_reason.as_deref(),
            )
            .await;
            acc.output_items.push(item);
        }
    }

    // response.completed with usage + finish_reason → Responses status/incomplete_details 映射
    // 同时把累积的 output_items 塞进 envelope.output（Bug #4 协议契约完整性）
    if emit_completed {
        send(
            &tx,
            &ev::response_completed_with_stop_reason(
                &acc.response_id,
                &acc.model,
                acc.usage.as_ref(),
                acc.finish_reason.as_deref(),
                &acc.output_items,
            ),
        )
        .await;
    }
    Ok(())
}

async fn send(tx: &EventTx, event: &str) {
    // 发送失败 = 客户端断开;这里不中断收尾逻辑,由读循环的 is_closed 检查停止读上游。
    let _ = tx.send(event).await;
}

pub(crate) async fn send_tool_call_added(
    tx: &EventTx,
    resolution: &crate::transform::tool_calls::ToolCallResolutionMap,
    item_id: &str,
    output_index: usize,
    call_id: &str,
    chat_name: &str,
) {
    let kind = crate::transform::tool_calls::resolve_tool_call_response_kind(chat_name, resolution);
    match kind {
        crate::transform::tool_calls::ToolCallResponseKind::Function { name, namespace } => {
            send(
                tx,
                &ev::function_call_added_with_namespace(
                    item_id,
                    output_index,
                    call_id,
                    &name,
                    namespace.as_deref(),
                ),
            )
            .await;
        }
        crate::transform::tool_calls::ToolCallResponseKind::Custom { name } => {
            send(
                tx,
                &ev::custom_tool_call_added(item_id, output_index, call_id, &name),
            )
            .await;
        }
        crate::transform::tool_calls::ToolCallResponseKind::ToolSearch => {
            send(tx, &ev::tool_search_call_added(output_index, call_id)).await;
        }
    }
}

pub(crate) async fn send_tool_call_arguments_delta(
    tx: &EventTx,
    resolution: &crate::transform::tool_calls::ToolCallResolutionMap,
    item_id: &str,
    output_index: usize,
    chat_name: &str,
    delta_args: &str,
) {
    let kind = crate::transform::tool_calls::resolve_tool_call_response_kind(chat_name, resolution);
    if matches!(
        kind,
        crate::transform::tool_calls::ToolCallResponseKind::Function { .. }
    ) {
        send(
            tx,
            &ev::function_call_arguments_delta(item_id, output_index, delta_args),
        )
        .await;
    }
}

pub(crate) async fn send_tool_call_done(
    tx: &EventTx,
    resolution: &crate::transform::tool_calls::ToolCallResolutionMap,
    item_id: &str,
    output_index: usize,
    call_id: &str,
    chat_name: &str,
    arguments: &str,
    reasoning_content: Option<&str>,
    finish_reason: Option<&str>,
) -> Value {
    let kind = crate::transform::tool_calls::resolve_tool_call_response_kind(chat_name, resolution);
    match kind {
        crate::transform::tool_calls::ToolCallResponseKind::Function { name, namespace } => {
            let arguments = crate::transform::tool_calls::salvage_tool_arguments(
                arguments,
                chat_name,
                call_id,
                finish_reason,
            );
            send(
                tx,
                &ev::function_call_arguments_done(item_id, output_index, &arguments),
            )
            .await;
            send(
                tx,
                &ev::function_call_done_with_namespace(
                    item_id,
                    output_index,
                    call_id,
                    &name,
                    &arguments,
                    reasoning_content,
                    namespace.as_deref(),
                ),
            )
            .await;
            let mut item = json!({
                "id": item_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
            });
            if let Some(rc) = reasoning_content {
                item["reasoning_content"] = json!(rc);
            }
            if let Some(ns) = namespace {
                item["namespace"] = json!(ns);
            }
            item
        }
        crate::transform::tool_calls::ToolCallResponseKind::Custom { name } => {
            let input = crate::transform::tool_calls::custom_tool_input_from_arguments(arguments);
            if !input.is_empty() {
                send(
                    tx,
                    &ev::custom_tool_call_input_delta(item_id, output_index, &input),
                )
                .await;
            }
            send(
                tx,
                &ev::custom_tool_call_input_done(item_id, output_index, &input),
            )
            .await;
            send(
                tx,
                &ev::custom_tool_call_done(
                    item_id,
                    output_index,
                    call_id,
                    &name,
                    &input,
                    reasoning_content,
                ),
            )
            .await;
            let mut item = json!({
                "id": item_id,
                "type": "custom_tool_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "input": input,
            });
            if let Some(rc) = reasoning_content {
                item["reasoning_content"] = json!(rc);
            }
            item
        }
        crate::transform::tool_calls::ToolCallResponseKind::ToolSearch => {
            let arguments_value =
                crate::transform::tool_calls::tool_search_arguments_from_arguments(arguments);
            send(
                tx,
                &ev::tool_search_call_done(
                    output_index,
                    call_id,
                    &arguments_value,
                    reasoning_content,
                ),
            )
            .await;
            let mut item = json!({
                "type": "tool_search_call",
                "status": "completed",
                "call_id": call_id,
                "execution": "client",
                "arguments": arguments_value,
            });
            if let Some(rc) = reasoning_content {
                item["reasoning_content"] = json!(rc);
            }
            item
        }
    }
}

async fn start_message_item(tx: &EventTx, acc: &mut SseAccumulator) -> usize {
    if let Some(oi) = acc.msg_output_index {
        return oi;
    }
    let oi = acc.next_output_index;
    acc.next_output_index += 1;
    acc.msg_output_index = Some(oi);
    send(tx, &ev::output_item_added_message(&acc.msg_item_id, oi)).await;
    send(tx, &ev::content_part_added(&acc.msg_item_id, oi, 0)).await;
    acc.text_content_started = true;
    oi
}

// salvage_tool_arguments 已提到 transform/tool_calls.rs 公共模块，
// 流式 / 非流式 / 入站 history 三处共用同一份逻辑。

/// 发送一段 reasoning 增量。首次调用会顺手把 reasoning 的 output_item
/// 占位事件先打出去（output_item.added），后续仅发 delta。output_index
/// 在首次时从 `acc.next_output_index` 抢占——这样和 tool_calls 共用
/// 同一个递增空间，不会冲突。
async fn stream_reasoning_delta(tx: &EventTx, acc: &mut SseAccumulator, delta: &str) {
    if delta.is_empty() {
        return;
    }
    let oi = match acc.reasoning_output_index {
        Some(oi) => oi,
        None => {
            let oi = acc.next_output_index;
            acc.next_output_index += 1;
            acc.reasoning_output_index = Some(oi);
            send(
                tx,
                &ev::output_item_added_reasoning(&acc.reasoning_item_id, oi),
            )
            .await;
            send(
                tx,
                &ev::reasoning_summary_part_added(&acc.reasoning_item_id, oi, 0),
            )
            .await;
            oi
        }
    };
    send(
        tx,
        &ev::reasoning_summary_text_delta(&acc.reasoning_item_id, oi, 0, delta),
    )
    .await;
}

/// Normalize upstream usage to Responses API format.
fn normalize_usage(u: &serde_json::Value) -> serde_json::Value {
    let input = u
        .get("prompt_tokens")
        .or(u.get("input_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let output = u
        .get("completion_tokens")
        .or(u.get("output_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let cached = u
        .get("prompt_cache_hit_tokens")
        .or(u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens")))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let reasoning = u
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    serde_json::json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": input + output,
        "input_tokens_details": { "cached_tokens": cached },
        "output_tokens_details": { "reasoning_tokens": reasoning }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_normalize_usage_openai_format() {
        let u = json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150
        });
        let result = normalize_usage(&u);
        assert_eq!(result["input_tokens"], 100);
        assert_eq!(result["output_tokens"], 50);
        assert_eq!(result["total_tokens"], 150);
        assert_eq!(result["input_tokens_details"]["cached_tokens"], 0);
        assert_eq!(result["output_tokens_details"]["reasoning_tokens"], 0);
    }

    #[test]
    fn test_normalize_usage_deepseek_format() {
        let u = json!({
            "input_tokens": 200,
            "output_tokens": 80,
            "total_tokens": 280,
            "prompt_cache_hit_tokens": 50,
            "completion_tokens_details": { "reasoning_tokens": 30 }
        });
        let result = normalize_usage(&u);
        assert_eq!(result["input_tokens"], 200);
        assert_eq!(result["output_tokens"], 80);
        assert_eq!(result["total_tokens"], 280);
        assert_eq!(result["input_tokens_details"]["cached_tokens"], 50);
        assert_eq!(result["output_tokens_details"]["reasoning_tokens"], 30);
    }

    #[test]
    fn test_normalize_usage_prompt_tokens_details_cached() {
        let u = json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "prompt_tokens_details": { "cached_tokens": 40 }
        });
        let result = normalize_usage(&u);
        assert_eq!(result["input_tokens_details"]["cached_tokens"], 40);
    }

    #[test]
    fn test_normalize_usage_empty() {
        let u = json!({});
        let result = normalize_usage(&u);
        assert_eq!(result["input_tokens"], 0);
        assert_eq!(result["output_tokens"], 0);
        assert_eq!(result["total_tokens"], 0);
    }

    #[test]
    fn test_sse_accumulator_new() {
        let acc = SseAccumulator::new("resp_abc".to_string(), "gpt-4".to_string());
        assert_eq!(acc.response_id, "resp_abc");
        assert_eq!(acc.model, "gpt-4");
        assert_eq!(acc.msg_item_id, "msg_abc");
        assert!(acc.full_text.is_empty());
        assert!(acc.tool_calls.is_empty());
        assert!(acc.reasoning_content.is_empty());
        assert!(acc.usage.is_none());
        assert!(acc.events_log.is_empty());
    }

    #[test]
    fn test_sse_accumulator_tool_calls_list_empty() {
        let acc = SseAccumulator::new("resp_1".to_string(), "m".to_string());
        assert!(acc.tool_calls_list().is_empty());
    }

    #[test]
    fn test_sse_accumulator_log_event() {
        let mut acc = SseAccumulator::new("resp_1".to_string(), "m".to_string());
        acc.log_event("event1");
        acc.log_event("event2");
        assert!(acc.events_log.contains("event1\n"));
        assert!(acc.events_log.contains("event2\n"));
    }

    #[test]
    fn test_sse_accumulator_log_event_truncation() {
        let mut acc = SseAccumulator::new("resp_1".to_string(), "m".to_string());
        let big = "x".repeat(MAX_EVENTS_LOG_SIZE + 1000);
        acc.log_event(&big);
        // log_event should cap at MAX_EVENTS_LOG_SIZE, never exceed it (plus 1 for newline)
        assert!(acc.events_size <= MAX_EVENTS_LOG_SIZE + 1);
        assert_eq!(acc.events_log.len(), MAX_EVENTS_LOG_SIZE + 1); // truncated content + newline
    }

    #[test]
    fn test_accumulated_tool_call_fields() {
        let tc = AccumulatedToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: "{\"q\":\"hi\"}".to_string(),
            emitted_added: false,
            last_args_len: 0,
            output_index: 0,
        };
        assert_eq!(tc.id, "call_1");
        assert_eq!(tc.name, "search");
        assert_eq!(tc.arguments, "{\"q\":\"hi\"}");
    }

    #[tokio::test]
    async fn stream_reasoning_delta_first_chunk_emits_added_and_delta() {
        let mut acc = SseAccumulator::new("resp_xyz".to_string(), "deepseek".to_string());
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let tx = EventTx::new(tx, acc.sequence.clone());
        stream_reasoning_delta(&tx, &mut acc, "Hello").await;
        drop(tx);

        let first = rx.recv().await.expect("expected added event");
        let second = rx.recv().await.expect("expected part added event");
        let third = rx.recv().await.expect("expected delta event");
        assert!(first.contains("response.output_item.added"), "got: {first}");
        assert!(first.contains("\"type\":\"reasoning\""), "got: {first}");
        assert!(first.contains("\"output_index\":0"), "got: {first}");
        assert!(first.contains("rs_xyz"), "got: {first}");
        assert!(
            second.contains("response.reasoning_summary_part.added"),
            "got: {second}"
        );
        assert!(second.contains("\"output_index\":0"), "got: {second}");
        assert!(
            third.contains("response.reasoning_summary_text.delta"),
            "got: {third}"
        );
        assert!(third.contains("\"delta\":\"Hello\""), "got: {third}");
        assert!(
            rx.recv().await.is_none(),
            "only added + part + delta on first chunk"
        );
        assert_eq!(acc.reasoning_output_index, Some(0));
        // next_output_index advanced so tool calls won't reuse the reasoning slot.
        assert_eq!(acc.next_output_index, 1);
    }

    #[tokio::test]
    async fn stream_reasoning_delta_subsequent_chunks_emit_delta_only() {
        let mut acc = SseAccumulator::new("resp_xyz".to_string(), "deepseek".to_string());
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let tx = EventTx::new(tx, acc.sequence.clone());
        stream_reasoning_delta(&tx, &mut acc, "A").await;
        // Drain the first three events (added + summary part + delta).
        let _ = rx.recv().await;
        let _ = rx.recv().await;
        let _ = rx.recv().await;
        stream_reasoning_delta(&tx, &mut acc, "B").await;
        drop(tx);

        let second_delta = rx.recv().await.expect("second delta");
        assert!(second_delta.contains("response.reasoning_summary_text.delta"));
        assert!(second_delta.contains("\"delta\":\"B\""));
        assert!(rx.recv().await.is_none(), "no second added event");
        assert_eq!(acc.next_output_index, 1, "index reserved only once");
    }

    #[tokio::test]
    async fn stream_reasoning_delta_empty_is_noop() {
        let mut acc = SseAccumulator::new("resp_xyz".to_string(), "deepseek".to_string());
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let tx = EventTx::new(tx, acc.sequence.clone());
        stream_reasoning_delta(&tx, &mut acc, "").await;
        drop(tx);

        assert!(
            rx.recv().await.is_none(),
            "empty delta must not emit events"
        );
        assert!(acc.reasoning_output_index.is_none());
    }

    /// 按 `n` 个上游 chunk 计数的上游流:每被读走一个 chunk 计数 +1。
    fn counted_upstream(
        n: usize,
        frame: String,
    ) -> (
        crate::gateway::sse_bootstrap::Bootstrap,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use futures::StreamExt as _;
        let polled = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = polled.clone();
        let stream = futures::stream::iter(0..n)
            .map(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, reqwest::Error>(bytes::Bytes::from(frame.clone()))
            })
            .boxed();
        (
            crate::gateway::sse_bootstrap::Bootstrap {
                prefix: Vec::new(),
                stream,
            },
            polled,
        )
    }

    fn sequence_numbers(events: &[String]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|e| {
                let data = e.lines().find_map(|l| l.strip_prefix("data: "))?;
                serde_json::from_str::<Value>(data)
                    .ok()?
                    .get("sequence_number")?
                    .as_u64()
            })
            .collect()
    }

    #[tokio::test]
    async fn client_disconnect_stops_reading_upstream() {
        let frame = format!(
            "data: {}\n\n",
            json!({"choices":[{"index":0,"delta":{"content":"x"}}]})
        );
        let (boot, polled) = counted_upstream(200, frame);
        let (tx, rx) = mpsc::channel::<String>(1024);
        drop(rx); // 客户端已断开
        let mut acc = SseAccumulator::new("resp_gone".to_string(), "m".to_string());

        let result = process_upstream_stream(boot, tx, &mut acc).await;

        assert!(
            result.is_err(),
            "client disconnect must end the stream as an error"
        );
        assert!(
            polled.load(std::sync::atomic::Ordering::SeqCst) < 200,
            "upstream must not be drained after the client is gone"
        );
    }

    #[tokio::test]
    async fn concurrent_streams_have_independent_sequence_numbers() {
        use futures::StreamExt as _;
        let frame = |t: &str| {
            bytes::Bytes::from(format!(
                "data: {}\n\n",
                json!({"choices":[{"index":0,"delta":{"content":t}}]})
            ))
        };
        let (up_a, rx_a) = mpsc::channel::<bytes::Bytes>(4);
        let (up_b, rx_b) = mpsc::channel::<bytes::Bytes>(4);
        let boot = |rx| crate::gateway::sse_bootstrap::Bootstrap {
            prefix: Vec::new(),
            stream: tokio_stream::wrappers::ReceiverStream::new(rx)
                .map(Ok::<_, reqwest::Error>)
                .boxed(),
        };
        let (tx_a, mut out_a) = mpsc::channel::<String>(256);
        let (tx_b, mut out_b) = mpsc::channel::<String>(256);
        let mut acc_a = SseAccumulator::new("resp_a".to_string(), "m".to_string());
        let mut acc_b = SseAccumulator::new("resp_b".to_string(), "m".to_string());

        let feeder = async move {
            for _ in 0..5 {
                up_a.send(frame("a")).await.unwrap();
                tokio::task::yield_now().await;
                up_b.send(frame("b")).await.unwrap();
                tokio::task::yield_now().await;
            }
        };
        let (ra, rb, ()) = tokio::join!(
            process_upstream_stream(boot(rx_a), tx_a, &mut acc_a),
            process_upstream_stream(boot(rx_b), tx_b, &mut acc_b),
            feeder
        );
        ra.unwrap();
        rb.unwrap();

        for out in [&mut out_a, &mut out_b] {
            let mut events = Vec::new();
            while let Some(e) = out.recv().await {
                events.push(e);
            }
            let seqs = sequence_numbers(&events);
            assert!(
                seqs.len() > 5,
                "expected a full event stream, got {events:#?}"
            );
            assert_eq!(
                seqs,
                (0..seqs.len() as u64).collect::<Vec<_>>(),
                "each stream must number its own events 0..n without gaps"
            );
        }
    }

    #[test]
    fn log_event_truncates_multibyte_text_at_cap_without_panic() {
        // 1MB 上限恰好切在中文字符中间:之前 `&event[..to_add]` 直接 panic,
        // 转换任务死掉,客户端流收不到 response.completed。
        let mut acc = SseAccumulator::new("resp_cap".to_string(), "m".to_string());
        acc.events_size = MAX_EVENTS_LOG_SIZE - 4;
        acc.log_event("你好世界");
        assert!(acc.events_log.starts_with("你"));
        assert!(!acc.events_log.contains("好"));
        assert!(acc.events_size <= MAX_EVENTS_LOG_SIZE + 1);
    }

    fn bootstrap_from_prefix(prefix: &str) -> crate::gateway::sse_bootstrap::Bootstrap {
        use bytes::Bytes;
        use futures::{stream, StreamExt as _};

        crate::gateway::sse_bootstrap::Bootstrap {
            prefix: prefix.as_bytes().to_vec(),
            stream: stream::empty::<Result<Bytes, reqwest::Error>>().boxed(),
        }
    }

    async fn collect_stream_events(prefix: &str) -> (Vec<String>, SseAccumulator) {
        let boot = bootstrap_from_prefix(prefix);
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let mut acc = SseAccumulator::new("resp_test".to_string(), "mimo-v2.5-pro".to_string());

        process_upstream_stream_inner(boot, tx, &mut acc, true, true)
            .await
            .expect("stream processing should succeed");

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        (events, acc)
    }

    fn position(events: &[String], needle: &str) -> usize {
        events
            .iter()
            .position(|event| event.contains(needle))
            .unwrap_or_else(|| panic!("missing event containing {needle}; events: {events:#?}"))
    }

    #[tokio::test]
    async fn annotations_open_message_before_annotation_event() {
        let prefix = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"mimo-v2.5-pro\",\"choices\":[{\"index\":0,\"delta\":{\"annotations\":[{\"type\":\"url_citation\",\"url\":\"https://a.com\",\"title\":\"A\",\"summary\":\"S\"}]},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"mimo-v2.5-pro\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"answer\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n"
        );

        let (events, acc) = collect_stream_events(prefix).await;
        let msg_added = position(&events, "\"type\":\"message\"");
        let content_added = position(&events, "response.content_part.added");
        let annotation_added = position(&events, "response.output_text.annotation.added");
        let msg_done = position(&events, "response.output_item.done");
        assert!(msg_added < annotation_added, "message item must open first");
        assert!(
            content_added < annotation_added,
            "content part must open first"
        );
        assert!(
            annotation_added < msg_done,
            "message item must close after annotation"
        );
        assert!(events[annotation_added].contains("\"output_index\":0"));
        assert!(events[annotation_added].contains("\"snippet\":\"S\""));
        assert!(!events[annotation_added].contains("\"summary\""));
        assert_eq!(acc.msg_output_index, Some(0));
        assert_eq!(acc.annotations[0]["snippet"], "S");
    }

    #[tokio::test]
    async fn commentary_tags_stripped_from_streamed_text() {
        // Codex prompt 让模型走 commentary channel,第三方模型只能在正文里
        // 模仿输出字面 <commentary> 标签,且 chunk 边界可能切在标签中间。
        let prefix = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"mimo-v2.5-pro\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"<commen\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"mimo-v2.5-pro\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"tary>正在检查工作区</comm\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"mimo-v2.5-pro\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"entary>好了\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let (events, acc) = collect_stream_events(prefix).await;
        assert_eq!(acc.full_text, "正在检查工作区好了");
        for event in &events {
            assert!(
                !event.contains("commentary"),
                "commentary tag leaked into event: {event}"
            );
        }
    }

    #[tokio::test]
    async fn reasoning_then_text_allocates_output_indexes_in_stream_order() {
        let prefix = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"mimo-v2.5-pro\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"thinking\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"mimo-v2.5-pro\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"answer\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n"
        );

        let (events, acc) = collect_stream_events(prefix).await;
        let reasoning_added = events
            .iter()
            .find(|event| {
                event.contains("response.output_item.added")
                    && event.contains("\"type\":\"reasoning\"")
            })
            .expect("reasoning added event");
        let message_added = events
            .iter()
            .find(|event| {
                event.contains("response.output_item.added")
                    && event.contains("\"type\":\"message\"")
            })
            .expect("message added event");
        assert!(
            reasoning_added.contains("\"output_index\":0"),
            "got: {reasoning_added}"
        );
        assert!(
            message_added.contains("\"output_index\":1"),
            "got: {message_added}"
        );
        assert_eq!(acc.reasoning_output_index, Some(0));
        assert_eq!(acc.msg_output_index, Some(1));
        assert_eq!(acc.next_output_index, 2);
    }
}
