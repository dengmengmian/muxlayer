//! Claude Code session log parser.
//!
//! ## File layout
//!
//! ```text
//! ~/.claude/projects/<project_hash>/<session_uuid>.jsonl
//! ```
//!
//! Each line is one JSON event. The events we care about are
//! `{"type":"assistant","sessionId":"...","timestamp":"...","message":{...}}`
//! where `message.id` is the upstream message id (the dedup key),
//! `message.model` names the model, and `message.usage` gives input /
//! output / cache-read / cache-creation token counts.
//!
//! Every other event type (`user`, `summary`, `file-history-snapshot`,
//! `permission-mode`, …) is skipped. We only care about *billable* events.
//!
//! ## Sync semantics
//!
//! Idempotent: `external_ids_for_source` filters out message ids already
//! present in `request_logs`. Re-running the sync after a fresh batch of
//! Claude usage only writes the delta.
//!
//! Robust to format drift: a malformed line is logged-and-skipped, not
//! crash. Files that don't exist (Claude isn't installed) just produce an
//! empty result.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::errors::AppError;
use crate::storage;

use super::SyncResult;

const SOURCE: &str = "claude_session";
const CLIENT: &str = "Claude Code";
const PROVIDER_LABEL: &str = "anthropic_official";
const ROUTE: &str = "/v1/messages";

/// `~/.claude/projects/`. Returns the path even if the dir doesn't exist —
/// the caller handles missing-dir as a no-op.
///
/// Falls back to `/` when HOME isn't set; in practice that means the dir
/// won't exist and sync becomes a no-op.
fn claude_projects_dir() -> PathBuf {
    // Windows 没有 HOME(只有 USERPROFILE),不补 fallback 会退到 "/",
    // 目录永远找不到,用量统计同步静默变 no-op。
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    home.join(".claude").join("projects")
}

/// One billable assistant event extracted from the JSONL stream.
#[derive(Debug, Clone)]
struct ParsedRow {
    message_id: String,
    session_id: String,
    timestamp: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
}

/// Top-level "assistant" event we care about.
#[derive(Debug, Deserialize)]
struct AssistantEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<String>,
    message: Option<AssistantMessage>,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Value>,
}

fn parse_usage(usage: &Value) -> (i64, i64, i64, i64) {
    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let cache_write = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    (input, output, cache_read, cache_write)
}

/// Parse a single .jsonl file. Returns the list of billable rows. Bad lines
/// are skipped (errors accumulate in `result.errors`).
fn parse_file(path: &Path, result: &mut SyncResult) -> Vec<ParsedRow> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            result.errors.push(format!("open {}: {e}", path.display()));
            return Vec::new();
        }
    };
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue, // partial / non-utf8 line — skip
        };
        if line.trim().is_empty() {
            continue;
        }
        let event: AssistantEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue, // malformed line — skip
        };
        if event.event_type != "assistant" {
            continue;
        }
        let msg = match event.message {
            Some(m) => m,
            None => continue,
        };
        let message_id = match msg.id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };
        let model = msg.model.unwrap_or_else(|| "unknown".to_string());
        let session_id = event.session_id.unwrap_or_default();
        let timestamp = event
            .timestamp
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        let usage = msg
            .usage
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let (input, output, cache_read, cache_write) = parse_usage(&usage);
        out.push(ParsedRow {
            message_id,
            session_id,
            timestamp,
            model,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
        });
    }
    out
}

/// Walk `~/.claude/projects/*/*.jsonl` and collect file paths.
fn collect_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return files,
    };
    for proj in entries.flatten() {
        let proj_path = proj.path();
        if !proj_path.is_dir() {
            continue;
        }
        let inner = match fs::read_dir(&proj_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for f in inner.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                files.push(p);
            }
        }
    }
    files
}

/// 会话里的一条对话消息——会话详情视图渲染气泡用。
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct ConversationMessage {
    pub role: String, // user / assistant
    pub text: String,
    pub timestamp: Option<String>,
}

