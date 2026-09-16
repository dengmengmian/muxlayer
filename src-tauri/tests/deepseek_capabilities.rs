//! L3 capability-layer tests for DeepSeek.
//!
//! DeepSeek's distinctive L3 behaviors: Flash keeps images while V4 Pro is
//! text-only so images get stripped (with a notice), reasoning_content roundtrips end-to-end
//! without being dropped, and the legacy `[1m]` Claude-Code suffix is
//! removed before the request hits the upstream Anthropic endpoint.

mod common;

use common::gateway_harness::{GatewayHarness, ProviderSpec};
use common::mock_upstream::MockUpstream;
use serde_json::json;

#[tokio::test]
async fn deepseek_strips_image_with_notice() {
    // V4 Pro is text-only. The gateway must strip image_url parts from the
    // Chat request and append a recovery notice so the model isn't left
    // wondering what "this image" referred to.
    let spec = ProviderSpec::chat_only("deepseek", "deepseek-v4-pro");

    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("deepseek-v4-pro", "ok").await;

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "deepseek-v4-pro",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "what's this?" },
                    { "type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo=" }
                ]
            }],
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
    let parts = received[0].body["messages"][0]["content"]
        .as_array()
        .expect("content array");
    assert!(
        parts
            .iter()
            .all(|p| p.get("type").and_then(|t| t.as_str()) != Some("image_url")),
        "image_url part should have been stripped"
    );
    let text_blob = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text_blob.contains("image stripped") && text_blob.contains("DeepSeek"),
        "stripped-image notice should mention DeepSeek: {text_blob:?}"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn deepseek_flash_keeps_image() {
    // Flash accepts Chat Completions image_url parts natively.
    let spec = ProviderSpec::chat_only("deepseek", "deepseek-flash");

    let mock = MockUpstream::start().await;
    mock.stub_chat_completions_ok("deepseek-flash", "ok").await;

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "deepseek-flash",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "what's this?" },
                    { "type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo=" }
                ]
            }],
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
    assert_eq!(received[0].body["model"], "deepseek-flash");
    let parts = received[0].body["messages"][0]["content"]
        .as_array()
        .expect("content array");
    assert!(
        parts
            .iter()
            .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url")),
        "image_url part should reach DeepSeek Flash: {parts:?}"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn deepseek_reasoning_content_survives_roundtrip() {
    // DeepSeek-V4 returns `reasoning_content` alongside the answer. The
    // Chat → Responses transform must surface it on the output item so
    // Codex can render the thinking block.
    let spec = ProviderSpec::chat_only("deepseek", "deepseek-v4-pro");

    let mock = MockUpstream::start().await;
    let reasoning = "Step 1: parse. Step 2: answer.";
    mock.stub_chat_completions_with_reasoning("deepseek-v4-pro", reasoning, "42")
        .await;

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/responses"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "deepseek-v4-pro",
            "input": "What's 6 times 7?",
            "stream": false,
            "max_output_tokens": 32,
            "reasoning": { "effort": "high" }
        }))
        .send()
        .await
        .expect("send /v1/responses");
    assert!(
        res.status().is_success(),
        "gateway returned {}",
        res.status()
    );

    let body: serde_json::Value = res.json().await.expect("parse gateway response");
    let output = body["output"]
        .as_array()
        .expect("Responses output should be an array");
    let has_reasoning = output.iter().any(|item| {
        item.get("reasoning_content")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("Step 1"))
            .unwrap_or(false)
    });
    assert!(
        has_reasoning,
        "reasoning_content should pass through to Responses output: {output:?}"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn deepseek_strips_legacy_1m_suffix_on_anthropic_passthrough() {
    // Claude Code's recommended mapping historically wrote `deepseek-v4-pro[1m]`.
    // DeepSeek's Anthropic endpoint 400s on that model id — the gateway
    // strips the `[1m]` suffix before forwarding.
    let mock = MockUpstream::start().await;
    mock.stub_anthropic_messages_ok("deepseek-v4-pro", "ok")
        .await;

    let spec = ProviderSpec::chat_only("deepseek", "deepseek-v4-pro").with_anthropic(mock.url());

    let harness = GatewayHarness::start(spec, &mock).await;
    let client = harness.client();

    let res = client
        .post(harness.url("/v1/messages"))
        .bearer_auth(&harness.token)
        .json(&json!({
            "model": "deepseek-v4-pro[1m]",
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
    assert_eq!(
        received[0].body["model"], "deepseek-v4-pro",
        "[1m] suffix should be stripped before hitting upstream"
    );

    harness.shutdown().await;
}
