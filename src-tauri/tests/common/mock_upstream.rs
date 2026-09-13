//! Mock upstream AI provider for offline capability tests.
//!
//! Wraps `wiremock` with helpers that speak the three protocols AgentGate
//! transforms between: OpenAI Chat Completions, OpenAI Responses, and
//! Anthropic Messages. Each stub method records the canned reply; callers
//! later inspect `received_requests()` to assert what AgentGate actually
//! sent upstream after L1 / L2 / L3 transforms.

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct MockUpstream {
    server: MockServer,
}

impl MockUpstream {
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// Base URL the gateway should use as the provider's `base_url` /
    /// `anthropic_base_url`. Already includes scheme + host + port, no path.
    pub fn url(&self) -> String {
        self.server.uri()
    }

    /// All requests the mock received since startup, in order. JSON body
    /// is best-effort parsed; non-JSON bodies surface as `Value::Null`.
    pub async fn received(&self) -> Vec<ReceivedRequest> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let body_json = serde_json::from_slice::<Value>(&r.body).unwrap_or(Value::Null);
                ReceivedRequest {
                    method: r.method.to_string(),
                    path: r.url.path().to_string(),
                    headers: r
                        .headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
                        .collect(),
                    body_raw: String::from_utf8_lossy(&r.body).to_string(),
                    body: body_json,
                }
            })
            .collect()
    }

    /// Stub `POST /v1/chat/completions` returning a minimal OpenAI-shaped
    /// chat completion. `model` and `content` are echoed in the response.
    pub async fn stub_chat_completions_ok(&self, model: &str, content: &str) {
        let body = chat_completion_body(model, content);
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `POST /v1/chat/completions` returning a chat completion that
    /// carries DeepSeek-style `reasoning_content`. Useful for verifying that
    /// the gateway preserves (does not strip) reasoning fields when the
    /// upstream model supports them.
    pub async fn stub_chat_completions_with_reasoning(
        &self,
        model: &str,
        reasoning: &str,
        content: &str,
    ) {
        let body = chat_completion_body_with_reasoning(model, reasoning, content);
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `POST /v1/chat/completions` returning the given status + JSON body.
    pub async fn stub_chat_completions_err(&self, status: u16, body: Value) {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `POST /v1/chat/completions` returning a streaming SSE body
    /// (`text/event-stream`): two content deltas (含 CJK,验证转换不破坏多字节)、
    /// 一个带 usage 的终块、`[DONE]`。用于端到端验证上游 chat SSE → 网关转换 →
    /// 客户端拿到合法的 responses / anthropic 事件流。
    pub async fn stub_chat_completions_sse(&self, model: &str) {
        let sse = format!(
            "data: {}\n\n\
             data: {}\n\n\
             data: {}\n\n\
             data: [DONE]\n\n",
            json!({"id":"c1","object":"chat.completion.chunk","model":model,
                   "choices":[{"index":0,"delta":{"role":"assistant","content":"你好"},"finish_reason":null}]}),
            json!({"id":"c1","object":"chat.completion.chunk","model":model,
                   "choices":[{"index":0,"delta":{"content":"世界"},"finish_reason":null}]}),
            json!({"id":"c1","object":"chat.completion.chunk","model":model,
                   "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                   "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}),
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse.into_bytes(), "text/event-stream"),
            )
            .mount(&self.server)
            .await;
    }

    /// Stub 任意 `POST {path}`:给定状态码 + content-type + 原始 body。
    /// 用于 SSE 错误帧、非 JSON 错误页等 `stub_*_ok/err` 覆盖不到的形态。
    pub async fn stub_raw(&self, route: &str, status: u16, content_type: &str, body: &str) {
        Mock::given(method("POST"))
            .and(path(route.to_string()))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("content-type", content_type)
                    .set_body_raw(body.as_bytes().to_vec(), content_type),
            )
            .mount(&self.server)
            .await;
    }

    /// Stub `POST /v1/messages` returning a minimal Anthropic message.
    pub async fn stub_anthropic_messages_ok(&self, model: &str, content: &str) {
        let body = anthropic_message_body(model, content);
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `GET /copilot_internal/v2/token` — GitHub token → Copilot bearer
    /// token 交换端点(配合 `AGENTGATE_COPILOT_GITHUB_API_BASE` 注入)。
    pub async fn stub_copilot_token_ok(&self, token: &str, expires_at: i64) {
        Mock::given(method("GET"))
            .and(path("/copilot_internal/v2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "token": token,
                "expires_at": expires_at,
                "refresh_in": 1500
            })))
            .mount(&self.server)
            .await;
    }

    /// Two-stage stub for MiMo's "web_search plugin not enabled" path:
    /// first call returns 400 with the upstream's plugin error marker
    /// (`webSearchEnabled is false`), every subsequent call returns 200.
    /// Lets tests verify the gateway strips `web_search` and retries.
    pub async fn stub_mimo_web_search_unavailable_then_ok(&self, model: &str, content: &str) {
        let err = json!({
            "error": {
                "message": "web_search tool found in the request body, but webSearchEnabled is false",
                "type": "invalid_request_error"
            }
        });
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(err))
            .up_to_n_times(1)
            .mount(&self.server)
            .await;
        self.stub_chat_completions_ok(model, content).await;
    }
}

