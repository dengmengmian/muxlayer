import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  Bot,
  CheckCircle2,
  Copy,
  Download,
  FolderOpen,
  Loader2,
  Play,
  Plug,
  RefreshCw,
  Server,
  Square,
  Settings2,
  Sparkles,
  Terminal,
  Wrench,
} from "lucide-react";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { EmptyState } from "@/components/common/EmptyState";
import { StatusBadge } from "@/components/common/StatusBadge";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";

type Tab = "agents" | "connections" | "settings";

interface ConfirmState {
  title: string;
  message: string;
  confirmLabel: string;
  variant?: "danger" | "default";
  run: () => Promise<void> | void;
}

const tabs: { id: Tab; icon: React.ComponentType<{ className?: string }> }[] = [
  { id: "agents", icon: Bot },
  { id: "connections", icon: Plug },
  { id: "settings", icon: Server },
];

function errorMessage(error: unknown): string {
  return (error as api.AppError)?.message ?? String(error);
}

function InfoRow({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: React.ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="flex min-w-0 items-start justify-between gap-4 py-1.5 text-xs">
      <span className="shrink-0 text-text-muted">{label}</span>
      <span
        className={`min-w-0 break-all text-right text-text-secondary ${mono ? "font-mono" : ""}`}
      >
        {value}
      </span>
    </div>
  );
}

function agentVariant(state: string): "success" | "warning" | "error" | "muted" {
  if (state === "available") return "success";
  if (state === "unverified" || state === "not_configured") return "warning";
  if (state === "not_installed") return "muted";
  return "error";
}

function clientVariant(status: string): "success" | "warning" | "error" | "muted" {
  if (status === "configured") return "success";
  if (status === "conflict" || status === "error") return "error";
  if (status === "not_installed") return "muted";
  return "warning";
}

function CopyAction({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="btn-secondary"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        } catch {
          /* clipboard permission can be unavailable in a webview */
        }
      }}
    >
      {copied ? (
        <CheckCircle2 className="h-3.5 w-3.5" />
      ) : (
        <Copy className="h-3.5 w-3.5" />
      )}
      {label}
    </button>
  );
}

