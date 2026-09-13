//! 重启 Codex Desktop 让新写的 config.toml / auth.json 生效。
//!
//! 2026-07 起 Codex 桌面端并入 ChatGPT 桌面应用（主进程名 "ChatGPT"），
//! 旧独立 Codex.app 仍可能存在。
//!
//! "重启" = 杀掉 **Codex** 桌面 App → 等一会 → 重新拉起。如果本来没在跑，就直接拉起。
//! 不会动小写 `codex` CLI 二进制（pkill -x 精确匹配 basename，大小写敏感）。
//!
//! **绝不结束 ChatGPT 桌面 app**:用户可能正在里面对话 / 跑任务,强杀会丢状态。
//! 如果检测到 ChatGPT 在运行,只在结果里置 `chatgpt_needs_manual_restart = true`,
//! 由 UI 提示用户自己重启。
//!
//! 支持 macOS 和 Windows。Windows 用 `taskkill /IM Codex.exe /F` 按映像名
//! 精确匹配（注意：Windows 映像名匹配大小写不敏感，若 CLI 也以 codex.exe
//! 进程名运行会被一并杀掉——npm 版 CLI 实际跑在 node.exe 下，不受影响），
//! 再从常见安装目录拉起。Linux Codex Desktop 没有官方包，直接 supported=false。

use serde::Serialize;

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CodexRestartResult {
    /// 本平台是否实现了重启路径。false 表示前端不该显示按钮。
    pub supported: bool,
    pub platform: String,
    /// kill 前桌面 App 是不是在跑。
    pub was_running: bool,
    /// 实际杀掉的进程数（macOS 上 pkill 一发一组，记 1 即可）。
    pub killed: u32,
    /// 是否成功重新拉起。
    pub relaunched: bool,
    /// ChatGPT 桌面 app(内嵌 Codex)正在运行:我们不会结束它,需要用户手动重启
    /// 才能让新配置生效。
    pub chatgpt_needs_manual_restart: bool,
}

/// Codex 独立桌面 App 的进程 / 应用名。
const CODEX_APP_NAME: &str = "Codex";
/// 合并后内嵌 Codex 的 ChatGPT 桌面 App,只探测、不结束。
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const CHATGPT_APP_NAME: &str = "ChatGPT";

