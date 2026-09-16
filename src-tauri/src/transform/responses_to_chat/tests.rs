//! responses_to_chat 模块测试(从原单文件整体迁移,内容未改)。

use super::super::providers::{DeepSeekProvider, DefaultProvider, KimiProvider};
use super::convert::merge_consecutive_messages;
use super::effort::apply_effort_overrides;
use super::input::{
    convert_input, convert_input_array, extract_content, flatten_tool_output_with_events, map_role,
    msg,
};
use super::think::trailing_partial;
use super::*;
use crate::protocol::chat_completions::{ChatMessage, ToolCall, ToolCallFunction};
use crate::protocol::openai_responses::ResponsesRequest;
use serde_json::json;
use serde_json::Value;

#[test]
fn test_convert_simple_string_input() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!("hello"),
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
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert_eq!(result.model, "gpt-4");
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].role, "user");
    assert_eq!(result.messages[0].content, Some(json!("hello")));
}

#[test]
fn test_embedded_tool_call_invalid_arguments_salvaged() {
    // 内嵌 tool_call 不经过入站 function_call 的 salvage，靠第 7 步兜底：
    // 半截 JSON → "{}"，合法 / 空参数原样保留。
    let req = ResponsesRequest {
        input: json!([
            {"type": "message", "role": "user", "content": "go"},
            {"type": "message", "role": "assistant", "content": [
                {"type": "tool_call", "id": "c1", "name": "a", "arguments": "{\"x\":"},
                {"type": "tool_call", "id": "c2", "name": "b", "arguments": "{\"y\":1}"},
                {"type": "tool_call", "id": "c3", "name": "c", "arguments": ""}
            ]}
        ]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    let args: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flatten()
        .map(|tc| tc.function.arguments.as_str())
        .collect();
    assert_eq!(args, vec!["{}", "{\"y\":1}", ""]);
}

#[test]
fn test_convert_with_instructions() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!("hello"),
        instructions: Some("Be helpful".to_string()),
        system: None,
        previous_response_id: None,
        tools: None,
        tool_choice: None,
        stream: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].role, "system");
    assert_eq!(result.messages[0].content, Some(json!("Be helpful")));
    assert_eq!(result.messages[1].role, "user");
}

// 复现:新会话首条「文字 + 图片」多模态 user turn,跟在一条纯文本 user 消息之后。
// merge_consecutive_messages 合并两条连续 user 时,多模态那条 content 是数组、
// as_str() 为 None,被当成空内容直接 continue 丢弃 —— 用户问题和图片一起消失。
#[test]
fn test_multimodal_user_message_not_dropped_when_merged_after_text() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "AGENTS 指令"}]
            },
            {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "这是什么内容？"},
                    {"type": "input_image", "image_url": "data:image/png;base64,AAAA"}
                ]
            }
        ]),
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
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();

    let mut all_text = String::new();
    let mut has_image = false;
    for m in &result.messages {
        match m.content.as_ref() {
            Some(Value::String(s)) => all_text.push_str(s),
            Some(Value::Array(parts)) => {
                for p in parts {
                    match p.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            all_text.push_str(p.get("text").and_then(|t| t.as_str()).unwrap_or(""))
                        }
                        Some("image_url") => has_image = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    assert!(
        all_text.contains("这是什么内容？"),
        "用户问题文字丢失: messages={:?}",
        result.messages
    );
    assert!(has_image, "用户图片丢失: messages={:?}", result.messages);
}

#[test]
fn test_convert_instructions_priority_over_system() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!("hello"),
        instructions: Some("Instr".to_string()),
        system: Some("Sys".to_string()),
        previous_response_id: None,
        tools: None,
        tool_choice: None,
        stream: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert_eq!(result.messages[0].content, Some(json!("Instr")));
}

#[test]
fn test_convert_input_array_messages() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "message", "role": "assistant", "content": "hello"}
        ]),
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
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].role, "user");
    assert_eq!(result.messages[1].role, "assistant");
}

#[test]
fn test_convert_function_call_and_output() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {"type": "function_call", "call_id": "call_1", "name": "search", "arguments": "{\"q\":\"hi\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "result"}
        ]),
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
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].role, "assistant");
    assert!(result.messages[0].tool_calls.is_some());
    assert_eq!(result.messages[1].role, "tool");
    assert_eq!(result.messages[1].tool_call_id, Some("call_1".to_string()));
}

