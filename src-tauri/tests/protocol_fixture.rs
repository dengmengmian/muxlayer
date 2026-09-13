//! L1 protocol conversion + L2 model-mapping regression tests.
//!
//! Mirrors the env-gated 8.1–8.9 block in `smoke_test.rs` but runs fully
//! offline against a wiremock upstream. The real smoke test stays as the
//! "true integration" verification — these are the CI-blockers that catch
//! regressions to the transform/mapping plumbing without needing keys.

mod common;

use common::gateway_harness::{GatewayHarness, ProviderSpec};
use common::mock_upstream::MockUpstream;
use serde_json::json;

// ── L1: protocol conversion ─────────────────────────────────────────────

#[tokio::test]
async fn l1_responses_to_anthropic_transform() {
    // Codex (/v1/responses) → Anthropic-typed provider with anthropic_base_url.
    // The gateway must convert to Anthropic Messages shape and hit /v1/messages.
    let mock = MockUpstream::start().await;
    mock.stub_anthropic_messages_ok("claude-sonnet-4-6", "ok")
        .await;

    let mut spec = ProviderSpec::chat_only("anthropic", "claude-sonnet-4-6");
    spec.protocol = r#"["anthropic_messages"]"#.to_string();
    spec.anthropic_base_url = Some(mock.url());

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "claude-sonnet-4-6",
            "input": "ping",
            "stream": false,
            "max_output_tokens": 16,
        }))
        .send()
        .await
        .expect("send /v1/responses");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].path, "/v1/messages",
        "must hit Anthropic Messages path"
    );
    // Anthropic shape requires a `messages` array, not Responses-style `input`.
    assert!(
        received[0].body.get("messages").is_some(),
        "upstream should receive Anthropic-shaped messages array: {}",
        received[0].body_raw
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn l1_chat_to_anthropic_non_stream_transform() {
    // Generic Chat client → Anthropic-typed provider with anthropic_base_url.
    // Goes through client_chat_to_anthropic_handle.
    let mock = MockUpstream::start().await;
    mock.stub_anthropic_messages_ok("claude-sonnet-4-6", "ok")
        .await;

    let mut spec = ProviderSpec::chat_only("anthropic", "claude-sonnet-4-6");
    spec.protocol = r#"["anthropic_messages"]"#.to_string();
    spec.anthropic_base_url = Some(mock.url());

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": "ping" }],
            "stream": false,
            "max_tokens": 16,
        }))
        .send()
        .await
        .expect("send /v1/chat/completions");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].path, "/v1/messages");
    assert!(
        received[0].body.get("messages").is_some(),
        "upstream should receive Anthropic-shaped body"
    );
    // Chat → Anthropic should also forward an explicit max_tokens.
    assert!(
        received[0].body.get("max_tokens").is_some(),
        "max_tokens should pass through to Anthropic upstream"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn l1_messages_to_chat_fallback_transform() {
    // Claude Code (/v1/messages) → Chat-only provider (no anthropic_base_url).
    // Gateway must fall back to the Messages → Chat translator and hit
    // /v1/chat/completions upstream with a Chat-shaped body.
    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("custom-model", "ok").await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "custom-model"), &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "custom-model",
            "max_tokens": 16,
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .expect("send /v1/messages");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].path, "/v1/chat/completions");
    let messages = received[0].body["messages"].as_array().expect("messages");
    assert!(!messages.is_empty());
    // Anthropic input uses `max_tokens` at the top level; Chat uses the same name
    // — the assertion that matters is the path + flat content shape.

    harness.shutdown().await;
}

// ── L3: vision-aware failover routing ───────────────────────────────────
//
// Regression for the asymmetry where /v1/chat/completions and /v1/messages
// did NOT skip vision-incapable providers for image requests (only
// /v1/responses did). An image request must route to the vision-capable
// candidate and never touch the non-vision primary.

#[tokio::test]
async fn messages_image_routes_to_vision_provider() {
    // Primary (non-vision) + vision candidate, failover mode. An image-bearing
    // /v1/messages request must land on the vision provider's upstream.
    let primary_mock = MockUpstream::start().await;
    primary_mock
        .stub_chat_completions_ok("novis-model", "ok")
        .await;
    let vision_mock = MockUpstream::start().await;
    vision_mock
        .stub_chat_completions_ok("vis-model", "ok")
        .await;

    let harness = GatewayHarness::start(
        ProviderSpec::chat_only("custom", "novis-model"),
        &primary_mock,
    )
    .await;
    harness.add_vision_failover_candidate(&vision_mock, "vis-model");
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "novis-model",
            "max_tokens": 16,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "describe this" },
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } }
                ]
            }]
        }))
        .send()
        .await
        .expect("send /v1/messages");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let vision_hits = vision_mock.received().await;
    let primary_hits = primary_mock.received().await;
    assert_eq!(
        vision_hits.len(),
        1,
        "image request must reach the vision provider"
    );
    assert_eq!(
        primary_hits.len(),
        0,
        "non-vision primary must be skipped for image requests"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn chat_completions_image_routes_to_vision_provider() {
    // Same guarantee on the /v1/chat/completions entry.
    let primary_mock = MockUpstream::start().await;
    primary_mock
        .stub_chat_completions_ok("novis-model", "ok")
        .await;
    let vision_mock = MockUpstream::start().await;
    vision_mock
        .stub_chat_completions_ok("vis-model", "ok")
        .await;

    let harness = GatewayHarness::start(
        ProviderSpec::chat_only("custom", "novis-model"),
        &primary_mock,
    )
    .await;
    harness.add_vision_failover_candidate(&vision_mock, "vis-model");
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "novis-model",
            "max_tokens": 16,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "describe this" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }
                ]
            }]
        }))
        .send()
        .await
        .expect("send /v1/chat/completions");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let vision_hits = vision_mock.received().await;
    let primary_hits = primary_mock.received().await;
    assert_eq!(
        vision_hits.len(),
        1,
        "image request must reach the vision provider"
    );
    assert_eq!(
        primary_hits.len(),
        0,
        "non-vision primary must be skipped for image requests"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn messages_text_only_stays_on_primary() {
    // Control: a text-only /v1/messages request must NOT be rerouted — it stays
    // on the primary even though a vision candidate exists.
    let primary_mock = MockUpstream::start().await;
    primary_mock
        .stub_chat_completions_ok("novis-model", "ok")
        .await;
    let vision_mock = MockUpstream::start().await;
    vision_mock
        .stub_chat_completions_ok("vis-model", "ok")
        .await;

    let harness = GatewayHarness::start(
        ProviderSpec::chat_only("custom", "novis-model"),
        &primary_mock,
    )
    .await;
    harness.add_vision_failover_candidate(&vision_mock, "vis-model");
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "novis-model",
            "max_tokens": 16,
            "messages": [{ "role": "user", "content": "just text" }]
        }))
        .send()
        .await
        .expect("send /v1/messages");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let primary_hits = primary_mock.received().await;
    assert_eq!(
        primary_hits.len(),
        1,
        "text-only request must stay on the primary provider"
    );

    harness.shutdown().await;
}

