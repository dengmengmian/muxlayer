//! Agent Task / Session 持久化。
//!
//! SQLite 是 Agent Gateway 任务与会话状态的唯一真相源。Service 运行时的内存态是
//! 写穿缓存：每次状态变更都同步落库，重启后从库恢复。
//!
//! 与 Model Gateway 的 `gateway/session_store.rs`（模型响应缓存）完全独立：
//! 这里只存 Agent Task / Session，不复用也不修改原来的 session store。
//! 表结构见 `storage/migrations.rs` v13。
use super::protocol::*;
use crate::storage::db::DbPool;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// Task / Session 的持久化接口。所有实现必须是幂等可重入的安全写入。
pub trait TaskStore: Send + Sync {
    fn upsert_session(&self, session: &SessionRecord) -> Result<(), AgentError>;
    fn insert_task(&self, task: &TaskRecord) -> Result<(), AgentError>;
    /// 原子提交任务与 Session 占位，失败时两者都不改变。
    fn submit_task(&self, task: &TaskRecord, session: &SessionRecord) -> Result<(), AgentError>;
    fn update_task(&self, task: &TaskRecord) -> Result<(), AgentError>;
    /// 原子完成：终态结果与 Session 占位清空在同一事务里提交。
    /// 不允许先写半个结果再宣布 Completed。
    fn finish_task(&self, task: &TaskRecord, session: &SessionRecord) -> Result<(), AgentError>;
    fn task(&self, task_id: &str) -> Result<Option<TaskRecord>, AgentError>;
    fn session(&self, session_id: &str) -> Result<Option<SessionRecord>, AgentError>;
    /// 全量装载（服务启动恢复用），按创建顺序返回。
    fn load(&self) -> Result<(Vec<SessionRecord>, Vec<TaskRecord>), AgentError>;
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn store_error(what: &str, e: impl std::fmt::Display) -> AgentError {
    AgentError::new(ErrorCode::Storage, format!("{what}: {e}"))
}

fn mode_str(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "read_only",
        PermissionMode::Execute => "execute",
    }
}

fn parse_mode(raw: &str) -> Result<PermissionMode, AgentError> {
    Ok(match raw {
        "read_only" => PermissionMode::ReadOnly,
        "execute" => PermissionMode::Execute,
        other => return Err(store_error("session mode", format!("unknown {other}"))),
    })
}

fn state_str(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "queued",
        TaskState::Running => "running",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
        TaskState::Interrupted => "interrupted",
    }
}

fn parse_state(raw: &str) -> Result<TaskState, AgentError> {
    Ok(match raw {
        "queued" => TaskState::Queued,
        "running" => TaskState::Running,
        "completed" => TaskState::Completed,
        "failed" => TaskState::Failed,
        "cancelled" => TaskState::Cancelled,
        "interrupted" => TaskState::Interrupted,
        other => return Err(store_error("task status", format!("unknown {other}"))),
    })
}

fn encode_events(events: &[TaskEvent]) -> Result<String, AgentError> {
    serde_json::to_string(events).map_err(|e| store_error("encode events", e))
}

fn decode_events(raw: Option<String>) -> Result<Vec<TaskEvent>, AgentError> {
    match raw {
        Some(raw) if !raw.is_empty() => {
            serde_json::from_str(&raw).map_err(|e| store_error("decode events", e))
        }
        _ => Ok(vec![]),
    }
}

fn encode_error(error: &Option<AgentError>) -> Result<Option<String>, AgentError> {
    match error {
        Some(e) => serde_json::to_string(e)
            .map(Some)
            .map_err(|e| store_error("encode error", e)),
        None => Ok(None),
    }
}

fn decode_error(raw: Option<String>) -> Result<Option<AgentError>, AgentError> {
    match raw {
        Some(raw) if !raw.is_empty() => {
            serde_json::from_str(&raw).map_err(|e| store_error("decode error", e))
        }
        _ => Ok(None),
    }
}