#[test]
fn test_convert_custom_tool_call_and_output() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {
                "type": "custom_tool_call",
                "call_id": "call_exec",
                "name": "exec",
                "input": "await tools.exec_command({cmd:\"pwd\"})"
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_exec",
                "output": "cwd=/tmp"
            }
        ]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].role, "assistant");
    let tcs = result.messages[0].tool_calls.as_ref().unwrap();
    assert_eq!(tcs[0].function.name, "exec");
    assert_eq!(
        tcs[0].function.arguments,
        r#"{"input":"await tools.exec_command({cmd:\"pwd\"})"}"#
    );
    assert_eq!(result.messages[1].role, "tool");
    assert_eq!(
        result.messages[1].tool_call_id,
        Some("call_exec".to_string())
    );
}

#[test]
fn test_assistant_text_and_function_call_merge_into_one() {
    // Codex 把"assistant 说一句"和 function_call 作为两个独立 item 下发;应合并成
    // 一条 assistant 消息(content + tool_calls),而不是拆成两条连续 assistant。
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "好的,我先查一下"}]},
            {"type": "function_call", "call_id": "call_1", "name": "search", "arguments": "{\"q\":\"hi\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "result"}
        ]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    // 期望 [assistant(content+tool_calls), tool] —— 2 条,不是拆成的 3 条
    assert_eq!(
        result.messages.len(),
        2,
        "assistant 文本 + function_call 应合并成一条"
    );
    assert_eq!(result.messages[0].role, "assistant");
    assert!(
        result.messages[0].content.is_some(),
        "合并后保留文本 content"
    );
    assert!(
        result.messages[0].tool_calls.is_some(),
        "合并后带 tool_calls"
    );
    assert_eq!(result.messages[1].role, "tool");
}

#[test]
fn test_function_call_without_preceding_text_stays_standalone() {
    // 没有前置 assistant 文本时,function_call 仍单独成一条 assistant(不误并)。
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "查一下"}]},
            {"type": "function_call", "call_id": "call_1", "name": "search", "arguments": "{}"}
        ]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    // user 不被污染;function_call 自成一条 assistant(不误并进 user)。
    // 注:孤儿 tool_call 会被合成一条空 tool 输出,故总数 > 2,这里只校验前两条。
    assert_eq!(result.messages[0].role, "user");
    assert!(
        result.messages[0].tool_calls.is_none(),
        "user 不应被挂 tool_calls"
    );
    assert_eq!(result.messages[1].role, "assistant");
    assert!(result.messages[1].tool_calls.is_some());
}

#[test]
fn test_convert_missing_call_id_errors() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {"type": "function_call_output", "call_id": "", "output": "result"}
        ]),
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
    };
    assert!(convert_with_provider(&req, "gpt-4", &DefaultProvider).is_err());
}

#[test]
fn test_convert_reuse_stream_options() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!("hi"),
        instructions: None,
        system: None,
        previous_response_id: None,
        tools: None,
        tool_choice: None,
        stream: Some(true),
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert!(result.stream);
    assert!(result.stream_options.is_some());
    assert_eq!(result.stream_options.unwrap()["include_usage"], true);
}

#[test]
fn test_convert_preserves_temperature_top_p_max_tokens() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!("hi"),
        instructions: None,
        system: None,
        previous_response_id: None,
        tools: None,
        tool_choice: None,
        stream: None,
        temperature: Some(0.7),
        top_p: Some(0.9),
        max_output_tokens: Some(1024),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert_eq!(result.temperature, Some(0.7));
    assert_eq!(result.top_p, Some(0.9));
    assert_eq!(result.max_tokens, Some(1024));
}

#[test]
fn test_convert_deepseek_strips_image_url_with_notice() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": "look"},
                {"type": "input_image", "image_url": "http://example.com/img.png"}
            ]
        }]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "deepseek-v4-pro", &DeepSeekProvider).unwrap();
    let parts = result.messages[0]
        .content
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["type"], "text");
    let text = parts[0]["text"].as_str().unwrap();
    assert!(text.contains("look"));
    assert!(text.contains("image stripped"));
}

#[test]
fn test_convert_deepseek_image_only_becomes_notice_text() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_image", "image_url": "http://example.com/img.png"}
            ]
        }]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "deepseek-v4-pro", &DeepSeekProvider).unwrap();
    let parts = result.messages[0]
        .content
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["type"], "text");
    assert!(parts[0]["text"]
        .as_str()
        .unwrap()
        .contains("vision-capable"));
}

#[test]
fn test_merge_consecutive_user_messages() {
    let messages = vec![msg("user", json!("hello")), msg("user", json!("world"))];
    let merged = merge_consecutive_messages(messages);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].content, Some(json!("hello\n\nworld")));
}

#[test]
fn test_merge_consecutive_system_messages() {
    let messages = vec![msg("system", json!("sys1")), msg("system", json!("sys2"))];
    let merged = merge_consecutive_messages(messages);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].content, Some(json!("sys1\n\nsys2")));
}

