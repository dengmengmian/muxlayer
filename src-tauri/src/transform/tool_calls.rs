use std::borrow::Cow;
use std::collections::HashMap;

use serde_json::{json, Value};

use crate::errors::AppError;
use crate::protocol::chat_completions::ChatMessage;
use crate::transform::schema_cleaner::clean_schema_for_deepseek;

/// Tool call id 上限。OpenAI Responses API 规范长度上限 64；其它上游通常更宽松。
pub const MAX_CALL_ID_LEN: usize = 64;

/// Tool **name** 上限。Anthropic / OpenAI 都是 `^[a-zA-Z0-9_-]{1,128}$`。
pub const MAX_TOOL_NAME_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallResponseKind {
    Function {
        name: String,
        namespace: Option<String>,
    },
    Custom {
        name: String,
    },
    ToolSearch,
}

pub type ToolCallResolutionMap = HashMap<String, ToolCallResponseKind>;

/// Codex Responses Lite 把顶层 function / custom 工具包进这个命名空间。
/// 它是默认命名空间：发给 Chat Completions 时不要加 `functions__` 前缀，
/// 回写给 Codex 时也不要带 `namespace: "functions"`，否则
/// `codex_core::tools::router` 会把 `exec` 当成错误 payload 拒掉。
pub const DEFAULT_FUNCTION_NAMESPACE: &str = "functions";

/// 把 namespace + tool name 收成 Chat Completions 侧的 function 名。
/// 默认 `functions` 命名空间不前缀。
pub fn namespaced_chat_name(namespace: &str, name: &str) -> String {
    if namespace.is_empty() || namespace == DEFAULT_FUNCTION_NAMESPACE {
        name.to_string()
    } else {
        format!("{namespace}__{name}")
    }
}

fn namespaced_response_namespace(namespace: Option<&str>) -> Option<String> {
    namespace
        .filter(|ns| !ns.is_empty() && *ns != DEFAULT_FUNCTION_NAMESPACE)
        .map(ToString::to_string)
}

fn namespace_children(tool: &Value) -> &[Value] {
    static EMPTY: [Value; 0] = [];
    tool.get("tools")
        .or_else(|| tool.get("children"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&EMPTY)
}

/// 请求里是否带有上游 Responses API 不接受的 custom 工具（含 namespace 嵌套）。
pub fn contains_disallowed_custom_tool(tools: &[Value], allowed: &[&str]) -> bool {
    tools
        .iter()
        .any(|tool| tool_has_disallowed_custom(tool, allowed))
}

fn tool_has_disallowed_custom(tool: &Value, allowed: &[&str]) -> bool {
    match tool.get("type").and_then(Value::as_str).unwrap_or("") {
        "custom" => tool
            .get("name")
            .and_then(Value::as_str)
            .is_none_or(|name| !allowed.contains(&name)),
        "namespace" => namespace_children(tool)
            .iter()
            .any(|child| tool_has_disallowed_custom(child, allowed)),
        _ => false,
    }
}

/// tool_call arguments 必须是合法 JSON。上游 finish_reason=length / 客户端
/// cancel / 网络断 都可能截断 arguments 留半截 JSON。原样塞回客户端 →
/// 下轮 history 带病 → 严格 provider 400 "unexpected end of data"。
/// salvage 成 "{}" 至少保证下轮不挂；空字符串透传（部分 tool 无参数）。
///
/// 三个调用点都用同一份逻辑：
/// - 流式响应 finalize（gateway/sse.rs）
/// - 非流式响应（gateway/routes.rs::handle_non_stream_response）
/// - 入站 client history（transform/responses_to_chat.rs）
pub fn salvage_tool_arguments(
    raw: &str,
    tool_name: &str,
    call_id: &str,
    finish_reason: Option<&str>,
) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if serde_json::from_str::<Value>(raw).is_ok() {
        return raw.to_string();
    }
    let cause = match finish_reason {
        Some("length") => "stream truncated by length limit".to_string(),
        Some(fr) => format!("finish_reason={fr} before arguments completed"),
        None => {
            "history-resurrected: arguments not valid JSON (likely truncated last turn)".to_string()
        }
    };
    tracing::warn!(
        tool = tool_name, call_id = call_id, len = raw.len(),
        cause = %cause,
        preview = %raw.chars().take(80).collect::<String>(),
        "tool_call arguments not valid JSON; salvaged to {{}}"
    );
    "{}".to_string()
}

/// 规范化 tool call id / tool name 这类**标识符**：白名单 `[a-zA-Z0-9_-]`、
/// 超长截断、空值占位。两个语义共用一个实现，区别只在最大长度和兜底占位符。
///
/// 设计原则：**对称纯函数**。在请求侧（Responses → Chat / Anthropic）和响应
/// 侧（Chat / Anthropic SSE → Responses）都调用同一个函数，client 回传时
/// 携带的 id/name 与上游所见自动一致，**不需要任何会话级映射表**。
///
/// 触发原因：Codex.app 偶尔产出含 `:` / `/` / `.` 等字符的 call_id，
/// MiMo/DeepSeek 这类严格上游会 400。Tool name 含中文 / 点号同理。
/// 严格白名单是三家协议的最小公分母。
///
/// 仅当输入合法且长度未超时返回 [`Cow::Borrowed`]——绝大多数请求零分配。
fn sanitize_identifier<'a>(
    input: &'a str,
    max_len: usize,
    empty_placeholder: &str,
) -> Cow<'a, str> {
    let needs_truncate = input.len() > max_len;
    let has_invalid = input
        .bytes()
        .any(|b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
    if !needs_truncate && !has_invalid && !input.is_empty() {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len().min(max_len));
    for ch in input.chars().take(max_len) {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str(empty_placeholder);
    }
    Cow::Owned(out)
}

/// Tool call id sanitize。详见 [`sanitize_identifier`] 设计说明。
pub fn sanitize_call_id(id: &str) -> Cow<'_, str> {
    sanitize_identifier(id, MAX_CALL_ID_LEN, "call_unknown")
}

/// Tool name sanitize。详见 [`sanitize_identifier`] 设计说明。
pub fn sanitize_tool_name(name: &str) -> Cow<'_, str> {
    sanitize_identifier(name, MAX_TOOL_NAME_LEN, "unknown_tool")
}

// 去掉模型 id 的 `[1m]` / `[...]` 限定后缀,得到能力矩阵的 key(矩阵按 base id 存)。
// 唯一实现在 `providers::model_id`;保留此名供 gateway 既有调用点使用。
pub(crate) use crate::providers::model_id::strip_qualifier as model_base;

