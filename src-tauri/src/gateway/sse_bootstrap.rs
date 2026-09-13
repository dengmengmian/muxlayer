//! Pre-stream validation of upstream SSE responses.
//!
//! Some providers (MiMo, GLM, DeepSeek under certain quota states) return HTTP
//! 200 and then immediately emit an `event: error` or a `data: {"error":...}`
//! frame instead of a real completion. Without intervention the client sees a
//! broken stream — Codex / CC then fail with a generic 500 or hang.
//!
//! `bootstrap_detect` consumes the leading window of the upstream stream
//! BEFORE any byte reaches the client, scans for error markers, and either:
//!   - returns Err with a synthesized status (429/401/403/etc.) so the routes
//!     loop can fail over to the next candidate as if the upstream had
//!     returned that status on the HTTP head, or
//!   - returns the already-consumed prefix bytes plus the remaining live
//!     stream so the regular consumer can resume without losing any byte.
//!
//! Borrowed from codex-switcher's `proxy_bootstrap_byte_cap` design.

use crate::errors::AppError;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::StreamExt;

/// How many bytes to scan before giving up and assuming the stream is healthy.
/// Sized so we always see the first SSE frame even for upstreams that emit a
/// short keepalive / `event: ping` first.
const BOOTSTRAP_MAX_BYTES: usize = 16 * 1024;

/// Hard ceiling on bootstrap latency. Thinking-mode upstreams (MiMo / DeepSeek)
/// can take 5-10s to emit the first reasoning chunk, so we keep this generous;
/// only used to defend against an upstream that opened the body and then
/// stalled completely.
const BOOTSTRAP_TIMEOUT_MS: u64 = 15_000;

pub type ChunkStream = BoxStream<'static, Result<Bytes, reqwest::Error>>;

pub struct Bootstrap {
    /// Bytes already pulled from the upstream during the scan. The downstream
    /// consumer must process these BEFORE pulling more from `stream`.
    pub prefix: Vec<u8>,
    /// Remaining live stream — pulls continue from wherever the scan stopped.
    pub stream: ChunkStream,
}

pub async fn bootstrap_detect(response: reqwest::Response) -> Result<Bootstrap, AppError> {
    let stream: ChunkStream = response.bytes_stream().boxed();
    bootstrap_detect_stream(stream, BOOTSTRAP_MAX_BYTES, BOOTSTRAP_TIMEOUT_MS).await
}

/// Inner driver — split out so tests can feed a synthetic stream without
/// spinning up an HTTP server.
async fn bootstrap_detect_stream(
    mut stream: ChunkStream,
    max_bytes: usize,
    timeout_ms: u64,
) -> Result<Bootstrap, AppError> {
    let mut buffer: Vec<u8> = Vec::with_capacity(4096);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);

    loop {
        if let Some(err) = scan_for_error(&buffer) {
            return Err(err);
        }
        if has_valid_data_event(&buffer) {
            return Ok(Bootstrap {
                prefix: buffer,
                stream,
            });
        }
        if buffer.len() >= max_bytes {
            return Ok(Bootstrap {
                prefix: buffer,
                stream,
            });
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(Bootstrap {
                prefix: buffer,
                stream,
            });
        }

        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                buffer.extend_from_slice(&chunk);
            }
            Ok(Some(Err(e))) => {
                return Err(AppError::new(
                    crate::errors::codes::PROVIDER_REQUEST_FAILED,
                    format!("Upstream connection failed during bootstrap: {e}"),
                ));
            }
            Ok(None) => {
                return Ok(Bootstrap {
                    prefix: buffer,
                    stream,
                });
            }
            Err(_) => {
                return Ok(Bootstrap {
                    prefix: buffer,
                    stream,
                });
            }
        }
    }
}

