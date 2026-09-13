import { useEffect, useState, useCallback, useRef } from "react";
import { CheckCircle, XCircle, AlertTriangle, X } from "lucide-react";
import { cn } from "@/lib/utils";

export type ToastType = "success" | "error" | "warning";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

interface ToastMessage {
  id: number;
  type: ToastType;
  message: string;
  action?: ToastAction;
}

let toastId = 0;
let addToastFn:
  | ((type: ToastType, message: string, action?: ToastAction) => void)
  | null = null;

export function toast(
  type: ToastType,
  message: string,
  opts?: { action?: ToastAction }
) {
  addToastFn?.(type, message, opts?.action);
}

const icons = {
  success: CheckCircle,
  error: XCircle,
  warning: AlertTriangle,
};

const styles = {
  success: "border-success/20 bg-success-soft",
  error: "border-error/20 bg-error-soft",
  warning: "border-warning/20 bg-warning-soft",
};

const iconColors = {
  success: "text-success",
  error: "text-error",
  warning: "text-warning",
};

const TOAST_DURATION = 4000;

export function ToastContainer() {
  const [toasts, setToasts] = useState<ToastMessage[]>([]);

  const addToast = useCallback(
    (type: ToastType, message: string, action?: ToastAction) => {
      const id = ++toastId;
      // 轮询失败会反复报同一条错误：同 type + message 的 toast 还在显示时不再叠加。
      setToasts((prev) =>
        prev.some((t) => t.type === type && t.message === message)
          ? prev
          : [...prev, { id, type, message, action }]
      );
      setTimeout(() => {
        setToasts((prev) => prev.filter((t) => t.id !== id));
      }, TOAST_DURATION);
    },
    []
  );

  useEffect(() => {
    addToastFn = addToast;
    return () => {
      addToastFn = null;
    };
  }, [addToast]);

  const dismiss = (id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  };

  return (
    <div className="fixed bottom-4 right-4 z-[100] flex flex-col gap-2">
      {toasts.map((t) => (
        <ToastItem key={t.id} toast={t} onDismiss={dismiss} />
      ))}
    </div>
  );
}

function ToastItem({
  toast: t,
  onDismiss,
}: {
  toast: ToastMessage;
  onDismiss: (id: number) => void;
}) {
  const Icon = icons[t.type];
  const progressRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (progressRef.current) {
      progressRef.current.style.transition = `width ${TOAST_DURATION}ms linear`;
      requestAnimationFrame(() => {
        if (progressRef.current) progressRef.current.style.width = "0%";
      });
    }
  }, []);

  return (
    <div
      className={cn(
        "animate-slide-in-right relative overflow-hidden rounded-lg border px-4 py-3",
        styles[t.type]
      )}
      style={{ boxShadow: "var(--shadow-md)" }}
    >
      <div className="flex items-center gap-3">
        <Icon className={cn("h-4 w-4 shrink-0", iconColors[t.type])} />
        <span className="text-xs text-text-primary">{t.message}</span>
        {t.action && (
          <button
            onClick={() => {
              t.action?.onClick();
              onDismiss(t.id);
            }}
            className={cn(
              "ml-1 shrink-0 rounded px-2 py-0.5 text-[11px] font-medium underline-offset-2 hover:underline",
              iconColors[t.type]
            )}
          >
            {t.action.label}
          </button>
        )}
        <button
          onClick={() => onDismiss(t.id)}
          className="ml-2 shrink-0 text-text-muted hover:text-text-primary"
        >
          <X className="h-3 w-3" />
        </button>
      </div>
      {/* Progress bar */}
      <div
        ref={progressRef}
        className={cn("absolute bottom-0 left-0 h-[2px] w-full", {
          "bg-success/40": t.type === "success",
          "bg-error/40": t.type === "error",
          "bg-warning/40": t.type === "warning",
        })}
      />
    </div>
  );
}
