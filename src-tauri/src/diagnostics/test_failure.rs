//! Classify test-connection failures into human-readable diagnostics.
//!
//! The Test Connection dialog used to surface raw `HTTP 401: <body>` /
//! `Connection error: ...` strings, which new users can't act on. This
//! module folds the upstream's machine-shaped error into:
//!   - a stable `code` (for analytics + UI branching),
//!   - a one-line plain-language `title` (what went wrong),
//!   - a one-line `hint` (what to do),
//!   - an optional `action_url` + `action_label` (a console page the user
//!     can open to fix it),
//!   - `raw` (the original error string, kept for power users).
//!
//! Detection reuses the cross-provider markers already shared by
//! `transform/providers/mod.rs::detect_auth_error` etc., plus a per-
//! provider URL table for the action button.

use serde::Serialize;

#[cfg(any(feature = "desktop", test))]
use crate::transform::providers as p;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, specta::Type)]
pub struct TestDiagnostic {
    pub code: String,
    pub title: String,
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_label: Option<String>,
    pub raw: String,
}

// 失败诊断只服务桌面端「测试连接」命令,cli 构建不编译。
#[cfg(any(feature = "desktop", test))]
#[derive(Default, Debug, Clone, Copy)]
struct ProviderConsoleUrls {
    keys: Option<&'static str>,
    billing: Option<&'static str>,
    plugin: Option<&'static str>,
}

#[cfg(any(feature = "desktop", test))]
fn console_urls(provider_type: &str) -> ProviderConsoleUrls {
    let pt = provider_type.trim().to_ascii_lowercase();
    match pt.as_str() {
        "mimo" | "xiaomi" => ProviderConsoleUrls {
            keys: Some("https://platform.xiaomimimo.com/#/console/api-keys"),
            billing: Some("https://platform.xiaomimimo.com/#/console/usage"),
            plugin: Some("https://platform.xiaomimimo.com/#/console/plugin"),
        },
        "deepseek" => ProviderConsoleUrls {
            keys: Some("https://platform.deepseek.com/api_keys"),
            billing: Some("https://platform.deepseek.com/usage"),
            plugin: None,
        },
        "kimi" | "moonshot" => ProviderConsoleUrls {
            keys: Some("https://platform.moonshot.cn/console/api-keys"),
            billing: Some("https://platform.moonshot.cn/console/account"),
            plugin: None,
        },
        "openai" => ProviderConsoleUrls {
            keys: Some("https://platform.openai.com/api-keys"),
            billing: Some("https://platform.openai.com/account/billing"),
            plugin: None,
        },
        "anthropic" | "claude" => ProviderConsoleUrls {
            keys: Some("https://console.anthropic.com/settings/keys"),
            billing: Some("https://console.anthropic.com/settings/billing"),
            plugin: None,
        },
        "google_gemini" => ProviderConsoleUrls {
            keys: Some("https://aistudio.google.com/apikey"),
            billing: None,
            plugin: None,
        },
        "zhipu" | "glm" => ProviderConsoleUrls {
            keys: Some("https://open.bigmodel.cn/usercenter/apikeys"),
            billing: Some("https://open.bigmodel.cn/usercenter/finance"),
            plugin: None,
        },
        "dashscope" | "qwen" => ProviderConsoleUrls {
            keys: Some("https://bailian.console.aliyun.com/?apiKey=1"),
            billing: None,
            plugin: None,
        },
        "siliconflow" => ProviderConsoleUrls {
            keys: Some("https://cloud.siliconflow.cn/account/ak"),
            billing: Some("https://cloud.siliconflow.cn/account/balance"),
            plugin: None,
        },
        "volcengine" | "doubao" => ProviderConsoleUrls {
            keys: Some("https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey"),
            billing: None,
            plugin: None,
        },
        _ => ProviderConsoleUrls::default(),
    }
}

