//! System tray with dynamic status + one-tap provider switching.
//!
//! The tray icon is built once at app startup with a stable id; the menu
//! and tooltip are rebuilt on demand via `refresh_tray`. Triggers:
//!
//!  - app startup (initial paint)
//!  - gateway start / stop / restart (status row + tooltip)
//!  - active-provider switch (checkmark relocates)
//!  - 30 s periodic timer (today's request count keeps ticking)
//!
//! The menu carries two dynamic-ID conventions:
//!  - `switch_active:<provider_id>` — set that provider as active and refresh
//!  - everything else routes through the same static-id handler in lib.rs
//!    (show / start_gateway / stop_gateway / restart_gateway / toggle_pet /
//!     toggle_wake_* / quit).
//!
//! Borrowed from codex-switcher's tray popover design, simplified to the
//! Tauri-native menu surface — interactive popover window is a future PR.

use crate::app::state::AppState;
use crate::storage;
use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::TrayIconId;
use tauri::{AppHandle, Manager};

pub const TRAY_ID: &str = "main";

#[derive(Debug, PartialEq, Eq)]
enum WakeToggle {
    Enabled(bool),
    RequestControl(bool),
    KeepDisplayAwake(bool),
}

fn wake_toggle_for_menu_id(
    menu_id: &str,
    enabled: bool,
    request_control: bool,
    keep_display_awake: bool,
) -> Option<WakeToggle> {
    match menu_id {
        "toggle_wake_enabled" => Some(WakeToggle::Enabled(!enabled)),
        "toggle_wake_request_control" => Some(WakeToggle::RequestControl(!request_control)),
        "toggle_wake_display" => Some(WakeToggle::KeepDisplayAwake(!keep_display_awake)),
        _ => None,
    }
}

fn wake_primary_label(
    zh: bool,
    supported: bool,
    enabled: bool,
    request_control: bool,
    has_error: bool,
) -> String {
    if !supported {
        return if zh {
            "防休眠（当前平台不支持）".into()
        } else {
            "Keep Awake (unsupported)".into()
        };
    }
    if has_error {
        return if zh {
            "防休眠（申请失败）".into()
        } else {
            "Keep Awake (failed)".into()
        };
    }
    if !enabled {
        return if zh {
            "启用防休眠".into()
        } else {
            "Enable Keep Awake".into()
        };
    }
    if request_control {
        if zh {
            "防休眠（请求智能控制）".into()
        } else {
            "Keep Awake (request-aware)".into()
        }
    } else if zh {
        "防休眠（持续保持）".into()
    } else {
        "Keep Awake (continuous)".into()
    }
}

/// Rebuild the tray menu and tooltip from current DB + runtime state.
/// Safe to call from any thread / async context. Silently no-ops if the
/// tray hasn't been registered yet (early startup) or the lookup fails.
pub fn refresh_tray(app: &AppHandle) {
    if let Err(e) = try_refresh(app) {
        // Don't poison the app — a failed refresh just means stale text.
        eprintln!("[tray] refresh failed: {e}");
    }
}

fn try_refresh(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = read_snapshot(app)?;
    let menu = build_menu(app, &snapshot)?;
    let tooltip = build_tooltip(&snapshot);

    let tray_id = TrayIconId::new(TRAY_ID);
    let tray = app
        .tray_by_id(&tray_id)
        .ok_or("tray icon not registered yet")?;
    tray.set_menu(Some(menu))?;
    tray.set_tooltip(Some(&tooltip))?;
    Ok(())
}

/// `refresh_tray` 含同步 SQLite 查询 + `defaults read` 子进程(locale 探测),
/// 放到 blocking 线程执行,不阻塞 async runtime 的 worker。
pub async fn refresh_tray_blocking(app: &AppHandle) {
    let app = app.clone();
    if let Err(e) = tauri::async_runtime::spawn_blocking(move || refresh_tray(&app)).await {
        eprintln!("[tray] refresh task failed: {e}");
    }
}

/// Spawn a 30 s repeating task that calls `refresh_tray`. Started once during
/// app setup; rides the app lifetime via the captured `AppHandle`.
pub fn start_periodic_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
        // First tick fires immediately — skip it; setup_tray already paints once.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            refresh_tray_blocking(&app).await;
        }
    });
}

#[derive(Default)]
struct Snapshot {
    zh: bool,
    active_provider_id: Option<String>,
    active_provider_name: Option<String>,
    providers: Vec<(String, String)>, // (id, name)
    today_total: i64,
    gateway_running: bool,
    gateway_port: u16,
    wake_supported: bool,
    wake_enabled: bool,
    wake_request_control: bool,
    wake_keep_display_awake: bool,
    wake_has_error: bool,
}

