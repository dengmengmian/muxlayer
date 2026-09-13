//! 读写用户配置文件的共享工具:家目录解析、"只把 NotFound 当空"的读取、原子写。
//!
//! 为什么集中到这里:
//! - 旧代码 `read_to_string(..).unwrap_or_else(|_| "{}")` 在 EACCES / 非 UTF-8 时把
//!   整份 `~/.claude.json` 当成空文件,写回后用户的 oauthAccount / projects 全部丢失。
//! - `fs::write` 不是原子的,写到一半崩溃会截断配置文件。
//! - HOME / USERPROFILE 都没设置时旧代码回落到相对路径,会把配置写进进程 cwd。
//! - 各客户端模块各自抄了一份 tmp + rename,行为不一致(有的不 fsync、有的丢权限)。
//!
//! 放在 crate 根(而不是 tools/)是因为 storage::apply_history 和 security::local_token
//! 在 headless cli 构建里也要用。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::errors::AppError;

/// 家目录无法确定时的错误码。
pub const HOME_DIR_UNAVAILABLE: &str = "HOME_DIR_UNAVAILABLE";

/// 用户主目录:先 HOME,再 USERPROFILE(Windows)。两者都没有(或为空)时报错,
/// 绝不回落到相对路径。
pub fn home_dir() -> Result<PathBuf, AppError> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key) {
            if !value.is_empty() {
                return Ok(PathBuf::from(value));
            }
        }
    }
    Err(AppError::new(
        HOME_DIR_UNAVAILABLE,
        "无法确定用户主目录:HOME 与 USERPROFILE 均未设置",
    ))
}

/// 客户端配置写锁:进程内唯一。
///
/// 客户端配置命令跑在 blocking 线程池里会并发执行,两个命令同时对同一文件做
/// 「读 → 改 → 写」(如 apply_gemini_config 与 gemini 的 MCP upsert 都写
/// ~/.gemini/settings.json)时后写的会覆盖先写的改动。所有客户端配置变更在整个
/// 读改写期间持有这把锁。
///
/// 同一线程可重入(toggle → apply、sync → upsert 这类嵌套调用不会自锁死);
/// 不按路径细分:配置变更是低频的用户操作,全局串行最简单也最不容易漏。
pub fn lock_client_configs() -> ClientConfigLock {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let depth = CONFIG_LOCK_DEPTH.with(|d| d.get());
    let guard = if depth == 0 {
        // 持锁线程 panic 只会让锁中毒,文件本身是原子写的,继续使用即可。
        Some(LOCK.lock().unwrap_or_else(|e| e.into_inner()))
    } else {
        None
    };
    CONFIG_LOCK_DEPTH.with(|d| d.set(depth + 1));
    ClientConfigLock {
        _guard: guard,
        _not_send: std::marker::PhantomData,
    }
}

thread_local! {
    static CONFIG_LOCK_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// [`lock_client_configs`] 的守卫;drop 时释放(嵌套时只减计数)。不可跨线程。
pub struct ClientConfigLock {
    _guard: Option<std::sync::MutexGuard<'static, ()>>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for ClientConfigLock {
    fn drop(&mut self) {
        CONFIG_LOCK_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

/// 读取配置文件文本。只有"文件不存在"才视为空串;权限不足、非 UTF-8 等其它错误
/// 一律返回 Err,调用方必须中止写回,否则会用残缺内容覆盖用户文件。
/// 调用方都在 tools/(桌面构建);headless cli 构建不编译。
#[cfg(any(feature = "desktop", test))]
pub fn read_config_or_empty(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// 原子写:同目录临时文件 → 写入 → fsync → rename 覆盖。
///
/// - `mode = Some(m)`(仅 unix 生效):最终文件权限精确为 `m`(不受 umask 影响),
///   临时文件从创建起就是 `m`,不存在更宽松权限的窗口。
/// - `mode = None`:目标已存在时保留其原权限;不存在时按系统默认(umask)创建。
/// - 目标是符号链接时写入链接指向的真实文件,不把用户的 dotfile 软链替换成普通文件。
/// - 只接受绝对路径:相对路径意味着家目录解析失败,写下去会落进进程 cwd。
pub fn atomic_write(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("拒绝写入相对路径: {}", path.display()),
        ));
    }
    let target = resolve_symlink_target(path)?;
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("路径没有父目录: {}", target.display()),
        )
    })?;
    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());

    let existing_meta = fs::metadata(&target).ok();
    // rename 会绕过目标文件的只读位(用户 `chmod 444` 保护的配置)。旧的 fs::write
    // 在这里会 EACCES 失败,保持同样语义:目标是普通文件且当前用户无法写入时报错。
    // 以写方式打开(不截断)做判定,unix 权限位 / ACL / Windows 只读属性都生效。
    if existing_meta.as_ref().is_some_and(|m| m.is_file()) {
        if let Err(e) = fs::OpenOptions::new().write(true).open(&target) {
            if e.kind() == io::ErrorKind::PermissionDenied {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("目标文件不可写(只读),拒绝替换: {}: {e}", target.display()),
                ));
            }
        }
    }
    let existing_perms = existing_meta.map(|m| m.permissions());
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));

    let result = (|| -> io::Result<()> {
        let mut file = open_tmp(&tmp, mode, existing_perms.is_some())?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        apply_final_permissions(&tmp, mode, existing_perms)?;
        fs::rename(&tmp, &target)?;
        sync_parent_dir(parent);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn resolve_symlink_target(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(real) => Ok(real),
            // 悬空软链(目标文件还不存在):按链接内容定位目标,相对路径相对链接所在目录。
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let link = fs::read_link(path)?;
                Ok(if link.is_absolute() {
                    link
                } else {
                    path.parent().unwrap_or(Path::new("/")).join(link)
                })
            }
            Err(e) => Err(e),
        },
        _ => Ok(path.to_path_buf()),
    }
}

