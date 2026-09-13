import { useState } from "react";
import { X, Loader2, Zap, AlertCircle, Check } from "lucide-react";
import { useI18n } from "@/lib/i18n";
import { toast } from "@/components/common/Toast";
import * as api from "@/lib/api";
import type { ProviderSpeedReport } from "@/types/provider";

interface SpeedtestDialogProps {
  open: boolean;
  onClose: () => void;
}

/// Manual provider speedtest. Each "运行测速" press fires
/// provider_speedtest_all which sends a 1-token probe in parallel to every
/// enabled provider. Results show connect/TTFB/total latency. User-triggered
/// only — never automatic, because probes cost real tokens.
export function SpeedtestDialog({ open, onClose }: SpeedtestDialogProps) {
  const { t } = useI18n();
  const [reports, setReports] = useState<ProviderSpeedReport[] | null>(null);
  const [running, setRunning] = useState(false);

  if (!open) return null;

  const run = async () => {
    setRunning(true);
    setReports(null);
    try {
      const data = await api.providerSpeedtestAll();
      // Sort: success first (by total_ms ascending), then failures at bottom
      const sorted = [...data].sort((a, b) => {
        if (a.success !== b.success) return a.success ? -1 : 1;
        return a.total_ms - b.total_ms;
      });
      setReports(sorted);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center">
      <div className="fixed inset-0 bg-black/50" onClick={onClose} />
      <div className="relative z-10 w-full max-w-2xl max-h-[80vh] overflow-y-auto rounded-lg border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-6 py-4">
          <div className="flex items-center gap-2">
            <Zap className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-text-primary">
              {t("providers.speedtest_title")}
            </h2>
          </div>
          <button
            onClick={onClose}
            className="rounded-md p-1.5 text-text-muted hover:bg-card-secondary hover:text-text-primary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="p-6 space-y-4">
          <p className="text-xs text-text-muted leading-relaxed">
            {t("providers.speedtest_desc_1")}{" "}
            <code className="rounded bg-card-secondary px-1 py-0.5">
              max_tokens=1
            </code>{" "}
            {t("providers.speedtest_desc_2")}
            <span className="text-warning">
              {t("providers.speedtest_desc_cost")}
            </span>
            {t("providers.speedtest_desc_3")}
          </p>

          <button
            onClick={run}
            disabled={running}
            className="flex items-center gap-2 rounded-md bg-accent px-4 py-2 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90 disabled:opacity-50"
          >
            {running ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Zap className="h-3.5 w-3.5" />
            )}
            {running
              ? t("providers.speedtest_running")
              : reports
                ? t("providers.speedtest_rerun")
                : t("providers.speedtest_run")}
          </button>

          {reports && reports.length === 0 && (
            <p className="text-xs text-text-muted">
              {t("providers.speedtest_no_enabled")}
            </p>
          )}

          {reports && reports.length > 0 && (
            <div className="overflow-hidden rounded-md border border-border">
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b border-border bg-card-secondary">
                    <th className="px-3 py-2 text-left font-medium text-text-muted">
                      Provider
                    </th>
                    <th className="px-3 py-2 text-right font-medium text-text-muted">
                      Connect
                    </th>
                    <th className="px-3 py-2 text-right font-medium text-text-muted">
                      TTFB
                    </th>
                    <th className="px-3 py-2 text-right font-medium text-text-muted">
                      Total
                    </th>
                    <th className="px-3 py-2 text-center font-medium text-text-muted">
                      {t("providers.speedtest_col_status")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {reports.map((r) => (
                    <tr
                      key={r.provider_id}
                      className="border-b border-border/40 last:border-b-0"
                    >
                      <td className="px-3 py-2 text-text-primary">
                        <div className="font-medium">{r.provider_name}</div>
                        <div
                          className="font-mono text-[10px] text-text-muted truncate max-w-xs"
                          title={r.endpoint}
                        >
                          {r.endpoint}
                        </div>
                      </td>
                      <td className="px-3 py-2 text-right font-mono text-text-secondary">
                        {r.connect_ms !== null ? `${r.connect_ms}ms` : "—"}
                      </td>
                      <td className="px-3 py-2 text-right font-mono text-text-secondary">
                        {r.ttfb_ms !== null ? `${r.ttfb_ms}ms` : "—"}
                      </td>
                      <td className="px-3 py-2 text-right font-mono">
                        <LatencyCell ms={r.total_ms} ok={r.success} />
                      </td>
                      <td className="px-3 py-2 text-center">
                        {r.success ? (
                          <span
                            title={`HTTP ${r.status_code ?? ""}`}
                            className="inline-flex items-center gap-1 text-success"
                          >
                            <Check className="h-3 w-3" />
                          </span>
                        ) : (
                          <span
                            title={r.error ?? ""}
                            className="inline-flex items-center gap-1 text-error cursor-help"
                          >
                            <AlertCircle className="h-3 w-3" />
                            <span className="text-[10px]">
                              {r.status_code ?? "ERR"}
                            </span>
                          </span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {reports.some((r) => !r.success) && (
                <div className="border-t border-border bg-card-secondary/60 p-3 text-[11px] text-text-muted">
                  {t("providers.speedtest_error_hint")}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/// Color-code latency bucket: green < 500ms, amber 500-1500ms, red > 1500ms.
function LatencyCell({ ms, ok }: { ms: number; ok: boolean }) {
  if (!ok) return <span className="text-text-muted">{ms}ms</span>;
  const cls =
    ms < 500 ? "text-success" : ms < 1500 ? "text-warning" : "text-error";
  return <span className={cls}>{ms}ms</span>;
}
