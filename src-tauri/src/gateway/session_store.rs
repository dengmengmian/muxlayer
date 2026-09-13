//! Dual-layer session store: L1 in-memory + L2 SQLite.
//!
//! L1: HashMap with LRU eviction (1000 entries, access-counter based).
//! L2: SQLite file at ~/.agentgate/sessions.db (survives restarts).
//! L1 miss → L2 lookup → L1 warm-up.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

use crate::protocol::chat_completions::ChatMessage;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StoredTurn {
    pub messages: Vec<ChatMessage>,
    pub assistant_messages: Vec<ChatMessage>,
    pub response_id: String,
    pub reasoning_content: Option<String>,
    /// 序列化后的 JSON 字节数，用于按字节预算淘汰。
    approx_bytes: usize,
    access: u64,
}

static L1: Mutex<Option<HashMap<String, StoredTurn>>> = Mutex::new(None);
static COUNTER: AtomicU64 = AtomicU64::new(0);
const MAX_L1: usize = 1000;
/// L1 字节预算。长 agent 会话单轮历史可达数 MB，只按条数封顶会涨到
/// 数 GB；字节预算保证 L1 只是个有界缓存，miss 由 L2 兜底。
const MAX_L1_BYTES: usize = 64 * 1024 * 1024;

fn next() -> u64 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn with_l1<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<String, StoredTurn>) -> R,
{
    let mut g = L1.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    f(g.as_mut().unwrap())
}

/// 按字节预算淘汰：超出预算时从最久未访问的条目开始删，至少保留一条
/// （刚插入的条目 access 最大，永远最后才轮到）。被淘汰的条目 L2 仍有，
/// miss 时回填。
fn evict_to_byte_budget(map: &mut HashMap<String, StoredTurn>, max_bytes: usize) {
    let mut total: usize = map.values().map(|t| t.approx_bytes).sum();
    if total <= max_bytes {
        return;
    }
    let mut entries: Vec<(String, u64, usize)> = map
        .iter()
        .map(|(k, v)| (k.clone(), v.access, v.approx_bytes))
        .collect();
    entries.sort_by_key(|(_, access, _)| *access);
    for (k, _, bytes) in entries {
        if total <= max_bytes || map.len() <= 1 {
            break;
        }
        map.remove(&k);
        total -= bytes;
    }
}

/// Get or create the L2 SQLite connection path.
fn l2_path() -> std::path::PathBuf {
    crate::security::local_token::token_dir().join("sessions.db")
}

static L2: Mutex<Option<rusqlite::Connection>> = Mutex::new(None);
static LAST_L2_CLEANUP: AtomicU64 = AtomicU64::new(0);

fn with_l2<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&rusqlite::Connection) -> R,
{
    let mut g = L2.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = open_l2();
    }
    g.as_ref().map(f)
}

fn open_l2() -> Option<rusqlite::Connection> {
    let path = l2_path();
    let _ = std::fs::create_dir_all(path.parent()?);
    let conn = rusqlite::Connection::open(&path).ok()?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .ok()?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            response_id TEXT PRIMARY KEY,
            data TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at);",
    )
    .ok()?;
    Some(conn)
}

/// Store a completed turn in L1 + L2.
pub fn store_turn(
    response_id: &str,
    messages: Vec<ChatMessage>,
    assistant_messages: Vec<ChatMessage>,
    reasoning_content: Option<String>,
) {
    // 只序列化一次：既给 L2 持久化，也作为 L1 字节预算的依据。
    let data_str = serde_json::json!({
        "messages": &messages,
        "assistant_messages": &assistant_messages,
        "reasoning_content": &reasoning_content,
    })
    .to_string();

    let turn = StoredTurn {
        messages,
        assistant_messages,
        response_id: response_id.to_string(),
        reasoning_content,
        approx_bytes: data_str.len(),
        access: next(),
    };

    // L1 store with LRU eviction
    with_l1(|map| {
        if map.len() >= MAX_L1 {
            let mut entries: Vec<(String, u64)> =
                map.iter().map(|(k, v)| (k.clone(), v.access)).collect();
            entries.sort_by_key(|(_, a)| *a);
            for (k, _) in entries.into_iter().take(MAX_L1 / 4) {
                map.remove(&k);
            }
        }
        map.insert(response_id.to_string(), turn);
        evict_to_byte_budget(map, MAX_L1_BYTES);
    });

    // L2 persist (best effort)。L2 是全局 Mutex + 同步 SQLite,流式任务在 tokio
    // worker 上直接写会卡住 worker;在 runtime 里就挪到 blocking 线程池,同步调用方
    // (CLI / 单测)就地写。L1 已同步写入,紧接着的 previous_response_id 查询不受影响。
    let response_id = response_id.to_string();
    let persist = move || {
        let _ = with_l2(|conn| {
            if let Err(e) = conn.execute(
                "INSERT OR REPLACE INTO sessions (response_id, data, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![response_id, data_str, chrono::Utc::now().to_rfc3339()],
            ) {
                tracing::warn!(error = %e, "session_store L2 persist failed");
            }
            maybe_cleanup_l2(conn);
        });
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(persist);
        }
        Err(_) => persist(),
    }
}

fn maybe_cleanup_l2(conn: &rusqlite::Connection) {
    let now = chrono::Utc::now().timestamp() as u64;
    let last = LAST_L2_CLEANUP.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 300 {
        return;
    }
    if LAST_L2_CLEANUP
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let _ = conn.execute("DELETE FROM sessions WHERE created_at < ?1", [&cutoff]);
}

