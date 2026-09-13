//! Claude Desktop（GUI app）接入：指向 AgentGate 本地网关。
//!
//! Claude Desktop 用的是官方的 "third-party inference gateway"（3p）机制：
//! 在 `Claude-3p/configLibrary/` 下放一个 profile JSON，profile 指向一个推理网关
//! base URL + key。我们把这个网关指向 AgentGate（127.0.0.1:<port>），请求进网关后
//! 由 route_profile + model mapping 路由到真实上游。模型列表由 Claude Desktop 向
//! gateway 动态拉取（实测 profile 里不含 inferenceModels），无需预枚举。
//!
//! ⚠️ schema（字段名、路径、_meta 结构）基于对同类工具的实现推断，**不同 Claude
//! Desktop 版本可能不同**。第一阶段只做 detect（只读）+ generate_profile（预览，
//! 不写盘），用于和用户机器上实际的 3p 配置对比、确认 schema 后再做写盘 apply。
//!
//! 路径支持 macOS（~/Library/Application Support）和 Windows（%APPDATA%），
//! 两者目录布局相同（Claude/ 与 Claude-3p/）；Linux 无官方包，明确报不支持。
//! ⚠️ Windows 的 Claude-3p 目录位置基于「与 1p 配置同级」推断，待真机确认。

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

use crate::errors::AppError;

/// AgentGate 在 Claude Desktop configLibrary 里固定使用的 profile id（即文件名，不带
/// 扩展名）。用 UUID 格式和官方 Default profile 一致；固定值便于 detect「是否已接入」。
const PROFILE_ID: &str = "00000000-0000-4000-8000-00000a6e7a7e";

/// Claude Desktop 3p 相关路径（macOS）。
pub struct DesktopPaths {
    /// 官方 1p 配置（含 deploymentMode）
    pub normal_config: PathBuf,
    /// 3p 配置
    pub threep_config: PathBuf,
    /// AgentGate 的 profile
    pub profile: PathBuf,
    /// profile 元数据（记录当前 appliedId）
    pub meta: PathBuf,
}

/// 由「应用数据根目录」拼出全部相关路径。macOS 传 `~/Library/Application Support`，
/// Windows 传 `%APPDATA%`。纯路径拼接，平台无关，便于单测。
// Linux 无官方包,paths() 不调用它;不门控会在 Linux 构建里成为死代码。
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn paths_from_base(base: &std::path::Path) -> DesktopPaths {
    let threep = base.join("Claude-3p");
    DesktopPaths {
        normal_config: base.join("Claude").join("claude_desktop_config.json"),
        threep_config: threep.join("claude_desktop_config.json"),
        profile: threep
            .join("configLibrary")
            .join(format!("{PROFILE_ID}.json")),
        meta: threep.join("configLibrary").join("_meta.json"),
    }
}

