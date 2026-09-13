//! Gemini CLI session log parser.
//!
//! ## File layout
//!
//! ```text
//! ~/.gemini/tmp/<project_hash>/chats/session-<timestamp>-<uuid>.jsonl
//! ```
//!
//! Despite living under `tmp/`, these are the real conversation logs Gemini
//! CLI writes (`tmp` is just where Gemini chose to put them; the directory
//! survives shell restarts).
//!
//! ## Line shape
//!
//! - First line: session header — `{"sessionId":"...","startTime":"...","kind":"main"}`.
//! - Each subsequent line is one event. Token-bearing events look like:
//!
//!   ```json
//!   {"id":"<uuid>","timestamp":"...","type":"gemini","content":"...",
//!    "tokens":{"input":n,"output":n,"cached":n,"thoughts":n,"tool":n,"total":n},
//!    "model":"gemini-3-flash-preview"}
//!   ```
//!
//! Other event types (`user`, `error`, `$set`, ...) carry no tokens and are
//! skipped. Each billable event already has a globally unique `id`, so dedup
//! is trivial.
//!
//! ## Output-token accounting
//!
//! Gemini reports tokens in 4 output-side buckets: `output` (user-visible
//! text), `thoughts` (chain-of-thought tokens), `tool` (function-call args).
//! All three are billed at the output rate, so we sum them into a single
//! `output_tokens` value before insertion.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::errors::AppError;
use crate::storage;

use super::SyncResult;

const SOURCE: &str = "gemini_session";
const CLIENT: &str = "Gemini CLI";
const PROVIDER_LABEL: &str = "google_official";
const ROUTE: &str = "/v1beta/generateContent";

fn gemini_chats_root() -> PathBuf {
    // Windows 没有 HOME,补 USERPROFILE fallback(同 claude.rs)。
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    home.join(".gemini").join("tmp")
}

#[derive(Debug, Clone)]
struct ParsedRow {
    external_id: String, // event id (UUID, globally unique)
    session_id: String,
    timestamp: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cached_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct GeminiEvent {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "type")]
    event_type: Option<String>,
    tokens: Option<TokenBuckets>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenBuckets {
    input: Option<i64>,
    output: Option<i64>,
    cached: Option<i64>,
    thoughts: Option<i64>,
    tool: Option<i64>,
    // total: reported by Gemini but recomputable; we don't store it
}

#[derive(Debug, Deserialize)]
struct SessionHeader {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

/// Walk `~/.gemini/tmp/*/chats/*.jsonl` and return file paths.
fn collect_session_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for proj in entries.flatten() {
        let chats = proj.path().join("chats");
        if !chats.is_dir() {
            continue;
        }
        let chat_entries = match fs::read_dir(&chats) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for f in chat_entries.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    out
}

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
    let mut rows = Vec::new();

    for (idx, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        // 第一行是 session header；后续行是 events。header 有 sessionId 但没 type 字段。
        if idx == 0 {
            if let Ok(header) = serde_json::from_str::<SessionHeader>(&line) {
                if let Some(sid) = header.session_id {
                    session_id = sid;
                }
            }
            continue;
        }
        let event: GeminiEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let tokens = match event.tokens {
            Some(t) => t,
            None => continue, // 非 billable 事件（user / error / $set）
        };
        let event_id = match event.id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };
        if session_id.is_empty() {
            continue;
        }
        let input = tokens.input.unwrap_or(0);
        // Gemini 把 output 拆成 3 桶：可见输出 + 思考 + 工具调用，全部按 output 计费。
        let output =
            tokens.output.unwrap_or(0) + tokens.thoughts.unwrap_or(0) + tokens.tool.unwrap_or(0);
        let cached = tokens.cached.unwrap_or(0);
        if input == 0 && output == 0 {
            continue;
        }
        rows.push(ParsedRow {
            external_id: event_id,
            session_id: session_id.clone(),
            timestamp: event
                .timestamp
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
            model: event.model.unwrap_or_else(|| "gemini".to_string()),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
        });
        let _ = event.event_type; // keep field for future filtering needs
    }
    rows
}

