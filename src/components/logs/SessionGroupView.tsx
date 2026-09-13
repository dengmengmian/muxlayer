import { useEffect, useState } from "react";
import {
  Loader2,
  Layers,
  MessageSquare,
  Trash2,
  ListFilter,
  ArrowRight,
} from "lucide-react";
import { EmptyState } from "@/components/common/EmptyState";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { ConversationModal } from "@/components/logs/ConversationModal";
import { formatTimestamp } from "@/lib/utils";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import { sourceLabel } from "@/components/logs/RequestLogTable";
import * as api from "@/lib/api";
import type {
  SessionUsageSummary,
  RequestLogFilter,
} from "@/types/request-log";

interface SessionGroupViewProps {
  filter: RequestLogFilter;
  /** Open conversation for this session (primary). */
  onOpenConversation?: (sessionId: string, source: string) => void;
  /** Switch to request list filtered to this session. */
  onPickSession: (sessionId: string) => void;
  /** Optional empty-state primary action (e.g. open sync). */
  onEmptyAction?: () => void;
}

function shortId(id: string): string {
  if (id.length <= 18) return id;
  return `${id.slice(0, 8)}…${id.slice(-6)}`;
}

/// Logs 页「按会话聚合」视图——卡片列表，主操作是打开对话。
export function SessionGroupView({
  filter,
  onPickSession,
  onEmptyAction,
}: SessionGroupViewProps) {
  const { t } = useI18n();
  const [rows, setRows] = useState<SessionUsageSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [convo, setConvo] = useState<{
    sessionId: string;
    source: string;
    usage: SessionUsageSummary;
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await api.deleteSession(deleteTarget);
      setRows((prev) => prev.filter((r) => r.session_id !== deleteTarget));
      toast("success", t("logs.session_deleted"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
    setDeleteTarget(null);
  };

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      try {
        const data = await api.aggregateRequestLogsBySession(filter, 100);
        if (!cancelled) setRows(data);
      } catch (err) {
        if (!cancelled) toast("error", (err as api.AppError).message);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [JSON.stringify(filter)]);

  if (loading) {
    return (
      <div className="surface-panel flex items-center justify-center gap-2 py-16 text-xs text-text-muted">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("common.loading")}
      </div>
    );
  }

  if (rows.length === 0) {
    return (
      <EmptyState
        icon={Layers}
        title={t("logs.session_empty_title")}
        description={t("logs.session_empty_desc")}
        action={
          onEmptyAction ? (
            <button
              type="button"
              onClick={onEmptyAction}
              className="btn-primary"
            >
              {t("logs.session_empty_cta")}
            </button>
          ) : undefined
        }
      />
    );
  }

  return (
    <>
      <div className="space-y-2">
        <p className="px-0.5 text-[11px] text-text-muted">
          {t("logs.session_list_count").replace("{n}", String(rows.length))}
          <span className="mx-1.5 text-text-muted/40">·</span>
          {t("logs.session_list_hint")}
        </p>
        <ul className="space-y-2">
          {rows.map((row) => {
            const totalTok = row.input_tokens + row.output_tokens;
            return (
              <li key={row.session_id}>
                <div className="surface-panel group p-4 transition-colors hover:border-accent/30 hover:bg-hover/40">
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <button
                      type="button"
                      className="min-w-0 flex-1 text-left"
                      onClick={() =>
                        setConvo({
                          sessionId: row.session_id,
                          source: row.source,
                          usage: row,
                        })
                      }
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <SourceChip source={row.source} />
                        {row.model && (
                          <span
                            className="max-w-[220px] truncate font-mono text-[11px] text-text-secondary"
                            title={row.model}
                          >
                            {row.model}
                          </span>
                        )}
                        <span className="text-[11px] text-text-muted">
                          {formatTimestamp(row.last_seen)}
                        </span>
                      </div>
                      <div
                        className="mt-1.5 font-mono text-sm font-medium text-text-primary"
                        title={row.session_id}
                      >
                        {shortId(row.session_id)}
                      </div>
                      <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-muted">
                        <span>
                          {t("logs.session_col_requests")}{" "}
                          <span className="font-mono text-text-primary">
                            {row.request_count.toLocaleString()}
                          </span>
                        </span>
                        <span>
                          {t("logs.session_usage_tokens")}{" "}
                          <span className="font-mono text-text-primary">
                            {totalTok.toLocaleString()}
                          </span>
                          <span className="text-text-muted">
                            {" "}
                            ({row.input_tokens.toLocaleString()} /{" "}
                            {row.output_tokens.toLocaleString()})
                          </span>
                        </span>
                        <span>
                          {t("logs.session_col_cost")}{" "}
                          <span className="font-mono text-text-primary">
                            {row.cost > 0 ? `$${row.cost.toFixed(4)}` : "—"}
                          </span>
                        </span>
                        {row.cache_read_tokens > 0 && (
                          <span>
                            {t("logs.session_col_cache_read")}{" "}
                            <span className="font-mono text-text-primary">
                              {row.cache_read_tokens.toLocaleString()}
                            </span>
                          </span>
                        )}
                      </div>
                    </button>

                    <div className="flex shrink-0 items-center gap-1.5">
                      <button
                        type="button"
                        onClick={() =>
                          setConvo({
                            sessionId: row.session_id,
                            source: row.source,
                            usage: row,
                          })
                        }
                        className="inline-flex items-center gap-1.5 rounded-lg bg-accent px-2.5 py-1.5 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90"
                      >
                        <MessageSquare className="h-3.5 w-3.5" />
                        {t("logs.session_open_convo")}
                      </button>
                      <button
                        type="button"
                        onClick={() => onPickSession(row.session_id)}
                        className="inline-flex items-center gap-1 rounded-lg border border-border bg-card-secondary px-2.5 py-1.5 text-[11px] font-medium text-text-secondary transition-colors hover:text-text-primary"
                        title={t("logs.session_view_requests")}
                      >
                        <ListFilter className="h-3.5 w-3.5" />
                        {t("logs.session_view_requests")}
                        <ArrowRight className="h-3 w-3 opacity-50" />
                      </button>
                      <button
                        type="button"
                        onClick={() => setDeleteTarget(row.session_id)}
                        className="rounded-lg p-1.5 text-text-muted transition-colors hover:bg-error/10 hover:text-error"
                        title={t("logs.session_delete")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      </div>
      {convo && (
        <ConversationModal
          sessionId={convo.sessionId}
          source={convo.source}
          usage={convo.usage}
          onClose={() => setConvo(null)}
        />
      )}
      <ConfirmDialog
        open={!!deleteTarget}
        title={t("logs.session_delete_title")}
        message={t("logs.session_delete_msg")}
        confirmLabel={t("common.delete")}
        variant="danger"
        onConfirm={handleDelete}
        onCancel={() => setDeleteTarget(null)}
      />
    </>
  );
}

function SourceChip({ source }: { source: string }) {
  const { t } = useI18n();
  const isMixed = source === "mixed";
  const color =
    source === "gateway"
      ? "bg-accent/15 text-accent"
      : isMixed
        ? "bg-warning/15 text-warning"
        : "bg-card-secondary text-text-secondary";
  return (
    <span
      className={`inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium ${color}`}
    >
      {sourceLabel(source, t)}
    </span>
  );
}