/// 解析当前平台的 Claude Desktop 路径。支持 macOS 和 Windows。
pub fn paths() -> Result<DesktopPaths, AppError> {
    #[cfg(target_os = "macos")]
    {
        let app_support = crate::fsutil::home_dir()?.join("Library/Application Support");
        Ok(paths_from_base(&app_support))
    }
    #[cfg(target_os = "windows")]
    {
        // Windows 官方配置目录在 %APPDATA%\Claude\（Roaming）。
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        if appdata.trim().is_empty() {
            return Err(AppError::new(
                crate::errors::codes::CLAUDE_DESKTOP_PATH_INVALID,
                "无法读取 %APPDATA% 环境变量，无法定位 Claude Desktop 配置目录",
            ));
        }
        Ok(paths_from_base(std::path::Path::new(&appdata)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(AppError::new(
            crate::errors::codes::CLAUDE_DESKTOP_UNSUPPORTED_OS,
            "Claude Desktop 接入目前仅支持 macOS 和 Windows",
        ))
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ClaudeDesktopStatus {
    /// 当前平台是否支持
    pub supported: bool,
    pub normal_config_path: String,
    pub threep_config_path: String,
    pub profile_path: String,
    /// Claude Desktop 是否安装（normal config 目录存在）
    pub installed: bool,
    /// 是否已存在 MuxLayer profile
    pub has_agentgate_profile: bool,
    /// _meta.json 里当前生效的 profile（用于判断是否「已接入」）
    pub applied_profile_id: Option<String>,
    /// 当前 deploymentMode（"1p" / "3p" / None）
    pub deployment_mode: Option<String>,
}

/// 读取 JSON 文件，不存在或解析失败返回 None。
fn read_json(path: &PathBuf) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 检测 Claude Desktop 现状（只读，不改任何文件）。
pub fn detect() -> ClaudeDesktopStatus {
    let p = match paths() {
        Ok(p) => p,
        Err(_) => {
            return ClaudeDesktopStatus {
                supported: false,
                normal_config_path: String::new(),
                threep_config_path: String::new(),
                profile_path: String::new(),
                installed: false,
                has_agentgate_profile: false,
                applied_profile_id: None,
                deployment_mode: None,
            };
        }
    };

    // 安装判断：Claude app support 目录（normal_config 的父目录）存在。
    let installed = p
        .normal_config
        .parent()
        .map(|d| d.exists())
        .unwrap_or(false);

    let deployment_mode = read_json(&p.normal_config)
        .as_ref()
        .and_then(|v| v.get("deploymentMode"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let applied_profile_id = read_json(&p.meta)
        .as_ref()
        .and_then(|v| v.get("appliedId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    ClaudeDesktopStatus {
        supported: true,
        normal_config_path: p.normal_config.display().to_string(),
        threep_config_path: p.threep_config.display().to_string(),
        profile_path: p.profile.display().to_string(),
        installed,
        has_agentgate_profile: p.profile.exists(),
        applied_profile_id,
        deployment_mode,
    }
}

/// 生成指向 MuxLayer 网关的 3p profile JSON（不写盘，用于预览/对比）。
/// host/port 是 MuxLayer 网关地址，token 是 ag_local 本地访问令牌。
pub fn generate_profile(host: &str, port: i64, token: &str) -> Value {
    let base_url = format!("http://{host}:{port}");
    // 极简 3 字段——用户机器上实测确认的 Claude Desktop 3p profile 真实 schema。
    // 外部工具那套 authScheme / inferenceModels 等额外字段实际不存在：Claude Desktop
    // 从 gateway 动态拉模型列表，不需要预枚举模型。
    json!({
        "inferenceProvider": "gateway",
        "inferenceGatewayBaseUrl": base_url,
        "inferenceGatewayApiKey": token,
    })
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ClaudeDesktopApplyResult {
    pub success: bool,
    pub profile_path: String,
    pub base_url: String,
    pub warnings: Vec<String>,
}

/// apply 会改动的文件——给 apply_history 做快照/回滚用。
pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    match paths() {
        Ok(p) => vec![("profile", p.profile), ("meta", p.meta)],
        Err(_) => vec![],
    }
}

/// 原子写 JSON + 读回校验。`mode` 见 [`crate::fsutil::atomic_write`]。
fn write_json(path: &std::path::Path, value: &Value, mode: Option<u32>) -> Result<(), AppError> {
    let data = serde_json::to_vec_pretty(value)
        .map_err(|e| AppError::internal(format!("serialize json: {e}")))?;
    crate::tools::client_files::write_verified(
        path,
        &data,
        mode,
        crate::errors::codes::CLAUDE_DESKTOP_WRITE_FAILED,
    )
}

/// 计算 surgical merge 后的 _meta.json：保留用户已有 entries，加/更新 AgentGate 条目，
/// 把 appliedId 切到 AgentGate。不动其他字段。_meta.json 读不出来或不是合法
/// JSON 时直接报错,绝不当成 `{}` 覆盖用户已有的 profile 列表。
fn merged_meta(meta_path: &std::path::Path, id: &str, name: &str) -> Result<Value, AppError> {
    let content = crate::tools::client_files::read_for_update(
        meta_path,
        crate::errors::codes::CLAUDE_DESKTOP_META_INVALID,
    )?;
    let mut meta: Value = if content.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&content).map_err(|e| {
            AppError::new(
                crate::errors::codes::CLAUDE_DESKTOP_META_INVALID,
                format!("_meta.json 解析失败: {e}"),
            )
        })?
    };
    let obj = meta.as_object_mut().ok_or_else(|| {
        AppError::new(
            crate::errors::codes::CLAUDE_DESKTOP_META_INVALID,
            "_meta.json 不是 JSON 对象",
        )
    })?;
    let mut entries = obj
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    entries.retain(|e| e.get("id").and_then(|v| v.as_str()) != Some(id));
    entries.push(json!({ "id": id, "name": name }));
    obj.insert("entries".to_string(), Value::Array(entries));
    obj.insert("appliedId".to_string(), Value::String(id.to_string()));
    Ok(meta)
}

/// 接入 Claude Desktop：在 configLibrary 写一个指向 MuxLayer 网关的 profile，并把
/// _meta 的 appliedId 切过去。**只在 configLibrary 内操作**，不碰官方 1p 配置。
/// 要求用户已启用过 3p（configLibrary 目录存在）——避免去猜 deploymentMode 写法。
/// 回滚由调用方经 apply_history 快照 snapshot_paths() 完成。
pub fn apply(host: &str, port: i64, token: &str) -> Result<ClaudeDesktopApplyResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let p = paths()?;
    let lib_dir = p.profile.parent().ok_or_else(|| {
        AppError::new(
            crate::errors::codes::CLAUDE_DESKTOP_PATH_INVALID,
            "无法解析 configLibrary 路径",
        )
    })?;
    if !lib_dir.exists() {
        return Err(AppError::new(
            crate::errors::codes::CLAUDE_DESKTOP_NO_3P,
            "未检测到 Claude Desktop 的第三方网关(3p)配置目录。请先在 Claude Desktop 里启用一次第三方推理网关，再回来应用。",
        ));
    }
    let base_url = format!("http://{host}:{port}");
    let profile = generate_profile(host, port, token);
    // 先读取并合并 _meta.json(读不出 / 非法则整体中止),再写含 token 的
    // profile(0600),最后切 appliedId——保证 appliedId 不会指向不存在的 profile。
    let meta = merged_meta(&p.meta, PROFILE_ID, "MuxLayer")?;
    write_json(
        &p.profile,
        &profile,
        Some(crate::tools::client_files::SECRET_FILE_MODE),
    )?;
    write_json(&p.meta, &meta, None)?;
    Ok(ClaudeDesktopApplyResult {
        success: true,
        profile_path: p.profile.display().to_string(),
        base_url,
        warnings: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_base_layout() {
        let base = std::path::Path::new("/tmp/AppDataRoot");
        let p = paths_from_base(base);
        // 全部挂在 base 之下
        assert!(p.normal_config.starts_with(base));
        assert!(p.threep_config.starts_with(base));
        assert!(p.profile.starts_with(base));
        assert!(p.meta.starts_with(base));
        // 相对布局与 macOS 现状完全一致
        assert!(p
            .normal_config
            .ends_with("Claude/claude_desktop_config.json"));
        assert!(p
            .threep_config
            .ends_with("Claude-3p/claude_desktop_config.json"));
        assert!(p
            .profile
            .ends_with(format!("Claude-3p/configLibrary/{PROFILE_ID}.json")));
        assert!(p.meta.ends_with("Claude-3p/configLibrary/_meta.json"));
    }

    /// 回归:_meta.json 读不出来 / 解析失败时,旧实现当成 `{}` 写回,用户在
    /// Claude Desktop 里配过的其它 profile 条目全部丢失。
    #[cfg(target_os = "macos")]
    #[test]
    fn apply_refuses_to_overwrite_corrupt_meta() {
        let _guard = crate::test_utils::FS_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = crate::test_utils::setup_temp_home();
        let p = paths().unwrap();
        std::fs::create_dir_all(p.meta.parent().unwrap()).unwrap();
        let original: &[u8] = b"{\"entries\":[{\"id\":\"mine\"}], \xff";
        std::fs::write(&p.meta, original).unwrap();
        assert!(apply("127.0.0.1", 9090, "ag_local_x").is_err());
        assert_eq!(std::fs::read(&p.meta).unwrap(), original);
        crate::test_utils::cleanup(&temp);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apply_writes_profile_owner_only_and_keeps_other_entries() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::test_utils::FS_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = crate::test_utils::setup_temp_home();
        let p = paths().unwrap();
        std::fs::create_dir_all(p.meta.parent().unwrap()).unwrap();
        std::fs::write(
            &p.meta,
            r#"{"entries":[{"id":"mine","name":"Mine"}],"appliedId":"mine"}"#,
        )
        .unwrap();
        apply("127.0.0.1", 9090, "ag_local_x").unwrap();
        let mode = std::fs::metadata(&p.profile).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "profile carries the gateway token");
        let meta: Value = serde_json::from_slice(&std::fs::read(&p.meta).unwrap()).unwrap();
        assert!(meta["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == "mine"));
        assert_eq!(meta["appliedId"], PROFILE_ID);
        crate::test_utils::cleanup(&temp);
    }

    #[test]
    fn generate_profile_shape() {
        let p = generate_profile("127.0.0.1", 9090, "ag_local_test");
        assert_eq!(p["inferenceProvider"], "gateway");
        assert_eq!(p["inferenceGatewayBaseUrl"], "http://127.0.0.1:9090");
        assert_eq!(p["inferenceGatewayApiKey"], "ag_local_test");
    }
}