/// 共享的列写入逻辑；`insert` 与 `update` 的列集合完全一致，避免两条路径漂移。
fn write_task(conn: &Connection, task: &TaskRecord, insert: bool) -> Result<(), AgentError> {
    let error_json = encode_error(&task.error)?;
    let events_json = encode_events(&task.events)?;
    let updated = now();
    if insert {
        conn.execute(
            "INSERT INTO agent_tasks (task_id, session_id, provider_turn_id, status, prompt,
                 final_answer, error_json, events_json, cancel_requested, created_at, updated_at,
                 completed_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                task.task_id,
                task.session_id,
                task.turn_id,
                state_str(task.state),
                task.prompt,
                task.final_answer,
                error_json,
                events_json,
                task.cancel_requested as i64,
                task.created_at,
                updated,
                task.finished_at,
            ],
        )
        .map_err(|e| store_error("insert task", e))?;
    } else {
        conn.execute(
            "UPDATE agent_tasks SET provider_turn_id=?2, status=?3, prompt=?4, final_answer=?5,
                 error_json=?6, events_json=?7, cancel_requested=?8, updated_at=?9, completed_at=?10
             WHERE task_id=?1",
            params![
                task.task_id,
                task.turn_id,
                state_str(task.state),
                task.prompt,
                task.final_answer,
                error_json,
                events_json,
                task.cancel_requested as i64,
                updated,
                task.finished_at,
            ],
        )
        .map_err(|e| store_error("update task", e))?;
    }
    Ok(())
}

fn write_session(
    conn: &Connection,
    session: &SessionRecord,
    insert: bool,
) -> Result<(), AgentError> {
    let updated = now();
    let cwd = session.cwd.to_string_lossy().to_string();
    if insert {
        conn.execute(
            "INSERT INTO agent_sessions (session_id, agent, provider_session_id, cwd, mode,
                 active_task_id, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                session.session_id,
                session.agent,
                session.thread_id,
                cwd,
                mode_str(session.permission),
                session.active_task_id,
                session.created_at,
                updated,
            ],
        )
        .map_err(|e| store_error("insert session", e))?;
    } else {
        conn.execute(
            "UPDATE agent_sessions SET agent=?2, provider_session_id=?3, cwd=?4, mode=?5,
                 active_task_id=?6, updated_at=?7 WHERE session_id=?1",
            params![
                session.session_id,
                session.agent,
                session.thread_id,
                cwd,
                mode_str(session.permission),
                session.active_task_id,
                updated,
            ],
        )
        .map_err(|e| store_error("update session", e))?;
    }
    Ok(())
}

fn read_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let cwd: String = row.get("cwd")?;
    let mode: String = row.get("mode")?;
    Ok(SessionRecord {
        session_id: row.get("session_id")?,
        agent: row.get("agent")?,
        cwd: PathBuf::from(cwd),
        permission: parse_mode(&mode).map_err(to_sql_error)?,
        thread_id: row.get("provider_session_id")?,
        active_task_id: row.get("active_task_id")?,
        task_ids: vec![],
        created_at: row.get("created_at")?,
    })
}

/// 把持久化解码错误捧回 rusqlite，避免静默兜底（silent fallback）。
fn to_sql_error(e: AgentError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.message,
        )),
    )
}

fn read_task(row: &rusqlite::Row<'_>, thread_id: Option<String>) -> rusqlite::Result<TaskRecord> {
    let status: String = row.get("status")?;
    let error_json: Option<String> = row.get("error_json")?;
    let events_json: Option<String> = row.get("events_json")?;
    Ok(TaskRecord {
        task_id: row.get("task_id")?,
        session_id: row.get("session_id")?,
        prompt: row.get("prompt")?,
        state: parse_state(&status).map_err(to_sql_error)?,
        thread_id,
        turn_id: row.get("provider_turn_id")?,
        final_answer: row.get("final_answer")?,
        error: decode_error(error_json).map_err(to_sql_error)?,
        events: decode_events(events_json).map_err(to_sql_error)?,
        cancel_requested: row.get::<_, i64>("cancel_requested")? != 0,
        created_at: row.get("created_at")?,
        finished_at: row.get("completed_at")?,
    })
}

/// MuxLayer 现有 SQLite 基础设施（`storage::db::DbPool` + migrations）。
pub struct SqliteStore {
    pool: DbPool,
}

