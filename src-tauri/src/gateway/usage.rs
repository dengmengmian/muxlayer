//! 从上游响应里提取 (input_tokens, output_tokens)。
//!
//! 三种上游协议的 usage 字段形态不同,这里收敛为单一来源,供各协议 handler 共用,
//! 避免在 routes.rs 里散落多份字段映射、改一处漏一处。

use serde_json::Value;

use crate::storage::pricing::{calculate_cost_with_cache, ModelPrice};

/// 一次请求的 token 计数。把原先 `log_request_success` 尾部 4 个相邻
/// `Option<i64>`(input/output/cache_write/cache_read)收敛成具名字段,
/// 消除"传参写反顺序"这一类隐患——4 个同型 `Option<i64>` 排在一起最易出错。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TokenUsage {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_write: Option<i64>,
    pub cache_read: Option<i64>,
    /// `input` 是否已包含 `cache_read`,由产出这组 usage 的上游协议决定。
    pub input_semantics: InputCacheSemantics,
}

/// 上游 usage 里 input 与缓存 token 的包含关系。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputCacheSemantics {
    /// OpenAI Chat / Responses(`prompt_tokens`/`input_tokens` 含 `cached_tokens`)、
    /// Gemini(`promptTokenCount` 含 `cachedContentTokenCount`)。
    /// 设为默认值:没有显式缓存价时计费结果与引入本字段前完全一致(整段 input 按原价)。
    #[default]
    IncludesCacheRead,
    /// Anthropic Messages:`input_tokens` 不含 `cache_read_input_tokens` /
    /// `cache_creation_input_tokens`,缓存 token 需另行计费。
    ExcludesCache,
}

impl TokenUsage {
    /// 按上游口径计算成本(USD)。`input` 原值不变,只影响 cost。
    ///
    /// - `ExcludesCache`(Anthropic):input + output + cache_read + cache_write 分别计费,
    ///   未配缓存价时按 Anthropic 默认倍率(读 ×0.1、写 ×1.25)。
    /// - `IncludesCacheRead`(OpenAI / Gemini):cached 已在 input 里,不能再加一遍。
    ///   只有该模型配了**显式** cache_read 价才拆成 (input − cache_read) 原价 +
    ///   cache_read 缓存价;没配就整段 input 按原价。原因:默认 0.1 倍率是 Anthropic 的,
    ///   OpenAI 的折扣是 0.5 / 0.25 / 0.1 不等,拿错倍率会低报 OpenAI 成本,宁可按原价高报。
    ///   这类协议没有 cache_write 概念,不计。
    pub fn cost(&self, price: &ModelPrice) -> f64 {
        match self.input_semantics {
            InputCacheSemantics::ExcludesCache => calculate_cost_with_cache(
                self.input,
                self.output,
                self.cache_read,
                self.cache_write,
                price,
            ),
            InputCacheSemantics::IncludesCacheRead => match (price.cache_read, self.cache_read) {
                (Some(_), Some(cache_read)) => {
                    let uncached = (self.input.unwrap_or(0) - cache_read).max(0);
                    calculate_cost_with_cache(
                        Some(uncached),
                        self.output,
                        Some(cache_read),
                        None,
                        price,
                    )
                }
                _ => calculate_cost_with_cache(self.input, self.output, None, None, price),
            },
        }
    }
}

/// 查价并计算成本;模型无价时返回 None。
pub fn cost_for_request(
    conn: &rusqlite::Connection,
    provider: &str,
    model: &str,
    usage: &TokenUsage,
) -> Option<f64> {
    let price = crate::storage::pricing::load_price_table(conn)
        .ok()?
        .lookup(provider, model)?;
    Some(usage.cost(&price))
}

/// 按一组候选 key 从 usage 对象里读第一个命中的 i64。
fn read_first(usage: Option<&Value>, keys: &[&str]) -> Option<i64> {
    let usage = usage?;
    keys.iter()
        .find_map(|k| usage.get(*k).and_then(|v| v.as_i64()))
}

/// OpenAI Chat / Responses 形态:`usage.{prompt_tokens|input_tokens}` /
/// `usage.{completion_tokens|output_tokens}`。兼容两套字段名。
pub fn extract_chat(upstream: &Value) -> (Option<i64>, Option<i64>) {
    let usage = upstream.get("usage");
    let input = read_first(usage, &["prompt_tokens", "input_tokens"]);
    let output = read_first(usage, &["completion_tokens", "output_tokens"]);
    (input, output)
}

