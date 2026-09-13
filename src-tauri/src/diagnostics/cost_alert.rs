//! 成本预警:后台周期检查今日花费,超过用户设的阈值时通知一次(当天去重)。
//!
//! 复用现成基建:今日花费来自 `request_logs::get_stats().today_cost`,通知走系统通知
//! 插件 + 桌宠气泡(同 cc_notify)。"上次预警日期"用进程内内存态去重——重启后当天若已
//! 超阈值会再提醒一次,无害,换来配置表不掺运行时状态。

use std::time::Duration;

use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event;

use crate::app::events::PetBubble;
use crate::storage::db::DbPool;

/// 启动延迟——给网关和数据库初始化留时间。
const STARTUP_DELAY: Duration = Duration::from_secs(60);
/// 检查周期。成本预警不需要秒级敏感,30 分钟足够。
const INTERVAL: Duration = Duration::from_secs(1800);

/// 是否应当发预警。纯函数,便于测试。
/// - `enabled`:预警开关
/// - `threshold`:今日花费阈值(USD)。None / <= 0 视为未设
/// - `current_cost`:今日已花费(USD)
/// - `last_alert_date`:上次预警日期(YYYY-MM-DD),内存态
/// - `today`:今天(YYYY-MM-DD)
fn should_alert(
    enabled: bool,
    threshold: Option<f64>,
    current_cost: f64,
    last_alert_date: Option<&str>,
    today: &str,
) -> bool {
    if !enabled {
        return false;
    }
    let Some(t) = threshold else {
        return false;
    };
    if t <= 0.0 {
        return false;
    }
    if current_cost < t {
        return false;
    }
    // 当天已提醒过则不重复;跨天后 last != today 自然重新放行。
    last_alert_date != Some(today)
}

pub fn spawn(db: DbPool, app_handle: tauri::AppHandle) {
    crate::runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        let mut last_alert_date: Option<String> = None;
        loop {
            run_once(&db, &app_handle, &mut last_alert_date);
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// 去重用的"今天"(YYYY-MM-DD)。与 `today_cost` 同口径按本地日,否则东八区
/// 00:00–08:00 超阈值时会被误判成"昨天已提醒过"。时区可注入便于测试。
fn alert_day<Tz: chrono::TimeZone>(now: chrono::DateTime<chrono::Utc>, tz: &Tz) -> String {
    now.with_timezone(tz)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

fn run_once(db: &DbPool, app_handle: &tauri::AppHandle, last_alert_date: &mut Option<String>) {
    let today = alert_day(chrono::Utc::now(), &chrono::Local);

    // 短暂持锁读配置 + 今日花费,锁外再发通知。
    let (enabled, threshold, today_cost) = {
        let conn = match db.get() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[cost-alert] db get failed: {e}");
                return;
            }
        };
        let settings = match crate::storage::gateway_settings::get(&conn) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[cost-alert] read settings failed: {e}");
                return;
            }
        };
        if !settings.cost_alert_enabled {
            return;
        }
        let stats = match crate::storage::request_logs::get_stats(&conn) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[cost-alert] read stats failed: {e}");
                return;
            }
        };
        (
            settings.cost_alert_enabled,
            settings.cost_alert_threshold,
            stats.today_cost,
        )
    };

    if should_alert(
        enabled,
        threshold,
        today_cost,
        last_alert_date.as_deref(),
        &today,
    ) {
        notify(app_handle, today_cost, threshold.unwrap_or(0.0));
        *last_alert_date = Some(today);
    }
}

fn notify(app_handle: &tauri::AppHandle, cost: f64, threshold: f64) {
    let body_zh = format!("今日 AI 花费 ${cost:.2} 已超预警阈值 ${threshold:.2}");
    let body_en =
        format!("Today's AI spend ${cost:.2} exceeded your alert threshold ${threshold:.2}");

    let _ = app_handle
        .notification()
        .builder()
        .title("MuxLayer")
        .body(&body_zh)
        .show();

    let bubble = PetBubble {
        text: body_en,
        text_zh: Some(body_zh),
        r#type: "error".to_string(),
    };
    if let Err(e) = bubble.emit_to(app_handle, "pet") {
        eprintln!("[cost-alert] emit bubble failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-06-22";

    #[test]
    fn disabled_never_alerts() {
        assert!(!should_alert(false, Some(1.0), 999.0, None, TODAY));
    }

    #[test]
    fn no_threshold_no_alert() {
        assert!(!should_alert(true, None, 999.0, None, TODAY));
    }

    #[test]
    fn zero_or_negative_threshold_no_alert() {
        assert!(!should_alert(true, Some(0.0), 999.0, None, TODAY));
        assert!(!should_alert(true, Some(-5.0), 999.0, None, TODAY));
    }

    #[test]
    fn below_threshold_no_alert() {
        assert!(!should_alert(true, Some(10.0), 9.99, None, TODAY));
    }

    #[test]
    fn at_or_above_threshold_alerts() {
        assert!(should_alert(true, Some(10.0), 10.0, None, TODAY));
        assert!(should_alert(true, Some(10.0), 50.0, None, TODAY));
    }

    #[test]
    fn same_day_no_repeat() {
        assert!(!should_alert(true, Some(10.0), 50.0, Some(TODAY), TODAY));
    }

    #[test]
    fn alert_day_uses_local_calendar_day() {
        // today_cost 按本地日统计,去重 key 必须同口径:UTC+8 的 04:00 已是次日。
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2026, 6, 21, 20, 0, 0).unwrap();
        let east8 = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(alert_day(now, &east8), "2026-06-22");
        let west5 = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        let early = chrono::Utc.with_ymd_and_hms(2026, 6, 22, 2, 0, 0).unwrap();
        assert_eq!(alert_day(early, &west5), "2026-06-21");
    }

    #[test]
    fn new_day_alerts_again() {
        assert!(should_alert(
            true,
            Some(10.0),
            50.0,
            Some("2026-06-21"),
            TODAY
        ));
    }
}
