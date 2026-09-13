import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { I18nProvider, useI18n } from "./i18n";
import { en } from "./i18n/en";
import { zh } from "./i18n/zh";

function TestComponent() {
  const { locale, setLocale, t } = useI18n();
  return (
    <div>
      <span data-testid="locale">{locale}</span>
      <span data-testid="dashboard">{t("dashboard.gateway", "fallback")}</span>
      <span data-testid="missing">{t("missing.key", "fallback")}</span>
      <button onClick={() => setLocale("zh")}>switch</button>
    </div>
  );
}

describe("I18nProvider", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    localStorage.clear();
  });

  it("defaults to navigator language when no saved preference", () => {
    render(
      <I18nProvider>
        <TestComponent />
      </I18nProvider>
    );
    const expectedLocale = navigator.language.startsWith("zh") ? "zh" : "en";
    expect(screen.getByTestId("locale").textContent).toBe(expectedLocale);
  });

  it("uses saved locale from localStorage", () => {
    localStorage.setItem("agentgate_locale", "zh");
    render(
      <I18nProvider>
        <TestComponent />
      </I18nProvider>
    );
    expect(screen.getByTestId("locale").textContent).toBe("zh");
    expect(screen.getByTestId("dashboard").textContent).toBe("网关");
  });

  it("translates known keys and falls back for unknown keys", () => {
    render(
      <I18nProvider>
        <TestComponent />
      </I18nProvider>
    );
    expect(screen.getByTestId("dashboard").textContent).toBe("Gateway");
    expect(screen.getByTestId("missing").textContent).toBe("fallback");
  });

  it("switches locale and persists to localStorage", async () => {
    render(
      <I18nProvider>
        <TestComponent />
      </I18nProvider>
    );
    fireEvent.click(screen.getByText("switch"));
    await waitFor(() => {
      expect(screen.getByTestId("locale").textContent).toBe("zh");
    });
    expect(screen.getByTestId("dashboard").textContent).toBe("网关");
    expect(localStorage.getItem("agentgate_locale")).toBe("zh");
  });

  it("returns the key itself when no translation and no fallback", () => {
    function NoFallback() {
      const { t } = useI18n();
      return <span>{t("no.such.key")}</span>;
    }
    render(
      <I18nProvider>
        <NoFallback />
      </I18nProvider>
    );
    expect(screen.getByText("no.such.key")).toBeInTheDocument();
  });
});

describe("translation tables", () => {
  it("en and zh define exactly the same keys", () => {
    expect(Object.keys(zh).sort()).toEqual(Object.keys(en).sort());
  });

  it("has no empty strings in either locale", () => {
    // 英文分页 "Page 2 / 5" 没有后缀，中文是「第 2 / 5 页」——唯一有意为空的值。
    const intentionallyEmpty = new Set(["en:logs.page_suffix"]);
    const empty = [
      ...Object.entries(en).map(([k, v]) => [`en:${k}`, v] as const),
      ...Object.entries(zh).map(([k, v]) => [`zh:${k}`, v] as const),
    ]
      .filter(([k, v]) => v.trim() === "" && !intentionallyEmpty.has(k))
      .map(([k]) => k);
    expect(empty).toEqual([]);
  });
});