impl SqliteStore {
    pub fn new(pool: DbPool) -> Arc<Self> {
        Arc::new(Self { pool })
    }

    fn conn(&self) -> Result<crate::storage::db::DbConn, AgentError> {
        self.pool
            .get()
            .map_err(|e| store_error("db pool unavailable", e))
    }
}

impl TaskStore for SqliteStore {
    fn upsert_session(&self, session: &SessionRecord) -> Result<(), AgentError> {
        let conn = self.conn()?;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM agent_sessions WHERE session_id=?1",
                params![session.session_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| store_error("lookup session", e))?;
        write_session(&conn, session, exists.is_none())
    }

    fn insert_task(&self, task: &TaskRecord) -> Result<(), AgentError> {
        let conn = self.conn()?;
        write_task(&conn, task, true)
    }

    fn submit_task(&self, task: &TaskRecord, session: &SessionRecord) -> Result<(), AgentError> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| store_error("begin submit tx", e))?;
        write_session(&tx, session, false)?;
        write_task(&tx, task, true)?;
        tx.commit().map_err(|e| store_error("commit submit tx", e))
    }

    fn update_task(&self, task: &TaskRecord) -> Result<(), AgentError> {
        let conn = self.conn()?;
        write_task(&conn, task, false)
    }

    fn finish_task(&self, task: &TaskRecord, session: &SessionRecord) -> Result<(), AgentError> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| store_error("begin finish tx", e))?;
        write_task(&tx, task, false)?;
        write_session(&tx, session, false)?;
        tx.commit().map_err(|e| store_error("commit finish tx", e))
    }

    fn task(&self, task_id: &str) -> Result<Option<TaskRecord>, AgentError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT t.*, s.provider_session_id AS thread_id FROM agent_tasks t
               JOIN agent_sessions s ON s.session_id = t.session_id
              WHERE t.task_id = ?1",
            params![task_id],
            |row| {
                let thread: Option<String> = row.get("thread_id")?;
                read_task(row, thread)
            },
        )
        .optional()
        .map_err(|e| store_error("read task", e))
    }

    fn session(&self, session_id: &str) -> Result<Option<SessionRecord>, AgentError> {
        let conn = self.conn()?;
        let mut session = conn
            .query_row(
                "SELECT * FROM agent_sessions WHERE session_id = ?1",
                params![session_id],
                read_session,
            )
            .optional()
            .map_err(|e| store_error("read session", e))?;
        if let Some(session) = session.as_mut() {
            let mut stmt = conn
                .prepare("SELECT task_id FROM agent_tasks WHERE session_id=?1 ORDER BY rowid")
                .map_err(|e| store_error("prepare task ids", e))?;
            let ids = stmt
                .query_map(params![session_id], |r| r.get::<_, String>(0))
                .map_err(|e| store_error("query task ids", e))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| store_error("collect task ids", e))?;
            session.task_ids = ids;
        }
        Ok(session)
    }

    fn load(&self) -> Result<(Vec<SessionRecord>, Vec<TaskRecord>), AgentError> {
        let conn = self.conn()?;
        let mut sessions = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT * FROM agent_sessions ORDER BY created_at, rowid")
                .map_err(|e| store_error("prepare sessions", e))?;
            let rows = stmt
                .query_map([], read_session)
                .map_err(|e| store_error("query sessions", e))?;
            for row in rows {
                sessions.push(row.map_err(|e| store_error("read session row", e))?);
            }
        }
        let mut tasks = Vec::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT t.*, s.provider_session_id AS thread_id FROM agent_tasks t
                       JOIN agent_sessions s ON s.session_id = t.session_id
                      ORDER BY t.rowid",
                )
                .map_err(|e| store_error("prepare tasks", e))?;
            let rows = stmt
                .query_map([], |row| {
                    let thread: Option<String> = row.get("thread_id")?;
                    read_task(row, thread)
                })
                .map_err(|e| store_error("query tasks", e))?;
            for row in rows {
                tasks.push(row.map_err(|e| store_error("read task row", e))?);
            }
        }
        // task_ids 由有序任务关系派生，与 session 查询保持一致。
        let mut by_session: HashMap<String, Vec<String>> = HashMap::new();
        for task in &tasks {
            by_session
                .entry(task.session_id.clone())
                .or_default()
                .push(task.task_id.clone());
        }
        for session in sessions.iter_mut() {
            if let Some(ids) = by_session.get(&session.session_id) {
                session.task_ids = ids.clone();
            }
        }
        Ok((sessions, tasks))
    }
}