/// Classify a test-connection failure. `status` is the HTTP status seen on
/// the last attempt (None when the request never reached the upstream); `body`
/// is the upstream's error response text; `raw_error` is the full message we
/// would have shown without diagnosis (used as the fallback `raw` field).
#[cfg(any(feature = "desktop", test))]
pub fn diagnose(
    provider_type: &str,
    status: Option<u16>,
    body: &str,
    raw_error: &str,
) -> TestDiagnostic {
    let urls = console_urls(provider_type);
    let body_lower = body.to_ascii_lowercase();

    // ── HTTP-status failures ─────────────────────────────────────────
    if let Some(code) = status {
        // MiMo-specific: web_search plugin not activated. Caught BEFORE the
        // generic auth check because the upstream returns 400 with text that
        // also contains "false", which the auth heuristic could mis-flag.
        if code == 400 && body_lower.contains("websearchenabled is false") {
            return TestDiagnostic {
                code: "web_search_plugin_disabled".into(),
                title: "MiMo Web Search Plugin 未开通".into(),
                hint: "按次计费的联网搜索插件需要先在 MiMo 控制台开通；如果用不到，从 default_model 的能力矩阵里取消 web_search 即可。".into(),
                action_url: urls.plugin.map(str::to_string),
                action_label: urls.plugin.map(|_| "去 MiMo 控制台开通".to_string()),
                raw: raw_error.to_string(),
            };
        }

        if p::detect_auth_error(code, body) {
            return TestDiagnostic {
                code: "invalid_api_key".into(),
                title: "API key 无效或已过期".into(),
                hint: "去 Provider 控制台重建 key，复制粘贴回 AgentGate。注意 sk-* / tp-* 等前缀对应不同账户类型。".into(),
                action_url: urls.keys.map(str::to_string),
                action_label: urls.keys.map(|_| "去重建 key".to_string()),
                raw: raw_error.to_string(),
            };
        }

        if p::detect_insufficient_balance(code, body) {
            return TestDiagnostic {
                code: "insufficient_balance".into(),
                title: "账户余额或配额不足".into(),
                hint: "去 Provider 控制台充值或确认 Token Plan 还有剩余。AgentGate 会自动 failover 到其它非冷却 provider。".into(),
                action_url: urls.billing.map(str::to_string),
                action_label: urls.billing.map(|_| "查看账户余额".to_string()),
                raw: raw_error.to_string(),
            };
        }

        if p::detect_rate_limit(code, body) {
            return TestDiagnostic {
                code: "rate_limited".into(),
                title: "触发了 Provider 限流".into(),
                hint: "降低并发或等待几分钟再测；AgentGate 已自动冷却这个 provider。".into(),
                action_url: None,
                action_label: None,
                raw: raw_error.to_string(),
            };
        }

        // Model not found — usually a typo in default_model or upstream removed
        // the model id. Two paths: English ("model ... not found/supported")
        // or Chinese ("模型 ... 不存在/不支持").
        if code == 400 || code == 404 {
            let en_match = body_lower.contains("model")
                && (body_lower.contains("not found")
                    || body_lower.contains("not supported")
                    || body_lower.contains("does not exist")
                    || body_lower.contains("invalid model"));
            let zh_match = body.contains("模型")
                && (body.contains("不存在") || body.contains("不支持") || body.contains("已下线"));
            if en_match || zh_match {
                return TestDiagnostic {
                    code: "model_not_found".into(),
                    title: "模型不存在或已下线".into(),
                    hint: "Provider 已下线这个模型 id 了。先点「拉取并识别能力」更新模型列表，再选一个仍在线的当 default_model。".into(),
                    action_url: None,
                    action_label: None,
                    raw: raw_error.to_string(),
                };
            }
        }

        if code == 403 && (body_lower.contains("region") || body_lower.contains("country")) {
            return TestDiagnostic {
                code: "region_blocked".into(),
                title: "当前区域不允许访问".into(),
                hint: "检查 base_url 是否匹配你 key 所属的区域（比如 MiMo Token Plan 的 cn / sgp / ams）。如果在境外访问，可能需要换 base_url 或加代理。".into(),
                action_url: None,
                action_label: None,
                raw: raw_error.to_string(),
            };
        }

        if code == 404 {
            return TestDiagnostic {
                code: "endpoint_not_found".into(),
                title: "接口路径不存在".into(),
                hint: "检查 base_url 是否完整。Custom OpenAI 兼容接口要确认是否带 /v1 前缀。"
                    .into(),
                action_url: None,
                action_label: None,
                raw: raw_error.to_string(),
            };
        }

        // Generic 5xx upstream.
        if (500..600).contains(&code) {
            return TestDiagnostic {
                code: "upstream_error".into(),
                title: format!("Provider 服务端 {code} 错误"),
                hint: "上游异常，过几分钟重试。如果持续出现可能是当前 provider 在做维护。".into(),
                action_url: None,
                action_label: None,
                raw: raw_error.to_string(),
            };
        }
    }

    // ── Network-level failures (no HTTP response received) ──────────
    let raw_lower = raw_error.to_ascii_lowercase();
    if raw_lower.contains("dns")
        || raw_lower.contains("name resolution")
        || raw_lower.contains("failed to lookup")
        || raw_lower.contains("无法解析")
    {
        return TestDiagnostic {
            code: "dns_failed".into(),
            title: "域名解析失败".into(),
            hint: "检查 base_url 拼写是否正确；如果用代理，确认代理本身能解析这个域名。".into(),
            action_url: None,
            action_label: None,
            raw: raw_error.to_string(),
        };
    }

    if raw_lower.contains("timed out")
        || raw_lower.contains("timeout")
        || raw_lower.contains("超时")
    {
        return TestDiagnostic {
            code: "network_timeout".into(),
            title: "网络请求超时".into(),
            hint: "Provider 不可达或过慢。检查网络 / 代理；如果用境内访问境外 provider，可能需要科学上网。".into(),
            action_url: None,
            action_label: None,
            raw: raw_error.to_string(),
        };
    }

    if raw_lower.contains("connection refused")
        || raw_lower.contains("connect error")
        || raw_lower.contains("tcp connect")
    {
        return TestDiagnostic {
            code: "connection_refused".into(),
            title: "无法建立到 Provider 的连接".into(),
            hint: "对方端口不通。如果是本地 vLLM / Ollama，确认服务在监听；如果是云端 provider，检查代理或防火墙。".into(),
            action_url: None,
            action_label: None,
            raw: raw_error.to_string(),
        };
    }

    if raw_lower.contains("certificate") || raw_lower.contains("tls") || raw_lower.contains("ssl") {
        return TestDiagnostic {
            code: "tls_error".into(),
            title: "TLS / 证书校验失败".into(),
            hint: "Provider 证书有问题，或者你的代理在做 MITM。检查代理设置或换一条出网线路。"
                .into(),
            action_url: None,
            action_label: None,
            raw: raw_error.to_string(),
        };
    }

    // ── Fallback ────────────────────────────────────────────────────
    TestDiagnostic {
        code: "unknown".into(),
        title: "连接失败".into(),
        hint: "下面是原始错误，截图反馈给我们能加快定位。".into(),
        action_url: None,
        action_label: None,
        raw: raw_error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(pt: &str, status: Option<u16>, body: &str) -> TestDiagnostic {
        diagnose(pt, status, body, &format!("HTTP {status:?}: {body}"))
    }

    #[test]
    fn mimo_web_search_plugin_not_enabled() {
        let d = diag(
            "mimo",
            Some(400),
            r#"{"error":{"message":"webSearchEnabled is false"}}"#,
        );
        assert_eq!(d.code, "web_search_plugin_disabled");
        assert!(d.action_url.as_deref().unwrap().contains("xiaomimimo.com"));
    }

    #[test]
    fn invalid_api_key_routes_to_provider_console() {
        let d = diag("deepseek", Some(401), "Invalid API key");
        assert_eq!(d.code, "invalid_api_key");
        assert!(d.action_url.as_deref().unwrap().contains("deepseek"));
    }

    #[test]
    fn invalid_api_key_unknown_provider_has_no_url() {
        let d = diag("custom", Some(401), "Unauthorized");
        assert_eq!(d.code, "invalid_api_key");
        assert!(d.action_url.is_none());
    }

    #[test]
    fn insufficient_balance_chinese_marker() {
        let d = diag("mimo", Some(402), "账户余额不足，请充值");
        assert_eq!(d.code, "insufficient_balance");
        assert!(d.action_url.as_deref().unwrap().contains("xiaomimimo.com"));
    }

    #[test]
    fn rate_limited_classifies_429() {
        let d = diag("kimi", Some(429), "Too Many Requests");
        assert_eq!(d.code, "rate_limited");
    }

    #[test]
    fn model_not_found_400() {
        let d = diag(
            "deepseek",
            Some(400),
            r#"{"error":{"message":"Model deepseek-old does not exist"}}"#,
        );
        assert_eq!(d.code, "model_not_found");
    }

    #[test]
    fn model_not_supported_chinese() {
        let d = diag(
            "mimo",
            Some(400),
            r#"{"error":{"message":"模型不支持该请求"}}"#,
        );
        assert_eq!(d.code, "model_not_found");
    }

    #[test]
    fn region_blocked_403() {
        let d = diag(
            "openai",
            Some(403),
            "Country, region or territory not supported",
        );
        assert_eq!(d.code, "region_blocked");
    }

    #[test]
    fn plain_404_is_endpoint_not_found() {
        let d = diag("custom", Some(404), "Not Found");
        assert_eq!(d.code, "endpoint_not_found");
    }

    #[test]
    fn upstream_5xx() {
        let d = diag("openai", Some(503), "Service Unavailable");
        assert_eq!(d.code, "upstream_error");
        assert!(d.title.contains("503"));
    }

    #[test]
    fn dns_failed_no_status() {
        let d = diagnose(
            "mimo",
            None,
            "",
            "Connection error: failed to lookup address info: nodename nor servname provided",
        );
        assert_eq!(d.code, "dns_failed");
    }

    #[test]
    fn network_timeout_no_status() {
        let d = diagnose("mimo", None, "", "Connection error: operation timed out");
        assert_eq!(d.code, "network_timeout");
    }

    #[test]
    fn connection_refused_local_vllm() {
        let d = diagnose(
            "custom",
            None,
            "",
            "Connection error: tcp connect error: Connection refused",
        );
        assert_eq!(d.code, "connection_refused");
    }

    #[test]
    fn tls_error() {
        let d = diagnose(
            "openai",
            None,
            "",
            "Connection error: invalid peer certificate: UnknownIssuer",
        );
        assert_eq!(d.code, "tls_error");
    }

    #[test]
    fn unknown_falls_back() {
        let d = diagnose("custom", None, "", "something totally unexpected");
        assert_eq!(d.code, "unknown");
        assert!(d.raw.contains("totally unexpected"));
    }

    #[test]
    fn mimo_invalid_key_specifically_does_not_get_websearch_diagnostic() {
        // Guard against the obvious mis-classification: body says "false"
        // but it's an auth body, not a web_search body.
        let d = diag(
            "mimo",
            Some(401),
            r#"{"error":{"message":"invalid api key"}}"#,
        );
        assert_eq!(d.code, "invalid_api_key");
    }
}