#[test]
fn test_do_not_merge_assistant_messages() {
    let messages = vec![msg("assistant", json!("a1")), msg("assistant", json!("a2"))];
    let merged = merge_consecutive_messages(messages);
    assert_eq!(merged.len(), 2);
}

#[test]
fn test_do_not_merge_messages_with_tool_calls() {
    let messages = vec![
        ChatMessage {
            role: "assistant".to_string(),
            content: Some(json!("call")),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCall {
                id: "tc1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                    name: "f".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
        },
        msg("assistant", json!("a2")),
    ];
    let merged = merge_consecutive_messages(messages);
    assert_eq!(merged.len(), 2);
}

#[test]
fn test_sanitize_invalid_tool_arguments() {
    let mut messages = vec![ChatMessage {
        role: "assistant".to_string(),
        content: None,
        reasoning_content: None,
        tool_calls: Some(vec![ToolCall {
            id: "tc1".to_string(),
            call_type: "function".to_string(),
            function: ToolCallFunction {
                name: "f".to_string(),
                arguments: "not json".to_string(),
            },
        }]),
        tool_call_id: None,
        name: None,
    }];
    // 与 convert 第 7 步共用同一个 salvage 实现
    for msg in &mut messages {
        if let Some(ref mut tcs) = msg.tool_calls {
            for tc in tcs {
                tc.function.arguments = crate::transform::tool_calls::salvage_tool_arguments(
                    &tc.function.arguments,
                    &tc.function.name,
                    &tc.id,
                    None,
                );
            }
        }
    }
    assert_eq!(
        messages[0].tool_calls.as_ref().unwrap()[0]
            .function
            .arguments,
        "{}"
    );
}

#[test]
fn test_kimi_web_search_disables_thinking() {
    let req = ResponsesRequest {
        model: Some("kimi-k2".to_string()),
        input: json!("search"),
        instructions: None,
        system: None,
        previous_response_id: None,
        tools: Some(vec![json!({"type": "web_search"})]),
        tool_choice: None,
        stream: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        ..Default::default()
    };
    let result = convert_with_provider(&req, "kimi-k2", &KimiProvider).unwrap();
    assert!(result.thinking.is_some());
    assert_eq!(result.thinking.unwrap()["type"], "disabled");
}

#[test]
fn test_deepseek_maps_xhigh_to_max_and_enables_thinking() {
    let req = ResponsesRequest {
        model: Some("deepseek-v4-pro".to_string()),
        input: json!("think hard"),
        instructions: None,
        system: None,
        previous_response_id: None,
        tools: None,
        tool_choice: None,
        stream: None,
        temperature: Some(0.7),
        top_p: Some(0.9),
        max_output_tokens: None,
        reasoning: Some(json!({"effort": "xhigh"})),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "deepseek-v4-pro", &DeepSeekProvider).unwrap();
    assert_eq!(result.thinking, Some(json!({"type": "enabled"})));
    assert_eq!(result.reasoning_effort.as_deref(), Some("max"));
    assert!(result.temperature.is_none());
    assert!(result.top_p.is_none());
}

#[test]
fn test_mcp_tools_inject_advisory_without_chat_tool() {
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!("use github"),
        instructions: Some("Be concise".to_string()),
        system: None,
        previous_response_id: None,
        tools: Some(vec![json!({
            "type": "mcp",
            "server_label": "GitHub",
            "connector_id": "github"
        })]),
        tool_choice: None,
        stream: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    assert!(
        result.tools.is_none(),
        "MCP tools must not be sent as Chat tools"
    );
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].role, "system");
    let sys = result.messages[0]
        .content
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(sys.contains("Be concise"));
    assert!(sys.contains("GitHub"));
    assert!(sys.contains("not callable"));
    assert_eq!(result.messages[1].role, "user");
    assert_eq!(result.diagnostic_events.len(), 1);
    assert_eq!(result.diagnostic_events[0].capability, "mcp");
}

