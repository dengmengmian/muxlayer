use rusqlite::{params, Connection};
use serde::Serialize;

use crate::errors::AppError;
use crate::storage::generated_provider_catalog as catalog;

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ModelPricing {
    pub id: String,
    pub provider: String,
    pub model_pattern: String,
    pub input_price: f64,  // $/1M input tokens
    pub output_price: f64, // $/1M output tokens
    pub is_custom: bool,
    pub updated_at: String,
}

/// Ensure the model_pricing table has default entries. Returns how many default
/// rows were newly inserted (INSERT OR IGNORE, idempotent).
pub fn ensure_defaults(conn: &Connection) -> Result<usize, AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut inserted = 0;
    for (provider, model, input_price, output_price) in catalog::MODEL_PRICING_DEFAULTS {
        inserted += insert_default(conn, &now, provider, model, *input_price, *output_price)?;
    }
    Ok(inserted)
}

fn insert_default(
    conn: &Connection,
    now: &str,
    provider: &str,
    model: &str,
    input_price: f64,
    output_price: f64,
) -> Result<usize, AppError> {
    let id = format!("default_{provider}_{model}");
    let n = conn.execute(
        "INSERT OR IGNORE INTO model_pricing (id, provider, model_pattern, input_price, output_price, is_custom, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
        params![&id, provider, model, input_price, output_price, now],
    )?;
    Ok(n)
}

/// 未配置缓存价时的默认倍率(Anthropic 定价口径):
/// 读缓存 = input × 0.1,写缓存(5 分钟 TTL)= input × 1.25。
pub const DEFAULT_CACHE_READ_MULTIPLIER: f64 = 0.1;
pub const DEFAULT_CACHE_WRITE_MULTIPLIER: f64 = 1.25;

/// 一个模型的单价($/1M tokens)。cache_* 为 None 表示未配置,计费时按默认倍率推算。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

impl ModelPrice {
    pub fn cache_read_price(&self) -> f64 {
        self.cache_read
            .unwrap_or(self.input * DEFAULT_CACHE_READ_MULTIPLIER)
    }

    pub fn cache_write_price(&self) -> f64 {
        self.cache_write
            .unwrap_or(self.input * DEFAULT_CACHE_WRITE_MULTIPLIER)
    }
}

/// model_pricing 的一行(匹配用)。
#[derive(Debug, Clone)]
pub struct PriceRow {
    provider_lower: String,
    model_pattern: String,
    is_custom: bool,
    price: ModelPrice,
}

#[cfg(test)]
impl PriceRow {
    fn test(provider: &str, model: &str, is_custom: bool, input: f64, output: f64) -> Self {
        Self {
            provider_lower: provider.to_lowercase(),
            model_pattern: model.to_string(),
            is_custom,
            price: ModelPrice {
                input,
                output,
                cache_read: None,
                cache_write: None,
            },
        }
    }
}

/// 一次性加载的价格表,按与 `get_price` 相同的规则在内存里匹配,
/// 供批量场景(成本聚合、会话同步、回填)避免每行查库。
#[derive(Debug, Clone, Default)]
pub struct PriceTable {
    /// 已按 custom 优先排序:同一匹配条件下取第一条即"custom 优先"。
    rows: Vec<PriceRow>,
}

impl PriceTable {
    pub fn from_rows(mut rows: Vec<PriceRow>) -> Self {
        // 稳定排序:custom 在前,同类保持原顺序。
        rows.sort_by_key(|r| !r.is_custom);
        Self { rows }
    }

    fn find(&self, pred: impl Fn(&PriceRow) -> bool) -> Option<ModelPrice> {
        self.rows.iter().find(|r| pred(r)).map(|r| r.price)
    }