/// Scan the buffered prefix for an upstream-emitted error event. Returns
/// `Some(AppError)` carrying the synthesized upstream status, so
/// `failover::upstream_status_of` routes failover decisions accordingly.
fn scan_for_error(buf: &[u8]) -> Option<AppError> {
    let text = String::from_utf8_lossy(buf);
    let mut last_event: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() || trimmed.starts_with(':') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("event:") {
            let name = rest.trim().to_string();
            if name == "error" || name.contains("error") {
                let detail = next_data_payload(&text, line).unwrap_or_default();
                let status = classify_status("", "", &detail);
                return Some(make_stream_error(
                    status,
                    "upstream emitted error event",
                    &detail,
                ));
            }
            last_event = Some(name);
            continue;
        }
        if let Some(data) = trimmed.strip_prefix("data:").map(str::trim) {
            if data == "[DONE]" {
                continue;
            }
            // Anthropic-style: previous `event: error` followed by a data frame.
            if last_event.as_deref() == Some("error") {
                let status = classify_status("", "", data);
                return Some(make_stream_error(
                    status,
                    "upstream emitted error event",
                    data,
                ));
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(err_obj) = v.get("error") {
                    let msg = err_obj
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("upstream error");
                    let code = err_obj.get("code").and_then(|c| c.as_str()).unwrap_or("");
                    let typ = err_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    let status = classify_status(code, typ, msg);
                    return Some(make_stream_error(status, msg, data));
                }
            }
            last_event = None;
        }
    }
    None
}

fn has_valid_data_event(buf: &[u8]) -> bool {
    let text = String::from_utf8_lossy(buf);
    for line in text.lines() {
        let trimmed = line.trim_end_matches('\r');
        if let Some(data) = trimmed.strip_prefix("data:").map(str::trim) {
            if data == "[DONE]" {
                return true;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                // Treat any frame whose `error` field is set as NOT-yet-valid;
                // scan_for_error owns the failure path.
                if v.get("error").is_some() {
                    continue;
                }
                if v.get("choices").is_some()
                    || v.get("type").is_some()
                    || v.get("usage").is_some()
                    || v.get("candidates").is_some()
                {
                    return true;
                }
            }
        }
    }
    false
}

fn classify_status(code: &str, typ: &str, msg: &str) -> u16 {
    let blob = format!("{code} {typ} {msg}").to_lowercase();
    if blob.contains("rate_limit")
        || blob.contains("ratelimit")
        || blob.contains("quota")
        || blob.contains("too many requests")
        || blob.contains("insufficient_balance")
    {
        429
    } else if blob.contains("unauthorized")
        || blob.contains("invalid_api_key")
        || blob.contains("authentication")
        || blob.contains("invalid api key")
    {
        401
    } else if blob.contains("forbidden") || blob.contains("permission") {
        403
    } else if blob.contains("context_length") || blob.contains("max_tokens") {
        413
    } else {
        502
    }
}

fn make_stream_error(status: u16, msg: &str, detail: &str) -> AppError {
    AppError::new(
        crate::errors::codes::UPSTREAM_STREAM_ERROR,
        format!("Provider returned HTTP {status}: {msg}"),
    )
    .with_detail(format!("Provider returned HTTP {status}; {detail}"))
    .with_upstream_status(status)
}

/// 从 [`make_stream_error`] 产出的错误里取回上游错误帧的 `error.type` / `error.message`。
/// detail 形如 `Provider returned HTTP {status}; {帧 data}`;帧不是 JSON 时返回 None。
pub fn error_frame_fields(err: &AppError) -> (Option<String>, Option<String>) {
    let frame = err
        .detail
        .as_deref()
        .and_then(|d| d.split_once("; "))
        .and_then(|(_, data)| serde_json::from_str::<serde_json::Value>(data.trim()).ok());
    let field = |key: &str| {
        frame
            .as_ref()
            .and_then(|v| v.pointer(&format!("/error/{key}")))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    (field("type"), field("message"))
}

/// 把上游 `bytes_stream()` 抛出的 `reqwest::Error` 转成人类可读的消息。
///
/// 当 `read_timeout` 触发（连续 600s 上游没发任何字节）时给出明确的中文文案，
/// 指明这是 idle timeout 而非整请求超时；其它错误（连接 RST、解码失败等）
/// 维持原 reqwest 错误描述。各 SSE/pass-through 处理器在 `Some(Err(e))` 分支
/// 调用这个函数生成给下游的 error message / response.failed event。
pub fn describe_stream_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return format!(
            "上游响应静默超过 {} 秒未发送新数据，MuxLayer 已主动放弃等待。\
             常见原因：thinking 模型 + 长 prompt 首字过慢、上游短暂抖动、或上游本身卡住。\
             建议重发请求；若 route profile 有其它 candidate，下一次会自动转到下个 provider。",
            STREAM_READ_IDLE_HINT_SECS
        );
    }
    format!("上游响应流读取失败：{e}。可能是网络抖动或上游主动断连。请重试。")
}

