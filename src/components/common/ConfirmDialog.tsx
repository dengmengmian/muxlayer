import { AlertTriangle } from "lucide-react";
import type { ReactNode } from "react";
import { useI18n } from "@/lib/i18n";

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  message: string;
  children?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  variant?: "danger" | "default";
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  message,
  children,
  confirmLabel,
  cancelLabel,
  variant = "default",
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const { t } = useI18n();
  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[130] flex items-center justify-center">
      <div
        className="fixed inset-0 bg-black/40 backdrop-blur-sm"
        onClick={onCancel}
      />
      <div
        className="animate-scale-in relative z-10 w-full max-w-md rounded-xl border border-border bg-card p-6"
        style={{ boxShadow: "var(--shadow-lg)" }}
      >
        <div className="mb-5 flex items-start gap-3">
          {variant === "danger" && (
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-error-soft">
              <AlertTriangle className="h-4 w-4 text-error" />
            </div>
          )}
          <div>
            <h3 className="text-sm font-semibold text-text-primary">{title}</h3>
            <p className="mt-1.5 text-xs leading-relaxed text-text-secondary">
              {message}
            </p>
          </div>
        </div>
        {children}
        <div className="flex justify-end gap-2">
          <button onClick={onCancel} className="btn-secondary">
            {cancelLabel || t("common.cancel")}
          </button>
          <button
            onClick={onConfirm}
            className={variant === "danger" ? "btn-danger" : "btn-primary"}
          >
            {confirmLabel || t("common.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
