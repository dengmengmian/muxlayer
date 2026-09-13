//! Look up running client CLI processes so the UI can warn users that the
//! client they just (re)configured is still alive in some terminal and needs
//! a restart to pick up the new config.
//!
//! 匹配规则(精确,不再做子串 / 大小写不敏感匹配):
//! - Unix:跑一次 `ps -A -ww -o pid= -o args=`,对每行取 argv0 的 basename,与
//!   客户端的可执行文件名**大小写敏感地精确比较**;argv0 是解释器(node / bun /
//!   deno / python*)时改比较第一个非 `-` 参数(脚本路径)的 basename——npm /
//!   uv 安装的 CLI(claude / gemini / kimi …)在 ps 里就是 `node …/bin/claude`。
//!   解释器名忽略大小写(Homebrew framework Python 是 `Python`);npm / pnpm 全局
//!   安装直接跑包内入口脚本(`…/claude-code/cli.js`、`…/codex/bin/codex.js`)时按
//!   包目录名识别。`ps` 丢失引号,含空格的路径按「最短的存在文件前缀」还原。
//! - 可执行文件 / 脚本位于 `.app/Contents/` 内的一律排除:ChatGPT.app 内置的
//!   `codex app-server`、Claude.app 的 helper 属于桌面 App,结束它们会弄坏 App。
//!   这样 Claude Desktop(`Claude`、`Claude Helper (Renderer)`)、Grok 桌面 app
//!   (`Grok`)、`vim notes-about-claude.md` 都不会进入可结束列表。
//! - Windows:`tasklist /FO CSV /NH` 的映像名与 `<name>.exe` 精确比较(Windows
//!   文件名大小写不敏感,因此这里只能忽略大小写)。
//! - 自身进程按 `std::process::id()` 与当前可执行文件名过滤。
//!
//! 工具缺失或失败时返回空列表,调用方按「没检测到」处理,不阻塞 apply。

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, specta::Type)]
pub struct RunningProcess {
    pub pid: u32,
    pub command: String,
}

/// 当前进程可执行文件名(不含路径),用于过滤自身。
fn self_exe_name() -> Option<String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
}

/// Find processes of the given client executables (`names` are exact
/// basenames such as `claude`). Returns an empty list whenever the underlying
/// tool is missing or fails.
pub fn find_running(names: &[&str]) -> Vec<RunningProcess> {
    #[cfg(unix)]
    {
        find_unix(names)
    }
    #[cfg(windows)]
    {
        find_windows(names)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = names;
        Vec::new()
    }
}

#[cfg(unix)]
fn find_unix(names: &[&str]) -> Vec<RunningProcess> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-A", "-ww", "-o", "pid=", "-o", "args="])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    match_ps_output(
        &String::from_utf8_lossy(&output.stdout),
        names,
        std::process::id(),
        self_exe_name().as_deref(),
        &|p| std::path::Path::new(p).is_file(),
    )
}

/// Windows：跑一次 `tasklist /FO CSV /NH` 拿全量进程，再在本地按映像名精确过滤。
/// tasklist 缺失或失败时返回空列表（按「没检测到」处理，不阻塞 apply）。
#[cfg(windows)]
fn find_windows(names: &[&str]) -> Vec<RunningProcess> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    // CREATE_NO_WINDOW：GUI 进程下 spawn 控制台程序默认会弹黑窗，
    // 且弹窗抢焦点会触发前端 focus 刷新→再次探测，形成无限弹窗循环。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(output) = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_tasklist_csv(
        &String::from_utf8_lossy(&output.stdout),
        names,
        std::process::id(),
        self_exe_name().as_deref(),
    )
}

#[cfg(any(unix, test))]
fn basename(token: &str) -> &str {
    token.rsplit(['/', '\\']).next().unwrap_or(token)
}

/// node / bun / deno / python / python3 / python3.12 …(忽略大小写:Homebrew
/// framework Python 的可执行名是 `Python`)。
#[cfg(any(unix, test))]
fn is_interpreter(base: &str) -> bool {
    let base = base.to_ascii_lowercase();
    ["node", "bun", "deno", "python"].iter().any(|interp| {
        base.strip_prefix(interp)
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
    })
}

