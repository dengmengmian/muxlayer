//! 客户端配置版本史。
//!
//! 每次 5 个客户端（codex / claude_code / opencode / gemini / atomcode）的
//! apply / disable / toggle 入口在写盘前打一次盘上配置文件的快照，写进
//! `client_apply_history`。用户在 UI 上选某条历史即可一键回滚到那个时点
//! 的磁盘状态。
//!
//! 设计要点：
//! - **snapshot 是盘上文件原文**（base64-encoded by the caller — caller 决定
//!   编码方式以容纳二进制；当前所有客户端都是文本配置，直接 raw string）。
//!   AgentGate 内部 state（active_provider_id 之类）不归这里管。
//! - **保留策略**：每个 client 第一条 `initial` 永久保留 + 最近 10 条滚动。
//!   apply / disable / toggle 都按"新历史"插入，写满后挤掉最老的非-initial。
//! - **回滚不写历史**：rollback 本身不产生新条目，否则用户连续多次回滚会
//!   把历史撑爆。但 rollback 之后的下一次 apply 仍会正常 record。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::errors::AppError;

const MAX_NON_INITIAL_PER_CLIENT: usize = 10;

/// 一条历史条目的可序列化形态。`snapshot_json` 是 `ClientSnapshot` 序列化
/// 后的字符串，反序列化交给 caller —— 5 个客户端各自知道怎么 restore。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct HistoryEntry {
    pub id: String,
    pub client_id: String,
    /// `apply` / `disable` / `toggle_to_agentgate` / `toggle_to_official`
    pub action: String,
    /// 序列化后的 `ClientSnapshot`。
    pub snapshot_json: String,
    /// 一句话摘要：changed_keys 拼起来 / "switch to official" / 等。
    pub summary: String,
    /// 同客户端第一条永远是 initial=true，从不被清理。
    pub is_initial: bool,
    pub agentgate_version: String,
    /// RFC3339。
    pub created_at: String,
}

/// 5 个客户端 snapshot 的统一结构。每个 file_name 是相对名（如 "config.toml"
/// / "auth.json" / "settings.json"），content 是 UTF-8 raw（当前所有客户端
/// 的配置都是文本）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientSnapshot {
    pub files: Vec<SnapshotFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotFile {
    /// 相对文件名，仅用于 UI 展示和 restore 时区分。
    pub name: String,
    /// 写入这一条 snapshot 时的绝对路径。restore 按这里写回去。
    pub absolute_path: String,
    /// 文件存不存在；不存在的话 content 为空字符串，restore 时要把对应文件
    /// 删掉而不是写入空内容（"配置不存在"和"配置为空"语义不同）。
    pub existed: bool,
    /// UTF-8 文件内容(原文)。existed=false 时为空字符串。
    pub content: String,
    /// 文件当时存在但读不出(权限 / 非 UTF-8)→ 内容未保存,该快照不能回滚此文件。
    /// 不能记成 existed=false,否则回滚会把这个文件删掉。
    #[serde(default)]
    pub content_omitted: bool,
}

/// Build a snapshot by reading each `(display_name, absolute_path)` off
/// disk. Missing files become `existed: false, content: ""` rows so the
/// restore path knows to delete instead of write-empty.
///
/// 保存文件原文(不脱敏):回滚必须逐字节还原 apply 之前的状态,脱敏会让回滚写回
/// 错误的 key、丢掉 OAuth token。provider key 本来就明文存在同一个 0600 数据库里。
pub fn snapshot_files_at(paths: &[(&str, std::path::PathBuf)]) -> ClientSnapshot {
    let files = paths
        .iter()
        .map(|(name, path)| {
            let (existed, content, content_omitted) = match std::fs::read_to_string(path) {
                Ok(raw) => (true, raw, false),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, String::new(), false),
                Err(_) => (true, String::new(), true),
            };
            SnapshotFile {
                name: (*name).to_string(),
                absolute_path: path.to_string_lossy().to_string(),
                existed,
                content,
                content_omitted,
            }
        })
        .collect();
    ClientSnapshot { files }
}