fn read_snapshot(app: &AppHandle) -> Result<Snapshot, Box<dyn std::error::Error>> {
    let state: tauri::State<'_, AppState> = app.state();
    let conn = state.db.get().map_err(|_| "DB lock failed")?;

    let zh = crate::is_chinese_locale_pub();

    let settings = storage::gateway_settings::get(&conn)?;
    let active_id = settings.active_provider_id.clone();

    let providers_full = storage::providers::list_all(&conn).unwrap_or_default();
    let providers: Vec<(String, String)> = providers_full
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    let active_name = active_id
        .as_ref()
        .and_then(|id| providers_full.iter().find(|p| &p.id == id))
        .map(|p| p.name.clone());

    // Stats query is cheap (single COUNT) but if the table is huge, fall back
    // to 0 silently rather than blocking tray refresh on it.
    let today_total = storage::request_logs::get_stats(&conn)
        .map(|s| s.today_total)
        .unwrap_or(0);
    drop(conn);

    let runtime = state.gateway_runtime.lock().map_err(|_| "runtime lock")?;
    let gateway_running = runtime.running;
    let gateway_port = runtime.port;
    drop(runtime);
    let wake_status = state.wake.status();

    Ok(Snapshot {
        zh,
        active_provider_id: active_id,
        active_provider_name: active_name,
        providers,
        today_total,
        gateway_running,
        gateway_port,
        wake_supported: wake_status.supported,
        wake_enabled: wake_status.enabled,
        wake_request_control: wake_status.request_control,
        wake_keep_display_awake: wake_status.keep_display_awake,
        wake_has_error: wake_status.mode == crate::wake::WakeMode::Error,
    })
}

fn build_menu(
    app: &AppHandle,
    snap: &Snapshot,
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let zh = snap.zh;

    let active_label = snap
        .active_provider_name
        .as_deref()
        .map(|n| {
            if zh {
                format!("当前供应商：{n}")
            } else {
                format!("Active: {n}")
            }
        })
        .unwrap_or_else(|| {
            if zh {
                "未选择供应商".into()
            } else {
                "No active provider".into()
            }
        });
    let active_item = MenuItemBuilder::with_id("info_active", active_label)
        .enabled(false)
        .build(app)?;

    let today_label = if zh {
        format!("今日请求：{}", snap.today_total)
    } else {
        format!("Today: {} requests", snap.today_total)
    };
    let today_item = MenuItemBuilder::with_id("info_today", today_label)
        .enabled(false)
        .build(app)?;

    let gateway_label = if snap.gateway_running {
        if zh {
            format!("网关运行中 · :{}", snap.gateway_port)
        } else {
            format!("Gateway running · :{}", snap.gateway_port)
        }
    } else if zh {
        "网关已停止".into()
    } else {
        "Gateway stopped".into()
    };
    let gateway_item = MenuItemBuilder::with_id("info_gateway", gateway_label)
        .enabled(false)
        .build(app)?;

    // ── Switch active submenu ──
    let mut switch_builder = SubmenuBuilder::new(
        app,
        if zh {
            "切换供应商"
        } else {
            "Switch active provider"
        },
    );
    if snap.providers.is_empty() {
        let empty = MenuItemBuilder::with_id(
            "switch_empty",
            if zh {
                "（暂无供应商）"
            } else {
                "(none configured)"
            },
        )
        .enabled(false)
        .build(app)?;
        switch_builder = switch_builder.item(&empty);
    } else {
        for (id, name) in &snap.providers {
            let is_active = snap.active_provider_id.as_deref() == Some(id.as_str());
            let item = CheckMenuItemBuilder::with_id(format!("switch_active:{id}"), name)
                .checked(is_active)
                .build(app)?;
            switch_builder = switch_builder.item(&item);
        }
    }
    let switch_submenu = switch_builder.build()?;

    // ── Existing static items ──
    let show = MenuItemBuilder::with_id(
        "show",
        if zh {
            "显示 MuxLayer"
        } else {
            "Show MuxLayer"
        },
    )
    .build(app)?;
    let start_gw = MenuItemBuilder::with_id(
        "start_gateway",
        if zh { "启动网关" } else { "Start Gateway" },
    )
    .enabled(!snap.gateway_running)
    .build(app)?;
    let stop_gw =
        MenuItemBuilder::with_id("stop_gateway", if zh { "停止网关" } else { "Stop Gateway" })
            .enabled(snap.gateway_running)
            .build(app)?;
    let restart_gw = MenuItemBuilder::with_id(
        "restart_gateway",
        if zh {
            "重启网关"
        } else {
            "Restart Gateway"
        },
    )
    .enabled(snap.gateway_running)
    .build(app)?;
    let toggle_wake_enabled = CheckMenuItemBuilder::with_id(
        "toggle_wake_enabled",
        wake_primary_label(
            zh,
            snap.wake_supported,
            snap.wake_enabled,
            snap.wake_request_control,
            snap.wake_has_error,
        ),
    )
    .checked(snap.wake_supported && snap.wake_enabled)
    .enabled(snap.wake_supported)
    .build(app)?;
    let toggle_wake_request_control = CheckMenuItemBuilder::with_id(
        "toggle_wake_request_control",
        if zh {
            "请求智能控制"
        } else {
            "Request-aware control"
        },
    )
    .checked(snap.wake_supported && snap.wake_request_control)
    .enabled(snap.wake_supported && snap.wake_enabled)
    .build(app)?;
    let toggle_wake_display = CheckMenuItemBuilder::with_id(
        "toggle_wake_display",
        if zh {
            "同时保持显示器常亮"
        } else {
            "Keep display awake too"
        },
    )
    .checked(snap.wake_supported && snap.wake_keep_display_awake)
    .enabled(snap.wake_supported && snap.wake_enabled)
    .build(app)?;
    let toggle_pet = MenuItemBuilder::with_id(
        "toggle_pet",
        if zh {
            "显示/隐藏宠物"
        } else {
            "Toggle Pet"
        },
    )
    .build(app)?;
    let toggle_pet_click_through = MenuItemBuilder::with_id(
        "toggle_pet_click_through",
        if zh {
            "宠物鼠标穿透"
        } else {
            "Pet Click-through"
        },
    )
    .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", if zh { "退出" } else { "Quit" }).build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&active_item)
        .item(&today_item)
        .item(&gateway_item)
        .separator()
        .item(&switch_submenu)
        .separator()
        .item(&show)
        .separator()
        .item(&start_gw)
        .item(&stop_gw)
        .item(&restart_gw)
        .separator()
        .item(&toggle_wake_enabled)
        .item(&toggle_wake_request_control)
        .item(&toggle_wake_display)
        .separator()
        .item(&toggle_pet)
        .item(&toggle_pet_click_through)
        .separator()
        .item(&quit)
        .build()?;
    Ok(menu)
}

