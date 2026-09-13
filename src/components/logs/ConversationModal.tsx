import { useEffect, useState } from "react";
import {
  Loader2,
  X,
  Copy,
  Terminal,
  FileText,
  ChevronDown,
  ChevronRight,
  Wrench,
} from "lucide-react";
import { formatTimestamp } from "@/lib/utils";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import { MarkdownContent } from "@/components/common/MarkdownContent";
import * as api from "@/lib/api";
import type {
  ConversationMessage,
  SessionUsageSummary,
} from "@/types/request-log";

/// 按会话来源生成恢复命令。网关请求没有「会话恢复」概念，返回 null。
export function resumeCommand(
  sessionId: string,
  source: string
): string | null {
  if (source === "codex_session") return `codex resume ${sessionId}`;
  if (source === "gateway") return null;
  return `claude --resume ${sessionId}`; // claude_session / mixed / 默认
}

type ConversationMessageKind = "chat" | "tool_call" | "tool_result";

export function getConversationMessageKind(
  text: string
): ConversationMessageKind {
  const trimmed = text.trim();
  if (/^\[Tool:\s*.+\]$/.test(trimmed)) return "tool_call";
  if (/^\[Tool result\]/.test(trimmed)) return "tool_result";
  return "chat";
}

function shortId(id: string): string {
  if (id.length <= 16) return id;
  return `${id.slice(0, 8)}…${id.slice(-6)}`;
}