// ── L2: model mapping + agentgate virtual model ─────────────────────────

#[tokio::test]
async fn l2_responses_endpoint_applies_model_mapping() {
    // Codex sends model="gpt-5", provider mapping rewrites to "deepseek-v4-pro".
    // The mock upstream must observe the post-mapping name.
    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("deepseek-v4-pro", "ok").await;

    let spec = ProviderSpec::chat_only("custom", "deepseek-v4-pro")
        .with_mapping(r#"{"gpt-5":"deepseek-v4-pro"}"#);

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "gpt-5",
            "input": "ping",
            "stream": false,
            "max_output_tokens": 16,
        }))
        .send()
        .await
        .expect("send /v1/responses");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].body["model"], "deepseek-v4-pro");

    harness.shutdown().await;
}

#[tokio::test]
async fn l2_chat_endpoint_applies_model_mapping() {
    // Generic Chat client sends gpt-5, provider rewrites to deepseek-v4-pro.
    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("deepseek-v4-pro", "ok").await;

    let spec = ProviderSpec::chat_only("custom", "deepseek-v4-pro")
        .with_mapping(r#"{"gpt-5":"deepseek-v4-pro"}"#);

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "gpt-5",
            "messages": [{ "role": "user", "content": "ping" }],
            "stream": false,
            "max_tokens": 16,
        }))
        .send()
        .await
        .expect("send /v1/chat/completions");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].body["model"], "deepseek-v4-pro");

    harness.shutdown().await;
}

#[tokio::test]
async fn l2_messages_endpoint_applies_model_mapping() {
    // Claude Code sends claude-sonnet-4-6, provider rewrites to deepseek-v4-pro
    // on the Anthropic passthrough path. The mock /v1/messages must observe
    // the post-mapping name (and not a [1m] qualifier we didn't ask for).
    let mock = MockUpstream::start().await;
    mock.stub_anthropic_messages_ok("deepseek-v4-pro", "ok")
        .await;

    let spec = ProviderSpec::chat_only("deepseek", "deepseek-v4-pro")
        .with_anthropic(mock.url())
        .with_mapping(r#"{"claude-sonnet-4-6":"deepseek-v4-pro"}"#);

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 16,
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .expect("send /v1/messages");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].path, "/v1/messages");
    assert_eq!(received[0].body["model"], "deepseek-v4-pro");

    harness.shutdown().await;
}

#[tokio::test]
async fn l2_muxlayer_virtual_model_resolves_to_real_model() {
    // New client apply writes `muxlayer` as the virtual model so the gateway
    // can pick whichever real model the active route lands on.
    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("real-model-v1", "ok").await;

    let spec = ProviderSpec::chat_only("custom", "real-model-v1");
    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "muxlayer",
            "messages": [{ "role": "user", "content": "ping" }],
            "stream": false,
            "max_tokens": 16,
        }))
        .send()
        .await
        .expect("send /v1/chat/completions");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].body["model"], "real-model-v1",
        "virtual `muxlayer` should resolve to the provider's default model"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn l2_agentgate_virtual_model_resolves_to_real_model() {
    // Old client configs still send `agentgate`; keep it as an alias of muxlayer.
    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("real-model-v1", "ok").await;

    let spec = ProviderSpec::chat_only("custom", "real-model-v1");
    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "agentgate",
            "messages": [{ "role": "user", "content": "ping" }],
            "stream": false,
            "max_tokens": 16,
        }))
        .send()
        .await
        .expect("send /v1/chat/completions");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let received = mock.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].body["model"], "real-model-v1",
        "virtual `agentgate` should resolve to the provider's default model"
    );

    harness.shutdown().await;
}

// ── L4: error-triggered failover ────────────────────────────────────────
//
// 故障自愈的核心承诺:主 provider 返回 5xx 时自动切到下一个候选,客户端无感。
// 此前集成层从未验证过——只有能力路由(vision)的 e2e。这里 stub 主上游 500、
// 次上游 200,断言两个上游都被打到且客户端拿到 200。

#[tokio::test]
async fn responses_failover_on_upstream_500() {
    let primary = MockUpstream::start().await;
    primary
        .stub_chat_completions_err(500, json!({"error": {"message": "boom"}}))
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_ok("backup-model", "recovered")
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "primary-model"), &primary).await;
    harness.add_failover_candidate(&secondary, "backup-model");
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({ "model": "primary-model", "input": "hi" }))
        .send()
        .await
        .expect("send /v1/responses");

    assert!(
        res.status().is_success(),
        "client should get 200 after failover, got {}",
        res.status()
    );
    // primary 被打多次是 adapter 对 5xx 的内部重试(MAX_RETRIES),重试耗尽后才
    // failover——这里只断言"主被尝试过"+"failover 真的切到了次"。
    assert!(
        !primary.received().await.is_empty(),
        "primary must be tried first"
    );
    assert!(
        !secondary.received().await.is_empty(),
        "failover must reach the secondary after primary's 500"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn responses_all_providers_fail_returns_error() {
    // 全部候选都 500 → 候选耗尽 → 客户端拿到错误(非 2xx),不静默假成功。
    let primary = MockUpstream::start().await;
    primary
        .stub_chat_completions_err(500, json!({"error": {"message": "boom1"}}))
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_err(500, json!({"error": {"message": "boom2"}}))
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "primary-model"), &primary).await;
    harness.add_failover_candidate(&secondary, "backup-model");
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({ "model": "primary-model", "input": "hi" }))
        .send()
        .await
        .expect("send /v1/responses");

    assert!(
        !res.status().is_success(),
        "exhausted failover must surface an error, got {}",
        res.status()
    );
    assert!(!primary.received().await.is_empty());
    assert!(
        !secondary.received().await.is_empty(),
        "both candidates must be attempted before giving up"
    );

    harness.shutdown().await;
}

