//! Unified redaction utility for sensitive values.

/// Redact a string that looks like an API key or token.
pub fn redact_value(val: &str) -> String {
    if val.len() <= 8 {
        return "*".repeat(val.len());
    }
    let prefix_len = if val.starts_with("ag_local_") {
        9
    } else if val.starts_with("sk-") {
        3
    } else {
        4
    };
    let suffix_len = 4;
    // 值可能混入多字节字符(如中文被误捕进 key 值),固定字节偏移会切在
    // 字符中间 panic。收敛到 char 边界,方向都朝"少暴露":前缀向下、后缀向上。
    let mut prefix_end = prefix_len.min(val.len());
    while !val.is_char_boundary(prefix_end) {
        prefix_end -= 1;
    }
    let mut suffix_start = val.len().saturating_sub(suffix_len);
    while !val.is_char_boundary(suffix_start) {
        suffix_start += 1;
    }
    let prefix = &val[..prefix_end];
    let suffix = &val[suffix_start..];
    format!("{prefix}••••••••{suffix}")
}

/// 精确脱敏的最短长度:过短的值精确替换会误伤普通文本。
const MIN_EXACT_SECRET_LEN: usize = 8;

/// 先把已知密钥(当前存储的 provider api_key、extra_headers 值等)按原值精确替换,
/// 再跑前缀 / 字段名启发式。启发式只认 `sk-` / `ag_local_` / Bearer / 固定字段名,
/// 没有前缀的 key(Google `AIza…`、Kimi、MiniMax、自定义鉴权 header)只能靠精确匹配。
pub fn redact_text_with_secrets(text: &str, secrets: &[String]) -> String {
    let mut exact: Vec<&str> = secrets
        .iter()
        .map(|s| s.trim())
        .filter(|s| s.len() >= MIN_EXACT_SECRET_LEN)
        .collect();
    // 长的先替换,避免短密钥是长密钥子串时把长密钥切碎后漏脱敏。
    exact.sort_by_key(|s| std::cmp::Reverse(s.len()));
    exact.dedup();
    let mut result = text.to_string();
    for secret in exact {
        if result.contains(secret) {
            result = result.replace(secret, &redact_value(secret));
        }
    }
    redact_text(&result)
}

/// 收集需要精确脱敏的 provider 密钥:api_key(单个或 JSON 数组)+ extra_headers
/// (JSON 对象)的所有字符串值。
pub fn provider_secrets(providers: &[crate::models::provider::Provider]) -> Vec<String> {
    let mut out = Vec::new();
    for p in providers {
        if let Some(raw) = p
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            match serde_json::from_str::<Vec<String>>(raw) {
                Ok(keys) if raw.starts_with('[') => out.extend(keys),
                _ => out.push(raw.to_string()),
            }
        }
        if let Some(headers) = p.extra_headers.as_deref().and_then(|h| {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(h).ok()
        }) {
            out.extend(
                headers
                    .values()
                    .filter_map(|v| v.as_str().map(str::to_string)),
            );
        }
    }
    out
}

/// Redact all sensitive patterns in a text block.
pub fn redact_text(text: &str) -> String {
    let mut result = text.to_string();

    // Redact ag_local_ tokens
    result = redact_pattern(&result, "ag_local_");
    // Redact sk- keys
    result = redact_pattern(&result, "sk-");
    // Redact Bearer tokens in headers
    result = redact_bearer(&result);
    // 命名键值:header 行 / JSON 字段两种形态(大小写不敏感)。
    // 注意顺序:x-api-key 先于 api-key,配合词边界避免后者重复命中前者的后缀。
    for name in [
        "x-goog-api-key",
        "x-api-key",
        "api-key",
        "api_key",
        "apikey",
        "access_token",
    ] {
        result = redact_named_value(&result, name);
    }
    // URL 查询参数 ?key= / &key=(Gemini 风格)。"key" 太泛,只在查询参数位置处理。
    result = redact_query_param(&result, "key");

    result
}

/// 大小写不敏感地查找 ASCII `needle`。命中位置必为 char 边界(ASCII 字节
/// 不会是多字节字符的延续字节),后续按字节切片安全。
fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < from + n.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// 脱敏 `name` 键的值,覆盖 `name: value`(header 行)和 `"name": "value"`
/// (JSON 字段)两种形态。要求:name 前是词边界;name 与值之间出现 `:` 或 `=`;
/// 值长度 >8 才脱敏(短值不像密钥)。
fn redact_named_value(text: &str, name: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;
    while let Some(start) = find_ci(text, name, pos) {
        let after_name = start + name.len();
        // 词边界:避免 "monkey" 命中 "key"、"x-api-key" 命中 "api-key"。
        if start > 0 && is_word_byte(text.as_bytes()[start - 1]) {
            result.push_str(&text[pos..after_name]);
            pos = after_name;
            continue;
        }
        result.push_str(&text[pos..after_name]);
        pos = after_name;

        let rest = &text[after_name..];
        let val_start = rest
            .find(|c: char| !matches!(c, '"' | '\'' | ':' | '=' | ' ' | '\t'))
            .unwrap_or(rest.len());
        let sep = &rest[..val_start];
        // 必须是键值对(有 : 或 =),否则只是普通文本里提到了这个词。
        if !(sep.contains(':') || sep.contains('=')) {
            continue;
        }
        let val_rest = &rest[val_start..];
        let val_end = val_rest
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '}' | ')' | '&'))
            .unwrap_or(val_rest.len());
        result.push_str(sep);
        if val_end > 8 {
            result.push_str(&redact_value(&val_rest[..val_end]));
        } else {
            result.push_str(&val_rest[..val_end]);
        }
        pos = after_name + val_start + val_end;
    }
    result.push_str(&text[pos..]);
    result
}