/// Look up a previous turn. L1 first, then L2 fallback.
pub fn get_history(previous_response_id: &str) -> Option<Vec<ChatMessage>> {
    // L1 lookup
    let l1_hit = with_l1(|map| {
        if let Some(turn) = map.get_mut(previous_response_id) {
            turn.access = next();
            let mut h = turn.messages.clone();
            h.extend(turn.assistant_messages.clone());
            Some(h)
        } else {
            None
        }
    });

    if l1_hit.is_some() {
        return l1_hit;
    }

    // L2 lookup
    let data_str: String = with_l2(|conn| {
        conn.query_row(
            "SELECT data FROM sessions WHERE response_id = ?1",
            [previous_response_id],
            |row| row.get(0),
        )
        .ok()
    })??;

    let data: serde_json::Value = serde_json::from_str(&data_str).ok()?;
    let messages: Vec<ChatMessage> = serde_json::from_value(data.get("messages")?.clone()).ok()?;
    let asst: Vec<ChatMessage> =
        serde_json::from_value(data.get("assistant_messages")?.clone()).ok()?;
    let rc = data
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Warm L1
    with_l1(|map| {
        map.insert(
            previous_response_id.to_string(),
            StoredTurn {
                messages: messages.clone(),
                assistant_messages: asst.clone(),
                response_id: previous_response_id.to_string(),
                reasoning_content: rc,
                approx_bytes: data_str.len(),
                access: next(),
            },
        );
        evict_to_byte_budget(map, MAX_L1_BYTES);
    });

    let mut history = messages;
    history.extend(asst);
    Some(history)
}

#[cfg(test)]
fn l1_total_bytes() -> usize {
    with_l1(|map| map.values().map(|t| t.approx_bytes).sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::chat_completions::ChatMessage;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};
    use serde_json::json;

    fn clear_l1() {
        with_l1(|map| map.clear());
    }

    #[test]
    fn test_l1_evicts_by_byte_budget() {
        // 长 agent 会话单轮历史可达数 MB，L1 只按条数封顶会涨到数 GB。
        // 塞 10 条 8MB（共 ~80MB > 64MB 预算），字节总量必须回到预算内，
        // 且淘汰旧条目、保留新条目。
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_l1();
        let temp = setup_temp_home();
        let big = "x".repeat(8 * 1024 * 1024);
        for i in 0..10 {
            let msgs = vec![ChatMessage {
                role: "user".to_string(),
                content: Some(json!(big.clone())),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }];
            store_turn(&format!("resp_big_{i}"), msgs, vec![], None);
        }
        let total = l1_total_bytes();
        assert!(
            total <= MAX_L1_BYTES,
            "L1 字节总量 {total} 超出预算 {MAX_L1_BYTES}"
        );
        with_l1(|map| {
            assert!(map.contains_key("resp_big_9"), "最新条目不应被淘汰");
            assert!(!map.contains_key("resp_big_0"), "最旧条目应先被淘汰");
        });
        cleanup(&temp);
    }

    fn dummy_messages() -> (Vec<ChatMessage>, Vec<ChatMessage>) {
        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: Some(json!("hello")),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        let asst = vec![ChatMessage {
            role: "assistant".to_string(),
            content: Some(json!("hi")),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        (msgs, asst)
    }

    #[test]
    fn test_store_and_get_history_l1() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_l1();
        let temp = setup_temp_home();
        let (msgs, asst) = dummy_messages();
        store_turn("resp_1", msgs.clone(), asst.clone(), Some("rc".to_string()));
        let history = get_history("resp_1").unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
        cleanup(&temp);
    }

    #[tokio::test]
    async fn store_turn_in_runtime_writes_l1_now_and_l2_off_worker() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_l1();
        let temp = setup_temp_home();
        let (msgs, asst) = dummy_messages();
        store_turn("resp_runtime", msgs, asst, None);
        assert!(
            get_history("resp_runtime").is_some(),
            "L1 must be written before store_turn returns"
        );

        // L2 在 blocking 线程池里落盘;清掉 L1 后应能从 L2 读回。
        clear_l1();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while get_history("resp_runtime").is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "L2 persist never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        cleanup(&temp);
    }

    #[test]
    fn test_get_history_not_found() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_l1();
        let temp = setup_temp_home();
        // Ensure L2 file does not exist so get_history truly returns None
        let l2 = l2_path();
        let _ = std::fs::remove_file(&l2);
        assert!(get_history("resp_nonexistent").is_none());
        cleanup(&temp);
    }

    #[test]
    fn test_l2_persistence() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_l1();
        let temp = setup_temp_home();
        let (msgs, asst) = dummy_messages();
        store_turn(
            "resp_persistent",
            msgs.clone(),
            asst.clone(),
            Some("rc2".to_string()),
        );

        // Clear L1 to force L2 lookup
        clear_l1();
        // L2 should return the stored turn and warm L1
        let history = get_history("resp_persistent").unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, Some(json!("hello")));
        assert_eq!(history[1].content, Some(json!("hi")));

        // Now L1 should be warmed
        let history2 = get_history("resp_persistent").unwrap();
        assert_eq!(history2.len(), 2);
        cleanup(&temp);
    }

    #[test]
    fn test_store_turn_overwrites() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_l1();
        let temp = setup_temp_home();
        let (msgs, asst) = dummy_messages();
        store_turn(
            "resp_same",
            msgs.clone(),
            asst.clone(),
            Some("first".to_string()),
        );
        store_turn(
            "resp_same",
            msgs.clone(),
            asst.clone(),
            Some("second".to_string()),
        );
        let history = get_history("resp_same").unwrap();
        assert_eq!(history[1].content, Some(json!("hi")));
        assert_eq!(history.len(), 2);
        cleanup(&temp);
    }
}