/// HTTP 层 SSE idle read timeout。必须与 `gateway::server` 里 `.read_timeout(...)`
/// 保持一致；server 直接引用这个常量，错误文案也用同一个数字。
pub const STREAM_READ_IDLE_HINT_SECS: u64 = 600;

fn next_data_payload(text: &str, event_line: &str) -> Option<String> {
    let mut iter = text.lines();
    while let Some(l) = iter.next() {
        if l == event_line {
            for next in iter {
                let trimmed = next.trim_end_matches('\r');
                if let Some(d) = trimmed.strip_prefix("data:") {
                    return Some(d.trim().to_string());
                }
                if trimmed.is_empty() {
                    break;
                }
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_picks_up_openai_style_data_error_with_quota() {
        let buf = b"data: {\"error\":{\"message\":\"You exceeded your current quota\",\"code\":\"insufficient_quota\",\"type\":\"insufficient_quota\"}}\n\n";
        let err = scan_for_error(buf).expect("should detect quota error");
        assert_eq!(err.code, "UPSTREAM_STREAM_ERROR");
        assert_eq!(err.upstream_status(), Some(429));
        assert!(err.message.contains("HTTP 429"), "got: {}", err.message);
    }

    #[test]
    fn scan_picks_up_event_error_then_data() {
        // Anthropic-style:
        // event: error
        // data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}
        let buf = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let err = scan_for_error(buf).expect("event: error should trigger detection");
        assert_eq!(err.code, "UPSTREAM_STREAM_ERROR");
    }

    #[test]
    fn scan_picks_up_rate_limit_error() {
        let buf = b"data: {\"error\":{\"message\":\"Rate limit reached for ...\",\"type\":\"rate_limit_exceeded\"}}\n\n";
        let err = scan_for_error(buf).expect("rate_limit should be detected");
        assert!(err.message.contains("HTTP 429"));
    }

    #[test]
    fn scan_picks_up_unauthorized() {
        let buf =
            b"data: {\"error\":{\"message\":\"Invalid API key\",\"code\":\"invalid_api_key\"}}\n\n";
        let err = scan_for_error(buf).expect("invalid api key should be detected");
        assert!(err.message.contains("HTTP 401"));
    }

    #[test]
    fn scan_ignores_valid_chat_completion_chunk() {
        let buf =
            b"data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n";
        assert!(scan_for_error(buf).is_none());
    }

    #[test]
    fn scan_ignores_anthropic_message_start() {
        let buf = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x\"}}\n\n";
        assert!(scan_for_error(buf).is_none());
    }

    #[test]
    fn scan_ignores_done_marker() {
        let buf = b"data: [DONE]\n\n";
        assert!(scan_for_error(buf).is_none());
    }

    #[test]
    fn has_valid_data_event_recognizes_chat_chunk() {
        let buf = b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n";
        assert!(has_valid_data_event(buf));
    }

    #[test]
    fn has_valid_data_event_recognizes_anthropic_typed_event() {
        let buf = b"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n";
        assert!(has_valid_data_event(buf));
    }

    #[test]
    fn has_valid_data_event_recognizes_done() {
        let buf = b"data: [DONE]\n\n";
        assert!(has_valid_data_event(buf));
    }

    #[test]
    fn has_valid_data_event_false_for_empty_buffer() {
        assert!(!has_valid_data_event(b""));
    }

    #[test]
    fn has_valid_data_event_false_for_keepalive_only() {
        let buf = b": keepalive\n\n";
        assert!(!has_valid_data_event(buf));
    }

    #[test]
    fn has_valid_data_event_skips_error_frame_so_scan_can_fail() {
        // We've seen a data frame, but it's an error. has_valid_data_event
        // must say "no, keep scanning" so scan_for_error owns the decision.
        let buf = b"data: {\"error\":{\"message\":\"boom\"}}\n\n";
        assert!(!has_valid_data_event(buf));
    }

    #[test]
    fn classify_status_routes_well_known_codes() {
        assert_eq!(classify_status("rate_limit_exceeded", "", ""), 429);
        assert_eq!(classify_status("", "rate_limit", ""), 429);
        assert_eq!(classify_status("", "", "quota exceeded"), 429);
        assert_eq!(classify_status("invalid_api_key", "", ""), 401);
        assert_eq!(classify_status("", "", "Unauthorized"), 401);
        assert_eq!(classify_status("", "", "permission denied"), 403);
        assert_eq!(classify_status("", "", "context_length_exceeded"), 413);
        assert_eq!(classify_status("", "", "unknown failure"), 502);
    }

    #[test]
    fn stream_read_idle_timeout_allows_long_thinking_prefill() {
        assert_eq!(STREAM_READ_IDLE_HINT_SECS, 600);
    }

    fn make_stream(chunks: Vec<&'static [u8]>) -> ChunkStream {
        let iter = chunks
            .into_iter()
            .map(|c| Ok::<Bytes, reqwest::Error>(Bytes::from_static(c)));
        futures::stream::iter(iter).boxed()
    }

    #[tokio::test]
    async fn bootstrap_returns_err_on_quota_in_first_chunk() {
        let stream = make_stream(vec![
            b"data: {\"error\":{\"message\":\"quota\",\"code\":\"insufficient_quota\"}}\n\n",
        ]);
        let err = bootstrap_detect_stream(stream, BOOTSTRAP_MAX_BYTES, 1_000)
            .await
            .err()
            .expect("quota frame should yield Err");
        assert!(err.message.contains("HTTP 429"), "{}", err.message);
    }

    #[tokio::test]
    async fn bootstrap_passes_through_valid_chat_chunk() {
        let stream = make_stream(vec![
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"}}]}\n\n",
        ]);
        let boot = bootstrap_detect_stream(stream, BOOTSTRAP_MAX_BYTES, 1_000)
            .await
            .expect("valid chunk should pass");
        // First frame consumed into prefix, the rest remains in the stream.
        assert!(String::from_utf8_lossy(&boot.prefix).contains("hi"));
        // We don't assert the remainder count — only that the stream survived
        // and the prefix is the complete first frame.
    }

    #[tokio::test]
    async fn bootstrap_passes_through_anthropic_message_start() {
        let stream = make_stream(vec![
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"x\"}}\n\n",
        ]);
        let boot = bootstrap_detect_stream(stream, BOOTSTRAP_MAX_BYTES, 1_000)
            .await
            .expect("anthropic typed event should pass");
        assert!(boot.prefix.windows(13).any(|w| w == b"message_start"));
    }

    #[tokio::test]
    async fn bootstrap_returns_err_on_event_error_then_data() {
        let stream = make_stream(vec![
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow down\"}}\n\n",
        ]);
        let err = bootstrap_detect_stream(stream, BOOTSTRAP_MAX_BYTES, 1_000)
            .await
            .err()
            .expect("event: error should yield Err");
        assert!(err.message.contains("HTTP 429"), "{}", err.message);
    }

    #[tokio::test]
    async fn bootstrap_passes_when_stream_ends_without_data() {
        // Provider closed the stream cleanly. Caller's normal "Stream ended
        // without [DONE]" handling takes over.
        let stream: ChunkStream = futures::stream::empty().boxed();
        let boot = bootstrap_detect_stream(stream, BOOTSTRAP_MAX_BYTES, 1_000)
            .await
            .expect("empty stream is allowed through");
        assert!(boot.prefix.is_empty());
    }

    #[tokio::test]
    async fn bootstrap_passes_when_window_fills_with_keepalives() {
        // A pathological upstream that emits comment keepalives only —
        // bootstrap shouldn't hang forever, it should give up at the
        // byte cap and pass through.
        let chunk = b": keepalive\n\n";
        // Build 100 copies — easily exceeds default 16KB? No, 13 bytes * 100
        // = 1300 bytes. Use a smaller max_bytes so the test is fast.
        let chunks: Vec<&'static [u8]> = (0..100).map(|_| chunk.as_slice()).collect();
        let stream = make_stream(chunks);
        let boot = bootstrap_detect_stream(stream, 256, 1_000)
            .await
            .expect("byte cap exit is OK, not Err");
        assert!(boot.prefix.len() >= 256);
    }
}
