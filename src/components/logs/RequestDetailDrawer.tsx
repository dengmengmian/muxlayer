import { useState } from "react";
import {
  Download,
  Info,
  MessageSquare,
  Circle,
  CheckCircle2,
  XCircle,
  AlertTriangle,
} from "lucide-react";
import { DetailDrawer } from "@/components/layout/DetailDrawer";
import { ConversationModal } from "@/components/logs/ConversationModal";
import { JsonCodeBlock } from "@/components/common/JsonCodeBlock";
import { ErrorExplanationCard } from "@/components/common/ErrorExplanationCard";
import { StatusBadge } from "@/components/common/StatusBadge";
import { toast } from "@/components/common/Toast";
import {
  formatCost,
  formatTimestamp,
  formatOptionalLatency,
} from "@/lib/utils";
import { useI18n } from "@/lib/i18n";
import { sourceLabel } from "@/components/logs/RequestLogTable";
import { buildReproPackage, pairBodies } from "@/lib/requestLogDebug";
import {
  buildStreamFailureTimeline,
  shouldShowFailureTimeline,
  type TimelineStatus,
  type TimelineStep,
} from "@/lib/streamFailureTimeline";
import type { RequestLogDetail } from "@/types/request-log";

interface RouteDecisionTrace {
  profile_name?: string;
  mode?: string;
  selected_provider_name?: string;
  selected_model?: string;
  matched_conditions?: Record<string, unknown> | null;
  candidates?: Array<{
    provider_name?: string;
    priority?: number;
    model?: string | null;
    in_cooldown?: boolean;
    has_conditions?: boolean;
    skip_reasons?: string[];
  }>;
  fallback_chain?: Array<{
    provider_name?: string;
    role?: "primary" | "fallback";
    step?: number;
    selected?: boolean;
  }>;
}

interface RequestTrace {
  route_decision?: RouteDecisionTrace;
  error_mapper?: {
    upstream_code?: string | null;
    upstream_message?: string | null;
    mapped_code?: string;
    mapped_message?: string;
  };
  circuit_breaker?: {
    observed_state?: string;
    transition?: string | null;
    provider_id?: string;
  };
  degradation?: {
    requested_model?: string;
    chain?: string[];
    picked?: string | null;
    reason?: string;
  };
}

interface RequestDetailDrawerProps {
  request: RequestLogDetail | null;
  onClose: () => void;
}