function PathEditor({
  value,
  placeholder,
  saveLabel,
  resetLabel,
  cancelLabel,
  busy,
  onChange,
  onSave,
  onReset,
  onCancel,
}: {
  value: string;
  placeholder: string;
  saveLabel: string;
  resetLabel: string;
  cancelLabel: string;
  busy: boolean;
  onChange: (value: string) => void;
  onSave: () => void;
  onReset: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="mt-3 space-y-2 rounded-lg border border-border bg-card-secondary p-3">
      <input
        className="form-input w-full"
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
        spellCheck={false}
      />
      <div className="flex flex-wrap gap-2">
        <button className="btn-primary" onClick={onSave} disabled={busy}>
          {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
          {saveLabel}
        </button>
        <button className="btn-secondary" onClick={onReset} disabled={busy}>
          {resetLabel}
        </button>
        <button className="btn-secondary" onClick={onCancel} disabled={busy}>
          {cancelLabel}
        </button>
      </div>
    </div>
  );
}

export function AgentGateway() {
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>("agents");
  const [overview, setOverview] = useState<api.AgentGatewayOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  const [testResult, setTestResult] =
    useState<api.AgentGatewayTestResult | null>(null);
  const [diagnostics, setDiagnostics] = useState<string | null>(null);
  const [editingAgent, setEditingAgent] = useState<string | null>(null);
  const [agentPath, setAgentPath] = useState("");
  const [editingCli, setEditingCli] = useState(false);
  const [cliPath, setCliPath] = useState("");
  const [manager, setManager] = useState<api.GatewayManagerStatus | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await api.getAgentGatewayOverview();
      setOverview(next);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadManager = useCallback(async () => {
    try {
      setManager(await api.getGatewayManagerStatus());
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    loadManager();
    const timer = setInterval(loadManager, 5000);
    return () => clearInterval(timer);
  }, [loadManager]);

  const refreshService = useCallback(async () => {
    const service = await api.getAgentGatewayServiceStatus();
    setOverview((previous) => (previous ? { ...previous, service } : previous));
    return service;
  }, []);

  useEffect(() => {
    if (tab !== "settings") return;
    refreshService().catch((err) => toast("error", errorMessage(err)));
  }, [tab, refreshService]);

  const perform = useCallback(
    async (key: string, action: () => Promise<void>, success?: string) => {
      setBusy(key);
      try {
        await action();
        if (success) toast("success", success);
        await load();
      } catch (err) {
        toast("error", errorMessage(err));
      } finally {
        setBusy(null);
      }
    },
    [load]
  );

  const startEditingAgent = (agent: api.AgentGatewayAgentStatus) => {
    setEditingAgent(agent.id);
    setAgentPath(agent.configured_path ?? agent.executable ?? "");
  };

  const saveAgentPath = async (agent: api.AgentGatewayAgentStatus) => {
    await perform(
      `path:${agent.id}`,
      async () => {
        const result = await api.setAgentGatewayAgentPath(
          agent.id,
          agentPath.trim() || null
        );
        setEditingAgent(null);
        if (result.requires_service_restart) {
          toast("warning", t("agw.agent.restart_required"));
        }
      },
      t("agw.agent.path_saved")
    );
  };

  const resetAgentPath = async (agent: api.AgentGatewayAgentStatus) => {
    await perform(
      `path:${agent.id}`,
      async () => {
        await api.setAgentGatewayAgentPath(agent.id, null);
        setEditingAgent(null);
      },
      t("agw.agent.path_saved")
    );
  };

  const askTest = (agent: api.AgentGatewayAgentStatus) => {
    setConfirm({
      title: t("agw.agent.test_title"),
      message: t("agw.agent.test_message"),
      confirmLabel: t("agw.agent.test"),
      run: async () => {
        setBusy(`test:${agent.id}`);
        try {
          const result = await api.testAgentGatewayAgent(agent.id, true);
          setTestResult(result);
          toast(
            result.success ? "success" : "error",
            result.success
              ? t("agw.agent.test_success")
              : result.error || t("agw.agent.test_failed")
          );
          await load();
        } catch (err) {
          toast("error", errorMessage(err));
        } finally {
          setBusy(null);
        }
      },
    });
  };

  const askConfigure = (client: api.AgentGatewayClientStatus) => {
    setConfirm({
      title: t("agw.connections.confirm_configure_title"),
      message: t("agw.connections.confirm_configure_message"),
      confirmLabel: t("agw.connections.configure"),
      run: () =>
        perform(
          `mcp:${client.id}`,
          async () => {
            const result = await api.configureAgentGatewayMcp(
              client.id,
              true
            );
            toast("success", result.message);
          },
          t("agw.connections.configured_ok")
        ),
    });
  };

  const askRemove = (client: api.AgentGatewayClientStatus) => {
    setConfirm({
      title: t("agw.connections.confirm_remove_title"),
      message: t("agw.connections.confirm_remove_message"),
      confirmLabel: t("agw.connections.remove"),
      variant: "danger",
      run: () =>
        perform(
          `mcp:${client.id}`,
          async () => {
            const result = await api.removeAgentGatewayMcp(client.id, true);
            toast("success", result.message);
          },
          t("agw.connections.removed_ok")
        ),
    });
  };

  const askService = async (action: "stop" | "restart") => {
    if (!overview) return;
    let service = overview.service;
    try {
      service = await refreshService();
    } catch (err) {
      toast("error", errorMessage(err));
      return;
    }
    const tasks = service.running_tasks.length;
    const taskNote =
      tasks > 0
        ? ` ${t("agw.service.stop_confirm_tasks").replace(
            "{count}",
            String(tasks)
          )}`
        : "";
    setConfirm({
      title: t(
        action === "stop"
          ? "agw.service.stop_confirm_title"
          : "agw.service.restart_confirm_title"
      ),
      message:
        t(
          action === "stop"
            ? "agw.service.stop_confirm_message"
            : "agw.service.restart_confirm_message"
        ) + taskNote,
      confirmLabel: t(
        action === "stop" ? "agw.service.stop" : "agw.service.restart"
      ),
      variant: "danger",
      run: () =>
        perform(
          `service:${action}`,
          async () => {
            if (action === "stop") {
              await api.stopAgentGatewayService(true);
            } else {
              await api.restartAgentGatewayService(true);
            }
            await loadManager();
          },
          t(
            action === "stop"
              ? "agw.service.stopped_msg"
              : "agw.service.restarted"
          )
        ),
    });
  };

  const startService = () =>
    perform(
      "service:start",
      async () => {
        await api.startAgentGatewayService();
        await loadManager();
      },
      t("agw.service.started")
    );

  const startModelGateway = () =>
    perform(
      "manager:model:start",
      async () => {
        await api.startGateway();
        await loadManager();
      },
      t("agw.gateway.model_started")
    );

  const stopModelGateway = () =>
    perform(
      "manager:model:stop",
      async () => {
        await api.stopGateway();
        await loadManager();
      },
      t("agw.gateway.model_stopped")
    );

  const setAgentAutoStart = async (enabled: boolean) => {
    setBusy("manager:auto");
    try {
      await api.setAgentGatewayAutoStart(enabled);
      await loadManager();
    } catch (err) {
      toast("error", errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const saveCliPath = async () => {
    setBusy("cli");
    try {
      await api.setAgentGatewayServePath(cliPath.trim() || null);
      setEditingCli(false);
      toast("success", t("agw.service.path_saved"));
      await load();
    } catch (err) {
      toast("error", errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const copyDiagnostics = async () => {
    setBusy("diagnostics");
    try {
      const text = await api.getAgentGatewayDiagnostics();
      setDiagnostics(text);
      await navigator.clipboard.writeText(text);
      toast("success", "Diagnostics copied.");
    } catch (err) {
      toast("error", errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const customJson = useMemo(() => {
    const command = overview?.cli.path ?? "agentgate-serve";
    return JSON.stringify(
      {
        mcpServers: {
          muxlayer: { command, args: ["mcp-agent"] },
        },
      },
      null,
      2
    );
  }, [overview?.cli.path]);

  if (loading && !overview) {
    return (
      <div className="desktop-page">
        <div className="surface-panel flex items-center gap-3 p-6 text-xs text-text-muted">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("agw.loading")}
        </div>
      </div>
    );
  }

  if (error && !overview) {
    return (
      <div className="desktop-page">
        <div className="surface-panel p-6">
          <div className="mb-3 flex items-center gap-2 text-sm font-medium text-error">
            <AlertTriangle className="h-4 w-4" />
            {error}
          </div>
          <button className="btn-secondary" onClick={load}>
            <RefreshCw className="h-3.5 w-3.5" />
            {t("agw.reload")}
          </button>
        </div>
      </div>
    );
  }

  if (!overview) return null;

  const cliMissing = !overview.cli.installed;
  const service = overview.service;

  return (
    <div className="desktop-page">
      <div className="desktop-page-header">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs font-semibold uppercase tracking-[0.16em] text-accent">
              {t("agw.title")}
            </p>
            <p className="mt-1 max-w-2xl text-xs leading-relaxed text-text-muted">
              {t("agw.subtitle")}
            </p>
          </div>
          <button
            className="btn-secondary"
            onClick={load}
            disabled={loading || busy !== null}
          >
            <RefreshCw
              className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
            />
            {t("agw.reload")}
          </button>
        </div>
        <div className="mt-4 flex flex-wrap items-center gap-2 text-xs">
          <StatusBadge variant={cliMissing ? "warning" : "success"}>
            {cliMissing
              ? t("agw.cli_missing_title")
              : `${overview.cli.version ?? "agentgate"} · ${overview.cli.path ?? ""}`}
          </StatusBadge>
          <StatusBadge variant={service.running ? "success" : "muted"}>
            {service.running ? t("agw.service.running") : t("agw.service.stopped")}
          </StatusBadge>
          <span className="font-mono text-text-muted">{overview.data_dir}</span>
        </div>
      </div>

      {manager && (
        <div className="surface-panel p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <h2 className="text-sm font-semibold text-text-primary">
                {t("agw.gateway.title")}
              </h2>
              <p className="mt-1 text-xs text-text-muted">
                {t("agw.gateway.desc")}
              </p>
            </div>
            <span className="font-mono text-[11px] text-text-muted">
              {manager.bundled_cli
                ? `${t("agw.gateway.bundled_cli")}: ${manager.bundled_cli}`
                : t("agw.gateway.no_bundled_cli")}
            </span>
          </div>
          <div className="mt-4 grid gap-3 md:grid-cols-2">
            <div className="rounded-lg border border-border bg-card-secondary p-3">
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-medium text-text-primary">
                  {t("agw.gateway.model")}
                </span>
                <StatusBadge
                  variant={
                    manager.model?.running
                      ? "success"
                      : manager.model_error
                        ? "error"
                        : "muted"
                  }
                >
                  {manager.model?.running
                    ? t("agw.service.running")
                    : manager.model_error
                      ? t("agw.service.error")
                      : t("agw.service.stopped")}
                </StatusBadge>
              </div>
              <p className="mt-2 font-mono text-[11px] text-text-muted">
                {manager.model
                  ? `${manager.model.host}:${manager.model.port}`
                  : manager.model_error ?? t("agw.common.none")}
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                {!manager.model?.running && (
                  <button
                    className="btn-secondary"
                    onClick={startModelGateway}
                    disabled={busy !== null}
                  >
                    {busy === "manager:model:start" ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Play className="h-3.5 w-3.5" />
                    )}
                    {t("agw.service.start")}
                  </button>
                )}
                {manager.model?.running && (
                  <button
                    className="btn-secondary"
                    onClick={stopModelGateway}
                    disabled={busy !== null}
                  >
                    <Square className="h-3.5 w-3.5" />
                    {t("agw.service.stop")}
                  </button>
                )}
              </div>
            </div>
            <div className="rounded-lg border border-border bg-card-secondary p-3">
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-medium text-text-primary">
                  {t("agw.gateway.agent")}
                </span>
                <StatusBadge
                  variant={
                    manager.agent.running
                      ? "success"
                      : manager.agent.error
                        ? "error"
                        : "muted"
                  }
                >
                  {manager.agent.running
                    ? t("agw.service.running")
                    : manager.agent.error
                      ? t("agw.service.error")
                      : t("agw.service.stopped")}
                </StatusBadge>
              </div>
              <p className="mt-2 font-mono text-[11px] text-text-muted">
                {manager.agent.service_version ??
                  manager.agent.error ??
                  t("agw.common.none")}
              </p>
              <label className="mt-2 flex items-center gap-2 text-xs text-text-secondary">
                <input
                  type="checkbox"
                  checked={manager.agent_auto_start}
                  disabled={busy !== null}
                  onChange={(event) => setAgentAutoStart(event.target.checked)}
                />
                {t("agw.gateway.auto_start")}
              </label>
              <div className="mt-3 flex flex-wrap gap-2">
                {!manager.agent.running && (
                  <button
                    className="btn-secondary"
                    onClick={startService}
                    disabled={busy !== null || !manager.agent.installed}
                  >
                    {busy === "service:start" ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Play className="h-3.5 w-3.5" />
                    )}
                    {t("agw.service.start")}
                  </button>
                )}
                {manager.agent.running && (
                  <button
                    className="btn-secondary"
                    onClick={() => askService("stop")}
                    disabled={busy !== null}
                  >
                    <Square className="h-3.5 w-3.5" />
                    {t("agw.service.stop")}
                  </button>
                )}
              </div>
            </div>
          </div>
        </div>
      )}

      {cliMissing && (
        <div className="surface-panel border-warning/30 bg-warning-soft/40 p-4">
          <div className="flex items-start gap-3">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
            <div className="min-w-0">
              <p className="text-sm font-medium text-text-primary">
                {t("agw.cli_missing_title")}
              </p>
              <p className="mt-1 text-xs leading-relaxed text-text-secondary">
                {overview.cli.install_hint || t("agw.cli_missing_desc")}
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                <a
                  className="btn-secondary"
                  href={overview.cli.download_url}
                  target="_blank"
                  rel="noreferrer"
                >
                  <Download className="h-3.5 w-3.5" />
                  {t("agw.download")}
                </a>
                <button
                  className="btn-primary"
                  onClick={() => {
                    setTab("settings");
                    setEditingCli(true);
                    setCliPath(overview.cli.path ?? "");
                  }}
                >
                  {t("agw.set_manual_path")}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      <nav className="flex flex-wrap gap-1.5">
        {tabs.map(({ id, icon: Icon }) => (
          <button
            key={id}
            onClick={() => setTab(id)}
            className={`flex items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors ${
              tab === id
                ? "bg-accent-soft text-accent font-medium"
                : "text-text-secondary hover:bg-hover hover:text-text-primary"
            }`}
          >
            <Icon className="h-4 w-4" />
            {t(`agw.tab.${id}`)}
          </button>
        ))}
      </nav>

      {tab === "agents" && (
        <div className="space-y-4">
          <div>
            <h2 className="text-sm font-semibold text-text-primary">
              {t("agw.agents.title")}
            </h2>
            <p className="mt-1 text-xs text-text-muted">
              {t("agw.agents.desc")}
            </p>
          </div>
          <div className="grid gap-4 xl:grid-cols-2">
            {overview.agents.map((agent) => (
              <div key={agent.id} className="surface-panel p-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="flex items-center gap-3">
                    <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent-soft text-accent">
                      {agent.id === "codex-native" ? (
                        <Terminal className="h-4 w-4" />
                      ) : (
                        <Sparkles className="h-4 w-4" />
                      )}
                    </div>
                    <div>
                      <h3 className="text-sm font-semibold text-text-primary">
                        {agent.name}
                      </h3>
                      <p className="text-xs text-text-muted">
                        {agent.executable ?? t("agw.agent.no_cli_hint")}
                      </p>
                    </div>
                  </div>
                  <StatusBadge variant={agentVariant(agent.state)}>
                    {t(`agw.agent.state.${agent.state}`, agent.state)}
                  </StatusBadge>
                </div>

                <div className="mt-4 divide-y divide-border/60">
                  <InfoRow
                    label={t("agw.agent.version")}
                    value={agent.version ?? t("agw.common.unknown")}
                    mono
                  />
                  <InfoRow
                    label={t("agw.agent.path")}
                    value={agent.executable ?? t("agw.common.none")}
                    mono
                  />
                  <InfoRow
                    label={t("agw.agent.auth")}
                    value={
                      agent.auth_detail ||
                      agent.auth_state ||
                      t("agw.common.unknown")
                    }
                  />
                  <InfoRow
                    label={t("agw.agent.source")}
                    value={agent.source}
                  />
                  <InfoRow
                    label={t("agw.agent.model")}
                    value={agent.model ?? "—"}
                  />
                  <InfoRow
                    label={t("agw.agent.config")}
                    value={agent.config_path ?? t("agw.common.none")}
                    mono
                  />
                  <InfoRow
                    label={t("agw.agent.detection")}
                    value={agent.detection}
                  />
                </div>

                {(agent.warnings.length > 0 || agent.error) && (
                  <div className="mt-3 space-y-1.5 rounded-lg bg-warning-soft/50 p-3 text-xs text-warning">
                    {agent.error && <p>{agent.error}</p>}
                    {agent.warnings.map((warning) => (
                      <p key={warning}>{warning}</p>
                    ))}
                  </div>
                )}

                {editingAgent === agent.id && (
                  <PathEditor
                    value={agentPath}
                    placeholder={t("agw.agent.path_placeholder")}
                    saveLabel={t("agw.agent.save_path")}
                    resetLabel={t("agw.agent.reset_path")}
                    cancelLabel={t("agw.common.cancel")}
                    busy={busy === `path:${agent.id}`}
                    onChange={setAgentPath}
                    onSave={() => saveAgentPath(agent)}
                    onReset={() => resetAgentPath(agent)}
                    onCancel={() => setEditingAgent(null)}
                  />
                )}

                <div className="mt-4 flex flex-wrap gap-2">
                  <button
                    className="btn-primary"
                    onClick={() => askTest(agent)}
                    disabled={
                      busy !== null ||
                      !agent.installed ||
                      !agent.available ||
                      cliMissing
                    }
                  >
                    {busy === `test:${agent.id}` ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <CheckCircle2 className="h-3.5 w-3.5" />
                    )}
                    {t("agw.agent.test")}
                  </button>
                  <button
                    className="btn-secondary"
                    onClick={() => startEditingAgent(agent)}
                    disabled={busy !== null}
                  >
                    <Settings2 className="h-3.5 w-3.5" />
                    {t("agw.agent.edit_path")}
                  </button>
                </div>

                {testResult?.agent === agent.id && (
                  <div className="mt-3 rounded-lg border border-border bg-card-secondary p-3 text-xs">
                    <div className="flex items-center justify-between gap-2">
                      <span className="font-medium text-text-primary">
                        {t("agw.agent.test_result")}
                      </span>
                      <StatusBadge
                        variant={testResult.success ? "success" : "error"}
                      >
                        {testResult.state}
                      </StatusBadge>
                    </div>
                    <InfoRow
                      label="task_id"
                      value={testResult.task_id ?? "—"}
                      mono
                    />
                    <InfoRow
                      label="session_id"
                      value={testResult.session_id ?? "—"}
                      mono
                    />
                    <InfoRow
                      label="duration"
                      value={`${testResult.duration_ms} ms`}
                    />
                    {testResult.final_answer && (
                      <p className="mt-2 whitespace-pre-wrap break-words text-text-secondary">
                        {testResult.final_answer.slice(0, 500)}
                      </p>
                    )}
                    {testResult.error && (
                      <p className="mt-2 text-error">{testResult.error}</p>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      {tab === "connections" && (
        <div className="space-y-4">
          <div>
            <h2 className="text-sm font-semibold text-text-primary">
              {t("agw.connections.title")}
            </h2>
            <p className="mt-1 text-xs text-text-muted">
              {t("agw.connections.desc")}
            </p>
          </div>
          <div className="grid gap-4 xl:grid-cols-2">
            {overview.clients.map((client) => (
              <div key={client.id} className="surface-panel p-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="flex items-center gap-3">
                    <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent-soft text-accent">
                      <Plug className="h-4 w-4" />
                    </div>
                    <div>
                      <h3 className="text-sm font-semibold text-text-primary">
                        {client.name}
                      </h3>
                      <p className="text-xs text-text-muted">
                        {client.installed
                          ? `${t("agw.connections.version")} ${client.version ?? ""}`
                          : client.executable
                            ? t("agw.connections.unavailable")
                            : t("agw.connections.not_installed")}
                      </p>
                    </div>
                  </div>
                  <StatusBadge variant={clientVariant(client.config_status)}>
                    {t(
                      `agw.connections.${client.config_status}`,
                      client.config_status
                    )}
                  </StatusBadge>
                </div>
                <div className="mt-4 divide-y divide-border/60">
                  <InfoRow
                    label={t("agw.connections.config_path")}
                    value={client.config_path || "—"}
                    mono
                  />
                  <InfoRow
                    label={t("agw.connections.command")}
                    value={
                      (client.command ?? client.copy_command) || "—"
                    }
                    mono
                  />
                  <InfoRow
                    label={t("agw.connections.args")}
                    value={(client.args.length
                      ? client.args
                      : client.copy_args
                    ).join(" ")}
                    mono
                  />
                  <InfoRow
                    label={t("agw.connections.compatibility")}
                    value={client.compatibility}
                  />
                </div>
                {client.warning && (
                  <p className="mt-3 rounded-lg bg-warning-soft/50 p-3 text-xs text-warning">
                    {client.warning}
                  </p>
                )}
                {client.error && (
                  <p className="mt-2 text-xs text-error">{client.error}</p>
                )}
                <p className="mt-3 text-[11px] leading-relaxed text-text-muted">
                  {client.live_status_note || t("agw.connections.online_note")}
                </p>
                <div className="mt-4 flex flex-wrap gap-2">
                  <button
                    className="btn-primary"
                    onClick={() => askConfigure(client)}
                    disabled={
                      busy !== null ||
                      !client.installed ||
                      client.matching ||
                      client.config_status === "conflict" ||
                      cliMissing
                    }
                  >
                    <Wrench className="h-3.5 w-3.5" />
                    {t("agw.connections.configure")}
                  </button>
                  <button
                    className="btn-secondary"
                    onClick={() => askRemove(client)}
                    disabled={busy !== null || !client.managed}
                  >
                    {t("agw.connections.remove")}
                  </button>
                  <CopyAction
                    text={
                      client.id === "codeleveler"
                        ? client.copy_toml
                        : client.copy_json
                    }
                    label={
                      client.id === "codeleveler"
                        ? t("agw.connections.copy_toml")
                        : t("agw.connections.copy_json")
                    }
                  />
                </div>
              </div>
            ))}
            <div className="surface-panel p-4">
              <div className="flex items-center gap-3">
                <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent-soft text-accent">
                  <Copy className="h-4 w-4" />
                </div>
                <div>
                  <h3 className="text-sm font-semibold text-text-primary">
                    {t("agw.connections.custom_title")}
                  </h3>
                  <p className="text-xs text-text-muted">
                    {t("agw.connections.custom_desc")}
                  </p>
                </div>
              </div>
              <pre className="surface-scroll mt-4 max-h-64 rounded-lg bg-card-secondary p-3 text-[11px] leading-relaxed text-text-secondary">
                {customJson}
              </pre>
              <div className="mt-3 flex flex-wrap gap-2">
                <CopyAction
                  text={customJson}
                  label={t("agw.connections.copy_json")}
                />
                <CopyAction
                  text={`${overview.cli.path ?? "agentgate-serve"} mcp-agent`}
                  label={t("agw.connections.copy_command")}
                />
              </div>
            </div>
          </div>
        </div>
      )}

      {tab === "settings" && (
        <div className="space-y-4">
          <div>
            <h2 className="text-sm font-semibold text-text-primary">
              {t("agw.settings.title")}
            </h2>
            <p className="mt-1 text-xs text-text-muted">
              {t("agw.settings.desc")}
            </p>
          </div>
          <div className="surface-panel p-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex items-center gap-3">
                <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent-soft text-accent">
                  <Server className="h-4 w-4" />
                </div>
                <div>
                  <h3 className="text-sm font-semibold text-text-primary">
                    {t("agw.settings.title")}
                  </h3>
                  <p className="text-xs text-text-muted">
                    {service.error ?? t("agw.service.data_dir_note")}
                  </p>
                </div>
              </div>
              <StatusBadge
                variant={
                  service.running
                    ? "success"
                    : service.error
                      ? "error"
                      : "muted"
                }
              >
                {service.running
                  ? t("agw.service.running")
                  : service.error
                    ? t("agw.service.error")
                    : t("agw.service.stopped")}
              </StatusBadge>
            </div>
            <div className="mt-4 divide-y divide-border/60">
              <InfoRow
                label={t("agw.service.version")}
                value={service.service_version ?? t("agw.common.none")}
                mono
              />
              <InfoRow
                label={t("agw.service.cli")}
                value={
                  service.cli_path
                    ? `${service.cli_version ?? ""} · ${service.cli_path}`
                    : overview.cli.error ?? t("agw.common.none")
                }
                mono
              />
              <InfoRow
                label={t("agw.service.data_dir")}
                value={service.data_dir}
                mono
              />
              <InfoRow
                label={t("agw.service.tasks")}
                value={
                  service.running_tasks.length
                    ? service.running_tasks.join(", ")
                    : t("agw.service.tasks_none")
                }
                mono
              />
              <InfoRow
                label={t("agw.service.last_error")}
                value={service.last_error ?? t("agw.service.no_error")}
              />
            </div>
            <div className="mt-4 flex flex-wrap gap-2">
              <button
                className="btn-primary"
                onClick={startService}
                disabled={busy !== null || service.running || cliMissing}
              >
                {busy === "service:start" ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : null}
                {t("agw.service.start")}
              </button>
              <button
                className="btn-secondary"
                onClick={() => askService("stop")}
                disabled={busy !== null || !service.running}
              >
                {t("agw.service.stop")}
              </button>
              <button
                className="btn-secondary"
                onClick={() => askService("restart")}
                disabled={busy !== null || !service.running || cliMissing}
              >
                {t("agw.service.restart")}
              </button>
              <button
                className="btn-secondary"
                onClick={() => api.openAgentGatewayDataDir().catch(() => {})}
              >
                <FolderOpen className="h-3.5 w-3.5" />
                {t("agw.service.open_data")}
              </button>
              <button
                className="btn-secondary"
                onClick={copyDiagnostics}
                disabled={busy !== null}
              >
                {busy === "diagnostics" ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {t("agw.service.copy_diagnostics")}
              </button>
            </div>
          </div>

          <div className="surface-panel p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h3 className="text-sm font-semibold text-text-primary">
                  {t("agw.service.cli_path")}
                </h3>
                <p className="mt-1 text-xs text-text-muted">
                  {overview.cli.path ?? overview.cli.error ?? t("agw.common.none")}
                </p>
              </div>
              <button
                className="btn-secondary"
                onClick={() => {
                  setEditingCli((value) => !value);
                  setCliPath(overview.cli.path ?? "");
                }}
              >
                {t("agw.set_manual_path")}
              </button>
            </div>
            {editingCli && (
              <PathEditor
                value={cliPath}
                placeholder={t("agw.service.cli_path_placeholder")}
                saveLabel={t("agw.agent.save_path")}
                resetLabel={t("agw.auto_detect")}
                cancelLabel={t("agw.common.cancel")}
                busy={busy === "cli"}
                onChange={setCliPath}
                onSave={saveCliPath}
                onReset={async () => {
                  setBusy("cli");
                  try {
                    await api.setAgentGatewayServePath(null);
                    setEditingCli(false);
                    toast("success", t("agw.service.path_saved"));
                    await load();
                  } catch (err) {
                    toast("error", errorMessage(err));
                  } finally {
                    setBusy(null);
                  }
                }}
                onCancel={() => setEditingCli(false)}
              />
            )}
          </div>

          {(
            <div className="surface-panel p-4">
              <h3 className="text-sm font-semibold text-text-primary">
                {t("agw.service.diagnostics_title")}
              </h3>
              <p className="mt-1 text-xs text-text-muted">
                {t("agw.service.data_dir_note")}
              </p>
              <pre className="surface-scroll mt-3 max-h-80 whitespace-pre-wrap rounded-lg bg-card-secondary p-3 text-[11px] leading-relaxed text-text-secondary">
                {diagnostics ?? t("agw.service.copy_diagnostics")}
              </pre>
            </div>
          )}
        </div>
      )}

      {overview.agents.length === 0 && (
        <EmptyState
          icon={Bot}
          title={t("agw.agent.no_agents")}
          description={t("agw.agent.no_cli_hint")}
        />
      )}

      <ConfirmDialog
        open={confirm !== null}
        title={confirm?.title ?? ""}
        message={confirm?.message ?? ""}
        confirmLabel={confirm?.confirmLabel}
        variant={confirm?.variant}
        onConfirm={() => {
          const run = confirm?.run;
          setConfirm(null);
          if (run) void run();
        }}
        onCancel={() => setConfirm(null)}
      />
    </div>
  );
}
