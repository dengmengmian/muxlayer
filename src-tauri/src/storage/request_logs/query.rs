//! 查询：列表 / 计数 / 详情 / 模型下拉，以及共享的过滤条件构造。

use rusqlite::Connection;

use crate::errors::AppError;
use crate::models::request_log::{RequestLogDetail, RequestLogFilter, RequestLogListItem};

/// Count rows matching the filter — used by the Logs page to compute the
/// total page count without fetching all rows. Shares the same WHERE-clause
/// construction as `list()` to guarantee identical filtering semantics.
pub fn count(conn: &Connection, filter: &RequestLogFilter) -> Result<i64, AppError> {
    let mut sql = String::from("SELECT COUNT(*) FROM request_logs WHERE 1=1");
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut idx = 1;

    apply_log_filter(filter, &mut sql, &mut param_values, &mut idx);

    let params: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(|b| &**b as &dyn rusqlite::types::ToSql)
        .collect();
    let total: i64 = conn
        .query_row(&sql, rusqlite::params_from_iter(params.iter()), |r| {
            r.get(0)
        })
        .map_err(AppError::from)?;
    Ok(total)
}

pub fn list(
    conn: &Connection,
    filter: RequestLogFilter,
) -> Result<Vec<RequestLogListItem>, AppError> {
    let mut sql = String::from(
        "SELECT id, request_id, timestamp, client, provider, model, route, status_code, latency_ms, error_message, source, session_id
         FROM request_logs WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut idx = 1;

    apply_log_filter(&filter, &mut sql, &mut param_values, &mut idx);

    sql.push_str(" ORDER BY timestamp DESC");

    let limit = filter.limit.unwrap_or(100);
    let offset = filter.offset.unwrap_or(0);
    sql.push_str(&format!(" LIMIT ?{idx} OFFSET ?{}", idx + 1));
    param_values.push(Box::new(limit));
    param_values.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(params_ref.as_slice(), |row| {
        Ok(RequestLogListItem {
            id: row.get(0)?,
            request_id: row.get(1)?,
            timestamp: row.get(2)?,
            client: row.get(3)?,
            provider: row.get(4)?,
            model: row.get(5)?,
            route: row.get(6)?,
            status_code: row.get(7)?,
            latency_ms: row.get(8)?,
            error_message: row.get(9)?,
            source: row.get(10)?,
            session_id: row.get(11)?,
        })
    })?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}

/// 日志里出现过的去重模型名，给 Logs 页的「模型」筛选下拉用。
pub fn distinct_models(conn: &Connection) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT model FROM request_logs
         WHERE model IS NOT NULL AND model != '' AND model != '<synthetic>'
         ORDER BY model",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 把 RequestLogFilter 的过滤条件转 WHERE 子句。count / list / aggregate_by_session
/// 共享，保证过滤语义一致。
pub(super) fn apply_log_filter(
    filter: &RequestLogFilter,
    sql: &mut String,
    param_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    idx: &mut usize,
) {
    if let Some(ref client) = filter.client {
        sql.push_str(&format!(" AND client = ?{idx}"));
        param_values.push(Box::new(client.clone()));
        *idx += 1;
    }
    if let Some(ref provider) = filter.provider {
        sql.push_str(&format!(" AND provider = ?{idx}"));
        param_values.push(Box::new(provider.clone()));
        *idx += 1;
    }
    if let Some(ref model) = filter.model {
        sql.push_str(&format!(" AND model = ?{idx}"));
        param_values.push(Box::new(model.clone()));
        *idx += 1;
    }
    if let Some(ref route_profile_id) = filter.route_profile_id {
        sql.push_str(&format!(
            // 写入路径与 v11 回填都已填好 route_profile_id,直接等值匹配;
            // 显式带上 `!= ''` 让查询条件蕴含部分索引的 WHERE,才能用上该索引。
            " AND route_profile_id = ?{idx} AND route_profile_id != ''"
        ));
        param_values.push(Box::new(route_profile_id.clone()));
        *idx += 1;
    }
    if let Some(ref source) = filter.source {
        // 'session_log' = 所有非 gateway 来源的合集（聚合视图用）。
        if source == "session_log" {
            sql.push_str(" AND source IS NOT NULL AND source != 'gateway'");
        } else {
            sql.push_str(&format!(" AND source = ?{idx}"));
            param_values.push(Box::new(source.clone()));
            *idx += 1;
        }
    }
    if let Some(ref sid) = filter.session_id {
        sql.push_str(&format!(" AND session_id = ?{idx}"));
        param_values.push(Box::new(sid.clone()));
        *idx += 1;
    }
    if let Some(ref status) = filter.status {
        match status.as_str() {
            "success" => {
                sql.push_str(&format!(
                    " AND status_code >= ?{} AND status_code < ?{}",
                    idx,
                    *idx + 1
                ));
                param_values.push(Box::new(200i64));
                param_values.push(Box::new(300i64));
                *idx += 2;
            }
            "error" => {
                sql.push_str(&format!(
                    " AND (status_code >= ?{} OR status_code < ?{})",
                    idx,
                    *idx + 1
                ));
                param_values.push(Box::new(400i64));
                param_values.push(Box::new(200i64));
                *idx += 2;
            }
            _ => {}
        }
    }
    if let Some(ref error_type) = filter.error_type {
        apply_error_type_filter(error_type, sql);
    }
    if let Some(ref keyword) = filter.keyword {
        // 转义 LIKE 通配符,让用户输入的 `%` / `_` 按字面匹配(如搜 "100%")。
        let like = format!("%{}%", escape_like(keyword));
        sql.push_str(&format!(
            " AND (request_id LIKE ?{idx} ESCAPE '\\' OR error_message LIKE ?{} ESCAPE '\\' OR model LIKE ?{} ESCAPE '\\' OR route LIKE ?{} ESCAPE '\\')",
            *idx + 1, *idx + 2, *idx + 3
        ));
        param_values.push(Box::new(like.clone()));
        param_values.push(Box::new(like.clone()));
        param_values.push(Box::new(like.clone()));
        param_values.push(Box::new(like));
        *idx += 4;
    }
}

fn escape_like(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn apply_error_type_filter(error_type: &str, sql: &mut String) {
    match error_type {
        "auth_failed" => {
            sql.push_str(
                " AND (status_code IN (401, 403)
                    OR lower(COALESCE(error_message, '')) LIKE '%unauthorized%'
                    OR lower(COALESCE(error_message, '')) LIKE '%authentication%'
                    OR lower(COALESCE(error_message, '')) LIKE '%invalid api key%'
                    OR lower(COALESCE(error_message, '')) LIKE '%invalid_api_key%')",
            );
        }
        "rate_limited" => {
            sql.push_str(
                " AND (status_code = 429
                    OR lower(COALESCE(error_message, '')) LIKE '%rate limit%'
                    OR lower(COALESCE(error_message, '')) LIKE '%rate_limit%')",
            );
        }
        "quota_or_balance" => {
            sql.push_str(
                " AND (status_code = 402
                    OR lower(COALESCE(error_message, '')) LIKE '%quota%'
                    OR lower(COALESCE(error_message, '')) LIKE '%balance%'
                    OR lower(COALESCE(error_message, '')) LIKE '%credit%')",
            );
        }
        "server_error" => {
            sql.push_str(" AND status_code >= 500");
        }
        "network_error" => {
            // 网络层失败（连接拒绝、超时、DNS、发请求失败）——通常没有 HTTP 状态码。
            sql.push_str(
                " AND (lower(COALESCE(error_message, '')) LIKE '%timeout%'
                    OR lower(COALESCE(error_message, '')) LIKE '%timed out%'
                    OR lower(COALESCE(error_message, '')) LIKE '%connection%'
                    OR lower(COALESCE(error_message, '')) LIKE '%error sending request%'
                    OR lower(COALESCE(error_message, '')) LIKE '%network%'
                    OR lower(COALESCE(error_message, '')) LIKE '%dns%')",
            );
        }
        "protocol_error" => {
            // 协议转换 / 解析失败（AgentGate 转换层或上游响应结构异常）。best-effort：
            // 靠 error_message 文本匹配，AgentGate 未把错误类型单独存为字段。
            sql.push_str(
                " AND (lower(COALESCE(error_message, '')) LIKE '%parse%'
                    OR lower(COALESCE(error_message, '')) LIKE '%convert%'
                    OR lower(COALESCE(error_message, '')) LIKE '%conversion%'
                    OR lower(COALESCE(error_message, '')) LIKE '%schema%'
                    OR lower(COALESCE(error_message, '')) LIKE '%unsupported%')",
            );
        }
        "other_error" => {
            sql.push_str(
                " AND (status_code >= 400 OR status_code < 200)
                  AND NOT (
                    status_code IN (401, 402, 403, 429)
                    OR status_code >= 500
                    OR lower(COALESCE(error_message, '')) LIKE '%unauthorized%'
                    OR lower(COALESCE(error_message, '')) LIKE '%authentication%'
                    OR lower(COALESCE(error_message, '')) LIKE '%invalid api key%'
                    OR lower(COALESCE(error_message, '')) LIKE '%invalid_api_key%'
                    OR lower(COALESCE(error_message, '')) LIKE '%rate limit%'
                    OR lower(COALESCE(error_message, '')) LIKE '%rate_limit%'
                    OR lower(COALESCE(error_message, '')) LIKE '%quota%'
                    OR lower(COALESCE(error_message, '')) LIKE '%balance%'
                    OR lower(COALESCE(error_message, '')) LIKE '%credit%'
                  )",
            );
        }
        _ => {}
    }
}