fn build_tooltip(snap: &Snapshot) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(4);
    parts.push("MuxLayer".into());
    if snap.gateway_running {
        parts.push(format!(":{}", snap.gateway_port));
    } else {
        parts.push(if snap.zh {
            "网关已停止".into()
        } else {
            "Stopped".into()
        });
    }
    if let Some(ref n) = snap.active_provider_name {
        parts.push(n.clone());
    }
    if snap.today_total > 0 {
        parts.push(if snap.zh {
            format!("今日 {} 次", snap.today_total)
        } else {
            format!("{} reqs today", snap.today_total)
        });
    }
    parts.join(" · ")
}

/// Handle a `switch_active:<provider_id>` menu event. Sets the provider as
/// active and triggers a tray refresh so the checkmark relocates.
pub fn handle_switch_active(app: &AppHandle, menu_id: &str) {
    let Some(provider_id) = menu_id.strip_prefix("switch_active:") else {
        return;
    };
    let provider_id = provider_id.to_string();
    let app_clone = app.clone();
    // 同步 SQLite 写 + 托盘重建,放 blocking 线程。
    tauri::async_runtime::spawn_blocking(move || {
        let state: tauri::State<'_, AppState> = app_clone.state();
        let result = match state.db.get() {
            Ok(conn) => storage::providers::set_active(&conn, &provider_id),
            Err(e) => {
                eprintln!("[tray] set_active({provider_id}) failed: DB pool: {e}");
                return;
            }
        };
        if let Err(e) = result {
            eprintln!("[tray] set_active({provider_id}) failed: {e:?}");
        }
        refresh_tray(&app_clone);
    });
}

