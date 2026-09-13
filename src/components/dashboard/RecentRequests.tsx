import { StatusBadge } from "@/components/common/StatusBadge";
import { formatTimestamp, formatLatency } from "@/lib/utils";
import { useI18n } from "@/lib/i18n";
import type { RequestLogListItem } from "@/types/request-log";
import type { ToolConfigView } from "@/types/tool";

interface RecentRequestsProps {
  requests: RequestLogListItem[];
  tools?: ToolConfigView[];
}

export function RecentRequests({ requests, tools }: RecentRequestsProps) {
  const { t } = useI18n();

  if (requests.length === 0) return null;

  return (
    <div className="rounded-lg border border-border/80 bg-card">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-border px-5 py-3">
        <h3 className="flex items-center gap-2 text-sm font-semibold text-text-primary">
          <span className="h-2 w-2 rounded-full bg-success shadow-[0_0_10px_rgba(56,161,105,0.45)]" />
          {t("dashboard.request_stream")}
        </h3>
        {tools && tools.length > 0 && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
            <span className="text-text-muted">
              {t("dashboard.tool_status")}
            </span>
            {tools.map((tool) => (
              <span
                key={tool.id}
                className="flex items-center gap-1"
                title={tool.config_path}
              >
                <span
                  className={`inline-block h-1.5 w-1.5 rounded-full ${tool.config_exists ? "bg-success" : "bg-text-muted/30"}`}
                />
                <span
                  className={
                    tool.config_exists ? "text-text-primary" : "text-text-muted"
                  }
                >
                  {tool.name}
                </span>
              </span>
            ))}
          </div>
        )}
      </div>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-xs">
          <thead>
            <tr className="border-b border-border bg-card-secondary/30 text-xs uppercase tracking-wide text-text-muted">
              <th className="px-5 py-2.5 font-medium">{t("logs.time")}</th>
              <th className="px-5 py-2.5 font-medium">{t("logs.client")}</th>
              <th className="px-5 py-2.5 font-medium">{t("logs.provider")}</th>
              <th className="px-5 py-2.5 font-medium">{t("logs.model")}</th>
              <th className="px-5 py-2.5 font-medium">{t("logs.status")}</th>
              <th className="px-5 py-2.5 font-medium text-right">
                {t("logs.latency")}
              </th>
            </tr>
          </thead>
          <tbody>
            {requests.map((req) => {
              const isError =
                req.status_code !== null &&
                (req.status_code >= 400 || req.status_code < 200);
              return (
                <tr
                  key={req.id}
                  className="border-b border-border/50 transition-colors hover:bg-accent/5"
                >
                  <td className="px-5 py-2.5 font-mono text-text-muted">
                    {formatTimestamp(req.timestamp)}
                  </td>
                  <td className="px-5 py-2.5 text-text-primary">
                    {req.client ?? "—"}
                  </td>
                  <td className="px-5 py-2.5 text-text-secondary">
                    {req.provider ?? "—"}
                  </td>
                  <td className="px-5 py-2.5 font-mono text-text-secondary">
                    {req.model ?? "—"}
                  </td>
                  <td className="px-5 py-2.5">
                    <StatusBadge variant={isError ? "error" : "success"}>
                      {req.status_code ?? "—"}
                    </StatusBadge>
                  </td>
                  <td className="px-5 py-2.5 text-right font-mono text-text-secondary">
                    {req.latency_ms !== null
                      ? formatLatency(req.latency_ms)
                      : "—"}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