/// Write each file back to its captured absolute path. For files that didn't
/// exist at snapshot time, the current on-disk file is removed (if any).
/// Parent dirs are created on demand. Errors short-circuit and report which
/// file failed.
///
/// 写入是原子的(保留原文件权限)。任何文件在快照时读不出(`content_omitted`)
/// 就整体拒绝,一个文件都不写。
pub fn restore_files(snapshot: &ClientSnapshot) -> Result<(), AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    use std::fs;
    let restore_err = |msg: String| AppError::new(crate::errors::codes::CLIENT_RESTORE_FAILED, msg);

    if let Some(file) = snapshot.files.iter().find(|f| f.content_omitted) {
        return Err(restore_err(format!(
            "{} 在快照时无法读取,内容未保存,不能回滚到该记录",
            file.name
        )));
    }
    for file in &snapshot.files {
        let path = std::path::PathBuf::from(&file.absolute_path);
        if file.existed {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    restore_err(format!("Cannot create parent of {}: {e}", file.name))
                })?;
            }
            crate::fsutil::atomic_write(&path, file.content.as_bytes(), None)
                .map_err(|e| restore_err(format!("Cannot write {}: {e}", file.name)))?;
        } else if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                AppError::new(
                    crate::errors::codes::CLIENT_RESTORE_FAILED,
                    format!("Cannot remove {}: {e}", file.name),
                )
            })?;
        }
    }
    Ok(())
}

/// Insert one new history row. Trims older non-initial rows so each client
/// keeps at most `MAX_NON_INITIAL_PER_CLIENT` non-initial entries plus its
/// initial row.
pub fn record(
    conn: &Connection,
    client_id: &str,
    action: &str,
    snapshot: &ClientSnapshot,
    summary: &str,
) -> Result<String, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let version = env!("CARGO_PKG_VERSION").to_string();

    // First row for this client gets is_initial=1.
    let existing_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM client_apply_history WHERE client_id = ?1",
        params![client_id],
        |r| r.get(0),
    )?;
    let is_initial = if existing_count == 0 { 1 } else { 0 };

    let snapshot_json = serde_json::to_string(snapshot)
        .map_err(|e| AppError::internal(format!("snapshot serialise failed: {e}")))?;

    conn.execute(
        "INSERT INTO client_apply_history
         (id, client_id, action, snapshot_json, summary, is_initial, agentgate_version, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &id,
            client_id,
            action,
            &snapshot_json,
            summary,
            is_initial,
            &version,
            &now
        ],
    )?;

    trim_old(conn, client_id)?;
    Ok(id)
}

/// Drop non-initial rows beyond `MAX_NON_INITIAL_PER_CLIENT` keeping the
/// newest. Initial rows are never deleted.
fn trim_old(conn: &Connection, client_id: &str) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM client_apply_history
         WHERE id IN (
             SELECT id FROM client_apply_history
             WHERE client_id = ?1 AND is_initial = 0
             ORDER BY created_at DESC
             LIMIT -1 OFFSET ?2
         )",
        params![client_id, MAX_NON_INITIAL_PER_CLIENT as i64],
    )?;
    Ok(())
}

pub fn list(conn: &Connection, client_id: &str) -> Result<Vec<HistoryEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, client_id, action, snapshot_json, summary, is_initial,
                agentgate_version, created_at
         FROM client_apply_history
         WHERE client_id = ?1
         ORDER BY created_at DESC",
    )?;
    let rows = stmt
        .query_map(params![client_id], row_to_entry)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 曾经 apply 过配置的客户端 id 列表——用于「配置漂移」判断：detected 但接入过
/// 的客户端说明配置被改回去了，提示重新应用。
pub fn distinct_clients(conn: &Connection) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare("SELECT DISTINCT client_id FROM client_apply_history")?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: &str) -> Result<HistoryEntry, AppError> {
    conn.query_row(
        "SELECT id, client_id, action, snapshot_json, summary, is_initial,
                agentgate_version, created_at
         FROM client_apply_history
         WHERE id = ?1",
        params![id],
        row_to_entry,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => AppError::not_found("client_apply_history", id),
        other => AppError::from(other),
    })
}

