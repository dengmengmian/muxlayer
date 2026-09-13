import { useState, useEffect, useRef, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import {
  Search,
  LayoutDashboard,
  Cloud,
  Monitor,
  Radio,
  GitBranch,
  ScrollText,
  Stethoscope,
  Settings,
  Rocket,
  Play,
  Square,
  RotateCcw,
  Sun,
  Moon,
} from "lucide-react";
import * as api from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { useGatewayStatus, useProviders } from "@/store/global";
import { toast } from "@/components/common/Toast";
import type { GatewayStatus } from "@/types/gateway";

interface Action {
  id: string;
  /// 显示的标题（中英文双语）
  title: string;
  /// 副标题/类别（如 "页面" / "供应商" / "操作"）
  hint?: string;
  icon: React.ComponentType<{ className?: string }>;
  /// 搜索时匹配的额外关键词（拼音首字母等）
  keywords?: string;
  run: () => void | Promise<void>;
}

/// Cmd+K / Ctrl+K 全局快速跳转面板。
///
/// 设计原则：**最常用的动作放最前面**——8 个页面跳转 + 4 个 gateway 控制 +
/// N 个 provider 编辑。模糊匹配 title / keywords，按相关度排序。
///
/// 触发方式：
/// - macOS：Cmd+K
/// - Windows/Linux：Ctrl+K
///
/// 数据来源：
/// - 页面列表：硬编码（与 sidebar 一致）
/// - Provider 列表：listProviders() 首次打开时加载一次
/// - 操作：start/stop/restart gateway、切主题
///
/// 没做 request log 跳转——log 跳转入口在 Logs 页 keyword 搜索更合适。
export function CommandPalette({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const providerItems = useProviders((s) => s.items);
  const [focused, setFocused] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  // 打开时自动 focus 搜索框 + reset query
  useEffect(() => {
    if (open) {
      setQuery("");
      setFocused(0);
      // requestAnimationFrame 确保 input 已挂载
      requestAnimationFrame(() => inputRef.current?.focus());
      // 顺手刷一下 provider 候选——用户可能刚加新的；store 内部防重入,
      // 多次打开 palette 不会真的多发 invoke。
      useProviders
        .getState()
        .refetch()
        .catch(() => {});
    }
  }, [open]);

  const providers = useMemo(
    () => providerItems.map((p) => ({ id: p.id, name: p.name })),
    [providerItems]
  );

  const actions: Action[] = useMemo(() => {
    const navActions: Action[] = [
      {
        id: "nav:overview",
        title: t("nav.overview"),
        hint: t("cmdk.page"),
        icon: LayoutDashboard,
        keywords: "overview dashboard 概览 首页 home",
        run: () => navigate("/"),
      },
      {
        id: "nav:providers",
        title: t("nav.providers"),
        hint: t("cmdk.page"),
        icon: Cloud,
        keywords: "providers 供应商 服务商 上游 ai",
        run: () => navigate("/providers"),
      },
      {
        id: "nav:clients",
        title: t("nav.clients"),
        hint: t("cmdk.page"),
        icon: Monitor,
        keywords: "clients tools 客户端 codex claude",
        run: () => navigate("/tools"),
      },
      {
        id: "nav:gateway",
        title: t("nav.gateway"),
        hint: t("cmdk.page"),
        icon: Radio,
        keywords: "gateway service 服务 端口 port",
        run: () => navigate("/gateway"),
      },
      {
        id: "nav:routes",
        title: t("nav.routes"),
        hint: t("cmdk.page"),
        icon: GitBranch,
        keywords: "routes routing 路由 failover 失败转移",
        run: () => navigate("/routes"),
      },
      {
        id: "nav:logs",
        title: t("nav.logs"),
        hint: t("cmdk.page"),
        icon: ScrollText,
        keywords: "logs 日志 requests 请求",
        run: () => navigate("/logs"),
      },
      {
        id: "nav:diagnostics",
        title: t("nav.diagnostics"),
        hint: t("cmdk.page"),
        icon: Stethoscope,
        keywords: "diagnostics 诊断 health 健康",
        run: () => navigate("/diagnostics"),
      },
      {
        id: "nav:settings",
        title: t("nav.settings"),
        hint: t("cmdk.page"),
        icon: Settings,
        keywords: "settings 设置 preferences",
        run: () => navigate("/settings"),
      },
      {
        id: "nav:quick-setup",
        title: t("nav.quick_setup"),
        hint: t("cmdk.page"),
        icon: Rocket,
        keywords: "quick setup wizard 快速 引导",
        run: () => navigate("/quick-setup"),
      },
    ];
    const gatewayActions: Action[] = [
      {
        id: "act:start",
        title: t("cmdk.start_gateway"),
        hint: t("cmdk.action"),
        icon: Play,
        keywords: "start gateway 启动",
        run: () => runGatewayCommand(api.startGateway),
      },
      {
        id: "act:stop",
        title: t("cmdk.stop_gateway"),
        hint: t("cmdk.action"),
        icon: Square,
        keywords: "stop gateway 停止",
        run: () => runGatewayCommand(api.stopGateway),
      },
      {
        id: "act:restart",
        title: t("cmdk.restart_gateway"),
        hint: t("cmdk.action"),
        icon: RotateCcw,
        keywords: "restart gateway 重启",
        run: () => runGatewayCommand(api.restartGateway),
      },
    ];
    const themeActions: Action[] = [
      {
        id: "act:theme-dark",
        title: t("cmdk.theme_dark"),
        hint: t("cmdk.action"),
        icon: Moon,
        keywords: "theme dark 暗色 主题",
        run: () => {
          document.documentElement.setAttribute("data-theme", "dark");
          localStorage.setItem("agentgate_theme", "dark");
        },
      },
      {
        id: "act:theme-light",
        title: t("cmdk.theme_light"),
        hint: t("cmdk.action"),
        icon: Sun,
        keywords: "theme light 亮色 主题",
        run: () => {
          document.documentElement.setAttribute("data-theme", "light");
          localStorage.setItem("agentgate_theme", "light");
        },
      },
    ];
    const providerActions: Action[] = providers.map((p) => ({
      id: `provider:${p.id}`,
      title: p.name,
      hint: t("cmdk.edit_provider"),
      icon: Cloud,
      keywords: p.name,
      run: () => navigate("/providers"),
    }));
    return [
      ...navActions,
      ...gatewayActions,
      ...themeActions,
      ...providerActions,
    ];
  }, [t, navigate, providers]);

  // 模糊匹配：所有 query 字符按顺序出现在 title+keywords 即匹配。
  // 然后按"title 前缀匹配 > title 包含 > keywords 包含"排序。
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return actions;
    return actions
      .map((a) => {
        const title = a.title.toLowerCase();
        const kw = (a.keywords ?? "").toLowerCase();
        let score;
        if (title.startsWith(q)) score = 100;
        else if (title.includes(q)) score = 50;
        else if (kw.includes(q)) score = 20;
        else if (fuzzyMatch(title + " " + kw, q)) score = 10;
        else score = 0;
        return { a, score };
      })
      .filter((x) => x.score > 0)
      .sort((x, y) => y.score - x.score)
      .map((x) => x.a);
  }, [actions, query]);

  // focused 越界保护
  useEffect(() => {
    if (focused >= filtered.length) setFocused(0);
  }, [filtered.length, focused]);

  // 键盘：Esc 关、↑↓ 移动、Enter 执行
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setFocused((i) => Math.min(i + 1, filtered.length - 1));
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setFocused((i) => Math.max(i - 1, 0));
      }
      if (e.key === "Enter") {
        e.preventDefault();
        const a = filtered[focused];
        if (a) {
          a.run();
          onClose();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, filtered, focused, onClose]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[200] flex items-start justify-center pt-[15vh]">
      <div
        className="fixed inset-0 bg-black/40 backdrop-blur-sm"
        onClick={onClose}
      />
      <div
        className="animate-scale-in relative z-10 w-full max-w-xl rounded-xl border border-border bg-card"
        style={{ boxShadow: "var(--shadow-lg)" }}
      >
        <div className="flex items-center gap-3 border-b border-border px-4 py-3">
          <Search className="h-4 w-4 shrink-0 text-text-muted" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("cmdk.placeholder")}
            className="flex-1 bg-transparent text-sm text-text-primary outline-none placeholder:text-text-muted"
          />
          <kbd className="rounded border border-border bg-card-secondary px-1.5 py-0.5 text-[10px] text-text-muted">
            Esc
          </kbd>
        </div>
        <div className="max-h-80 overflow-y-auto py-1">
          {filtered.length === 0 ? (
            <p className="px-4 py-6 text-center text-xs text-text-muted">
              {t("cmdk.no_results")}
            </p>
          ) : (
            filtered.map((a, i) => {
              const Icon = a.icon;
              const isFocused = i === focused;
              return (
                <button
                  key={a.id}
                  type="button"
                  onClick={() => {
                    a.run();
                    onClose();
                  }}
                  onMouseEnter={() => setFocused(i)}
                  className={`flex w-full items-center gap-3 px-4 py-2 text-left text-sm transition-colors ${
                    isFocused
                      ? "bg-accent-soft text-accent"
                      : "text-text-primary hover:bg-hover"
                  }`}
                >
                  <Icon className="h-4 w-4 shrink-0" />
                  <span className="flex-1 truncate">{a.title}</span>
                  {a.hint && (
                    <span className="shrink-0 text-[11px] text-text-muted">
                      {a.hint}
                    </span>
                  )}
                </button>
              );
            })
          )}
        </div>
        <div className="flex items-center gap-3 border-t border-border px-4 py-2 text-[11px] text-text-muted">
          <span>
            <kbd className="rounded border border-border bg-card-secondary px-1 text-[10px]">
              ↑
            </kbd>{" "}
            <kbd className="rounded border border-border bg-card-secondary px-1 text-[10px]">
              ↓
            </kbd>{" "}
            {t("cmdk.navigate")}
          </span>
          <span>
            <kbd className="rounded border border-border bg-card-secondary px-1 text-[10px]">
              ↵
            </kbd>{" "}
            {t("cmdk.select")}
          </span>
        </div>
      </div>
    </div>
  );
}

/// start/stop/restart 返回的新状态直接写进全局 store（Topbar 等订阅者即时更新）；
/// 失败给用户一个 error toast。
async function runGatewayCommand(command: () => Promise<GatewayStatus>) {
  try {
    useGatewayStatus.getState().setValue(await command());
  } catch (err) {
    toast("error", (err as api.AppError).message);
  }
}

/// 简单模糊匹配：query 的每个字符按顺序出现在 text 即匹配。
function fuzzyMatch(text: string, query: string): boolean {
  let qi = 0;
  for (let ti = 0; ti < text.length && qi < query.length; ti++) {
    if (text[ti] === query[qi]) qi++;
  }
  return qi === query.length;
}