/// 起一个只回一次的原始 HTTP/1.1 SSE 上游:先写响应头 + `first`,停 `gap`
/// 后再写 `second` 并关连接。wiremock 的 body 是一次性写出的,模拟不了
/// "首帧已转发给客户端、之后才出错"的时序,这里用裸 TCP 精确控制分段。
/// 返回 `(base_url, hits)`,`hits` 记录被连接的次数。
pub async fn start_split_sse_upstream(
    first: String,
    second: String,
    gap: std::time::Duration,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    start_trickle_sse_upstream(vec![first, second], gap).await
}

/// 裸 TCP SSE 上游:写完响应头后逐帧写 `frames`,帧与帧之间停 `gap`,写完关连接。
/// 用于模拟慢速长流(如客户端中途断开)。写失败(对端已断)时直接结束。
/// 返回 `(base_url, hits)`,`hits` 记录被连接的次数。
pub async fn start_trickle_sse_upstream(
    frames: Vec<String>,
    gap: std::time::Duration,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind trickle sse upstream");
    let addr = listener.local_addr().expect("local addr");
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let hits_task = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            hits_task.fetch_add(1, Ordering::SeqCst);
            let frames = frames.clone();
            tokio::spawn(async move {
                // 读完请求头 + body(按 content-length),避免对端还在写时就回包。
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(head_end) = text.find("\r\n\r\n") {
                        let len = text[..head_end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if buf.len() >= head_end + 4 + len {
                            break;
                        }
                    }
                }
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for (i, frame) in frames.iter().enumerate() {
                    if i > 0 {
                        tokio::time::sleep(gap).await;
                    }
                    if sock.write_all(frame.as_bytes()).await.is_err()
                        || sock.flush().await.is_err()
                    {
                        return;
                    }
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), hits)
}

#[derive(Debug, Clone)]
pub struct ReceivedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body_raw: String,
    pub body: Value,
}

impl ReceivedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn chat_completion_body(model: &str, content: &str) -> Value {
    json!({
        "id": "chatcmpl-mock-001",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6 }
    })
}

fn chat_completion_body_with_reasoning(model: &str, reasoning: &str, content: &str) -> Value {
    json!({
        "id": "chatcmpl-mock-002",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": content,
                "reasoning_content": reasoning
            },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 4, "completion_tokens": 6, "total_tokens": 10 }
    })
}

fn anthropic_message_body(model: &str, content: &str) -> Value {
    json!({
        "id": "msg_mock_001",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{ "type": "text", "text": content }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 4, "output_tokens": 2 }
    })
}