// ── apply_effort_overrides（env-driven） ──
// env 是进程全局,用 #[serial(env)] 与其他动 env 的测试互斥。
#[test]
#[serial_test::serial(env)]
fn apply_effort_overrides_covers_all_scenarios() {
    // 1. floor 覆盖 low → high
    std::env::set_var("AGENTGATE_EFFORT_FLOOR_PROVIDERS", "test_provider");
    assert_eq!(
        apply_effort_overrides("test_provider", Some("low".to_string())),
        Some("high".to_string())
    );

    // 2. floor 覆盖 medium → high
    assert_eq!(
        apply_effort_overrides("test_provider", Some("medium".to_string())),
        Some("high".to_string())
    );

    // 3. floor 不动 high
    assert_eq!(
        apply_effort_overrides("test_provider", Some("high".to_string())),
        Some("high".to_string())
    );

    // 4. floor 不把 DeepSeek/OpenAI-style max 降级成 high
    assert_eq!(
        apply_effort_overrides("test_provider", Some("max".to_string())),
        Some("max".to_string())
    );

    // 5. floor 覆盖 None → high
    assert_eq!(
        apply_effort_overrides("test_provider", None),
        Some("high".to_string())
    );

    // 6. provider 大小写不敏感
    std::env::set_var("AGENTGATE_EFFORT_FLOOR_PROVIDERS", "MiMo,DeepSeek");
    assert_eq!(
        apply_effort_overrides("mimo", Some("low".to_string())),
        Some("high".to_string())
    );

    // 7. provider 不在 floor 列表 → 原值透传
    assert_eq!(
        apply_effort_overrides("not_in_list", Some("low".to_string())),
        Some("low".to_string())
    );

    std::env::remove_var("AGENTGATE_EFFORT_FLOOR_PROVIDERS");

    // 8. fill：客户端 None → 补 high
    std::env::set_var("AGENTGATE_FORCE_HIGH_EFFORT_PROVIDERS", "test_fill");
    assert_eq!(
        apply_effort_overrides("test_fill", None),
        Some("high".to_string())
    );

    // 9. fill：客户端传 low → 不覆盖
    assert_eq!(
        apply_effort_overrides("test_fill", Some("low".to_string())),
        Some("low".to_string()),
        "fill 仅在 None 时生效，不覆盖客户端 low"
    );
    std::env::remove_var("AGENTGATE_FORCE_HIGH_EFFORT_PROVIDERS");

    // 10. 两 env 都不设：透传
    assert_eq!(
        apply_effort_overrides("anything", Some("low".to_string())),
        Some("low".to_string())
    );
    assert_eq!(apply_effort_overrides("anything", None), None);
}

#[test]
fn test_split_think_tags_basic() {
    let (text, reasoning) = split_think_tags("Hello <think>thinking</think> world");
    assert_eq!(text, "Hello  world");
    assert_eq!(reasoning, Some("thinking".to_string()));
}

#[test]
fn test_split_think_tags_no_tags() {
    let (text, reasoning) = split_think_tags("Just text");
    assert_eq!(text, "Just text");
    assert_eq!(reasoning, None);
}

#[test]
fn test_split_think_tags_empty_thinking() {
    let (text, reasoning) = split_think_tags("Hello <think>   </think> world");
    assert_eq!(text, "Hello  world");
    assert_eq!(reasoning, None);
}

#[test]
fn test_map_role_developer_to_system() {
    assert_eq!(map_role("developer"), "system");
    assert_eq!(map_role("user"), "user");
    assert_eq!(map_role("assistant"), "assistant");
}

#[test]
fn test_extract_content_string() {
    assert_eq!(extract_content(Some(&json!("hello"))), json!("hello"));
}

#[test]
fn test_extract_content_array_text_parts() {
    let arr = json!([
        {"type": "input_text", "text": "hello"},
        {"type": "output_text", "text": " world"},
        {"type": "text", "text": "!"}
    ]);
    assert_eq!(extract_content(Some(&arr)), json!("hello world!"));
}

#[test]
fn test_extract_content_array_no_text() {
    let arr = json!([{"type": "image", "url": "http://example.com"}]);
    assert_eq!(
        extract_content(Some(&arr)),
        json!("[{\"type\":\"image\",\"url\":\"http://example.com\"}]")
    );
}

#[test]
fn test_extract_content_object_with_text() {
    let obj = json!({"text": "hello", "format": "plain"});
    assert_eq!(extract_content(Some(&obj)), json!("hello"));
}

#[test]
fn test_extract_content_object_no_text() {
    let obj = json!({"format": "plain"});
    assert_eq!(extract_content(Some(&obj)), json!("{\"format\":\"plain\"}"));
}

#[test]
fn test_convert_input_object() {
    let input = json!({"text": "hello object"});
    let mut events = Vec::new();
    let result = convert_input(&input, "test-model", &mut events).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].content, Some(json!("hello object")));
    assert!(events.is_empty());
}

#[test]
fn test_convert_input_number() {
    let input = json!(42);
    let mut events = Vec::new();
    let result = convert_input(&input, "test-model", &mut events).unwrap();
    assert_eq!(result[0].content, Some(json!("42")));
    assert!(events.is_empty());
}

// ── Tests for fixes ──

#[test]
fn test_split_think_tags_multiple_blocks() {
    let (text, reasoning) = split_think_tags("<think>A</think> middle <think>B</think> end");
    assert_eq!(text, "middle  end");
    assert_eq!(reasoning, Some("A\n\nB".to_string()));
}

