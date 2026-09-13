import { StatusBadge } from "@/components/common/StatusBadge";
import { formatTimestamp, formatOptionalLatency } from "@/lib/utils";
import { useI18n } from "@/lib/i18n";
import type { RequestLogListItem } from "@/types/request-log";

interface RequestLogTableProps {
  requests: RequestLogListItem[];
  onSelect: (req: RequestLogListItem) => void;
}

export function RequestLogTable({ requests, onSelect }: RequestLogTableProps) {
  const { t } = useI18n();

  return (
    <div className="surface-scroll rounded-md border border-border bg-card">
      <table className="w-full text-left text-xs">
        <thead>
          <tr className="border-b border-border text-text-muted">
            <th className="px-5 py-3 font-medium">{t("logs.time")}</th>
            <th className="px-5 py-3 font-medium">{t("logs.route")}</th>
            <th className="px-5 py-3 font-medium">{t("logs.client")}</th>
            <th className="px-5 py-3 font-medium">{t("logs.provider")}</th>
            <th className="px-5 py-3 font-medium">{t("logs.model")}</th>
            <th className="px-5 py-3 font-medium">{t("logs.status")}</th>
            <th className="px-5 py-3 font-medium text-right">
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
                onClick={() => onSelect(req)}
                className="cursor-pointer border-b border-border/50 transition-colors hover:bg-hover"
              >
                <td className="px-5 py-2.5 font-mono text-text-muted">
                  {formatTimestamp(req.timestamp)}
                </td>
                <td className="px-5 py-2.5 font-mono text-text-secondary">
                  {req.route ?? "—"}
                </td>
                <td className="px-5 py-2.5 text-text-primary">
                  {req.client ?? "—"}
                </td>
                <td className="px-5 py-2.5 text-text-secondary">
                  <div className="flex items-center gap-1.5">
                    <SourceBadge source={req.source} />
                    <span>{req.provider ?? "—"}</span>
                  </div>
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
                  {formatOptionalLatency(req.latency_ms)}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/// 来源徽章——区分 gateway 流量 vs 从客户端本地日志扫出来的条目。后者拿不到
/// raw_request / SSE / tool_calls 等字段，详情页会显示降级 banner。
function SourceBadge({ source }: { source: string | null }) {
  const { t } = useI18n();
  if (!source || source === "gateway") return null;
  const label = sourceLabel(source, t);
  return (
    <span
      title={label}
      className="inline-flex h-4 items-center rounded bg-card-secondary px-1.5 text-[10px] font-medium uppercase tracking-wider text-text-muted"
    >
      {label}
    </span>
  );
}

export function sourceLabel(
  source: string | null,
  t: (key: string) => string
): string {
  switch (source) {
    case "gateway":
      return t("logs.source_gateway");
    case "claude_session":
      return "Claude";
    case "codex_session":
      return "Codex";
    case "gemini_session":
      return "Gemini";
    case "mixed":
      return t("logs.source_mixed");
    default:
      return source ?? "—";
  }
}