    /// Priority: exact (provider + model, custom first) → base-id match (qualifier
    /// stripped) → provider wildcard → model-only across providers → None.
    pub fn lookup(&self, provider: &str, model: &str) -> Option<ModelPrice> {
        let provider_lower = provider.to_lowercase();

        // 1. Exact match (custom first, case-insensitive on provider)
        if let Some(p) =
            self.find(|r| r.provider_lower == provider_lower && r.model_pattern == model)
        {
            return Some(p);
        }

        // 2. Base-id match after stripping a `[qualifier]` suffix.
        let base = strip_model_qualifier(model);
        if base != model {
            if let Some(p) =
                self.find(|r| r.provider_lower == provider_lower && r.model_pattern == base)
            {
                return Some(p);
            }
        }

        // 3. Wildcard match —— 本 provider 配的 '*' 通配是用户明确意图，
        // 优先于跨 provider 的同名 model 价。
        if let Some(p) = self.find(|r| r.provider_lower == provider_lower && r.model_pattern == "*")
        {
            return Some(p);
        }

        // 4. Model-only match across providers —— provider 实例名几乎从不等于 pricing
        // 的类型名（"anthropic_official" vs "anthropic"），但 model 名通常全局唯一。
        // 退而按 model 跨 provider 查价（custom 优先）。这是成本计算的主路径，
        // 纯 model 匹配。候选：原 model、去 qualifier 的 base，
        // 以及各自去掉 "vendor/" 前缀的形式（"z-ai/glm-5" → "glm-5"，应对 OpenRouter
        // 风格的带前缀 model id）。
        let mut model_candidates = vec![model, base];
        for c in [model, base] {
            if let Some((_, after)) = c.rsplit_once('/') {
                model_candidates.push(after);
            }
        }
        model_candidates
            .into_iter()
            .find_map(|m| self.find(|r| r.model_pattern == m))
    }
}

