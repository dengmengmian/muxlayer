use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    /// OpenAI 协议里 stream 可选,缺省 false;很多 SDK / curl 不带。
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    /// OpenAI 2024-09 标准字段（thinking 模型推荐）。MiMo / DeepSeek-thinking /
    /// GPT-5 等都接受。max_tokens 是 deprecated alias——同时填两个对老/新上游都安全。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    /// Responses 协议同名字段透传。语义一致（false = 禁用并行 tool_call）。
    /// 上游不识别会无视，识别的（OpenAI / Kimi / 多数 OpenAI-兼容）会照办。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// Internal-only diagnostics for capability degradations applied while
    /// converting or adapting a request. Never forwarded upstream.
    #[serde(skip)]
    pub diagnostic_events: Vec<CapabilityDegradationEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityDegradationEvent {
    pub kind: String,
    pub capability: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

// ── Stream chunk types ──

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatCompletionChunk {
    pub id: Option<String>,
    pub choices: Option<Vec<ChunkChoice>>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChunkChoice {
    pub delta: Option<ChunkDelta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChunkDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub reasoning_details: Option<Vec<serde_json::Value>>,
    /// Legacy single-tool format (pre-tool_calls API)
    pub function_call: Option<serde_json::Value>,
    pub tool_calls: Option<Vec<ChunkToolCall>>,
    /// Web-search citations / annotations. MiMo emits these on the first
    /// streaming chunk; OpenAI's gpt-4o-search-preview emits per chunk.
    /// Shape is provider-defined; passed through verbatim to the client.
    pub annotations: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChunkToolCall {
    pub index: Option<i64>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: Option<ChunkToolCallFunction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkToolCallFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

// ── Non-stream response ──

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatCompletionResponse {
    pub id: Option<String>,
    pub choices: Option<Vec<CompletionChoice>>,
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CompletionChoice {
    pub message: Option<CompletionMessage>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CompletionMessage {
    pub role: Option<String>,
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_message_serialization() {
        let msg = ChatMessage {
            role: "user".into(),
            content: Some(json!("hello")),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("user"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn tool_call_roundtrip() {
        let tc = ToolCall {
            id: "tc-1".into(),
            call_type: "function".into(),
            function: ToolCallFunction {
                name: "get_weather".into(),
                arguments: r#"{"city":"Beijing"}"#.into(),
            },
        };
        let json = serde_json::to_string(&tc).unwrap();
        let de: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(de.function.name, "get_weather");
    }

    #[test]
    fn chat_completions_request_skips_none_fields() {
        let req = ChatCompletionsRequest {
            model: "gpt-4".into(),
            messages: vec![],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            thinking: None,
            stream_options: None,
            response_format: None,
            reasoning_effort: None,
            seed: None,
            stop: None,
            frequency_penalty: None,
            presence_penalty: None,
            parallel_tool_calls: None,
            diagnostic_events: Vec::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("temperature"));
        assert!(!json.contains("max_tokens"));
    }

    #[test]
    fn chunk_delta_deserialization() {
        let json = r#"{"role":"assistant","content":"hi","reasoning_content":"think"}"#;
        let delta: ChunkDelta = serde_json::from_str(json).unwrap();
        assert_eq!(delta.role, Some("assistant".into()));
        assert_eq!(delta.content, Some("hi".into()));
        assert_eq!(delta.reasoning_content, Some("think".into()));
    }

    #[test]
    fn completion_choice_deserialization() {
        let json = r#"{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}"#;
        let choice: CompletionChoice = serde_json::from_str(json).unwrap();
        assert_eq!(choice.finish_reason, Some("stop".into()));
        assert_eq!(choice.message.as_ref().unwrap().content, Some("ok".into()));
    }
}
