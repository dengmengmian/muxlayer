import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  Copy,
  Download,
  Edit2,
  KeyRound,
  Loader2,
  MoreHorizontal,
  Plus,
  Plug,
  Save,
  Search,
  Trash2,
  Upload,
  XCircle,
} from "lucide-react";
import { EmptyState } from "@/components/common/EmptyState";
import { toast } from "@/components/common/Toast";
import { DetailDrawer } from "@/components/layout/DetailDrawer";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";
import type { McpServer, McpValidationStatus } from "@/lib/api";
import { ClientLogo, type ClientLogoId } from "@/components/tools/ClientLogo";

type Draft = {
  client: string;
  originalName?: string;
  name: string;
  command: string;
  argsText: string;
  envText: string;
};

type TransferMode = "export" | "import" | null;
type Filter =
  | "all"
  | "issues"
  | "codex"
  | "claude_code"
  | "gemini"
  | "opencode";

const clients = [
  { id: "codex", label: "Codex", logo: "codex" as ClientLogoId },
  {
    id: "claude_code",
    label: "Claude Code",
    logo: "claude_code" as ClientLogoId,
  },
  { id: "gemini", label: "Gemini CLI", logo: "gemini_cli" as ClientLogoId },
  { id: "opencode", label: "OpenCode", logo: "opencode" as ClientLogoId },
];