/// 加载整张价格表。兼容尚无 cache_*_price 列的旧表结构。
pub fn load_price_table(conn: &Connection) -> Result<PriceTable, AppError> {
    let has_cache_cols = conn
        .prepare("SELECT cache_read_price, cache_write_price FROM model_pricing LIMIT 0")
        .is_ok();
    let cache_cols = if has_cache_cols {
        "cache_read_price, cache_write_price"
    } else {
        "NULL, NULL"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT provider, model_pattern, input_price, output_price, is_custom, {cache_cols}
         FROM model_pricing ORDER BY rowid"
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok(PriceRow {
            provider_lower: r.get::<_, String>(0)?.to_lowercase(),
            model_pattern: r.get(1)?,
            is_custom: r.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
            price: ModelPrice {
                input: r.get(2)?,
                output: r.get(3)?,
                cache_read: r.get(5)?,
                cache_write: r.get(6)?,
            },
        })
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(PriceTable::from_rows(rows))
}

/// MiMo / DeepSeek 在 Claude Code 端点用 `[1m]` 之类的后缀请求长上下文,
/// 价格表按 base model id 存,匹配前要去掉后缀。
fn strip_model_qualifier(model: &str) -> &str {
    crate::providers::model_id::strip_qualifier(model)
}

/// Get the price for a specific provider + model. Matching rules: see `PriceTable::lookup`.
pub fn get_price(conn: &Connection, provider: &str, model: &str) -> Option<(f64, f64)> {
    // 签名保持 Option(网关热路径依赖);加载失败与"无价"同样返回 None,与旧实现一致。
    let table = load_price_table(conn).ok()?;
    table.lookup(provider, model).map(|p| (p.input, p.output))
}

/// Calculate cost in USD from token counts and prices.
pub fn calculate_cost(
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    input_price: f64,
    output_price: f64,
) -> f64 {
    calculate_cost_with_cache(
        input_tokens,
        output_tokens,
        None,
        None,
        &ModelPrice {
            input: input_price,
            output: output_price,
            cache_read: None,
            cache_write: None,
        },
    )
}

/// 含缓存 token 的成本(USD)。按 Anthropic 口径:`input_tokens` 不含缓存 token,
/// cache_read / cache_write 单独计费。OpenAI 口径(cached 已包含在 input 里)的调用方
/// 不应把 cached 再作为 cache_read 传入,否则会重复计费。
pub fn calculate_cost_with_cache(
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    price: &ModelPrice,
) -> f64 {
    let input = input_tokens.unwrap_or(0) as f64;
    let output = output_tokens.unwrap_or(0) as f64;
    let cache_read = cache_read_tokens.unwrap_or(0) as f64;
    let cache_write = cache_write_tokens.unwrap_or(0) as f64;
    (input * price.input
        + output * price.output
        + cache_read * price.cache_read_price()
        + cache_write * price.cache_write_price())
        / 1_000_000.0
}

/// Calculate cost for a request, looking up the price from the DB.
pub fn calculate_cost_for_request(
    conn: &Connection,
    provider: &str,
    model: &str,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
) -> Option<f64> {
    calculate_cost_for_request_with_cache(
        conn,
        provider,
        model,
        input_tokens,
        output_tokens,
        None,
        None,
    )
}

/// 同 `calculate_cost_for_request`,额外计入缓存 token(Anthropic 口径,见
/// `calculate_cost_with_cache`)。
pub fn calculate_cost_for_request_with_cache(
    conn: &Connection,
    provider: &str,
    model: &str,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
) -> Option<f64> {
    let price = load_price_table(conn).ok()?.lookup(provider, model)?;
    Some(calculate_cost_with_cache(
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        &price,
    ))
}

/// 回填每批提交的行数:单个写事务不宜过大,避免长时间持有写锁挡住网关写日志。
const BACKFILL_BATCH_SIZE: usize = 500;

/// Backfill cost for existing request_logs that have tokens but no cost.
/// 只处理 cost 为 NULL 且模型现在能匹配到价格的行,已有 cost 的行不动,重复执行无副作用。
/// 缓存 token 只对 Anthropic 口径的 claude_session 行计入(网关行无法区分上游协议口径)。
///
/// 实现:一次扫描收集需要回填的 (id, cost),再按主键分批(每批一个事务)UPDATE。
/// 中途失败时已提交批次保留,连接回到 autocommit,下次启动续跑。
/// 必须在迁移完成(source / cache_* 列已存在)后、且不在外层事务里调用——
/// 启动后由后台任务执行,不挡启动。
pub fn backfill_costs(conn: &Connection) -> Result<u64, AppError> {
    let table = load_price_table(conn)?;
    let mut pending: Vec<(String, f64)> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, provider, model, source, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens
             FROM request_logs
             WHERE cost IS NULL AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)
               AND provider IS NOT NULL AND model IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let provider: String = r.get(1)?;
            let model: String = r.get(2)?;
            let Some(price) = table.lookup(&provider, &model) else {
                continue;
            };
            let is_claude_session =
                r.get::<_, Option<String>>(3)?.as_deref() == Some("claude_session");
            let (cache_read, cache_write) = if is_claude_session {
                (r.get(6)?, r.get(7)?)
            } else {
                (None, None)
            };
            let cost =
                calculate_cost_with_cache(r.get(4)?, r.get(5)?, cache_read, cache_write, &price);
            pending.push((r.get(0)?, cost));
        }
    }

    let mut updated = 0u64;
    for chunk in pending.chunks(BACKFILL_BATCH_SIZE) {
        // Transaction 在 commit 失败或提前返回时 Drop 会 ROLLBACK,连接不会遗留写事务。
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare("UPDATE request_logs SET cost = ?1 WHERE id = ?2 AND cost IS NULL")?;
            for (id, cost) in chunk {
                updated += stmt.execute(params![cost, id])? as u64;
            }
        }
        tx.commit()?;
        // 后台回填发生在启动之后,今日花费缓存可能已被读过,改了 cost 必须失效。
        crate::storage::request_logs::invalidate_cost_caches();
    }
    Ok(updated)
}

/// List all pricing entries (default + custom).
pub fn list_all(conn: &Connection) -> Result<Vec<ModelPricing>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, model_pattern, input_price, output_price, is_custom, updated_at
         FROM model_pricing ORDER BY provider, model_pattern",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ModelPricing {
            id: row.get(0)?,
            provider: row.get(1)?,
            model_pattern: row.get(2)?,
            input_price: row.get(3)?,
            output_price: row.get(4)?,
            is_custom: row.get::<_, i64>(5)? != 0,
            updated_at: row.get(6)?,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Add or update a custom pricing entry.
