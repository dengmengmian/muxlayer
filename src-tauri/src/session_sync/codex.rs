//! Codex session log parser.
//!
//! ## File layout
//!
//! ```text
//! ~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<session_uuid>.jsonl
//! ```
//!
//! Each line is one JSON event. The events we care about flow as a small
//! state machine within a file:
//!
//!   1. `session_meta` (first line) — carries `payload.id` (the session uuid).
//!   2. `turn_context` — carries the current `payload.model` name; reset each
//!      time the user starts a new turn with a different model selection.
//!   3. `event_msg` with `payload.type == "token_count"` — has
//!      `payload.info.last_token_usage` = tokens consumed *by the most recent
//!      LLM call*. This is the billable event.
//!
//! We track session id + current model across the file, and emit one
//! `ParsedRow` per token_count event whose `last_token_usage` is populated.
//!
//! ## Dedup
//!
//! Codex events don't carry a globally unique message id. We compose
//! `external_id = "{session_id}:{event_index}"` where `event_index` is the
//! zero-based line number of the token_count event within its file. The
//! pair is stable across reruns (file content doesn't shift) and unique
//! within `source = 'codex_session'`.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::errors::AppError;
use crate::storage;

use super::SyncResult;

const SOURCE: &str = "codex_session";
const CLIENT: &str = "Codex";
const PROVIDER_LABEL: &str = "openai_official";
const ROUTE: &str = "/v1/responses";

/// `~/.codex/sessions/`. Returns the path even if missing — caller no-ops.
fn codex_sessions_dir() -> PathBuf {
    // Windows 没有 HOME,补 USERPROFILE fallback(同 claude.rs)。
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    home.join(".codex").join("sessions")
}

#[derive(Debug, Clone)]
struct ParsedRow {
    external_id: String,
    session_id: String,
    timestamp: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct Event {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    event_type: String,
    payload: Option<Value>,
}

/// Walk YYYY/MM/DD subtrees and gather all *.jsonl files.
fn collect_session_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
        if depth > 4 {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out, depth + 1);
            } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out, 0);
    out
}

/// Parse one rollout file. Returns billable rows; bad lines silently skipped.
fn parse_file(path: &Path, result: &mut SyncResult) -> Vec<ParsedRow> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            result.errors.push(format!("open {}: {e}", path.display()));
            return Vec::new();
        }
    };
    let reader = BufReader::new(file);
    let mut session_id = String::new();
    let mut current_model = String::from("codex");
    let mut rows = Vec::new();

    for (idx, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let payload = match event.payload {
            Some(p) => p,
            None => continue,
        };

        match event.event_type.as_str() {
            "session_meta" => {
                if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                    session_id = id.to_string();
                }
            }
            "turn_context" => {
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    current_model = m.to_string();
                }
            }
            "event_msg" => {
                if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
                    continue;
                }
                let info = match payload.get("info") {
                    Some(i) if !i.is_null() => i,
                    _ => continue, // 空 token_count（rate_limits-only ping），不算 billable
                };
                let last = match info.get("last_token_usage") {
                    Some(l) if !l.is_null() => l,
                    _ => continue,
                };
                let input = last
                    .get("input_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let output = last
                    .get("output_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let cached = last
                    .get("cached_input_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if input == 0 && output == 0 {
                    continue; // 全零事件，估计是 rate_limits-only ping，跳过
                }
                if session_id.is_empty() {
                    continue; // session_meta 没解析到（异常文件），跳过
                }
                let timestamp = event
                    .timestamp
                    .clone()
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
                rows.push(ParsedRow {
                    external_id: format!("{session_id}:{idx}"),
                    session_id: session_id.clone(),
                    timestamp,
                    model: current_model.clone(),
                    input_tokens: input,
                    output_tokens: output,
                    cached_input_tokens: cached,
                });
            }
            _ => {} // response_item / 其他事件不携带 token 用量
        }
    }
    rows
}

