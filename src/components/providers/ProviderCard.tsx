import { useState, useEffect } from "react";
import {
  Cloud,
  Key,
  ExternalLink,
  Pencil,
  Trash2,
  Star,
  Loader2,
  Database,
  ChevronDown,
  ChevronUp,
} from "lucide-react";
import { StatusBadge } from "@/components/common/StatusBadge";
import { CapabilityIcons } from "@/components/common/CapabilityIcons";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";
import {
  getProviderCooldownSummary,
  getProviderSignalSummary,
  summarizeProviderErrorStatuses,
  type ProviderErrorStatus,
} from "@/lib/providerHealth";
import type { ProviderView } from "@/types/provider";
import type { ProviderHealth } from "@/types/stats";
import type { ProviderRuntimeStatus } from "@/types/route-profile";

interface ProviderCardProps {
  provider: ProviderView;
  onEdit: (provider: ProviderView) => void;
  onDelete: (provider: ProviderView) => void;
  onSetActive: (provider: ProviderView) => void;
  onTest: (provider: ProviderView) => void;
  onDetails?: (provider: ProviderView) => void;
  testing?: boolean;
  runtime?: ProviderRuntimeStatus;
  onResetRuntime?: (providerId: string) => void;
}

function humanizeProviderIssue(
  t: (key: string) => string,
  code?: string | null,
  message?: string | null
) {
  const raw = `${code ?? ""} ${message ?? ""}`.trim();
  if (!raw) return "";
  if (raw.includes("PASS_THROUGH_STREAM_FAILED")) {
    return t("providers.error_pass_through_stream_failed");
  }
  if (raw.includes("AUTH") || raw.includes("401") || raw.includes("403")) {
    return t("providers.error_auth_failed");
  }
  if (raw.includes("RATE_LIMIT") || raw.includes("429")) {
    return t("providers.error_rate_limited");
  }
  if (raw.includes("QUOTA") || raw.includes("INSUFFICIENT")) {
    return t("providers.error_quota");
  }
  return t("providers.error_request_failed");
}