/// 从 content（string 或 Anthropic content block 数组）提取可读文本。
/// tool_use → `[Tool: name]`，tool_result → `[Tool result] ...`。
fn extract_content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            let mut parts = Vec::new();
            for item in items {
                match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                    "text" => {
                        if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
                            parts.push(s.to_string());
                        }
                    }
                    "tool_use" => {
                        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                        parts.push(format!("[Tool: {name}]"));
                    }
                    "tool_result" => {
                        let inner = extract_content_text(item.get("content"));
                        parts.push(if inner.is_empty() {
                            "[Tool result]".to_string()
                        } else {
                            format!("[Tool result] {inner}")
                        });
                    }
                    _ => {}
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// 按 session_id 在 ~/.claude/projects/*/ 下找到对应 jsonl 文件。
fn find_session_file(session_id: &str) -> Option<PathBuf> {
    let dir = claude_projects_dir();
    for proj in fs::read_dir(&dir).ok()?.flatten() {
        let candidate = proj.path().join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// 读取某个 Claude Code 会话的完整对话（user / assistant 消息）。
pub fn read_conversation(session_id: &str) -> Result<Vec<ConversationMessage>, AppError> {
    let path = find_session_file(session_id).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::SESSION_NOT_FOUND,
            "找不到该会话的本地日志文件",
        )
    })?;
    let content = fs::read_to_string(&path).map_err(|e| {
        AppError::new(
            crate::errors::codes::SESSION_READ_FAILED,
            format!("读取会话日志失败: {e}"),
        )
    })?;

    let mut msgs = Vec::new();
    for line in content.lines() {
        let event: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // 坏行跳过
        };
        let typ = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if typ != "user" && typ != "assistant" {
            continue;
        }
        let message = event.get("message");
        let role = message
            .and_then(|m| m.get("role"))
            .and_then(|v| v.as_str())
            .unwrap_or(typ)
            .to_string();
        let timestamp = event
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(String::from);
        let text = extract_content_text(message.and_then(|m| m.get("content")));
        if text.trim().is_empty() {
            continue;
        }
        msgs.push(ConversationMessage {
            role,
            text,
            timestamp,
        });
    }
    Ok(msgs)
}