/// macOS App bundle 内的可执行文件 / 脚本(ChatGPT.app 内置的 `codex app-server`、
/// Claude.app 的 helper)属于桌面 App,不能出现在「结束进程」列表里。
#[cfg(any(unix, test))]
fn inside_app_bundle(path: &str) -> bool {
    path.contains(".app/Contents/")
}

/// `ps -o args=` 丢失了引号,路径里的空格和参数分隔无法区分。从 `s` 开头切出一个
/// 程序 / 脚本路径:以 `/` 开头时取「最短的、磁盘上存在的文件」前缀;都不存在
/// (或不是绝对路径)时退回第一个空白分隔的 token。返回 (路径, 剩余参数)。
#[cfg(any(unix, test))]
fn split_path_arg<'a>(s: &'a str, is_file: &dyn Fn(&str) -> bool) -> Option<(&'a str, &'a str)> {
    let s = s.trim_start();
    let first_end = s.find(char::is_whitespace).unwrap_or(s.len());
    if first_end == 0 {
        return None;
    }
    if s.starts_with('/') {
        let mut end = first_end;
        // 上限防止超长参数列表逐个 stat。
        for _ in 0..32 {
            if is_file(&s[..end]) {
                return Some((&s[..end], &s[end..]));
            }
            // 跳过连续空白,扩展到下一个 token 末尾
            let Some(next_start) = s[end..].find(|c: char| !c.is_whitespace()) else {
                break;
            };
            let next_start = end + next_start;
            end = s[next_start..]
                .find(char::is_whitespace)
                .map_or(s.len(), |i| next_start + i);
        }
    }
    Some((&s[..first_end], &s[first_end..]))
}

/// npm / pnpm 全局安装时解释器直接跑包内脚本(`…/@anthropic-ai/claude-code/cli.js`、
/// `…/@openai/codex/bin/codex.js`、`…/@google/gemini-cli/dist/index.js`)。
/// 脚本名是常见入口名,且某一级目录恰好是 `<name>` / `<name>-cli` / `<name>-code`。
#[cfg(any(unix, test))]
fn is_client_package_script(script: &str, name: &str) -> bool {
    let file = basename(script).to_ascii_lowercase();
    let entry = ["cli.js", "cli.mjs", "cli.cjs", "index.js", "index.mjs"].contains(&file.as_str())
        || file == format!("{name}.js")
        || file == format!("{name}.mjs");
    if !entry {
        return false;
    }
    let accepted = [
        name.to_string(),
        format!("{name}-cli"),
        format!("{name}-code"),
    ];
    // 包目录必须紧跟在 `node_modules/`(或 `node_modules/@scope/`)后面,
    // 普通项目目录恰好叫 codex 之类的不算。
    let segs: Vec<String> = script.split('/').map(|d| d.to_ascii_lowercase()).collect();
    segs.iter().enumerate().any(|(i, seg)| {
        if seg != "node_modules" {
            return false;
        }
        let pkg = match segs.get(i + 1) {
            Some(next) if next.starts_with('@') => segs.get(i + 2),
            other => other,
        };
        pkg.is_some_and(|p| accepted.contains(p))
    })
}

fn sort_dedup(mut all: Vec<RunningProcess>) -> Vec<RunningProcess> {
    all.sort_by_key(|p| p.pid);
    all.dedup_by_key(|p| p.pid);
    all
}

