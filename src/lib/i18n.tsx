import {
  createContext,
  useContext,
  useState,
  useCallback,
  type ReactNode,
} from "react";
import { en } from "./i18n/en";
import { zh } from "./i18n/zh";

export type Locale = "en" | "zh";

const translations: Record<Locale, Record<string, string>> = { en, zh };

interface I18nContextType {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: string, fallback?: string) => string;
}

const I18nContext = createContext<I18nContextType>({
  locale: "en",
  setLocale: () => {},
  t: (key) => key,
});

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() => {
    const saved = localStorage.getItem("agentgate_locale");
    if (saved === "zh" || saved === "en") return saved;
    return navigator.language.startsWith("zh") ? "zh" : "en";
  });

  const setLocale = useCallback((l: Locale) => {
    setLocaleState(l);
    localStorage.setItem("agentgate_locale", l);
  }, []);

  const t = useCallback(
    (key: string, fallback?: string) => {
      const table = translations[locale];
      if (Object.prototype.hasOwnProperty.call(table, key)) return table[key];
      return fallback ?? key;
    },
    [locale]
  );

  return (
    <I18nContext.Provider value={{ locale, setLocale, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n() {
  return useContext(I18nContext);
}
