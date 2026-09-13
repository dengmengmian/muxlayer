import { clsx, type ClassValue } from "clsx";

export function cn(...inputs: ClassValue[]) {
  return clsx(inputs);
}

export function formatTimestamp(iso: string, locale: string = "en-US"): string {
  const d = new Date(iso);
  const loc = locale === "zh" ? "zh-CN" : locale;
  const month = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  const time = d.toLocaleTimeString(loc, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
  return `${month}-${day} ${time}`;
}

export function formatLatency(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function formatOptionalLatency(ms: number | null): string {
  if (ms === null || ms <= 0) return "—";
  return formatLatency(ms);
}

/// 金额展示：null/undefined 表示未知（无定价）→ "—"；≤0 → "$0.00"；
/// 不足 1 美分保留 4 位，不足 1 美元保留 3 位，其余 2 位。
export function formatCost(cost: number | null | undefined): string {
  if (cost == null) return "—";
  if (cost <= 0) return "$0.00";
  if (cost < 0.01) return `$${cost.toFixed(4)}`;
  if (cost < 1) return `$${cost.toFixed(3)}`;
  return `$${cost.toFixed(2)}`;
}
