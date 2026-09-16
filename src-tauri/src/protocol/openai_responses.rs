use serde::Deserialize;
use serde_json::Value;

/// Loosely-typed Responses API request — accepts unknown fields via flatten.
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct ResponsesRequest {
    pub model: Option<String>,
    pub input: Value,
    pub instructions: Option<String>,
    pub system: Option<String>,
    pub previous_response_id: Option<String>,
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
    pub stream: Option<bool>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_output_tokens: Option<i64>,
    pub parallel_tool_calls: Option<bool>,
    pub reasoning: Option<Value>,
    pub text: Option<Value>,
    pub metadata: Option<Value>,
    pub seed: Option<Value>,
    pub stop: Option<Value>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

impl ResponsesRequest {
    /// Codex 新版协议（gpt-5.6+）把工具定义放进 input 数组的
    /// `{"type":"additional_tools","role":"developer","tools":[...]}` 项，
    /// 不再放顶层 `tools` 字段。这里把它们提升合并进 `tools` 并从 input
    /// 中移除该项——下游各协议转换（chat / anthropic / gemini）只认顶层
    /// `tools`，不提升则工具整批丢失，模型只能在正文里输出假 `<tool_call>`。
    /// 同名工具以顶层 `tools` 里已有的为准，不重复追加。
    pub fn hoist_additional_tools(&mut self) {
        let hoisted = take_additional_tools(&mut self.input);
        if hoisted.is_empty() {
            return;
        }
        merge_tools(self.tools.get_or_insert_with(Vec::new), hoisted);
    }
}

/// 与 [`ResponsesRequest::hoist_additional_tools`] 同一套语义，但作用在**原始
/// 请求 JSON** 上。原生直通把 body 原样转发给上游，结构体上的提升对它无效——
/// 上游只认顶层 `tools`，不在 body 上做同样的提升就等于没带工具。
/// 在 Value 上改而不是重新序列化结构体，是为了保住 client_metadata / include
/// 这类结构体没有显式建模的字段。
pub fn hoist_additional_tools_in_body(body: &mut Value) {
    let Some(input) = body.get_mut("input") else {
        return;
    };
    let hoisted = take_additional_tools(input);
    if hoisted.is_empty() {
        return;
    }
    let tools = match body.get_mut("tools") {
        Some(Value::Array(existing)) => existing,
        _ => {
            body["tools"] = Value::Array(Vec::new());
            body["tools"].as_array_mut().expect("just inserted")
        }
    };
    merge_tools(tools, hoisted);
}

/// 摘出 input 数组里 `additional_tools` 项携带的工具，并把这些项从 input 移除。
fn take_additional_tools(input: &mut Value) -> Vec<Value> {
    let Value::Array(items) = input else {
        return Vec::new();
    };
    let mut hoisted: Vec<Value> = Vec::new();
    items.retain(|item| {
        let is_additional = item.get("type").and_then(|t| t.as_str()) == Some("additional_tools");
        if is_additional {
            if let Some(Value::Array(tools)) = item.get("tools") {
                hoisted.extend(tools.iter().cloned());
            }
        }
        !is_additional
    });
    hoisted
}

/// 合并工具，同名以 `tools` 里已有的为准。
fn merge_tools(tools: &mut Vec<Value>, hoisted: Vec<Value>) {
    let existing: std::collections::HashSet<String> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    tools.extend(hoisted.into_iter().filter(|t| {
        t.get("name")
            .and_then(|n| n.as_str())
            .is_none_or(|n| !existing.contains(n))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn responses_request_deserialization() {
        let body = json!({
            "model": "gpt-4",
            "input": "hello",
            "stream": true,
            "temperature": 0.5,
            "extra_field": 123
        });
        let req: ResponsesRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.model, Some("gpt-4".into()));
        assert_eq!(req.stream, Some(true));
        assert_eq!(req.temperature, Some(0.5));
        assert!(req.extra.contains_key("extra_field"));
    }

    #[test]
    fn responses_request_default() {
        let req = ResponsesRequest::default();
        assert!(req.model.is_none());
        assert!(req.input.is_null());
    }

    #[test]
    fn hoist_moves_additional_tools_into_top_level_tools() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {"type": "additional_tools", "role": "developer", "tools": [
                    {"type": "custom", "name": "exec", "description": "run js"},
                    {"type": "function", "name": "wait", "parameters": {}}
                ]},
                {"type": "message", "role": "user", "content": "hi"}
            ]
        });
        let mut req: ResponsesRequest = serde_json::from_value(body).unwrap();
        req.hoist_additional_tools();

        let tools = req.tools.as_ref().expect("tools hoisted");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        assert_eq!(names, vec!["exec", "wait"]);
        // additional_tools 项从 input 移除,不再落进 convert_input 的
        // 未知类型兜底(会产生空 system message)
        let items = req.input.as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
    }

    #[test]
    fn hoist_merges_with_existing_tools_without_duplicates() {
        let body = json!({
            "input": [
                {"type": "additional_tools", "tools": [
                    {"type": "function", "name": "shell"},
                    {"type": "function", "name": "wait"}
                ]}
            ],
            "tools": [{"type": "function", "name": "shell", "description": "top-level wins"}]
        });
        let mut req: ResponsesRequest = serde_json::from_value(body).unwrap();
        req.hoist_additional_tools();

        let tools = req.tools.as_ref().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["description"], "top-level wins");
        assert_eq!(tools[1]["name"], "wait");
    }

    #[test]
    fn hoist_is_noop_without_additional_tools() {
        let body = json!({
            "input": [{"type": "message", "role": "user", "content": "hi"}],
            "tools": [{"type": "function", "name": "shell"}]
        });
        let mut req: ResponsesRequest = serde_json::from_value(body).unwrap();
        req.hoist_additional_tools();
        assert_eq!(req.tools.as_ref().unwrap().len(), 1);
        assert_eq!(req.input.as_array().unwrap().len(), 1);
    }

    #[test]
    fn hoist_is_noop_for_string_input() {
        let body = json!({ "input": "hello" });
        let mut req: ResponsesRequest = serde_json::from_value(body).unwrap();
        req.hoist_additional_tools();
        assert!(req.tools.is_none());
        assert_eq!(req.input, json!("hello"));
    }

    // ── 直通用的 body 级提升 ──────────────────────────────────────

    #[test]
    fn hoist_in_body_moves_tools_to_top_level_and_keeps_unmodeled_fields() {
        let mut body = json!({
            "model": "deepseek-flash",
            "client_metadata": {"session_id": "s1"},
            "include": ["reasoning.encrypted_content"],
            "input": [
                {"type": "additional_tools", "role": "developer",
                 "tools": [{"type": "function", "name": "shell"}]},
                {"type": "message", "role": "user", "content": "hi"}
            ]
        });
        hoist_additional_tools_in_body(&mut body);
        assert_eq!(
            body["tools"],
            json!([{"type": "function", "name": "shell"}])
        );
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert_eq!(body["input"][0]["type"], "message");
        // 结构体没建模的字段必须原样保留,直通才是直通
        assert_eq!(body["client_metadata"]["session_id"], "s1");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }

    #[test]
    fn hoist_in_body_merges_with_existing_tools_without_duplicates() {
        let mut body = json!({
            "tools": [{"type": "function", "name": "shell", "description": "keep me"}],
            "input": [
                {"type": "additional_tools",
                 "tools": [{"type": "function", "name": "shell", "description": "drop me"},
                           {"type": "custom", "name": "apply_patch"}]}
            ]
        });
        hoist_additional_tools_in_body(&mut body);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["description"], "keep me");
        assert_eq!(tools[1]["name"], "apply_patch");
    }

    #[test]
    fn hoist_in_body_is_noop_without_additional_tools() {
        let mut body = json!({"input": [{"type": "message", "role": "user", "content": "hi"}]});
        let before = body.clone();
        hoist_additional_tools_in_body(&mut body);
        assert_eq!(body, before);
    }

    #[test]
    fn responses_request_unknown_fields_via_flatten() {
        let body = json!({
            "input": [],
            "custom_key": "custom_value"
        });
        let req: ResponsesRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.extra.get("custom_key").unwrap(), "custom_value");
    }
}