/// Convert Responses API tools to Chat Completions tools format —— 简化入口，
/// 适用于不需要 provider-type / capability matrix 的场景（如 Anthropic
/// fallback 转 Chat 时）。完整能力请走 [`convert_tools_with_matrix`]。
pub fn convert_tools(tools: &[Value], clean_for_deepseek: bool) -> Vec<Value> {
    convert_tools_with_matrix(tools, clean_for_deepseek, "", "", &Default::default())
}

/// Convert Responses API tools to Chat Completions tools format with provider-
/// type + per-model capability matrix awareness. Handles structure A (flat),
/// structure B (nested function), namespace tools, custom tools, and provider-
/// specific builtins (Kimi `web_search`, MiMo `web_search`).
///
/// `matrix` 是 model → capability set 的映射。若 matrix 含 target model 条目
/// 且该条目**不**包含 "web_search"，跳过 MiMo `web_search` 注入——允许用户
/// 按模型禁用（例如 MiMo 账号没开 Web Search Plugin）。matrix 空时退化为
/// 旧的"为 MiMo 总是 emit web_search"行为，向后兼容。
pub fn convert_tools_with_matrix(
    tools: &[Value],
    clean_for_deepseek: bool,
    provider_type: &str,
    model: &str,
    matrix: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<Value> {
    let is_kimi = provider_type == "kimi" || provider_type.contains("moonshot");
    let is_mimo =
        provider_type == "mimo" || provider_type == "xiaomi" || provider_type.contains("mimo");

    // Strip [1m]/[...] qualifier before looking up in matrix (matrix keys use base id).
    let model_base = model_base(model);
    // Whether to emit web_search for MiMo: only suppress when matrix has an
    // explicit entry that excludes web_search. Empty matrix → emit (back-compat).
    let mimo_emit_web_search = matrix
        .get(model_base)
        .map(|caps| caps.iter().any(|c| c == "web_search"))
        .unwrap_or(true);

    let mut result = Vec::new();
    for tool in tools {
        let tool_type = tool.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match tool_type {
            "function" => {
                if let Some(converted) = convert_function_tool(tool, clean_for_deepseek) {
                    result.push(converted);
                }
            }
            "namespace" => {
                flatten_namespace_into(tool, clean_for_deepseek, &mut result);
            }
            "custom" => {
                let name = tool
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("custom_tool");
                let desc = tool
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                result.push(convert_custom_tool(name, desc));
            }
            "local_shell" => {
                // Codex's builtin local_shell → standard function tool named "shell".
                // Codex accepts tool_calls with either name, so emitting "shell" works.
                result.push(json!({
                    "type": "function",
                    "function": {
                        "name": "shell",
                        "description": "Execute a shell command on the local machine. Returns stdout, stderr and exit code.",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "command": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Argv array, e.g. [\"ls\", \"-la\"]. The first element is the program; remaining elements are arguments."
                                },
                                "workdir": {
                                    "type": "string",
                                    "description": "Working directory to run the command in (optional)."
                                },
                                "timeout_ms": {
                                    "type": "number",
                                    "description": "Timeout in milliseconds (optional, default 30000)."
                                }
                            },
                            "required": ["command"]
                        }
                    }
                }));
            }
            "mcp" => {
                log_dropped_mcp_tool(tool);
            }
            "tool_search" => {
                // Codex Desktop's deferred tool discovery builtin is client-executed,
                // but the upstream model still needs a callable function shape so it
                // can ask Codex to reveal matching tools for the next turn.
                let desc = tool
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("Search deferred local tool metadata and return matching tools.");
                let params = tool.get("parameters").cloned().unwrap_or_else(|| {
                    json!({
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Search query for matching tools."
                            }
                        },
                        "required": ["query"]
                    })
                });
                result.push(json!({
                    "type": "function",
                    "function": {
                        "name": "tool_search",
                        "description": desc,
                        "parameters": params,
                    }
                }));
            }
            "web_search" | "web_search_preview" => {
                if is_kimi {
                    // Kimi uses builtin_function/$web_search
                    result.push(json!({
                        "type": "builtin_function",
                        "function": { "name": "$web_search" }
                    }));
                } else if is_mimo && mimo_emit_web_search {
                    // MiMo's native web_search builtin. Preserve Codex's optional
                    // config fields; MiMo's account-level "Web Search Plugin" must
                    // be activated, otherwise upstream 400s with
                    // "webSearchEnabled is false". Users opt out per-model by
                    // unchecking `web_search` in the capability matrix.
                    let mut ws = json!({ "type": "web_search" });
                    for k in &["user_location", "max_keyword", "force_search", "limit"] {
                        if let Some(v) = tool.get(*k) {
                            ws[*k] = v.clone();
                        }
                    }
                    result.push(ws);
                }
                // Other providers: skip (not supported by Chat Completions)
            }
            _ => {
                // Skip code_interpreter, file_search, etc.
            }
        }
    }
    dedupe_tools_by_name(result)
}