pub fn sync(db: &crate::storage::db::DbPool) -> Result<SyncResult, AppError> {
    let mut result = SyncResult::default();
    let dir = gemini_chats_root();
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
        // cached 已包含在 input 里,按 input 计费,不额外计缓存费。
        let cost = prices.lookup(PROVIDER_LABEL, &row.model).map(|price| {
            storage::pricing::calculate_cost(
                Some(row.input_tokens),
                Some(row.output_tokens),
                price.input,
                price.output,
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
            external_id: row.external_id,
            input_tokens: Some(row.input_tokens),
            output_tokens: Some(row.output_tokens),
            cache_write_tokens: None,
            cache_read_tokens: (row.cached_tokens > 0).then_some(row.cached_tokens),
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
    fn chats_root_falls_back_to_userprofile_on_windows_style_env() {
        // Windows 没有 HOME,原实现退 "/" → ~/.gemini/tmp 找不到,同步 no-op。
        crate::test_utils::with_windows_style_home(|fake| {
            assert!(gemini_chats_root().starts_with(fake));
        });
    }

    fn write_session_file(content: &str) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        tmp
    }

    #[test]
    fn parse_file_sums_output_thoughts_tool_into_output_tokens() {
        // Gemini 把生成 token 拆 3 桶，调用方应该看到合并后的 output。
        let content = r#"{"sessionId":"sess-x","startTime":"2026-05-18T00:00:00Z","kind":"main"}
{"id":"evt-1","timestamp":"2026-05-18T00:00:01Z","type":"gemini","content":"hi","tokens":{"input":100,"output":10,"thoughts":50,"tool":5,"cached":3,"total":168},"model":"gemini-3-flash-preview"}
"#;
        let tmp = write_session_file(content);
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.input_tokens, 100);
        assert_eq!(r.output_tokens, 65, "10 + 50 + 5");
        assert_eq!(r.cached_tokens, 3);
        assert_eq!(r.external_id, "evt-1");
        assert_eq!(r.session_id, "sess-x");
        assert_eq!(r.model, "gemini-3-flash-preview");
    }

    #[test]
    fn parse_file_skips_events_without_tokens() {
        let content = r#"{"sessionId":"s1","startTime":"x","kind":"main"}
{"id":"u1","timestamp":"x","type":"user","content":[{"text":"hello"}]}
{"$set":{"lastUpdated":"x"}}
{"id":"err1","timestamp":"x","type":"error","content":"[API Error]"}
"#;
        let tmp = write_session_file(content);
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert!(rows.is_empty(), "user / $set / error 均无 tokens 字段");
    }

    #[test]
    fn parse_file_skips_events_with_zero_tokens() {
        let content = r#"{"sessionId":"s1","startTime":"x","kind":"main"}
{"id":"z1","timestamp":"x","type":"gemini","tokens":{"input":0,"output":0,"thoughts":0,"tool":0,"cached":0,"total":0},"model":"gemini"}
"#;
        let tmp = write_session_file(content);
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert!(rows.is_empty(), "全 0 tokens 不计费");
    }

    #[test]
    fn parse_file_uses_default_model_when_missing() {
        let content = r#"{"sessionId":"s1","startTime":"x","kind":"main"}
{"id":"e1","timestamp":"x","type":"gemini","tokens":{"input":5,"output":3}}
"#;
        let tmp = write_session_file(content);
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "gemini");
    }

    #[test]
    fn parse_file_requires_session_header_first_line() {
        // 没 sessionId 的文件就算后续有 token 事件也不计费——避免误归到空 session。
        let content = r#"{"id":"e1","timestamp":"x","type":"gemini","tokens":{"input":10,"output":5},"model":"gemini"}
"#;
        let tmp = write_session_file(content);
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_file_recovers_from_malformed_event() {
        let content = "{\"sessionId\":\"s1\",\"startTime\":\"x\",\"kind\":\"main\"}
{not valid json
{\"id\":\"e1\",\"timestamp\":\"x\",\"type\":\"gemini\",\"tokens\":{\"input\":5,\"output\":3},\"model\":\"gemini-3-pro\"}
";
        let tmp = write_session_file(content);
        let mut result = SyncResult::default();
        let rows = parse_file(tmp.path(), &mut result);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn collect_session_files_walks_project_chats() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("project_a");
        let chats = proj.join("chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(chats.join("session-1.jsonl"), "").unwrap();
        fs::write(chats.join("ignore.txt"), "").unwrap();
        // 没 chats 子目录的项目应该被跳过
        fs::create_dir_all(tmp.path().join("project_b")).unwrap();
        fs::write(tmp.path().join("project_b").join("loose.jsonl"), "").unwrap();
        let files = collect_session_files(tmp.path());
        assert_eq!(files.len(), 1, "只有 chats/ 下的 .jsonl 算");
    }
}