/// 解析 `ps -o pid= -o args=` 输出并按 basename 精确匹配(规则见模块文档)。
/// `command` 展示为匹配到的可执行名,解释器场景为 `node claude` 形式。
/// `is_file` 用于还原含空格的路径(生产传真实文件系统判断,测试可注入)。
#[cfg(any(unix, test))]
fn match_ps_output(
    output: &str,
    names: &[&str],
    self_pid: u32,
    self_exe: Option<&str>,
    is_file: &dyn Fn(&str) -> bool,
) -> Vec<RunningProcess> {
    let names: Vec<&str> = names
        .iter()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .collect();
    let all = output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid_str, args) = line.split_once(char::is_whitespace)?;
            let pid = pid_str.parse::<u32>().ok()?;
            // 先按名字粗筛再 stat:无关进程的路径不碰文件系统(macOS 上 stat
            // Documents / Desktop 里的文件可能弹隐私授权框)。
            let lower = args.to_ascii_lowercase();
            if !names
                .iter()
                .any(|n| lower.contains(&n.to_ascii_lowercase()))
            {
                return None;
            }
            let (argv0_path, rest) = split_path_arg(args, is_file)?;
            let argv0 = basename(argv0_path);
            if pid == self_pid || Some(argv0) == self_exe {
                return None;
            }
            if names.contains(&argv0) {
                if inside_app_bundle(argv0_path) {
                    return None;
                }
                return Some(RunningProcess {
                    pid,
                    command: argv0.to_string(),
                });
            }
            if is_interpreter(argv0) {
                // 跳过解释器参数(`--max-old-space-size=4096` 等),取脚本路径。
                let mut rest = rest.trim_start();
                while rest.starts_with('-') {
                    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                    rest = rest[end..].trim_start();
                }
                let (script_path, _) = split_path_arg(rest, is_file)?;
                if inside_app_bundle(script_path) {
                    return None;
                }
                let script = basename(script_path);
                let matched = names
                    .iter()
                    .find(|n| **n == script || is_client_package_script(script_path, n))?;
                let shown = if *matched == script { script } else { matched };
                return Some(RunningProcess {
                    pid,
                    command: format!("{argv0} {shown}"),
                });
            }
            None
        })
        .collect();
    sort_dedup(all)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillCheck {
    Ok,
    InvalidPid,
    SelfProcess,
    NotFound,
}

/// Only PIDs we just listed for this client are killable. Never PID 0/1 or us.
pub fn kill_check(pid: u32, live: &[RunningProcess]) -> KillCheck {
    if pid <= 1 {
        return KillCheck::InvalidPid;
    }
    if pid == std::process::id() {
        return KillCheck::SelfProcess;
    }
    if !live.iter().any(|p| p.pid == pid) {
        return KillCheck::NotFound;
    }
    KillCheck::Ok
}

/// SIGTERM, then SIGKILL if still alive. Windows uses `taskkill /T /F`.
/// 阻塞(unix 上等 400ms),调用方需在 blocking 线程执行。
pub fn terminate(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        run_cmd("kill", &["-TERM", &pid.to_string()])?;
        std::thread::sleep(std::time::Duration::from_millis(400));
        if pid_alive(pid) {
            run_cmd("kill", &["-KILL", &pid.to_string()])?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err("kill is not supported on this platform".to_string())
    }
}

