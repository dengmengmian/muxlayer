//! 本地 Skills 管理（`~/.claude/skills/` 与 `~/.codex/skills/`）。
//!
//! 设计要点：
//! - **两个来源**：Claude Code 和 Codex 用完全相同的结构——`skills/<dir>/`
//!   下含 `SKILL.md`，frontmatter 里有 `name` / `description`。所有操作都带
//!   一个 `source`（`"claude"` / `"codex"`）来定位是哪一边。
//! - **以文件系统为真相源**，不建 DB 表。
//! - **启用/禁用靠重命名 manifest**：禁用时把 `SKILL.md` 改名为
//!   `SKILL.md.disabled`，客户端按 `skills/*/SKILL.md` 扫描就找不到它，等于禁用；
//!   启用时改回来。可逆、无 sidecar 状态文件。
//!   （注意：给**目录**加 `.disabled` 后缀不行——`foo.disabled/SKILL.md` 仍会被
//!   扫到，所以后缀必须落在 manifest 文件上。）
//! - **跳过隐藏目录**（`.system` 等以 `.` 开头的）：那是客户端内部目录，不是用户
//!   skill。
//! - **ZIP 导入只接受本地字节**：前端把用户选的 .zip 读成字节传进来，后端在内存
//!   里解压，做 zip-slip 防护，不碰网络下载。

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::AppError;

const MANIFEST: &str = "SKILL.md";
const MANIFEST_DISABLED: &str = "SKILL.md.disabled";

/// 支持的来源客户端。新增来源在这里加一条 + `skills_dir_for` 加分支即可。
const SOURCES: &[&str] = &["claude", "codex", "opencode", "gemini"];

/// 一个本地 skill（归一后的展示形态）。
#[derive(Debug, Clone, Serialize, PartialEq, specta::Type)]
pub struct Skill {
    /// 来源客户端：`"claude"` / `"codex"`。和 `id` 一起唯一定位一个 skill。
    pub source: String,
    /// 目录名，作为来源内的稳定标识（启用/禁用/删除都用它）。
    pub id: String,
    /// frontmatter 的 name，缺失时回退到目录名。
    pub name: String,
    /// frontmatter 的 description，缺失为空串。
    pub description: String,
    pub enabled: bool,
    pub path: String,
}

/// 导出/备份用结构（6.5）。文本文件内容内联，二进制文件跳过并上报。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SkillsExport {
    pub version: u32,
    pub skills: Vec<SkillExportItem>,
    /// 导出时因非 UTF-8 被跳过的文件相对路径，不静默丢弃。
    #[serde(default)]
    pub skipped_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SkillExportItem {
    /// 来源客户端。导入时按它写回对应目录；缺省回退 claude。
    #[serde(default = "default_source")]
    pub source: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    pub files: Vec<SkillFile>,
}

fn default_source() -> String {
    "claude".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SkillFile {
    pub rel_path: String,
    pub content: String,
}

/// 某来源的 skills 根目录。未知来源 / 家目录不可用返回 `None`
/// (写操作经 `require_dir` 会先报出家目录错误)。
fn skills_dir_for(source: &str) -> Option<PathBuf> {
    let home = crate::fsutil::home_dir().ok()?;
    match source {
        "claude" => Some(home.join(".claude").join("skills")),
        "codex" => Some(home.join(".codex").join("skills")),
        "opencode" => Some(home.join(".config").join("opencode").join("skills")),
        "gemini" => Some(home.join(".gemini").join("skills")),
        _ => None,
    }
}

/// 校验来源合法并返回其根目录。
fn require_dir(source: &str) -> Result<PathBuf, AppError> {
    crate::fsutil::home_dir()?;
    skills_dir_for(source).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::SKILL_BAD_SOURCE,
            format!("unknown source: {source}"),
        )
    })
}