/// MCP 面板。以客户端文件为真相源，读写 Codex / Claude Code 现有 MCP server。
export function Mcp() {
  const { t } = useI18n();
  const [servers, setServers] = useState<McpServer[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [exportText, setExportText] = useState("");
  const [importText, setImportText] = useState("");
  const [importClient, setImportClient] = useState("codex");
  const [transferMode, setTransferMode] = useState<TransferMode>(null);
  const [moreOpen, setMoreOpen] = useState(false);
  const [transferring, setTransferring] = useState(false);

  const load = () => {
    let cancelled = false;
    setLoading(true);
    api
      .listMcpServers()
      .then((data) => {
        if (!cancelled) setServers(data);
      })
      .catch((err) => {
        if (!cancelled) toast("error", (err as api.AppError).message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  };

  useEffect(() => load(), []);

  const selected = servers.find((server) => server.id === selectedId) ?? null;

  const counts = useMemo(() => {
    const issues = servers.filter(
      (server) => server.validation.status !== "valid"
    ).length;
    return {
      all: servers.length,
      issues,
      codex: servers.filter((server) =>
        server.enabled_clients.includes("codex")
      ).length,
      claude_code: servers.filter((server) =>
        server.enabled_clients.includes("claude_code")
      ).length,
      gemini: servers.filter((server) =>
        server.enabled_clients.includes("gemini")
      ).length,
      opencode: servers.filter((server) =>
        server.enabled_clients.includes("opencode")
      ).length,
    };
  }, [servers]);

  const visibleServers = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    return servers.filter((server) => {
      if (filter === "issues" && server.validation.status === "valid")
        return false;
      if (filter === "codex" && !server.enabled_clients.includes("codex"))
        return false;
      if (
        filter === "claude_code" &&
        !server.enabled_clients.includes("claude_code")
      )
        return false;
      if (filter === "gemini" && !server.enabled_clients.includes("gemini"))
        return false;
      if (filter === "opencode" && !server.enabled_clients.includes("opencode"))
        return false;
      if (!keyword) return true;
      const haystack = [
        server.name,
        server.command,
        server.args.join(" "),
        server.env.map((env) => env.key).join(" "),
        server.sources.map((source) => source.config_path).join(" "),
      ]
        .join(" ")
        .toLowerCase();
      return haystack.includes(keyword);
    });
  }, [filter, query, servers]);

  const openCreate = () => {
    setSelectedId(null);
    setDraft({
      client: "codex",
      name: "",
      command: "",
      argsText: "",
      envText: "",
    });
  };

  const openEdit = (server: McpServer) => {
    setSelectedId(null);
    setDraft({
      client: server.enabled_clients[0] ?? "codex",
      originalName: server.name,
      name: server.name,
      command: server.command,
      argsText: server.args.join("\n"),
      envText: server.env.map((env) => `${env.key}=`).join("\n"),
    });
  };

  const parseEnv = (text: string) =>
    text
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const eq = line.indexOf("=");
        return eq === -1
          ? { key: line, value: "" }
          : { key: line.slice(0, eq).trim(), value: line.slice(eq + 1) };
      });

  const refreshServers = async () => {
    const next = await api.listMcpServers();
    setServers(next);
    if (selectedId && !next.some((server) => server.id === selectedId)) {
      setSelectedId(null);
    }
  };

  const handleSave = async () => {
    if (!draft) return;
    if (!draft.name.trim() || !draft.command.trim()) {
      toast("error", t("mcp.name_command_required"));
      return;
    }
    setSaving(true);
    try {
      if (draft.originalName && draft.originalName !== draft.name) {
        await api.deleteMcpServer(draft.client, draft.originalName);
      }
      await api.upsertMcpServer({
        client: draft.client,
        name: draft.name.trim(),
        command: draft.command.trim(),
        args: draft.argsText
          .split("\n")
          .map((item) => item.trim())
          .filter(Boolean),
        env: parseEnv(draft.envText),
      });
      toast("success", t("mcp.saved"));
      setDraft(null);
      await refreshServers();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (server: McpServer) => {
    const client = server.enabled_clients[0] ?? "codex";
    if (!window.confirm(t("mcp.delete_confirm").replace("{name}", server.name)))
      return;
    try {
      await api.deleteMcpServer(client, server.name);
      toast("success", t("mcp.deleted"));
      setServers((items) => items.filter((item) => item.id !== server.id));
      if (selectedId === server.id) setSelectedId(null);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleSync = async (server: McpServer) => {
    const fromClient = server.enabled_clients[0] ?? "codex";
    const toClient = fromClient === "codex" ? "claude_code" : "codex";
    const toLabel = clientLabel(toClient);
    if (
      !window.confirm(
        t("mcp.sync_confirm")
          .replace("{name}", server.name)
          .replace("{target}", toLabel)
      )
    )
      return;
    try {
      await api.syncMcpServer({
        from_client: fromClient,
        name: server.name,
        to_clients: [toClient],
      });
      toast("success", t("mcp.synced_to").replace("{target}", toLabel));
      await refreshServers();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleExport = async () => {
    if (includeSecrets && !window.confirm(t("mcp.export_secrets_confirm")))
      return;
    setMoreOpen(false);
    setTransferring(true);
    try {
      const text = await api.exportMcpServers(includeSecrets);
      setExportText(text);
      setTransferMode("export");
      toast(
        "success",
        includeSecrets ? t("mcp.exported_with_secrets") : t("mcp.exported")
      );
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setTransferring(false);
    }
  };

  const openImport = () => {
    setMoreOpen(false);
    setTransferMode((mode) => (mode === "import" ? null : "import"));
  };

  const handleImport = async () => {
    if (!importText.trim()) {
      toast("error", t("mcp.paste_json_required"));
      return;
    }
    const targetLabel = clientLabel(importClient);
    if (
      !window.confirm(t("mcp.import_confirm").replace("{target}", targetLabel))
    )
      return;
    setTransferring(true);
    try {
      await api.importMcpServers(importText, [importClient]);
      toast("success", t("mcp.imported_to").replace("{target}", targetLabel));
      await refreshServers();
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
      <header className="desktop-page-header relative z-10">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h2 className="flex items-center gap-2 text-lg font-semibold text-text-primary">
              <Plug className="h-4 w-4" />
              {t("mcp.title")}
            </h2>
            <p className="mt-1 max-w-2xl text-xs text-text-muted">
              {t("mcp.subtitle_before")}
              <code className="font-mono">config.toml</code>
              {t("mcp.subtitle_mid")}
              <code className="font-mono">.claude.json</code>
              {t("mcp.subtitle_after")}
            </p>
          </div>
          <div className="relative flex shrink-0 items-center gap-2">
            <button
              onClick={openCreate}
              className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-on-accent hover:bg-accent-hover"
            >
              <Plus className="h-3.5 w-3.5" />
              {t("mcp.add_server")}
            </button>
            <button
              onClick={() => setMoreOpen((open) => !open)}
              className="rounded-md border border-border bg-card-secondary p-1.5 text-text-secondary hover:border-accent/40 hover:text-text-primary"
              title={t("mcp.more")}
              aria-expanded={moreOpen}
            >
              <MoreHorizontal className="h-4 w-4" />
            </button>
            {moreOpen && (
              <>
                {/* 点击外部关闭 */}
                <button
                  type="button"
                  className="fixed inset-0 z-30 cursor-default bg-transparent"
                  aria-label="close menu"
                  onClick={() => setMoreOpen(false)}
                />
                <div className="absolute right-0 top-full z-40 mt-1 w-52 rounded-lg border border-border bg-card p-2 shadow-lg">
                  <label className="mb-1 flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-text-muted">
                    <input
                      type="checkbox"
                      checked={includeSecrets}
                      onChange={(event) =>
                        setIncludeSecrets(event.target.checked)
                      }
                      className="h-3.5 w-3.5 rounded border-border"
                    />
                    {t("mcp.export_include_secrets")}
                  </label>
                  <button
                    onClick={handleExport}
                    disabled={transferring}
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs text-text-secondary hover:bg-card-secondary disabled:opacity-60"
                  >
                    {transferring ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Download className="h-3.5 w-3.5" />
                    )}
                    {t("mcp.export_json")}
                  </button>
                  <button
                    onClick={openImport}
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs text-text-secondary hover:bg-card-secondary"
                  >
                    <Upload className="h-3.5 w-3.5" />
                    {t("mcp.import_json")}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      </header>

      {servers.length > 0 && (
        <section className="surface-panel p-4">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
            <div>
              <h3 className="text-sm font-semibold text-text-primary">
                {t("mcp.server_matrix")}
              </h3>
              <p className="mt-0.5 text-xs text-text-muted">
                {t("mcp.server_matrix_hint")}
              </p>
            </div>
            <label className="flex min-w-[240px] items-center gap-2 rounded-md border border-border bg-card-secondary px-2.5 py-1.5 text-xs text-text-muted">
              <Search className="h-3.5 w-3.5" />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                className="w-full bg-transparent text-text-primary outline-none placeholder:text-text-muted"
                placeholder={t("mcp.search_placeholder")}
              />
            </label>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <FilterButton
              active={filter === "all"}
              label={t("mcp.filter_all")}
              count={counts.all}
              onClick={() => setFilter("all")}
            />
            <FilterButton
              active={filter === "issues"}
              label={t("mcp.filter_issues")}
              count={counts.issues}
              onClick={() => setFilter("issues")}
            />
            <FilterButton
              active={filter === "codex"}
              label="Codex"
              count={counts.codex}
              onClick={() => setFilter("codex")}
            />
            <FilterButton
              active={filter === "claude_code"}
              label="Claude Code"
              count={counts.claude_code}
              onClick={() => setFilter("claude_code")}
            />
            <FilterButton
              active={filter === "gemini"}
              label="Gemini CLI"
              count={counts.gemini}
              onClick={() => setFilter("gemini")}
            />
            <FilterButton
              active={filter === "opencode"}
              label="OpenCode"
              count={counts.opencode}
              onClick={() => setFilter("opencode")}
            />
          </div>
        </section>
      )}

      {transferMode && (
        <section className="rounded-lg border border-border bg-card p-4">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
            <div>
              <div className="text-xs font-medium text-text-primary">
                {transferMode === "export"
                  ? t("mcp.export_config")
                  : t("mcp.import_config")}
              </div>
              <div className="mt-0.5 text-xs text-text-muted">
                {transferMode === "export"
                  ? includeSecrets
                    ? t("mcp.export_hint_with_value")
                    : t("mcp.export_hint_hidden")
                  : t("mcp.import_hint")}
              </div>
            </div>
            <div className="flex items-center gap-2">
              {transferMode === "import" && (
                <>
                  <select
                    value={importClient}
                    onChange={(event) => setImportClient(event.target.value)}
                    className="rounded-md border border-border bg-card-secondary px-2 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
                  >
                    <option value="codex">Codex</option>
                    <option value="claude_code">Claude Code</option>
                    <option value="gemini">Gemini CLI</option>
                    <option value="opencode">OpenCode</option>
                  </select>
                  <button
                    onClick={handleImport}
                    disabled={transferring}
                    className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-on-accent hover:bg-accent-hover disabled:opacity-60"
                  >
                    {transferring ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Upload className="h-3.5 w-3.5" />
                    )}
                    {t("mcp.import")}
                  </button>
                </>
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
            onChange={(event) =>
              transferMode === "export"
                ? setExportText(event.target.value)
                : setImportText(event.target.value)
            }
            rows={6}
            className="w-full resize-none rounded-md border border-border bg-card-secondary px-2.5 py-2 font-mono text-xs text-text-primary outline-none focus:border-accent"
            placeholder={
              transferMode === "export"
                ? t("mcp.export_result_placeholder")
                : t("mcp.import_paste_placeholder")
            }
          />
        </section>
      )}

      {servers.length === 0 ? (
        <EmptyState
          icon={Plug}
          title={t("mcp.empty_title")}
          description={t("mcp.empty_desc")}
        />
      ) : (
        <ServerTable
          servers={visibleServers}
          selectedId={selectedId}
          onSelect={(server) => setSelectedId(server.id)}
          onEdit={openEdit}
          onSync={handleSync}
          onDelete={handleDelete}
        />
      )}

      <DetailDrawer
        open={Boolean(selected)}
        title={selected?.name ?? t("mcp.server")}
        onClose={() => setSelectedId(null)}
      >
        {selected && (
          <ServerDetail
            server={selected}
            onEdit={() => openEdit(selected)}
            onSync={() => handleSync(selected)}
            onDelete={() => handleDelete(selected)}
          />
        )}
      </DetailDrawer>

      <DetailDrawer
        open={Boolean(draft)}
        title={draft?.originalName ? t("mcp.edit_server") : t("mcp.add_server")}
        onClose={() => setDraft(null)}
      >
        {draft && (
          <DraftForm
            draft={draft}
            saving={saving}
            onChange={setDraft}
            onSave={handleSave}
          />
        )}
      </DetailDrawer>
    </div>
  );
}

function ServerTable({
  servers,
  selectedId,
  onSelect,
  onEdit,
  onSync,
  onDelete,
}: {
  servers: McpServer[];
  selectedId: string | null;
  onSelect: (server: McpServer) => void;
  onEdit: (server: McpServer) => void;
  onSync: (server: McpServer) => void;
  onDelete: (server: McpServer) => void;
}) {
  const { t } = useI18n();
  if (servers.length === 0) {
    return (
      <div className="rounded-lg border border-border bg-card p-8 text-center text-xs text-text-muted">
        {t("mcp.no_match")}
      </div>
    );
  }

  return (
    <div className="surface-scroll rounded-md border border-border bg-card">
      <div className="min-w-[840px]">
        <div className="grid grid-cols-[120px_minmax(160px,1.2fr)_120px_minmax(240px,2fr)_90px_104px] border-b border-border bg-card-secondary px-4 py-2 text-xs font-medium text-text-muted">
          <div>{t("mcp.col_status")}</div>
          <div>{t("mcp.col_name")}</div>
          <div>{t("mcp.col_client")}</div>
          <div>command</div>
          <div>env</div>
          <div className="text-right">{t("mcp.col_actions")}</div>
        </div>
        <div className="divide-y divide-border">
          {servers.map((server) => (
            <div
              key={server.id}
              onClick={() => onSelect(server)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ")
                  onSelect(server);
              }}
              role="button"
              tabIndex={0}
              className={`grid w-full grid-cols-[120px_minmax(160px,1.2fr)_120px_minmax(240px,2fr)_90px_104px] items-center gap-0 px-4 py-3 text-left text-xs hover:bg-card-secondary ${
                selectedId === server.id ? "bg-accent/5" : ""
              }`}
            >
              <StatusPill
                status={server.validation.status as McpValidationStatus}
              />
              <div className="min-w-0">
                <div className="truncate font-mono font-semibold text-text-primary">
                  {server.name}
                </div>
                {server.validation.issues.length > 0 && (
                  <div className="mt-0.5 truncate text-xs text-text-muted">
                    {server.validation.issues[0].message}
                  </div>
                )}
              </div>
              <ClientBadges server={server} />
              <div
                className="min-w-0 truncate font-mono text-xs text-text-muted"
                title={`${server.command} ${server.args.join(" ")}`}
              >
                {server.command || t("mcp.no_command")} {server.args.join(" ")}
              </div>
              <EnvSummary server={server} />
              <div className="flex justify-end gap-1">
                <IconButton
                  title={t("common.edit")}
                  onClick={(event) => {
                    event.stopPropagation();
                    onEdit(server);
                  }}
                >
                  <Edit2 className="h-3.5 w-3.5" />
                </IconButton>
                <IconButton
                  title={t("mcp.sync_to_other")}
                  onClick={(event) => {
                    event.stopPropagation();
                    onSync(server);
                  }}
                >
                  <Copy className="h-3.5 w-3.5" />
                </IconButton>
                <IconButton
                  danger
                  title={t("common.delete")}
                  onClick={(event) => {
                    event.stopPropagation();
                    onDelete(server);
                  }}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </IconButton>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function ServerDetail({
  server,
  onEdit,
  onSync,
  onDelete,
}: {
  server: McpServer;
  onEdit: () => void;
  onSync: () => void;
  onDelete: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="space-y-5">
      <div className="flex items-center justify-between gap-3">
        <StatusPill status={server.validation.status as McpValidationStatus} />
        <div className="flex items-center gap-1.5">
          <IconButton title={t("common.edit")} onClick={onEdit}>
            <Edit2 className="h-3.5 w-3.5" />
          </IconButton>
          <IconButton title={t("mcp.sync_to_other")} onClick={onSync}>
            <Copy className="h-3.5 w-3.5" />
          </IconButton>
          <IconButton danger title={t("common.delete")} onClick={onDelete}>
            <Trash2 className="h-3.5 w-3.5" />
          </IconButton>
        </div>
      </div>

      <DetailSection title={t("mcp.col_client")}>
        <ClientBadges server={server} />
      </DetailSection>

      <DetailSection title="command">
        <CodeBlock value={server.command || t("mcp.no_command")} />
      </DetailSection>

      <DetailSection title="args">
        {server.args.length > 0 ? (
          <CodeBlock value={server.args.join("\n")} />
        ) : (
          <MutedText>{t("mcp.no_args")}</MutedText>
        )}
      </DetailSection>

      <DetailSection title="env">
        {server.env.length > 0 ? (
          <div className="space-y-1.5">
            {server.env.map((env) => (
              <div
                key={env.key}
                className="flex items-center justify-between gap-2 rounded-md bg-card-secondary px-2.5 py-1.5"
              >
                <div className="min-w-0 truncate font-mono text-xs text-text-secondary">
                  {env.key}
                </div>
                <div className="flex shrink-0 items-center gap-1.5 text-xs text-text-muted">
                  {env.is_sensitive && <span>{t("mcp.sensitive")}</span>}
                  {!env.has_value && (
                    <span className="text-warning">missing</span>
                  )}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <MutedText>{t("mcp.no_env")}</MutedText>
        )}
      </DetailSection>

      <DetailSection title={t("mcp.sources")}>
        <div className="space-y-2">
          {server.sources.map((source) => (
            <div
              key={`${source.client}:${source.config_path}`}
              className="rounded-md bg-card-secondary px-2.5 py-2"
            >
              <div className="text-xs font-medium text-text-secondary">
                {clientLabel(source.client)}
              </div>
              <div className="mt-1 break-all font-mono text-xs text-text-muted">
                {source.config_path}
              </div>
            </div>
          ))}
        </div>
      </DetailSection>

      {server.validation.issues.length > 0 && (
        <DetailSection title={t("mcp.validation_issues")}>
          <div className="space-y-2">
            {server.validation.issues.map((issue) => (
              <div
                key={`${issue.code}:${issue.field ?? ""}`}
                className="rounded-md border border-warning/30 bg-warning/5 px-2.5 py-2 text-xs text-text-secondary"
              >
                {issue.message}
              </div>
            ))}
          </div>
        </DetailSection>
      )}
    </div>
  );
}

function DraftForm({
  draft,
  saving,
  onChange,
  onSave,
}: {
  draft: Draft;
  saving: boolean;
  onChange: (draft: Draft) => void;
  onSave: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="space-y-4">
      <label className="space-y-1 text-xs text-text-muted">
        client
        <select
          value={draft.client}
          disabled={Boolean(draft.originalName)}
          onChange={(event) =>
            onChange({ ...draft, client: event.target.value })
          }
          className="w-full rounded-md border border-border bg-card-secondary px-2.5 py-2 text-xs text-text-primary outline-none focus:border-accent disabled:opacity-60"
        >
          <option value="codex">Codex</option>
          <option value="claude_code">Claude Code</option>
          <option value="gemini">Gemini CLI</option>
          <option value="opencode">OpenCode</option>
        </select>
      </label>
      <label className="space-y-1 text-xs text-text-muted">
        name
        <input
          value={draft.name}
          onChange={(event) => onChange({ ...draft, name: event.target.value })}
          className="w-full rounded-md border border-border bg-card-secondary px-2.5 py-2 font-mono text-xs text-text-primary outline-none focus:border-accent"
        />
      </label>
      <label className="space-y-1 text-xs text-text-muted">
        command
        <input
          value={draft.command}
          onChange={(event) =>
            onChange({ ...draft, command: event.target.value })
          }
          className="w-full rounded-md border border-border bg-card-secondary px-2.5 py-2 font-mono text-xs text-text-primary outline-none focus:border-accent"
        />
      </label>
      <label className="space-y-1 text-xs text-text-muted">
        {t("mcp.field_args")}
        <textarea
          value={draft.argsText}
          onChange={(event) =>
            onChange({ ...draft, argsText: event.target.value })
          }
          rows={5}
          className="w-full resize-none rounded-md border border-border bg-card-secondary px-2.5 py-2 font-mono text-xs text-text-primary outline-none focus:border-accent"
        />
      </label>
      <label className="space-y-1 text-xs text-text-muted">
        {t("mcp.field_env")}
        <textarea
          value={draft.envText}
          onChange={(event) =>
            onChange({ ...draft, envText: event.target.value })
          }
          rows={5}
          className="w-full resize-none rounded-md border border-border bg-card-secondary px-2.5 py-2 font-mono text-xs text-text-primary outline-none focus:border-accent"
        />
      </label>
      <button
        onClick={onSave}
        disabled={saving}
        className="flex w-full items-center justify-center gap-1.5 rounded-md bg-accent px-3 py-2 text-xs font-medium text-on-accent hover:bg-accent-hover disabled:opacity-60"
      >
        {saving ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        ) : (
          <Save className="h-3.5 w-3.5" />
        )}
        {t("common.save")}
      </button>
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
      className={`rounded-md px-2.5 py-1.5 text-xs ${
        active
          ? "bg-accent text-on-accent"
          : "text-text-secondary hover:bg-card-secondary"
      }`}
    >
      {label}
      <span
        className={active ? "ml-1 text-on-accent/80" : "ml-1 text-text-muted"}
      >
        {count}
      </span>
    </button>
  );
}

function StatusPill({ status }: { status: McpValidationStatus }) {
  const Icon =
    status === "valid"
      ? CheckCircle2
      : status === "invalid"
        ? XCircle
        : AlertTriangle;
  return (
    <span
      className={`inline-flex w-fit items-center gap-1 rounded px-1.5 py-0.5 text-xs ${
        status === "valid"
          ? "bg-success/10 text-success"
          : status === "invalid"
            ? "bg-error/10 text-error"
            : "bg-warning/10 text-warning"
      }`}
    >
      <Icon className="h-3 w-3" />
      {status}
    </span>
  );
}

function ClientBadges({ server }: { server: McpServer }) {
  return (
    <div className="flex flex-wrap items-center gap-1">
      {server.enabled_clients.map((client) => (
        <span
          key={client}
          className="inline-flex items-center gap-1 rounded bg-card-secondary px-1.5 py-0.5 text-xs text-text-secondary"
        >
          {clientIcon(client)}
          {clientLabel(client)}
        </span>
      ))}
    </div>
  );
}

function EnvSummary({ server }: { server: McpServer }) {
  const { t } = useI18n();
  const missing = server.env.filter((env) => !env.has_value).length;
  return (
    <div className="flex items-center gap-1.5 text-xs text-text-muted">
      <KeyRound className="h-3 w-3" />
      <span>{server.env.length}</span>
      {missing > 0 && (
        <span className="text-warning">
          /{missing} {t("mcp.missing")}
        </span>
      )}
    </div>
  );
}

function IconButton({
  children,
  title,
  danger,
  onClick,
}: {
  children: React.ReactNode;
  title: string;
  danger?: boolean;
  onClick: (event: React.MouseEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      className={`rounded p-1 text-text-muted hover:bg-card-secondary ${
        danger ? "hover:text-error" : "hover:text-text-primary"
      }`}
    >
      {children}
    </button>
  );
}

function DetailSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="mb-2 text-xs font-medium uppercase tracking-wide text-text-muted">
        {title}
      </div>
      {children}
    </section>
  );
}

function CodeBlock({ value }: { value: string }) {
  return (
    <pre className="max-h-44 overflow-auto whitespace-pre-wrap break-all rounded-md bg-card-secondary px-3 py-2 font-mono text-xs text-text-secondary">
      {value}
    </pre>
  );
}

function MutedText({ children }: { children: React.ReactNode }) {
  return <div className="text-xs text-text-muted">{children}</div>;
}

function clientLabel(client: string) {
  return clients.find((item) => item.id === client)?.label ?? client;
}

function clientIcon(client: string) {
  const logo = clients.find((item) => item.id === client)?.logo;
  if (!logo) return <ChevronDown className="h-3 w-3" />;
  return <ClientLogo id={logo} className="h-3.5 w-3.5" />;
}
