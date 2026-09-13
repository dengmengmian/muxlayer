import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  FileText,
  Save,
  RefreshCcw,
  Loader2,
  Check,
  AlertCircle,
  Sparkles,
  ChevronDown,
  Eye,
  Pencil,
  Archive,
  Upload,
  Copy,
  XCircle,
} from "lucide-react";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";
import { toast } from "@/components/common/Toast";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { ClientHistoryButton } from "@/components/tools/ClientHistoryButton";
import { MarkdownContent } from "@/components/common/MarkdownContent";

/// 用户全局指令文件（~/.claude/CLAUDE.md、~/.codex/AGENTS.md）管理页。
/// - 编辑器全宽优先；模板做成工具栏里的下拉菜单，避免窄屏下把内容挤到屏外。
/// - 写盘前由后端打 snapshot 并复用 client_apply_history 表，回滚走通用 UI。
export function Instructions() {
  const { t } = useI18n();
  const [scope, setScope] = useState<api.InstructionsScope>("claude_global");
  const [status, setStatus] = useState<api.InstructionsStatus | null>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [templates, setTemplates] = useState<api.InstructionsTemplate[]>([]);
  const [templateMenuOpen, setTemplateMenuOpen] = useState(false);
  const [viewMode, setViewMode] = useState<"edit" | "preview">("preview");
  const templateMenuRef = useRef<HTMLDivElement | null>(null);
  const [pending, setPending] = useState<{
    tpl: api.InstructionsTemplate;
    mode: api.InstructionsApplyMode;
  } | null>(null);
  const [backupMode, setBackupMode] = useState<"export" | "import" | null>(
    null
  );
  const [backupExport, setBackupExport] = useState("");
  const [backupImport, setBackupImport] = useState("");
  const [backupPending, setBackupPending] = useState(false);

  const loadStatus = useCallback(async (s: api.InstructionsScope) => {
    setLoading(true);
    try {
      const next = await api.readGlobalInstructions(s);
      setStatus(next);
      setDraft(next.content);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadStatus(scope);
  }, [scope, loadStatus]);

  useEffect(() => {
    api
      .listInstructionsTemplates()
      .then(setTemplates)
      .catch((err) => toast("error", (err as api.AppError).message));
  }, []);

  // 点外面关下拉。
  useEffect(() => {
    if (!templateMenuOpen) return;
    const onDocClick = (e: MouseEvent) => {
      if (!templateMenuRef.current?.contains(e.target as Node)) {
        setTemplateMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, [templateMenuOpen]);

  const scopeTag = scope === "claude_global" ? "claude" : "codex";
  const visibleTemplates = useMemo(
    () =>
      templates.filter(
        (tpl) => tpl.scopes.includes("all") || tpl.scopes.includes(scopeTag)
      ),
    [templates, scopeTag]
  );

  // 模板按 category 分组展示，组顺序固定。
  const groupedTemplates = useMemo(() => {
    const order = ["general", "coding", "review", "debug", "security", "docs"];
    const groups = new Map<string, api.InstructionsTemplate[]>();
    for (const tpl of visibleTemplates) {
      const key = tpl.category || "general";
      (groups.get(key) ?? groups.set(key, []).get(key)!).push(tpl);
    }
    return order
      .filter((cat) => groups.has(cat))
      .map((cat) => ({ category: cat, items: groups.get(cat)! }));
  }, [visibleTemplates]);

  const handleExportBackup = async () => {
    try {
      const data = await api.exportInstructions();
      setBackupExport(JSON.stringify(data, null, 2));
      setBackupMode("export");
      toast("success", t("instructions.backup.exported"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleImportBackup = async () => {
    if (!backupImport.trim()) return;
    if (!window.confirm(t("instructions.backup.import_confirm"))) return;
    setBackupPending(true);
    try {
      await api.importInstructions(backupImport);
      toast("success", t("instructions.backup.imported"));
      setBackupMode(null);
      setBackupImport("");
      loadStatus(scope);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setBackupPending(false);
    }
  };

  const dirty = status !== null && draft !== status.content;

  const handleSave = async () => {
    if (!status) return;
    setSaving(true);
    try {
      const next = await api.writeGlobalInstructions(scope, draft);
      setStatus(next);
      setDraft(next.content);
      toast("success", t("instructions.saved"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setSaving(false);
    }
  };

  const handleApplyTemplate = async (
    tpl: api.InstructionsTemplate,
    mode: api.InstructionsApplyMode
  ) => {
    setTemplateMenuOpen(false);
    // append 不会丢数据，直接应用；overwrite 弹确认。
    if (mode === "append") {
      try {
        const next = await api.applyInstructionsTemplate(
          scope,
          tpl.id,
          "append"
        );
        setStatus(next);
        setDraft(next.content);
        toast("success", t("instructions.applied"));
      } catch (err) {
        toast("error", (err as api.AppError).message);
      }
      return;
    }
    setPending({ tpl, mode });
  };

  const confirmOverwrite = async () => {
    if (!pending) return;
    try {
      const next = await api.applyInstructionsTemplate(
        scope,
        pending.tpl.id,
        "overwrite"
      );
      setStatus(next);
      setDraft(next.content);
      toast("success", t("instructions.applied"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setPending(null);
    }
  };

  const historyClientId =
    scope === "claude_global" ? "claude_instructions" : "codex_instructions";
  const historyClientName =
    scope === "claude_global" ? "CLAUDE.md" : "AGENTS.md";

  return (
    <div className="desktop-page mx-auto h-full min-h-0 w-full max-w-[1120px]">
      {/* Header */}
      <div className="desktop-page-header">
        <div>
          <h2 className="flex items-center gap-2 text-lg font-semibold text-text-primary">
            <FileText className="h-4 w-4" />
            {t("instructions.title")}
          </h2>
          <p className="mt-1 max-w-2xl text-xs text-text-muted">
            {t("instructions.subtitle")}
          </p>
        </div>
      </div>

      {/* Scope tabs */}
      <section className="surface-panel p-4">
        <div className="mb-3">
          <h3 className="text-sm font-semibold text-text-primary">
            {t("instructions.target_matrix")}
          </h3>
          <p className="mt-0.5 text-xs text-text-muted">
            {t("instructions.target_matrix_hint")}
          </p>
        </div>
        <div className="grid gap-2 sm:grid-cols-2">
          {(
            [
              { id: "claude_global", labelKey: "instructions.scope.claude" },
              { id: "codex_global", labelKey: "instructions.scope.codex" },
            ] as const
          ).map((it) => (
            <button
              key={it.id}
              onClick={() => setScope(it.id)}
              className={`rounded-lg border px-3 py-2 text-left text-xs transition-colors ${
                scope === it.id
                  ? "border-accent/40 bg-accent-soft text-accent"
                  : "border-border bg-card-secondary/45 text-text-secondary hover:border-accent/30 hover:text-text-primary"
              }`}
            >
              <span className="font-medium">{t(it.labelKey)}</span>
            </button>
          ))}
        </div>
      </section>

      {/* 编辑器从 AppShell 的 flex 高度链取剩余空间，窄高窗口仍由页面滚动兜底。 */}
      <section className="surface-panel flex min-h-64 flex-1 flex-col">
        {/* Toolbar */}
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border px-4 py-2.5">
          <div className="min-w-0 flex-1">
            <h3 className="mb-1 text-sm font-semibold text-text-primary">
              {t("instructions.editor")}
            </h3>
            <div className="truncate font-mono text-xs text-text-secondary">
              {status?.path ?? "—"}
            </div>
            <div className="mt-0.5 flex items-center gap-2 text-xs text-text-muted">
              {status?.exists ? (
                <>
                  <Check className="h-3 w-3 text-success" />
                  {t("instructions.file_status.exists").replace(
                    "{size}",
                    String(status.size_bytes)
                  )}
                </>
              ) : (
                <>
                  <AlertCircle className="h-3 w-3 text-warning" />
                  {t("instructions.file_status.missing")}
                </>
              )}
              {dirty && (
                <span className="text-warning">
                  · {t("instructions.unsaved")}
                </span>
              )}
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            <div className="flex items-center rounded-md bg-card-secondary p-0.5">
              <button
                type="button"
                onClick={() => setViewMode("edit")}
                className={`flex items-center gap-1 rounded px-2.5 py-1 text-xs font-medium transition-colors ${viewMode === "edit" ? "bg-card text-text-primary" : "text-text-muted hover:text-text-primary"}`}
              >
                <Pencil className="h-3 w-3" />
                {t("common.edit")}
              </button>
              <button
                type="button"
                onClick={() => setViewMode("preview")}
                className={`flex items-center gap-1 rounded px-2.5 py-1 text-xs font-medium transition-colors ${viewMode === "preview" ? "bg-card text-text-primary" : "text-text-muted hover:text-text-primary"}`}
              >
                <Eye className="h-3 w-3" />
                {t("common.preview")}
              </button>
            </div>

            {/* Template dropdown */}
            <div className="relative" ref={templateMenuRef}>
              <button
                type="button"
                onClick={() => setTemplateMenuOpen((v) => !v)}
                className="btn-secondary"
              >
                <Sparkles className="h-3 w-3" />
                {t("instructions.templates.title")}
                <ChevronDown className="h-3 w-3" />
              </button>
              {templateMenuOpen && (
                // 用 inline width + maxWidth 强制宽度。w-80 在 Tauri webview
                // 嵌套 flex 里被推断出 ~90px 的奇怪宽度，inline 风格最稳。
                // maxWidth 保证在窄窗口不撑出视口右边。
                <div
                  className="absolute right-0 top-full z-20 mt-1 rounded-lg border border-border bg-card p-2"
                  style={{
                    width: "380px",
                    maxWidth: "calc(100vw - 32px)",
                    boxShadow: "var(--shadow-lg)",
                  }}
                >
                  <p className="px-2 pb-2 text-xs leading-snug text-text-muted">
                    {t("instructions.templates.hint")}
                  </p>
                  {visibleTemplates.length === 0 ? (
                    <div className="rounded-md border border-dashed border-border p-3 text-center text-xs text-text-muted">
                      {t("instructions.templates.empty")}
                    </div>
                  ) : (
                    <div className="space-y-2">
                      {groupedTemplates.map((group) => (
                        <div key={group.category}>
                          {/* 分组标题：coding / review / debug / security / docs / general */}
                          <div className="px-2 pb-1 text-xs font-semibold uppercase tracking-wide text-text-muted">
                            {t(`instructions.tpl_category.${group.category}`)}
                          </div>
                          <ul className="space-y-1">
                            {group.items.map((tpl) => (
                              // 每条模板做横排：左边名称+描述吃满，右边两个动作按钮。
                              <li
                                key={tpl.id}
                                className="flex items-start gap-2 rounded-md p-2 hover:bg-hover"
                              >
                                <div className="min-w-0 flex-1">
                                  <div className="text-xs font-medium text-text-primary">
                                    {tpl.title}
                                  </div>
                                  <p className="mt-0.5 text-xs leading-snug text-text-muted">
                                    {tpl.description}
                                  </p>
                                </div>
                                <div className="flex shrink-0 flex-col gap-1">
                                  <button
                                    onClick={() =>
                                      handleApplyTemplate(tpl, "overwrite")
                                    }
                                    className="rounded border border-border bg-card-secondary px-2 py-0.5 text-xs font-medium text-text-primary hover:bg-hover"
                                  >
                                    {t(
                                      "instructions.templates.apply_overwrite"
                                    )}
                                  </button>
                                  <button
                                    onClick={() =>
                                      handleApplyTemplate(tpl, "append")
                                    }
                                    className="rounded border border-border bg-card-secondary px-2 py-0.5 text-xs font-medium text-text-primary hover:bg-hover"
                                  >
                                    {t("instructions.templates.apply_append")}
                                  </button>
                                </div>
                              </li>
                            ))}
                          </ul>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </div>

            <button
              onClick={() => loadStatus(scope)}
              disabled={loading}
              className="btn-secondary"
            >
              {loading ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <RefreshCcw className="h-3 w-3" />
              )}
              {t("instructions.reload")}
            </button>
            <button
              onClick={() => setBackupMode((m) => (m ? null : "import"))}
              className="btn-secondary"
              title={t("instructions.backup")}
            >
              <Archive className="h-3 w-3" />
              {t("instructions.backup")}
            </button>
            <ClientHistoryButton
              clientId={historyClientId}
              clientName={historyClientName}
              onRollbackDone={() => loadStatus(scope)}
            />
            <button
              onClick={handleSave}
              disabled={saving || !dirty}
              className="btn-primary"
            >
              {saving ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Save className="h-3 w-3" />
              )}
              {t("instructions.save")}
            </button>
          </div>
        </div>

        {viewMode === "edit" ? (
          /* Textarea —— flex-1 + min-h-0 让它吃满 section 剩余高度。
              resize-none 禁掉手动拖拽：高度已经跟着视口走，无谓的 resize 反而
              会破坏布局。 */
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={t("instructions.editor.placeholder")}
            spellCheck={false}
            className="w-full min-h-0 flex-1 resize-none bg-transparent p-4 font-mono text-xs leading-relaxed text-text-primary outline-none placeholder:text-text-muted"
          />
        ) : (
          <div className="surface-scroll min-h-0 flex-1 p-4 text-sm text-text-primary">
            {draft.trim() ? (
              <MarkdownContent content={draft} />
            ) : (
              <p className="text-xs text-text-muted">
                {t("instructions.editor.placeholder")}
              </p>
            )}
          </div>
        )}
      </section>

      {/* 备份面板（6.5）：导出两份指令到 JSON，或从备份恢复。 */}
      {backupMode && (
        <section className="rounded-lg border border-border bg-card p-4">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-1.5">
              <button
                onClick={() => setBackupMode("import")}
                className={`rounded-md px-2.5 py-1 text-xs ${backupMode === "import" ? "bg-accent-soft text-accent" : "text-text-secondary hover:bg-hover"}`}
              >
                {t("instructions.backup.import")}
              </button>
              <button
                onClick={handleExportBackup}
                className={`rounded-md px-2.5 py-1 text-xs ${backupMode === "export" ? "bg-accent-soft text-accent" : "text-text-secondary hover:bg-hover"}`}
              >
                {t("instructions.backup.export")}
              </button>
            </div>
            <div className="flex items-center gap-2">
              {backupMode === "export" ? (
                <button
                  onClick={() => {
                    navigator.clipboard.writeText(backupExport);
                    toast("success", t("common.copied"));
                  }}
                  className="btn-secondary"
                >
                  <Copy className="h-3 w-3" />
                  {t("common.copy")}
                </button>
              ) : (
                <button
                  onClick={handleImportBackup}
                  disabled={backupPending || !backupImport.trim()}
                  className="btn-primary"
                >
                  {backupPending ? (
                    <Loader2 className="h-3 w-3 animate-spin" />
                  ) : (
                    <Upload className="h-3 w-3" />
                  )}
                  {t("instructions.backup.import")}
                </button>
              )}
              <button
                onClick={() => setBackupMode(null)}
                className="rounded p-1 text-text-muted hover:bg-hover hover:text-text-primary"
              >
                <XCircle className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
          <textarea
            value={backupMode === "export" ? backupExport : backupImport}
            onChange={(e) =>
              backupMode === "export"
                ? setBackupExport(e.target.value)
                : setBackupImport(e.target.value)
            }
            rows={6}
            spellCheck={false}
            className="w-full resize-none rounded-md border border-border bg-card-secondary px-2.5 py-2 font-mono text-xs text-text-primary outline-none focus:border-accent"
            placeholder={
              backupMode === "export" ? "" : t("instructions.backup.paste")
            }
          />
        </section>
      )}

      <ConfirmDialog
        open={!!pending}
        variant="danger"
        title={t("instructions.confirm_overwrite_title")}
        message={
          pending
            ? t("instructions.confirm_overwrite_msg")
                .replace("{file}", historyClientName)
                .replace("{template}", pending.tpl.title)
            : ""
        }
        confirmLabel={t("instructions.confirm_btn")}
        onConfirm={confirmOverwrite}
        onCancel={() => setPending(null)}
      />
    </div>
  );
}