/// 校验 skill id（目录名）：非空、无路径分隔符、无 `..`，防越权操作。
fn validate_id(id: &str) -> Result<(), AppError> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id == ".."
        || id == "."
        || id.contains("..")
    {
        return Err(AppError::new(
            crate::errors::codes::SKILL_BAD_ID,
            format!("invalid skill id: {id}"),
        ));
    }
    Ok(())
}

/// 从 SKILL.md 内容解析 frontmatter 的 name / description。
/// 只做最小行解析，不引 YAML 库——frontmatter 字段都是单行标量。
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, None);
    }
    let mut name = None;
    let mut description = None;
    for line in trimmed.lines().skip(1) {
        let line = line.trim_end();
        if line.trim() == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("name:") {
            name = Some(unquote(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = Some(unquote(rest.trim()));
        }
    }
    (name, description)
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// 读出一个 skill 目录的展示形态。`None` 表示该目录不是合法 skill（无 manifest）。
fn read_skill(source: &str, dir: &Path) -> Option<Skill> {
    let id = dir.file_name()?.to_string_lossy().to_string();
    let enabled_manifest = dir.join(MANIFEST);
    let disabled_manifest = dir.join(MANIFEST_DISABLED);
    let (enabled, manifest_path) = if enabled_manifest.is_file() {
        (true, enabled_manifest)
    } else if disabled_manifest.is_file() {
        (false, disabled_manifest)
    } else {
        return None;
    };
    let content = fs::read_to_string(&manifest_path).unwrap_or_default();
    let (fm_name, fm_desc) = parse_frontmatter(&content);
    Some(Skill {
        source: source.to_string(),
        name: fm_name
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.clone()),
        description: fm_desc.unwrap_or_default(),
        enabled,
        path: dir.to_string_lossy().to_string(),
        id,
    })
}

/// 列出所有来源的本地 skill。目录不存在或为空都返回对应空结果（首次使用很正常）。
pub fn list_skills() -> Vec<Skill> {
    let mut out = Vec::new();
    for source in SOURCES {
        let Some(dir) = skills_dir_for(source) else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // 跳过 .system 等客户端内部隐藏目录。
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if let Some(skill) = read_skill(source, &path) {
                out.push(skill);
            }
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.source.cmp(&b.source))
    });
    out
}

/// 启用/禁用：重命名 manifest 文件。已是目标状态则幂等返回。
pub fn set_skill_enabled(source: &str, id: &str, enabled: bool) -> Result<Skill, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    validate_id(id)?;
    let dir = require_dir(source)?.join(id);
    if !dir.is_dir() {
        return Err(AppError::new(
            crate::errors::codes::SKILL_NOT_FOUND,
            format!("skill not found: {source}/{id}"),
        ));
    }
    let active = dir.join(MANIFEST);
    let disabled = dir.join(MANIFEST_DISABLED);
    match (enabled, active.is_file(), disabled.is_file()) {
        (true, false, true) => rename(&disabled, &active)?,
        (false, true, false) => rename(&active, &disabled)?,
        (true, true, _) | (false, _, true) => {}
        _ => {
            return Err(AppError::new(
                crate::errors::codes::SKILL_NO_MANIFEST,
                format!("skill {source}/{id} has no SKILL.md"),
            ))
        }
    }
    read_skill(source, &dir).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::SKILL_NOT_FOUND,
            format!("skill not found after toggle: {source}/{id}"),
        )
    })
}

fn rename(from: &Path, to: &Path) -> Result<(), AppError> {
    fs::rename(from, to).map_err(|e| {
        AppError::new(
            crate::errors::codes::SKILL_TOGGLE_FAILED,
            format!("cannot rename {}: {e}", from.display()),
        )
    })
}

/// 删除一个 skill 目录。强二次确认在前端。
pub fn delete_skill(source: &str, id: &str) -> Result<bool, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    validate_id(id)?;
    let dir = require_dir(source)?.join(id);
    if !dir.is_dir() {
        return Ok(false);
    }
    fs::remove_dir_all(&dir).map_err(|e| {
        AppError::new(
            crate::errors::codes::SKILL_DELETE_FAILED,
            format!("cannot delete {}: {e}", dir.display()),
        )
    })?;
    Ok(true)
}