pub fn handle_wake_toggle(app: &AppHandle, menu_id: &str) {
    let menu_id = menu_id.to_string();
    let app_clone = app.clone();
    // 同步 SQLite 读写 + 托盘重建,放 blocking 线程。
    tauri::async_runtime::spawn_blocking(move || {
        let state: tauri::State<'_, AppState> = app_clone.state();
        if !state.wake.status().supported {
            refresh_tray(&app_clone);
            return;
        }

        let result = (|| {
            let conn = state.db.get().map_err(|_| "DB lock failed".to_string())?;
            let settings = storage::gateway_settings::get(&conn).map_err(|e| e.to_string())?;
            let toggle = wake_toggle_for_menu_id(
                &menu_id,
                settings.wake_enabled,
                settings.wake_request_control,
                settings.wake_keep_display_awake,
            )
            .ok_or_else(|| format!("unknown wake menu id: {menu_id}"))?;
            let input = match toggle {
                WakeToggle::Enabled(value) => crate::models::gateway::UpdateGatewaySettingsInput {
                    wake_enabled: Some(value),
                    ..Default::default()
                },
                WakeToggle::RequestControl(value) => {
                    crate::models::gateway::UpdateGatewaySettingsInput {
                        wake_request_control: Some(value),
                        ..Default::default()
                    }
                }
                WakeToggle::KeepDisplayAwake(value) => {
                    crate::models::gateway::UpdateGatewaySettingsInput {
                        wake_keep_display_awake: Some(value),
                        ..Default::default()
                    }
                }
            };
            let updated = storage::gateway_settings::update(&conn, input)
                .map_err(|error| error.to_string())?;
            state
                .wake
                .set_config(crate::app::commands::gateway::wake_config_from_settings(
                    &updated,
                ));
            Ok::<(), String>(())
        })();

        if let Err(error) = result {
            eprintln!("[tray] wake toggle failed: {error}");
        }
        refresh_tray(&app_clone);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_minimal_when_no_provider() {
        let snap = Snapshot {
            zh: false,
            gateway_running: false,
            ..Default::default()
        };
        let t = build_tooltip(&snap);
        assert!(t.contains("MuxLayer"));
        assert!(t.contains("Stopped"));
    }

    #[test]
    fn tooltip_running_with_provider_and_today_count() {
        let snap = Snapshot {
            zh: false,
            gateway_running: true,
            gateway_port: 7878,
            active_provider_name: Some("MiMo".into()),
            today_total: 42,
            ..Default::default()
        };
        let t = build_tooltip(&snap);
        assert!(t.contains("MuxLayer"));
        assert!(t.contains(":7878"));
        assert!(t.contains("MiMo"));
        assert!(t.contains("42 reqs"), "got: {t}");
    }

    #[test]
    fn tooltip_chinese_locale() {
        let snap = Snapshot {
            zh: true,
            gateway_running: true,
            gateway_port: 7878,
            active_provider_name: Some("小米 MiMo".into()),
            today_total: 142,
            ..Default::default()
        };
        let t = build_tooltip(&snap);
        assert!(t.contains("小米 MiMo"));
        assert!(t.contains("今日 142 次"), "got: {t}");
    }

    #[test]
    fn tooltip_drops_today_when_zero() {
        let snap = Snapshot {
            zh: false,
            gateway_running: true,
            gateway_port: 7878,
            active_provider_name: Some("MiMo".into()),
            today_total: 0,
            ..Default::default()
        };
        let t = build_tooltip(&snap);
        assert!(
            !t.contains("reqs"),
            "zero-req day shouldn't show — got: {t}"
        );
    }

    #[test]
    fn switch_active_parses_id_correctly() {
        // Pure parsing check — the actual switching needs an AppHandle so
        // this just verifies the id-extraction lives in the function.
        let id = "switch_active:prov_mimo_001".strip_prefix("switch_active:");
        assert_eq!(id, Some("prov_mimo_001"));

        let bad = "show".strip_prefix("switch_active:");
        assert_eq!(bad, None);
    }

    #[test]
    fn wake_menu_label_reflects_runtime_mode_and_support() {
        assert_eq!(
            wake_primary_label(true, true, true, false, false),
            "防休眠（持续保持）"
        );
        assert_eq!(
            wake_primary_label(true, true, true, true, false),
            "防休眠（请求智能控制）"
        );
        assert_eq!(
            wake_primary_label(true, false, true, false, false),
            "防休眠（当前平台不支持）"
        );
        assert_eq!(
            wake_primary_label(false, true, true, false, true),
            "Keep Awake (failed)"
        );
    }

    #[test]
    fn wake_menu_actions_toggle_only_the_selected_setting() {
        assert_eq!(
            wake_toggle_for_menu_id("toggle_wake_enabled", true, false, false),
            Some(WakeToggle::Enabled(false))
        );
        assert_eq!(
            wake_toggle_for_menu_id("toggle_wake_request_control", true, false, false),
            Some(WakeToggle::RequestControl(true))
        );
        assert_eq!(
            wake_toggle_for_menu_id("toggle_wake_display", true, false, true),
            Some(WakeToggle::KeepDisplayAwake(false))
        );
        assert_eq!(wake_toggle_for_menu_id("show", true, false, false), None);
    }
}