/// 脱敏 URL 查询参数 `?name=value` / `&name=value` 的值。
fn redact_query_param(text: &str, name: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;
    let bytes = text.as_bytes();
    while let Some(start) = find_ci(text, name, pos) {
        let after_name = start + name.len();
        let prev_is_qmark_or_amp = start > 0 && matches!(bytes[start - 1], b'?' | b'&');
        let next_is_eq = bytes.get(after_name) == Some(&b'=');
        result.push_str(&text[pos..after_name]);
        pos = after_name;
        if !(prev_is_qmark_or_amp && next_is_eq) {
            continue;
        }
        let val_rest = &text[after_name + 1..];
        let val_end = val_rest
            .find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\''))
            .unwrap_or(val_rest.len());
        result.push('=');
        if val_end > 8 {
            result.push_str(&redact_value(&val_rest[..val_end]));
        } else {
            result.push_str(&val_rest[..val_end]);
        }
        pos = after_name + 1 + val_end;
    }
    result.push_str(&text[pos..]);
    result
}

fn redact_pattern(text: &str, prefix: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;

    while let Some(start) = remaining.find(prefix) {
        result.push_str(&remaining[..start]);
        let after = &remaining[start..];
        // Find end of token (whitespace, quote, comma, brace, or end)
        let end = after
            .find(|c: char| {
                c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == '}' || c == ')'
            })
            .unwrap_or(after.len());
        if end > 8 {
            let token = &after[..end];
            result.push_str(&redact_value(token));
        } else {
            result.push_str(&after[..end]);
        }
        remaining = &after[end..];
    }
    result.push_str(remaining);
    result
}