#[cfg(unix)]
fn open_tmp(tmp: &Path, mode: Option<u32>, target_exists: bool) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    // 显式 mode 直接用;目标已存在时先用 0600 创建,写完再改成原权限,
    // 避免内容在临时文件上以更宽松权限短暂可读。
    if let Some(m) = mode {
        opts.mode(m);
    } else if target_exists {
        opts.mode(0o600);
    }
    opts.open(tmp)
}

#[cfg(not(unix))]
fn open_tmp(tmp: &Path, _mode: Option<u32>, _target_exists: bool) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
}

#[cfg(unix)]
fn apply_final_permissions(
    tmp: &Path,
    mode: Option<u32>,
    existing: Option<fs::Permissions>,
) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(m) = mode {
        fs::set_permissions(tmp, fs::Permissions::from_mode(m))
    } else if let Some(perms) = existing {
        fs::set_permissions(tmp, perms)
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn apply_final_permissions(
    _tmp: &Path,
    _mode: Option<u32>,
    _existing: Option<fs::Permissions>,
) -> io::Result<()> {
    Ok(())
}

/// rename 之后 fsync 父目录,让目录项在掉电后也持久化。部分文件系统不支持目录
/// fsync(返回 EINVAL),此时 rename 本身已完成、文件内容已 fsync,只影响掉电
/// 场景的持久性,因此这里不把它当写入失败。
#[cfg(unix)]
fn sync_parent_dir(parent: &Path) {
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_prefers_home_then_userprofile_and_errors_when_both_missing() {
        crate::test_utils::with_windows_style_home(|fake| {
            assert_eq!(home_dir().unwrap(), fake.to_path_buf());
            std::env::remove_var("USERPROFILE");
            let err = home_dir().unwrap_err();
            assert_eq!(err.code, HOME_DIR_UNAVAILABLE);
        });
    }

    #[test]
    fn client_config_lock_is_reentrant_and_serializes_threads() {
        let outer = lock_client_configs();
        let inner = lock_client_configs();
        drop(inner);
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            let _g = lock_client_configs();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "other thread must wait while lock is held"
        );
        drop(outer);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        t.join().unwrap();
    }

    #[test]
    fn read_config_or_empty_treats_only_not_found_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        assert_eq!(read_config_or_empty(&missing).unwrap(), "");

        let bad_utf8 = dir.path().join("bad.json");
        fs::write(&bad_utf8, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_config_or_empty(&bad_utf8).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn read_config_or_empty_errors_on_permission_denied() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.json");
        fs::write(&path, "{\"oauthAccount\":1}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        // root 可以无视权限读取,此时该用例无意义
        if fs::read_to_string(&path).is_err() {
            assert!(read_config_or_empty(&path).is_err());
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn atomic_write_creates_and_replaces_without_leaving_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        atomic_write(&path, b"a = 1\n", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a = 1\n");
        atomic_write(&path, b"a = 2\n", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a = 2\n");
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1, "临时文件必须被 rename 掉");
    }

    /// 回归:用户 `chmod 444` 保护的配置文件,旧的 fs::write 会 EACCES 失败;
    /// rename 却能绕过只读位直接替换。目标不可写时必须报错且内容不动。
    #[test]
    fn atomic_write_refuses_to_replace_read_only_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{\"protected\":true}").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();
        // root(或 Windows 管理员绕过)能直接写只读文件时,该用例无意义
        if fs::OpenOptions::new().write(true).open(&path).is_err() {
            let err = atomic_write(&path, b"{}", None).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(fs::read_to_string(&path).unwrap(), "{\"protected\":true}");
            let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
            assert_eq!(entries.len(), 1, "no temp file left behind");
        }
        let mut perms = fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&path, perms).unwrap();
    }

    #[test]
    fn atomic_write_rejects_relative_path() {
        let err = atomic_write(Path::new(".codex/config.toml"), b"x", None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&path, b"{\"a\":1}", None).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_applies_explicit_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        fs::write(&path, "X=1").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        atomic_write(&path, b"GEMINI_API_KEY=ag_local_x\n", Some(0o600)).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let fresh = dir.path().join("token");
        atomic_write(&fresh, b"ag_local_y", Some(0o600)).unwrap();
        let mode = fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_symlink_instead_of_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-settings.json");
        fs::write(&real, "{}").unwrap();
        let link = dir.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        atomic_write(&link, b"{\"a\":1}", None).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "{\"a\":1}");

        // 悬空软链:写到链接指向的位置,链接本身保留
        let dangling = dir.path().join("dangling.json");
        std::os::unix::fs::symlink("not-yet.json", &dangling).unwrap();
        atomic_write(&dangling, b"{}", None).unwrap();
        assert!(fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(dir.path().join("not-yet.json")).unwrap(),
            "{}"
        );
    }
}