fn log_dropped_mcp_tool(tool: &Value) {
    let label = tool
        .get("server_label")
        .or_else(|| tool.get("connector_id"))
        .or_else(|| tool.get("server_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("(unnamed)");
    if tool
        .get("connector_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .is_some()
    {
        tracing::debug!(
            label = %label,
            "dropped OpenAI MCP connector tool; advisory system note will be injected"
        );
    } else if tool
        .get("server_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .is_some()
    {
        tracing::warn!(
            label = %label,
            "dropped remote MCP tool; Chat Completions upstream does not support OpenAI MCP runtime"
        );
    } else {
        tracing::warn!(
            label = %label,
            "dropped MCP tool without connector_id or server_url"
        );
    }
}

/// #1 修复：响应侧 namespace 工具还原。
///
/// 入站 `convert_tools_with_matrix` 把 namespace tool `{ns_name, function: foo}`
/// 拍平成 `function: ns_name__foo` 发上游（L116-122）。上游响应里 function_call.name
/// 也是 `ns_name__foo`——如果不还原，Codex Desktop 多 agent / namespace 工具
/// **找不到对应工具**。
///
/// 算法：split 第一个 `__`，前半作 namespace，后半作 tool name。返回 `None`
/// 表示不是 namespace tool（普通 function tool），原样发回。
///
/// 边缘 case：tool 名**本身**含 `__`（例如 `my__tool`），会被误判为
/// namespace tool。OpenAI / Anthropic 标准命名很少这样（标准约定 snake_case
/// 单 underscore），实际触发率极低。未来若需消除歧义，应升级为传递
/// NamespaceMap 的方案（请求侧记 map，响应侧查表）。
pub fn split_namespace_tool_name(name: &str) -> Option<(String, String)> {
    let pos = name.find("__")?;
    if pos == 0 || pos + 2 == name.len() {
        // `__foo` 或 `foo__` 都不是合法 namespace 拼接（namespace 和 tool 都不能空）
        return None;
    }
    let ns = &name[..pos];
    let tool = &name[pos + 2..];
    Some((ns.to_string(), tool.to_string()))
}

pub fn build_tool_call_resolution_map(raw_responses_body: &str) -> ToolCallResolutionMap {
    let Ok(body) = serde_json::from_str::<Value>(raw_responses_body) else {
        return ToolCallResolutionMap::new();
    };
    let Some(tools) = body.get("tools").and_then(|v| v.as_array()) else {
        return ToolCallResolutionMap::new();
    };
    let mut map = ToolCallResolutionMap::new();
    for tool in tools {
        collect_tool_resolution(tool, None, &mut map);
    }
    map
}

fn collect_tool_resolution(tool: &Value, namespace: Option<&str>, map: &mut ToolCallResolutionMap) {
    let tool_type = tool.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match tool_type {
        "function" => {
            if let Some(name) = responses_tool_name(tool) {
                let chat_name = namespaced_chat_name(namespace.unwrap_or(""), &name);
                map.entry(chat_name)
                    .or_insert_with(|| ToolCallResponseKind::Function {
                        name,
                        namespace: namespaced_response_namespace(namespace),
                    });
            }
        }
        "namespace" => {
            let ns_name = tool
                .get("name")
                .and_then(|n| n.as_str())
                .filter(|s| !s.is_empty());
            let Some(ns_name) = ns_name else {
                return;
            };
            for child in namespace_children(tool) {
                collect_tool_resolution(child, Some(ns_name), map);
            }
        }
        "custom" => {
            if let Some(name) = responses_tool_name(tool) {
                let chat_name = namespaced_chat_name(namespace.unwrap_or(""), &name);
                map.entry(chat_name)
                    .or_insert_with(|| ToolCallResponseKind::Custom { name });
            }
        }
        "tool_search" => {
            map.entry("tool_search".to_string())
                .or_insert(ToolCallResponseKind::ToolSearch);
        }
        "local_shell" => {
            map.entry("shell".to_string())
                .or_insert_with(|| ToolCallResponseKind::Function {
                    name: "shell".to_string(),
                    namespace: None,
                });
        }
        _ => {}
    }
}

fn responses_tool_name(tool: &Value) -> Option<String> {
    tool.get("name")
        .or_else(|| tool.get("function").and_then(|f| f.get("name")))
        .and_then(|n| n.as_str())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

pub fn resolve_tool_call_response_kind(
    chat_name: &str,
    map: &ToolCallResolutionMap,
) -> ToolCallResponseKind {
    if let Some(kind) = map.get(chat_name) {
        return kind.clone();
    }
    if let Some((namespace, name)) = split_namespace_tool_name(chat_name) {
        return ToolCallResponseKind::Function {
            name,
            namespace: Some(namespace),
        };
    }
    ToolCallResponseKind::Function {
        name: chat_name.to_string(),
        namespace: None,
    }
}

pub fn custom_tool_input_from_arguments(arguments: &str) -> String {
    if arguments.trim().is_empty() {
        return String::new();
    }
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(obj)) => obj
            .get("input")
            .and_then(|value| value.as_str())
            .unwrap_or(arguments)
            .to_string(),
        Ok(Value::String(s)) => s,
        _ => arguments.to_string(),
    }
}

/// Chat Completions 侧 function.arguments ← Codex custom_tool_call.input。
pub fn custom_tool_arguments_from_input(input: &str) -> String {
    json!({ "input": input }).to_string()
}

pub fn tool_search_arguments_from_arguments(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| json!({ "query": arguments }))
}

/// Chat / Anthropic / Gemini 上游回的 tool name，按请求侧 resolution 还原成
/// Codex Responses 的 `function_call` / `custom_tool_call` / `tool_search_call`。
pub fn responses_tool_call_item_from_chat_name(
    item_id: &str,
    call_id: &str,
    chat_name: &str,
    arguments: &str,
    finish_reason: Option<&str>,
    resolution: &ToolCallResolutionMap,
) -> Value {
    match resolve_tool_call_response_kind(chat_name, resolution) {
        ToolCallResponseKind::Function { name, namespace } => {
            let arguments = salvage_tool_arguments(arguments, chat_name, call_id, finish_reason);
            let mut item = json!({
                "id": item_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
            });
            if let Some(ns) = namespace {
                item["namespace"] = json!(ns);
            }
            item
        }
        ToolCallResponseKind::Custom { name } => {
            let input = custom_tool_input_from_arguments(arguments);
            json!({
                "id": item_id,
                "type": "custom_tool_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "input": input,
            })
        }
        ToolCallResponseKind::ToolSearch => {
            let arguments = tool_search_arguments_from_arguments(arguments);
            json!({
                "type": "tool_search_call",
                "status": "completed",
                "call_id": call_id,
                "execution": "client",
                "arguments": arguments,
            })
        }
    }
}

/// #7 修复：工具名去重。Codex CLI/Desktop 偶发 bug 把同名工具发两次，
/// 上游可能不接受重复（OpenAI strict mode / DeepSeek 都会 400）。
/// function tool 用 function.name 做 key，builtin 用 type 做 key。
pub fn dedupe_tools_by_name(tools: Vec<Value>) -> Vec<Value> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<Value> = Vec::with_capacity(tools.len());
    for t in tools {
        let key = match t.get("type").and_then(|x| x.as_str()) {
            Some("function") => t
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|n| format!("fn:{n}")),
            Some("builtin_function") => t
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|n| format!("builtin_fn:{n}")),
            Some(other) => Some(format!("builtin:{other}")),
            None => t
                .get("name")
                .and_then(|n| n.as_str())
                .map(|n| format!("tool:{n}")),
        };
        match key {
            Some(k) if seen.contains(&k) => {
                tracing::warn!(
                    key = %k,
                    "dropping duplicate tool — client sent it more than once (defensive dedupe)"
                );
            }
            Some(k) => {
                seen.insert(k);
                out.push(t);
            }
            None => {
                // 无法识别 key，保留
                out.push(t);
            }
        }
    }
    out
}