pub fn upsert_custom(
    conn: &Connection,
    provider: &str,
    model_pattern: &str,
    input_price: f64,
    output_price: f64,
) -> Result<ModelPricing, AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    let id = format!("custom_{provider}_{model_pattern}");

    conn.execute(
        "INSERT INTO model_pricing (id, provider, model_pattern, input_price, output_price, is_custom, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)
         ON CONFLICT(id) DO UPDATE SET input_price=?4, output_price=?5, updated_at=?6",
        params![&id, provider, model_pattern, input_price, output_price, &now],
    )?;

    Ok(ModelPricing {
        id,
        provider: provider.to_string(),
        model_pattern: model_pattern.to_string(),
        input_price,
        output_price,
        is_custom: true,
        updated_at: now,
    })
}

/// Delete a custom pricing entry. Cannot delete built-in defaults.
pub fn delete_custom(conn: &Connection, id: &str) -> Result<bool, AppError> {
    let rows = conn.execute(
        "DELETE FROM model_pricing WHERE id = ?1 AND is_custom = 1",
        [id],
    )?;
    Ok(rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_pricing (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                model_pattern TEXT NOT NULL,
                input_price REAL NOT NULL,
                output_price REAL NOT NULL,
                is_custom INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_ensure_defaults() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        let all = list_all(&conn).unwrap();
        assert!(all.len() >= 10);
        assert!(all.iter().all(|p| !p.is_custom));
    }

    #[test]
    fn test_get_price_exact() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        let price = get_price(&conn, "deepseek", "deepseek-v4-pro");
        assert!(price.is_some());
        let (inp, out) = price.unwrap();
        assert!((inp - 2.0).abs() < 0.01);
        assert!((out - 8.0).abs() < 0.01);
    }

    #[test]
    fn model_only_fallback_when_provider_name_mismatches() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        // provider 实例名（"anthropic_official"）和 pricing 的类型名（"deepseek"）对不上，
        // 但 model 名全局唯一——应靠 model-only 兜底命中。这是真实主路径：provider
        // 实例名几乎从不等于 pricing 的类型名，否则全部成本算成 0。
        let price = get_price(&conn, "anthropic_official", "deepseek-v4-pro");
        assert!(price.is_some(), "应靠 model-only 兜底命中价格");
        assert!((price.unwrap().0 - 2.0).abs() < 0.01);

        // 带 qualifier 的也要能兜底到 base。
        let q = get_price(&conn, "some_proxy", "deepseek-v4-pro[1m]");
        assert!(q.is_some(), "qualifier 去除后应靠 model-only 命中");
    }

    #[test]
    fn model_only_strips_vendor_prefix() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        // OpenRouter 风格 "vendor/model" id（如 "z-ai/glm-5"）应去掉前缀后匹配到 glm-5。
        let p = get_price(&conn, "some_proxy", "z-ai/glm-5");
        assert!(p.is_some(), "应去 vendor 前缀后命中 glm-5");
        // 不硬编码具体价格(catalog 调价会让断言变脆),改为对齐直接查 glm-5 的结果,
        // 验证"前缀被剥离后命中同一条目"这一真实意图。
        let direct = get_price(&conn, "some_proxy", "glm-5");
        assert_eq!(p, direct, "去前缀后应命中与 glm-5 相同的价格条目");
    }

    #[test]
    fn mimo_defaults_present() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        let (inp, out) = get_price(&conn, "mimo", "mimo-v2.5-pro").expect("v2.5-pro priced");
        assert!((inp - 0.435).abs() < 1e-4);
        assert!((out - 0.87).abs() < 1e-4);

        let (inp, out) = get_price(&conn, "mimo", "mimo-v2-flash").expect("flash priced");
        assert!((inp - 0.10).abs() < 1e-4);
        assert!((out - 0.30).abs() < 1e-4);

        // TTS family is free
        let (inp, out) = get_price(&conn, "mimo", "mimo-v2.5-tts").expect("tts priced");
        assert_eq!(inp, 0.0);
        assert_eq!(out, 0.0);
    }

    #[test]
    fn qualifier_suffix_strips_to_base_id() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        // [1m] suffix used by Claude Code path on MiMo / DeepSeek must
        // still match the base-id price entry.
        let mimo = get_price(&conn, "mimo", "mimo-v2.5-pro[1m]")
            .expect("mimo-v2.5-pro[1m] should resolve to mimo-v2.5-pro");
        let ds = get_price(&conn, "deepseek", "deepseek-v4-pro[1m]")
            .expect("deepseek-v4-pro[1m] should resolve to deepseek-v4-pro");
        assert!((mimo.0 - 0.435).abs() < 1e-4);
        assert!((ds.0 - 2.0).abs() < 1e-4);
    }

    #[test]
    fn unknown_qualifier_falls_through_to_base() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        // Any qualifier is stripped before matching; bogus qualifier still resolves.
        let price = get_price(&conn, "mimo", "mimo-v2.5-pro[128k]");
        assert!(price.is_some());
    }

    #[test]
    fn test_get_price_wildcard() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        let price = get_price(&conn, "groq", "llama-3.3-70b");
        assert!(price.is_some());
        let (inp, out) = price.unwrap();
        assert!((inp - 0.0).abs() < 0.01);
        assert!((out - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_get_price_unknown() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        assert!(get_price(&conn, "unknown_provider", "unknown_model").is_none());
    }

    #[test]
    fn test_calculate_cost() {
        // 1000 input tokens at $2/1M + 500 output tokens at $8/1M
        let cost = calculate_cost(Some(1000), Some(500), 2.0, 8.0);
        assert!((cost - 0.006).abs() < 0.0001);
    }

    #[test]
    fn test_custom_overrides_default() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        upsert_custom(&conn, "deepseek", "deepseek-v4-pro", 99.0, 99.0).unwrap();
        let price = get_price(&conn, "deepseek", "deepseek-v4-pro").unwrap();
        assert!((price.0 - 99.0).abs() < 0.01);
    }

    #[test]
    fn test_delete_custom() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        upsert_custom(&conn, "test", "model", 1.0, 2.0).unwrap();
        assert!(delete_custom(&conn, "custom_test_model").unwrap());
        assert!(!delete_custom(&conn, "default_deepseek_deepseek-v4-pro").unwrap());
        // can't delete defaults
    }

    #[test]
    fn calculate_cost_with_cache_uses_anthropic_style_defaults() {
        // 未知缓存价:cache_read = input × 0.1,cache_write = input × 1.25。
        let price = ModelPrice {
            input: 3.0,
            output: 15.0,
            cache_read: None,
            cache_write: None,
        };
        let cost =
            calculate_cost_with_cache(Some(1000), Some(500), Some(10_000), Some(2000), &price);
        // (1000×3 + 500×15 + 10000×0.3 + 2000×3.75) / 1e6
        assert!((cost - 0.021).abs() < 1e-12, "cost={cost}");
    }

    #[test]
    fn calculate_cost_with_cache_prefers_explicit_cache_prices() {
        let price = ModelPrice {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.5),
            cache_write: Some(6.0),
        };
        let cost = calculate_cost_with_cache(None, None, Some(1_000_000), Some(1_000_000), &price);
        assert!((cost - 6.5).abs() < 1e-12, "cost={cost}");
    }

    #[test]
    fn calculate_cost_without_cache_tokens_is_unchanged() {
        let price = ModelPrice {
            input: 2.0,
            output: 8.0,
            cache_read: None,
            cache_write: None,
        };
        let a = calculate_cost(Some(1000), Some(500), 2.0, 8.0);
        let b = calculate_cost_with_cache(Some(1000), Some(500), None, None, &price);
        assert!((a - b).abs() < 1e-15);
        assert!((a - 0.006).abs() < 1e-12);
    }

    #[test]
    fn calculate_cost_for_request_with_cache_reads_cache_columns() {
        let conn = setup_db();
        conn.execute_batch(
            "ALTER TABLE model_pricing ADD COLUMN cache_read_price REAL;
             ALTER TABLE model_pricing ADD COLUMN cache_write_price REAL;
             INSERT INTO model_pricing (id, provider, model_pattern, input_price, output_price, is_custom, updated_at, cache_read_price, cache_write_price)
             VALUES ('x', 'anthropic', 'claude-test', 3.0, 15.0, 0, '', 0.3, NULL);",
        )
        .unwrap();
        let cost = calculate_cost_for_request_with_cache(
            &conn,
            "anthropic_official",
            "claude-test",
            Some(0),
            Some(0),
            Some(1_000_000),
            Some(1_000_000),
        )
        .unwrap();
        // read 显式 0.3;write 缺省 3.0 × 1.25 = 3.75
        assert!((cost - 4.05).abs() < 1e-12, "cost={cost}");
    }

    #[test]
    fn price_table_lookup_follows_priority_rules() {
        // 规则与原 SQL 版 get_price 一致:
        // provider 精确(custom 优先) → 去 qualifier → provider 通配 → 跨 provider 按 model。
        let rows = vec![
            PriceRow::test("deepseek", "deepseek-v4-pro", false, 2.0, 8.0),
            PriceRow::test("DeepSeek", "deepseek-v4-pro", true, 9.0, 9.0),
            PriceRow::test("groq", "*", false, 0.0, 0.0),
            PriceRow::test("other", "llama-3.3-70b", false, 5.0, 5.0),
            PriceRow::test("zhipu", "glm-5", false, 1.0, 3.0),
            PriceRow::test("mimo", "mimo-v2.5-pro", false, 0.435, 0.87),
        ];
        let table = PriceTable::from_rows(rows);
        let p = |prov: &str, model: &str| table.lookup(prov, model).map(|m| (m.input, m.output));
        assert_eq!(
            p("deepseek", "deepseek-v4-pro"),
            Some((9.0, 9.0)),
            "custom 优先,provider 大小写不敏感"
        );
        assert_eq!(
            p("mimo", "mimo-v2.5-pro[1m]"),
            Some((0.435, 0.87)),
            "去 qualifier"
        );
        assert_eq!(
            p("groq", "llama-3.3-70b"),
            Some((0.0, 0.0)),
            "本 provider 通配优先于跨 provider 同名"
        );
        assert_eq!(
            p("proxy", "llama-3.3-70b"),
            Some((5.0, 5.0)),
            "跨 provider 按 model"
        );
        assert_eq!(p("proxy", "z-ai/glm-5"), Some((1.0, 3.0)), "去 vendor 前缀");
        assert_eq!(
            p("proxy", "deepseek-v4-pro[1m]"),
            Some((9.0, 9.0)),
            "model-only 也 custom 优先"
        );
        assert_eq!(p("proxy", "unknown"), None);
    }

    #[test]
    fn get_price_matches_price_table_lookup_on_defaults() {
        let conn = setup_db();
        ensure_defaults(&conn).unwrap();
        upsert_custom(&conn, "deepseek", "deepseek-v4-pro", 99.0, 99.0).unwrap();
        let table = load_price_table(&conn).unwrap();
        for (prov, model) in [
            ("deepseek", "deepseek-v4-pro"),
            ("anthropic_official", "deepseek-v4-pro"),
            ("some_proxy", "z-ai/glm-5"),
            ("mimo", "mimo-v2.5-pro[128k]"),
            ("groq", "llama-3.3-70b"),
            ("unknown_provider", "unknown_model"),
        ] {
            assert_eq!(
                get_price(&conn, prov, model),
                table.lookup(prov, model).map(|m| (m.input, m.output)),
                "{prov}/{model}"
            );
        }
    }

    #[test]
    fn ensure_defaults_reports_inserted_rows() {
        let conn = setup_db();
        let first = ensure_defaults(&conn).unwrap();
        assert!(first > 0);
        assert_eq!(ensure_defaults(&conn).unwrap(), 0, "幂等:第二次不插入");
    }

    #[test]
    fn backfill_costs_only_fills_null_cost_rows_with_known_price() {
        let conn = setup_db();
        conn.execute_batch(
            "CREATE TABLE request_logs (id TEXT PRIMARY KEY, provider TEXT, model TEXT, source TEXT,
                input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
                cache_write_tokens INTEGER, cost REAL);
             INSERT INTO model_pricing (id, provider, model_pattern, input_price, output_price, is_custom, updated_at)
             VALUES ('p', 'x', 'priced', 2.0, 8.0, 0, '');
             INSERT INTO request_logs VALUES ('a', 'gw', 'priced', 'gateway', 1000, 500, NULL, NULL, NULL);
             INSERT INTO request_logs VALUES ('b', 'gw', 'priced', 'gateway', 1000, 500, NULL, NULL, 42.0);
             INSERT INTO request_logs VALUES ('c', 'gw', 'unpriced', 'gateway', 1000, 500, NULL, NULL, NULL);
             INSERT INTO request_logs VALUES ('d', 'gw', 'priced', 'gateway', NULL, NULL, NULL, NULL, NULL);",
        )
        .unwrap();
        let updated = backfill_costs(&conn).unwrap();
        assert_eq!(updated, 1);
        let cost = |id: &str| -> Option<f64> {
            conn.query_row("SELECT cost FROM request_logs WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert!((cost("a").unwrap() - 0.006).abs() < 1e-12);
        assert_eq!(cost("b"), Some(42.0));
        assert_eq!(cost("c"), None);
        assert_eq!(cost("d"), None);
        assert_eq!(backfill_costs(&conn).unwrap(), 0, "幂等");
    }

    fn backfill_db() -> Connection {
        let conn = setup_db();
        conn.execute_batch(
            "ALTER TABLE model_pricing ADD COLUMN cache_read_price REAL;
             ALTER TABLE model_pricing ADD COLUMN cache_write_price REAL;
             CREATE TABLE request_logs (id TEXT PRIMARY KEY, provider TEXT, model TEXT, source TEXT,
                input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
                cache_write_tokens INTEGER, cost REAL);
             INSERT INTO model_pricing (id, provider, model_pattern, input_price, output_price, is_custom, updated_at)
             VALUES ('p', 'x', 'priced', 2.0, 8.0, 0, '');",
        )
        .unwrap();
        conn
    }

    fn null_cost_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE cost IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// 旧实现(按 provider/model 分组整表 UPDATE)的 SQL 口径,用来比对新实现结果。
    fn legacy_sql_backfill(conn: &Connection) {
        let table = load_price_table(conn).unwrap();
        let pairs: Vec<(String, String)> = conn
            .prepare(
                "SELECT DISTINCT provider, model FROM request_logs
                 WHERE cost IS NULL AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)
                   AND provider IS NOT NULL AND model IS NOT NULL",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (provider, model) in pairs {
            let Some(price) = table.lookup(&provider, &model) else {
                continue;
            };
            conn.execute(
                "UPDATE request_logs
                    SET cost = (COALESCE(input_tokens, 0) * ?1
                              + COALESCE(output_tokens, 0) * ?2
                              + CASE WHEN source = 'claude_session'
                                     THEN COALESCE(cache_read_tokens, 0) * ?3
                                        + COALESCE(cache_write_tokens, 0) * ?4
                                     ELSE 0 END) / 1000000.0
                  WHERE cost IS NULL AND provider = ?5 AND model = ?6
                    AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)",
                params![
                    price.input,
                    price.output,
                    price.cache_read_price(),
                    price.cache_write_price(),
                    &provider,
                    &model
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn backfill_costs_matches_previous_sql_semantics() {
        let seed = "INSERT INTO model_pricing (id, provider, model_pattern, input_price, output_price, is_custom, updated_at, cache_read_price, cache_write_price)
                    VALUES ('c', 'anthropic', 'claude-x', 3.0, 15.0, 1, '', 0.3, NULL);
                    INSERT INTO request_logs VALUES ('g1', 'gw', 'priced', 'gateway', 1234, 567, 900, 80, NULL);
                    INSERT INTO request_logs VALUES ('g2', 'gw', 'priced', 'gateway', NULL, 10, NULL, NULL, NULL);
                    INSERT INTO request_logs VALUES ('c1', 'anthropic_official', 'claude-x', 'claude_session', 1000, 500, 10000, 2000, NULL);
                    INSERT INTO request_logs VALUES ('c2', 'anthropic_official', 'claude-x', 'claude_session', 7, NULL, NULL, 33, NULL);
                    INSERT INTO request_logs VALUES ('c3', 'anthropic_official', 'claude-x', 'claude_session', 1, 1, 1, 1, 9.5);
                    INSERT INTO request_logs VALUES ('x1', 'gw', 'priced[1m]', 'codex_session', 50, 60, 70, NULL, NULL);
                    INSERT INTO request_logs VALUES ('n1', NULL, 'priced', 'gateway', 50, 60, NULL, NULL, NULL);
                    INSERT INTO request_logs VALUES ('n2', 'gw', NULL, 'gateway', 50, 60, NULL, NULL, NULL);
                    INSERT INTO request_logs VALUES ('u1', 'gw', 'unpriced', 'gateway', 50, 60, NULL, NULL, NULL);";
        let legacy = backfill_db();
        legacy.execute_batch(seed).unwrap();
        legacy_sql_backfill(&legacy);
        let current = backfill_db();
        current.execute_batch(seed).unwrap();
        let updated = backfill_costs(&current).unwrap();
        assert_eq!(updated, 5);

        let dump = |c: &Connection| -> Vec<(String, Option<f64>)> {
            c.prepare("SELECT id, cost FROM request_logs ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let (a, b) = (dump(&legacy), dump(&current));
        assert_eq!(a.len(), b.len());
        for ((id_a, cost_a), (id_b, cost_b)) in a.iter().zip(b.iter()) {
            assert_eq!(id_a, id_b);
            match (cost_a, cost_b) {
                (Some(x), Some(y)) => assert!(
                    (x - y).abs() <= 1e-15 * x.abs().max(1.0),
                    "{id_a}: {x} vs {y}"
                ),
                (None, None) => {}
                _ => panic!("{id_a}: {cost_a:?} vs {cost_b:?}"),
            }
        }
    }

    #[test]
    fn backfill_costs_commits_in_batches_and_keeps_progress_on_failure() {
        // 大库回填分批提交:中途失败时已提交批次保留,连接回到 autocommit,下次启动续跑。
        let conn = backfill_db();
        {
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..1201 {
                tx.execute(
                    "INSERT INTO request_logs VALUES (?1, 'gw', 'priced', 'gateway', 1000, 500, NULL, NULL, NULL)",
                    [format!("r{i:04}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        conn.execute_batch(
            "CREATE TRIGGER fail_one BEFORE UPDATE OF cost ON request_logs
             WHEN NEW.id = 'r0700'
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();

        assert!(backfill_costs(&conn).is_err());
        assert!(conn.is_autocommit(), "失败后不能遗留未结束的事务");
        assert_eq!(null_cost_count(&conn), 701, "第一批 500 行应已提交");

        conn.execute_batch("DROP TRIGGER fail_one;").unwrap();
        assert_eq!(backfill_costs(&conn).unwrap(), 701, "重试续跑剩余行");
        assert_eq!(null_cost_count(&conn), 0);
        assert_eq!(backfill_costs(&conn).unwrap(), 0, "幂等");
    }

    #[test]
    fn backfill_costs_rolls_back_when_commit_fails() {
        // 提交阶段失败(此处用延迟外键在 COMMIT 时触发)也必须回滚,
        // 否则池化连接一直挂着写事务、持有写锁。
        let conn = backfill_db();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE parent (id TEXT PRIMARY KEY);
             CREATE TABLE child (pid TEXT REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TRIGGER orphan AFTER UPDATE OF cost ON request_logs
             BEGIN INSERT INTO child VALUES ('missing'); END;
             INSERT INTO request_logs VALUES ('a', 'gw', 'priced', 'gateway', 1000, 500, NULL, NULL, NULL);",
        )
        .unwrap();

        assert!(backfill_costs(&conn).is_err());
        assert!(conn.is_autocommit(), "提交失败后连接必须回到 autocommit");
        assert_eq!(null_cost_count(&conn), 1);
        conn.execute("INSERT INTO parent VALUES ('p')", [])
            .expect("后续写入应成功");
    }
}