export function RequestDetailDrawer({
  request,
  onClose,
}: RequestDetailDrawerProps) {
  const { t } = useI18n();
  const [convoOpen, setConvoOpen] = useState(false);
  const [bodyTab, setBodyTab] = useState<"request" | "response">("request");
  const [diffMode, setDiffMode] = useState<"side" | "raw" | "converted">(
    "side"
  );
  const [includeBodiesInExport, setIncludeBodiesInExport] = useState(true);

  if (!request) return null;

  const isError =
    request.status_code !== null &&
    (request.status_code >= 400 || request.status_code < 200);
  const trace = parseTrace(request.trace_json);
  const routeDecision = trace?.route_decision ?? null;
  const totalTokens =
    (request.input_tokens ?? 0) + (request.output_tokens ?? 0);
  const bodyPairs = pairBodies(request);
  const activePair =
    bodyPairs.find((p) => p.label === bodyTab) ?? bodyPairs[0] ?? null;

  const handleExportRepro = async () => {
    const pkg = buildReproPackage({
      request_id: request.request_id,
      timestamp: request.timestamp,
      client: request.client,
      provider: request.provider,
      model: request.model,
      route: request.route,
      status_code: request.status_code,
      latency_ms: request.latency_ms,
      error_message: request.error_message,
      trace_json: request.trace_json,
      raw_request: request.raw_request,
      converted_request: request.converted_request,
      raw_response: request.raw_response,
      converted_response: request.converted_response,
      app_version: "1.6.2",
      include_bodies: includeBodiesInExport,
    });
    try {
      await navigator.clipboard.writeText(pkg);
      toast("success", t("logs.export_repro_copied"));
    } catch {
      // Fallback download if clipboard blocked
      const blob = new Blob([pkg], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `agentgate-repro-${request.request_id}.json`;
      a.click();
      URL.revokeObjectURL(url);
      toast("success", t("logs.export_repro_downloaded"));
    }
  };

  return (
    <DetailDrawer
      open={!!request}
      onClose={onClose}
      title={`${t("common.details")} ${request.request_id}`}
    >
      <div className="space-y-5">
        {/* 非 gateway 来源：raw_request / SSE / tool_calls 等大部分字段为 NULL，
            给个 banner 解释为啥下面一堆字段是空的。 */}
        {request.source && request.source !== "gateway" && (
          <div className="flex items-start gap-2 rounded-md border border-warning/30 bg-warning/10 p-3">
            <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-warning" />
            <div className="text-[11px] leading-relaxed text-text-secondary">
              <span className="font-medium text-text-primary">
                {sourceLabel(request.source, t)} {t("logs.client_log_entry")}
              </span>
              {t("logs.client_log_banner")}
            </div>
          </div>
        )}

        {/* 7.4 日志→会话:有 session_id 就给一个入口直接看整段会话对话 */}
        {request.session_id && (
          <div className="flex items-center justify-between gap-3 rounded-md border border-border bg-card-secondary px-3 py-2">
            <div className="min-w-0">
              <span className="text-[11px] text-text-muted">
                {t("logs.belongs_to_session")}
              </span>
              <p
                className="truncate font-mono text-[11px] text-text-primary"
                title={request.session_id}
              >
                {request.session_id}
              </p>
            </div>
            <button
              onClick={() => setConvoOpen(true)}
              className="flex shrink-0 items-center gap-1 rounded-md border border-border px-2 py-1 text-[11px] text-text-secondary transition-colors hover:text-accent"
            >
              <MessageSquare className="h-3.5 w-3.5" />{" "}
              {t("logs.view_session_convo")}
            </button>
          </div>
        )}
        {convoOpen && request.session_id && (
          <ConversationModal
            sessionId={request.session_id}
            source={request.source ?? ""}
            onClose={() => setConvoOpen(false)}
          />
        )}

        <RouteCostSummary
          request={request}
          trace={trace}
          isError={isError}
          totalTokens={totalTokens}
        />

        {request.error_message && (
          <ErrorExplanationCard
            statusCode={request.status_code ?? 0}
            message={request.error_message}
          />
        )}

        {/* A2.3: 流式/请求失败时间轴 — 开始→选路→上游→last SSE→failover→结束 */}
        {shouldShowFailureTimeline(request) && (
          <StreamFailureTimelineCard request={request} />
        )}

        {isError && <ErrorChainCard request={request} trace={trace} />}

        {routeDecision && <RouteDecisionCard decision={routeDecision} />}

        {/* A2: export always available (route decision / error / version even without bodies). */}
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border bg-card-secondary px-3 py-2">
          <span className="text-[11px] text-text-muted">
            {t("logs.export_repro")}
          </span>
          <div className="flex flex-wrap items-center gap-2">
            <label className="flex items-center gap-1 text-[10px] text-text-muted">
              <input
                type="checkbox"
                checked={includeBodiesInExport}
                onChange={(e) => setIncludeBodiesInExport(e.target.checked)}
              />
              {t("logs.export_include_bodies")}
            </label>
            <button
              type="button"
              onClick={handleExportRepro}
              className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-[11px] text-text-secondary hover:text-accent"
            >
              <Download className="h-3 w-3" />
              {t("logs.export_repro")}
            </button>
          </div>
        </div>

        {/* A2: raw vs converted Diff when bodies exist */}
        {bodyPairs.length > 0 && (
          <div className="space-y-2 rounded-lg border border-border bg-card-secondary p-3">
            <h4 className="text-xs font-semibold text-text-primary">
              {t("logs.body_diff")}
            </h4>
            <div className="flex flex-wrap gap-1">
              {bodyPairs.map((p) => (
                <button
                  key={p.label}
                  type="button"
                  onClick={() => setBodyTab(p.label as "request" | "response")}
                  className={`rounded px-2 py-0.5 text-[11px] ${
                    activePair?.label === p.label
                      ? "bg-accent/15 text-accent"
                      : "text-text-muted hover:bg-hover"
                  }`}
                >
                  {p.label === "request"
                    ? t("logs.diff_request")
                    : t("logs.diff_response")}
                </button>
              ))}
              {(["side", "raw", "converted"] as const).map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => setDiffMode(m)}
                  className={`rounded px-2 py-0.5 text-[11px] ${
                    diffMode === m
                      ? "bg-card text-text-primary"
                      : "text-text-muted hover:bg-hover"
                  }`}
                >
                  {m === "side"
                    ? t("logs.diff_side")
                    : m === "raw"
                      ? t("logs.raw")
                      : t("logs.converted")}
                </button>
              ))}
            </div>
            {activePair && diffMode === "side" && (
              <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
                <JsonCodeBlock
                  title={t("logs.raw")}
                  content={activePair.raw ?? t("logs.body_empty")}
                />
                <JsonCodeBlock
                  title={t("logs.converted")}
                  content={activePair.converted ?? t("logs.body_empty")}
                />
              </div>
            )}
            {activePair && diffMode === "raw" && (
              <JsonCodeBlock
                title={t("logs.raw")}
                content={activePair.raw ?? t("logs.body_empty")}
              />
            )}
            {activePair && diffMode === "converted" && (
              <JsonCodeBlock
                title={t("logs.converted")}
                content={activePair.converted ?? t("logs.body_empty")}
              />
            )}
          </div>
        )}

        {request.tool_calls && (
          <JsonCodeBlock
            title={t("logs.tool_calls")}
            content={request.tool_calls}
          />
        )}
        {request.trace_json && (
          <JsonCodeBlock title={t("logs.trace")} content={request.trace_json} />
        )}
      </div>
    </DetailDrawer>
  );
}

