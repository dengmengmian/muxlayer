export type AppTheme = "light" | "dark";

const LEGACY_DARK_THEMES = new Set(["dark", "slate", "forest", "violet"]);

export function normalizeTheme(stored: string | null): AppTheme {
  return stored && LEGACY_DARK_THEMES.has(stored) ? "dark" : "light";
}

export function readStoredTheme(storage: Storage = localStorage): AppTheme {
  const stored = storage.getItem("agentgate_theme");
  const normalized = normalizeTheme(stored);
  if (stored !== normalized) storage.setItem("agentgate_theme", normalized);
  return normalized;
}

export function applyStoredTheme(
  root: HTMLElement = document.documentElement,
  storage: Storage = localStorage
): AppTheme {
  const theme = readStoredTheme(storage);
  root.setAttribute("data-theme", theme);
  return theme;
}