// ── L5: streaming conversion (chat SSE → Responses events) ───────────────
//
// 日常最高频的流式链路此前只有 codex_compact 一条专路有 wire 级 e2e。这里走
// 常规 /v1/responses 流式:上游吐 chat SSE(含 CJK)→ 网关转 Responses 事件 →
// 断言客户端拿到合法事件流且中文内容完整(不被转换破坏)。

#[tokio::test]
async fn responses_stream_converts_chat_sse_to_events() {
    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_sse("stream-model").await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "stream-model"), &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({ "model": "stream-model", "input": "hi", "stream": true }))
        .send()
        .await
        .expect("send streaming /v1/responses");

    assert!(res.status().is_success(), "stream status {}", res.status());
    let body = res.text().await.expect("read stream body");

    // 客户端应收到 Responses SSE 事件,且 CJK 内容完整拼回。
    assert!(
        body.contains("response.") || body.contains("output_text"),
        "expected Responses event frames, got: {}",
        &body[..body.len().min(300)]
    );
    assert!(
        body.contains("你好") && body.contains("世界"),
        "CJK content must survive SSE conversion intact"
    );

    harness.shutdown().await;
}

// ── L6: Gemini route (generateContent → chat conversion) ─────────────────

#[tokio::test]
async fn gemini_generate_converts_to_chat() {
    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("gemini-backed", "hello from chat")
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "gemini-backed"), &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1beta/models/gemini-backed:generateContent"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }]
        }))
        .send()
        .await
        .expect("send gemini generateContent");

    assert!(
        res.status().is_success(),
        "gemini route status {}",
        res.status()
    );
    let received = mock.received().await;
    assert_eq!(
        received.len(),
        1,
        "gemini request must reach the chat upstream"
    );
    assert!(
        received[0].body.get("messages").is_some(),
        "Gemini contents must be converted to chat messages"
    );

    harness.shutdown().await;
}

// ── L7: 上游非 2xx 必须当作 provider 失败(failover + 熔断),最后一跳原样回客户端 ──

#[tokio::test]
async fn chat_pass_through_single_provider_429_keeps_upstream_status_and_body() {
    // 单 provider(无 failover):客户端看到的仍是上游原始状态码 + 原始 body,
    // 但 429 要记进熔断(之前被当成功 mark_success)。
    let mock = MockUpstream::start().await;
    let upstream_body = json!({"error": {"message": "slow down", "type": "rate_limit"}});
    mock.stub_chat_completions_err(429, upstream_body.clone())
        .await;

    let harness = GatewayHarness::start(ProviderSpec::chat_only("custom", "pt-model"), &mock).await;
    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "pt-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat");

    assert_eq!(res.status().as_u16(), 429);
    let body: serde_json::Value = res.json().await.expect("json body");
    assert_eq!(body, upstream_body, "client must see the raw upstream body");
    let (failures, code) = harness.wait_runtime_failures(&harness.provider_id, 1).await;
    assert!(
        failures >= 1 && code.is_some(),
        "upstream 429 must be recorded as a provider failure, got ({failures}, {code:?})"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn chat_pass_through_503_fails_over_and_marks_primary_failure() {
    let primary = MockUpstream::start().await;
    primary
        .stub_chat_completions_err(503, json!({"error": {"message": "unavailable"}}))
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_ok("backup-model", "recovered")
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "primary-model"), &primary).await;
    let secondary_id = harness.add_failover_candidate(&secondary, "backup-model");

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "primary-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat");

    assert!(
        res.status().is_success(),
        "503 on primary must fail over, got {}",
        res.status()
    );
    assert_eq!(primary.received().await.len(), 1);
    assert_eq!(secondary.received().await.len(), 1);
    let (failures, _) = harness.wait_runtime_failures(&harness.provider_id, 1).await;
    assert!(failures >= 1, "primary must be marked failed");
    assert_eq!(harness.runtime_status(&secondary_id).0, 0);

    harness.shutdown().await;
}

#[tokio::test]
async fn chat_pass_through_400_does_not_fail_over() {
    // 400 是请求本身的问题,换 provider 也没用——不能挨个打一遍。
    let primary = MockUpstream::start().await;
    primary
        .stub_chat_completions_err(400, json!({"error": {"message": "bad request"}}))
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_ok("backup-model", "x")
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "primary-model"), &primary).await;
    harness.add_failover_candidate(&secondary, "backup-model");

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "primary-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat");

    assert_eq!(res.status().as_u16(), 400);
    assert!(
        secondary.received().await.is_empty(),
        "400 must not fail over to the secondary"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn messages_failover_on_upstream_500_marks_primary_failure() {
    let primary = MockUpstream::start().await;
    primary
        .stub_chat_completions_err(500, json!({"error": {"message": "boom"}}))
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_ok("backup-model", "recovered")
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "primary-model"), &primary).await;
    let secondary_id = harness.add_failover_candidate(&secondary, "backup-model");

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "primary-model",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("send messages");

    assert!(
        res.status().is_success(),
        "messages route must fail over after primary 500, got {}",
        res.status()
    );
    assert!(!primary.received().await.is_empty());
    assert_eq!(secondary.received().await.len(), 1);
    let (failures, _) = harness.wait_runtime_failures(&harness.provider_id, 1).await;
    assert!(failures >= 1, "primary must be marked failed");
    assert_eq!(harness.runtime_status(&secondary_id).0, 0);

    harness.shutdown().await;
}