#[cfg(unix)]
fn run_cmd(bin: &str, args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(err.trim().to_string())
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Windows：解析 `tasklist /FO CSV /NH` 的输出。每行形如
/// `"Codex.exe","1234","Console","1","123,456 K"`，取映像名 + PID。
/// 映像名与 `<name>.exe` 精确比较(Windows 文件名本身大小写不敏感),
/// 过滤自身(PID / 当前可执行文件名),按 PID 排序去重。
#[cfg(any(windows, test))]
fn parse_tasklist_csv(
    output: &str,
    names: &[&str],
    self_pid: u32,
    self_exe: Option<&str>,
) -> Vec<RunningProcess> {
    let images: Vec<String> = names
        .iter()
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .map(|n| {
            if n.ends_with(".exe") {
                n
            } else {
                format!("{n}.exe")
            }
        })
        .collect();
    if images.is_empty() {
        return Vec::new();
    }
    let self_exe = self_exe.map(|s| s.to_ascii_lowercase());

    let all = output
        .lines()
        .filter_map(|line| {
            let fields = parse_csv_fields(line.trim());
            // 前两列固定是「映像名称」「PID」；非 CSV 的提示行（如
            // "信息: 没有运行的任务…"）解析不出两列或 PID 非数字，自然跳过。
            let image = fields.first()?;
            let pid = fields.get(1)?.parse::<u32>().ok()?;
            let image_lc = image.to_ascii_lowercase();
            if pid == self_pid || self_exe.as_deref() == Some(image_lc.as_str()) {
                return None;
            }
            if !images.contains(&image_lc) {
                return None;
            }
            Some(RunningProcess {
                pid,
                command: image.clone(),
            })
        })
        .collect();
    sort_dedup(all)
}

/// 解析一行带引号的 CSV（tasklist /FO CSV 风格）：字段全部用双引号包裹，
/// 引号内可含逗号，`""` 表示转义的双引号。
#[cfg(any(windows, test))]
fn parse_csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    // "" 转义为单个双引号
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    fields.push(std::mem::take(&mut cur));
                }
                _ => cur.push(c),
            }
        }
    }
    if !cur.is_empty() || !fields.is_empty() {
        fields.push(cur);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_files(_: &str) -> bool {
        false
    }

    #[test]
    fn ps_matcher_handles_blank_and_garbage_lines() {
        let out = "\n  abc claude\n12345 claude\n\n";
        let parsed = match_ps_output(out, &["claude"], 1, None, &no_files);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pid, 12345);
        assert_eq!(parsed[0].command, "claude");
    }

    // ---- Windows tasklist CSV 解析（纯函数，macOS 上也可跑） ----

    #[test]
    fn csv_fields_basic_quoted_line() {
        let fields = parse_csv_fields(r#""Codex.exe","1234","Console","1","123,456 K""#);
        assert_eq!(
            fields,
            vec!["Codex.exe", "1234", "Console", "1", "123,456 K"]
        );
    }

    #[test]
    fn csv_fields_escaped_quote_inside_field() {
        let fields = parse_csv_fields(r#""a""b","2""#);
        assert_eq!(fields, vec![r#"a"b"#, "2"]);
    }

    #[test]
    fn tasklist_matches_needles_case_insensitive() {
        let out = concat!(
            "\"Codex.exe\",\"200\",\"Console\",\"1\",\"10,000 K\"\r\n",
            "\"notepad.exe\",\"300\",\"Console\",\"1\",\"1,000 K\"\r\n",
            "\"Claude.exe\",\"100\",\"Console\",\"1\",\"20,000 K\"\r\n",
        );
        let got = parse_tasklist_csv(out, &["codex", "CLAUDE"], 1, None);
        assert_eq!(got.len(), 2);
        // 按 PID 排序
        assert_eq!(got[0].pid, 100);
        assert_eq!(got[0].command, "Claude.exe");
        assert_eq!(got[1].pid, 200);
        assert_eq!(got[1].command, "Codex.exe");
    }

    #[test]
    fn tasklist_filters_self_and_requires_exact_image_name() {
        let out = concat!(
            "\"MuxLayer.exe\",\"42\",\"Console\",\"1\",\"5,000 K\"\r\n",
            "\"codex-helper.exe\",\"43\",\"Console\",\"1\",\"5,000 K\"\r\n",
            "\"codex.exe\",\"44\",\"Console\",\"1\",\"5,000 K\"\r\n",
        );
        assert!(parse_tasklist_csv(out, &["muxlayer"], 1, Some("MuxLayer.exe")).is_empty());
        let got = parse_tasklist_csv(out, &["codex"], 44, None);
        assert!(
            got.is_empty(),
            "substring / self pid must not match: {got:?}"
        );
    }

    #[test]
    fn tasklist_dedups_pid_matched_by_multiple_needles() {
        let out = "\"Codex.exe\",\"77\",\"Console\",\"1\",\"5,000 K\"\r\n";
        let got = parse_tasklist_csv(out, &["codex", "Codex.exe"], 1, None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].pid, 77);
    }

    #[test]
    fn tasklist_skips_garbage_and_blank_lines() {
        let out = concat!(
            "信息: 没有运行的任务匹配指定标准。\r\n",
            "\r\n",
            "\"Codex.exe\",\"notanumber\",\"Console\",\"1\",\"1 K\"\r\n",
            "\"Codex.exe\",\"88\",\"Console\",\"1\",\"1 K\"\r\n",
        );
        let got = parse_tasklist_csv(out, &["codex"], 1, None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].pid, 88);
    }

    #[test]
    fn tasklist_ignores_blank_needles() {
        let out = "\"Codex.exe\",\"88\",\"Console\",\"1\",\"1 K\"\r\n";
        assert!(parse_tasklist_csv(out, &["", "  "], 1, None).is_empty());
    }

    // ---- Unix 精确匹配(item 7):按 argv0 / 解释器脚本名的 basename 精确比较 ----

    const PS_SAMPLE: &str = "\
  101 /Applications/Claude.app/Contents/MacOS/Claude
  102 /Applications/Claude.app/Contents/Frameworks/Claude Helper (Renderer).app/Contents/MacOS/Claude Helper (Renderer) --type=renderer
  103 /Applications/Grok.app/Contents/MacOS/Grok
  104 node /opt/homebrew/bin/claude --resume
  105 claude
  106 /Users/me/.local/share/uv/tools/kimi-cli/bin/python3 /Users/me/.local/bin/kimi
  107 /opt/homebrew/bin/grok
  108 vim notes-about-claude.md
  109 /usr/bin/python3 -m http.server
  110 node --max-old-space-size=4096 /usr/local/lib/node_modules/@google/gemini-cli/dist/index.js
  111 node /opt/homebrew/bin/gemini
  112 claude-helper
  113 /Applications/MuxLayer.app/Contents/MacOS/agentgate
  114 codex
";

    #[test]
    fn ps_matcher_matches_exact_cli_basenames_only() {
        let got = match_ps_output(PS_SAMPLE, &["claude"], 999_999, None, &no_files);
        let pids: Vec<u32> = got.iter().map(|p| p.pid).collect();
        assert_eq!(
            pids,
            vec![104, 105],
            "Claude Desktop / Helper / claude-helper / vim must not match"
        );
    }

    #[test]
    fn ps_matcher_excludes_desktop_apps_and_handles_interpreters() {
        let grok = match_ps_output(PS_SAMPLE, &["grok"], 999_999, None, &no_files);
        assert_eq!(grok.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![107]);
        let kimi = match_ps_output(PS_SAMPLE, &["kimi"], 999_999, None, &no_files);
        assert_eq!(kimi.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![106]);
        let gemini = match_ps_output(PS_SAMPLE, &["gemini"], 999_999, None, &no_files);
        assert_eq!(
            gemini.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![110, 111],
            "gemini-cli relaunches itself as node …/gemini-cli/dist/index.js"
        );
        assert_eq!(kimi[0].command, "python3 kimi");
    }

    #[test]
    fn ps_matcher_filters_self_by_pid_and_executable() {
        let got = match_ps_output(
            PS_SAMPLE,
            &["codex", "agentgate"],
            114,
            Some("agentgate"),
            &no_files,
        );
        assert!(
            got.is_empty(),
            "self pid and self exe must be filtered: {got:?}"
        );
    }

    const PS_BUNDLES: &str = "\
  201 /Applications/ChatGPT.app/Contents/Resources/codex app-server --listen stdio
  202 /Applications/Claude.app/Contents/Resources/claude --mcp
  203 /Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper --type=gpu
  204 /opt/homebrew/Cellar/python@3.12/3.12.4/Frameworks/Python.framework/Versions/3.12/Resources/Python.app/Contents/MacOS/Python /Users/me/.local/bin/kimi
  205 /Users/me/.nvm/versions/node/v22.1.0/bin/node /Users/me/Library/pnpm/global/5/node_modules/@anthropic-ai/claude-code/cli.js --resume
  206 node /usr/local/lib/node_modules/@openai/codex/bin/codex.js
  207 node /Users/me/Library/Application Support/JetBrains/IntelliJIdea2025.2/plugins/claude/node_modules/@anthropic-ai/claude-code/cli.js --output-format stream-json
  208 /Users/me/Library/Application Support/JetBrains/Tools/bin/codex exec
  209 node /tmp/server.js --dir /tmp/claude
  210 node /x/node_modules/claude-helper/index.js
  211 node /x/node_modules/@modelcontextprotocol/server-filesystem/dist/index.js /Users/me/codex
  212 /usr/local/bin/codex
";

    /// 测试里「存在的文件」:含空格的路径靠它判定 argv 边界。
    fn bundle_files(p: &str) -> bool {
        [
            "/Users/me/Library/Application Support/JetBrains/IntelliJIdea2025.2/plugins/claude/node_modules/@anthropic-ai/claude-code/cli.js",
            "/Users/me/Library/Application Support/JetBrains/Tools/bin/codex",
            "/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper",
        ]
        .contains(&p)
    }

    fn pids(names: &[&str], files: &dyn Fn(&str) -> bool) -> Vec<u32> {
        match_ps_output(PS_BUNDLES, names, 999_999, None, files)
            .iter()
            .map(|p| p.pid)
            .collect()
    }

    /// 回归:ChatGPT.app 内置的 `codex app-server` basename 也是 codex,结束它会
    /// 弄坏 ChatGPT 桌面端;Claude.app 里的 helper 同理。
    #[test]
    fn ps_matcher_excludes_binaries_inside_app_bundles() {
        for files in [&no_files as &dyn Fn(&str) -> bool, &bundle_files] {
            let codex = pids(&["codex"], files);
            assert!(!codex.contains(&201), "ChatGPT.app codex listed: {codex:?}");
            let claude = pids(&["claude"], files);
            assert!(
                !claude.contains(&202),
                "Claude.app claude listed: {claude:?}"
            );
            assert!(!claude.contains(&203));
        }
    }

    #[test]
    fn ps_matcher_handles_framework_python_and_package_scripts() {
        assert_eq!(
            pids(&["kimi"], &no_files),
            vec![204],
            "Homebrew framework Python"
        );
        let claude = pids(&["claude"], &no_files);
        assert!(
            claude.contains(&205),
            "pnpm global claude-code/cli.js: {claude:?}"
        );
        assert!(
            !claude.contains(&209),
            "argument path /tmp/claude is not the script"
        );
        assert!(
            !claude.contains(&210),
            "claude-helper package is not claude"
        );
        let codex = pids(&["codex"], &no_files);
        assert!(
            codex.contains(&206),
            "@openai/codex/bin/codex.js: {codex:?}"
        );
        assert!(
            !codex.contains(&211),
            "MCP server with codex dir arg: {codex:?}"
        );
        assert!(codex.contains(&212));
    }

    /// 不含任何客户端名的进程行不能 stat 路径:macOS 上 stat `~/Documents` 等目录里的
    /// 文件可能弹隐私授权框,而检测每 60 秒跑一次。
    #[test]
    fn ps_matcher_does_not_stat_paths_of_unrelated_processes() {
        let out = "\
  301 /usr/bin/node /Users/me/Documents/proj/server.js --port 3000
  302 /Users/me/Desktop/tools/some app/run --flag
";
        let calls = std::cell::Cell::new(0);
        let counting = |_: &str| {
            calls.set(calls.get() + 1);
            false
        };
        let got = match_ps_output(out, &["claude", "codex"], 1, None, &counting);
        assert!(got.is_empty());
        assert_eq!(
            calls.get(),
            0,
            "unrelated lines must not touch the filesystem"
        );
    }

    /// 包脚本只认 `node_modules/<name>` 或 `node_modules/@scope/<name>` 这一级,
    /// 普通项目目录恰好叫 codex、opencode 数据目录里的语言服务器都不算。
    #[test]
    fn ps_matcher_package_scripts_require_node_modules_package_dir() {
        let out = "\
  401 node /home/me/codex/dist/index.js
  402 node /home/me/.local/share/opencode/bin/typescript-language-server/lib/cli.mjs --stdio
  403 node /usr/lib/node_modules/@google/gemini-cli/dist/index.js
  404 node /usr/lib/node_modules/opencode/bin/cli.js
";
        let names = |n: &[&str]| -> Vec<u32> {
            match_ps_output(out, n, 1, None, &no_files)
                .iter()
                .map(|p| p.pid)
                .collect()
        };
        assert!(names(&["codex"]).is_empty(), "project dir named codex");
        assert_eq!(names(&["opencode"]), vec![404]);
        assert_eq!(names(&["gemini"]), vec![403]);
    }

    /// `ps -o args=` 丢失引号:含空格的脚本 / 可执行路径按「最短的存在文件前缀」还原。
    #[test]
    fn ps_matcher_resolves_paths_with_spaces() {
        assert_eq!(pids(&["claude"], &bundle_files), vec![205, 207]);
        assert_eq!(pids(&["codex"], &bundle_files), vec![206, 208, 212]);
    }

    fn live(pid: u32, command: &str) -> RunningProcess {
        RunningProcess {
            pid,
            command: command.to_string(),
        }
    }

    #[test]
    fn kill_check_allows_pid_in_live_list() {
        assert_eq!(kill_check(4242, &[live(4242, "dsh")]), KillCheck::Ok);
    }

    #[test]
    fn kill_check_rejects_pid_zero_and_init() {
        let list = [live(1, "init")];
        assert_eq!(kill_check(0, &list), KillCheck::InvalidPid);
        assert_eq!(kill_check(1, &list), KillCheck::InvalidPid);
    }

    #[test]
    fn kill_check_rejects_self_and_missing() {
        let self_pid = std::process::id();
        assert_eq!(
            kill_check(self_pid, &[live(self_pid, "dsh")]),
            KillCheck::SelfProcess
        );
        assert_eq!(
            kill_check(999_999, &[live(4242, "dsh")]),
            KillCheck::NotFound
        );
    }
}