/// 命令计划所针对的平台(与编译目标解耦,便于在任意平台单测命令构造)。
// 单一平台构建里另一个变体不会被构造。
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanOs {
    MacOs,
    Windows,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedCommand {
    program: &'static str,
    args: Vec<String>,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl PlannedCommand {
    fn new(program: &'static str, args: &[&str]) -> Self {
        Self {
            program,
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// 重启要执行的命令(纯函数):只针对 Codex。Windows 拉起路径依赖安装目录探测,
/// 在 `restart_windows` 里处理,这里 relaunch 为空。
// Windows 构建不读 relaunch(拉起靠安装目录探测)。
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
struct RestartPlan {
    kill: Vec<PlannedCommand>,
    relaunch: Vec<PlannedCommand>,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn restart_plan(os: PlanOs) -> RestartPlan {
    match os {
        PlanOs::MacOs => RestartPlan {
            kill: vec![PlannedCommand::new("pkill", &["-x", CODEX_APP_NAME])],
            relaunch: vec![PlannedCommand::new("open", &["-a", CODEX_APP_NAME])],
        },
        PlanOs::Windows => RestartPlan {
            kill: vec![PlannedCommand::new(
                "taskkill",
                &["/IM", &format!("{CODEX_APP_NAME}.exe"), "/F"],
            )],
            relaunch: Vec::new(),
        },
    }
}

/// 阻塞(含 1 秒等待),调用方需在 blocking 线程执行。
pub fn restart() -> Result<CodexRestartResult, AppError> {
    #[cfg(target_os = "macos")]
    {
        Ok(restart_macos())
    }
    #[cfg(target_os = "windows")]
    {
        Ok(restart_windows())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(CodexRestartResult {
            supported: false,
            platform: std::env::consts::OS.to_string(),
            was_running: false,
            killed: 0,
            relaunched: false,
            chatgpt_needs_manual_restart: false,
        })
    }
}

/// ChatGPT 桌面 app 是否在跑(精确 basename / 映像名匹配,见 process_detect)。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn chatgpt_running() -> bool {
    !crate::tools::process_detect::find_running(&[CHATGPT_APP_NAME]).is_empty()
}

#[cfg(target_os = "macos")]
fn restart_macos() -> CodexRestartResult {
    use std::process::Command;

    let plan = restart_plan(PlanOs::MacOs);
    let run = |cmd: &PlannedCommand| {
        Command::new(cmd.program)
            .args(&cmd.args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    // pkill -x 严格按 basename 精确匹配:大写 "Codex" 只匹到桌面 App 主进程。
    // 退码 0 = 至少杀掉一个;其它都按"本来就没跑"处理。
    let killed = plan.kill.iter().filter(|cmd| run(cmd)).count() as u32;
    let was_running = killed > 0;
    if was_running {
        // 给桌面 App 关窗口、写盘的时间。1000ms 是实测值。
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
    let relaunched = plan.relaunch.iter().all(run);

    CodexRestartResult {
        supported: true,
        platform: "macos".to_string(),
        was_running,
        killed,
        relaunched,
        chatgpt_needs_manual_restart: chatgpt_running(),
    }
}

/// Windows 版与 macOS 同语义：杀掉 Codex 桌面 App → 等 1 秒 → 重新拉起。
/// 拉起靠枚举常见安装路径找 Codex.exe；都不存在则 relaunched=false，
/// 让前端提示用户手动启动（不假装成功）。
#[cfg(target_os = "windows")]
fn restart_windows() -> CodexRestartResult {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    // CREATE_NO_WINDOW：避免 GUI 进程下 spawn taskkill 弹出黑色控制台窗。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let plan = restart_plan(PlanOs::Windows);
    let mut killed = 0u32;
    // /IM 按映像名匹配，/F 强杀。退码 0 = 至少杀掉一个；128 = 没匹到；
    // 其他 = taskkill 缺失或权限不足，都按「本来就没跑」处理（同 macOS pkill）。
    for cmd in &plan.kill {
        if let Ok(status) = Command::new(cmd.program)
            .args(&cmd.args)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
        {
            if taskkill_killed(status.code()) {
                killed += 1;
            }
        }
    }
    let was_running = killed > 0;
    if was_running {
        // 给桌面 App 释放文件句柄、写盘的时间，与 macOS 路径一致。
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }

    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let relaunched = windows_codex_exe_candidates(&local_app_data)
        .into_iter()
        .find(|p| p.exists())
        .map(|exe| Command::new(exe).spawn().is_ok())
        .unwrap_or(false);

    CodexRestartResult {
        supported: true,
        platform: "windows".to_string(),
        was_running,
        killed,
        relaunched,
        chatgpt_needs_manual_restart: chatgpt_running(),
    }
}

/// Windows：解释 `taskkill /IM Codex.exe /F` 的退出码。
/// 0 = 至少杀掉一个；128 = 没有匹配的进程；其他（taskkill 缺失、权限不足等）
/// 一律按「本来就没跑」处理——与 macOS pkill 的容错语义对齐。
#[cfg(any(windows, test))]
fn taskkill_killed(code: Option<i32>) -> bool {
    code == Some(0)
}

/// Windows：Codex 桌面 App 可执行文件的候选安装路径（按优先级）。
/// 传入 %LOCALAPPDATA%；为空时返回空列表（让调用方按「拉起失败」处理）。
/// 只包含 Codex 自己,不会去拉起 / 关联 ChatGPT。
fn windows_codex_exe_candidates(local_app_data: &str) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let base = local_app_data.trim();
    if base.is_empty() {
        return Vec::new();
    }
    let base = PathBuf::from(base);
    let exe = format!("{CODEX_APP_NAME}.exe");
    vec![
        // NSIS 风格安装目录（Claude Desktop 等同类 App 的常见位置）
        base.join("Programs").join(CODEX_APP_NAME).join(&exe),
        // Squirrel 风格安装目录
        base.join(CODEX_APP_NAME).join(&exe),
    ]
}

/// 「重启 Codex」按钮是否可用(同步、只做 stat,不 spawn 进程)。前端据此决定显隐。
pub fn desktop_available() -> bool {
    let home = crate::fsutil::home_dir().ok();
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    desktop_available_with(
        std::env::consts::OS,
        home.as_deref(),
        &local_app_data,
        &|p| p.exists(),
    )
}

/// 纯判定:按运行时平台名选候选路径,任一存在即可用。平台用字符串而非 cfg,
/// 所有平台都编译同一份逻辑(Linux 构建不会出现只在 macOS/Windows 使用的死代码)。
/// - macOS:`/Applications/Codex.app` 或 `~/Applications/Codex.app`(重启用 `open -a Codex`);
/// - Windows:重启拉起时使用的 `Codex.exe` 候选路径;
/// - 其它:没有官方桌面包,false。
fn desktop_available_with(
    os: &str,
    home: Option<&std::path::Path>,
    local_app_data: &str,
    exists: &dyn Fn(&std::path::Path) -> bool,
) -> bool {
    let candidates = match os {
        "macos" => {
            let app = format!("{CODEX_APP_NAME}.app");
            let mut c = vec![std::path::Path::new("/Applications").join(&app)];
            if let Some(home) = home {
                c.push(home.join("Applications").join(&app));
            }
            c
        }
        "windows" => windows_codex_exe_candidates(local_app_data),
        _ => Vec::new(),
    };
    candidates.iter().any(|p| exists(p))
}

// macOS / Windows 路径会真的 kill + 拉起 Codex Desktop，跑测试会误伤开发者
// 正在用的 App，所以只在 restart() 为 no-op 桩的平台上执行。
#[cfg(all(test, not(any(target_os = "macos", target_os = "windows"))))]
mod tests {
    use super::*;

    #[test]
    fn restart_is_unsupported_on_other_platforms() {
        let r = restart().unwrap();
        assert!(!r.supported);
        assert_eq!(r.killed, 0);
        assert!(!r.relaunched);
        assert!(!r.chatgpt_needs_manual_restart);
    }
}

// 命令构造 / Windows 纯逻辑（退出码解释、候选路径），平台无关，macOS 上也可跑。
#[cfg(test)]
mod windows_logic_tests {
    use super::*;

    /// 回归:「重启 Codex」曾经顺带 `pkill -x ChatGPT` / `taskkill /IM ChatGPT.exe /F`,
    /// 把用户正在用的 ChatGPT 桌面 app 一起杀掉。现在命令计划里只能出现 Codex。
    #[test]
    fn restart_plan_only_targets_codex() {
        for os in [PlanOs::MacOs, PlanOs::Windows] {
            let plan = restart_plan(os);
            for cmd in plan.kill.iter().chain(plan.relaunch.iter()) {
                assert!(
                    !cmd.args.iter().any(|a| a.contains(CHATGPT_APP_NAME)),
                    "{os:?} must never touch ChatGPT: {cmd:?}"
                );
            }
        }
        let mac = restart_plan(PlanOs::MacOs);
        assert_eq!(
            mac.kill,
            vec![PlannedCommand::new("pkill", &["-x", "Codex"])]
        );
        assert_eq!(
            mac.relaunch,
            vec![PlannedCommand::new("open", &["-a", "Codex"])]
        );
        let win = restart_plan(PlanOs::Windows);
        assert_eq!(
            win.kill,
            vec![PlannedCommand::new("taskkill", &["/IM", "Codex.exe", "/F"])]
        );
        assert!(win.relaunch.is_empty());
    }

    /// 「重启 Codex」按钮是否可用:macOS 看 /Applications 或 ~/Applications 下的
    /// Codex.app(与 `open -a Codex` 对应),Windows 看重启拉起用的 Codex.exe 候选路径,
    /// 其它平台一律 false。
    #[test]
    fn desktop_available_decision_per_platform() {
        use std::path::{Path, PathBuf};
        let home = Path::new("/Users/me");
        let only = |hit: PathBuf| move |p: &Path| p == hit;

        let sys = only(PathBuf::from("/Applications/Codex.app"));
        assert!(desktop_available_with("macos", Some(home), "", &sys));
        let user = only(home.join("Applications").join("Codex.app"));
        assert!(desktop_available_with("macos", Some(home), "", &user));
        let chatgpt = only(PathBuf::from("/Applications/ChatGPT.app"));
        assert!(!desktop_available_with("macos", Some(home), "", &chatgpt));
        assert!(desktop_available_with("macos", None, "", &sys));

        let lad = r"C:\Users\me\AppData\Local";
        let win_exe = windows_codex_exe_candidates(lad)[1].clone();
        assert!(desktop_available_with("windows", None, lad, &only(win_exe)));
        assert!(!desktop_available_with("windows", None, "", &|_: &Path| {
            true
        }));
        assert!(!desktop_available_with(
            "windows",
            None,
            lad,
            &|_: &Path| false
        ));

        assert!(!desktop_available_with(
            "linux",
            Some(home),
            lad,
            &|_: &Path| true
        ));
    }

    #[test]
    fn taskkill_zero_means_killed() {
        assert!(taskkill_killed(Some(0)));
    }

    #[test]
    fn taskkill_128_means_not_running() {
        assert!(!taskkill_killed(Some(128)));
    }

    #[test]
    fn taskkill_other_codes_treated_as_not_running() {
        assert!(!taskkill_killed(Some(1)));
        assert!(!taskkill_killed(None));
    }

    #[test]
    fn codex_exe_candidates_under_local_app_data() {
        let c = windows_codex_exe_candidates(r"C:\Users\me\AppData\Local");
        assert_eq!(c.len(), 2);
        assert!(c[0].ends_with("Programs/Codex/Codex.exe"));
        assert!(c[1].ends_with("Codex/Codex.exe"));
        assert!(c[0].starts_with(r"C:\Users\me\AppData\Local"));
        assert!(!c.iter().any(|p| p.to_string_lossy().contains("ChatGPT")));
    }

    #[test]
    fn codex_exe_candidates_empty_when_no_local_app_data() {
        assert!(windows_codex_exe_candidates("").is_empty());
        assert!(windows_codex_exe_candidates("  ").is_empty());
    }
}