#[tokio::test]
async fn gemini_failover_on_upstream_500_marks_primary_failure() {
    let primary = MockUpstream::start().await;
    primary
        .stub_chat_completions_err(500, json!({"error": {"message": "boom"}}))
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_ok("backup-model", "recovered")
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "primary-model"), &primary).await;
    let secondary_id = harness.add_failover_candidate(&secondary, "backup-model");

    let res = harness
        .client()
        .post(harness.url("/v1beta/models/primary-model:generateContent"))
        .bearer_auth(&harness.token)
        .json(&json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}))
        .send()
        .await
        .expect("send gemini");

    assert!(
        res.status().is_success(),
        "gemini route must fail over after primary 500, got {}",
        res.status()
    );
    assert!(!primary.received().await.is_empty());
    assert_eq!(secondary.received().await.len(), 1);
    let (failures, _) = harness.wait_runtime_failures(&harness.provider_id, 1).await;
    assert!(failures >= 1, "primary must be marked failed");
    assert_eq!(harness.runtime_status(&secondary_id).0, 0);

    harness.shutdown().await;
}

#[tokio::test]
async fn messages_stream_already_started_does_not_retry() {
    // 首帧已转发给客户端后上游才出错:绝不能再换 provider 重发(客户端会收到两份输出)。
    let first = format!(
        "data: {}\n\n",
        json!({"id":"c1","object":"chat.completion.chunk","model":"primary-model",
               "choices":[{"index":0,"delta":{"role":"assistant","content":"partial"},"finish_reason":null}]})
    );
    let second = format!(
        "data: {}\n\n",
        json!({"error": {"message": "server overloaded", "type": "server_error"}})
    );
    let (primary_url, primary_hits) = common::mock_upstream::start_split_sse_upstream(
        first,
        second,
        std::time::Duration::from_millis(300),
    )
    .await;
    let placeholder = MockUpstream::start().await;
    let secondary = MockUpstream::start().await;
    secondary.stub_chat_completions_sse("backup-model").await;

    let harness = GatewayHarness::start(
        ProviderSpec::chat_only("custom", "primary-model"),
        &placeholder,
    )
    .await;
    {
        let conn = harness.db.get().unwrap();
        conn.execute(
            "UPDATE providers SET base_url = ?1 WHERE id = ?2",
            rusqlite::params![primary_url, harness.provider_id],
        )
        .unwrap();
    }
    harness.add_failover_candidate(&secondary, "backup-model");

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "primary-model",
            "max_tokens": 16,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("send messages stream");
    assert!(res.status().is_success(), "stream status {}", res.status());
    let body = res.text().await.expect("read stream");
    assert!(
        body.contains("partial"),
        "first frame must reach client: {body}"
    );

    assert_eq!(primary_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        secondary.received().await.is_empty(),
        "stream already started — must not retry on the secondary"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn messages_anthropic_pass_through_error_frame_fails_over() {
    // HTTP 200 + 首帧 `event: error`(overloaded):必须当失败处理并切到备用,
    // 而不是原样透传一个 200 的坏流给 Claude Code。
    let primary = MockUpstream::start().await;
    primary
        .stub_raw(
            "/v1/messages",
            200,
            "text/event-stream",
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        )
        .await;
    let secondary = MockUpstream::start().await;
    secondary.stub_chat_completions_sse("backup-model").await;

    let spec =
        ProviderSpec::chat_only("anthropic", "claude-sonnet-4-6").with_anthropic(primary.url());
    let harness = GatewayHarness::start(spec, &primary).await;
    harness.add_failover_candidate(&secondary, "backup-model");

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 16,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("send messages stream");
    assert!(res.status().is_success(), "status {}", res.status());
    let _ = res.text().await;

    assert_eq!(primary.received().await.len(), 1);
    assert_eq!(
        secondary.received().await.len(),
        1,
        "error frame on primary must fail over to the secondary"
    );
    let (failures, _) = harness.wait_runtime_failures(&harness.provider_id, 1).await;
    assert!(failures >= 1, "primary must be marked failed");

    harness.shutdown().await;
}

// ── L8: 入口鉴权 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn unauthenticated_request_is_rejected_before_body_is_read() {
    // body 先发 1KB 然后永远不结束:鉴权如果排在读 body 之后,请求会一直挂住,
    // 未鉴权客户端就能让网关替它缓冲请求体。
    use futures::StreamExt;
    let mock = MockUpstream::start().await;
    let harness = GatewayHarness::start(ProviderSpec::chat_only("custom", "m"), &mock).await;
    let body = futures::stream::once(async {
        Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b'x'; 1024]))
    })
    .chain(futures::stream::pending());

    let res = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness
            .client()
            .post(harness.url("/v1/responses"))
            .header("content-type", "application/json")
            .body(reqwest::Body::wrap_stream(body))
            .send(),
    )
    .await
    .expect("gateway must answer 401 without waiting for the request body")
    .expect("send");
    assert_eq!(res.status().as_u16(), 401);
    assert!(mock.received().await.is_empty());

    harness.shutdown().await;
}

#[tokio::test]
async fn metrics_requires_token_only_and_health_does_not() {
    // /metrics 只要 token:Docker 里 Prometheus 按服务名抓取(Host `muxlayer:9090`)
    // 不能被 Host 边界拦掉;token 本身已挡住 DNS rebinding 读取。其余路由仍校验边界。
    let mock = MockUpstream::start().await;
    let harness = GatewayHarness::start(ProviderSpec::chat_only("custom", "m"), &mock).await;
    let client = harness.client();

    let anonymous = client.get(harness.url("/metrics")).send().await.unwrap();
    assert_eq!(
        anonymous.status().as_u16(),
        401,
        "/metrics must require the local token"
    );

    let authed = client
        .get(harness.url("/metrics"))
        .bearer_auth(&harness.token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status().as_u16(), 200);

    let service_name_host = client
        .get(harness.url("/metrics"))
        .bearer_auth(&harness.token)
        .header("host", "muxlayer:9090")
        .send()
        .await
        .unwrap();
    assert_eq!(
        service_name_host.status().as_u16(),
        200,
        "/metrics with token must accept a service-name Host"
    );

    let anonymous_service_host = client
        .get(harness.url("/metrics"))
        .header("host", "muxlayer:9090")
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous_service_host.status().as_u16(), 401);

    let rebinding = client
        .get(harness.url("/v1/models"))
        .bearer_auth(&harness.token)
        .header("host", "evil.example.com")
        .send()
        .await
        .unwrap();
    assert!(
        !rebinding.status().is_success(),
        "other routes must still enforce the Host boundary, got {}",
        rebinding.status()
    );

    let health = client.get(harness.url("/health")).send().await.unwrap();
    assert_eq!(
        health.status().as_u16(),
        200,
        "/health stays unauthenticated"
    );

    harness.shutdown().await;
}