/// 按 session_id 在 ~/.codex/sessions 下找到对应 jsonl（文件名含 session_id）。
fn find_session_file(session_id: &str) -> Option<PathBuf> {
    collect_session_files(&codex_sessions_dir())
        .into_iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(session_id))
        })
}

/// 删除某个 Codex 会话的本地 jsonl 文件。文件不存在返回 Ok(false)；删除失败返回 Err。
pub fn delete_session_file(session_id: &str) -> Result<bool, AppError> {
    match find_session_file(session_id) {
        Some(path) => {
            fs::remove_file(&path).map_err(|e| {
                AppError::new(
                    crate::errors::codes::SESSION_DELETE_FAILED,
                    format!("删除 Codex 会话日志失败: {e}"),
                )
            })?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// 读取某个 Codex 会话的完整对话。Codex 日志是 event 流，对话文本在
/// `event_msg` 的 `user_message` / `agent_message` 的 `payload.message`。
pub fn read_conversation(
    session_id: &str,
) -> Result<Vec<crate::session_sync::claude::ConversationMessage>, AppError> {
    use crate::session_sync::claude::ConversationMessage;
    let path = find_session_file(session_id).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::SESSION_NOT_FOUND,
            "找不到该 Codex 会话的本地日志",
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
            Err(_) => continue,
        };
        if event.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = event.get("payload");
        let role = match payload
            .and_then(|p| p.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
        {
            "user_message" => "user",
            "agent_message" => "assistant",
            _ => continue,
        };
        let text = payload
            .and_then(|p| p.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        msgs.push(ConversationMessage {
            role: role.to_string(),
            text: text.to_string(),
            timestamp: event
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(String::from),
        });
    }
    Ok(msgs)
}

/// Sync all Codex session logs into request_logs. Idempotent.
pub fn sync(db: &crate::storage::db::DbPool) -> Result<SyncResult, AppError> {
    let mut result = SyncResult::default();
    let dir = codex_sessions_dir();
    if !dir.exists() {
        return Ok(result);
    }
    let files = collect_session_files(&dir);
    result.files_scanned = files.len() as u32;
    if files.is_empty() {
        return Ok(result);
    }

    let mut all_rows: Vec<ParsedRow> = Vec::new();
    for f in &files {
        all_rows.extend(parse_file(f, &mut result));
    }
    if all_rows.is_empty() {
        return Ok(result);
    }

    let candidate_ids: Vec<String> = all_rows.iter().map(|r| r.external_id.clone()).collect();
    let conn = db.get().map_err(|_| AppError::internal("DB lock failed"))?;
    let already = storage::request_logs::external_ids_for_source(&conn, SOURCE, &candidate_ids)?;
    // 价格表只加载一次,逐行内存匹配。
    let prices = storage::pricing::load_price_table(&conn)?;

    let mut new_rows = Vec::new();
    for row in all_rows {
        if already.contains(&row.external_id) {
            result.skipped += 1;
            continue;
        }
        // OpenAI 口径:cached_input_tokens 已包含在 input_tokens 里,按 input 计费,
        // 不再作为 cache_read 额外计费(否则重复)。
        let cost = prices.lookup(PROVIDER_LABEL, &row.model).map(|price| {
            storage::pricing::calculate_cost(
                Some(row.input_tokens),
                Some(row.output_tokens),
                price.input,
                price.output,
            )
        });
        // Codex 的 cached_input_tokens 在 OpenAI Responses 协议里属于「读缓存」语义，
        // 没有单独的 cache_creation；映射到 cache_read_tokens。
        new_rows.push(storage::request_logs::SessionLogRow {
            timestamp: row.timestamp,
            client: CLIENT.to_string(),
            provider: PROVIDER_LABEL.to_string(),
            model: row.model,
            route: ROUTE.to_string(),
            source: SOURCE.to_string(),
            session_id: row.session_id,
            external_id: row.external_id,
            input_tokens: Some(row.input_tokens),
            output_tokens: Some(row.output_tokens),
            cache_write_tokens: None,
            cache_read_tokens: (row.cached_input_tokens > 0).then_some(row.cached_input_tokens),
            cost,
        });
    }
    let outcome = storage::request_logs::insert_session_logs(&conn, &new_rows)?;
    result.imported += outcome.imported;
    for (id, e) in outcome.errors {
        result.errors.push(format!("insert {id}: {}", e.message));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sessions_dir_falls_back_to_userprofile_on_windows_style_env() {
        // Windows 没有 HOME,原实现退 "/" → ~/.codex/sessions 找不到,同步 no-op。
        crate::test_utils::with_windows_style_home(|fake| {
            assert!(codex_sessions_dir().starts_with(fake));
        });
    }

    #[test]
    fn parse_file_tracks_session_then_model_then_token_count() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        // session_meta (line 0)
        writeln!(f, r#"{{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{{"id":"sess_x"}}}}"#).unwrap();
        // turn_context (line 1)
        writeln!(f, r#"{{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{{"model":"gpt-5-codex"}}}}"#).unwrap();
        // event_msg / task_started — should be ignored
        writeln!(f, r#"{{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{{"type":"task_started"}}}}"#).unwrap();
        // token_count with usage (line 3)
        writeln!(f, r#"{{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":100,"output_tokens":50,"cached_input_tokens":20}}}}}}}}"#).unwrap();

        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.session_id, "sess_x");
        assert_eq!(r.external_id, "sess_x:3");
        assert_eq!(r.model, "gpt-5-codex");
        assert_eq!(r.input_tokens, 100);
        assert_eq!(r.output_tokens, 50);
        assert_eq!(r.cached_input_tokens, 20);
    }

    #[test]
    fn parse_file_skips_empty_token_count_pings() {
        // token_count 经常以 rate_limits-only 形式出现（info=null）；不该计费。
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"x","type":"session_meta","payload":{{"id":"s1"}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":null}}}}"#).unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":0,"output_tokens":0}}}}}}}}"#).unwrap();
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_file_handles_model_switch_mid_session() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"x","type":"session_meta","payload":{{"id":"s1"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"x","type":"turn_context","payload":{{"model":"gpt-5"}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":10,"output_tokens":5}}}}}}}}"#).unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"x","type":"turn_context","payload":{{"model":"gpt-5-codex"}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":20,"output_tokens":15}}}}}}}}"#).unwrap();
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].model, "gpt-5");
        assert_eq!(rows[1].model, "gpt-5-codex");
    }

    #[test]
    fn parse_file_uses_default_model_when_no_turn_context() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"x","type":"session_meta","payload":{{"id":"s1"}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":1,"output_tokens":2}}}}}}}}"#).unwrap();
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].model, "codex",
            "default model when turn_context absent"
        );
    }

    #[test]
    fn parse_file_skips_token_count_before_session_meta() {
        // 防御性测试：异常文件，session_meta 缺失 → token_count 也不计费。
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":99,"output_tokens":99}}}}}}}}"#).unwrap();
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert!(rows.is_empty(), "no session_id → can't dedup → skip");
    }

    #[test]
    fn collect_session_files_walks_y_m_d_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let day_dir = tmp.path().join("2026").join("05").join("18");
        fs::create_dir_all(&day_dir).unwrap();
        fs::write(day_dir.join("rollout-a.jsonl"), "").unwrap();
        fs::write(day_dir.join("not_session.txt"), "").unwrap();
        let files = collect_session_files(tmp.path());
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn external_id_is_session_plus_line_number() {
        // 同一 session 多个 token_count 事件 → external_id 各不相同
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"x","type":"session_meta","payload":{{"id":"s1"}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":10,"output_tokens":5}}}}}}}}"#).unwrap();
        writeln!(f, r#"{{"timestamp":"x","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":20,"output_tokens":7}}}}}}}}"#).unwrap();
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].external_id, rows[1].external_id);
        assert!(rows[0].external_id.starts_with("s1:"));
        assert!(rows[1].external_id.starts_with("s1:"));
    }
}
