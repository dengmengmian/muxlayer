import { useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpen,
  CheckCircle2,
  Copy,
  Download,
  Loader2,
  Trash2,
  Upload,
  XCircle,
} from "lucide-react";
import { EmptyState } from "@/components/common/EmptyState";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";
import type { Skill } from "@/lib/api";

type TransferMode = "export" | "import" | null;

const SOURCES = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "opencode", label: "OpenCode" },
  { id: "gemini", label: "Gemini CLI" },
] as const;

type Filter = "all" | (typeof SOURCES)[number]["id"];

const skillKey = (s: Skill) => `${s.source}:${s.id}`;
const sourceLabel = (source: string) =>
  SOURCES.find((s) => s.id === source)?.label ?? source;

/// 本地 Skills 管理页。来源 Claude Code / Codex / OpenCode / Gemini CLI 的 skills
/// 目录：启用/禁用靠重命名 manifest，导入支持本地 .zip（不碰网络），备份走 JSON。
export function Skills() {
  const { t } = useI18n();
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [importSource, setImportSource] = useState("claude");
  const [transferMode, setTransferMode] = useState<TransferMode>(null);
  const [exportText, setExportText] = useState("");
  const [importText, setImportText] = useState("");
  const [transferring, setTransferring] = useState(false);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const load = () => {
    setLoading(true);
    api
      .listSkills()
      .then(setSkills)
      .catch((err) => toast("error", (err as api.AppError).message))
      .finally(() => setLoading(false));
  };

  useEffect(() => load(), []);

  const counts = useMemo(() => {
    const next: Record<string, number> = { all: skills.length };
    for (const source of SOURCES) {
      next[source.id] = skills.filter((s) => s.source === source.id).length;
    }
    return next;
  }, [skills]);

  const visibleSkills = useMemo(
    () =>
      filter === "all" ? skills : skills.filter((s) => s.source === filter),
    [filter, skills]
  );

  const handleToggle = async (skill: Skill) => {
    setBusyKey(skillKey(skill));
    try {
      const next = await api.setSkillEnabled(
        skill.source,
        skill.id,
        !skill.enabled
      );
      setSkills((items) =>
        items.map((s) => (skillKey(s) === skillKey(skill) ? next : s))
      );
      toast("success", t("skills.toggled"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setBusyKey(null);
    }
  };

  const handleDelete = async (skill: Skill) => {
    if (
      !window.confirm(t("skills.delete_confirm").replace("{name}", skill.name))
    )
      return;
    setBusyKey(skillKey(skill));
    try {
      await api.deleteSkill(skill.source, skill.id);
      setSkills((items) =>
        items.filter((s) => skillKey(s) !== skillKey(skill))
      );
      toast("success", t("skills.deleted"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setBusyKey(null);
    }
  };

  const handleImportZip = async (file: File) => {
    setTransferring(true);
    try {
      const buf = await file.arrayBuffer();
      const bytes = Array.from(new Uint8Array(buf));
      const skill = await api.importSkillFromZip(importSource, bytes);
      toast("success", t("skills.imported_zip"));
      setSkills((items) =>
        [...items.filter((s) => skillKey(s) !== skillKey(skill)), skill].sort(
          byName
        )
      );
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setTransferring(false);
      if (fileRef.current) fileRef.current.value = "";
    }
  };

  const handleExport = async () => {
    setTransferring(true);
    try {
      const data = await api.exportSkills();
      setExportText(JSON.stringify(data, null, 2));
      setTransferMode("export");
      if ((data.skipped_files?.length ?? 0) > 0) {
        toast(
          "warning",
          t("skills.skipped_note").replace(
            "{count}",
            String(data.skipped_files?.length ?? 0)
          )
        );
      } else {
        toast("success", t("skills.exported"));
      }
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setTransferring(false);
    }
  };

  const handleImportBackup = async () => {
    if (!importText.trim()) {
      toast("error", t("skills.paste_backup"));
      return;
    }
    setTransferring(true);
    try {
      const imported = await api.importSkills(importText);
      if (imported.length === 0) {
        toast("warning", t("skills.import_backup_none"));
      } else {
        toast(
          "success",
          t("skills.imported_backup").replace(
            "{count}",
            String(imported.length)
          )
        );
      }
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setTransferring(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center gap-2 text-xs text-text-muted">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        {t("common.loading")}
      </div>
    );
  }

  return (
    <div className="desktop-page">
      <header className="desktop-page-header">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h2 className="flex items-center gap-2 text-lg font-semibold text-text-primary">
              <BookOpen className="h-4 w-4" />
              {t("skills.title")}
            </h2>
            <p className="mt-1 max-w-2xl text-xs text-text-muted">
              {t("skills.subtitle")}
            </p>
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            {/* 导入 zip 的目标客户端 */}
            <select
              value={importSource}
              onChange={(e) => setImportSource(e.target.value)}
              className="rounded-md border border-border bg-card-secondary px-2 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
              title={t("skills.import_zip")}
            >
              {SOURCES.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.label}
                </option>
              ))}
            </select>
            <input
              ref={fileRef}
              type="file"
              accept=".zip"
              className="hidden"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file) handleImportZip(file);
              }}
            />
            <button
              onClick={() => fileRef.current?.click()}
              disabled={transferring}
              className="flex items-center gap-1.5 rounded-md border border-border bg-card-secondary px-2.5 py-1.5 text-xs text-text-secondary hover:border-accent/40 hover:text-text-primary disabled:opacity-60"
            >
              {transferring ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Upload className="h-3.5 w-3.5" />
              )}
              {t("skills.import_zip")}
            </button>
            <button
              onClick={handleExport}
              disabled={transferring}
              className="flex items-center gap-1.5 rounded-md border border-border bg-card-secondary px-2.5 py-1.5 text-xs text-text-secondary hover:border-accent/40 hover:text-text-primary disabled:opacity-60"
            >
              <Download className="h-3.5 w-3.5" />
              {t("skills.export")}
            </button>
            <button
              onClick={() =>
                setTransferMode((mode) => (mode === "import" ? null : "import"))
              }
              className="flex items-center gap-1.5 rounded-md border border-border bg-card-secondary px-2.5 py-1.5 text-xs text-text-secondary hover:border-accent/40 hover:text-text-primary"
            >
              <Upload className="h-3.5 w-3.5" />
              {t("skills.import_backup")}
            </button>
          </div>
        </div>
      </header>

      {/* 来源筛选 */}
      {skills.length > 0 && (
        <section className="surface-panel p-4">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
            <div>
              <h3 className="text-sm font-semibold text-text-primary">
                {t("skills.source_matrix")}
              </h3>
              <p className="mt-0.5 text-xs text-text-muted">
                {t("skills.source_matrix_hint")}
              </p>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <FilterButton
              active={filter === "all"}
              label={t("nav.skills")}
              count={counts.all}
              onClick={() => setFilter("all")}
            />
            {SOURCES.map((source) => (
              <FilterButton
                key={source.id}
                active={filter === source.id}
                label={source.label}
                count={counts[source.id] ?? 0}
                onClick={() => setFilter(source.id)}
              />
            ))}
          </div>
        </section>
      )}

      {transferMode && (
        <section className="rounded-lg border border-border bg-card p-4">
          <div className="mb-3 flex items-center justify-between gap-2">
            <div className="text-xs font-medium text-text-primary">
              {transferMode === "export"
                ? t("skills.export")
                : t("skills.import_backup")}
            </div>
            <div className="flex items-center gap-2">
              {transferMode === "export" ? (
                <button
                  onClick={() => {
                    navigator.clipboard.writeText(exportText);
                    toast("success", t("common.copied"));
                  }}
                  className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-card-secondary"
                >
                  <Copy className="h-3.5 w-3.5" />
                  {t("common.copy")}
                </button>
              ) : (
                <button
                  onClick={handleImportBackup}
                  disabled={transferring}
                  className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-on-accent hover:bg-accent-hover disabled:opacity-60"
                >
                  {transferring ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Upload className="h-3.5 w-3.5" />
                  )}
                  {t("skills.import_backup")}
                </button>
              )}
              <button
                onClick={() => setTransferMode(null)}
                className="rounded p-1 text-text-muted hover:bg-card-secondary hover:text-text-primary"
              >
                <XCircle className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
          <textarea
            value={transferMode === "export" ? exportText : importText}
            onChange={(e) =>
              transferMode === "export"
                ? setExportText(e.target.value)
                : setImportText(e.target.value)
            }
            rows={6}
            className="w-full resize-none rounded-md border border-border bg-card-secondary px-2.5 py-2 font-mono text-xs text-text-primary outline-none focus:border-accent"
            placeholder={
              transferMode === "export" ? "" : t("skills.paste_backup")
            }
          />
        </section>
      )}

      {skills.length === 0 ? (
        <EmptyState
          icon={BookOpen}
          title={t("skills.title")}
          description={t("skills.empty")}
        />
      ) : (
        <div className="surface-panel overflow-hidden divide-y divide-border">
          {visibleSkills.map((skill) => (
            <div
              key={skillKey(skill)}
              className="flex items-center gap-3 px-4 py-3"
            >
              <span
                className={`inline-flex w-fit shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-xs ${
                  skill.enabled
                    ? "bg-success/10 text-success"
                    : "bg-card-secondary text-text-muted"
                }`}
              >
                {skill.enabled ? (
                  <CheckCircle2 className="h-3 w-3" />
                ) : (
                  <XCircle className="h-3 w-3" />
                )}
                {skill.enabled ? t("skills.enabled") : t("skills.disabled")}
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-xs font-semibold text-text-primary">
                    {skill.name}
                  </span>
                  <span className="shrink-0 rounded bg-card-secondary px-1.5 py-0.5 text-xs text-text-secondary">
                    {sourceLabel(skill.source)}
                  </span>
                </div>
                {skill.description && (
                  <p className="mt-0.5 truncate text-xs text-text-muted">
                    {skill.description}
                  </p>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <button
                  onClick={() => handleToggle(skill)}
                  disabled={busyKey === skillKey(skill)}
                  className="rounded-md border border-border px-2.5 py-1 text-xs text-text-secondary hover:bg-card-secondary disabled:opacity-60"
                >
                  {busyKey === skillKey(skill) ? (
                    <Loader2 className="h-3 w-3 animate-spin" />
                  ) : skill.enabled ? (
                    t("skills.disable")
                  ) : (
                    t("skills.enable")
                  )}
                </button>
                <button
                  onClick={() => handleDelete(skill)}
                  disabled={busyKey === skillKey(skill)}
                  title={t("skills.delete")}
                  className="flex items-center gap-1 rounded-md border border-transparent px-2 py-1 text-xs text-text-muted hover:border-error/30 hover:bg-error-soft hover:text-error disabled:opacity-60"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  {t("skills.delete")}
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function FilterButton({
  active,
  label,
  count,
  onClick,
}: {
  active: boolean;
  label: string;
  count: number;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex items-center justify-between rounded-lg border px-3 py-2 text-xs transition-colors ${
        active
          ? "border-accent/40 bg-accent-soft text-accent"
          : "border-border bg-card-secondary/45 text-text-secondary hover:border-accent/30 hover:text-text-primary"
      }`}
    >
      <span className="font-medium">{label}</span>
      <span
        className={
          active ? "font-mono text-accent" : "font-mono text-text-muted"
        }
      >
        {count}
      </span>
    </button>
  );
}

function byName(a: Skill, b: Skill) {
  return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
}