// ── L8: 缓存 token 计费口径(request_logs.cost) ─────────────────────────────

/// 插一条自定义价(provider = harness 的 'mock'),$/1M tokens。
fn insert_custom_price(
    harness: &GatewayHarness,
    model: &str,
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
) {
    let conn = harness.db.get().expect("borrow conn");
    conn.execute(
        "INSERT INTO model_pricing (id, provider, model_pattern, input_price, output_price,
             is_custom, updated_at, cache_read_price, cache_write_price)
         VALUES (?1, 'mock', ?2, ?3, ?4, 1, '2026-01-01T00:00:00Z', ?5, ?6)",
        rusqlite::params![
            uuid::Uuid::new_v4().to_string(),
            model,
            input,
            output,
            cache_read,
            cache_write
        ],
    )
    .expect("insert custom price");
}

struct LoggedUsage {
    input_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cost: Option<f64>,
    trace_json: Option<String>,
}

/// 日志是 fire-and-forget 落盘,轮询等该 route 的成功行出现。
async fn wait_success_log(harness: &GatewayHarness, route: &str) -> LoggedUsage {
    for _ in 0..100 {
        let row = {
            let conn = harness.db.get().expect("borrow conn");
            conn.query_row(
                "SELECT input_tokens, cache_read_tokens, cost, trace_json FROM request_logs
                 WHERE route = ?1 AND status_code = 200 ORDER BY rowid DESC LIMIT 1",
                [route],
                |r| {
                    Ok(LoggedUsage {
                        input_tokens: r.get(0)?,
                        cache_read_tokens: r.get(1)?,
                        cost: r.get(2)?,
                        trace_json: r.get(3)?,
                    })
                },
            )
            .ok()
        };
        if let Some(row) = row {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no success log for {route}");
}

fn assert_cost(actual: Option<f64>, expected: f64) {
    let actual = actual.expect("cost must be computed");
    assert!(
        (actual - expected).abs() < 1e-12,
        "cost {actual} != expected {expected}"
    );
}

#[tokio::test]
async fn cost_chat_to_anthropic_bills_cache_tokens_on_top_of_input() {
    // Anthropic 上游:input_tokens 不含缓存,cache_read / cache_creation 单独计费。
    let mock = MockUpstream::start().await;
    let body = json!({
        "id": "msg_cost", "type": "message", "role": "assistant", "model": "cost-claude",
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 1000, "output_tokens": 100,
            "cache_read_input_tokens": 10000, "cache_creation_input_tokens": 2000
        }
    });
    mock.stub_raw("/v1/messages", 200, "application/json", &body.to_string())
        .await;
    let mut spec = ProviderSpec::chat_only("anthropic", "cost-claude");
    spec.protocol = r#"["anthropic_messages"]"#.to_string();
    spec.anthropic_base_url = Some(mock.url());
    let harness = GatewayHarness::start(spec, &mock).await;
    insert_custom_price(&harness, "cost-claude", 3.0, 15.0, None, None);

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "cost-claude", "stream": false,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat");
    assert!(res.status().is_success(), "status {}", res.status());

    let log = wait_success_log(&harness, "/v1/chat/completions").await;
    assert_eq!(log.input_tokens, Some(1000), "raw input_tokens kept");
    // 1000×3 + 100×15 + 10000×(3×0.1) + 2000×(3×1.25)
    assert_cost(log.cost, (3000.0 + 1500.0 + 3000.0 + 7500.0) / 1e6);

    harness.shutdown().await;
}

#[tokio::test]
async fn cost_messages_to_chat_discounts_cached_prompt_with_explicit_cache_price() {
    // OpenAI Chat 上游:prompt_tokens 已含 cached_tokens;有显式缓存价时 cached 部分按缓存价。
    let mock = MockUpstream::start().await;
    let body = json!({
        "id": "chatcmpl_cost", "object": "chat.completion", "model": "cost-gpt",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": {
            "prompt_tokens": 10000, "completion_tokens": 100, "total_tokens": 10100,
            "prompt_tokens_details": {"cached_tokens": 8000}
        }
    });
    mock.stub_raw(
        "/v1/chat/completions",
        200,
        "application/json",
        &body.to_string(),
    )
    .await;
    let harness = GatewayHarness::start(ProviderSpec::chat_only("custom", "cost-gpt"), &mock).await;
    insert_custom_price(&harness, "cost-gpt", 2.0, 8.0, Some(0.5), None);

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "cost-gpt", "max_tokens": 16,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send messages");
    assert!(res.status().is_success(), "status {}", res.status());

    let log = wait_success_log(&harness, "/v1/messages").await;
    assert_eq!(log.input_tokens, Some(10000), "raw prompt_tokens kept");
    // (10000-8000)×2 + 8000×0.5 + 100×8
    assert_cost(log.cost, (4000.0 + 4000.0 + 800.0) / 1e6);

    harness.shutdown().await;
}