/// Check if converted tools contain Kimi's $web_search builtin.
pub fn contains_kimi_web_search(tools: &[Value]) -> bool {
    tools.iter().any(|t| {
        t.get("type").and_then(|ty| ty.as_str()) == Some("builtin_function")
            && t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                == Some("$web_search")
    })
}

fn convert_custom_tool(name: &str, description: &str) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": { "input": { "type": "string" } },
                "required": ["input"]
            }
        }
    })
}

fn flatten_namespace_into(tool: &Value, clean_for_deepseek: bool, result: &mut Vec<Value>) {
    let ns_name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
    for sub in namespace_children(tool) {
        let sub_type = sub.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match sub_type {
            "function" => {
                if let Some(mut converted) = convert_function_tool(sub, clean_for_deepseek) {
                    if let Some(func) = converted.get_mut("function") {
                        if let Some(name) = func
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(str::to_string)
                        {
                            let chat_name = namespaced_chat_name(ns_name, &name);
                            if chat_name != name {
                                func["name"] = json!(chat_name);
                            }
                        }
                    }
                    result.push(converted);
                }
            }
            "custom" => {
                let name = sub
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("custom_tool");
                let desc = sub
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                let chat_name = namespaced_chat_name(ns_name, name);
                result.push(convert_custom_tool(&chat_name, desc));
            }
            "namespace" => flatten_namespace_into(sub, clean_for_deepseek, result),
            _ => {}
        }
    }
}

fn convert_function_tool(tool: &Value, clean_for_deepseek: bool) -> Option<Value> {
    // Structure B `{type, function:{name, description, parameters}}` 与
    // Structure A `{type, name, description, parameters}` 只是包裹层不同，
    // 统一只取 name / description / parameters 三个字段，产出完全一致。
    let func = tool.get("function").unwrap_or(tool);
    let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let desc = func
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("");
    let mut params = func.get("parameters").cloned().unwrap_or(json!({}));

    if clean_for_deepseek {
        clean_schema_for_deepseek(&mut params);
    }

    Some(json!({
        "type": "function",
        "function": { "name": name, "description": desc, "parameters": params }
    }))
}

/// Convert tool_choice from Responses API to Chat Completions format.
pub fn convert_tool_choice(tc: &Value) -> Value {
    match tc {
        Value::String(s) => Value::String(s.clone()),
        Value::Object(obj) => {
            if obj.get("function").is_some() {
                return tc.clone();
            }
            if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                return json!({
                    "type": "function",
                    "function": { "name": name }
                });
            }
            tc.clone()
        }
        _ => tc.clone(),
    }
}

/// Fix tool message ordering for DeepSeek.
///
/// Rules (matching codex_proxy.py):
/// 1. After an assistant message with tool_calls, corresponding tool messages must follow immediately.
/// 2. System/developer messages injected between assistant and tool messages are moved before the assistant.
///    (Codex sometimes injects approval notifications as system messages.)
/// 3. Multiple tool_calls in one assistant message: tool outputs follow in matching order.
/// 4. The LAST assistant with tool_calls may have missing tool outputs (model pending request).
/// Lightweight orphan-tool-message cleanup that does NOT reorder messages.
/// Walks the conversation; whenever an assistant message has tool_calls that
/// aren't matched by subsequent tool messages (anywhere in the remaining
/// stream, not just immediately after), synthesize a placeholder tool message
/// right after the assistant. Safe to call on any provider — does not
/// reposition system/developer messages, just patches structural holes.
///
/// Use this when you want the safety net of valid tool_calls topology without
/// the stricter reordering that `fix_tool_message_order` performs.
pub fn synthesize_orphan_tool_outputs(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    // 一次遍历记下每个 tool_call_id 最后一次出现的位置；assistant(i) 的某个调用
    // "在其后被应答" 等价于 last_answer[id] > i。避免每个 assistant 重扫后缀的 O(n²)。
    let mut last_answer: HashMap<&str, usize> = HashMap::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.role == "tool" {
            if let Some(ref tcid) = msg.tool_call_id {
                last_answer.insert(tcid.as_str(), i);
            }
        }
    }

    let mut out: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for (i, msg) in messages.iter().enumerate() {
        out.push(msg.clone());

        if msg.role == "assistant" {
            if let Some(ref tcs) = msg.tool_calls {
                for tc in tcs {
                    let answered_later = last_answer.get(tc.id.as_str()).is_some_and(|&p| p > i);
                    if !answered_later {
                        out.push(ChatMessage {
                            role: "tool".to_string(),
                            content: Some(serde_json::Value::String(String::new())),
                            reasoning_content: None,
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                            name: None,
                        });
                    }
                }
            }
        }
    }
    out
}

/// 删孤儿 tool 消息——前面没对应 assistant.tool_calls.id 在窗口内（直到下一个
/// non-tool 消息为止）。Codex undo / interrupt / redo 后偶发：assistant 被
/// 撤回但 tool message 留着，原样发上游 → DeepSeek/MiMo 严格 enforce
/// invariant，400 "tool messages must be a response to a preceding assistant
/// message with tool_calls"。
///
/// 移除无配对 assistant.tool_calls 的孤儿 tool 消息。
pub fn remove_orphan_tool_messages(mut messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut valid_ids: Option<std::collections::HashSet<String>> = None;
    let mut i = 0;
    while i < messages.len() {
        let role = messages[i].role.as_str();
        if role == "assistant" {
            valid_ids = messages[i]
                .tool_calls
                .as_ref()
                .map(|tcs| tcs.iter().map(|tc| tc.id.clone()).collect());
            i += 1;
        } else if role == "tool" {
            let keep = match (&valid_ids, &messages[i].tool_call_id) {
                (Some(ids), Some(tcid)) => ids.contains(tcid),
                _ => false,
            };
            if keep {
                i += 1;
            } else {
                let tcid = messages[i].tool_call_id.clone().unwrap_or_default();
                tracing::warn!(
                    tool_call_id = %tcid,
                    "dropped orphan tool message (no preceding assistant.tool_calls in scope)"
                );
                messages.remove(i);
                // 不 i += 1：splice 后下一个元素已经填到 i
            }
        } else if role == "system" || role == "developer" {
            // system/developer 注入消息不重置 tool-receiving 窗口——Codex 会在
            // assistant(tool_calls) 和 tool(result) 之间插入 approval 通知等 system
            // 消息（fix_tool_message_order 也会把这些 system 移到 assistant 前），
            // 不能因此让后面合法的 tool message 被误判为孤儿删掉。
            i += 1;
        } else {
            // user / 其他——重置 tool-receiving 窗口
            valid_ids = None;
            i += 1;
        }
    }
    messages
}