/// 删除一条历史记录。拒绝删除「初始」快照——它是回滚到接入 AgentGate 前原始
/// 配置的唯一退路。删不存在的 id 报 not_found,删初始项报明确错误。
pub fn delete(conn: &Connection, id: &str) -> Result<(), AppError> {
    let entry = get(conn, id)?;
    if entry.is_initial {
        return Err(AppError::new(
            "APPLY_HISTORY_INITIAL_PROTECTED",
            "初始快照不可删除——它是回滚到原始配置的唯一退路",
        ));
    }
    conn.execute(
        "DELETE FROM client_apply_history WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get(0)?,
        client_id: row.get(1)?,
        action: row.get(2)?,
        snapshot_json: row.get(3)?,
        summary: row.get(4)?,
        is_initial: row.get::<_, i64>(5)? != 0,
        agentgate_version: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归:快照脱敏后回滚会把 gateway token 当成原 key 写回、丢掉
    /// ANTHROPIC_AUTH_TOKEN、把 Codex auth.json 的 tokens 删光(需重新登录)。
    /// 回滚必须逐字节恢复 apply 之前的文件。
    #[test]
    fn rollback_after_key_replacing_apply_restores_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cases: [(&'static str, &str, &str); 3] = [
            (
                "settings.json",
                "{\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"https://corp.example\",\n    \"ANTHROPIC_AUTH_TOKEN\": \"corp-token\",\n    \"ANTHROPIC_API_KEY\": \"sk-ant-orig\"\n  }\n}\n",
                "{\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:9090\",\n    \"ANTHROPIC_API_KEY\": \"ag_local_gw\"\n  }\n}\n",
            ),
            (
                "auth.json",
                r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"eyJ.id","access_token":"eyJ.at","refresh_token":"rt-orig","account_id":"acct-1"},"last_refresh":"2026-01-01T00:00:00Z"}"#,
                r#"{"OPENAI_API_KEY":"ag_local_gw"}"#,
            ),
            (
                "config.toml",
                "model_provider = \"corp\"\n[model_providers.corp]\nbase_url = \"https://corp\"\nenv_key = \"CORP_API_KEY\"\n",
                "model_provider = \"OpenAI\"\n[model_providers.OpenAI]\nexperimental_bearer_token = \"ag_local_gw\"\n",
            ),
        ];
        for (name, original, applied) in cases {
            let path = dir.path().join(name);
            std::fs::write(&path, original).unwrap();
            let snap = snapshot_files_at(&[(name, path.clone())]);
            std::fs::write(&path, applied).unwrap();
            restore_files(&snap).unwrap();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                original,
                "{name} must be restored byte-for-byte"
            );
        }
    }

    #[test]
    fn rollback_removes_file_that_did_not_exist_at_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let snap = snapshot_files_at(&[(".env", path.clone())]);
        std::fs::write(&path, "GEMINI_API_KEY=ag_local_gw\n").unwrap();
        restore_files(&snap).unwrap();
        assert!(!path.exists());
    }

    /// 快照时文件存在但读不出(非 UTF-8 / 权限):不能记成「不存在」,否则回滚会删掉它。
    #[test]
    fn rollback_refuses_snapshot_of_unreadable_file_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("config.toml");
        std::fs::write(&bad, b"a = 1 \xff").unwrap();
        let ok = dir.path().join("auth.json");
        std::fs::write(&ok, "{}").unwrap();
        let snap = snapshot_files_at(&[("auth.json", ok.clone()), ("config.toml", bad.clone())]);
        std::fs::write(&ok, "{\"changed\":1}").unwrap();
        assert!(restore_files(&snap).is_err());
        assert_eq!(std::fs::read(&bad).unwrap(), b"a = 1 \xff");
        assert_eq!(std::fs::read_to_string(&ok).unwrap(), "{\"changed\":1}");
    }

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();
        conn
    }

    fn dummy_snapshot(content: &str) -> ClientSnapshot {
        ClientSnapshot {
            files: vec![SnapshotFile {
                name: "config.toml".to_string(),
                absolute_path: "/tmp/codex/config.toml".to_string(),
                existed: true,
                content: content.to_string(),
                content_omitted: false,
            }],
        }
    }

    #[test]
    fn first_record_per_client_is_marked_initial() {
        let conn = setup();
        let id = record(&conn, "codex", "apply", &dummy_snapshot("a"), "first").unwrap();
        let entry = get(&conn, &id).unwrap();
        assert!(entry.is_initial);
    }

    #[test]
    fn subsequent_records_are_not_initial() {
        let conn = setup();
        record(&conn, "codex", "apply", &dummy_snapshot("a"), "1").unwrap();
        let id2 = record(&conn, "codex", "apply", &dummy_snapshot("b"), "2").unwrap();
        let entry = get(&conn, &id2).unwrap();
        assert!(!entry.is_initial);
    }

    #[test]
    fn delete_removes_non_initial_entry() {
        let conn = setup();
        record(&conn, "codex", "apply", &dummy_snapshot("a"), "1").unwrap();
        let id2 = record(&conn, "codex", "apply", &dummy_snapshot("b"), "2").unwrap();
        delete(&conn, &id2).unwrap();
        assert!(get(&conn, &id2).is_err(), "deleted entry should be gone");
        assert_eq!(list(&conn, "codex").unwrap().len(), 1);
    }

    #[test]
    fn delete_refuses_initial_entry() {
        let conn = setup();
        let id = record(&conn, "codex", "apply", &dummy_snapshot("a"), "first").unwrap();
        assert!(delete(&conn, &id).is_err(), "initial must be protected");
        assert!(
            get(&conn, &id).is_ok(),
            "initial still present after refused delete"
        );
    }

    #[test]
    fn list_returns_newest_first() {
        let conn = setup();
        record(&conn, "codex", "apply", &dummy_snapshot("a"), "1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        record(&conn, "codex", "apply", &dummy_snapshot("b"), "2").unwrap();
        let rows = list(&conn, "codex").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].summary, "2");
        assert_eq!(rows[1].summary, "1");
    }

    #[test]
    fn retention_keeps_initial_plus_10_most_recent() {
        let conn = setup();
        for i in 0..15 {
            record(
                &conn,
                "codex",
                "apply",
                &dummy_snapshot(&i.to_string()),
                &i.to_string(),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let rows = list(&conn, "codex").unwrap();
        // 1 initial (i=0) + 10 most-recent non-initial = 11
        assert_eq!(rows.len(), 11);
        // Initial row is the original (summary="0").
        assert!(rows.iter().any(|r| r.is_initial && r.summary == "0"));
        // Newest non-initial is i=14.
        assert_eq!(rows[0].summary, "14");
    }

    #[test]
    fn retention_per_client_isolation() {
        let conn = setup();
        record(&conn, "codex", "apply", &dummy_snapshot("c1"), "c1").unwrap();
        record(&conn, "claude_code", "apply", &dummy_snapshot("cl1"), "cl1").unwrap();
        let codex_rows = list(&conn, "codex").unwrap();
        let claude_rows = list(&conn, "claude_code").unwrap();
        assert_eq!(codex_rows.len(), 1);
        assert_eq!(claude_rows.len(), 1);
        assert!(codex_rows[0].is_initial);
        assert!(claude_rows[0].is_initial);
    }

    #[test]
    fn snapshot_json_roundtrips_through_storage() {
        let conn = setup();
        let snap = ClientSnapshot {
            files: vec![
                SnapshotFile {
                    name: "config.toml".into(),
                    absolute_path: "/x".into(),
                    existed: true,
                    content: "[k]\nv=1".into(),
                    content_omitted: false,
                },
                SnapshotFile {
                    name: "auth.json".into(),
                    absolute_path: "/y".into(),
                    existed: false,
                    content: "".into(),
                    content_omitted: false,
                },
            ],
        };
        let id = record(&conn, "codex", "apply", &snap, "test").unwrap();
        let entry = get(&conn, &id).unwrap();
        let restored: ClientSnapshot = serde_json::from_str(&entry.snapshot_json).unwrap();
        assert_eq!(restored.files.len(), 2);
        assert!(!restored.files[1].existed);
        assert_eq!(restored.files[0].content, "[k]\nv=1");
    }
}