#[tokio::test]
async fn cost_chat_pass_through_stream_discounts_cached_prompt_with_explicit_cache_price() {
    // Chat 直通流式:usage 在末尾 chunk,口径同 OpenAI(prompt_tokens 含 cached)。
    let mock = MockUpstream::start().await;
    let sse = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"id":"c1","object":"chat.completion.chunk","model":"cost-gpt",
               "choices":[{"index":0,"delta":{"role":"assistant","content":"ok"},"finish_reason":null}]}),
        json!({"id":"c1","object":"chat.completion.chunk","model":"cost-gpt",
               "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
               "usage":{"prompt_tokens":10000,"completion_tokens":100,"total_tokens":10100,
                        "prompt_tokens_details":{"cached_tokens":8000}}}),
    );
    mock.stub_raw("/v1/chat/completions", 200, "text/event-stream", &sse)
        .await;
    let harness = GatewayHarness::start(ProviderSpec::chat_only("custom", "cost-gpt"), &mock).await;
    insert_custom_price(&harness, "cost-gpt", 2.0, 8.0, Some(0.5), None);

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "cost-gpt", "stream": true,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat stream");
    assert!(res.status().is_success(), "status {}", res.status());
    let _ = res.text().await.expect("drain stream");

    let log = wait_success_log(&harness, "/v1/chat/completions").await;
    assert_eq!(log.input_tokens, Some(10000), "raw prompt_tokens kept");
    assert_eq!(log.cache_read_tokens, Some(8000));
    assert_cost(log.cost, (4000.0 + 4000.0 + 800.0) / 1e6);

    harness.shutdown().await;
}

#[tokio::test]
async fn chat_to_anthropic_non_stream_traces_dropped_redacted_thinking() {
    let mock = MockUpstream::start().await;
    let body = json!({
        "id": "msg_rt", "type": "message", "role": "assistant", "model": "claude-rt",
        "content": [
            {"type": "redacted_thinking", "data": "opaque"},
            {"type": "text", "text": "ok"}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 4, "output_tokens": 2}
    });
    mock.stub_raw("/v1/messages", 200, "application/json", &body.to_string())
        .await;
    let mut spec = ProviderSpec::chat_only("anthropic", "claude-rt");
    spec.protocol = r#"["anthropic_messages"]"#.to_string();
    spec.anthropic_base_url = Some(mock.url());
    let harness = GatewayHarness::start(spec, &mock).await;

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "claude-rt", "stream": false,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat");
    assert!(res.status().is_success(), "status {}", res.status());

    let log = wait_success_log(&harness, "/v1/chat/completions").await;
    let trace: serde_json::Value =
        serde_json::from_str(log.trace_json.as_deref().expect("trace")).expect("trace json");
    let events = trace["degradation_events"]
        .as_array()
        .unwrap_or_else(|| panic!("trace must carry degradation_events: {trace}"));
    assert!(
        events
            .iter()
            .any(|e| e.to_string().contains("redacted_thinking")),
        "redacted_thinking drop must be traced: {trace}"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn cost_anthropic_pass_through_non_stream_logs_tokens_and_cache() {
    // Claude Code 主路径(有 anthropic_base_url 的 provider 直通):以前完全不记 token / cost,
    // 今日花费和预算闸对这部分流量漏算。Anthropic 口径 input 不含缓存 token。
    let mock = MockUpstream::start().await;
    let body = json!({
        "id": "msg_c", "type": "message", "role": "assistant", "model": "cost-claude",
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1000, "output_tokens": 100,
                  "cache_read_input_tokens": 8000, "cache_creation_input_tokens": 2000}
    });
    mock.stub_raw("/v1/messages", 200, "application/json", &body.to_string())
        .await;
    let spec = ProviderSpec::chat_only("anthropic", "cost-claude").with_anthropic(mock.url());
    let harness = GatewayHarness::start(spec, &mock).await;
    insert_custom_price(&harness, "cost-claude", 3.0, 15.0, None, None);

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(
            &json!({"model": "cost-claude", "max_tokens": 16, "stream": false,
                      "messages": [{"role": "user", "content": "hi"}]}),
        )
        .send()
        .await
        .expect("send messages");
    assert!(res.status().is_success(), "status {}", res.status());
    let _ = res.text().await;

    let log = wait_success_log(&harness, "/v1/messages").await;
    assert_eq!(log.input_tokens, Some(1000));
    assert_eq!(log.cache_read_tokens, Some(8000));
    // 1000×3 + 100×15 + 8000×0.3 + 2000×3.75
    assert_cost(log.cost, (3000.0 + 1500.0 + 2400.0 + 7500.0) / 1e6);

    harness.shutdown().await;
}

#[tokio::test]
async fn cost_anthropic_pass_through_stream_logs_usage_from_message_start_of_long_stream() {
    // message_start 带 input / cache token,在流开头;长流时它早已滚出 16KB 的尾部缓冲。
    let mock = MockUpstream::start().await;
    let long_text = "x".repeat(40_000);
    let sse = format!(
        "event: message_start\ndata: {}\n\n\
         event: content_block_start\ndata: {}\n\n\
         event: content_block_delta\ndata: {}\n\n\
         event: content_block_stop\ndata: {}\n\n\
         event: message_delta\ndata: {}\n\n\
         event: message_stop\ndata: {}\n\n",
        json!({"type":"message_start","message":{"id":"msg_s","type":"message","role":"assistant",
               "model":"cost-claude","content":[],
               "usage":{"input_tokens":1000,"output_tokens":1,
                        "cache_read_input_tokens":8000,"cache_creation_input_tokens":2000}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":long_text}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":100}}),
        json!({"type":"message_stop"}),
    );
    mock.stub_raw("/v1/messages", 200, "text/event-stream", &sse)
        .await;
    let spec = ProviderSpec::chat_only("anthropic", "cost-claude").with_anthropic(mock.url());
    let harness = GatewayHarness::start(spec, &mock).await;
    insert_custom_price(&harness, "cost-claude", 3.0, 15.0, None, None);

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(
            &json!({"model": "cost-claude", "max_tokens": 16, "stream": true,
                      "messages": [{"role": "user", "content": "hi"}]}),
        )
        .send()
        .await
        .expect("send messages stream");
    assert!(res.status().is_success(), "status {}", res.status());
    let _ = res.text().await.expect("drain stream");

    let log = wait_success_log(&harness, "/v1/messages").await;
    assert_eq!(log.input_tokens, Some(1000));
    assert_eq!(log.cache_read_tokens, Some(8000));
    assert_cost(log.cost, (3000.0 + 1500.0 + 2400.0 + 7500.0) / 1e6);

    harness.shutdown().await;
}

#[tokio::test]
async fn cost_chat_pass_through_non_stream_logs_tokens() {
    let mock = MockUpstream::start().await;
    let body = json!({
        "id": "c1", "object": "chat.completion", "model": "cost-gpt",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"},
                     "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10000, "completion_tokens": 100, "total_tokens": 10100,
                  "prompt_tokens_details": {"cached_tokens": 8000}}
    });
    mock.stub_raw(
        "/v1/chat/completions",
        200,
        "application/json",
        &body.to_string(),
    )
    .await;
    let harness = GatewayHarness::start(ProviderSpec::chat_only("custom", "cost-gpt"), &mock).await;
    insert_custom_price(&harness, "cost-gpt", 2.0, 8.0, Some(0.5), None);

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "cost-gpt", "stream": false,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat");
    assert!(res.status().is_success(), "status {}", res.status());
    let _ = res.text().await;

    let log = wait_success_log(&harness, "/v1/chat/completions").await;
    assert_eq!(log.input_tokens, Some(10000));
    assert_eq!(log.cache_read_tokens, Some(8000));
    assert_cost(log.cost, (4000.0 + 4000.0 + 800.0) / 1e6);

    harness.shutdown().await;
}