/// 从本地 ZIP 字节安装一个 skill 到指定来源客户端。
/// - 在内存解压，逐条做 zip-slip 防护（拒绝绝对路径 / `..`）。
/// - ZIP 里必须能找到 `SKILL.md`（根目录或单层子目录），否则拒绝。
/// - 目标目录已存在则拒绝，避免静默覆盖用户现有 skill。
pub fn import_skill_from_zip(source: &str, bytes: &[u8]) -> Result<Skill, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let root = require_dir(source)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| {
        AppError::new(
            crate::errors::codes::SKILL_ZIP_INVALID,
            format!("cannot read zip: {e}"),
        )
    })?;

    // 1) 先把所有文件读进内存（rel_path -> bytes），同时做 zip-slip 防护。
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| {
            AppError::new(
                crate::errors::codes::SKILL_ZIP_INVALID,
                format!("bad entry: {e}"),
            )
        })?;
        if file.is_dir() {
            continue;
        }
        let safe = file.enclosed_name().ok_or_else(|| {
            AppError::new(
                crate::errors::codes::SKILL_ZIP_UNSAFE,
                format!("unsafe path in zip: {}", file.name()),
            )
        })?;
        let rel = safe.to_string_lossy().replace('\\', "/");
        if safe_rel_path(&rel).is_none() {
            return Err(AppError::new(
                crate::errors::codes::SKILL_ZIP_UNSAFE,
                format!("unsafe path in zip: {}", file.name()),
            ));
        }
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).map_err(|e| {
            AppError::new(
                crate::errors::codes::SKILL_ZIP_INVALID,
                format!("read entry failed: {e}"),
            )
        })?;
        files.push((rel, buf));
    }

    // 2) 找 SKILL.md，确定要剥离的前缀（根 or 单层目录）。
    let manifest_rel = files
        .iter()
        .map(|(p, _)| p.as_str())
        .find(|p| *p == MANIFEST || p.ends_with(&format!("/{MANIFEST}")))
        .ok_or_else(|| {
            AppError::new(
                crate::errors::codes::SKILL_ZIP_NO_MANIFEST,
                "zip 内没有 SKILL.md".to_string(),
            )
        })?;
    let prefix = manifest_rel
        .strip_suffix(MANIFEST)
        .unwrap_or("")
        .to_string();

    // 3) 解析 frontmatter 拿 skill 名；回退到 zip 内目录名。
    let manifest_bytes = files
        .iter()
        .find(|(p, _)| p == manifest_rel)
        .map(|(_, b)| b.clone())
        .unwrap_or_default();
    let manifest_text = String::from_utf8_lossy(&manifest_bytes);
    let (fm_name, _) = parse_frontmatter(&manifest_text);
    let dir_name = fm_name
        .filter(|s| !s.is_empty())
        .or_else(|| {
            prefix
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| {
            AppError::new(
                crate::errors::codes::SKILL_ZIP_NO_NAME,
                "无法确定 skill 名（缺 frontmatter name 且 zip 无目录名）".to_string(),
            )
        })?;
    let dir_name = sanitize_dir_name(&dir_name);
    validate_id(&dir_name)?;

    let target = root.join(&dir_name);
    if target.exists() {
        return Err(AppError::new(
            crate::errors::codes::SKILL_EXISTS,
            format!("skill 已存在：{source}/{dir_name}，请先删除再导入"),
        ));
    }

    // 4) 写盘：剥前缀后落到 target 下；只写 prefix 内的文件。
    for (rel, data) in &files {
        let Some(stripped) = rel.strip_prefix(&prefix) else {
            continue;
        };
        if stripped.is_empty() {
            continue;
        }
        let dest = target.join(stripped);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AppError::new(
                    crate::errors::codes::SKILL_WRITE_FAILED,
                    format!("mkdir failed: {e}"),
                )
            })?;
        }
        fs::write(&dest, data).map_err(|e| {
            AppError::new(
                crate::errors::codes::SKILL_WRITE_FAILED,
                format!("write failed: {e}"),
            )
        })?;
    }

    read_skill(source, &target).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::SKILL_WRITE_FAILED,
            "导入后读取 skill 失败".to_string(),
        )
    })
}