#[test]
fn test_split_think_tags_unclosed() {
    let (text, reasoning) = split_think_tags("hello <think>unclosed");
    assert_eq!(text, "hello <think>unclosed");
    assert_eq!(reasoning, None);
}

#[test]
fn test_split_think_tags_adjacent() {
    let (text, reasoning) = split_think_tags("<think>first</think><think>second</think>");
    assert_eq!(reasoning, Some("first\n\nsecond".to_string()));
    assert_eq!(text, "");
}

#[test]
fn test_large_tool_output_not_truncated() {
    let big_output = "x".repeat(10000);
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {"type": "function_call", "call_id": "c1", "name": "read", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": big_output}
        ]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    let tool_msg = &result.messages[1];
    let content_str = tool_msg.content.as_ref().unwrap().as_str().unwrap();
    assert_eq!(
        content_str.len(),
        10000,
        "Tool output should not be truncated"
    );
}

#[test]
fn test_chinese_tool_output_not_truncated() {
    let chinese_output = "中文".repeat(3000); // 6000 chars, ~18000 bytes
    let req = ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([
            {"type": "function_call", "call_id": "c1", "name": "read", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": chinese_output}
        ]),
        ..Default::default()
    };
    let result = convert_with_provider(&req, "gpt-4", &DefaultProvider).unwrap();
    let tool_msg = &result.messages[1];
    let content_str = tool_msg.content.as_ref().unwrap().as_str().unwrap();
    assert_eq!(
        content_str, chinese_output,
        "Chinese tool output should pass through intact"
    );
}

// ── split_think_tags whitespace preservation (critical for markdown rendering) ──

#[test]
fn test_split_think_tags_preserves_whitespace_no_tags() {
    // SSE delta chunks with leading/trailing newlines must be preserved
    // for markdown tables and headers to render correctly
    let (text, reasoning) = split_think_tags("\n\n## Header\n\n");
    assert_eq!(text, "\n\n## Header\n\n");
    assert_eq!(reasoning, None);
}

#[test]
fn test_split_think_tags_preserves_table_newlines() {
    let chunk = "\n| col1 | col2 |\n| --- | --- |\n| a | b |\n";
    let (text, reasoning) = split_think_tags(chunk);
    assert_eq!(
        text, chunk,
        "Table newlines must be preserved for markdown rendering"
    );
    assert_eq!(reasoning, None);
}

#[test]
fn test_split_think_tags_preserves_leading_newline() {
    let (text, reasoning) = split_think_tags("\nhello");
    assert_eq!(text, "\nhello");
    assert_eq!(reasoning, None);
}

#[test]
fn test_split_think_tags_preserves_trailing_newline() {
    let (text, reasoning) = split_think_tags("hello\n\n");
    assert_eq!(text, "hello\n\n");
    assert_eq!(reasoning, None);
}

#[test]
fn test_split_think_tags_preserves_spaces_in_delta() {
    // A delta chunk that is just whitespace (common in streaming)
    let (text, reasoning) = split_think_tags("  ");
    assert_eq!(text, "  ");
    assert_eq!(reasoning, None);
}

#[test]
fn test_split_think_tags_with_tags_does_trim() {
    // When think tags are extracted, trimming the remaining text is OK
    let (text, reasoning) = split_think_tags("  <think>thinking</think>  hello  ");
    assert_eq!(text, "hello");
    assert_eq!(reasoning, Some("thinking".to_string()));
}

// ── flatten_tool_output tests ──

#[test]
fn test_flatten_tool_output_string() {
    assert_eq!(flatten_tool_output(&json!("hello")), "hello");
}

#[test]
fn test_flatten_tool_output_array_text_parts() {
    let output = json!([
        {"type": "output_text", "text": "result line 1"},
        {"type": "output_text", "text": "result line 2"}
    ]);
    assert_eq!(flatten_tool_output(&output), "result line 1result line 2");
}

#[test]
fn test_flatten_tool_output_array_with_images() {
    let output = json!([
        {"type": "output_text", "text": "some text"},
        {"type": "input_image", "image_url": {"url": "data:image/png;base64,abc"}}
    ]);
    let mut events = Vec::new();
    let result = flatten_tool_output_with_events(&output, &mut events);
    assert!(result.contains("some text"));
    assert!(result.contains("[1 image attachment omitted from tool output]"));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].capability, "vision");
    assert_eq!(events[0].source, "tool_output_transform");
}

