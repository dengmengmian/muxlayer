import { useState, useEffect, Suspense, lazy } from "react";
import { Outlet, useLocation } from "react-router-dom";
import { Sidebar } from "./Sidebar";
import { Topbar } from "./Topbar";
import { ErrorBoundary } from "@/components/common/ErrorBoundary";

const CommandPalette = lazy(() =>
  import("@/components/common/CommandPalette").then((m) => ({
    default: m.CommandPalette,
  }))
);

export function AppShell() {
  // Cmd+K（mac）/ Ctrl+K（其它平台）全局快捷键打开命令面板。
  const [cmdkOpen, setCmdkOpen] = useState(false);
  const { pathname } = useLocation();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setCmdkOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-bg">
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <Topbar onOpenCmdK={() => setCmdkOpen(true)} />
        <main className="min-w-0 flex-1 overflow-y-auto scroll-smooth p-5">
          <div className="h-full min-w-0 animate-fade-in">
            {/* 懒加载页面 chunk 时只替换内容区，侧边栏 / 顶栏保持不动。
                本地磁盘加载 chunk 是毫秒级，骨架几乎不可见，仅防白屏。 */}
            <ErrorBoundary key={pathname}>
              <Suspense fallback={<div className="skeleton h-40 w-full" />}>
                <Outlet />
              </Suspense>
            </ErrorBoundary>
          </div>
        </main>
      </div>
      {cmdkOpen && (
        <Suspense fallback={null}>
          <CommandPalette open={cmdkOpen} onClose={() => setCmdkOpen(false)} />
        </Suspense>
      )}
    </div>
  );
}
