//! 全局指令文件管理（`~/.claude/CLAUDE.md`、`~/.codex/AGENTS.md`）。
//!
//! 设计要点：
//! - **只管全局两份**。项目级 `CLAUDE.md` / `AGENTS.md` 跟着仓库走，AgentGate
//!   不碰。
//! - **读纯文本，写整文件**。Markdown 不解析、不渲染，原样回显给前端 textarea。
//! - **写盘前不在本模块打 snapshot**。snapshot + DB 记录在 `commands.rs` 里
//!   走 `record_pre_apply`，和其他 5 个客户端的处理方式保持一致；本模块只负责
//!   纯粹的文件 I/O，方便单测。
//! - **文件不存在时自动 `mkdir -p` 后创建**。用户首次打开页面就能用，不需要
//!   先去 CLI 里跑一次。
//! - **append 模式在中间插入分隔横线**，保持原内容可读。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::errors::AppError;

/// 用户全局指令的两个目标文件。前端只暴露这两个值，新增 scope 必须先在这里
/// 加一条枚举 + `path()` 分支，避免「字符串路径满天飞」式的硬编码。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum InstructionsScope {
    ClaudeGlobal,
    CodexGlobal,
}

impl InstructionsScope {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude_global" => Some(Self::ClaudeGlobal),
            "codex_global" => Some(Self::CodexGlobal),
            _ => None,
        }
    }

    /// `client_apply_history.client_id` 列要用的字符串，必须保持稳定，
    /// 一旦换名旧历史就找不回来了。
    pub fn history_client_id(self) -> &'static str {
        match self {
            Self::ClaudeGlobal => "claude_instructions",
            Self::CodexGlobal => "codex_instructions",
        }
    }

    /// 该 scope 对应的全局指令文件绝对路径。Windows 下 `HOME` 不存在，
    /// fallback 到 `USERPROFILE`。
    pub fn path(self) -> Result<PathBuf, AppError> {
        let base = crate::fsutil::home_dir()?;
        Ok(match self {
            Self::ClaudeGlobal => base.join(".claude").join("CLAUDE.md"),
            Self::CodexGlobal => base.join(".codex").join("AGENTS.md"),
        })
    }

    /// snapshot 时给 `client_apply_history` 用的文件名（仅 UI 展示，与
    /// `absolute_path` 一起被前端用来识别这是哪个文件）。
    pub fn file_name(self) -> &'static str {
        match self {
            Self::ClaudeGlobal => "CLAUDE.md",
            Self::CodexGlobal => "AGENTS.md",
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct InstructionsStatus {
    pub scope: String,
    pub path: String,
    pub exists: bool,
    /// 文件 UTF-8 原文。`exists=false` 时为空字符串。
    pub content: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    /// 覆盖整文件。
    Overwrite,
    /// 追加到现有内容末尾。如果文件不存在或为空，等价于 overwrite。
    Append,
}

impl ApplyMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "overwrite" => Some(Self::Overwrite),
            "append" => Some(Self::Append),
            _ => None,
        }
    }
}

pub fn read(scope: InstructionsScope) -> InstructionsStatus {
    // 只读展示:家目录不可用时路径为空,按「不存在」展示。
    let path = scope.path().unwrap_or_default();
    let (exists, content, size_bytes) = match fs::read_to_string(&path) {
        Ok(s) => {
            let len = s.len() as u64;
            (true, s, len)
        }
        Err(_) => (false, String::new(), 0),
    };
    InstructionsStatus {
        scope: scope_string(scope),
        path: path.to_string_lossy().to_string(),
        exists,
        content,
        size_bytes,
    }
}

/// 写入整文件。如果父目录不存在会自动 `mkdir -p`——首次使用 Claude Code
/// 或 Codex 之前用户可能根本没建过 `~/.claude/`。
pub fn write(scope: InstructionsScope, content: &str) -> Result<InstructionsStatus, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let path = scope.path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AppError::new(
                crate::errors::codes::INSTRUCTIONS_WRITE_FAILED,
                format!("Cannot create directory {}: {e}", parent.display()),
            )
        })?;
    }
    // 原子写:写到一半崩溃不会截断用户的全局指令文件;保留原权限。
    crate::fsutil::atomic_write(&path, content.as_bytes(), None).map_err(|e| {
        AppError::new(
            crate::errors::codes::INSTRUCTIONS_WRITE_FAILED,
            format!("Cannot write {}: {e}", path.display()),
        )
    })?;
    Ok(read(scope))
}

