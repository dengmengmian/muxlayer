use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

use crate::errors::AppError;
use crate::storage::migrations;

/// 全局数据库连接池类型别名。AppState 持有 Clone 的 handle,
/// 各处用 `pool.get()` 拿 `PooledConnection`(deref 到 `Connection`)。
pub type DbPool = Pool<SqliteConnectionManager>;

/// 从池借出的连接(deref 到 `Connection`)。一次性 CLI 子命令借一条用完即还。
pub type DbConn = r2d2::PooledConnection<SqliteConnectionManager>;

/// 池大小。SQLite WAL 模式允许多 reader 并发,写仍内部串行。
/// 桌面应用 QPS 不大,4 个 connection 足够吸收短期 burst。
const POOL_MAX_SIZE: u32 = 8;

/// 每个连接初始化时统一开 WAL + 外键约束,跟旧单连接实现保持行为一致。
fn init_connection(conn: &mut Connection) -> rusqlite::Result<()> {
    // WAL 下多连接并发写仍会 SQLITE_BUSY。给 5s 重试窗口,避免高并发瞬间直接失败。
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    // Set the timeout before switching journal mode: r2d2 may initialize a
    // second connection while the first one is still creating the database.
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    // WAL 下 NORMAL 仍保证崩溃一致性(仅可能丢最后几个未 checkpoint 的事务),
    // 避免每次提交都 fsync;gateway/session_store.rs 同样设置。
    conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
    Ok(())
}

pub fn init_database(app_data_dir: &PathBuf) -> Result<DbPool, AppError> {
    // 目录是否由本次初始化创建:用户经 MUXLAYER_DB_PATH / --db-path 指定的已有目录
    // (如 /tmp、$HOME、bind mount)不能被改权限。
    let dir_created = !app_data_dir.exists();
    fs::create_dir_all(app_data_dir)
        .map_err(|e| AppError::internal(format!("Failed to create app data directory: {e}")))?;

    let db_path = app_data_dir.join("agentgate.db");

    let manager = SqliteConnectionManager::file(&db_path).with_init(init_connection);
    let pool = Pool::builder()
        .max_size(POOL_MAX_SIZE)
        .build(manager)
        .map_err(|e| AppError::internal(format!("Failed to build DB pool: {e}")))?;

    // migrations 在 pool ready 后跑一次:借一个连接、跑完归还。
    {
        let conn = pool
            .get()
            .map_err(|e| AppError::internal(format!("Failed to acquire connection: {e}")))?;
        migrations::run_migrations(&conn)?;
    }

    // 收紧权限是加固而非启动前提:失败只告警,不能让应用起不来。
    for warning in restrict_permissions(app_data_dir, dir_created, &db_path) {
        tracing::warn!("{warning}");
    }

    Ok(pool)
}

/// DB 内含明文 provider api_key 与原始请求/响应体,收紧为仅属主可读写:
/// agentgate.db / -wal / -shm 0600(与 token 文件一致);目录仅在本次新建时改 0700。
/// SQLite 创建 -wal / -shm 时沿用主库文件权限,所以主库改完后续新建的也是 0600。
/// 返回每个失败步骤的告警文案,由调用方记日志。
#[cfg(unix)]
fn restrict_permissions(
    dir: &std::path::Path,
    dir_created: bool,
    db_path: &std::path::Path,
) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let mut warnings = Vec::new();
    let mut set = |p: &std::path::Path, mode: u32| {
        if let Err(e) = fs::set_permissions(p, fs::Permissions::from_mode(mode)) {
            warnings.push(format!(
                "Failed to restrict permissions of {}: {e}",
                p.display()
            ));
        }
    };
    if dir_created {
        set(dir, 0o700);
    }
    set(db_path, 0o600);
    for suffix in ["-wal", "-shm"] {
        let mut name = db_path.as_os_str().to_owned();
        name.push(suffix);
        let p = std::path::PathBuf::from(name);
        if p.exists() {
            set(&p, 0o600);
        }
    }
    warnings
}

#[cfg(not(unix))]
fn restrict_permissions(
    _dir: &std::path::Path,
    _dir_created: bool,
    _db_path: &std::path::Path,
) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_database_in_memory() {
        let temp = std::env::temp_dir().join("agentgate_test_db");
        let pool = init_database(&temp).unwrap();
        let conn = pool.get().unwrap();
        // Verify WAL mode is enabled
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");
        // Verify foreign keys are enabled
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        // Verify key tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(tables.contains(&"providers".to_string()));
        assert!(tables.contains(&"gateway_settings".to_string()));
        assert!(tables.contains(&"route_profiles".to_string()));
        assert!(tables.contains(&"request_logs".to_string()));
        assert!(tables.contains(&"model_pricing".to_string()));
        assert!(tables.contains(&"pet_settings".to_string()));
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_pool_concurrent_reads() {
        let temp = std::env::temp_dir().join("agentgate_test_db_concurrent");
        let pool = init_database(&temp).unwrap();
        // 同时拿 2 个 connection,确认 Pool 没把它们 serialize
        let c1 = pool.get().unwrap();
        let c2 = pool.get().unwrap();
        let n1: i64 = c1.query_row("SELECT 1", [], |r| r.get(0)).unwrap();
        let n2: i64 = c2.query_row("SELECT 2", [], |r| r.get(0)).unwrap();
        assert_eq!(n1, 1);
        assert_eq!(n2, 2);
        drop(c1);
        drop(c2);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn connections_use_synchronous_normal() {
        let temp = tempfile::tempdir().unwrap();
        let pool = init_database(&temp.path().to_path_buf()).unwrap();
        let conn = pool.get().unwrap();
        let sync: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, 1, "WAL 下应为 NORMAL(1)");
    }

    #[cfg(unix)]
    #[test]
    fn database_files_are_private_to_owner() {
        // DB 里有明文 api_key 与原始请求体,不能按默认 umask 创建成 0644。
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("appdata");
        let pool = init_database(&dir).unwrap();
        let _conn = pool.get().unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("agentgate.db")), 0o600);
        for suffix in ["agentgate.db-wal", "agentgate.db-shm"] {
            let p = dir.join(suffix);
            if p.exists() {
                assert_eq!(mode(&p), 0o600, "{suffix}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn preexisting_directory_mode_is_untouched() {
        // 用户指定的已有目录(如 $HOME、/tmp、bind mount)不能被改成 0700,只收紧 DB 文件。
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("shared");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pool = init_database(&dir).unwrap();
        let _conn = pool.get().unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o755, "已有目录权限必须保持原样");
        assert_eq!(mode(&dir.join("agentgate.db")), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn permission_failures_become_warnings_not_errors() {
        // chmod 失败(文件系统不支持、非属主等)只产生告警,不能让初始化失败。
        let temp = tempfile::tempdir().unwrap();
        let missing_dir = temp.path().join("gone");
        let missing_db = missing_dir.join("agentgate.db");
        let warnings = restrict_permissions(&missing_dir, true, &missing_db);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings
            .iter()
            .all(|w| w.contains("Failed to restrict permissions")));
    }
}