fn redact_bearer(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    let pattern = "Bearer ";

    while let Some(start) = remaining.find(pattern) {
        result.push_str(&remaining[..start]);
        result.push_str("Bearer ");
        let after = &remaining[start + pattern.len()..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(after.len());
        if end > 4 {
            result.push_str(&redact_value(&after[..end]));
        } else {
            result.push_str(&after[..end]);
        }
        remaining = &after[end..];
    }
    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_short_value() {
        assert_eq!(redact_value("abc"), "***");
        assert_eq!(redact_value("abcdefgh"), "********");
    }

    #[test]
    fn test_redact_ag_local_token() {
        let token = "ag_local_abcdefghijklmnopqrstuvwxyz1234";
        let redacted = redact_value(token);
        assert!(redacted.starts_with("ag_local_"));
        assert!(redacted.contains("••••••••"));
        assert!(redacted.ends_with("1234"));
    }

    #[test]
    fn test_redact_value_multibyte_no_panic() {
        // 值里混入多字节字符时,len-4 / prefix_len 可能落在字符中间——
        // 之前按字节切片直接 panic(生产日志:redaction.rs:17 char boundary)。
        let redacted = redact_value("sk-abcdefgh中文测试。");
        assert!(redacted.starts_with("sk-"));
        assert!(redacted.contains("••••••••"));
        // 前后缀任意组合都不允许 panic
        for val in [
            "中文中文中文。",
            "sk-中文中文中文",
            "abcd中文本",
            "ag_local_中文中文中文",
        ] {
            let _ = redact_value(val);
        }
    }

    #[test]
    fn test_redact_sk_key() {
        let key = "sk-abcdefghijklmnopqrstuvwxyz1234567890";
        let redacted = redact_value(key);
        assert!(redacted.starts_with("sk-"));
        assert!(redacted.contains("••••••••"));
        assert!(redacted.ends_with("7890"));
    }

    #[test]
    fn test_redact_generic_long_value() {
        let val = "mysecretkey1234567890abcdef";
        let redacted = redact_value(val);
        assert_eq!(redacted, "myse••••••••cdef");
    }

    #[test]
    fn test_redact_text_ag_local() {
        let text = "token is ag_local_abc123xyz789 and more";
        let result = redact_text(text);
        assert!(!result.contains("ag_local_abc123xyz789"));
        assert!(result.contains("ag_local_"));
        assert!(result.contains("••••••••"));
    }

    #[test]
    fn test_redact_text_sk_key() {
        let text = "key: sk-live-12345abcdef, ok";
        let result = redact_text(text);
        assert!(!result.contains("sk-live-12345abcdef"));
        assert!(result.contains("sk-"));
    }

    #[test]
    fn test_redact_text_bearer() {
        let text = "Authorization: Bearer supersecrettoken12345";
        let result = redact_text(text);
        assert!(!result.contains("supersecrettoken12345"));
        assert!(result.contains("Bearer "));
        assert!(result.contains("••••••••"));
    }

    #[test]
    fn test_redact_text_multiple_tokens() {
        let text = "tokens: ag_local_abc123 and sk-xyz789";
        let result = redact_text(text);
        assert!(!result.contains("ag_local_abc123"));
        assert!(!result.contains("sk-xyz789"));
    }

    #[test]
    fn test_redact_text_no_match() {
        let text = "hello world, no secrets here";
        assert_eq!(redact_text(text), text);
    }

    // ── server 端脱敏加固:x-api-key / api_key 字段 / ?key= 查询参数 ──

    #[test]
    fn test_redact_x_api_key_header_line_and_json() {
        // header 行形态
        let t1 = redact_text("x-api-key: supersecretvalue123");
        assert!(!t1.contains("supersecretvalue123"));
        // JSON header map 形态(大小写不敏感)
        let t2 = redact_text(r#"{"X-Api-Key": "supersecretvalue123"}"#);
        assert!(!t2.contains("supersecretvalue123"));
    }

    #[test]
    fn test_redact_api_key_json_field() {
        let t = redact_text(r#"{"api_key": "longsecret1234567890", "model": "m"}"#);
        assert!(!t.contains("longsecret1234567890"));
        assert!(t.contains(r#""model": "m""#), "其他字段不受影响");
    }

    #[test]
    fn test_redact_gemini_query_key_param() {
        let t = redact_text("POST https://generativelanguage.googleapis.com/v1beta/models/g:streamGenerateContent?key=AIzaSyABCDEF1234567890&alt=sse");
        assert!(!t.contains("AIzaSyABCDEF1234567890"));
        assert!(t.contains("&alt=sse"), "后续参数保留");
    }

    #[test]
    fn test_named_value_word_boundary_no_false_positive() {
        // "monkey=..." 不能因为含 "key" 被误脱敏;短值也不动
        let text = "monkey=12345678901234 and donkey: abcdefghijkl";
        assert_eq!(redact_text(text), text);
    }

    #[test]
    fn exact_secrets_are_redacted_even_without_known_prefix() {
        let secrets = vec![
            "AIzaSyNoPrefixKey0123456789".to_string(),
            "short".to_string(),
            "kimi9f8e7d6c5b4a".to_string(),
        ];
        let text = "401 for AIzaSyNoPrefixKey0123456789, kimi9f8e7d6c5b4a; short word stays";
        assert!(
            redact_text(text).contains("AIzaSyNoPrefixKey0123456789"),
            "heuristics alone leak"
        );
        let out = redact_text_with_secrets(text, &secrets);
        assert!(!out.contains("AIzaSyNoPrefixKey0123456789"), "{out}");
        assert!(!out.contains("kimi9f8e7d6c5b4a"), "{out}");
        assert!(
            out.contains("short word stays"),
            "short values are not exact-redacted"
        );
    }

    #[test]
    fn provider_secrets_collects_key_arrays_and_header_values() {
        let mut p: crate::models::provider::Provider = serde_json::from_value(serde_json::json!({
            "id": "p", "name": "p", "provider_type": "custom", "base_url": "http://x",
            "api_key": "[\"k-one-12345678\",\"k-two-12345678\"]",
            "default_model": "m", "reasoning_model": null, "supported_models": null,
            "model_mapping": null, "extra_headers": "{\"X-Auth\":\"hdr-secret-12345\"}",
            "anthropic_base_url": null, "responses_base_url": null,
            "protocol": "openai_chat_completions", "timeout_seconds": 60, "status": "ok",
            "supports_vision": null, "auto_cache_control": null, "supports_cache": null,
            "model_capabilities": null, "provider_quirks": null, "body_filter_enabled": null,
            "thinking_rectifier_enabled": null, "error_mapper_enabled": null,
            "model_degradation_chain": null, "model_context_windows": null,
            "enabled": true, "is_active": false, "created_at": "", "updated_at": ""
        }))
        .unwrap();
        let secrets = provider_secrets(std::slice::from_ref(&p));
        assert!(secrets.contains(&"k-one-12345678".to_string()));
        assert!(secrets.contains(&"k-two-12345678".to_string()));
        assert!(secrets.contains(&"hdr-secret-12345".to_string()));
        p.api_key = Some("single-plain-key-123".to_string());
        assert!(provider_secrets(&[p]).contains(&"single-plain-key-123".to_string()));
    }

    #[test]
    fn test_redact_access_token_field() {
        let t = redact_text(r#"{"access_token": "tok_1234567890abcdef"}"#);
        assert!(!t.contains("tok_1234567890abcdef"));
    }
}
