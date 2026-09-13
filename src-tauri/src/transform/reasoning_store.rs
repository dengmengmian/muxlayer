//! Server-side store for DeepSeek reasoning_content.
//!
//! Codex doesn't pass reasoning_content back in subsequent requests.
//! We store it keyed by content hash and tool_call_id, then re-inject
//! it when converting the next request.
//!
//! Uses LRU-like eviction: each entry has an access counter, oldest entries
//! are evicted first when the store exceeds MAX_ENTRIES.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// (reasoning_content, last_access_counter)
static STORE: Mutex<Option<HashMap<String, (String, u64)>>> = Mutex::new(None);
static ACCESS_COUNTER: AtomicU64 = AtomicU64::new(0);

const MAX_ENTRIES: usize = 500;
const MAX_STORE_BYTES: usize = 16 * 1024 * 1024;

fn with_store<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<String, (String, u64)>) -> R,
{
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    f(guard.as_mut().unwrap())
}

fn next_counter() -> u64 {
    ACCESS_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn evict_reasoning(map: &mut HashMap<String, (String, u64)>) {
    let over_count = map.len() > MAX_ENTRIES;
    let total_bytes: usize = map.values().map(|(s, _)| s.len()).sum();
    if !over_count && total_bytes <= MAX_STORE_BYTES {
        return;
    }
    let mut entries: Vec<(String, u64, usize)> = map
        .iter()
        .map(|(k, (s, c))| (k.clone(), *c, s.len()))
        .collect();
    entries.sort_by_key(|(_, c, _)| *c);
    let mut remaining = total_bytes;
    for (k, _, bytes) in entries {
        if map.len() <= 1 {
            break;
        }
        if map.len() <= MAX_ENTRIES && remaining <= MAX_STORE_BYTES {
            break;
        }
        map.remove(&k);
        remaining = remaining.saturating_sub(bytes);
    }
}

fn content_hash(text: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    // Use two independent hash seeds to produce 128 bits, reducing collision probability
    let mut h1 = DefaultHasher::new();
    text.hash(&mut h1);
    let hash1 = h1.finish();
    let mut h2 = DefaultHasher::new();
    hash1.hash(&mut h2);
    text.hash(&mut h2);
    let hash2 = h2.finish();
    // Include text length as additional discriminator
    format!("{:016x}{:016x}_{}", hash1, hash2, text.len())
}

/// 模型分区键：去掉 `[1m]` 之类限定后缀并转小写，保证 store（响应侧）与
/// lookup（下一轮请求侧）拿到的是同一个上游模型 id。
///
/// 仍然存在的歧义（store/lookup 两侧都拿不到更多区分信息，需要会话级
/// plumbing 才能消除）：
/// - **同一模型**下，不同会话 / 不同客户端产出完全相同的 assistant 文本
///   （如 "Done."）仍会共用一条 reasoning；
/// - 同一模型名挂在不同 provider 下（例如两个 DeepSeek 兼容上游）不区分；
/// - tool_call id 由上游生成，一般全局唯一，但 Gemini 路径的
///   `call_gemini_{n}` 是按输出序号编号的，同模型跨会话可能重复。
fn model_scope(model: &str) -> String {
    crate::providers::model_id::strip_qualifier(model.trim()).to_ascii_lowercase()
}

fn content_key(model: &str, text: &str) -> String {
    format!("{}|{}", model_scope(model), content_hash(text))
}

fn tool_call_key(model: &str, tc_id: &str) -> String {
    format!("tc|{}|{tc_id}", model_scope(model))
}

/// Store reasoning_content keyed by (model, assistant text hash) and optionally
/// by (model, tool_call_id).
pub fn store(model: &str, text: &str, reasoning: &str, tool_call_ids: &[String]) {
    if reasoning.is_empty() {
        return;
    }
    let counter = next_counter();
    with_store(|map| {
        map.insert(content_key(model, text), (reasoning.to_string(), counter));

        for tc_id in tool_call_ids {
            map.insert(
                tool_call_key(model, tc_id),
                (reasoning.to_string(), counter),
            );
        }

        // 插入后再淘汰，保证任何时刻条目数都不超过 MAX_ENTRIES（含 tool_call 键）。
        evict_reasoning(map);
    });
}

/// Look up stored reasoning_content by assistant text.
pub fn lookup_by_content(model: &str, text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let key = content_key(model, text);
    let counter = next_counter();
    with_store(|map| {
        map.get_mut(&key).map(|(rc, c)| {
            *c = counter; // Update access time
            rc.clone()
        })
    })
}

/// Look up stored reasoning_content by tool_call_id.
pub fn lookup_by_tool_call_id(model: &str, tc_id: &str) -> Option<String> {
    let key = tool_call_key(model, tc_id);
    let counter = next_counter();
    with_store(|map| {
        map.get_mut(&key).map(|(rc, c)| {
            *c = counter;
            rc.clone()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::FS_LOCK;
    use std::sync::Mutex;

    // Global lock to prevent concurrent access to the static store during tests
    #[allow(dead_code)]
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    const M: &str = "deepseek-v4";

    fn clear_store() {
        with_store(|map| map.clear());
    }

    #[test]
    fn test_store_and_lookup_by_content() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        store(M, "hello", "thinking...", &[]);
        assert_eq!(
            lookup_by_content(M, "hello"),
            Some("thinking...".to_string())
        );
    }

    #[test]
    fn test_store_and_lookup_by_tool_call_id() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        store(M, "hello", "thinking...", &["tc1".to_string()]);
        assert_eq!(
            lookup_by_tool_call_id(M, "tc1"),
            Some("thinking...".to_string())
        );
    }

    #[test]
    fn test_same_text_different_model_does_not_cross_hit() {
        // "Done." 这类短文本跨会话/跨 provider 大量重复,不同模型之间不得串味。
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        store(
            "deepseek-v4",
            "Done.",
            "ds-reasoning",
            &["call_1".to_string()],
        );
        assert_eq!(lookup_by_content("mimo-v2.5-pro", "Done."), None);
        assert_eq!(lookup_by_tool_call_id("mimo-v2.5-pro", "call_1"), None);
        // [1m] 限定后缀与大小写不影响同一模型命中
        assert_eq!(
            lookup_by_content("DeepSeek-V4[1m]", "Done."),
            Some("ds-reasoning".to_string())
        );
    }

    #[test]
    fn test_store_cap_enforced_including_tool_call_keys() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        for i in 0..MAX_ENTRIES + 50 {
            store(
                M,
                &format!("t_{i}"),
                "rc",
                &[format!("a_{i}"), format!("b_{i}")],
            );
            let len = with_store(|map| map.len());
            assert!(len <= MAX_ENTRIES, "store grew to {len} > {MAX_ENTRIES}");
        }
        // 最新写入的仍可命中
        let last = MAX_ENTRIES + 49;
        assert!(lookup_by_content(M, &format!("t_{last}")).is_some());
        assert!(lookup_by_tool_call_id(M, &format!("b_{last}")).is_some());
    }

    #[test]
    fn test_lookup_empty_text() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        assert_eq!(lookup_by_content(M, ""), None);
    }

    #[test]
    fn test_store_empty_reasoning_skipped() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        store(M, "hello", "", &[]);
        assert_eq!(lookup_by_content(M, "hello"), None);
    }

    #[test]
    fn test_recent_entry_survives_eviction() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        // Fill store to trigger eviction
        for i in 0..MAX_ENTRIES + 20 {
            store(M, &format!("filler_{i}"), &format!("rc_{i}"), &[]);
        }
        // The most recent entry should still exist
        assert!(lookup_by_content(M, &format!("filler_{}", MAX_ENTRIES + 19)).is_some());
    }

    #[test]
    fn test_lru_eviction() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        // Fill store well beyond capacity
        for i in 0..MAX_ENTRIES + 100 {
            store(M, &format!("key_{i:04}"), &format!("rc_{i}"), &[]);
        }
        // Some oldest entries should have been evicted
        let mut found_old = false;
        let mut found_new = false;
        for i in 0..50 {
            if lookup_by_content(M, &format!("key_{i:04}")).is_none() {
                found_old = true;
            }
        }
        for i in MAX_ENTRIES..MAX_ENTRIES + 50 {
            if lookup_by_content(M, &format!("key_{i:04}")).is_some() {
                found_new = true;
            }
        }
        assert!(found_old, "Expected some old entries to be evicted");
        assert!(found_new, "Expected newer entries to still exist");
    }

    #[test]
    fn test_overwrite_existing_key() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        store(M, "same", "first", &[]);
        store(M, "same", "second", &[]);
        assert_eq!(lookup_by_content(M, "same"), Some("second".to_string()));
    }

    #[test]
    fn test_multiple_tool_call_ids() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        store(M, "text", "rc", &["tc1".to_string(), "tc2".to_string()]);
        assert_eq!(lookup_by_tool_call_id(M, "tc1"), Some("rc".to_string()));
        assert_eq!(lookup_by_tool_call_id(M, "tc2"), Some("rc".to_string()));
        assert_eq!(lookup_by_content(M, "text"), Some("rc".to_string()));
    }

    #[test]
    fn test_content_hash_includes_length() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Two strings that differ only in length should have different hashes
        let h1 = content_hash("hello");
        let h2 = content_hash("hello!");
        assert_ne!(h1, h2);
        // Hash includes length suffix
        assert!(h1.ends_with("_5"));
        assert!(h2.ends_with("_6"));
    }

    #[test]
    fn test_different_texts_produce_different_keys() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        store(M, "text_a", "reasoning_a", &[]);
        store(M, "text_b", "reasoning_b", &[]);
        assert_eq!(
            lookup_by_content(M, "text_a"),
            Some("reasoning_a".to_string())
        );
        assert_eq!(
            lookup_by_content(M, "text_b"),
            Some("reasoning_b".to_string())
        );
    }

    #[test]
    fn test_chinese_content_hash_no_panic() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_store();
        let chinese = "你好世界，这是一段中文测试内容";
        store(M, chinese, "中文推理", &[]);
        assert_eq!(lookup_by_content(M, chinese), Some("中文推理".to_string()));
    }
}