pub fn fix_tool_message_order(messages: Vec<ChatMessage>) -> Result<Vec<ChatMessage>, AppError> {
    // 先删孤儿 tool 消息（前面没对应 assistant.tool_calls），再做重排 + 补缺失。
    // 删除 + 补缺失互补：删处理"应该没的"，补处理"应该有的"。
    let messages = remove_orphan_tool_messages(messages);

    let mut reordered: Vec<ChatMessage> = Vec::new();
    let len = messages.len();
    let mut i = 0;

    while i < len {
        let msg = &messages[i];

        if let Some(tcs) = msg.tool_calls.as_ref().filter(|_| msg.role == "assistant") {
            let mut expected_ids: Vec<String> = tcs.iter().map(|tc| tc.id.clone()).collect();
            let mut tool_msgs: Vec<ChatMessage> = Vec::new();
            let mut non_tool_msgs: Vec<ChatMessage> = Vec::new();

            let mut j = i + 1;
            while j < len && !expected_ids.is_empty() {
                let nxt = &messages[j];
                if nxt.role == "tool" {
                    if let Some(ref tcid) = nxt.tool_call_id {
                        if let Some(pos) = expected_ids.iter().position(|id| id == tcid) {
                            expected_ids.remove(pos);
                            tool_msgs.push(nxt.clone());
                            j += 1;
                            continue;
                        }
                    }
                }

                if nxt.role == "system" || nxt.role == "developer" {
                    // Move injected system messages before the assistant
                    non_tool_msgs.push(nxt.clone());
                    j += 1;
                    continue;
                }

                // Stop at user/assistant boundary
                break;
            }

            // Missing tool outputs in the middle of the conversation: synthesize
            // empty placeholders so the upstream still sees a structurally valid
            // assistant→tool sequence. Without this, providers that strictly
            // enforce the Chat Completions invariant (DeepSeek, MiMo) return 400.
            // Lenient cleanup of orphan tool outputs.
            //
            // The last assistant in the conversation is exempt: that's the pending
            // tool call awaiting our response, and synthesizing an empty placeholder
            // there would falsely tell the model the tool already returned nothing.
            let is_last = j >= len;
            if !is_last {
                for missing_id in &expected_ids {
                    tool_msgs.push(ChatMessage {
                        role: "tool".to_string(),
                        content: Some(serde_json::Value::String(String::new())),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: Some(missing_id.clone()),
                        name: None,
                    });
                }
            }

            // Put system/developer messages before assistant, then assistant, then tool messages
            reordered.extend(non_tool_msgs);
            reordered.push(msg.clone());
            reordered.extend(tool_msgs);
            i = j;
        } else {
            reordered.push(msg.clone());
            i += 1;
        }
    }

    Ok(reordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::chat_completions::{ChatMessage, ToolCall, ToolCallFunction};
    use serde_json::json;

    #[test]
    fn sanitize_call_id_passes_through_clean() {
        assert!(matches!(sanitize_call_id("call_abc123"), Cow::Borrowed(_)));
        assert!(matches!(
            sanitize_call_id("toolu_01A-9zZ"),
            Cow::Borrowed(_)
        ));
        assert_eq!(sanitize_call_id("call_abc123").as_ref(), "call_abc123");
    }

    #[test]
    fn sanitize_call_id_replaces_invalid_characters() {
        assert_eq!(
            sanitize_call_id("call:abc/def.1").as_ref(),
            "call_abc_def_1"
        );
        assert_eq!(sanitize_call_id("a b c").as_ref(), "a_b_c");
        assert_eq!(sanitize_call_id("call#7").as_ref(), "call_7");
    }

    #[test]
    fn sanitize_call_id_truncates_to_64() {
        let long = "a".repeat(200);
        let out = sanitize_call_id(&long);
        assert_eq!(out.len(), MAX_CALL_ID_LEN);
        assert!(out.chars().all(|c| c == 'a'));
    }

    #[test]
    fn sanitize_call_id_empty_becomes_placeholder() {
        assert_eq!(sanitize_call_id("").as_ref(), "call_unknown");
    }

    #[test]
    fn sanitize_call_id_is_idempotent() {
        // Applying twice yields the same result — the symmetric-application
        // property the request/response paths rely on.
        let once = sanitize_call_id("weird:id/x").into_owned();
        let twice = sanitize_call_id(&once).into_owned();
        assert_eq!(once, twice);
    }

    #[test]
    fn sanitize_call_id_unicode_collapses_to_underscores() {
        // Each non-ASCII char counts as one position, replaced by one '_'.
        assert_eq!(sanitize_call_id("调用1").as_ref(), "__1");
    }

    #[test]
    fn sanitize_tool_name_passes_clean_names() {
        assert!(matches!(
            sanitize_tool_name("get_weather"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            sanitize_tool_name("search-web-v2"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(sanitize_tool_name("MyTool123"), Cow::Borrowed(_)));
    }

    #[test]
    fn sanitize_tool_name_replaces_dots_and_unicode() {
        assert_eq!(sanitize_tool_name("foo.bar.baz").as_ref(), "foo_bar_baz");
        assert_eq!(sanitize_tool_name("查询天气").as_ref(), "____");
        assert_eq!(sanitize_tool_name("tool:v1").as_ref(), "tool_v1");
        assert_eq!(sanitize_tool_name("a b c").as_ref(), "a_b_c");
    }

    #[test]
    fn sanitize_tool_name_truncates_to_128() {
        let long = "a".repeat(200);
        assert_eq!(sanitize_tool_name(&long).len(), MAX_TOOL_NAME_LEN);
    }

    #[test]
    fn sanitize_tool_name_empty_placeholder() {
        assert_eq!(sanitize_tool_name("").as_ref(), "unknown_tool");
    }

    #[test]
    fn sanitize_tool_name_idempotent() {
        let once = sanitize_tool_name("a.b/c").into_owned();
        let twice = sanitize_tool_name(&once).into_owned();
        assert_eq!(once, twice);
    }

    #[test]
    fn test_convert_function_tool_structure_b() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "search",
                "description": "Search the web",
                "parameters": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}
            }
        });
        let result = convert_function_tool(&tool, false);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r["type"], "function");
        assert_eq!(r["function"]["name"], "search");
    }

    #[test]
    fn test_convert_function_tool_structure_a() {
        let tool = json!({
            "type": "function",
            "name": "search",
            "description": "Search the web",
            "parameters": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}
        });
        let result = convert_function_tool(&tool, false);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r["type"], "function");
        assert_eq!(r["function"]["name"], "search");
    }

    #[test]
    fn test_convert_function_tool_structures_a_and_b_normalize_identically() {
        // 两种结构必须产出同一套字段；B 不能把 strict 等额外字段整体 clone 过去。
        let params = json!({"type": "object", "properties": {"q": {"type": "string"}}});
        let a = json!({
            "type": "function", "name": "search", "description": "d",
            "parameters": params, "strict": true
        });
        let b = json!({
            "type": "function",
            "function": {"name": "search", "description": "d", "parameters": params, "strict": true}
        });
        for clean in [false, true] {
            assert_eq!(
                convert_function_tool(&a, clean),
                convert_function_tool(&b, clean),
                "clean={clean}"
            );
        }
        // B 缺 description/parameters 时与 A 一样补默认值
        assert_eq!(
            convert_function_tool(
                &json!({"type": "function", "function": {"name": "x"}}),
                false
            ),
            convert_function_tool(&json!({"type": "function", "name": "x"}), false)
        );
    }

    #[test]
    fn test_convert_tools_namespace() {
        let tools = vec![json!({
            "type": "namespace",
            "name": "math",
            "tools": [
                {"type": "function", "name": "add", "description": "Add numbers", "parameters": {"type": "object"}}
            ]
        })];
        let result = convert_tools(&tools, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["function"]["name"], "math__add");
    }

    #[test]
    fn convert_tools_flattens_codex_functions_namespace_exec_without_prefix() {
        // Codex 0.152.1 Responses Lite wraps code-mode `exec` (custom/freeform)
        // plus sibling functions inside `{type: namespace, name: functions}`.
        // DeepSeek Chat Completions 必须看到名为 `exec` 的 function，回写时
        // 也必须按 custom_tool_call 还原——前缀成 functions__exec 会被
        // Codex router 当成错误 payload 拒掉。
        let tools = vec![json!({
            "type": "namespace",
            "name": "functions",
            "description": "",
            "tools": [
                {
                    "type": "custom",
                    "name": "exec",
                    "description": "Run JavaScript code to orchestrate/compose tool calls",
                    "format": {"type": "grammar", "syntax": "lark", "definition": "start: SOURCE"}
                },
                {
                    "type": "function",
                    "name": "wait",
                    "description": "Wait for a running exec cell",
                    "parameters": {"type": "object", "properties": {}}
                }
            ]
        })];
        let result = convert_tools(&tools, false);
        let names: Vec<&str> = result
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        assert_eq!(names, vec!["exec", "wait"]);
        assert_eq!(result[0]["function"]["parameters"]["required"][0], "input");
    }

    #[test]
    fn test_convert_tools_custom() {
        let tools = vec![json!({
            "type": "custom",
            "name": "my_tool",
            "description": "Does stuff"
        })];
        let result = convert_tools(&tools, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["function"]["name"], "my_tool");
        assert_eq!(
            result[0]["function"]["parameters"]["properties"]["input"]["type"],
            "string"
        );
    }

    #[test]
    fn test_convert_tools_local_shell() {
        let tools = vec![json!({"type": "local_shell"})];
        let result = convert_tools(&tools, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "shell");
        assert!(result[0]["function"]["parameters"]["properties"]["command"].is_object());
        assert_eq!(
            result[0]["function"]["parameters"]["required"][0],
            "command"
        );
    }

    #[test]
    fn test_convert_tools_tool_search() {
        let tools = vec![json!({
            "type": "tool_search",
            "description": "Search available deferred tools",
            "parameters": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }
        })];
        let result = convert_tools(&tools, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "tool_search");
        assert_eq!(
            result[0]["function"]["description"],
            "Search available deferred tools"
        );
        assert_eq!(result[0]["function"]["parameters"]["required"][0], "query");
    }

    #[test]
    fn tool_resolution_restores_namespace_custom_and_tool_search_semantics() {
        let raw = json!({
            "tools": [
                {
                    "type": "namespace",
                    "name": "multi_agent_v1",
                    "tools": [
                        {"type": "function", "name": "spawn_agent", "parameters": {"type": "object"}}
                    ]
                },
                {"type": "custom", "name": "grammar_answer"},
                {"type": "tool_search"}
            ]
        })
        .to_string();
        let map = build_tool_call_resolution_map(&raw);

        assert_eq!(
            resolve_tool_call_response_kind("multi_agent_v1__spawn_agent", &map),
            ToolCallResponseKind::Function {
                name: "spawn_agent".into(),
                namespace: Some("multi_agent_v1".into())
            }
        );
        assert_eq!(
            resolve_tool_call_response_kind("grammar_answer", &map),
            ToolCallResponseKind::Custom {
                name: "grammar_answer".into()
            }
        );
        assert_eq!(
            resolve_tool_call_response_kind("tool_search", &map),
            ToolCallResponseKind::ToolSearch
        );
    }

    #[test]
    fn tool_resolution_marks_functions_namespace_exec_as_custom() {
        let raw = json!({
            "tools": [{
                "type": "namespace",
                "name": "functions",
                "tools": [
                    {"type": "custom", "name": "exec", "description": "run js"},
                    {"type": "function", "name": "wait", "parameters": {"type": "object"}}
                ]
            }]
        })
        .to_string();
        let map = build_tool_call_resolution_map(&raw);
        assert_eq!(
            map.get("exec"),
            Some(&ToolCallResponseKind::Custom {
                name: "exec".into()
            })
        );
        assert_eq!(
            map.get("wait"),
            Some(&ToolCallResponseKind::Function {
                name: "wait".into(),
                namespace: None
            })
        );
        assert!(!map.contains_key("functions__exec"));
        assert!(!map.contains_key("functions__wait"));
    }

    #[test]
    fn tool_resolution_preserves_user_function_named_tool_search() {
        let raw = json!({
            "tools": [
                {"type": "function", "name": "tool_search", "parameters": {"type": "object"}},
                {"type": "tool_search"}
            ]
        })
        .to_string();
        let map = build_tool_call_resolution_map(&raw);
        assert_eq!(
            resolve_tool_call_response_kind("tool_search", &map),
            ToolCallResponseKind::Function {
                name: "tool_search".into(),
                namespace: None
            }
        );
    }

    #[test]
    fn custom_and_tool_search_arguments_restore_responses_shapes() {
        assert_eq!(
            custom_tool_input_from_arguments(r#"{"input":"hello"}"#),
            "hello"
        );
        assert_eq!(custom_tool_input_from_arguments("plain"), "plain");
        assert_eq!(
            custom_tool_input_from_arguments(r#"await tools.exec_command({cmd:"pwd"})"#),
            r#"await tools.exec_command({cmd:"pwd"})"#
        );
        assert_eq!(
            custom_tool_arguments_from_input(r#"await tools.exec_command({cmd:"pwd"})"#),
            json!({"input": r#"await tools.exec_command({cmd:"pwd"})"#}).to_string()
        );
        assert_eq!(
            tool_search_arguments_from_arguments(r#"{"query":"rust","limit":3}"#),
            json!({"query": "rust", "limit": 3})
        );
        assert_eq!(
            tool_search_arguments_from_arguments("rust"),
            json!({"query": "rust"})
        );
    }

    #[test]
    fn responses_item_restores_namespaced_exec_as_custom_tool_call() {
        let raw = json!({
            "tools": [{
                "type": "namespace",
                "name": "functions",
                "tools": [
                    {"type": "custom", "name": "exec", "description": "run js"},
                    {"type": "function", "name": "wait", "parameters": {"type": "object"}}
                ]
            }]
        })
        .to_string();
        let map = build_tool_call_resolution_map(&raw);
        let js = r#"await tools.exec_command({cmd:"pwd"})"#;
        let item = responses_tool_call_item_from_chat_name(
            "fc_1",
            "call_1",
            "exec",
            &json!({"input": js}).to_string(),
            None,
            &map,
        );
        assert_eq!(item["type"], "custom_tool_call");
        assert_eq!(item["name"], "exec");
        assert_eq!(item["call_id"], "call_1");
        assert_eq!(item["input"], js);
        assert!(item.get("arguments").is_none());
        assert!(item.get("namespace").is_none());

        let wait =
            responses_tool_call_item_from_chat_name("fc_2", "call_2", "wait", "{}", None, &map);
        assert_eq!(wait["type"], "function_call");
        assert_eq!(wait["name"], "wait");
        assert_eq!(wait["arguments"], "{}");
    }

    #[test]
    fn test_convert_tools_drops_mcp() {
        let tools = vec![json!({
            "type": "mcp",
            "server_label": "GitHub",
            "connector_id": "github"
        })];
        let result = convert_tools(&tools, false);
        assert!(result.is_empty());
    }

    #[test]
    fn test_dedupe_tools_by_top_level_name() {
        let tools = vec![
            json!({"name": "search", "description": "a"}),
            json!({"name": "search", "description": "b"}),
            json!({"name": "read_file", "description": "c"}),
        ];
        let result = dedupe_tools_by_name(tools);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["description"], "a");
        assert_eq!(result[1]["name"], "read_file");
    }

    #[test]
    fn test_convert_tools_web_search_kimi() {
        let tools = vec![json!({"type": "web_search"})];
        let result = convert_tools_with_matrix(&tools, false, "kimi", "", &Default::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "builtin_function");
        assert_eq!(result[0]["function"]["name"], "$web_search");
    }

    #[test]
    fn test_convert_tools_web_search_non_kimi() {
        let tools = vec![json!({"type": "web_search"})];
        let result = convert_tools_with_matrix(&tools, false, "openai", "", &Default::default());
        assert!(result.is_empty());
    }

    #[test]
    fn test_convert_tools_web_search_mimo_minimal() {
        let tools = vec![json!({"type": "web_search_preview"})];
        let result = convert_tools_with_matrix(&tools, false, "mimo", "", &Default::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "web_search");
    }

    #[test]
    fn test_convert_tools_mimo_matrix_suppresses_web_search() {
        // Matrix entry exists for the target model but doesn't include web_search
        // → user has opted out, translator must skip emitting web_search.
        let tools = vec![json!({"type": "web_search"})];
        let mut matrix: std::collections::HashMap<String, Vec<String>> = Default::default();
        matrix.insert(
            "mimo-v2.5".to_string(),
            vec!["text".into(), "vision".into()],
        );
        let result = convert_tools_with_matrix(&tools, false, "mimo", "mimo-v2.5", &matrix);
        assert!(
            result.is_empty(),
            "web_search should be suppressed when matrix says model doesn't have it"
        );
    }

    #[test]
    fn test_convert_tools_mimo_matrix_qualifier_stripped() {
        // model = "mimo-v2.5-pro[1m]" — should look up "mimo-v2.5-pro" in matrix.
        let tools = vec![json!({"type": "web_search"})];
        let mut matrix: std::collections::HashMap<String, Vec<String>> = Default::default();
        matrix.insert("mimo-v2.5-pro".to_string(), vec!["text".into()]);
        let result = convert_tools_with_matrix(&tools, false, "mimo", "mimo-v2.5-pro[1m]", &matrix);
        assert!(
            result.is_empty(),
            "[1m] qualifier should be stripped before matrix lookup"
        );
    }

    #[test]
    fn test_convert_tools_mimo_matrix_allows_web_search_when_listed() {
        let tools = vec![json!({"type": "web_search"})];
        let mut matrix: std::collections::HashMap<String, Vec<String>> = Default::default();
        matrix.insert(
            "mimo-v2.5-pro".to_string(),
            vec!["text".into(), "web_search".into()],
        );
        let result = convert_tools_with_matrix(&tools, false, "mimo", "mimo-v2.5-pro", &matrix);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "web_search");
    }

    #[test]
    fn test_convert_tools_mimo_empty_matrix_keeps_back_compat() {
        // No matrix configured → keep emitting (existing behavior).
        let tools = vec![json!({"type": "web_search"})];
        let result =
            convert_tools_with_matrix(&tools, false, "mimo", "mimo-v2.5-pro", &Default::default());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_convert_tools_mimo_matrix_unknown_model_keeps_emit() {
        // model not in matrix at all → emit (matrix only restricts when entry exists).
        let tools = vec![json!({"type": "web_search"})];
        let mut matrix: std::collections::HashMap<String, Vec<String>> = Default::default();
        matrix.insert("other-model".to_string(), vec!["text".into()]);
        let result = convert_tools_with_matrix(&tools, false, "mimo", "mimo-v2.5-pro", &matrix);
        assert_eq!(
            result.len(),
            1,
            "unknown model → assume web_search supported"
        );
    }

    #[test]
    fn test_convert_tools_web_search_mimo_preserves_optional_fields() {
        let tools = vec![json!({
            "type": "web_search",
            "max_keyword": 3,
            "force_search": true,
            "limit": 10,
            "user_location": {"country": "CN"}
        })];
        let result = convert_tools_with_matrix(&tools, false, "mimo", "", &Default::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "web_search");
        assert_eq!(result[0]["max_keyword"], 3);
        assert_eq!(result[0]["force_search"], true);
        assert_eq!(result[0]["limit"], 10);
        assert_eq!(result[0]["user_location"]["country"], "CN");
    }

    #[test]
    fn test_contains_kimi_web_search() {
        let tools = vec![json!({"type": "builtin_function", "function": {"name": "$web_search"}})];
        assert!(contains_kimi_web_search(&tools));

        let other = vec![json!({"type": "function", "function": {"name": "search"}})];
        assert!(!contains_kimi_web_search(&other));
    }

    #[test]
    fn test_convert_tool_choice_string() {
        assert_eq!(convert_tool_choice(&json!("auto")), json!("auto"));
        assert_eq!(convert_tool_choice(&json!("none")), json!("none"));
    }

    #[test]
    fn test_convert_tool_choice_function() {
        let tc = json!({"function": {"name": "search"}});
        assert_eq!(convert_tool_choice(&tc), tc);
    }

    #[test]
    fn test_convert_tool_choice_name() {
        let tc = json!({"name": "search"});
        let result = convert_tool_choice(&tc);
        assert_eq!(result["type"], "function");
        assert_eq!(result["function"]["name"], "search");
    }

    #[test]
    fn test_fix_tool_message_order_basic() {
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: Some(json!("call tool")),
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some(json!("result")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: None,
            },
        ];
        let result = fix_tool_message_order(messages).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "assistant");
        assert_eq!(result[1].role, "tool");
    }

    #[test]
    fn synthesize_orphan_fills_missing_tool_outputs_without_reordering() {
        // assistant with 2 tool_calls; only one has output; placeholder is
        // appended directly after the assistant so the topology is valid.
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: None,
                reasoning_content: None,
                tool_calls: Some(vec![
                    ToolCall {
                        id: "a".into(),
                        call_type: "function".into(),
                        function: ToolCallFunction {
                            name: "f".into(),
                            arguments: "{}".into(),
                        },
                    },
                    ToolCall {
                        id: "b".into(),
                        call_type: "function".into(),
                        function: ToolCallFunction {
                            name: "g".into(),
                            arguments: "{}".into(),
                        },
                    },
                ]),
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some(json!("result-a")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("a".into()),
                name: None,
            },
        ];
        let out = synthesize_orphan_tool_outputs(messages);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].tool_call_id.as_deref(), Some("b"));
        assert!(matches!(out[1].content, Some(serde_json::Value::String(ref s)) if s.is_empty()));
        assert_eq!(out[2].tool_call_id.as_deref(), Some("a"));
    }

    #[test]
    fn synthesize_orphan_noop_when_all_tool_outputs_present() {
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: None,
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "x".into(),
                    call_type: "function".into(),
                    function: ToolCallFunction {
                        name: "f".into(),
                        arguments: "{}".into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some(json!("ok")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("x".into()),
                name: None,
            },
        ];
        let out = synthesize_orphan_tool_outputs(messages.clone());
        assert_eq!(out.len(), messages.len());
    }

    #[test]
    fn test_fix_tool_message_order_missing_non_last_synthesizes_placeholder() {
        // Non-last assistant with missing tool output: cleanup must synthesize
        // an empty placeholder tool message so the topology stays valid for
        // strict upstreams (DeepSeek / MiMo).
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: Some(json!("call tool")),
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: Some(json!("hello")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        let result = fix_tool_message_order(messages).expect("orphan now synthesized, not errored");
        // assistant + synthesized tool placeholder + user
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].role, "assistant");
        assert_eq!(result[1].role, "tool");
        assert_eq!(result[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(result[2].role, "user");
    }

    #[test]
    fn test_fix_tool_message_order_lenient_last() {
        let messages = vec![ChatMessage {
            role: "assistant".to_string(),
            content: Some(json!("call tool")),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                    name: "search".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
        }];
        let result = fix_tool_message_order(messages).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_fix_tool_message_order_multiple_tools() {
        // Tool messages preserved in input order (matching Python codex_proxy behavior)
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: Some(json!("call tools")),
                reasoning_content: None,
                tool_calls: Some(vec![
                    ToolCall {
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: ToolCallFunction {
                            name: "a".to_string(),
                            arguments: "{}".to_string(),
                        },
                    },
                    ToolCall {
                        id: "call_2".to_string(),
                        call_type: "function".to_string(),
                        function: ToolCallFunction {
                            name: "b".to_string(),
                            arguments: "{}".to_string(),
                        },
                    },
                ]),
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some(json!("r2")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("call_2".to_string()),
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some(json!("r1")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: None,
            },
        ];
        let result = fix_tool_message_order(messages).unwrap();
        // Tool messages keep input order (call_2 first, then call_1)
        assert_eq!(result[1].tool_call_id, Some("call_2".to_string()));
        assert_eq!(result[2].tool_call_id, Some("call_1".to_string()));
    }

    #[test]
    fn test_fix_tool_message_order_orphan_tool_removed() {
        // #2 修复后行为：[user, tool(orphan)] → 孤儿 tool 被删，只剩 user。
        // 不删的话上游严格 enforce invariant 会 400。
        let messages = vec![
            ChatMessage {
                role: "user".to_string(),
                content: Some(json!("hi")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some(json!("orphan")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("call_x".to_string()),
                name: None,
            },
        ];
        let result = fix_tool_message_order(messages).unwrap();
        assert_eq!(result.len(), 1, "orphan tool message should be dropped");
        assert_eq!(result[0].role, "user");
    }

    #[test]
    fn test_fix_tool_message_order_moves_system_before_assistant() {
        // Codex injects system messages (e.g. approval notifications) between
        // assistant(tool_calls) and tool messages. These must be moved before
        // the assistant for DeepSeek compatibility.
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: Some(json!("calling tool")),
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "system".to_string(),
                content: Some(json!("approval notification")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some(json!("result")),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: None,
            },
        ];
        let result = fix_tool_message_order(messages).unwrap();
        assert_eq!(result.len(), 3);
        // System message moved before assistant
        assert_eq!(result[0].role, "system");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "tool");
    }
}
