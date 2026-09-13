import React, { Suspense, lazy } from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { App } from "./app/App";
import { applyStoredTheme } from "./lib/theme";
import "./index.css";

// 宠物窗口和主窗口共用一个 bundle、按 window label 分流——懒加载让
// 主窗口不用解析 PetApp 代码，宠物窗口也只多加载自己那个小 chunk。
const PetApp = lazy(() =>
  import("./pet/PetApp").then((m) => ({ default: m.PetApp }))
);

// Keep one brand palette with system-like light/dark appearance. Older named
// palettes are migrated once so stale storage cannot resurrect retired colors.
applyStoredTheme();

const label = getCurrentWindow().label;

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {label === "pet" ? (
      <Suspense fallback={null}>
        <PetApp />
      </Suspense>
    ) : (
      <App />
    )}
  </React.StrictMode>
);
