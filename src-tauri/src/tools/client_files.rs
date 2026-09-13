//! 各客户端接入模块共用的文件读写小工具。
//!
//! 8 个客户端(codex / claude_code / gemini / atomcode / opencode / kimi / grok /
//! deepseek harness)原来各自复制了一份「读配置 → tmp+rename → 读回校验」「备份
//! 官方配置」「生成带掩码 token 的片段」,行为还不一致(有的吞读取错误、有的丢权限)。

use std::path::Path;

use crate::errors::AppError;
use crate::security::local_token;

/// 含本地 gateway token / 用户凭据的文件统一用 0600。
pub const SECRET_FILE_MODE: u32 = 0o600;

/// 写回前读取配置:文件不存在视为空串;权限不足 / 非 UTF-8 等返回 `code` 错误,
/// 调用方必须中止,绝不能拿空内容覆盖用户文件。
pub fn read_for_update(path: &Path, code: &'static str) -> Result<String, AppError> {
    crate::fsutil::read_config_or_empty(path)
        .map_err(|e| AppError::new(code, format!("Cannot read {}: {e}", path.display())))
}

/// 读取可选文件(如官方配置备份):不存在 → `None`,其它错误 → `code` 错误。
pub fn read_optional(path: &Path, code: &'static str) -> Result<Option<String>, AppError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::new(
            code,
            format!("Cannot read {}: {e}", path.display()),
        )),
    }
}

/// 确保父目录存在。
pub fn ensure_parent(path: &Path, code: &'static str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::new(
                code,
                format!("Cannot create directory {}: {e}", parent.display()),
            )
        })?;
    }
    Ok(())
}

/// 原子写 + 读回逐字节校验。`mode` 语义见 [`crate::fsutil::atomic_write`]。
pub fn write_verified(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    code: &'static str,
) -> Result<(), AppError> {
    ensure_parent(path, code)?;
    crate::fsutil::atomic_write(path, bytes, mode)
        .map_err(|e| AppError::new(code, format!("Failed to write {}: {e}", path.display())))?;
    crate::tools::config_verify::verify_written(path, bytes).map_err(|e| AppError::new(code, e))
}

/// 把 `src` 备份到 `dst`(0600,原子写)。`src` 不存在时删除旧备份并返回 `Ok(false)`,
/// 让备份始终代表「本次 apply 之前」的状态,不会拿很久以前的旧值去 restore。
pub fn backup_file(src: &Path, dst: &Path, code: &'static str) -> Result<bool, AppError> {
    let Some(content) = read_optional(src, code)? else {
        match std::fs::remove_file(dst) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(AppError::new(
                    code,
                    format!("Cannot remove stale backup {}: {e}", dst.display()),
                ))
            }
        }
        return Ok(false);
    };
    write_verified(dst, content.as_bytes(), Some(SECRET_FILE_MODE), code)?;
    Ok(true)
}

/// 片段预览里展示的 token:已生成则掩码,否则占位符。
pub fn masked_token_for_snippet() -> String {
    match local_token::read_token() {
        Ok(t) => local_token::mask_token(&t),
        Err(_) => "ag_local_<not_generated>".to_string(),
    }
}

/// 是否是 MuxLayer 本地 token(结构化识别客户端配置是否接入的唯一可靠依据)。
pub fn is_local_token(value: &str) -> bool {
    value.starts_with("ag_local_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn read_for_update_errors_instead_of_returning_empty_on_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{\"keep\":true}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_to_string(&path).is_err() {
            assert!(read_for_update(&path, "X").is_err());
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"keep\":true}");
    }

    #[test]
    fn backup_file_skips_missing_source_and_drops_stale_backup() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("saved").join("config.toml");
        assert!(!backup_file(&dir.path().join("nope"), &dst, "X").unwrap());
        assert!(!dst.exists());
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&dst, "stale").unwrap();
        assert!(!backup_file(&dir.path().join("nope"), &dst, "X").unwrap());
        assert!(!dst.exists(), "stale backup must not survive");
    }

    #[cfg(unix)]
    #[test]
    fn backup_file_writes_owner_only_copy() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("auth.json");
        std::fs::write(&src, "{\"tokens\":1}").unwrap();
        let dst = dir.path().join("saved").join("auth.json");
        assert!(backup_file(&src, &dst, "X").unwrap());
        let mode = std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "{\"tokens\":1}");
    }
}