/// frontmatter name 可能含空格/大写，转成安全目录名（kebab）。
fn sanitize_dir_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// 导出所有 skill 为可备份的 JSON 结构（6.5）。
/// 文本文件内联；非 UTF-8 文件跳过并记进 `skipped_files`，不静默丢。
pub fn export_skills() -> SkillsExport {
    let mut skills = Vec::new();
    let mut skipped = Vec::new();
    for skill in list_skills() {
        let dir = PathBuf::from(&skill.path);
        let mut files = Vec::new();
        collect_files(&dir, &dir, &mut files, &mut skipped);
        skills.push(SkillExportItem {
            source: skill.source,
            name: skill.name,
            description: skill.description,
            enabled: skill.enabled,
            files,
        });
    }
    SkillsExport {
        version: 1,
        skills,
        skipped_files: skipped,
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<SkillFile>, skipped: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out, skipped);
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            match fs::read(&path) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(content) => out.push(SkillFile {
                        rel_path: rel,
                        content,
                    }),
                    Err(_) => skipped.push(rel),
                },
                Err(_) => skipped.push(rel),
            }
        }
    }
}

/// 校验导入文件的相对路径,返回可安全 join 到 skill 目录下的路径。
///
/// 所有平台统一拒绝:绝对路径(`/x`、`\x`)、盘符(`C:\x`、`c:x`)、UNC(`\\server\share`)、
/// `..` 以及含 `:` 的分量(Windows ADS)。反斜杠先规范成 `/` 再按分量检查,最后再用
/// `Path::components` 兜一遍只允许 `Normal`。ZIP 导入在 `enclosed_name` 之后也走这里。
fn safe_rel_path(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\0') {
        return None;
    }
    let normalized = rel.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    let only_normal = out
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !only_normal || out.as_os_str().is_empty() {
        return None;
    }
    Some(out)
}