/// 删除某个 Claude Code 会话的本地 jsonl 文件。文件不存在返回 Ok(false)（可能是别的
/// 客户端的会话或网关会话）；删除失败（如权限）返回 Err，不静默吞。
pub fn delete_session_file(session_id: &str) -> Result<bool, AppError> {
    match find_session_file(session_id) {
        Some(path) => {
            fs::remove_file(&path).map_err(|e| {
                AppError::new(
                    crate::errors::codes::SESSION_DELETE_FAILED,
                    format!("删除 Claude 会话日志失败: {e}"),
                )
            })?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Sync all Claude session logs into request_logs. Idempotent: messages
/// already present (by external_id) are skipped.
pub fn sync(db: &crate::storage::db::DbPool) -> Result<SyncResult, AppError> {
    let mut result = SyncResult::default();
    let dir = claude_projects_dir();
    if !dir.exists() {
        // Claude not installed / never used. Not an error.
        return Ok(result);
    }
    let files = collect_session_files(&dir);
    result.files_scanned = files.len() as u32;
    if files.is_empty() {
        return Ok(result);
    }

    // Phase 1: parse all files, collect rows.
    let mut all_rows: Vec<ParsedRow> = Vec::new();
    for f in &files {
        all_rows.extend(parse_file(f, &mut result));
    }
    if all_rows.is_empty() {
        return Ok(result);
    }

    let conn = db.get().map_err(|_| AppError::internal("DB lock failed"))?;
    import_parsed_rows(&conn, all_rows, &mut result)?;
    Ok(result)
}

/// Phase 2 + 3:去重后批量写入(分批事务,见 `insert_session_logs`)。
fn import_parsed_rows(
    conn: &rusqlite::Connection,
    all_rows: Vec<ParsedRow>,
    result: &mut SyncResult,
) -> Result<(), AppError> {
    // Phase 2: filter out external_ids we've already imported.
    let candidate_ids: Vec<String> = all_rows.iter().map(|r| r.message_id.clone()).collect();
    let already = storage::request_logs::external_ids_for_source(conn, SOURCE, &candidate_ids)?;
    // 价格表只加载一次,逐行内存匹配。
    let prices = storage::pricing::load_price_table(conn)?;

    // Phase 3: write new rows.
    let mut new_rows = Vec::new();
    for row in all_rows {
        if already.contains(&row.message_id) {
            result.skipped += 1;
            continue;
        }
        let cache_write = (row.cache_write_tokens > 0).then_some(row.cache_write_tokens);
        let cache_read = (row.cache_read_tokens > 0).then_some(row.cache_read_tokens);
        // Anthropic 口径:input_tokens 不含缓存 token,缓存读写单独计费。
        let cost = prices.lookup(PROVIDER_LABEL, &row.model).map(|price| {
            storage::pricing::calculate_cost_with_cache(
                Some(row.input_tokens),
                Some(row.output_tokens),
                cache_read,
                cache_write,
                &price,
            )
        });
        new_rows.push(storage::request_logs::SessionLogRow {
            timestamp: row.timestamp,
            client: CLIENT.to_string(),
            provider: PROVIDER_LABEL.to_string(),
            model: row.model,
            route: ROUTE.to_string(),
            source: SOURCE.to_string(),
            session_id: row.session_id,
            external_id: row.message_id,
            input_tokens: Some(row.input_tokens),
            output_tokens: Some(row.output_tokens),
            cache_write_tokens: cache_write,
            cache_read_tokens: cache_read,
            cost,
        });
    }
    let outcome = storage::request_logs::insert_session_logs(conn, &new_rows)?;
    result.imported += outcome.imported;
    for (id, e) in outcome.errors {
        result
            .errors
            .push(format!("insert msg {id}: {}", e.message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::io::Write;

    #[test]
    fn projects_dir_falls_back_to_userprofile_on_windows_style_env() {
        // 复现 bug:Windows 没有 HOME(只有 USERPROFILE),原实现退到 "/",
        // ~/.claude/projects 永远找不到,用量统计同步静默变 no-op。
        crate::test_utils::with_windows_style_home(|fake| {
            let dir = claude_projects_dir();
            assert!(dir.starts_with(fake), "应退到 USERPROFILE,实际 {dir:?}");
        });
    }

    #[test]
    fn parse_usage_handles_full_anthropic_shape() {
        let usage = serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 200,
            "cache_creation_input_tokens": 10,
        });
        let (i, o, r, w) = parse_usage(&usage);
        assert_eq!((i, o, r, w), (100, 50, 200, 10));
    }

    #[test]
    fn parse_usage_treats_missing_fields_as_zero() {
        let usage = serde_json::json!({"input_tokens": 5});
        let (i, o, r, w) = parse_usage(&usage);
        assert_eq!((i, o, r, w), (5, 0, 0, 0));
    }

    #[test]
    fn parse_file_skips_non_assistant_events() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"role":"user"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"summary"}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{{"id":"msg_a","model":"claude-x","usage":{{"input_tokens":1,"output_tokens":2}}}}}}"#).unwrap();

        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_id, "msg_a");
        assert_eq!(rows[0].input_tokens, 1);
        assert_eq!(rows[0].output_tokens, 2);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn parse_file_skips_malformed_lines_without_error() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(f, "{{not json").unwrap();
        writeln!(f).unwrap();
        writeln!(f, r#"{{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{{"id":"msg_b","model":"claude-y","usage":{{"input_tokens":3,"output_tokens":4}}}}}}"#).unwrap();

        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 1, "good line survives bad neighbours");
        assert!(
            result.errors.is_empty(),
            "malformed lines don't accumulate errors"
        );
    }

    #[test]
    fn parse_file_skips_assistant_without_message_id() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(f, r#"{{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{{"model":"x","usage":{{}}}}}}"#).unwrap();
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert!(rows.is_empty(), "message without id is not billable");
    }

    #[test]
    fn collect_session_files_returns_empty_for_missing_dir() {
        let dir = std::env::temp_dir().join("nonexistent_claude_test_dir");
        let files = collect_session_files(&dir);
        assert!(files.is_empty());
    }

    #[test]
    fn collect_session_files_walks_two_levels() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj_a");
        fs::create_dir(&proj).unwrap();
        fs::write(proj.join("a.jsonl"), "").unwrap();
        fs::write(proj.join("not_a_session.txt"), "").unwrap();
        let proj2 = tmp.path().join("proj_b");
        fs::create_dir(&proj2).unwrap();
        fs::write(proj2.join("b.jsonl"), "").unwrap();
        let files = collect_session_files(tmp.path());
        assert_eq!(files.len(), 2, "only .jsonl files counted");
    }

    #[test]
    fn sync_idempotent_skips_already_imported() {
        let conn = Connection::open_in_memory().unwrap();
        storage::migrations::run_migrations(&conn).unwrap();
        // 直接构造一条已存在的条目
        storage::request_logs::insert_session_log(
            &conn,
            "2026-01-01T00:00:00Z",
            CLIENT,
            PROVIDER_LABEL,
            "claude-x",
            ROUTE,
            SOURCE,
            "s1",
            "msg_existing",
            Some(10),
            Some(20),
            None,
            None,
            None,
        )
        .unwrap();
        let already = storage::request_logs::external_ids_for_source(
            &conn,
            SOURCE,
            &["msg_existing".to_string(), "msg_new".to_string()],
        )
        .unwrap();
        assert!(already.contains("msg_existing"));
        assert!(!already.contains("msg_new"));
    }

    fn parsed(id: &str, ts: &str) -> ParsedRow {
        ParsedRow {
            message_id: id.to_string(),
            session_id: "s1".to_string(),
            timestamp: ts.to_string(),
            model: "claude-sonnet-4-6".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 10_000,
            cache_write_tokens: 2000,
        }
    }

    #[test]
    fn import_counts_cache_tokens_in_cost_and_normalizes_timestamp() {
        // Anthropic 的 input_tokens 不含缓存 token;重缓存会话只按 input/output 计费会严重低估。
        let conn = Connection::open_in_memory().unwrap();
        storage::migrations::run_migrations(&conn).unwrap();
        let mut result = SyncResult::default();
        import_parsed_rows(
            &conn,
            vec![parsed("msg_1", "2026-06-01T20:00:00+08:00")],
            &mut result,
        )
        .unwrap();
        assert_eq!(result.imported, 1);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let (ts, cost, cr, cw): (String, f64, i64, i64) = conn
            .query_row(
                "SELECT timestamp, cost, cache_read_tokens, cache_write_tokens FROM request_logs WHERE external_id = 'msg_1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(ts, "2026-06-01T12:00:00+00:00");
        assert_eq!((cr, cw), (10_000, 2000));
        // (1000×3 + 500×15 + 10000×0.3 + 2000×3.75) / 1e6
        assert!((cost - 0.021).abs() < 1e-12, "cost={cost}");

        // 幂等:再次导入全部跳过。
        let mut again = SyncResult::default();
        import_parsed_rows(
            &conn,
            vec![parsed("msg_1", "2026-06-01T20:00:00+08:00")],
            &mut again,
        )
        .unwrap();
        assert_eq!((again.imported, again.skipped), (0, 1));
    }

    // 注：sync() 全链路测试需要 mock 文件系统 + DB，留作集成测试。
}
