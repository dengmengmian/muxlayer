import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  Copy,
  Download,
  Loader2,
  Play,
  RefreshCw,
  Square,
  Settings2,
  Wrench,
} from "lucide-react";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { StatusBadge } from "@/components/common/StatusBadge";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";

interface ConfirmState {
  title: string;
  message: string;
  confirmLabel: string;
  variant?: "danger" | "default";
  run: () => Promise<void> | void;
}



function errorMessage(error: unknown): string {
  return (error as api.AppError)?.message ?? String(error);
}

function redactSecrets(text: string): string {
  return text
    .replace(/(bearer\s+)[^\s,;]+/gi, "$1[redacted]")
    .replace(/((?:api[_-]?key|token|authorization|auth)\s*[:=]\s*)[^\s,;]+/gi, "$1[redacted]")
    .replace(/sk-[A-Za-z0-9_-]{8,}/g, "[redacted]");
}

function isAuthError(message: string): boolean {
  const value = message.toLowerCase();
  return (
    value.includes("401") ||
    value.includes("unauthorized") ||
    value.includes("invalid x-api-key") ||
    value.includes("authentication")
  );
}

function friendlyMessage(message: string, t: (key: string, fallback?: string) => string): string {
  if (!message.trim()) return t("agw.ux.error.generic");
  const value = message.toLowerCase();
  if (isAuthError(message)) return t("agw.ux.error.auth");
  if (value.includes("403") || value.includes("forbidden")) return t("agw.ux.error.permission");
  if (value.includes("429") || value.includes("rate limit")) return t("agw.ux.error.rate");
  if (value.includes("timed out") || value.includes("timeout")) return t("agw.ux.error.timeout");
  if (value.includes("permission denied") || value.includes("not writable")) return t("agw.ux.error.write");
  if (value.includes("env: node") || value.includes("node: no such file")) {
    return t("agw.ux.error.node");
  }
  if (
    value.includes("no such file") ||
    value.includes("not found on path") ||
    value.includes("not installed") ||
    value.includes("not executable")
  ) {
    return t("agw.ux.error.missing");
  }
  if (value.includes("database is locked") || value.includes("db lock")) return t("agw.ux.error.db");
  if (value.includes("did not become ready")) return t("agw.ux.error.service_start");
  if (value.includes("already running")) return t("agw.ux.error.already_running");
  return t("agw.ux.error.generic");
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
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [expandedAgents, setExpandedAgents] = useState<Set<string>>(new Set());
  const [expandedClients, setExpandedClients] = useState<Set<string>>(new Set());
  const [exampleAgent, setExampleAgent] = useState("codex-native");
  const [verifiedAgents, setVerifiedAgents] = useState<Set<string>>(new Set());

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await api.getAgentGatewayOverview();
      setOverview(next);
    } catch (err) {
      setError(friendlyMessage(errorMessage(err), t));
    } finally {
      setLoading(false);
    }
  }, [t]);

  const loadManager = useCallback(async () => {
    try {
      setManager(await api.getGatewayManagerStatus());
    } catch (err) {
      setError(friendlyMessage(errorMessage(err), t));
    }
  }, [t]);

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

  const perform = useCallback(
    async (key: string, action: () => Promise<void>, success?: string) => {
      setBusy(key);
      try {
        await action();
        if (success) toast("success", success);
        await load();
      } catch (err) {
        toast("error", friendlyMessage(errorMessage(err), t));
      } finally {
        setBusy(null);
      }
    },
    [load, t]
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
          setVerifiedAgents((previous) => {
            const next = new Set(previous);
            if (result.success) next.add(agent.id);
            else next.delete(agent.id);
            return next;
          });
          toast(
            result.success ? "success" : "error",
            result.success
              ? t("agw.agent.test_success")
              : friendlyMessage(result.error ?? "", t)
          );
          await load();
        } catch (err) {
          toast("error", friendlyMessage(errorMessage(err), t));
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
      toast("error", friendlyMessage(errorMessage(err), t));
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

  const setAgentAutoStart = async (enabled: boolean) => {
    setBusy("manager:auto");
    try {
      await api.setAgentGatewayAutoStart(enabled);
      await loadManager();
    } catch (err) {
      toast("error", friendlyMessage(errorMessage(err), t));
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
      toast("error", friendlyMessage(errorMessage(err), t));
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
      toast("success", t("agw.ux.diagnostics_copied"));
    } catch (err) {
      toast("error", friendlyMessage(errorMessage(err), t));
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
  const serviceError = manager?.agent_start_error ?? service.error;
  const serviceHealthy = service.running && !serviceError;

  const usableAgents = overview.agents.filter((agent) =>
    verifiedAgents.has(agent.id)
  );
  const readyClients = overview.clients.filter(
    (client) => client.config_status === "configured"
  );
  const staleClients = overview.clients.filter(
    (client) => client.config_status === "stale"
  );
  const activeExample =
    exampleAgent === "claude-code" && usableAgents.some((agent) => agent.id === "claude-code")
      ? "claude-code"
      : usableAgents.some((agent) => agent.id === "codex-native")
        ? "codex-native"
        : usableAgents[0]?.id ?? "codex-native";
  const exampleText =
    activeExample === "claude-code"
      ? "请调用 muxlayer 的 agent_list，然后使用 agent_run，agent 选择 claude-code，对当前项目做只读架构审计，列出主要问题和代码证据；不要修改任何文件。"
      : "请调用 muxlayer 的 agent_list，然后使用 agent_run，agent 选择 codex-native，对当前项目做只读架构审计，列出主要问题和代码证据；不要修改任何文件。";

  const toggleAgentDetails = (id: string) => {
    setExpandedAgents((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const toggleClientDetails = (id: string) => {
    setExpandedClients((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const editAgentPath = (agent: api.AgentGatewayAgentStatus) => {
    setExpandedAgents((previous) => new Set(previous).add(agent.id));
    startEditingAgent(agent);
  };

  return (
    <div className="desktop-page">
      <div className="desktop-page-header">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs font-semibold uppercase tracking-[0.16em] text-accent">
              {t("agw.title")}
            </p>
            <h1 className="mt-1 text-lg font-semibold text-text-primary">
              {t("agw.ux.headline")}
            </h1>
            <p className="mt-1 max-w-2xl text-xs leading-relaxed text-text-muted">
              {t("agw.ux.intro")}
            </p>
          </div>
          <button
            className="btn-secondary"
            onClick={load}
            disabled={loading || busy !== null}
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
            {t("agw.reload")}
          </button>
        </div>
        <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-2 text-xs">
          <span className="inline-flex items-center gap-2 rounded-full border border-border bg-card-secondary px-3 py-1">
            <span
              className={`h-1.5 w-1.5 rounded-full ${
                serviceHealthy
                  ? "bg-success"
                  : service.running
                    ? "bg-warning"
                    : "bg-text-muted"
              }`}
            />
            <span className="font-medium text-text-secondary">
              {manager?.agent_starting
                ? t("agw.service.starting")
                : serviceHealthy
                  ? t("agw.ux.service_healthy")
                  : serviceError
                    ? t("agw.ux.service_error")
                    : t("agw.ux.service_stopped")}
            </span>
            {service.service_version && (
              <span className="font-mono text-text-muted">
                {service.service_version}
              </span>
            )}
          </span>
          <span className="text-text-muted">{t("agw.ux.service_note")}</span>
        </div>
      </div>

      {cliMissing && (
        <div className="surface-panel border-warning/30 bg-warning-soft/40 p-4">
          <div className="flex items-start gap-3">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
            <div className="min-w-0">
              <p className="text-sm font-medium text-text-primary">
                {t("agw.ux.cli_missing_title")}
              </p>
              <p className="mt-1 text-xs leading-relaxed text-text-secondary">
                {t("agw.ux.cli_missing_desc")}
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
                    setAdvancedOpen(true);
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

      {error && (
        <div className="surface-panel border-error/30 bg-error-soft/30 p-4 text-xs text-error">
          {friendlyMessage(error, t)}
        </div>
      )}

      <section className="space-y-3">
        <div>
          <h2 className="text-sm font-semibold text-text-primary">
            {t("agw.ux.step1_title")}
          </h2>
          <p className="mt-1 text-xs text-text-muted">{t("agw.ux.step1_desc")}</p>
        </div>
        {overview.agents.filter((agent) => agent.installed).length === 0 ? (
          <div className="surface-panel p-5">
            <p className="text-sm font-medium text-text-primary">
              {t("agw.ux.no_agent_title")}
            </p>
            <p className="mt-1 text-xs leading-relaxed text-text-secondary">
              {t("agw.ux.no_agent_desc")}
            </p>
            <div className="mt-3 flex flex-wrap gap-2">
              <a className="btn-secondary" href="https://developers.openai.com/codex/cli/" target="_blank" rel="noreferrer">
                {t("agw.ux.install_codex")}
              </a>
              <a className="btn-secondary" href="https://docs.anthropic.com/en/docs/claude-code" target="_blank" rel="noreferrer">
                {t("agw.ux.install_claude")}
              </a>
              <button className="btn-secondary" onClick={load} disabled={loading}>
                {t("agw.reload")}
              </button>
            </div>
          </div>
        ) : (
          <div className="grid gap-4 lg:grid-cols-2">
            {overview.agents.map((agent) => {
              const failed =
                testResult?.agent === agent.id && !testResult.success;
              const verified =
                testResult?.agent === agent.id && testResult.success;
              const configured =
                !verified && agent.state === "available";
              const state = failed
                ? "test_failed"
                : verified
                  ? "available"
                  : configured
                    ? "configured"
                    : agent.state;
              const variant = failed
                ? "error"
                : verified
                  ? "success"
                  : configured
                    ? "warning"
                    : agentVariant(agent.state);
              const hint = failed
                ? t("agw.ux.agent.test_failed_hint")
                : verified
                  ? t("agw.ux.agent.verified_hint")
                  : configured
                    ? t("agw.ux.agent.configured_hint")
                    : agent.state === "available"
                  ? t("agw.ux.agent.available_hint")
                  : agent.state === "unverified"
                    ? t("agw.ux.agent.unverified_hint")
                    : agent.state === "not_configured"
                      ? t("agw.ux.agent.not_configured_hint")
                      : agent.state === "not_executable"
                        ? t("agw.ux.agent.not_executable_hint")
                        : agent.state === "not_installed"
                          ? t("agw.ux.agent.not_installed_hint")
                          : t("agw.ux.agent.error_hint");
              const detailsOpen = expandedAgents.has(agent.id);
              return (
                <div key={agent.id} className="surface-panel flex flex-col gap-3 p-4">
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <h3 className="text-sm font-semibold text-text-primary">
                        {agent.name}
                      </h3>
                      <p className="mt-1 text-xs text-text-secondary">
                        {agent.source || t("agw.ux.unknown_source")}
                        {agent.model ? ` · ${agent.model}` : ""}
                      </p>
                    </div>
                    <StatusBadge variant={variant}>
                      {t(`agw.ux.agent_state.${state}`, state)}
                    </StatusBadge>
                  </div>
                  <p className="text-xs leading-relaxed text-text-muted">{hint}</p>

                  {testResult?.agent === agent.id && (
                    <div
                      className={`rounded-lg border p-3 text-xs ${
                        testResult.success
                          ? "border-success/25 bg-success-soft/30 text-text-secondary"
                          : "border-error/25 bg-error-soft/20 text-text-secondary"
                      }`}
                    >
                      <div className="font-medium text-text-primary">
                        {testResult.success
                          ? t("agw.ux.test_passed")
                          : t("agw.ux.test_failed")}
                      </div>
                      {!testResult.success && (
                        <>
                          <p className="mt-1 leading-relaxed">
                            {friendlyMessage(testResult.error ?? "", t)}
                          </p>
                          {isAuthError(testResult.error ?? "") && (
                            <p className="mt-1 text-text-muted">
                              {t("agw.ux.test_error_auth_hint")}
                            </p>
                          )}
                        </>
                      )}
                      <details className="mt-2">
                        <summary className="cursor-pointer text-text-muted">
                          {t("agw.ux.technical_details")}
                        </summary>
                        <pre className="surface-scroll mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-text-muted">
                          {redactSecrets(testResult.error ?? "")}
                        </pre>
                      </details>
                    </div>
                  )}

                  <div className="mt-auto flex flex-wrap items-center gap-2">
                    {agent.installed ? (
                      <button
                        className="btn-primary"
                        onClick={() => askTest(agent)}
                        disabled={busy !== null || !agent.available}
                      >
                        {busy === `test:${agent.id}` ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <CheckCircle2 className="h-3.5 w-3.5" />
                        )}
                        {t("agw.ux.test_connection")}
                      </button>
                    ) : (
                      <a
                        className="btn-primary"
                        href={
                          agent.id === "claude-code"
                            ? "https://docs.anthropic.com/en/docs/claude-code"
                            : "https://developers.openai.com/codex/cli/"
                        }
                        target="_blank"
                        rel="noreferrer"
                      >
                        {t("agw.ux.install_guide")}
                      </a>
                    )}
                    <button
                      className="btn-secondary"
                      onClick={() => editAgentPath(agent)}
                      disabled={busy !== null}
                    >
                      <Settings2 className="h-3.5 w-3.5" />
                      {t("agw.ux.edit_path")}
                    </button>
                    <button
                      className="btn-secondary"
                      onClick={() => toggleAgentDetails(agent.id)}
                    >
                      {detailsOpen ? t("agw.ux.hide_details") : t("agw.ux.details")}
                      <ChevronDown
                        className={`h-3.5 w-3.5 transition-transform ${detailsOpen ? "rotate-180" : ""}`}
                      />
                    </button>
                  </div>

                  {detailsOpen && (
                    <div className="space-y-2 border-t border-border pt-3">
                      <div className="divide-y divide-border/60">
                        <InfoRow label={t("agw.ux.version")} value={agent.version ?? t("agw.common.none")} mono />
                        <InfoRow label={t("agw.ux.executable")} value={agent.executable ?? t("agw.common.none")} mono />
                        <InfoRow label={t("agw.ux.auth")} value={agent.auth_detail || agent.auth_state} />
                        <InfoRow label={t("agw.ux.model")} value={agent.model ?? t("agw.common.none")} mono />
                        <InfoRow label={t("agw.ux.source")} value={agent.source} />
                        <InfoRow label={t("agw.ux.config")} value={agent.config_path ?? t("agw.common.none")} mono />
                        <InfoRow label={t("agw.ux.detection")} value={agent.detection} />
                      </div>
                      {agent.warnings.map((warning) => (
                        <p key={warning} className="text-xs leading-relaxed text-warning">
                          {warning}
                        </p>
                      ))}
                      {agent.error && (
                        <details>
                          <summary className="cursor-pointer text-xs text-text-muted">
                            {t("agw.ux.technical_details")}
                          </summary>
                          <pre className="surface-scroll mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-text-muted">
                            {redactSecrets(agent.error)}
                          </pre>
                        </details>
                      )}
                      {editingAgent === agent.id ? (
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
                      ) : null}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </section>

      <section className="space-y-3">
        <div>
          <h2 className="text-sm font-semibold text-text-primary">
            {t("agw.ux.step2_title")}
          </h2>
          <p className="mt-1 text-xs text-text-muted">{t("agw.ux.step2_desc")}</p>
        </div>
        <div className="grid gap-4 lg:grid-cols-2">
          {overview.clients.map((client) => {
            const detailsOpen = expandedClients.has(client.id);
            const hint =
              client.config_status === "configured"
                ? t("agw.ux.client.configured_hint")
                : client.config_status === "stale"
                  ? t("agw.ux.client.stale_hint")
                  : client.config_status === "conflict"
                    ? t("agw.ux.client.conflict_hint")
                    : client.config_status === "unavailable"
                      ? t("agw.ux.client.unavailable_hint")
                      : client.config_status === "not_installed"
                        ? t("agw.ux.client.not_installed_hint")
                        : t("agw.ux.client.not_configured_hint");
            return (
              <div key={client.id} className="surface-panel flex flex-col gap-3 p-4">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <h3 className="text-sm font-semibold text-text-primary">
                      {client.name}
                    </h3>
                    <p className="mt-1 text-xs text-text-secondary">{hint}</p>
                  </div>
                  <StatusBadge variant={clientVariant(client.config_status)}>
                    {t(`agw.connections.${client.config_status}`, client.config_status)}
                  </StatusBadge>
                </div>
                {client.requires_upgrade && (
                  <div className="rounded-lg border border-warning/25 bg-warning-soft/30 p-3 text-xs leading-relaxed text-text-secondary">
                    {t("agw.ux.client.old_codeleveler")}
                    <button
                      className="ml-1 underline decoration-dotted underline-offset-2"
                      onClick={() => toggleClientDetails(client.id)}
                    >
                      {t("agw.ux.view_upgrade")}
                    </button>
                  </div>
                )}
                {client.error && client.config_status !== "configured" && (
                  <p className="text-xs leading-relaxed text-text-muted">
                    {friendlyMessage(client.error, t)}
                  </p>
                )}
                <div className="mt-auto flex flex-wrap items-center gap-2">
                  {client.installed &&
                    client.config_status !== "conflict" &&
                    !client.matching && (
                      <button
                        className="btn-primary"
                        onClick={() => askConfigure(client)}
                        disabled={busy !== null || cliMissing}
                      >
                        <Wrench className="h-3.5 w-3.5" />
                        {client.configured
                          ? t("agw.ux.reconfigure")
                          : t("agw.connections.configure")}
                      </button>
                    )}
                  {client.managed && (
                    <button
                      className="btn-secondary"
                      onClick={() => askRemove(client)}
                      disabled={busy !== null || !client.installed}
                    >
                      {t("agw.connections.remove")}
                    </button>
                  )}
                  <button
                    className="btn-secondary"
                    onClick={() => toggleClientDetails(client.id)}
                  >
                    {detailsOpen ? t("agw.ux.hide_details") : t("agw.ux.details")}
                    <ChevronDown
                      className={`h-3.5 w-3.5 transition-transform ${detailsOpen ? "rotate-180" : ""}`}
                    />
                  </button>
                </div>
                {detailsOpen && (
                  <div className="space-y-2 border-t border-border pt-3">
                    <div className="divide-y divide-border/60">
                      <InfoRow label={t("agw.ux.version")} value={client.version ?? t("agw.common.none")} mono />
                      <InfoRow label={t("agw.ux.config")} value={client.config_path} mono />
                      <InfoRow label={t("agw.ux.command")} value={client.copy_command} mono />
                      <InfoRow label={t("agw.ux.args")} value={client.copy_args.join(" ") || t("agw.common.none")} mono />
                      <InfoRow label={t("agw.ux.compatibility")} value={client.compatibility} />
                    </div>
                    {client.warning && (
                      <p className="text-xs leading-relaxed text-warning">
                        {client.warning}
                      </p>
                    )}
                    {client.error && (
                      <details>
                        <summary className="cursor-pointer text-xs text-text-muted">
                          {t("agw.ux.technical_details")}
                        </summary>
                        <pre className="surface-scroll mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-text-muted">
                          {redactSecrets(client.error)}
                        </pre>
                      </details>
                    )}
                    <div className="flex flex-wrap gap-2">
                      <CopyAction
                        text={client.id === "codeleveler" ? client.copy_toml : client.copy_json}
                        label={client.id === "codeleveler" ? t("agw.connections.copy_toml") : t("agw.connections.copy_json")}
                      />
                      <CopyAction
                        text={
                          client.id === "pi"
                            ? "pi mcp list"
                            : "leveler mcp list"
                        }
                        label={t(
                          client.id === "pi"
                            ? "agw.ux.copy_verify_command"
                            : "agw.ux.copy_check_command"
                        )}
                      />
                    </div>
                    <p className="text-[11px] leading-relaxed text-text-muted">
                      {t(
                        client.id === "pi"
                          ? "agw.ux.client.verify_note"
                          : "agw.ux.client.leveler_verify_note"
                      )}
                    </p>
                    {client.requires_upgrade && (
                      <p className="text-[11px] leading-relaxed text-warning">
                        {t("agw.ux.client.old_codeleveler_detail")}
                      </p>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </section>

      <section className="space-y-3">
        <div>
          <h2 className="text-sm font-semibold text-text-primary">
            {t("agw.ux.step3_title")}
          </h2>
          <p className="mt-1 text-xs text-text-muted">{t("agw.ux.step3_desc")}</p>
        </div>
        <div className="surface-panel p-4">
          {usableAgents.length === 0 ? (
            <p className="text-sm text-text-secondary">{t("agw.ux.start_no_agent")}</p>
          ) : readyClients.length === 0 && staleClients.length > 0 ? (
            <p className="text-sm text-text-secondary">{t("agw.ux.start_stale_client")}</p>
          ) : readyClients.length === 0 ? (
            <p className="text-sm text-text-secondary">{t("agw.ux.start_no_client")}</p>
          ) : (
            <div className="space-y-3">
              <p className="text-sm text-text-secondary">
                {t("agw.ux.start_ready")}
              </p>
              <div className="flex flex-wrap gap-1.5">
                {overview.agents.map((agent) => {
                  const usable = usableAgents.some((item) => item.id === agent.id);
                  return (
                    <button
                      key={agent.id}
                      className={
                        activeExample === agent.id
                          ? "btn-primary !px-3 !py-1.5 text-xs"
                          : "btn-secondary !px-3 !py-1.5 text-xs"
                      }
                      disabled={!usable}
                      onClick={() => setExampleAgent(agent.id)}
                    >
                      {agent.name}
                    </button>
                  );
                })}
              </div>
              <pre className="whitespace-pre-wrap break-words rounded-lg border border-border bg-card-secondary p-3 text-xs leading-relaxed text-text-secondary">
                {exampleText}
              </pre>
              <div className="flex flex-wrap items-center gap-2">
                <CopyAction text={exampleText} label={t("agw.ux.copy_example")} />
                <span className="text-[11px] text-text-muted">
                  {t("agw.ux.start_note")}
                </span>
              </div>
              {overview.clients.some((client) => client.requires_upgrade) && (
                <p className="text-xs leading-relaxed text-warning">
                  {t("agw.ux.start_codeleveler_warning")}
                </p>
              )}
            </div>
          )}
        </div>
      </section>

      <section className="space-y-3">
        <button
          className="flex w-full items-center justify-between rounded-lg border border-border bg-card-secondary px-4 py-3 text-left text-sm font-medium text-text-primary"
          onClick={() => setAdvancedOpen((value) => !value)}
          aria-expanded={advancedOpen}
        >
          <span>{t("agw.ux.advanced_title")}</span>
          <ChevronDown
            className={`h-4 w-4 text-text-muted transition-transform ${advancedOpen ? "rotate-180" : ""}`}
          />
        </button>
        {advancedOpen && (
          <div className="space-y-4">
            <div className="surface-panel p-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <h3 className="text-sm font-semibold text-text-primary">
                    {t("agw.ux.service_title")}
                  </h3>
                  <p className="mt-1 text-xs text-text-muted">
                    {serviceHealthy
                      ? t("agw.ux.service_healthy")
                      : serviceError
                        ? friendlyMessage(serviceError, t)
                        : t("agw.ux.service_stopped")}
                  </p>
                </div>
                <StatusBadge
                  variant={
                    serviceHealthy ? "success" : serviceError ? "error" : "muted"
                  }
                >
                  {serviceHealthy
                    ? t("agw.service.running")
                    : serviceError
                      ? t("agw.service.error")
                      : t("agw.service.stopped")}
                </StatusBadge>
              </div>
              <div className="mt-3 divide-y divide-border/60">
                <InfoRow label={t("agw.ux.version")} value={service.service_version ?? t("agw.common.none")} mono />
                <InfoRow label={t("agw.ux.tasks")} value={service.running_tasks.length ? service.running_tasks.join(", ") : t("agw.service.tasks_none")} mono />
                <InfoRow label={t("agw.ux.data_dir")} value={service.data_dir} mono />
              </div>
              <label className="mt-3 flex items-center gap-2 text-xs text-text-secondary">
                <input
                  type="checkbox"
                  checked={manager?.agent_auto_start ?? true}
                  disabled={busy !== null}
                  onChange={(event) => setAgentAutoStart(event.target.checked)}
                />
                {t("agw.gateway.auto_start")}
              </label>
              <div className="mt-3 flex flex-wrap gap-2">
                <button className="btn-primary" onClick={startService} disabled={busy !== null || service.running || cliMissing}>
                  <Play className="h-3.5 w-3.5" />
                  {t("agw.service.start")}
                </button>
                <button className="btn-secondary" onClick={() => askService("stop")} disabled={busy !== null || !service.running}>
                  <Square className="h-3.5 w-3.5" />
                  {t("agw.service.stop")}
                </button>
                <button className="btn-secondary" onClick={() => askService("restart")} disabled={busy !== null || !service.running || cliMissing}>
                  {t("agw.service.restart")}
                </button>
              </div>
              {service.last_error && (
                <details className="mt-3">
                  <summary className="cursor-pointer text-xs text-text-muted">
                    {t("agw.ux.historical_error")}
                  </summary>
                  <pre className="surface-scroll mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-text-muted">
                    {redactSecrets(service.last_error)}
                  </pre>
                </details>
              )}
            </div>

            <div className="surface-panel p-4">
              <h3 className="text-sm font-semibold text-text-primary">
                {t("agw.ux.cli_title")}
              </h3>
              <p className="mt-1 text-xs text-text-muted">
                {overview.cli.path ?? t("agw.ux.cli_missing_short")}
              </p>
              {editingCli ? (
                <PathEditor
                  value={cliPath}
                  placeholder={t("agw.ux.cli_placeholder")}
                  saveLabel={t("agw.agent.save_path")}
                  resetLabel={t("agw.auto_detect")}
                  cancelLabel={t("agw.common.cancel")}
                  busy={busy === "cli"}
                  onChange={setCliPath}
                  onSave={saveCliPath}
                  onReset={() => {
                    setCliPath("");
                    setEditingCli(false);
                  }}
                  onCancel={() => setEditingCli(false)}
                />
              ) : (
                <button
                  className="btn-secondary mt-3"
                  onClick={() => {
                    setEditingCli(true);
                    setCliPath(overview.cli.path ?? "");
                  }}
                >
                  {t("agw.set_manual_path")}
                </button>
              )}
            </div>

            <div className="surface-panel p-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 className="text-sm font-semibold text-text-primary">
                  {t("agw.ux.diagnostics_title")}
                </h3>
                <button className="btn-secondary" onClick={copyDiagnostics} disabled={busy !== null}>
                  <Copy className="h-3.5 w-3.5" />
                  {t("agw.service.copy_diagnostics")}
                </button>
              </div>
              {diagnostics && (
                <pre className="surface-scroll mt-3 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-card-secondary p-3 font-mono text-[11px] text-text-muted">
                  {redactSecrets(diagnostics)}
                </pre>
              )}
            </div>

            <div className="surface-panel p-4">
              <h3 className="text-sm font-semibold text-text-primary">
                {t("agw.ux.custom_mcp_title")}
              </h3>
              <pre className="surface-scroll mt-2 max-h-48 overflow-auto rounded-lg bg-card-secondary p-3 font-mono text-[11px] text-text-secondary">
                {customJson}
              </pre>
              <div className="mt-2">
                <CopyAction text={customJson} label={t("agw.connections.copy_json")} />
              </div>
            </div>
          </div>
        )}
      </section>

      {confirm && (
        <ConfirmDialog
          open={true}
          title={confirm.title}
          message={confirm.message}
          confirmLabel={confirm.confirmLabel}
          variant={confirm.variant}
          onConfirm={async () => {
            const action = confirm;
            setConfirm(null);
            await action.run();
          }}
          onCancel={() => setConfirm(null)}
        />
      )}
    </div>
  );
}