/// 从备份 JSON 恢复 skill。每条按自身 `source` 写回 `skills/<name>/`；已存在的目录
/// 跳过并上报，不覆盖用户现有 skill。返回成功导入的 skill 列表。
pub fn import_skills(payload: &str) -> Result<Vec<Skill>, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let export: SkillsExport = serde_json::from_str(payload).map_err(|e| {
        AppError::new(
            crate::errors::codes::SKILL_IMPORT_BAD_JSON,
            format!("invalid json: {e}"),
        )
    })?;
    let mut imported = Vec::new();
    for item in export.skills {
        let Some(root) = skills_dir_for(&item.source) else {
            continue; // 未知来源跳过
        };
        let dir_name = sanitize_dir_name(&item.name);
        if validate_id(&dir_name).is_err() {
            continue;
        }
        let target = root.join(&dir_name);
        if target.exists() {
            continue; // 不覆盖
        }
        // 先校验全部路径再落盘:任一路径不安全就整体拒绝,不留半个 skill 目录。
        let mut planned = Vec::with_capacity(item.files.len());
        for file in &item.files {
            let rel = if !item.enabled && file.rel_path == MANIFEST {
                MANIFEST_DISABLED.to_string()
            } else {
                file.rel_path.clone()
            };
            let safe = safe_rel_path(&rel).ok_or_else(|| {
                AppError::new(
                    crate::errors::codes::SKILL_ZIP_UNSAFE,
                    format!("unsafe path in import: {rel}"),
                )
            })?;
            planned.push((safe, &file.content));
        }
        for (rel, content) in planned {
            let dest = target.join(&rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    AppError::new(
                        crate::errors::codes::SKILL_WRITE_FAILED,
                        format!("mkdir failed: {e}"),
                    )
                })?;
            }
            fs::write(&dest, content).map_err(|e| {
                AppError::new(
                    crate::errors::codes::SKILL_WRITE_FAILED,
                    format!("write failed: {e}"),
                )
            })?;
        }
        if let Some(skill) = read_skill(&item.source, &target) {
            imported.push(skill);
        }
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = FS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = setup_temp_home();
        f();
        cleanup(&temp);
    }

    fn make_skill(source: &str, id: &str, name: &str, desc: &str, enabled: bool) {
        let dir = skills_dir_for(source).unwrap().join(id);
        fs::create_dir_all(&dir).unwrap();
        let manifest = if enabled { MANIFEST } else { MANIFEST_DISABLED };
        let body = format!("---\nname: {name}\ndescription: {desc}\n---\n\n# {name}\n");
        fs::write(dir.join(manifest), body).unwrap();
    }

    #[test]
    fn list_empty_when_no_dir() {
        with_temp_home(|| {
            assert!(list_skills().is_empty());
        });
    }

    #[test]
    fn list_merges_claude_and_codex() {
        with_temp_home(|| {
            make_skill("claude", "alpha", "Alpha", "a", true);
            make_skill("codex", "beta", "Beta", "b", true);
            let skills = list_skills();
            assert_eq!(skills.len(), 2);
            assert!(skills
                .iter()
                .any(|s| s.source == "claude" && s.id == "alpha"));
            assert!(skills.iter().any(|s| s.source == "codex" && s.id == "beta"));
        });
    }

    #[test]
    fn same_name_in_both_sources_kept_separate() {
        with_temp_home(|| {
            make_skill("claude", "pdf", "Pdf", "", true);
            make_skill("codex", "pdf", "Pdf", "", true);
            let skills = list_skills();
            assert_eq!(skills.len(), 2);
            assert_eq!(skills.iter().filter(|s| s.id == "pdf").count(), 2);
        });
    }

    #[test]
    fn hidden_dirs_skipped() {
        with_temp_home(|| {
            let dir = skills_dir_for("codex").unwrap().join(".system");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(MANIFEST), "---\nname: hidden\n---\n").unwrap();
            assert!(list_skills().is_empty());
        });
    }

    #[test]
    fn list_reads_frontmatter_and_enabled_state() {
        with_temp_home(|| {
            make_skill("claude", "alpha", "Alpha Skill", "does alpha", true);
            make_skill("claude", "beta", "Beta Skill", "does beta", false);
            let skills = list_skills();
            let alpha = skills.iter().find(|s| s.id == "alpha").unwrap();
            assert_eq!(alpha.name, "Alpha Skill");
            assert_eq!(alpha.description, "does alpha");
            assert!(alpha.enabled);
            assert!(!skills.iter().find(|s| s.id == "beta").unwrap().enabled);
        });
    }

    #[test]
    fn name_falls_back_to_dir_when_no_frontmatter() {
        with_temp_home(|| {
            let dir = skills_dir_for("claude").unwrap().join("plain");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(MANIFEST), "no frontmatter here").unwrap();
            let skills = list_skills();
            assert_eq!(skills[0].name, "plain");
            assert_eq!(skills[0].description, "");
        });
    }

    #[test]
    fn disable_then_enable_renames_manifest() {
        with_temp_home(|| {
            make_skill("codex", "alpha", "Alpha", "x", true);
            let dir = skills_dir_for("codex").unwrap().join("alpha");

            let s = set_skill_enabled("codex", "alpha", false).unwrap();
            assert!(!s.enabled);
            assert!(!dir.join(MANIFEST).exists());
            assert!(dir.join(MANIFEST_DISABLED).exists());

            let s = set_skill_enabled("codex", "alpha", true).unwrap();
            assert!(s.enabled);
            assert!(dir.join(MANIFEST).exists());
            assert!(!dir.join(MANIFEST_DISABLED).exists());
        });
    }

    #[test]
    fn toggle_is_idempotent() {
        with_temp_home(|| {
            make_skill("claude", "alpha", "Alpha", "x", true);
            assert!(set_skill_enabled("claude", "alpha", true).unwrap().enabled);
        });
    }

    #[test]
    fn toggle_unknown_skill_errors() {
        with_temp_home(|| {
            assert_eq!(
                set_skill_enabled("claude", "nope", false).unwrap_err().code,
                "SKILL_NOT_FOUND"
            );
        });
    }

    #[test]
    fn bad_source_rejected() {
        with_temp_home(|| {
            assert_eq!(
                set_skill_enabled("not-a-client", "x", false)
                    .unwrap_err()
                    .code,
                "SKILL_BAD_SOURCE"
            );
        });
    }

    #[test]
    fn bad_id_rejected() {
        with_temp_home(|| {
            assert_eq!(
                set_skill_enabled("claude", "../etc", false)
                    .unwrap_err()
                    .code,
                "SKILL_BAD_ID"
            );
            assert_eq!(
                delete_skill("claude", "a/b").unwrap_err().code,
                "SKILL_BAD_ID"
            );
        });
    }

    #[test]
    fn delete_removes_dir() {
        with_temp_home(|| {
            make_skill("claude", "alpha", "Alpha", "x", true);
            assert!(delete_skill("claude", "alpha").unwrap());
            assert!(list_skills().is_empty());
            assert!(!delete_skill("claude", "alpha").unwrap());
        });
    }

    #[test]
    fn parse_frontmatter_handles_quotes() {
        let (name, desc) =
            parse_frontmatter("---\nname: \"My Skill\"\ndescription: 'hi: there'\n---\n");
        assert_eq!(name.unwrap(), "My Skill");
        assert_eq!(desc.unwrap(), "hi: there");
    }

    #[test]
    fn sanitize_dir_name_kebabs() {
        assert_eq!(sanitize_dir_name("My Cool Skill"), "my-cool-skill");
        assert_eq!(sanitize_dir_name("  edge--case  "), "edge--case");
    }

    #[test]
    fn export_then_import_round_trips_with_source() {
        with_temp_home(|| {
            make_skill("codex", "alpha", "Alpha", "does alpha", true);
            let dir = skills_dir_for("codex").unwrap().join("alpha");
            fs::write(dir.join("helper.py"), "print('hi')").unwrap();

            let export = export_skills();
            assert_eq!(export.skills.len(), 1);
            assert_eq!(export.skills[0].source, "codex");
            let json = serde_json::to_string(&export).unwrap();

            delete_skill("codex", "alpha").unwrap();
            assert!(list_skills().is_empty());

            let imported = import_skills(&json).unwrap();
            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].source, "codex");
            assert!(dir.join("helper.py").exists());
        });
    }

    /// 回归:JSON 导入只拦 `..` 和前导 `/`;Windows 上 `C:\\x`、`\\\\server\\share`
    /// 传给 Path::join 会直接替换目标目录 → 任意写。现在任何平台都整体拒绝。
    #[test]
    fn import_rejects_absolute_and_prefixed_rel_paths() {
        with_temp_home(|| {
            for bad in [
                r"C:\evil.txt",
                r"c:evil.txt",
                r"\\server\share\evil.txt",
                r"\evil.txt",
                "/etc/evil",
                r"..\evil.txt",
                "a/../../evil.txt",
                "notes.txt:stream",
            ] {
                let payload = serde_json::json!({
                    "version": 1,
                    "skills": [{
                        "source": "claude",
                        "name": "gamma",
                        "enabled": true,
                        "files": [
                            {"rel_path": "SKILL.md", "content": "---\nname: gamma\n---\n"},
                            {"rel_path": bad, "content": "pwned"}
                        ]
                    }]
                });
                let err = import_skills(&payload.to_string()).unwrap_err();
                assert_eq!(err.code, crate::errors::codes::SKILL_ZIP_UNSAFE, "{bad}");
                let dir = skills_dir_for("claude").unwrap().join("gamma");
                assert!(!dir.exists(), "{bad}: nothing may be written");
            }
        });
    }

    #[test]
    fn safe_rel_path_accepts_nested_relative_paths() {
        assert_eq!(
            safe_rel_path("scripts/helper.py"),
            Some(PathBuf::from("scripts").join("helper.py"))
        );
        assert_eq!(
            safe_rel_path(r"scripts\helper.py"),
            Some(PathBuf::from("scripts").join("helper.py"))
        );
        assert_eq!(safe_rel_path("./SKILL.md"), Some(PathBuf::from("SKILL.md")));
        assert_eq!(safe_rel_path(""), None);
    }

    #[test]
    fn import_skips_existing() {
        with_temp_home(|| {
            make_skill("claude", "alpha", "Alpha", "x", true);
            let json = serde_json::to_string(&export_skills()).unwrap();
            assert!(import_skills(&json).unwrap().is_empty());
        });
    }

    #[test]
    fn import_disabled_skill_writes_disabled_manifest() {
        with_temp_home(|| {
            make_skill("claude", "beta", "Beta", "x", false);
            let json = serde_json::to_string(&export_skills()).unwrap();
            delete_skill("claude", "beta").unwrap();
            import_skills(&json).unwrap();
            let dir = skills_dir_for("claude").unwrap().join("beta");
            assert!(dir.join(MANIFEST_DISABLED).exists());
            assert!(!dir.join(MANIFEST).exists());
        });
    }

    fn build_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (path, content) in entries {
                use std::io::Write as _;
                writer.start_file(*path, opts).unwrap();
                writer.write_all(content.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn import_zip_into_codex() {
        with_temp_home(|| {
            let zip = build_zip(&[
                (
                    "SKILL.md",
                    "---\nname: Zipped Skill\ndescription: from zip\n---\n",
                ),
                ("run.sh", "echo hi"),
            ]);
            let skill = import_skill_from_zip("codex", &zip).unwrap();
            assert_eq!(skill.source, "codex");
            assert_eq!(skill.id, "zipped-skill");
            let dir = skills_dir_for("codex").unwrap().join("zipped-skill");
            assert!(dir.join("SKILL.md").exists());
            assert!(dir.join("run.sh").exists());
        });
    }

    #[test]
    fn import_zip_with_top_dir_strips_prefix() {
        with_temp_home(|| {
            let zip = build_zip(&[
                ("my-skill/SKILL.md", "---\nname: My Skill\n---\n"),
                ("my-skill/data.txt", "x"),
            ]);
            let skill = import_skill_from_zip("claude", &zip).unwrap();
            assert_eq!(skill.id, "my-skill");
            let dir = skills_dir_for("claude").unwrap().join("my-skill");
            assert!(dir.join("SKILL.md").exists());
            assert!(dir.join("data.txt").exists());
        });
    }

    #[test]
    fn import_zip_without_manifest_errors() {
        with_temp_home(|| {
            let zip = build_zip(&[("readme.txt", "no manifest")]);
            assert_eq!(
                import_skill_from_zip("claude", &zip).unwrap_err().code,
                "SKILL_ZIP_NO_MANIFEST"
            );
        });
    }

    #[test]
    fn import_zip_bad_source_errors() {
        with_temp_home(|| {
            let zip = build_zip(&[("SKILL.md", "---\nname: X\n---\n")]);
            assert_eq!(
                import_skill_from_zip("not-a-client", &zip)
                    .unwrap_err()
                    .code,
                "SKILL_BAD_SOURCE"
            );
        });
    }

    #[test]
    fn import_zip_rejects_existing() {
        with_temp_home(|| {
            make_skill("claude", "zipped-skill", "Zipped Skill", "x", true);
            let zip = build_zip(&[("SKILL.md", "---\nname: Zipped Skill\n---\n")]);
            assert_eq!(
                import_skill_from_zip("claude", &zip).unwrap_err().code,
                "SKILL_EXISTS"
            );
        });
    }
}