#[test]
fn test_flatten_tool_output_array_multiple_images() {
    let output = json!([
        {"type": "input_image", "image_url": {"url": "img1"}},
        {"type": "input_image", "image_url": {"url": "img2"}},
        {"type": "input_image", "image_url": {"url": "img3"}}
    ]);
    let result = flatten_tool_output(&output);
    assert!(result.contains("[3 image attachments omitted from tool output]"));
}

#[test]
fn test_flatten_tool_output_non_string_non_array() {
    // Numbers, objects, etc. → JSON stringify
    assert_eq!(flatten_tool_output(&json!(42)), "42");
    assert_eq!(
        flatten_tool_output(&json!({"key": "val"})),
        "{\"key\":\"val\"}"
    );
}

// ── extract_content image preservation tests ──

#[test]
fn test_extract_content_text_only() {
    let content = json!([
        {"type": "input_text", "text": "hello"},
        {"type": "text", "text": " world"}
    ]);
    let result = extract_content(Some(&content));
    // Text-only → joined string
    assert_eq!(result, Value::String("hello world".to_string()));
}

#[test]
fn test_extract_content_with_image_preserves_array() {
    let content = json!([
        {"type": "input_text", "text": "describe this"},
        {"type": "input_image", "image_url": "data:image/png;base64,abc123"}
    ]);
    let result = extract_content(Some(&content));
    // Has image → returns array
    assert!(result.is_array());
    let arr = result.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "describe this");
    assert_eq!(arr[1]["type"], "image_url");
    assert_eq!(arr[1]["image_url"]["url"], "data:image/png;base64,abc123");
}

#[test]
fn test_convert_initial_top_level_content_parts_preserves_image() {
    let mut events = Vec::new();
    let items = json!([
        {"type": "input_text", "text": "describe this"},
        {"type": "input_image", "image_url": {"url": "data:image/png;base64,abc123"}}
    ]);
    let msgs = convert_input_array(items.as_array().unwrap(), "test-model", &mut events).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "user");
    let content = msgs[0].content.as_ref().unwrap().as_array().unwrap();
    assert_eq!(content[0], json!({"type": "text", "text": "describe this"}));
    assert_eq!(content[1]["type"], "image_url");
    assert_eq!(
        content[1]["image_url"]["url"],
        "data:image/png;base64,abc123"
    );
}

#[test]
fn test_extract_content_image_url_passthrough() {
    let content = json!([
        {"type": "text", "text": "hi"},
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,xyz"}}
    ]);
    let result = extract_content(Some(&content));
    assert!(result.is_array());
    let arr = result.as_array().unwrap();
    assert_eq!(arr[1]["type"], "image_url");
}

#[test]
fn test_extract_content_input_image_nested_url() {
    let content = json!([
        {"type": "input_image", "image_url": {"url": "data:image/jpeg;base64,abc"}}
    ]);
    let result = extract_content(Some(&content));
    assert!(result.is_array());
    let arr = result.as_array().unwrap();
    assert_eq!(arr[0]["image_url"]["url"], "data:image/jpeg;base64,abc");
}

#[test]
fn test_extract_content_input_image_detail_top_level_preserved() {
    // Responses 协议规范：detail 在 input_image 顶层
    let content = json!([
        {"type": "input_image", "image_url": "https://x/y.png", "detail": "high"}
    ]);
    let result = extract_content(Some(&content));
    let arr = result.as_array().unwrap();
    assert_eq!(arr[0]["type"], "image_url");
    assert_eq!(arr[0]["image_url"]["url"], "https://x/y.png");
    assert_eq!(arr[0]["image_url"]["detail"], "high");
}

#[test]
fn test_extract_content_input_image_detail_nested_preserved() {
    // 部分 client（Codex 等）把 detail 嵌进 image_url 对象里——也要保留
    let content = json!([
        {"type": "input_image", "image_url": {"url": "https://x/y.png", "detail": "low"}}
    ]);
    let result = extract_content(Some(&content));
    let arr = result.as_array().unwrap();
    assert_eq!(arr[0]["image_url"]["detail"], "low");
}

#[test]
fn test_extract_content_input_image_no_detail_no_field() {
    // 不指定 detail 时不要往 image_url 对象里塞 detail: null
    let content = json!([
        {"type": "input_image", "image_url": "https://x/y.png"}
    ]);
    let result = extract_content(Some(&content));
    let arr = result.as_array().unwrap();
    assert!(arr[0]["image_url"].get("detail").is_none());
}

#[test]
fn test_extract_content_string_unchanged() {
    let result = extract_content(Some(&json!("plain text")));
    assert_eq!(result, Value::String("plain text".to_string()));
}

#[test]
fn test_extract_content_none() {
    let result = extract_content(None);
    assert_eq!(result, Value::String(String::new()));
}

