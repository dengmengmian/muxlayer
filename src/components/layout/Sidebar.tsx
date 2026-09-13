import { useState, useEffect } from "react";
import { NavLink } from "react-router-dom";
import {
  LayoutDashboard,
  Cloud,
  GitBranch,
  Radio,
  Monitor,
  ScrollText,
  Stethoscope,
  Settings,
  Rocket,
  PanelLeftClose,
  PanelLeftOpen,
  FileText,
  Plug,
  BookOpen,
  MessageCircle,
} from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import logo from "@/assets/logo.svg";
import { cn } from "@/lib/utils";
import { useI18n } from "@/lib/i18n";
import { useProviders } from "@/store/global";

const navGroups = [
  {
    labelKey: "nav.group.overview",
    items: [{ to: "/", labelKey: "nav.overview", icon: LayoutDashboard }],
  },
  {
    labelKey: "nav.group.models",
    items: [
      { to: "/providers", labelKey: "nav.providers", icon: Cloud },
      { to: "/routes", labelKey: "nav.routes", icon: GitBranch },
    ],
  },
  {
    labelKey: "nav.group.gateway",
    items: [
      { to: "/gateway", labelKey: "nav.gateway", icon: Radio },
      { to: "/logs", labelKey: "nav.logs", icon: ScrollText },
      { to: "/diagnostics", labelKey: "nav.diagnostics", icon: Stethoscope },
    ],
  },
  {
    labelKey: "nav.group.tools",
    items: [
      { to: "/tools", labelKey: "nav.clients", icon: Monitor },
      { to: "/instructions", labelKey: "nav.instructions", icon: FileText },
      { to: "/mcp", labelKey: "nav.mcp", icon: Plug },
      { to: "/skills", labelKey: "nav.skills", icon: BookOpen },
      { to: "/pet-chat", labelKey: "nav.pet_chat", icon: MessageCircle },
    ],
  },
  {
    labelKey: "nav.group.system",
    items: [{ to: "/settings", labelKey: "nav.settings", icon: Settings }],
  },
];

