import { useEffect, useState } from "react";
import {
  CheckCircle2,
  AlertTriangle,
  X,
  RefreshCw,
  Loader2,
} from "lucide-react";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";
import type { RunningProcess } from "@/lib/api";

interface Props {
  open: boolean;
  /// Client id used to gate the "restart desktop" button — only `codex` shows it.
  clientId?: string;
  /// Display name of the client we just applied config for, e.g. "Claude Code".
  clientName: string;
  /// Where the config landed on disk.
  configPath: string;
  /// Live processes whose basename matches the client. Empty list either
  /// means "definitely not running" or "couldn't detect" (Windows path) —
  /// we phrase the copy carefully so both cases are acceptable.
  processes: RunningProcess[];
  onClose: () => void;
}

/// Post-apply summary dialog. Does not auto-kill: the user clicks 结束进程.
/// Kill re-checks the live process list so a stale PID cannot hit something else.
export function PostApplyDialog({
  open,
  clientId,
  clientName,
  configPath,
  processes,
  onClose,
}: Props) {
  const { t } = useI18n();
  const [restarting, setRestarting] = useState(false);
  // 只有装了 Codex 桌面端才给重启按钮;Linux / 只有 CLI / 已并入 ChatGPT 时点了只会报错。
  const [codexDesktop, setCodexDesktop] = useState(false);
  const [killingPid, setKillingPid] = useState<number | null>(null);
  const [alive, setAlive] = useState(processes);
  useEffect(() => {
    if (open) setAlive(processes);
  }, [open, processes]);
  useEffect(() => {
    if (!open || clientId !== "codex") return;
    let cancelled = false;
    api
      .codexDesktopAvailable()
      .then((available) => {
        if (!cancelled) setCodexDesktop(available);
      })
      .catch(() => {
        if (!cancelled) setCodexDesktop(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, clientId]);

  if (!open) return null;

  const handleKill = async (pid: number) => {
    if (!clientId) {
      toast("error", t("tools.post_apply.kill_failed"));
      return;
    }
    setKillingPid(pid);
    try {
      await api.killClientProcess(clientId, pid);
      setAlive((list) => list.filter((p) => p.pid !== pid));
      toast("success", t("tools.post_apply.killed"));
    } catch (err) {
      const code = (err as api.AppError | undefined)?.code;
      toast(
        "error",
        code === "CLIENT_PROCESS_NOT_FOUND"
          ? t("tools.post_apply.kill_gone")
          : t("tools.post_apply.kill_failed")
      );
    } finally {
      setKillingPid(null);
    }
  };

  const handleRestartCodex = async () => {
    setRestarting(true);
    try {
      const r = await api.restartCodexDesktop();
      if (!r.supported) {
        toast("error", t("tools.post_apply.restart_unsupported"));
        return;
      }
      if (r.relaunched) {
        toast(
          "success",
          r.was_running
            ? t("tools.post_apply.restart_done")
            : t("tools.post_apply.restart_launched")
        );
        // 重启 Codex 不会结束 ChatGPT 桌面端,它在跑时要用户自己重启才生效。
        if (r.chatgpt_needs_manual_restart) {
          toast("warning", t("tools.post_apply.chatgpt_manual_restart"));
        }
        onClose();
      } else {
        toast("error", t("tools.post_apply.restart_failed"));
      }
    } catch {
      toast("error", t("tools.post_apply.restart_failed"));
    } finally {
      setRestarting(false);
    }
  };

  // Codex Desktop 是唯一一个 GUI 客户端；其他 CLI 重启 shell 无意义。
  const showCodexRestart = clientId === "codex" && codexDesktop;

  return (
    <div className="fixed inset-0 z-[120] flex items-center justify-center">
      <div
        className="fixed inset-0 bg-black/40 backdrop-blur-sm"
        onClick={onClose}
      />
      <div
        className="animate-scale-in relative z-10 w-full max-w-md rounded-xl border border-border bg-card p-6"
        style={{ boxShadow: "var(--shadow-lg)" }}
      >
        <div className="mb-4 flex items-start justify-between gap-3">
          <h3 className="text-sm font-semibold text-text-primary">
            {t("tools.post_apply.title").replace("{name}", clientName)}
          </h3>
          <button
            onClick={onClose}
            className="rounded-md p-1 text-text-muted hover:bg-hover hover:text-text-primary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-3">
          <div className="flex items-start gap-2 rounded-md border border-success/30 bg-success-soft p-3">
            <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-success" />
            <div className="min-w-0 flex-1">
              <p className="text-xs font-medium text-success">
                {t("tools.post_apply.written")}
              </p>
              <p className="mt-0.5 break-all text-[11px] text-text-muted">
                {configPath}
              </p>
            </div>
          </div>

          {alive.length > 0 ? (
            <div className="rounded-md border border-warning/30 bg-warning-soft p-3">
              <div className="flex items-start gap-2">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
                <div className="min-w-0 flex-1">
                  <p className="text-xs font-medium text-warning">
                    {t("tools.post_apply.running").replace(
                      "{name}",
                      clientName
                    )}
                  </p>
                  <p className="mt-0.5 text-[11px] leading-relaxed text-text-secondary">
                    {t("tools.post_apply.running_desc")}
                  </p>
                </div>
              </div>
              <div className="mt-2 space-y-1">
                {alive.map((p) => (
                  <div
                    key={p.pid}
                    className="flex items-center justify-between gap-2 rounded border border-border bg-card px-2 py-1"
                  >
                    <code className="truncate text-[11px] text-text-secondary">
                      PID {p.pid} · {p.command}
                    </code>
                    <button
                      type="button"
                      onClick={() => handleKill(p.pid)}
                      disabled={killingPid === p.pid}
                      className="shrink-0 rounded bg-warning/15 px-2 py-0.5 text-[10px] font-medium text-warning hover:bg-warning/25 disabled:opacity-60"
                    >
                      {killingPid === p.pid ? (
                        <Loader2 className="inline h-3 w-3 animate-spin" />
                      ) : (
                        t("tools.post_apply.kill")
                      )}
                    </button>
                  </div>
                ))}
              </div>
            </div>
          ) : (
            <div className="flex items-start gap-2 rounded-md border border-border bg-card-secondary p-3">
              <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-text-muted" />
              <div className="min-w-0 flex-1">
                <p className="text-xs text-text-secondary">
                  {t("tools.post_apply.not_running").replace(
                    "{name}",
                    clientName
                  )}
                </p>
              </div>
            </div>
          )}

          {/* 精确进程匹配认不出 Codex 桌面端(可执行文件名是 Codex),按钮不能依赖进程列表;
              按是否装了桌面端决定。没在跑时点它会直接拉起。 */}
          {showCodexRestart && (
            <button
              type="button"
              onClick={handleRestartCodex}
              disabled={restarting}
              className="flex w-full items-center justify-center gap-1.5 rounded-md border border-warning/40 bg-warning/10 px-2 py-1.5 text-[11px] font-medium text-warning hover:bg-warning/20 disabled:opacity-60"
            >
              {restarting ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <RefreshCw className="h-3 w-3" />
              )}
              {t("tools.post_apply.restart_codex")}
            </button>
          )}
        </div>

        <button
          onClick={onClose}
          className="mt-4 w-full rounded-md border border-border bg-card-secondary py-1.5 text-xs font-medium text-text-primary hover:bg-hover"
        >
          {t("common.close")}
        </button>
      </div>
    </div>
  );
}
