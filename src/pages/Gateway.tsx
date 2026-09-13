import { useState, useEffect, useCallback } from "react";
import {
  Radio,
  Play,
  Square,
  RotateCcw,
  Settings,
  Save,
  AlertTriangle,
  Copy,
  Link2,
} from "lucide-react";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";
import { useGatewaySettings } from "@/store/global";
import { PROTOCOLS } from "@/types/provider";
import type { GatewayStatus } from "@/types/gateway";

export function Gateway() {
  const { t } = useI18n();
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  // settings 走全局 store——Settings 页 / 其它 page 也读这份;mutation 后
  // 这页 handleSave 内显式 refetch 同步给 store。
  const settings = useGatewaySettings((s) => s.value);

  // Editable fields
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [inputProtocol, setInputProtocol] = useState("");
  const [outputProtocol, setOutputProtocol] = useState("");
  const [autoStart, setAutoStart] = useState(false);
  const [logRetention, setLogRetention] = useState("");
  const [dirty, setDirty] = useState(false);

  const load = useCallback(async () => {
    try {
      await Promise.all([
        api.getGatewayStatus().then(setStatus),
        useGatewaySettings.getState().refetch(),
      ]);
      const g = useGatewaySettings.getState().value;
      if (g) {
        setHost(g.host);
        setPort(String(g.port));
        setInputProtocol(g.input_protocol);
        setOutputProtocol(g.output_protocol);
        setAutoStart(g.auto_start);
        setLogRetention(String(g.log_retention_days));
        setDirty(false);
      }
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const markDirty = () => setDirty(true);

  const handleSave = async () => {
    const portNum = parseInt(port, 10);
    const retentionNum = parseInt(logRetention, 10);
    if (isNaN(portNum) || portNum < 1 || portNum > 65535) {
      toast("error", t("gateway.invalid_port"));
      return;
    }
    if (isNaN(retentionNum) || retentionNum < 1) {
      toast("error", t("gateway.invalid_retention"));
      return;
    }
    try {
      await api.updateGatewaySettings({
        host,
        port: portNum,
        input_protocol: inputProtocol,
        output_protocol: outputProtocol,
        auto_start: autoStart,
        log_retention_days: retentionNum,
      });
      toast("success", t("gateway.settings_saved"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleStart = async () => {
    try {
      const s = await api.startGateway();
      setStatus(s);
      toast("success", t("gateway.started"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleStop = async () => {
    try {
      const s = await api.stopGateway();
      setStatus(s);
      toast("success", t("gateway.stopped"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleRestart = async () => {
    try {
      const s = await api.restartGateway();
      setStatus(s);
      toast("success", t("gateway.restarted"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  if (!status || !settings) {
    return <p className="text-xs text-text-muted">{t("common.loading")}</p>;
  }

  const startedAt = status.started_at
    ? new Date(status.started_at).toLocaleTimeString()
    : null;
  const listenUrl = `http://${status.host}:${status.port}`;
  const chatUrl = `${listenUrl}/v1/chat/completions`;
  const responsesUrl = `${listenUrl}/v1/responses`;
  const messagesUrl = `${listenUrl}/v1/messages`;

  const copyText = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast("success", t("gateway.copied"));
    } catch {
      toast("error", text);
    }
  };

  return (
    <div className="desktop-page">
      {/* ── 1. Status strip first — primary ops (run/stop/restart). ── */}
      <div
        className="desktop-page-header command-strip relative overflow-hidden border-accent/25"
        style={{
          background:
            "linear-gradient(135deg, var(--color-card) 0%, var(--color-accent-soft) 100%)",
        }}
      >
        <div className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-accent/50 to-transparent" />
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex min-w-0 items-center gap-3">
            <div className="relative flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent-soft">
              <span
                className={`absolute -right-0.5 -top-0.5 h-2.5 w-2.5 rounded-full ${status.running ? "bg-info animate-pulse-dot" : "bg-text-muted/50"}`}
              />
              <Radio className="h-4 w-4 text-accent" />
            </div>
            <div className="min-w-0">
              <p className="text-xs font-semibold uppercase tracking-[0.16em] text-accent">
                {t("gateway.service_console")}
              </p>
              <div className="mt-1 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs">
                <span className="font-medium text-text-primary">
                  {t("gateway.local_gateway")}
                </span>
                <a
                  href={listenUrl}
                  className="font-mono text-accent hover:underline"
                  target="_blank"
                  rel="noreferrer"
                >
                  {listenUrl}
                </a>
                <span className="text-text-muted/40">·</span>
                <span className="text-text-secondary">
                  <span className="text-text-muted">
                    {t("gateway.active_provider")}{" "}
                  </span>
                  <span className="text-text-primary">
                    {status.active_provider ?? t("common.none")}
                  </span>
                </span>
                {startedAt && (
                  <>
                    <span className="text-text-muted/40">·</span>
                    <span className="text-text-secondary">
                      <span className="text-text-muted">
                        {t("gateway.started_at")}{" "}
                      </span>
                      <span className="font-mono text-text-primary">
                        {startedAt}
                      </span>
                    </span>
                  </>
                )}
              </div>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {status.running ? (
              <>
                <button
                  onClick={handleStop}
                  className="flex items-center gap-1.5 rounded-md bg-error-soft px-2.5 py-1 text-xs font-medium text-error transition-colors hover:bg-error/20"
                >
                  <Square className="h-3 w-3" />
                  {t("gateway.stop")}
                </button>
                <button
                  onClick={handleRestart}
                  className="flex items-center gap-1.5 rounded-md bg-warning-soft px-2.5 py-1 text-xs font-medium text-warning transition-colors hover:bg-warning/20"
                >
                  <RotateCcw className="h-3 w-3" />
                  {t("gateway.restart")}
                </button>
              </>
            ) : (
              <button
                onClick={handleStart}
                className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90"
              >
                <Play className="h-3 w-3" />
                {t("gateway.start")}
              </button>
            )}
          </div>
        </div>
        {dirty && (
          <p className="mt-2 text-xs text-warning">
            {t("gateway.settings_changed")}
          </p>
        )}
      </div>

      {/* Connection info below — copy endpoints after gateway is up. */}
      <div className="surface-panel overflow-hidden">
        <div className="flex items-start gap-3 border-b border-border px-5 py-4">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent-soft">
            <Link2 className="h-4 w-4 text-accent" />
          </div>
          <div className="min-w-0 flex-1">
            <h3 className="text-sm font-semibold text-text-primary">
              {t("gateway.connection_info")}
            </h3>
            <p className="mt-0.5 text-xs text-text-muted">
              {t("gateway.connection_info_hint")}
            </p>
          </div>
        </div>
        <div className="divide-y divide-border">
          {(
            [
              {
                label: t("gateway.base_url"),
                value: `${listenUrl}/v1`,
                primary: true,
              },
              {
                label: t("gateway.base_url_chat"),
                value: chatUrl,
                primary: false,
              },
              {
                label: t("gateway.base_url_responses"),
                value: responsesUrl,
                primary: false,
              },
              {
                label: t("gateway.base_url_messages"),
                value: messagesUrl,
                primary: false,
              },
            ] as const
          ).map((row) => (
            <div
              key={row.value}
              className="flex items-center gap-3 px-5 py-2.5 hover:bg-hover/40"
            >
              <div className="min-w-0 flex-1">
                <p
                  className={`text-xs font-medium uppercase tracking-wide ${
                    row.primary ? "text-accent" : "text-text-muted"
                  }`}
                >
                  {row.label}
                </p>
                <p
                  className="mt-0.5 truncate font-mono text-[12px] text-text-primary"
                  title={row.value}
                >
                  {row.value}
                </p>
              </div>
              <button
                type="button"
                onClick={() => copyText(row.value)}
                className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-card-secondary px-2.5 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:border-accent/40 hover:text-accent"
                title={t("common.copy")}
              >
                <Copy className="h-3 w-3" />
                {t("common.copy")}
              </button>
            </div>
          ))}
        </div>
      </div>

      {/* 已保存的设置 != 运行中的设置时显示重启 banner——避免用户改了 port
          但 gateway 还在用旧 port 听，请求莫名连不上。一键重启把保存的设置
          应用到运行实例。 */}
      {status.running &&
        settings &&
        (settings.host !== status.host ||
          settings.port !== status.port ||
          settings.input_protocol !== status.input_protocol ||
          settings.output_protocol !== status.output_protocol) && (
          <div className="rounded-lg border border-warning/30 bg-warning-soft px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <div className="flex items-start gap-2.5">
                <AlertTriangle className="h-4 w-4 shrink-0 text-warning mt-0.5" />
                <div>
                  <p className="text-xs font-medium text-text-primary">
                    {t("gateway.restart_required_title")}
                  </p>
                  <p className="mt-0.5 text-xs text-text-secondary">
                    {t("gateway.restart_required_desc")}
                  </p>
                  <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-mono">
                    {settings.host !== status.host && (
                      <span>
                        host:{" "}
                        <span className="text-text-muted line-through">
                          {status.host}
                        </span>{" "}
                        → <span className="text-warning">{settings.host}</span>
                      </span>
                    )}
                    {settings.port !== status.port && (
                      <span>
                        port:{" "}
                        <span className="text-text-muted line-through">
                          {status.port}
                        </span>{" "}
                        → <span className="text-warning">{settings.port}</span>
                      </span>
                    )}
                    {settings.input_protocol !== status.input_protocol && (
                      <span>
                        input:{" "}
                        <span className="text-warning">
                          {settings.input_protocol}
                        </span>
                      </span>
                    )}
                    {settings.output_protocol !== status.output_protocol && (
                      <span>
                        output:{" "}
                        <span className="text-warning">
                          {settings.output_protocol}
                        </span>
                      </span>
                    )}
                  </div>
                </div>
              </div>
              <button
                onClick={handleRestart}
                className="shrink-0 flex items-center gap-1.5 rounded-md bg-warning-soft px-3 py-1.5 text-xs font-medium text-warning transition-colors hover:bg-warning/20"
              >
                <RotateCcw className="h-3 w-3" />
                {t("gateway.restart_now")}
              </button>
            </div>
          </div>
        )}

      {/* ── 2. Configuration — the editable settings ── */}
      <div className="surface-panel p-5">
        <div className="mb-4 flex items-center justify-between">
          <h3 className="flex items-center gap-2 text-sm font-semibold text-text-primary">
            <Settings className="h-4 w-4 text-text-muted" />
            {t("gateway.configuration")}
          </h3>
          <button
            onClick={handleSave}
            disabled={!dirty}
            className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-accent"
          >
            <Save className="h-3.5 w-3.5" />
            {t("gateway.save")}
          </button>
        </div>

        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <SettingsField label={t("gateway.listen_address")}>
            <input
              value={host}
              onChange={(e) => {
                setHost(e.target.value);
                markDirty();
              }}
              className="form-input"
            />
          </SettingsField>
          <SettingsField label={t("gateway.port")}>
            <input
              type="number"
              value={port}
              onChange={(e) => {
                setPort(e.target.value);
                markDirty();
              }}
              className="form-input"
            />
          </SettingsField>
          <SettingsField label={t("gateway.log_retention")}>
            <input
              type="number"
              value={logRetention}
              onChange={(e) => {
                setLogRetention(e.target.value);
                markDirty();
              }}
              min={1}
              className="form-input"
            />
          </SettingsField>
          <SettingsField label={t("gateway.input_protocol")}>
            <select
              value={inputProtocol}
              onChange={(e) => {
                setInputProtocol(e.target.value);
                markDirty();
              }}
              className="form-input"
            >
              {PROTOCOLS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </SettingsField>
          <SettingsField label={t("gateway.output_protocol")}>
            <select
              value={outputProtocol}
              onChange={(e) => {
                setOutputProtocol(e.target.value);
                markDirty();
              }}
              className="form-input"
            >
              {PROTOCOLS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </SettingsField>
          <SettingsField label={t("gateway.auto_start")}>
            <label className="mt-1 flex cursor-pointer items-center gap-2">
              <input
                type="checkbox"
                checked={autoStart}
                onChange={(e) => {
                  setAutoStart(e.target.checked);
                  markDirty();
                }}
                className="accent-accent"
              />
              <span className="text-xs text-text-secondary">
                {autoStart ? t("providers.enabled") : t("providers.disabled")}
              </span>
            </label>
          </SettingsField>
        </div>
      </div>

      {/* ── 3. Route reference — what the gateway exposes ── */}
      <div className="surface-panel p-5">
        <h3 className="mb-3 flex items-center gap-2 text-sm font-semibold text-text-primary">
          <span className="h-2 w-2 rounded-full bg-accent" />
          {t("gateway.route_matrix")}
        </h3>
        <div className="grid grid-cols-1 gap-1.5 lg:grid-cols-2">
          {ROUTE_REFERENCE.map((r) => (
            <div
              key={r.path}
              className="flex items-center justify-between rounded-md border border-border/50 bg-card-secondary px-3 py-1.5 text-xs"
            >
              <div className="flex min-w-0 items-center gap-2">
                <span className="w-10 shrink-0 rounded bg-bg px-1.5 py-0.5 text-center font-mono text-xs text-text-muted">
                  {r.method}
                </span>
                <span className="truncate font-mono text-text-primary">
                  {r.path}
                </span>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {r.detail && (
                  <span className="hidden text-xs text-text-muted lg:inline">
                    {r.detail}
                  </span>
                )}
                <span
                  className={`rounded-full px-2 py-0.5 text-xs font-medium ${
                    r.mode === "pass-through"
                      ? "bg-accent-soft text-accent"
                      : r.mode === "transform"
                        ? "bg-warning-soft text-warning"
                        : "bg-hover text-text-muted"
                  }`}
                >
                  {r.mode}
                </span>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

const ROUTE_REFERENCE: {
  method: string;
  path: string;
  mode: string;
  detail?: string;
}[] = [
  { method: "GET", path: "/health", mode: "internal" },
  { method: "GET", path: "/v1/models", mode: "internal" },
  {
    method: "POST",
    path: "/v1/responses",
    mode: "transform",
    detail: "Responses → Chat Completions",
  },
  { method: "POST", path: "/responses", mode: "transform", detail: "alias" },
  {
    method: "POST",
    path: "/v1/chat/completions",
    mode: "pass-through",
    detail: "Chat Completions → Chat Completions",
  },
  {
    method: "POST",
    path: "/chat/completions",
    mode: "pass-through",
    detail: "alias",
  },
  {
    method: "POST",
    path: "/v1/messages",
    mode: "transform",
    detail: "Anthropic Messages → Chat Completions",
  },
  {
    method: "POST",
    path: "/v1beta/models/{model}:generateContent",
    mode: "transform",
    detail: "Gemini → Chat Completions",
  },
];

function SettingsField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <label className="mb-1 block text-xs font-medium text-text-secondary">
        {label}
      </label>
      {children}
    </div>
  );
}