#[test]
fn reasoning_encrypted_content_round_trips_to_assistant_message() {
    // Codex echoes a `reasoning` item with `encrypted_content` after a
    // prior turn; convert_input must pull that text and attach it to the
    // next assistant message as reasoning_content.
    let items = vec![
        json!({"type": "message", "role": "user", "content": "what's 2+2"}),
        json!({"type": "reasoning", "encrypted_content": "Let me think... 4."}),
        json!({"type": "message", "role": "assistant", "content": "4"}),
    ];
    let mut events = Vec::new();
    let msgs = convert_input_array(&items, "test-model", &mut events).unwrap();
    // user, assistant(reasoning=...)
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(
        msgs[1].reasoning_content.as_deref(),
        Some("Let me think... 4.")
    );
    assert!(events.is_empty());
}

#[test]
fn reasoning_encrypted_content_attaches_to_tool_call_turn() {
    // The critical case: tool_calls turn missing reasoning_content would
    // 400 on MiMo / DeepSeek thinking mode. encrypted_content from input
    // must land on the function_call turn.
    let items = vec![
        json!({"type": "message", "role": "user", "content": "search for X"}),
        json!({"type": "reasoning", "encrypted_content": "I should search."}),
        json!({
            "type": "function_call",
            "call_id": "c1",
            "name": "search",
            "arguments": "{\"q\":\"X\"}",
        }),
        json!({"type": "function_call_output", "call_id": "c1", "output": "found"}),
    ];
    let mut events = Vec::new();
    let msgs = convert_input_array(&items, "test-model", &mut events).unwrap();
    // user + assistant(tool_calls, reasoning) + tool
    let assistant = msgs
        .iter()
        .find(|m| m.role == "assistant")
        .expect("assistant present");
    assert_eq!(
        assistant.reasoning_content.as_deref(),
        Some("I should search.")
    );
    assert!(assistant.tool_calls.is_some());
    assert!(events.is_empty());
}

#[test]
fn reasoning_encrypted_content_takes_priority_over_summary() {
    let items = vec![
        json!({
            "type": "reasoning",
            "encrypted_content": "full trace",
            "summary": [{"type": "summary_text", "text": "short summary"}],
        }),
        json!({"type": "message", "role": "assistant", "content": "ok"}),
    ];
    let mut events = Vec::new();
    let msgs = convert_input_array(&items, "test-model", &mut events).unwrap();
    assert_eq!(msgs[0].reasoning_content.as_deref(), Some("full trace"));
    assert!(events.is_empty());
}

// ── ThinkSplitter（带状态，跨 chunk 边界） ────────────────────

#[test]
fn think_splitter_single_chunk_full_tags() {
    let mut sp = ThinkSplitter::new();
    let (vis, rs) = sp.process_chunk("hello <think>thinking</think> world");
    assert_eq!(vis, "hello  world");
    assert_eq!(rs.as_deref(), Some("thinking"));
    let (vis2, rs2) = sp.flush();
    assert!(vis2.is_empty() && rs2.is_none());
}

#[test]
fn think_splitter_split_open_tag_across_chunks() {
    // chunk1 末尾是半截 `<thi`，chunk2 接上 `nk>...</think>`
    let mut sp = ThinkSplitter::new();
    let (v1, r1) = sp.process_chunk("hello <thi");
    assert_eq!(v1, "hello ");
    assert!(r1.is_none());
    let (v2, r2) = sp.process_chunk("nk>secret</think> world");
    assert_eq!(v2, " world");
    assert_eq!(r2.as_deref(), Some("secret"));
}

#[test]
fn think_splitter_split_close_tag_across_chunks() {
    let mut sp = ThinkSplitter::new();
    let (_, _) = sp.process_chunk("a<think>think");
    let (v2, r2) = sp.process_chunk("ing</th");
    assert_eq!(v2, "");
    assert_eq!(r2.as_deref(), Some("ing"));
    let (v3, r3) = sp.process_chunk("ink>tail");
    assert_eq!(v3, "tail");
    assert!(r3.is_none());
}

#[test]
fn think_splitter_no_think_tag_passes_through() {
    let mut sp = ThinkSplitter::new();
    let (v1, r1) = sp.process_chunk("just plain text");
    assert_eq!(v1, "just plain text");
    assert!(r1.is_none());
}

#[test]
fn think_splitter_flush_with_unclosed_think() {
    // 上游 chunk 里 `<think>` 开了头但 chunk 末尾正好是个半截 `</thi`——carry 留着。
    // stream 结束时 flush，in_think 状态下 carry 当 reasoning emit 出去。
    let mut sp = ThinkSplitter::new();
    let (v1, r1) = sp.process_chunk("text<think>reasoning</thi");
    assert_eq!(v1, "text");
    // chunk 内已确定的 reasoning 部分先返回（"reasoning"），半截 `</thi` 进 carry
    assert_eq!(r1.as_deref(), Some("reasoning"));
    let (v, r) = sp.flush();
    // flush 时仍 in_think，carry `</thi` 当 reasoning 字面文本输出
    assert!(v.is_empty());
    assert_eq!(r.as_deref(), Some("</thi"));
}