#[tokio::test]
async fn chat_request_without_stream_field_is_accepted() {
    // OpenAI 协议里 stream 是可选字段,很多 SDK / curl 不带;以前走转换路径会 500 CHAT_PARSE_ERROR。
    let mock = MockUpstream::start().await;
    mock.stub_anthropic_messages_ok("claude-ns", "ok").await;
    let mut spec = ProviderSpec::chat_only("anthropic", "claude-ns");
    spec.protocol = r#"["anthropic_messages"]"#.to_string();
    spec.anthropic_base_url = Some(mock.url());
    let harness = GatewayHarness::start(spec, &mock).await;

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "claude-ns",
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send chat");
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    assert!(status.is_success(), "status {status}: {text}");

    harness.shutdown().await;
}

// ── 评审回归:熔断 / 错误形态 / 日志 ─────────────────────────────────────

/// 轮询 request_logs 里某 route + 状态码的最新一行:`(error_message, trace_json)`。
async fn wait_log_with_status(
    harness: &GatewayHarness,
    route: &str,
    status: i64,
) -> (Option<String>, Option<String>) {
    for _ in 0..250 {
        let row = {
            let conn = harness.db.get().expect("borrow conn");
            conn.query_row(
                "SELECT error_message, trace_json FROM request_logs
                 WHERE route = ?1 AND status_code = ?2 ORDER BY rowid DESC LIMIT 1",
                rusqlite::params![route, status],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
        };
        if let Some(row) = row {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    panic!("no {status} log for {route}");
}

#[tokio::test]
async fn failover_request_side_400_does_not_cool_down_primary() {
    // 400(如 prompt is too long)是请求本身的问题:不能把主 provider 打进 cooldown,
    // 否则接下来 10 分钟流量都去备用(换模型、丢 prompt cache)。
    let primary = MockUpstream::start().await;
    primary
        .stub_chat_completions_err(400, json!({"error": {"message": "prompt is too long"}}))
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_ok("backup-model", "x")
        .await;

    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "primary-model"), &primary).await;
    harness.add_failover_candidate(&secondary, "backup-model");
    let send = || {
        harness
            .client()
            .post(harness.url("/v1/chat/completions"))
            .bearer_auth(&harness.token)
            .json(
                &json!({"model": "primary-model", "messages": [{"role": "user", "content": "hi"}]}),
            )
            .send()
    };

    let first = send().await.expect("send first");
    assert_eq!(first.status().as_u16(), 400);
    // 熔断标记是后台写入:给它足够时间落盘,再发第二个请求。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        harness.runtime_status(&harness.provider_id).0,
        0,
        "request-side 400 must not count as a provider failure"
    );

    let second = send().await.expect("send second");
    assert_eq!(second.status().as_u16(), 400);
    assert_eq!(
        primary.received().await.len(),
        2,
        "second request must still go to the primary"
    );
    assert!(secondary.received().await.is_empty());

    harness.shutdown().await;
}

#[tokio::test]
async fn messages_single_provider_overloaded_frame_returns_anthropic_529() {
    // 200 + 首帧 overloaded_error,且没有可切的备用:Claude Code 需要 529 +
    // Anthropic 形态的错误体才会走它自己的 overloaded 重试逻辑。
    let mock = MockUpstream::start().await;
    mock.stub_raw(
        "/v1/messages",
        200,
        "text/event-stream",
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
    )
    .await;
    let spec = ProviderSpec::chat_only("anthropic", "claude-sonnet-4-6").with_anthropic(mock.url());
    let harness = GatewayHarness::start(spec, &mock).await;

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 16,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("send messages stream");
    assert_eq!(res.status().as_u16(), 529);
    let body: serde_json::Value = res.json().await.expect("json body");
    assert_eq!(
        body,
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn messages_single_provider_rate_limit_frame_returns_anthropic_429() {
    let mock = MockUpstream::start().await;
    mock.stub_raw(
        "/v1/messages",
        200,
        "text/event-stream",
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow down\"}}\n\n",
    )
    .await;
    let spec = ProviderSpec::chat_only("anthropic", "claude-sonnet-4-6").with_anthropic(mock.url());
    let harness = GatewayHarness::start(spec, &mock).await;

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 16,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("send messages stream");
    assert_eq!(res.status().as_u16(), 429);
    let body: serde_json::Value = res.json().await.expect("json body");
    assert_eq!(
        body,
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}})
    );

    harness.shutdown().await;
}

/// 客户端读到首块后断开:直通流必须记 499 + CLIENT_DISCONNECTED,而不是 200。
async fn assert_stream_disconnect_logs_499(
    harness: &GatewayHarness,
    route: &str,
    body: serde_json::Value,
) {
    let client = reqwest::Client::new();
    let mut res = client
        .post(harness.url(route))
        .bearer_auth(&harness.token)
        .json(&body)
        .send()
        .await
        .expect("send stream");
    assert!(res.status().is_success(), "status {}", res.status());
    let first = res.chunk().await.expect("read first chunk");
    assert!(first.is_some(), "first chunk must arrive");
    drop(res);
    drop(client);

    let (error_message, trace) = wait_log_with_status(harness, route, 499).await;
    assert!(
        error_message
            .as_deref()
            .is_some_and(|m| m.contains("client disconnected")),
        "error_message: {error_message:?}"
    );
    assert!(
        trace
            .as_deref()
            .is_some_and(|t| t.contains("CLIENT_DISCONNECTED")),
        "trace: {trace:?}"
    );
}