/// 会话对话弹窗：读 Claude Code / Codex 本地日志，渲染对话气泡 + 恢复命令。
export function ConversationModal({
  sessionId,
  source,
  usage,
  onClose,
}: {
  sessionId: string;
  source: string;
  usage?: SessionUsageSummary | null;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [msgs, setMsgs] = useState<ConversationMessage[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [usageSummary, setUsageSummary] = useState<SessionUsageSummary | null>(
    usage ?? null
  );

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    api
      .getSessionConversation(sessionId)
      .then((d) => {
        if (!cancelled) setMsgs(d);
      })
      .catch((e) => {
        if (!cancelled) setErr((e as api.AppError).message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  useEffect(() => {
    if (usage) {
      setUsageSummary(usage);
      return;
    }
    let cancelled = false;
    api
      .aggregateRequestLogsBySession({ session_id: sessionId }, 1)
      .then((rows) => {
        if (!cancelled) setUsageSummary(rows[0] ?? null);
      })
      .catch(() => {
        if (!cancelled) setUsageSummary(null);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, usage]);

  const cmd = resumeCommand(sessionId, source);
  const providersLabel =
    usageSummary?.providers?.filter(Boolean).join(", ") ||
    usageSummary?.provider ||
    null;
  const totalTokens = usageSummary
    ? usageSummary.input_tokens + usageSummary.output_tokens
    : null;

  const copyId = () => {
    navigator.clipboard.writeText(sessionId);
    toast("success", t("common.copied"));
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4 sm:p-6"
      onClick={onClose}
    >
      <div
        className="flex h-[min(90vh,880px)] w-full max-w-3xl flex-col overflow-hidden rounded-2xl border border-border bg-card shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-start justify-between gap-3 border-b border-border px-5 py-4">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="text-base font-semibold text-text-primary">
                {t("logs.conversation_title")}
              </h3>
              {usageSummary?.model && (
                <span className="rounded-full bg-card-secondary px-2 py-0.5 font-mono text-[10px] text-text-secondary">
                  {usageSummary.model}
                </span>
              )}
            </div>
            <button
              type="button"
              onClick={copyId}
              className="mt-1 max-w-full truncate font-mono text-[11px] text-text-muted transition-colors hover:text-accent"
              title={sessionId}
            >
              {shortId(sessionId)}
            </button>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            {cmd && (
              <button
                type="button"
                onClick={() => {
                  navigator.clipboard.writeText(cmd);
                  toast("success", t("logs.resume_cmd_copied"));
                }}
                className="inline-flex items-center gap-1.5 rounded-lg border border-border bg-card-secondary px-2.5 py-1.5 text-[11px] font-medium text-text-secondary transition-colors hover:border-accent/40 hover:text-accent"
                title={cmd}
              >
                <Terminal className="h-3.5 w-3.5" />
                {t("logs.resume_copy")}
                <Copy className="h-3 w-3 opacity-60" />
              </button>
            )}
            <button
              onClick={onClose}
              className="rounded-lg p-1.5 text-text-muted transition-colors hover:bg-card-secondary hover:text-text-primary"
              aria-label="close"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        {/* Compact stats */}
        {usageSummary && (
          <div className="flex flex-wrap gap-2 border-b border-border bg-card-secondary/30 px-5 py-2.5">
            <StatPill
              label={t("logs.session_col_requests")}
              value={usageSummary.request_count.toLocaleString()}
            />
            <StatPill
              label={t("logs.session_usage_tokens")}
              value={
                totalTokens != null
                  ? `${totalTokens.toLocaleString()} (${usageSummary.input_tokens.toLocaleString()}↑ / ${usageSummary.output_tokens.toLocaleString()}↓)`
                  : "—"
              }
            />
            <StatPill
              label={t("logs.session_col_cost")}
              value={
                usageSummary.cost > 0 ? `$${usageSummary.cost.toFixed(4)}` : "—"
              }
            />
            {providersLabel && (
              <StatPill
                label={t("logs.session_usage_providers")}
                value={providersLabel}
              />
            )}
          </div>
        )}

        {/* Messages */}
        <div className="flex-1 space-y-3 overflow-y-auto bg-background/40 px-4 py-4 sm:px-5">
          {loading ? (
            <div className="flex items-center justify-center gap-2 py-16 text-xs text-text-muted">
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("common.loading")}
            </div>
          ) : err ? (
            <p className="rounded-lg border border-error/30 bg-error/5 px-3 py-2 text-xs text-error">
              {err}
            </p>
          ) : msgs.length === 0 ? (
            <p className="py-16 text-center text-xs text-text-muted">
              {t("logs.conversation_empty")}
            </p>
          ) : (
            msgs.map((m, i) => <MessageBubble key={i} msg={m} />)
          )}
        </div>
      </div>
    </div>
  );
}

function StatPill({ label, value }: { label: string; value: string }) {
  return (
    <span className="inline-flex max-w-full items-center gap-1.5 rounded-full border border-border/80 bg-card px-2.5 py-1 text-[11px]">
      <span className="shrink-0 text-text-muted">{label}</span>
      <span className="truncate font-mono text-text-primary" title={value}>
        {value}
      </span>
    </span>
  );
}

function MessageBubble({ msg }: { msg: ConversationMessage }) {
  const { t } = useI18n();
  const isUser = msg.role === "user";
  const kind = getConversationMessageKind(msg.text);
  const time = msg.timestamp ? formatTimestamp(msg.timestamp) : "";
  const toolBody =
    kind === "tool_result" ? msg.text.replace(/^\[Tool result\]\s*/, "") : "";
  const toolResultLong =
    toolBody.length > 280 || toolBody.split("\n").length > 6;
  // Hooks must stay unconditional (even when not a tool_result bubble).
  const [toolOpen, setToolOpen] = useState(!toolResultLong);

  if (kind === "tool_call") {
    const toolName = msg.text
      .trim()
      .replace(/^\[Tool:\s*/, "")
      .replace(/\]$/, "");
    return (
      <div className="flex justify-start">
        <div className="inline-flex items-center gap-2 rounded-full border border-border bg-card px-3 py-1.5 text-xs text-text-secondary shadow-sm">
          <Wrench className="h-3.5 w-3.5 text-accent" />
          <span className="text-text-muted">{t("logs.tool_call")}</span>
          <span className="rounded-md bg-accent/10 px-1.5 py-0.5 font-mono text-[11px] font-medium text-accent">
            {toolName}
          </span>
          {time && <span className="text-[10px] text-text-muted">{time}</span>}
        </div>
      </div>
    );
  }

  if (kind === "tool_result") {
    return (
      <div className="flex justify-start">
        <div className="w-full max-w-[94%] overflow-hidden rounded-xl border border-border bg-card shadow-sm">
          <button
            type="button"
            onClick={() => setToolOpen((v) => !v)}
            className="flex w-full items-center gap-2 px-3 py-2 text-left text-[11px] font-medium text-text-secondary hover:bg-hover"
          >
            {toolOpen ? (
              <ChevronDown className="h-3.5 w-3.5 shrink-0" />
            ) : (
              <ChevronRight className="h-3.5 w-3.5 shrink-0" />
            )}
            <FileText className="h-3.5 w-3.5 shrink-0 text-text-muted" />
            <span>{t("logs.tool_result")}</span>
            {time && (
              <span className="ml-auto text-[10px] font-normal text-text-muted">
                {time}
              </span>
            )}
          </button>
          {toolOpen && (
            <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words border-t border-border bg-card-secondary/30 px-3 py-2.5 font-mono text-[11px] leading-relaxed text-text-secondary">
              {toolBody}
            </pre>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className={`flex flex-col ${isUser ? "items-end" : "items-start"}`}>
      <div className="mb-1 flex items-center gap-1.5 px-1 text-[10px] text-text-muted">
        <span>{isUser ? t("logs.msg_user") : t("logs.msg_ai")}</span>
        {time && <span>· {time}</span>}
      </div>
      <div
        className={`max-w-[82%] break-words px-3.5 py-2.5 text-sm leading-relaxed shadow-sm ${
          isUser
            ? "rounded-2xl rounded-br-md bg-accent text-on-accent"
            : "rounded-2xl rounded-bl-md border border-border bg-card text-text-primary"
        }`}
      >
        <MarkdownContent content={msg.text} />
      </div>
    </div>
  );
}