export function ProviderCard({
  provider,
  onEdit,
  onDelete,
  onSetActive,
  onTest,
  onDetails,
  testing,
  runtime,
  onResetRuntime,
}: ProviderCardProps) {
  const { t } = useI18n();
  const [health, setHealth] = useState<ProviderHealth | null>(null);
  const [showDetails, setShowDetails] = useState(false);
  const keyCount = keyCountFromMasked(provider.masked_api_key);

  useEffect(() => {
    api
      .getProviderHealth(provider.name)
      .then(setHealth)
      .catch(() => {});
  }, [provider.name]);

  // 兼容提示：人话说明「出 400 时可以开哪些开关」。默认收在详情里，
  // 不顶在卡片第一行用工程师术语吓人（精炼层 / budget_tokens 等）。
  const compatTipKey = `providers.compat_tip.${provider.provider_type}`;
  const compatTip = t(compatTipKey);
  const hasCompatTip = compatTip !== compatTipKey;

  // ── status dot color ──
  const statusDotColor =
    provider.status === "connected"
      ? "bg-success"
      : provider.status === "failed"
        ? "bg-error"
        : "bg-text-muted";
  const statusLabel =
    provider.status === "connected"
      ? t("providers.status_connected")
      : provider.status === "failed"
        ? t("providers.status_failed")
        : t("providers.status_not_tested");

  // ── parsed protocol labels ──
  const protocolList: string[] = (() => {
    try {
      return JSON.parse(provider.protocol);
    } catch {
      return [provider.protocol];
    }
  })();
  const protocolLabels: string[] = (() => {
    const labels: Record<string, string> = {
      openai_chat_completions: "Chat Completions",
      openai_responses: "Responses",
      anthropic_messages: "Anthropic Messages",
    };
    return protocolList.map((p) => labels[p] || p);
  })();
  // 直连 chips：每个 protocol 表示上游原生支持的入口（=直连路径）。
  // 客户端若用 list 里没有的协议，网关会做协议转换。
  const passThroughChips: { key: string; label: string }[] = (() => {
    const shortLabel: Record<string, string> = {
      openai_chat_completions: "Chat",
      openai_responses: "Responses",
      anthropic_messages: "Anthropic",
    };
    return protocolList.map((p) => ({ key: p, label: shortLabel[p] || p }));
  })();

  // ── cache capability inference (anthropic-style only) ──
  const isAnthropicCapable =
    provider.provider_type === "anthropic" || !!provider.anthropic_base_url;
  const cacheEnabled =
    provider.supports_cache === true ||
    (provider.supports_cache == null &&
      provider.auto_cache_control !== false &&
      isAnthropicCapable);
  const cacheUnsupported =
    provider.supports_cache === false && isAnthropicCapable;

  // ── 故障自愈运行时状态 ──
  // 数据由父页周期刷新（usePolling）拉取并下传；只在「有问题」时亮出，
  // 平时卡片保持干净。cooldown 剩余秒为加载时刻快照，靠周期刷新更新。
  const cooldownSummary = getProviderCooldownSummary(runtime);
  const inCooldown = !!cooldownSummary;
  const h24Failures = health
    ? Math.max(health.h24_total - health.h24_success, 0)
    : 0;
  const latestError = health?.recent_errors[0];
  const errorStatuses = summarizeProviderErrorStatuses(
    health?.recent_errors ?? []
  );
  const signalSummary = getProviderSignalSummary(runtime, health);
  const recentFailure = humanizeProviderIssue(
    t,
    runtime?.last_error_code,
    runtime?.last_error ?? latestError?.message
  );
  const errorStatusVariant: Record<
    ProviderErrorStatus,
    "error" | "warning" | "muted"
  > = {
    rate_limited: "warning",
    quota_exhausted: "error",
    insufficient_balance: "error",
    auth_failed: "error",
    other_error: "muted",
  };

  return (
    <div
      className={`surface-panel flex min-w-0 flex-col p-4 ${provider.is_active ? "border-accent/50" : "border-border"}`}
    >
      {/* ── Header: icon + name + url ; status dot + capability icons ── */}
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent-soft">
            <Cloud className="h-4 w-4 text-accent" />
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h3 className="truncate text-sm font-semibold text-text-primary">
                {provider.name}
              </h3>
              {provider.is_active && (
                <StatusBadge variant="accent">
                  {t("providers.active")}
                </StatusBadge>
              )}
            </div>
            <p
              className="truncate font-mono text-xs text-text-muted"
              title={provider.base_url}
            >
              {provider.base_url}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <span className="flex items-center gap-1.5" title={statusLabel}>
            <span
              className={`inline-block h-2 w-2 rounded-full ${statusDotColor}`}
            />
          </span>
          <CapabilityIcons
            modelCapabilities={provider.model_capabilities}
            legacyVision={provider.supports_vision}
          />
          {cacheEnabled && (
            <Database
              className="h-3.5 w-3.5 text-accent"
              aria-label={t("providers.cache_enabled")}
            />
          )}
          {cacheUnsupported && (
            <Database
              className="h-3.5 w-3.5 text-text-muted/60"
              aria-label={t("providers.cache_not_supported")}
            />
          )}
        </div>
      </div>

      {/* ── Essentials: model · key · timeout · 直连 chips ── */}
      <div className="mb-3 border-y border-border/70 py-3">
        <div className="grid grid-cols-1 gap-2 text-xs sm:grid-cols-2">
          <div className="min-w-0">
            <span className="text-xs uppercase tracking-wide text-text-muted">
              {t("providers.default_model")}
            </span>
            <p className="truncate font-mono text-text-primary">
              {provider.default_model}
            </p>
          </div>
          <div className="min-w-0">
            <span className="text-xs uppercase tracking-wide text-text-muted">
              {t("providers.api_key")}
            </span>
            <p className="flex items-center gap-1 truncate font-mono text-xs text-text-secondary">
              <Key className="h-3 w-3 shrink-0" />
              {provider.masked_api_key ?? "—"}
              {keyCount > 1 && (
                <span className="ml-1 shrink-0 font-sans text-xs text-text-muted">
                  {t("providers.key_round_robin").replace(
                    "{n}",
                    String(keyCount)
                  )}
                </span>
              )}
            </p>
          </div>
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="text-xs text-text-muted">
            {provider.timeout_seconds}s
          </span>
          {passThroughChips.map((c) => (
            <span
              key={c.key}
              className="rounded bg-success/10 px-1.5 py-0.5 text-xs font-medium text-success"
              title={t("providers.pass_through_tooltip")}
            >
              {t("providers.pass_through_prefix")} {c.label}
            </span>
          ))}
        </div>
      </div>

      {/* ── Operational status: runtime + probe + real traffic in one vocabulary ── */}
      {(runtime || health) && (
        <div className="mb-3 space-y-2 text-xs text-text-muted">
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs font-semibold text-text-primary">
              {t("providers.card_health_status")}
            </span>
            {onResetRuntime &&
              runtime &&
              signalSummary.runtime.status !== "available" &&
              signalSummary.runtime.status !== "unknown" && (
                <button
                  onClick={() => onResetRuntime(provider.id)}
                  className="text-accent transition-colors hover:underline"
                >
                  {t("providers.runtime_reset")}
                </button>
              )}
          </div>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <StatusBadge
              variant={signalSummary.runtime.variant}
              className="px-1.5 py-0 text-xs"
            >
              {t(`providers.runtime_status.${signalSummary.runtime.status}`)}
              {inCooldown && cooldownSummary
                ? ` ${cooldownSummary.remainingSeconds}s`
                : ""}
            </StatusBadge>
            <StatusBadge
              variant={signalSummary.probe.variant}
              className="px-1.5 py-0 text-xs"
            >
              {t("providers.probe")} ·{" "}
              {t(`providers.probe_status.${signalSummary.probe.status}`)}
              {signalSummary.probe.latencyMs != null
                ? ` ${signalSummary.probe.latencyMs}ms`
                : ""}
            </StatusBadge>
            <StatusBadge
              variant={signalSummary.traffic.variant}
              className="px-1.5 py-0 text-xs"
            >
              {t("providers.traffic")} ·{" "}
              {t(`providers.traffic_status.${signalSummary.traffic.status}`)}
            </StatusBadge>
            {health && health.h24_total > 0 && (
              <>
                <span>1h {health.h1_success_rate}%</span>
                <span>24h {health.h24_success_rate}%</span>
                <span>{health.h1_avg_latency_ms}ms avg</span>
                <span>P95 {health.h1_p95_latency_ms}ms</span>
                <span>
                  {h24Failures} {t("providers.health_failures")}
                </span>
              </>
            )}
            {errorStatuses.map((status) => (
              <StatusBadge
                key={status}
                variant={errorStatusVariant[status]}
                className="px-1.5 py-0 text-xs"
              >
                {t(`providers.error_status.${status}`)}
              </StatusBadge>
            ))}
          </div>
          {cooldownSummary && (
            <div
              className="truncate"
              title={cooldownSummary.reason ?? undefined}
            >
              {t("providers.runtime_recovers_at")}{" "}
              {new Date(cooldownSummary.recoverAt).toLocaleTimeString([], {
                hour: "2-digit",
                minute: "2-digit",
                second: "2-digit",
              })}
              {cooldownSummary.reason
                ? ` · ${t("providers.runtime_reason")} ${cooldownSummary.reason}`
                : ""}
            </div>
          )}
          {runtime?.consecutive_failures ? (
            <div>
              {t("providers.runtime_failures")} {runtime.consecutive_failures}
            </div>
          ) : null}
          {signalSummary.probe.error && (
            <div className="truncate" title={signalSummary.probe.error}>
              {t("providers.probe")}: {signalSummary.probe.error}
            </div>
          )}
          {recentFailure && (
            <div className="truncate">
              {t("providers.recent_failure")}: {recentFailure}
            </div>
          )}
          {latestError && (
            <div
              className="truncate text-xs text-text-muted"
              title={`${latestError.status_code} ${latestError.message}`}
            >
              {t("providers.health_recent_errors")}: {latestError.status_code} ·{" "}
              {latestError.message}
            </div>
          )}
        </div>
      )}

      {/* ── Collapsible details ── */}
      {showDetails && (
        <div className="mb-3 grid grid-cols-2 gap-y-2 rounded-md bg-card-secondary/50 p-3 text-xs">
          <div>
            <span className="text-text-muted">{t("providers.type")}</span>
            <p className="text-text-primary">{provider.provider_type}</p>
          </div>
          <div>
            <span className="text-text-muted">{t("providers.protocol")}</span>
            <p className="flex flex-wrap gap-1">
              {protocolLabels.map((p) => (
                <span
                  key={p}
                  className="rounded bg-card-secondary px-1.5 py-0.5 text-xs text-text-primary"
                >
                  {p}
                </span>
              ))}
            </p>
          </div>
          {provider.reasoning_model && (
            <div className="col-span-2">
              <span className="text-text-muted">
                {t("providers.reasoning_model")}
              </span>
              <p className="font-mono text-text-primary">
                {provider.reasoning_model}
              </p>
            </div>
          )}
          {hasCompatTip && (
            <div className="col-span-2 rounded-md border border-border/60 bg-card-secondary/50 px-2.5 py-2">
              <p className="text-xs font-medium text-text-secondary">
                {t("providers.compat_tip_title")}
              </p>
              <p className="mt-1 text-xs leading-relaxed text-text-muted">
                {compatTip}
              </p>
            </div>
          )}
        </div>
      )}

      {/* ── Actions ── */}
      <div className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-3">
        <div className="flex flex-wrap gap-1.5">
          <button
            onClick={() => onTest(provider)}
            disabled={testing}
            className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90 disabled:opacity-50"
          >
            {testing ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <ExternalLink className="h-3 w-3" />
            )}
            {t("providers.test")}
          </button>
          <button
            onClick={() => onEdit(provider)}
            className="flex items-center gap-1.5 rounded-md bg-card-secondary px-2.5 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary"
          >
            <Pencil className="h-3 w-3" />
            {t("common.edit")}
          </button>
          {onDetails && (
            <button
              onClick={() => onDetails(provider)}
              className="flex items-center gap-1.5 rounded-md bg-card-secondary px-2.5 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary"
            >
              <ExternalLink className="h-3 w-3" />
              {t("common.details")}
            </button>
          )}
          {!provider.is_active && (
            <button
              onClick={() => onSetActive(provider)}
              className="flex items-center gap-1.5 rounded-md bg-card-secondary px-2.5 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary"
            >
              <Star className="h-3 w-3" />
              {t("providers.set_active")}
            </button>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button
            onClick={() => setShowDetails((v) => !v)}
            aria-expanded={showDetails}
            className="flex items-center gap-1 text-xs text-text-muted transition-colors hover:text-text-primary"
          >
            {showDetails ? (
              <ChevronUp className="h-3 w-3" />
            ) : (
              <ChevronDown className="h-3 w-3" />
            )}
            {t("providers.configuration_details")}
          </button>
          {showDetails && (
            <button
              onClick={() => onDelete(provider)}
              className="flex items-center gap-1.5 rounded-md bg-card-secondary px-2 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-error/20 hover:text-error"
            >
              <Trash2 className="h-3 w-3" />
              {t("common.delete")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** Backend masks JSON key arrays as `sk-ab****cdef (+2 more)`. */
export function keyCountFromMasked(masked?: string | null): number {
  if (!masked) return 0;
  const extra = masked.match(/\(\+(\d+) more\)/i);
  if (extra) return Number(extra[1]) + 1;
  return 1;
}