#[tokio::test]
async fn chat_pass_through_stream_client_disconnect_logs_499() {
    let mut frames = vec![format!(
        "data: {}\n\n",
        json!({"id":"c1","object":"chat.completion.chunk","model":"pt-model",
               "choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]})
    )];
    for _ in 0..40 {
        frames.push(format!(
            "data: {}\n\n",
            json!({"id":"c1","object":"chat.completion.chunk","model":"pt-model",
                   "choices":[{"index":0,"delta":{"content":"x"},"finish_reason":null}]})
        ));
    }
    let (upstream_url, _) = common::mock_upstream::start_trickle_sse_upstream(
        frames,
        std::time::Duration::from_millis(100),
    )
    .await;
    let placeholder = MockUpstream::start().await;
    let harness =
        GatewayHarness::start(ProviderSpec::chat_only("custom", "pt-model"), &placeholder).await;
    {
        let conn = harness.db.get().unwrap();
        conn.execute(
            "UPDATE providers SET base_url = ?1 WHERE id = ?2",
            rusqlite::params![upstream_url, harness.provider_id],
        )
        .unwrap();
    }

    assert_stream_disconnect_logs_499(
        &harness,
        "/v1/chat/completions",
        json!({"model": "pt-model", "stream": true,
               "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;

    harness.shutdown().await;
}

#[tokio::test]
async fn messages_anthropic_pass_through_stream_client_disconnect_logs_499() {
    let mut frames = vec![format!(
        "event: message_start\ndata: {}\n\n",
        json!({"type":"message_start","message":{"id":"msg_d","type":"message","role":"assistant",
               "model":"claude-sonnet-4-6","content":[],
               "usage":{"input_tokens":10,"output_tokens":1}}})
    )];
    for _ in 0..40 {
        frames.push(format!(
            "event: content_block_delta\ndata: {}\n\n",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}})
        ));
    }
    let (upstream_url, _) = common::mock_upstream::start_trickle_sse_upstream(
        frames,
        std::time::Duration::from_millis(100),
    )
    .await;
    let placeholder = MockUpstream::start().await;
    let spec =
        ProviderSpec::chat_only("anthropic", "claude-sonnet-4-6").with_anthropic(upstream_url);
    let harness = GatewayHarness::start(spec, &placeholder).await;

    assert_stream_disconnect_logs_499(
        &harness,
        "/v1/messages",
        json!({"model": "claude-sonnet-4-6", "max_tokens": 16, "stream": true,
               "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;

    harness.shutdown().await;
}

/// 等某 route 出现一条 error_message 含 `needle` 的日志。
async fn wait_error_log_containing(harness: &GatewayHarness, route: &str, needle: &str) {
    for _ in 0..100 {
        let found = {
            let conn = harness.db.get().expect("borrow conn");
            conn.query_row(
                "SELECT COUNT(*) FROM request_logs WHERE route = ?1 AND error_message LIKE ?2",
                rusqlite::params![route, format!("%{needle}%")],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
        };
        if found > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no error log containing {needle:?} for {route}");
}

#[tokio::test]
async fn messages_missing_api_key_is_logged() {
    let mock = MockUpstream::start().await;
    let mut spec = ProviderSpec::chat_only("custom", "m");
    spec.api_key = None;
    let harness = GatewayHarness::start(spec, &mock).await;

    let res = harness
        .client()
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({"model": "m", "max_tokens": 16,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .expect("send messages");
    assert!(!res.status().is_success());
    wait_error_log_containing(&harness, "/v1/messages", "no API key").await;
    assert!(mock.received().await.is_empty());

    harness.shutdown().await;
}

#[tokio::test]
async fn gemini_missing_api_key_is_logged() {
    let mock = MockUpstream::start().await;
    let mut spec = ProviderSpec::chat_only("custom", "m");
    spec.api_key = None;
    let harness = GatewayHarness::start(spec, &mock).await;

    let res = harness
        .client()
        .post(harness.url("/v1beta/models/m:generateContent"))
        .bearer_auth(&harness.token)
        .json(&json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}))
        .send()
        .await
        .expect("send gemini");
    assert!(!res.status().is_success());
    wait_error_log_containing(&harness, "/v1beta/generateContent", "no API key").await;
    assert!(mock.received().await.is_empty());

    harness.shutdown().await;
}

#[tokio::test]
async fn chat_to_anthropic_unparseable_upstream_response_fails_over() {
    // 转换分支里不带 "HTTP nnn" 的上游错误(响应不是合法 JSON、Copilot token 交换失败)
    // 仍按 502 参与 failover 判断,和改造前一致。
    let primary = MockUpstream::start().await;
    primary
        .stub_raw(
            "/v1/messages",
            200,
            "application/json",
            "<html>not json</html>",
        )
        .await;
    let secondary = MockUpstream::start().await;
    secondary
        .stub_chat_completions_ok("backup-model", "recovered")
        .await;

    let spec =
        ProviderSpec::chat_only("anthropic", "claude-sonnet-4-6").with_anthropic(primary.url());
    let harness = GatewayHarness::start(spec, &primary).await;
    harness.add_failover_candidate(&secondary, "backup-model");

    let res = harness
        .client()
        .post(harness.url("/v1/chat/completions"))
        .bearer_auth(&harness.token)
        .json(
            &json!({"model": "claude-sonnet-4-6", "stream": false, "max_tokens": 16,
                      "messages": [{"role": "user", "content": "hi"}]}),
        )
        .send()
        .await
        .expect("send chat");
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    assert!(status.is_success(), "must fail over, got {status}: {text}");
    assert_eq!(primary.received().await.len(), 1);
    assert_eq!(secondary.received().await.len(), 1);

    harness.shutdown().await;
}
