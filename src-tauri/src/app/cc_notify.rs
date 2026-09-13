//! Claude Code 实时状态的本地接收端(文件方式,无端口)。
//!
//! CC 在状态转换时(提交指令 / 调工具 / 等授权 / 完成等)通过注入的多个 hook
//! 把事件原子写入约定信箱文件(见 tools::claude_code::set_cc_hook)。这里轮询该
//! 文件 mtime,变化即读内容,按事件类型推导出 working / waiting / done 三种状态,
//! 编码进 PetBubble 的 type(`cc-<status>`)发给宠物窗口,桌宠据此切动画 + 文案。
//! 用文件而非 HTTP 端口:彻底避免端口被占用导致提醒失效。

use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::{Duration, SystemTime};

use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event;

use crate::app::events::PetBubble;
use crate::tools::claude_code::cc_notify_file;

const POLL_INTERVAL: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CcStatus {
    Working,
    Waiting,
    Done,
}

impl CcStatus {
    fn as_pet_type(self) -> &'static str {
        match self {
            Self::Working => "cc-working",
            Self::Waiting => "cc-waiting",
            Self::Done => "cc-done",
        }
    }

    fn bubble_text(self) -> (&'static str, &'static str) {
        match self {
            Self::Waiting => ("Claude Code is waiting", "等你确认"),
            Self::Done => ("Claude Code finished", "这轮已完成"),
            Self::Working => ("Claude Code is working", "正在处理"),
        }
    }

    fn system_notification_body(self) -> Option<&'static str> {
        match self {
            Self::Waiting => Some("等你确认"),
            Self::Done => Some("这轮已完成"),
            Self::Working => None,
        }
    }
}

pub fn spawn(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let path = match cc_notify_file() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[cc-notify] 无法定位信箱文件,状态提醒不可用: {}", e.message);
                return;
            }
        };
        // 记录启动时 mtime,避免把启动前的旧通知误弹一次。
        let mut last = file_mtime(&path);
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let cur = file_mtime(&path);
            if cur != last {
                last = cur;
                if cur.is_some() {
                    handle_notify(&path, &app_handle);
                }
            }
        }
    });
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn handle_notify(path: &Path, app_handle: &tauri::AppHandle) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let v: serde_json::Value = serde_json::from_str(&content).unwrap_or(serde_json::Value::Null);

    // CC payload 字段:hook_event_name(兼容 hookEventName)+ notification_type。
    let event = v
        .get("hook_event_name")
        .or_else(|| v.get("hookEventName"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let ntype = v
        .get("notification_type")
        .and_then(|x| x.as_str())
        .unwrap_or("");

    let status = cc_status_from_payload(event, ntype);

    // 各状态文案(text_zh 给中文用户,text 给英文)。
    let (text, text_zh) = status.bubble_text();

    let bubble = PetBubble {
        text: text.to_string(),
        text_zh: Some(text_zh.to_string()),
        // 把状态编码进 type:前端解析 "cc-" 前缀,切动画 + 决定是否弹气泡。
        r#type: status.as_pet_type().to_string(),
    };
    send_system_notification(status, app_handle);
    // 宠物是独立 webview 窗口,定向发给 "pet"(全局 emit 收不到)。
    if let Err(e) = bubble.emit_to(app_handle, "pet") {
        eprintln!("[cc-notify] emit PetBubble failed: {e}");
    }
}

fn cc_status_from_payload(event: &str, notification_type: &str) -> CcStatus {
    match (event, notification_type) {
        ("Notification", "permission_prompt") => CcStatus::Waiting,
        ("Notification", "idle_prompt") => CcStatus::Done,
        ("Stop", _) => CcStatus::Done,
        ("PreToolUse", _) | ("UserPromptSubmit", _) => CcStatus::Working,
        _ => CcStatus::Working,
    }
}

fn should_send_system_notification(status: CcStatus) -> bool {
    status.system_notification_body().is_some()
}

fn send_system_notification(status: CcStatus, app_handle: &tauri::AppHandle) {
    if !should_send_system_notification(status) {
        return;
    };
    let Some(body) = status.system_notification_body() else {
        return;
    };

    let tauri_result = app_handle
        .notification()
        .builder()
        .title("Claude Code")
        .body(body)
        .show();

    #[cfg(target_os = "macos")]
    {
        // In `tauri dev`, the notification plugin routes through Terminal and
        // reports success before macOS actually displays anything. Keep a
        // dev-only AppleScript fallback so the hook remains testable locally.
        if tauri::is_dev() {
            if let Err(e) = send_macos_system_notification("Claude Code", body) {
                eprintln!("[cc-notify] dev fallback notification failed: {e}");
            }
            return;
        }
    }

    if let Err(e) = tauri_result {
        #[cfg(target_os = "macos")]
        {
            if send_macos_system_notification("Claude Code", body).is_ok() {
                return;
            }
        }
        eprintln!("[cc-notify] show system notification failed: {e}");
    }
}

#[cfg(target_os = "macos")]
fn send_macos_system_notification(title: &str, body: &str) -> std::io::Result<()> {
    let script = format!(
        "display notification {} with title {}",
        apple_script_quoted(body),
        apple_script_quoted(title)
    );
    let output = Command::new("osascript").arg("-e").arg(script).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

// 测试在所有平台都跑转义逻辑。
#[cfg(any(target_os = "macos", test))]
fn apple_script_quoted(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{apple_script_quoted, cc_status_from_payload, should_send_system_notification};

    #[test]
    fn sends_system_notification_only_when_user_attention_is_needed() {
        assert!(should_send_system_notification(cc_status_from_payload(
            "Notification",
            "permission_prompt"
        )));
        assert!(should_send_system_notification(cc_status_from_payload(
            "Notification",
            "idle_prompt"
        )));
        assert!(should_send_system_notification(cc_status_from_payload(
            "Stop", ""
        )));

        assert!(!should_send_system_notification(cc_status_from_payload(
            "PreToolUse",
            ""
        )));
        assert!(!should_send_system_notification(cc_status_from_payload(
            "UserPromptSubmit",
            ""
        )));
    }

    #[test]
    fn apple_script_notification_text_is_escaped() {
        assert_eq!(apple_script_quoted("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }

    #[test]
    fn system_notification_body_is_short_status_copy() {
        assert_eq!(
            cc_status_from_payload("Notification", "permission_prompt").system_notification_body(),
            Some("等你确认")
        );
        assert_eq!(
            cc_status_from_payload("Stop", "").system_notification_body(),
            Some("这轮已完成")
        );
    }
}