function parseTrace(traceJson: string | null): RequestTrace | null {
  if (!traceJson) return null;
  try {
    return JSON.parse(traceJson) as RequestTrace;
  } catch {
    return null;
  }
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <span className="text-text-muted">{label}</span>
      <p className="font-mono text-text-primary">{value}</p>
    </div>
  );
}

function RouteCostSummary({
  request,
  trace,
  isError,
  totalTokens,
}: {
  request: RequestLogDetail;
  trace: RequestTrace | null;
  isError: boolean;
  totalTokens: number;
}) {
  const { t } = useI18n();
  const decision = trace?.route_decision;
  const mapper = trace?.error_mapper;
  const breaker = trace?.circuit_breaker;
  const degradation = trace?.degradation;
  const fallbackChain = decision?.fallback_chain ?? [];
  const selectedProvider =
    decision?.selected_provider_name ?? request.provider ?? "—";
  const selectedModel = decision?.selected_model ?? request.model ?? "—";

  return (
    <div className="rounded-lg border border-border bg-card-secondary p-4">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div>
          <h4 className="text-xs font-semibold text-text-primary">
            {t("logs.route_and_cost")}
          </h4>
          <p className="mt-1 text-[11px] text-text-muted">
            {formatTimestamp(request.timestamp)} · {request.client ?? "—"}
          </p>
        </div>
        <StatusBadge variant={isError ? "error" : "success"}>
          {request.status_code ?? "—"}
        </StatusBadge>
      </div>

      <div className="grid grid-cols-2 gap-3 text-xs">
        <Metric
          label={t("logs.route")}
          value={decision?.profile_name ?? request.route ?? "—"}
        />
        <Metric label={t("logs.provider")} value={selectedProvider} />
        <Metric label={t("logs.model")} value={selectedModel} />
        <Metric
          label={t("logs.latency")}
          value={formatOptionalLatency(request.latency_ms)}
        />
        <Metric
          label={t("logs.tokens_input")}
          value={request.input_tokens?.toLocaleString() ?? "—"}
        />
        <Metric
          label={t("logs.tokens_output")}
          value={request.output_tokens?.toLocaleString() ?? "—"}
        />
        <Metric
          label={t("logs.tokens_total")}
          value={totalTokens > 0 ? totalTokens.toLocaleString() : "—"}
        />
        <Metric label={t("logs.cost")} value={formatCost(request.cost)} />
      </div>

      <div className="mt-3 grid grid-cols-2 gap-3 text-xs">
        <Metric
          label={t("logs.cache_write")}
          value={request.cache_write_tokens?.toLocaleString() ?? "—"}
        />
        <Metric
          label={t("logs.cache_read")}
          value={request.cache_read_tokens?.toLocaleString() ?? "—"}
        />
      </div>

      {decision?.matched_conditions &&
        Object.keys(decision.matched_conditions).length > 0 && (
          <div className="mt-3 rounded-md border border-border bg-card px-3 py-2 text-xs">
            <span className="text-text-muted">
              {t("logs.matched_conditions")}
            </span>
            <p className="mt-1 text-text-primary">
              {formatConditions(decision.matched_conditions)}
            </p>
          </div>
        )}

      {fallbackChain.length > 0 && (
        <div className="mt-3 space-y-1">
          <span className="text-[11px] text-text-muted">
            {t("logs.fallback_chain")}
          </span>
          <div className="flex flex-wrap gap-1.5">
            {fallbackChain.map((step, idx) => (
              <span
                key={`${step.provider_name ?? "fallback"}-${idx}`}
                className={`rounded-md border px-2 py-1 text-[11px] ${
                  step.selected
                    ? "border-accent/40 bg-accent/10 text-accent"
                    : "border-border bg-card text-text-secondary"
                }`}
              >
                {step.step ?? idx + 1}.{" "}
                {step.role === "primary"
                  ? t("logs.fallback_primary")
                  : t("logs.fallback_backup")}{" "}
                · {step.provider_name ?? "—"}
              </span>
            ))}
          </div>
        </div>
      )}

      {(isError || mapper || breaker || degradation) && (
        <div className="mt-3 space-y-2 rounded-md border border-error/20 bg-error/5 px-3 py-2 text-xs">
          <div>
            <span className="text-text-muted">{t("logs.error_final")}</span>
            <p className="mt-1 text-text-primary">
              HTTP {request.status_code ?? "—"}
              {request.error_message ? ` · ${request.error_message}` : ""}
            </p>
          </div>
          {mapper && (
            <p className="text-text-secondary">
              {t("logs.error_mapper")}：{mapper.upstream_code ?? "upstream"} →{" "}
              {mapper.mapped_code ?? "mapped"}
            </p>
          )}
          {breaker && (
            <p className="text-text-secondary">
              {t("logs.circuit_breaker")}：{breaker.observed_state ?? "—"}
              {breaker.transition ? ` · ${breaker.transition}` : ""}
            </p>
          )}
          {degradation && (
            <p className="text-text-secondary">
              {t("logs.model_degradation")}：
              {degradation.requested_model ?? "—"} → {degradation.picked ?? "—"}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function formatConditions(
  conditions: Record<string, unknown> | null | undefined
): string {
  if (!conditions || Object.keys(conditions).length === 0) return "—";
  return Object.entries(conditions)
    .map(([key, value]) => {
      if (Array.isArray(value)) return `${key}: ${value.join(", ")}`;
      return `${key}: ${String(value)}`;
    })
    .join(" · ");
}

function RouteDecisionCard({ decision }: { decision: RouteDecisionTrace }) {
  const { t } = useI18n();

  return (
    <div className="rounded-lg border border-border bg-card-secondary p-4">
      <div className="mb-3 flex items-center justify-between">
        <h4 className="text-xs font-semibold text-text-primary">
          {t("logs.route_decision")}
        </h4>
        {decision.mode && (
          <StatusBadge variant="muted">{decision.mode}</StatusBadge>
        )}
      </div>
      <div className="grid grid-cols-2 gap-3 text-xs">
        <div>
          <span className="text-text-muted">{t("logs.route_profile")}</span>
          <p className="text-text-primary">{decision.profile_name ?? "—"}</p>
        </div>
        <div>
          <span className="text-text-muted">{t("logs.selected_provider")}</span>
          <p className="text-text-primary">
            {decision.selected_provider_name ?? "—"}
          </p>
        </div>
        <div>
          <span className="text-text-muted">{t("logs.selected_model")}</span>
          <p className="font-mono text-text-primary">
            {decision.selected_model ?? "—"}
          </p>
        </div>
        <div>
          <span className="text-text-muted">
            {t("logs.matched_conditions")}
          </span>
          <p className="text-text-primary">
            {formatConditions(decision.matched_conditions)}
          </p>
        </div>
      </div>
      {decision.candidates && decision.candidates.length > 0 && (
        <div className="mt-3 space-y-1">
          <span className="text-[11px] text-text-muted">
            {t("logs.route_candidates")}
          </span>
          <div className="flex flex-wrap gap-1.5">
            {decision.candidates.map((candidate, idx) => (
              <span
                key={`${candidate.provider_name ?? "candidate"}-${idx}`}
                className="rounded-md border border-border bg-card px-2 py-1 text-[11px] text-text-secondary"
              >
                {candidate.priority ?? idx + 1}.{" "}
                {candidate.provider_name ?? "—"}
                {candidate.has_conditions
                  ? ` · ${t("routes.has_conditions")}`
                  : ""}
                {candidate.skip_reasons?.length
                  ? ` · ${candidate.skip_reasons.map((r) => skipReasonLabel(r, t)).join(", ")}`
                  : ""}
              </span>
            ))}
          </div>
        </div>
      )}
      {decision.fallback_chain && decision.fallback_chain.length > 0 && (
        <div className="mt-3 space-y-1">
          <span className="text-[11px] text-text-muted">
            {t("logs.fallback_chain")}
          </span>
          <div className="flex flex-wrap gap-1.5">
            {decision.fallback_chain.map((step, idx) => (
              <span
                key={`${step.provider_name ?? "fallback"}-${idx}`}
                className={`rounded-md border px-2 py-1 text-[11px] ${
                  step.selected
                    ? "border-accent/40 bg-accent/10 text-accent"
                    : "border-border bg-card text-text-secondary"
                }`}
              >
                {step.step ?? idx + 1}.{" "}
                {step.role === "primary"
                  ? t("logs.fallback_primary")
                  : t("logs.fallback_backup")}{" "}
                · {step.provider_name ?? "—"}
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function StreamFailureTimelineCard({ request }: { request: RequestLogDetail }) {
  const { t } = useI18n();
  const steps = buildStreamFailureTimeline(request);
  if (steps.length === 0) return null;

  return (
    <div className="rounded-lg border border-border bg-card-secondary p-4">
      <div className="mb-1 flex items-center justify-between gap-2">
        <h4 className="text-xs font-semibold text-text-primary">
          {t("logs.timeline_title")}
        </h4>
        <span className="text-[10px] text-text-muted">
          {t("logs.timeline_hint")}
        </span>
      </div>
      <ol className="mt-3 space-y-0">
        {steps.map((step, idx) => (
          <TimelineRow
            key={step.id}
            step={step}
            isLast={idx === steps.length - 1}
            title={timelineTitle(step, t)}
          />
        ))}
      </ol>
    </div>
  );
}

function timelineTitle(step: TimelineStep, t: (key: string) => string): string {
  return t(step.titleKey);
}

function TimelineRow({
  step,
  isLast,
  title,
}: {
  step: TimelineStep;
  isLast: boolean;
  title: string;
}) {
  return (
    <li className="flex gap-3">
      <div className="flex w-4 shrink-0 flex-col items-center">
        <TimelineDot status={step.status} />
        {!isLast && (
          <div className="my-0.5 w-px flex-1 min-h-[12px] bg-border" />
        )}
      </div>
      <div className={`min-w-0 flex-1 ${isLast ? "pb-0" : "pb-3"}`}>
        <p className="text-xs font-medium text-text-primary">{title}</p>
        {step.detail && (
          <p
            className="mt-0.5 break-words font-mono text-[11px] leading-relaxed text-text-secondary"
            title={step.detail}
          >
            {step.detail}
          </p>
        )}
      </div>
    </li>
  );
}

function TimelineDot({ status }: { status: TimelineStatus }) {
  const cls = "h-3.5 w-3.5 shrink-0";
  switch (status) {
    case "error":
      return <XCircle className={`${cls} text-error`} />;
    case "warn":
      return <AlertTriangle className={`${cls} text-warning`} />;
    case "ok":
      return <CheckCircle2 className={`${cls} text-success`} />;
    case "skip":
      return <Circle className={`${cls} text-text-muted/50`} />;
    default:
      return <Circle className={`${cls} text-accent`} />;
  }
}

function ErrorChainCard({
  request,
  trace,
}: {
  request: RequestLogDetail;
  trace: RequestTrace | null;
}) {
  const { t } = useI18n();
  const mapper = trace?.error_mapper;
  const breaker = trace?.circuit_breaker;
  const degradation = trace?.degradation;

  return (
    <div className="rounded-lg border border-error/20 bg-error/5 p-4">
      <h4 className="mb-3 text-xs font-semibold text-text-primary">
        {t("logs.error_chain")}
      </h4>
      <div className="space-y-2 text-xs">
        <div className="rounded-md border border-border bg-card px-3 py-2">
          <span className="text-text-muted">{t("logs.error_final")}</span>
          <p className="mt-1 text-text-primary">
            HTTP {request.status_code ?? "—"} · {request.error_message ?? "—"}
          </p>
        </div>
        {mapper && (
          <div className="rounded-md border border-border bg-card px-3 py-2">
            <span className="text-text-muted">{t("logs.error_mapper")}</span>
            <p className="mt-1 text-text-primary">
              {mapper.upstream_code ?? "upstream"} →{" "}
              {mapper.mapped_code ?? "mapped"}
            </p>
            <p
              className="mt-1 truncate text-text-muted"
              title={mapper.upstream_message ?? mapper.mapped_message ?? ""}
            >
              {mapper.upstream_message ?? mapper.mapped_message ?? "—"}
            </p>
          </div>
        )}
        {breaker && (
          <div className="rounded-md border border-border bg-card px-3 py-2">
            <span className="text-text-muted">{t("logs.circuit_breaker")}</span>
            <p className="mt-1 text-text-primary">
              {breaker.observed_state ?? "—"}
              {breaker.transition ? ` · ${breaker.transition}` : ""}
            </p>
          </div>
        )}
        {degradation && (
          <div className="rounded-md border border-border bg-card px-3 py-2">
            <span className="text-text-muted">
              {t("logs.model_degradation")}
            </span>
            <p className="mt-1 text-text-primary">
              {degradation.requested_model ?? "—"} → {degradation.picked ?? "—"}
            </p>
            {degradation.chain?.length ? (
              <p className="mt-1 font-mono text-[11px] text-text-muted">
                {degradation.chain.join(" → ")}
              </p>
            ) : null}
          </div>
        )}
      </div>
    </div>
  );
}

function skipReasonLabel(reason: string, t: (key: string) => string): string {
  const labels: Record<string, string> = {
    disabled: t("logs.skip_disabled"),
    runtime_unavailable: t("logs.skip_runtime_unavailable"),
    cooldown: t("logs.skip_cooldown"),
    unsupported_vision: t("logs.skip_unsupported_vision"),
  };
  return labels[reason] ?? reason;
}