#[test]
fn think_splitter_flush_with_unclosed_partial_open() {
    // carry 是 `<thi` 这种半截开始标签，stream 结束时按字面文本输出。
    let mut sp = ThinkSplitter::new();
    let (v1, _) = sp.process_chunk("hello <thi");
    assert_eq!(v1, "hello ");
    let (v2, r2) = sp.flush();
    assert_eq!(v2, "<thi");
    assert!(r2.is_none());
}

#[test]
fn think_splitter_multiple_think_blocks() {
    let mut sp = ThinkSplitter::new();
    let (v1, r1) = sp.process_chunk("a<think>X</think>b<think>Y</think>c");
    assert_eq!(v1, "abc");
    // 两段 reasoning 分别返回（拼接到一起，因为同一 chunk 内）
    assert_eq!(r1.as_deref(), Some("XY"));
}

#[test]
fn think_splitter_tiny_chunks_byte_by_byte() {
    // 极端 case：上游逐字节流出 "<think>"，确保 carry 累积正确
    let mut sp = ThinkSplitter::new();
    for ch in "<think>r</think>".chars() {
        let _ = sp.process_chunk(&ch.to_string());
    }
    let (v, r) = sp.flush();
    assert_eq!(v, "");
    assert!(r.is_none());
}

#[test]
fn trailing_partial_finds_longest_prefix() {
    // 末尾 `<thi` 是 `<think>` 的前 4 字节
    assert_eq!(trailing_partial("hello <thi", "<think>"), Some(6));
    // 末尾 `<t` 是 `<think>` 的前 2 字节
    assert_eq!(trailing_partial("hi<t", "<think>"), Some(2));
    // 末尾不是任何前缀
    assert_eq!(trailing_partial("hello", "<think>"), None);
    // 整个字符串是 target 前缀（不含完整 target）
    assert_eq!(trailing_partial("<thi", "<think>"), Some(0));
}

// ── 通用 body 层剥图兜底(matrix 显式声明无 vision)──

fn image_req() -> ResponsesRequest {
    ResponsesRequest {
        model: Some("gpt-4".to_string()),
        input: json!([{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": "look"},
                {"type": "input_image", "image_url": "http://example.com/img.png"}
            ]
        }]),
        ..Default::default()
    }
}

#[test]
fn matrix_without_vision_strips_images_for_generic_provider() {
    // 复现缺口:候选层 vision 过滤在"全部候选都不支持"时会放行原始顺序,
    // 图片原样发给 text-only 模型 → 400。matrix 显式声明无 vision 时应剥图。
    let mut matrix = std::collections::HashMap::new();
    matrix.insert("text-only-model".to_string(), vec!["tools".to_string()]);
    let result =
        convert_with_provider_matrix(&image_req(), "text-only-model", &DefaultProvider, &matrix)
            .unwrap();
    let content = serde_json::to_string(&result.messages.last().unwrap().content).unwrap();
    assert!(!content.contains("image_url"), "图片块应被剥除: {content}");
    assert!(content.contains("look"), "文本保留");
    assert!(content.contains("image"), "应有剥图提示文本");
    assert!(
        result
            .diagnostic_events
            .iter()
            .any(|e| e.capability == "vision"),
        "应记录 vision 降级事件"
    );
}

#[test]
fn matrix_with_vision_keeps_images() {
    let mut matrix = std::collections::HashMap::new();
    matrix.insert(
        "vision-model".to_string(),
        vec!["tools".to_string(), "vision".to_string()],
    );
    let result =
        convert_with_provider_matrix(&image_req(), "vision-model", &DefaultProvider, &matrix)
            .unwrap();
    let content = serde_json::to_string(&result.messages.last().unwrap().content).unwrap();
    assert!(
        content.contains("image_url"),
        "声明 vision 的模型图片应保留"
    );
}

#[test]
fn empty_matrix_keeps_images_back_compat() {
    let matrix = std::collections::HashMap::new();
    let result =
        convert_with_provider_matrix(&image_req(), "unknown-model", &DefaultProvider, &matrix)
            .unwrap();
    let content = serde_json::to_string(&result.messages.last().unwrap().content).unwrap();
    assert!(
        content.contains("image_url"),
        "矩阵无条目时不动(与 web_search 门控同语义)"
    );
}