export function Sidebar() {
  const { t } = useI18n();
  const [version, setVersion] = useState("");
  const [showQuickSetup, setShowQuickSetup] = useState(false);
  // 折叠状态：用 localStorage 持久化——用户折叠后下次打开仍是折叠的。
  const [collapsed, setCollapsed] = useState<boolean>(
    () => localStorage.getItem("agentgate_sidebar_collapsed") === "1"
  );
  const toggleCollapse = () => {
    const next = !collapsed;
    setCollapsed(next);
    localStorage.setItem("agentgate_sidebar_collapsed", next ? "1" : "0");
  };
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  // Show quick setup only if: no providers AND not manually hidden
  const providers = useProviders((s) => s.items);
  const providersLoading = useProviders((s) => s.loading);
  useEffect(() => {
    // store 在别处可能也触发 fetch（如 Providers 页），这里只在没数据时拉。
    if (providers.length === 0 && !providersLoading) {
      useProviders
        .getState()
        .fetch()
        .catch(() => {});
    }
  }, [providers.length, providersLoading]);
  useEffect(() => {
    if (localStorage.getItem("agentgate_hide_quick_setup") === "1") {
      setShowQuickSetup(false);
      return;
    }
    if (localStorage.getItem("agentgate_show_quick_setup") === "1") {
      setShowQuickSetup(true);
      return;
    }
    // 等首次 fetch 真正返回后再决定——loading 期间不闪 quick-setup banner。
    if (providersLoading) return;
    setShowQuickSetup(providers.length === 0);
  }, [providers, providersLoading]);

  return (
    <aside
      className={cn(
        "flex h-full min-h-0 shrink-0 flex-col border-r border-border bg-sidebar transition-[width] duration-150",
        collapsed ? "w-14" : "w-52"
      )}
    >
      {/* Logo + collapse toggle */}
      <div
        className={cn(
          "flex h-14 items-center border-b border-border",
          collapsed ? "justify-center px-2" : "justify-between px-5"
        )}
      >
        <div className="flex items-center gap-2.5 min-w-0">
          <img
            src={logo}
            alt=""
            className="h-10 w-10 shrink-0"
            aria-hidden="true"
          />
          {!collapsed && (
            <span className="text-sm font-semibold tracking-tight text-text-primary truncate">
              MuxLayer
            </span>
          )}
        </div>
        {!collapsed && (
          <button
            type="button"
            onClick={toggleCollapse}
            title={t("nav.collapse")}
            className="shrink-0 rounded p-1 text-text-muted hover:bg-hover hover:text-text-primary"
          >
            <PanelLeftClose className="h-4 w-4" />
          </button>
        )}
      </div>

      {/* 折叠态：单独一个展开按钮放在 logo 下面，避免点 logo 误折叠 */}
      {collapsed && (
        <button
          type="button"
          onClick={toggleCollapse}
          title={t("nav.expand")}
          className="mt-2 mx-auto rounded p-1.5 text-text-muted hover:bg-hover hover:text-text-primary"
        >
          <PanelLeftOpen className="h-4 w-4" />
        </button>
      )}

      {/* Navigation */}
      <nav
        className={cn(
          "flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto overscroll-contain py-3",
          collapsed ? "px-2" : "px-3"
        )}
      >
        {showQuickSetup && (
          <NavLink
            to="/quick-setup"
            title={collapsed ? t("nav.quick_setup") : undefined}
            className={({ isActive }) =>
              cn(
                "group relative mb-1 flex items-center rounded-lg text-[13px] font-medium transition-all duration-150",
                collapsed ? "justify-center px-2 py-2" : "gap-3 px-3 py-2",
                isActive
                  ? "bg-accent-soft text-accent"
                  : "text-accent hover:bg-accent-soft"
              )
            }
          >
            {({ isActive }) => (
              <>
                {isActive && !collapsed && (
                  <span className="absolute left-0 top-1/2 h-4 w-[3px] -translate-y-1/2 rounded-r-full bg-accent" />
                )}
                <Rocket className="h-4 w-4 shrink-0" />
                {!collapsed && t("nav.quick_setup")}
              </>
            )}
          </NavLink>
        )}
        {navGroups.map((group, groupIndex) => (
          <div
            key={group.labelKey}
            className={cn(groupIndex > 0 && (collapsed ? "mt-2" : "mt-3"))}
          >
            {!collapsed && (
              <div className="mb-1 px-3 text-xs font-semibold uppercase tracking-wide text-text-muted">
                {t(group.labelKey)}
              </div>
            )}
            <div className="space-y-0.5">
              {group.items.map((item) => (
                <NavLink
                  key={item.to}
                  to={item.to}
                  end={item.to === "/"}
                  title={collapsed ? t(item.labelKey) : undefined}
                  className={({ isActive }) =>
                    cn(
                      "group relative flex items-center rounded-lg text-[13px] font-medium transition-all duration-150",
                      collapsed
                        ? "justify-center px-2 py-2"
                        : "gap-3 px-3 py-2",
                      isActive
                        ? "bg-accent-soft text-accent"
                        : "text-text-secondary hover:bg-hover hover:text-text-primary"
                    )
                  }
                >
                  {({ isActive }) => (
                    <>
                      {isActive && !collapsed && (
                        <span className="absolute left-0 top-1/2 h-4 w-[3px] -translate-y-1/2 rounded-r-full bg-accent" />
                      )}
                      <item.icon className="h-4 w-4 shrink-0" />
                      {!collapsed && t(item.labelKey)}
                    </>
                  )}
                </NavLink>
              ))}
            </div>
          </div>
        ))}
      </nav>

      {/* Footer */}
      {!collapsed && (
        <div className="shrink-0 border-t border-border px-5 py-3.5">
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs font-medium text-text-secondary">
              MuxLayer
            </span>
            {version && (
              <span className="rounded bg-hover px-1.5 py-0.5 font-mono text-xs text-text-muted">
                v{version}
              </span>
            )}
          </div>
        </div>
      )}
    </aside>
  );
}