/// 纯内存实现：用于 `AgentService::new` 与单元测试。生产服务必须使用 SqliteStore。
#[derive(Default)]
pub struct MemoryStore {
    sessions: Mutex<HashMap<String, SessionRecord>>,
    tasks: Mutex<HashMap<String, TaskRecord>>,
}

impl MemoryStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl TaskStore for MemoryStore {
    fn upsert_session(&self, session: &SessionRecord) -> Result<(), AgentError> {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.session_id.clone(), session.clone());
        Ok(())
    }
    fn insert_task(&self, task: &TaskRecord) -> Result<(), AgentError> {
        self.tasks
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), task.clone());
        Ok(())
    }
    fn submit_task(&self, task: &TaskRecord, session: &SessionRecord) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut tasks = self.tasks.lock().unwrap();
        sessions.insert(session.session_id.clone(), session.clone());
        tasks.insert(task.task_id.clone(), task.clone());
        Ok(())
    }
    fn update_task(&self, task: &TaskRecord) -> Result<(), AgentError> {
        self.tasks
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), task.clone());
        Ok(())
    }
    fn finish_task(&self, task: &TaskRecord, session: &SessionRecord) -> Result<(), AgentError> {
        self.upsert_session(session)?;
        self.update_task(task)
    }
    fn task(&self, task_id: &str) -> Result<Option<TaskRecord>, AgentError> {
        Ok(self.tasks.lock().unwrap().get(task_id).cloned())
    }
    fn session(&self, session_id: &str) -> Result<Option<SessionRecord>, AgentError> {
        Ok(self.sessions.lock().unwrap().get(session_id).cloned())
    }
    fn load(&self) -> Result<(Vec<SessionRecord>, Vec<TaskRecord>), AgentError> {
        let sessions = self.sessions.lock().unwrap();
        let tasks = self.tasks.lock().unwrap();
        let mut sessions: Vec<_> = sessions.values().cloned().collect();
        sessions.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let mut tasks: Vec<_> = tasks.values().cloned().collect();
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        for session in sessions.iter_mut() {
            session.task_ids = tasks
                .iter()
                .filter(|t| t.session_id == session.session_id)
                .map(|t| t.task_id.clone())
                .collect();
        }
        Ok((sessions, tasks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(dir: &std::path::Path) -> DbPool {
        crate::storage::db::init_database(&dir.to_path_buf()).expect("init db")
    }

    fn session(id: &str) -> SessionRecord {
        SessionRecord {
            session_id: id.into(),
            agent: "codex-native".into(),
            cwd: PathBuf::from("/tmp"),
            permission: PermissionMode::ReadOnly,
            thread_id: None,
            active_task_id: None,
            task_ids: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn task(id: &str, session_id: &str) -> TaskRecord {
        TaskRecord {
            task_id: id.into(),
            session_id: session_id.into(),
            prompt: "audit".into(),
            state: TaskState::Queued,
            thread_id: None,
            turn_id: None,
            final_answer: None,
            error: None,
            events: vec![],
            cancel_requested: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            finished_at: None,
        }
    }

    #[test]
    fn migration_creates_agent_tables_without_bumping_user_version() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path());
        let conn = pool.get().unwrap();
        for table in ["agent_sessions", "agent_tasks"] {
            let found: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table} missing");
        }
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        // 新增表不能 bump user_version：2.0.5 及更早版本看到更高版本会拒绝启动。
        assert_eq!(version, 12);
        // 幂等：再跑一次不能报错（升级路径与全新安装共用）。
        drop(conn);
        crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();
    }

    #[test]
    fn task_row_requires_an_existing_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::new(pool(dir.path()));
        let err = store
            .insert_task(&task("t1", "missing-session"))
            .expect_err("foreign key must reject orphan task");
        assert_eq!(err.code, ErrorCode::Storage);
    }

    #[test]
    fn session_and_task_round_trip_including_final_answer() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::new(pool(dir.path()));
        let mut s = session("s1");
        store.upsert_session(&s).unwrap();
        let mut t = task("t1", "s1");
        store.insert_task(&t).unwrap();

        s.thread_id = Some("thread-1".into());
        s.active_task_id = Some("t1".into());
        store.upsert_session(&s).unwrap();
        t.thread_id = Some("thread-1".into());
        t.turn_id = Some("turn-1".into());
        t.state = TaskState::Running;
        t.cancel_requested = true;
        t.events.push(TaskEvent {
            kind: "item/started".into(),
            message: "reading".into(),
        });
        store.update_task(&t).unwrap();

        t.state = TaskState::Completed;
        t.final_answer = Some("final answer\nwith newline".into());
        t.finished_at = Some("2026-01-01T00:01:00Z".into());
        let mut done_session = s.clone();
        done_session.active_task_id = None;
        store.finish_task(&t, &done_session).unwrap();

        let stored = store.task("t1").unwrap().unwrap();
        assert_eq!(stored.state, TaskState::Completed);
        assert_eq!(
            stored.final_answer.as_deref(),
            Some("final answer\nwith newline")
        );
        assert_eq!(stored.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(stored.thread_id.as_deref(), Some("thread-1"));
        assert!(stored.cancel_requested);
        assert_eq!(stored.events.len(), 1);
        assert_eq!(stored.finished_at.as_deref(), Some("2026-01-01T00:01:00Z"));

        let stored_session = store.session("s1").unwrap().unwrap();
        assert_eq!(stored_session.thread_id.as_deref(), Some("thread-1"));
        assert!(stored_session.active_task_id.is_none());
        assert_eq!(stored_session.task_ids, vec!["t1".to_string()]);
    }

    #[test]
    fn completed_result_survives_a_new_store_instance() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = SqliteStore::new(pool(dir.path()));
            let s = session("s1");
            store.upsert_session(&s).unwrap();
            let mut t = task("t1", "s1");
            store.insert_task(&t).unwrap();
            t.state = TaskState::Completed;
            t.final_answer = Some("durable answer".into());
            t.finished_at = Some("2026-01-01T00:01:00Z".into());
            store.finish_task(&t, &s).unwrap();
        }
        let reopened = SqliteStore::new(pool(dir.path()));
        let (sessions, tasks) = reopened.load().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, TaskState::Completed);
        assert_eq!(tasks[0].final_answer.as_deref(), Some("durable answer"));
        assert_eq!(sessions[0].task_ids, vec!["t1".to_string()]);
    }

    #[test]
    fn error_and_state_are_decoded_not_silently_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::new(pool(dir.path()));
        store.upsert_session(&session("s1")).unwrap();
        let mut t = task("t1", "s1");
        store.insert_task(&t).unwrap();
        t.state = TaskState::Failed;
        t.error = Some(AgentError::new(ErrorCode::Adapter, "boom"));
        store.update_task(&t).unwrap();
        let stored = store.task("t1").unwrap().unwrap();
        assert_eq!(stored.error.unwrap().code, ErrorCode::Adapter);

        // 手工写入非法 status：必须报错，而不是悄悄映射成别的状态。
        let conn = store.conn().unwrap();
        conn.execute(
            "UPDATE agent_tasks SET status='weird' WHERE task_id='t1'",
            [],
        )
        .unwrap();
        assert!(store.task("t1").is_err());
    }

    #[test]
    fn memory_store_loads_in_creation_order() {
        let store = MemoryStore::new();
        let mut s = session("s1");
        store.upsert_session(&s).unwrap();
        for (id, created) in [
            ("t2", "2026-01-01T00:00:02Z"),
            ("t1", "2026-01-01T00:00:01Z"),
        ] {
            let mut t = task(id, "s1");
            t.created_at = created.into();
            store.insert_task(&t).unwrap();
            s.task_ids.push(id.into());
        }
        let (_, tasks) = store.load().unwrap();
        assert_eq!(
            tasks.iter().map(|t| t.task_id.as_str()).collect::<Vec<_>>(),
            vec!["t1", "t2"]
        );
    }
}