/// Anthropic Messages 形态:`usage.input_tokens` / `usage.output_tokens`。
pub fn extract_anthropic(upstream: &Value) -> (Option<i64>, Option<i64>) {
    let usage = upstream.get("usage");
    let input = read_first(usage, &["input_tokens"]);
    let output = read_first(usage, &["output_tokens"]);
    (input, output)
}

/// Gemini 形态:`usageMetadata.promptTokenCount` / `usageMetadata.candidatesTokenCount`。
pub fn extract_gemini(upstream: &Value) -> (Option<i64>, Option<i64>) {
    let usage = upstream.get("usageMetadata");
    let input = read_first(usage, &["promptTokenCount"]);
    let output = read_first(usage, &["candidatesTokenCount"]);
    (input, output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_reads_prompt_completion_tokens() {
        let v = json!({"usage": {"prompt_tokens": 12, "completion_tokens": 34}});
        assert_eq!(extract_chat(&v), (Some(12), Some(34)));
    }

    #[test]
    fn chat_falls_back_to_input_output_tokens() {
        let v = json!({"usage": {"input_tokens": 5, "output_tokens": 7}});
        assert_eq!(extract_chat(&v), (Some(5), Some(7)));
    }

    #[test]
    fn anthropic_reads_input_output_tokens() {
        let v = json!({"usage": {"input_tokens": 100, "output_tokens": 200}});
        assert_eq!(extract_anthropic(&v), (Some(100), Some(200)));
    }

    #[test]
    fn gemini_reads_usage_metadata() {
        let v = json!({"usageMetadata": {"promptTokenCount": 9, "candidatesTokenCount": 11}});
        assert_eq!(extract_gemini(&v), (Some(9), Some(11)));
    }

    fn price(cache_read: Option<f64>, cache_write: Option<f64>) -> ModelPrice {
        ModelPrice {
            input: 2.0,
            output: 8.0,
            cache_read,
            cache_write,
        }
    }

    fn usage(semantics: InputCacheSemantics) -> TokenUsage {
        TokenUsage {
            input: Some(10_000),
            output: Some(100),
            cache_write: Some(2_000),
            cache_read: Some(8_000),
            input_semantics: semantics,
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn cost_excludes_cache_bills_cache_tokens_with_default_multipliers() {
        // 10000×2 + 100×8 + 8000×(2×0.1) + 2000×(2×1.25)
        let cost = usage(InputCacheSemantics::ExcludesCache).cost(&price(None, None));
        assert_close(cost, (20_000.0 + 800.0 + 1_600.0 + 5_000.0) / 1e6);
    }

    #[test]
    fn cost_excludes_cache_prefers_explicit_cache_prices() {
        // 10000×2 + 100×8 + 8000×0.3 + 2000×4
        let cost = usage(InputCacheSemantics::ExcludesCache).cost(&price(Some(0.3), Some(4.0)));
        assert_close(cost, (20_000.0 + 800.0 + 2_400.0 + 8_000.0) / 1e6);
    }

    #[test]
    fn cost_includes_cache_read_without_explicit_price_keeps_full_input_price() {
        // 无显式缓存价:cached 不打折,也不重复计;cache_write 不计。
        let cost = usage(InputCacheSemantics::IncludesCacheRead).cost(&price(None, None));
        assert_close(cost, (20_000.0 + 800.0) / 1e6);
    }

    #[test]
    fn cost_includes_cache_read_with_explicit_price_discounts_cached_part() {
        // (10000-8000)×2 + 8000×0.5 + 100×8;cache_write 不计。
        let cost = usage(InputCacheSemantics::IncludesCacheRead).cost(&price(Some(0.5), Some(9.0)));
        assert_close(cost, (4_000.0 + 4_000.0 + 800.0) / 1e6);
    }

    #[test]
    fn cost_includes_cache_read_floors_uncached_input_at_zero() {
        let u = TokenUsage {
            input: Some(100),
            output: None,
            cache_write: None,
            cache_read: Some(500),
            input_semantics: InputCacheSemantics::IncludesCacheRead,
        };
        assert_close(u.cost(&price(Some(0.5), None)), 500.0 * 0.5 / 1e6);
    }

    #[test]
    fn default_semantics_is_includes_cache_read() {
        assert_eq!(
            TokenUsage::default().input_semantics,
            InputCacheSemantics::IncludesCacheRead
        );
    }

    #[test]
    fn missing_usage_yields_none() {
        let v = json!({});
        assert_eq!(extract_chat(&v), (None, None));
        assert_eq!(extract_anthropic(&v), (None, None));
        assert_eq!(extract_gemini(&v), (None, None));
    }
}