/// 把模板按 mode 应用到目标 scope。
/// - overwrite：直接覆盖。
/// - append：读现有内容，加一段分隔再拼模板；旧文件不存在或为空时等价 overwrite。
pub fn apply_template(
    scope: InstructionsScope,
    template_id: &str,
    mode: ApplyMode,
) -> Result<InstructionsStatus, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let tpl = super::instructions_templates::find(template_id).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::INSTRUCTIONS_TEMPLATE_NOT_FOUND,
            format!("Unknown template: {template_id}"),
        )
    })?;

    let final_content = match mode {
        ApplyMode::Overwrite => tpl.content.to_string(),
        ApplyMode::Append => {
            // 读失败(权限 / 非 UTF-8)必须中止,不能当成空文件用模板覆盖。
            let path = scope.path()?;
            let existing = crate::fsutil::read_config_or_empty(&path).map_err(|e| {
                AppError::new(
                    crate::errors::codes::INSTRUCTIONS_WRITE_FAILED,
                    format!("Cannot read {}: {e}", path.display()),
                )
            })?;
            if existing.trim().is_empty() {
                tpl.content.to_string()
            } else {
                let mut out = existing.trim_end().to_string();
                out.push_str("\n\n---\n\n");
                out.push_str(tpl.content);
                out
            }
        }
    };

    write(scope, &final_content)
}

/// 用于打 `client_apply_history` snapshot 的 (file_name, absolute_path) pair。
/// 由 commands.rs 在 write/apply 前调用，保持和其他 5 个客户端同一套流程。
pub fn snapshot_paths(scope: InstructionsScope) -> Vec<(&'static str, PathBuf)> {
    scope
        .path()
        .map(|p| vec![(scope.file_name(), p)])
        .unwrap_or_default()
}

/// 指令备份（6.5）：把两个 scope 的全局指令内容打包成一份 JSON，便于迁移。
/// 沿用「不新增重复导出格式」原则——结构跟 SkillsExport 一样朴素。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct InstructionsBackup {
    pub version: u32,
    /// `~/.claude/CLAUDE.md` 原文，文件不存在为空串。
    pub claude: String,
    /// `~/.codex/AGENTS.md` 原文，文件不存在为空串。
    pub codex: String,
}

/// 导出两个 scope 的指令内容。
pub fn export_backup() -> InstructionsBackup {
    InstructionsBackup {
        version: 1,
        claude: read(InstructionsScope::ClaudeGlobal).content,
        codex: read(InstructionsScope::CodexGlobal).content,
    }
}

fn scope_string(scope: InstructionsScope) -> String {
    match scope {
        InstructionsScope::ClaudeGlobal => "claude_global".to_string(),
        InstructionsScope::CodexGlobal => "codex_global".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    /// 单元测试用临时 HOME，确保不污染真实文件，也不互相污染。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = FS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = setup_temp_home();
        f();
        cleanup(&temp);
    }

    #[test]
    fn read_returns_not_exists_when_file_missing() {
        with_temp_home(|| {
            let s = read(InstructionsScope::ClaudeGlobal);
            assert!(!s.exists);
            assert_eq!(s.content, "");
            assert_eq!(s.size_bytes, 0);
        });
    }

    #[test]
    fn write_creates_parent_dir() {
        with_temp_home(|| {
            let s = write(InstructionsScope::ClaudeGlobal, "hello").unwrap();
            assert!(s.exists);
            assert_eq!(s.content, "hello");
        });
    }

    #[test]
    fn apply_overwrite_replaces_existing() {
        with_temp_home(|| {
            write(InstructionsScope::ClaudeGlobal, "old").unwrap();
            let s = apply_template(
                InstructionsScope::ClaudeGlobal,
                "minimal-zh",
                ApplyMode::Overwrite,
            )
            .unwrap();
            assert!(!s.content.contains("old"));
            assert!(s.content.contains("核心原则"));
        });
    }

    #[test]
    fn apply_append_keeps_existing_and_separates_with_rule() {
        with_temp_home(|| {
            write(InstructionsScope::ClaudeGlobal, "OLD_CONTENT").unwrap();
            let s =
                apply_template(InstructionsScope::ClaudeGlobal, "tdd", ApplyMode::Append).unwrap();
            assert!(s.content.contains("OLD_CONTENT"));
            assert!(s.content.contains("---"));
            assert!(s.content.contains("TDD 模式"));
        });
    }

    #[test]
    fn apply_append_to_empty_file_acts_like_overwrite() {
        with_temp_home(|| {
            write(InstructionsScope::ClaudeGlobal, "").unwrap();
            let s =
                apply_template(InstructionsScope::ClaudeGlobal, "tdd", ApplyMode::Append).unwrap();
            assert!(!s.content.contains("---"));
            assert!(s.content.contains("TDD 模式"));
        });
    }

    /// 回归:append 模式读现有内容失败(非 UTF-8 / 权限)时,旧实现当成空文件,
    /// 直接用模板覆盖掉用户的 CLAUDE.md。
    #[test]
    fn apply_append_refuses_to_overwrite_unreadable_file() {
        with_temp_home(|| {
            let path = InstructionsScope::ClaudeGlobal.path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let original: &[u8] = b"# my rules\n\xff\xfe";
            std::fs::write(&path, original).unwrap();
            assert!(
                apply_template(InstructionsScope::ClaudeGlobal, "tdd", ApplyMode::Append).is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), original);
        });
    }

    #[test]
    fn apply_unknown_template_errors() {
        with_temp_home(|| {
            let err = apply_template(
                InstructionsScope::ClaudeGlobal,
                "nope",
                ApplyMode::Overwrite,
            )
            .unwrap_err();
            assert_eq!(err.code, "INSTRUCTIONS_TEMPLATE_NOT_FOUND");
        });
    }
}
