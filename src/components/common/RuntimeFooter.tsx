// Sticky bottom KPI footer surfaced on Dashboard + Routes.
//
// Single source of truth for the "long-running scoreboard" view. Combines:
//   - runtime-only:  active_requests, uptime
//   - lifetime:      total_requests, total_tokens, total_cost, success_rate
//
// Deliberately omits today_* metrics — the Dashboard's "今日" strip already
// covers them, and duplicating them here led to two overlapping bottom
// blocks. Polls every 5 s.

import { useCallback, useEffect, useState } from "react";
import {
  Activity,
  Clock,
  TrendingUp,
  Database,
  DollarSign,
  CheckCircle2,
} from "lucide-react";
import * as api from "@/lib/api";
import { formatCost } from "@/lib/utils";
import { useI18n } from "@/lib/i18n";
import { usePolling } from "@/lib/usePolling";
import type { RuntimeKpis } from "@/types/stats";

function formatUptime(secs: number): string {
  if (secs <= 0) return "—";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function formatTokens(n: number): string {
  if (n === 0) return "0";
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

interface MetricProps {
  icon: React.ReactNode;
  label: string;
  value: string;
  tone?: "default" | "accent" | "muted";
}

function Metric({ icon, label, value, tone = "default" }: MetricProps) {
  const tones: Record<string, string> = {
    default: "text-text-primary",
    accent: "text-accent",
    muted: "text-text-muted",
  };
  return (
    <div className="flex min-w-0 items-center gap-2 border-l border-border/70 pl-4 first:border-l-0 first:pl-0">
      <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-accent-soft text-accent">
        {icon}
      </div>
      <div className="flex min-w-0 flex-col">
        <span className="text-xs uppercase tracking-wide text-text-muted">
          {label}
        </span>
        <span
          className={`font-mono text-base font-semibold tabular-nums ${tones[tone]}`}
        >
          {value}
        </span>
      </div>
    </div>
  );
}

export function RuntimeFooter() {
  const { t } = useI18n();
  const [kpis, setKpis] = useState<RuntimeKpis | null>(null);

  const load = useCallback(async () => {
    try {
      const k = await api.getRuntimeKpis();
      setKpis(k);
    } catch {
      // Silent — footer is a passive observer, don't toast errors.
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);
  usePolling(load, 15_000);

  if (!kpis) return null;

  const rateStr =
    kpis.total_requests > 0 ? `${kpis.success_rate_lifetime.toFixed(0)}%` : "—";

  return (
    <div className="rounded-lg border border-border/80 bg-card px-5 py-3">
      <div className="mb-3 flex items-center justify-between">
        <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-text-secondary">
          <span className="h-2 w-2 rounded-full bg-accent shadow-[0_0_10px_rgba(194,112,43,0.45)]" />
          {t("stats.runtime_strip")}
        </h3>
      </div>
      <div className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3 lg:grid-cols-6">
        <Metric
          icon={<Activity className="h-3.5 w-3.5" />}
          label={t("stats.active_connections")}
          value={String(kpis.active_requests)}
        />
        <Metric
          icon={<Clock className="h-3.5 w-3.5" />}
          label={t("stats.uptime")}
          value={
            kpis.gateway_running
              ? formatUptime(kpis.uptime_seconds)
              : t("stats.stopped")
          }
          tone={kpis.gateway_running ? "default" : "muted"}
        />
        <Metric
          icon={<TrendingUp className="h-3.5 w-3.5" />}
          label={t("stats.total_requests")}
          value={kpis.total_requests.toLocaleString()}
        />
        <Metric
          icon={<Database className="h-3.5 w-3.5" />}
          label={t("stats.total_tokens")}
          value={formatTokens(kpis.total_tokens)}
        />
        <Metric
          icon={<DollarSign className="h-3.5 w-3.5" />}
          label={t("stats.total_cost")}
          value={formatCost(kpis.total_cost)}
        />
        <Metric
          icon={<CheckCircle2 className="h-3.5 w-3.5" />}
          label={t("stats.success_rate_lifetime")}
          value={rateStr}
          tone={
            kpis.total_requests > 0 && kpis.success_rate_lifetime < 90
              ? "default"
              : "accent"
          }
        />
      </div>
    </div>
  );
}
