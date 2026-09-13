//! 主动延迟探测的内存存储,喂 "fastest" 路由策略的冷启动。
//!
//! fastest 排序的主数据源是近 24h 真实请求延迟(request_logs);冷启动或
//! 长期闲置的 provider 没有记录,会被排到末尾。后台探测循环(server.rs)
//! 周期性发最小补全请求测延迟,结果存这里,排序时兜底。
//!
//! 探测发的是真实补全(会产生少量 token 费用),所以默认关:
//! 设 `AGENTGATE_LATENCY_PROBE_MINUTES=N` 才开启,且只在存在 fastest
//! 策略的路由档位时才真正探测。

use std::collections::HashMap;
use std::sync::Mutex;

/// 探测值的有效期:超过即视为过期,不参与排序兜底。
pub(crate) const PROBE_STALE_MS: u64 = 30 * 60 * 1000;

static STORE: Mutex<Option<HashMap<String, (f64, u64)>>> = Mutex::new(None);

fn with_store<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<String, (f64, u64)>) -> R,
{
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    f(guard.as_mut().unwrap())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 记录一次探测结果(provider 实例名 → 延迟毫秒)。
pub fn record(provider_name: &str, latency_ms: f64) {
    let now = now_ms();
    with_store(|m| {
        m.insert(provider_name.to_string(), (latency_ms, now));
    });
}

/// 取未过期的探测值快照。
pub fn snapshot(max_age_ms: u64) -> HashMap<String, f64> {
    let now = now_ms();
    with_store(|m| {
        m.retain(|_, (_, at)| now.saturating_sub(*at) <= max_age_ms);
        m.iter().map(|(k, (v, _))| (k.clone(), *v)).collect()
    })
}

/// fastest 排序的延迟解析:近 24h 真实请求延迟优先;没有(冷启动/闲置)
/// 用主动探测值兜底;两者都没有排末尾(f64::MAX,保持原有行为)。
pub fn resolve_latency(
    history: &HashMap<String, f64>,
    probes: &HashMap<String, f64>,
    provider_name: &str,
) -> f64 {
    history
        .get(provider_name)
        .or_else(|| probes.get(provider_name))
        .copied()
        .unwrap_or(f64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_then_snapshot_hits() {
        record("probe-p1", 123.0);
        let snap = snapshot(PROBE_STALE_MS);
        assert_eq!(snap.get("probe-p1"), Some(&123.0));
    }

    #[test]
    fn stale_entries_filtered_from_snapshot() {
        record("probe-stale", 50.0);
        // 回拨时间戳模拟过期(同 session_affinity 的测试手法)
        with_store(|m| {
            if let Some(e) = m.get_mut("probe-stale") {
                e.1 = now_ms() - PROBE_STALE_MS - 1000;
            }
        });
        let snap = snapshot(PROBE_STALE_MS);
        assert!(!snap.contains_key("probe-stale"), "过期探测值不应参与兜底");
    }

    #[test]
    fn resolve_prefers_history_then_probe_then_max() {
        let mut history = HashMap::new();
        let mut probes = HashMap::new();
        history.insert("a".to_string(), 100.0);
        probes.insert("a".to_string(), 999.0);
        probes.insert("b".to_string(), 200.0);
        // 真实请求延迟优先
        assert_eq!(resolve_latency(&history, &probes, "a"), 100.0);
        // 无历史 → 探测兜底
        assert_eq!(resolve_latency(&history, &probes, "b"), 200.0);
        // 都没有 → 垫底
        assert_eq!(resolve_latency(&history, &probes, "c"), f64::MAX);
    }
}
