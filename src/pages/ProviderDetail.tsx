import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, AlertTriangle, Activity, Boxes } from "lucide-react";
import { StatusBadge } from "@/components/common/StatusBadge";
import { useI18n } from "@/lib/i18n";
import { formatCost, formatLatency, formatTimestamp } from "@/lib/utils";
import * as api from "@/lib/api";
import type { ProviderView } from "@/types/provider";
import type { ProviderHealth } from "@/types/stats";
import type { ProviderDetailStats } from "@/types/request-log";
import type { ProviderRuntimeStatus } from "@/types/route-profile";

export function ProviderDetail() {
  const { t, locale } = useI18n();
  const { id } = useParams();
  const navigate = useNavigate();
  const [provider, setProvider] = useState<ProviderView | null>(null);
  const [health, setHealth] = useState<ProviderHealth | null>(null);
  const [stats, setStats] = useState<ProviderDetailStats | null>(null);
  const [runtime, setRuntime] = useState<ProviderRuntimeStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    if (!id) return;
    setLoading(true);
    setError("");
    try {
      const providerData = await api.getProvider(id);
      const [healthData, statsData, runtimeRows] = await Promise.all([
        api.getProviderHealth(providerData.name),
        api.aggregateProviderDetailStats(providerData.name, 7, 40),
        api
          .listProviderRuntimeStatus()
          .catch(() => [] as ProviderRuntimeStatus[]),
      ]);
      setProvider(providerData);
      setHealth(healthData);
      setStats(statsData);
      setRuntime(
        runtimeRows.find((row) => row.provider_id === providerData.id) ?? null
      );
    } catch (err) {
      setError((err as api.AppError).message);
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    load();
  }, [load]);

  const maxLatency = useMemo(() => {
    return Math.max(
      ...(stats?.latency_points.map((point) => point.latency_ms) ?? [0]),
      1
    );
  }, [stats]);

  if (loading) {
    return <p className="text-xs text-text-muted">{t("common.loading")}</p>;
  }

  if (error || !provider) {
    return (
      <div className="space-y-3">
        <button
          onClick={() => navigate("/providers")}
          className="flex items-center gap-1 text-xs text-text-muted hover:text-text-primary"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          {t("providers.detail.back")}
        </button>
        <div className="rounded-md border border-error/30 bg-error/10 p-3 text-sm text-error">
          {error || t("providers.detail.not_found")}
        </div>
      </div>
    );
  }

  return (
    <div className="desktop-page">
      <div className="desktop-page-header">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="space-y-3">
            <button
              onClick={() => navigate("/providers")}
              className="flex items-center gap-1 text-xs text-text-muted hover:text-text-primary"
            >
              <ArrowLeft className="h-3.5 w-3.5" />
              {t("providers.detail.back")}
            </button>
            <div>
              <div className="flex flex-wrap items-center gap-2">
                <h1 className="text-xl font-semibold text-text-primary">
                  {provider.name}
                </h1>
                {provider.is_active && (
                  <StatusBadge variant="accent">
                    {t("providers.active")}
                  </StatusBadge>
                )}
                {runtime?.quota_exhausted && (
                  <StatusBadge variant="error">
                    {t("providers.runtime_quota")}
                  </StatusBadge>
                )}
              </div>
              <p className="mt-1 font-mono text-xs text-text-muted">
                {provider.base_url}
              </p>
            </div>
          </div>
          <button
            onClick={load}
            className="rounded-md border border-border bg-card-secondary px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:border-accent/40 hover:text-text-primary"
          >
            {t("common.refresh")}
          </button>
        </div>
      </div>

      <section className="surface-panel p-4">
        <div className="mb-3 flex items-center justify-between gap-3">
          <h2 className="text-sm font-semibold text-text-primary">
            {t("providers.detail.health_strip")}
          </h2>
          <span className="font-mono text-xs text-text-muted">
            {provider.id}
          </span>
        </div>
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <MetricCard
            label={t("providers.detail.requests_24h")}
            value={(health?.h24_total ?? 0).toLocaleString()}
          />
          <MetricCard
            label={t("providers.health_success")}
            value={`${health?.h24_success_rate ?? 0}%`}
          />
          <MetricCard
            label={t("providers.health_avg_latency")}
            value={formatLatency(health?.h24_avg_latency_ms ?? 0)}
          />
          <MetricCard
            label={t("providers.detail.cost_7d")}
            value={formatCost(
              stats?.model_stats.reduce((sum, row) => sum + row.cost, 0) ?? 0
            )}
          />
        </div>
      </section>

      <section className="surface-panel space-y-2 p-4">
        <h2 className="flex items-center gap-2 text-sm font-semibold text-text-primary">
          <Activity className="h-4 w-4 text-accent" />
          {t("providers.detail.latency_monitor")}
        </h2>
        <div className="rounded-lg border border-border/70 bg-card-secondary/45 p-3">
          {stats && stats.latency_points.length > 0 ? (
            <div className="flex h-36 items-end gap-1">
              {stats.latency_points.map((point) => (
                <div
                  key={`${point.timestamp}-${point.latency_ms}-${point.model ?? ""}`}
                  className="group flex min-w-0 flex-1 flex-col items-center justify-end"
                  title={`${formatTimestamp(point.timestamp, locale)} · ${point.model ?? "-"} · ${formatLatency(point.latency_ms)}`}
                >
                  <div
                    className={`w-full rounded-t ${point.status_code && point.status_code >= 400 ? "bg-error/70" : "bg-accent/70"}`}
                    style={{
                      height: `${Math.max(8, Math.round((point.latency_ms / maxLatency) * 120))}px`,
                    }}
                  />
                </div>
              ))}
            </div>
          ) : (
            <p className="text-xs text-text-muted">
              {t("providers.detail.empty_latency")}
            </p>
          )}
        </div>
      </section>

      <section className="surface-panel space-y-2 p-4">
        <h2 className="flex items-center gap-2 text-sm font-semibold text-text-primary">
          <Boxes className="h-4 w-4 text-accent" />
          {t("providers.detail.model_stats")}
        </h2>
        <div className="surface-scroll rounded-md border border-border bg-card">
          <table className="w-full text-left text-xs">
            <thead className="bg-card-secondary text-text-muted">
              <tr>
                <th className="px-3 py-2 font-medium">
                  {t("providers.detail.model")}
                </th>
                <th className="px-3 py-2 font-medium">
                  {t("providers.detail.requests")}
                </th>
                <th className="px-3 py-2 font-medium">
                  {t("providers.health_success")}
                </th>
                <th className="px-3 py-2 font-medium">
                  {t("providers.health_avg_latency")}
                </th>
                <th className="px-3 py-2 font-medium">
                  {t("providers.detail.cost")}
                </th>
              </tr>
            </thead>
            <tbody>
              {stats && stats.model_stats.length > 0 ? (
                stats.model_stats.map((row) => (
                  <tr key={row.model} className="border-t border-border/60">
                    <td className="px-3 py-2 font-mono text-text-primary">
                      {row.model}
                    </td>
                    <td className="px-3 py-2 text-text-secondary">
                      {row.request_count}
                    </td>
                    <td className="px-3 py-2 text-text-secondary">
                      {Math.round(row.success_rate * 100)}%
                    </td>
                    <td className="px-3 py-2 text-text-secondary">
                      {formatLatency(row.avg_latency_ms)}
                    </td>
                    <td className="px-3 py-2 text-text-secondary">
                      {formatCost(row.cost)}
                    </td>
                  </tr>
                ))
              ) : (
                <tr>
                  <td className="px-3 py-4 text-text-muted" colSpan={5}>
                    {t("providers.detail.empty_models")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </section>

      <section className="surface-panel space-y-2 p-4">
        <h2 className="flex items-center gap-2 text-sm font-semibold text-text-primary">
          <AlertTriangle className="h-4 w-4 text-warning" />
          {t("providers.health_recent_errors")}
        </h2>
        <div className="rounded-lg border border-border bg-card-secondary/45">
          {health && health.recent_errors.length > 0 ? (
            health.recent_errors.map((item) => (
              <div
                key={`${item.timestamp}-${item.status_code}-${item.message}`}
                className="border-b border-border/60 px-3 py-2 last:border-b-0"
              >
                <div className="flex items-center justify-between gap-3 text-xs">
                  <span className="font-mono text-text-muted">
                    {formatTimestamp(item.timestamp, locale)}
                  </span>
                  <StatusBadge variant="error">{item.status_code}</StatusBadge>
                </div>
                <p
                  className="mt-1 truncate text-xs text-text-secondary"
                  title={item.message}
                >
                  {item.message}
                </p>
              </div>
            ))
          ) : (
            <p className="px-3 py-4 text-xs text-text-muted">
              {t("providers.detail.empty_errors")}
            </p>
          )}
        </div>
      </section>
    </div>
  );
}

function MetricCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-border/70 bg-card-secondary/45 p-3">
      <p className="text-xs text-text-muted">{label}</p>
      <p className="mt-1 text-lg font-semibold text-text-primary">{value}</p>
    </div>
  );
}
