import { useEffect, lazy } from "react";
import { BrowserRouter, Routes, Route, useNavigate } from "react-router-dom";
import { events } from "@/lib/bindings";
import { I18nProvider } from "@/lib/i18n";
import { AppShell } from "@/components/layout/AppShell";
import { ToastContainer } from "@/components/common/Toast";
import { UpdateChecker } from "@/components/common/UpdateChecker";
// Dashboard 是首屏，保持同步加载；其余页面按路由拆 chunk——
// react-markdown 等重依赖只被 Logs / Instructions 引用，跟着页面一起拆出主包。
import { Dashboard } from "@/pages/Dashboard";

const Tools = lazy(() =>
  import("@/pages/Tools").then((m) => ({ default: m.Tools }))
);
const Providers = lazy(() =>
  import("@/pages/Providers").then((m) => ({ default: m.Providers }))
);
const ProviderDetail = lazy(() =>
  import("@/pages/ProviderDetail").then((m) => ({ default: m.ProviderDetail }))
);
const RoutesPage = lazy(() =>
  import("@/pages/Routes").then((m) => ({ default: m.Routes }))
);
const Gateway = lazy(() =>
  import("@/pages/Gateway").then((m) => ({ default: m.Gateway }))
);
const Logs = lazy(() =>
  import("@/pages/Logs").then((m) => ({ default: m.Logs }))
);
const Diagnostics = lazy(() =>
  import("@/pages/Diagnostics").then((m) => ({ default: m.Diagnostics }))
);
const Settings = lazy(() =>
  import("@/pages/Settings").then((m) => ({ default: m.Settings }))
);
const QuickSetup = lazy(() =>
  import("@/pages/QuickSetup").then((m) => ({ default: m.QuickSetup }))
);
const Instructions = lazy(() =>
  import("@/pages/Instructions").then((m) => ({ default: m.Instructions }))
);
const Mcp = lazy(() => import("@/pages/Mcp").then((m) => ({ default: m.Mcp })));
const AgentGateway = lazy(() =>
  import("@/pages/AgentGateway").then((m) => ({ default: m.AgentGateway }))
);
const Skills = lazy(() =>
  import("@/pages/Skills").then((m) => ({ default: m.Skills }))
);
const PetChat = lazy(() =>
  import("@/pages/PetChat").then((m) => ({ default: m.PetChat }))
);

/// 宠物右键菜单会发页面导航事件,这里统一跳过去。
function PetEventBridge() {
  const navigate = useNavigate();
  useEffect(() => {
    const unSettings = events.petOpenSettings.listen(() =>
      navigate("/settings?tab=pet")
    );
    const unGateway = events.petOpenGateway.listen(() => navigate("/gateway"));
    const unLogs = events.petOpenLogs.listen(() =>
      navigate("/logs?source=gateway")
    );
    return () => {
      unSettings.then((fn) => fn());
      unGateway.then((fn) => fn());
      unLogs.then((fn) => fn());
    };
  }, [navigate]);
  return null;
}

export function App() {
  return (
    <I18nProvider>
      <BrowserRouter>
        <PetEventBridge />
        <Routes>
          <Route element={<AppShell />}>
            <Route path="/" element={<Dashboard />} />
            <Route path="/quick-setup" element={<QuickSetup />} />
            <Route path="/tools" element={<Tools />} />
            <Route path="/providers" element={<Providers />} />
            <Route path="/providers/:id" element={<ProviderDetail />} />
            <Route path="/routes" element={<RoutesPage />} />
            <Route path="/gateway" element={<Gateway />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/diagnostics" element={<Diagnostics />} />
            <Route path="/instructions" element={<Instructions />} />
            <Route path="/mcp" element={<Mcp />} />
            <Route path="/agent-gateway" element={<AgentGateway />} />
            <Route path="/skills" element={<Skills />} />
            <Route path="/pet-chat" element={<PetChat />} />
            <Route path="/settings" element={<Settings />} />
          </Route>
        </Routes>
        <ToastContainer />
        <UpdateChecker />
      </BrowserRouter>
    </I18nProvider>
  );
}
