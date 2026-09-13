use tauri::State;

use super::record_pre_apply;
use crate::app::state::AppState;
use crate::errors::AppError;

// ── Global Instructions (CLAUDE.md / AGENTS.md) ────────────────

/// 列出内置模板。模板是只读静态资源，不需要参数也不消耗 DB。
#[tauri::command]
#[specta::specta]
pub fn list_instructions_templates(
) -> Result<Vec<crate::tools::instructions_templates::InstructionsTemplate>, AppError> {
    // 静态 slice → Vec 让 Tauri 能序列化。
    Ok(crate::tools::instructions_templates::TEMPLATES.to_vec())
}

/// 读取某 scope（claude_global / codex_global）的全局指令文件原文。
/// 文件不存在时返回 `exists=false, content=""`，让前端 textarea 仍可编辑。
#[tauri::command]
#[specta::specta]
pub fn read_global_instructions(
    scope: String,
) -> Result<crate::tools::instructions::InstructionsStatus, AppError> {
    let s = crate::tools::instructions::InstructionsScope::from_str(&scope).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::INSTRUCTIONS_BAD_SCOPE,
            format!("unknown scope: {scope}"),
        )
    })?;
    Ok(crate::tools::instructions::read(s))
}

/// 手动编辑后保存。和 5 个客户端的 apply 流程一致：写盘前 snapshot 一次磁盘
/// 原文，便于事后回滚。
#[tauri::command]
#[specta::specta]
pub fn write_global_instructions(
    state: State<'_, AppState>,
    scope: String,
    content: String,
) -> Result<crate::tools::instructions::InstructionsStatus, AppError> {
    let s = crate::tools::instructions::InstructionsScope::from_str(&scope).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::INSTRUCTIONS_BAD_SCOPE,
            format!("unknown scope: {scope}"),
        )
    })?;
    record_pre_apply(
        &state.db,
        s.history_client_id(),
        "write",
        crate::tools::instructions::snapshot_paths(s),
        "manual edit",
    );
    crate::tools::instructions::write(s, &content)
}

/// 把模板按 overwrite / append 写入目标 scope。同样在写盘前打 snapshot。
#[tauri::command]
#[specta::specta]
pub fn apply_instructions_template(
    state: State<'_, AppState>,
    scope: String,
    template_id: String,
    mode: String,
) -> Result<crate::tools::instructions::InstructionsStatus, AppError> {
    let s = crate::tools::instructions::InstructionsScope::from_str(&scope).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::INSTRUCTIONS_BAD_SCOPE,
            format!("unknown scope: {scope}"),
        )
    })?;
    let m = crate::tools::instructions::ApplyMode::from_str(&mode).ok_or_else(|| {
        AppError::new(
            crate::errors::codes::INSTRUCTIONS_BAD_MODE,
            format!("unknown mode: {mode}"),
        )
    })?;
    let summary = format!("template {template_id} ({mode})");
    record_pre_apply(
        &state.db,
        s.history_client_id(),
        "apply_template",
        crate::tools::instructions::snapshot_paths(s),
        &summary,
    );
    crate::tools::instructions::apply_template(s, &template_id, m)
}

/// 导出两个 scope 的全局指令为一份 JSON 备份（6.5）。
#[tauri::command]
#[specta::specta]
pub fn export_instructions() -> Result<crate::tools::instructions::InstructionsBackup, AppError> {
    Ok(crate::tools::instructions::export_backup())
}

/// 从备份 JSON 恢复全局指令。每个非空 scope overwrite 写入，写盘前各打一次
/// snapshot，复用现有回滚机制。返回恢复后的两个 scope 状态。
#[tauri::command]
#[specta::specta]
pub fn import_instructions(
    state: State<'_, AppState>,
    payload: String,
) -> Result<Vec<crate::tools::instructions::InstructionsStatus>, AppError> {
    let backup: crate::tools::instructions::InstructionsBackup = serde_json::from_str(&payload)
        .map_err(|e| {
            AppError::new(
                crate::errors::codes::INSTRUCTIONS_IMPORT_BAD_JSON,
                format!("invalid json: {e}"),
            )
        })?;
    use crate::tools::instructions::InstructionsScope;
    let mut out = Vec::new();
    for (scope, content) in [
        (InstructionsScope::ClaudeGlobal, backup.claude),
        (InstructionsScope::CodexGlobal, backup.codex),
    ] {
        if content.trim().is_empty() {
            continue;
        }
        record_pre_apply(
            &state.db,
            scope.history_client_id(),
            "import_backup",
            crate::tools::instructions::snapshot_paths(scope),
            "restore from backup",
        );
        out.push(crate::tools::instructions::write(scope, &content)?);
    }
    Ok(out)
}

// ── Local Skills (~/.claude/skills) ────────────────────────────

/// 列出本地 skill（读 frontmatter + 启用状态）。
#[tauri::command]
#[specta::specta]
pub fn list_skills() -> Result<Vec<crate::tools::skills::Skill>, AppError> {
    Ok(crate::tools::skills::list_skills())
}

/// 启用/禁用一个 skill（重命名 manifest）。source 为 claude / codex。
#[tauri::command]
#[specta::specta]
pub fn set_skill_enabled(
    source: String,
    id: String,
    enabled: bool,
) -> Result<crate::tools::skills::Skill, AppError> {
    crate::tools::skills::set_skill_enabled(&source, &id, enabled)
}

/// 删除一个 skill 目录（强确认在前端）。
#[tauri::command]
#[specta::specta]
pub fn delete_skill(source: String, id: String) -> Result<bool, AppError> {
    crate::tools::skills::delete_skill(&source, &id)
}

/// 从本地 ZIP 字节安装一个 skill 到指定来源客户端（前端读文件成字节传入）。
#[tauri::command]
#[specta::specta]
pub fn import_skill_from_zip(
    source: String,
    bytes: Vec<u8>,
) -> Result<crate::tools::skills::Skill, AppError> {
    crate::tools::skills::import_skill_from_zip(&source, &bytes)
}

/// 导出所有 skill 为可备份 JSON（6.5）。
#[tauri::command]
#[specta::specta]
pub fn export_skills() -> Result<crate::tools::skills::SkillsExport, AppError> {
    Ok(crate::tools::skills::export_skills())
}

/// 从备份 JSON 恢复 skill（已存在的目录跳过，不覆盖）。
#[tauri::command]
#[specta::specta]
pub fn import_skills(payload: String) -> Result<Vec<crate::tools::skills::Skill>, AppError> {
    crate::tools::skills::import_skills(&payload)
}