pub fn get_detail(conn: &Connection, id: &str) -> Result<RequestLogDetail, AppError> {
    conn.query_row(
        "SELECT id, request_id, timestamp, client, provider, model, route, status_code,
                latency_ms, input_tokens, output_tokens, raw_request, converted_request,
                raw_response, converted_response, sse_events, tool_calls, error_message, trace_json,
                source, session_id, external_id, cost, cache_write_tokens, cache_read_tokens
         FROM request_logs WHERE id = ?1",
        [id],
        |row| {
            Ok(RequestLogDetail {
                id: row.get(0)?,
                request_id: row.get(1)?,
                timestamp: row.get(2)?,
                client: row.get(3)?,
                provider: row.get(4)?,
                model: row.get(5)?,
                route: row.get(6)?,
                status_code: row.get(7)?,
                latency_ms: row.get(8)?,
                input_tokens: row.get(9)?,
                output_tokens: row.get(10)?,
                raw_request: row.get(11)?,
                converted_request: row.get(12)?,
                raw_response: row.get(13)?,
                converted_response: row.get(14)?,
                sse_events: row.get(15)?,
                tool_calls: row.get(16)?,
                error_message: row.get(17)?,
                trace_json: row.get(18)?,
                source: row.get(19)?,
                session_id: row.get(20)?,
                external_id: row.get(21)?,
                cost: row.get(22)?,
                cache_write_tokens: row.get(23)?,
                cache_read_tokens: row.get(24)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => AppError::not_found("RequestLog", id),
        other => AppError::database(other),
    })
}
