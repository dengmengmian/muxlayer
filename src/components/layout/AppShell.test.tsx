import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, act } from "@testing-library/react";
import { Routes, Route } from "react-router-dom";
import { AppShell } from "./AppShell";
import { renderWithProviders } from "@/components/test-utils";
import {
  useProviders,
  useGatewayStatus,
  __resetGlobalStoresForTest,
} from "@/store/global";
import type { ProviderView } from "@/types/provider";
import type { GatewayStatus } from "@/types/gateway";

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("1.4.4"),
}));

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    listProviders: vi.fn().mockResolvedValue([]),
    getGatewayStatus: vi.fn().mockResolvedValue({
      running: true,
      host: "127.0.0.1",
      port: 8080,
    } as GatewayStatus),
  };
});

function LayoutWithPage() {
  return (
    <Routes>
      <Route path="/" element={<AppShell />}>
        <Route index element={<div data-testid="page">Hello World</div>} />
      </Route>
    </Routes>
  );
}

describe("AppShell", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.clearAllMocks();
    useProviders.setState({
      items: [{ id: "p1", name: "DeepSeek" } as ProviderView],
      loading: false,
      error: null,
    });
    useGatewayStatus.setState({
      value: { running: true, host: "127.0.0.1", port: 8080 } as GatewayStatus,
      loading: false,
      error: null,
    });
  });

  it("renders sidebar, topbar and outlet content", async () => {
    await act(async () => {
      renderWithProviders(<LayoutWithPage />, { route: "/" });
    });

    expect(screen.getByTestId("page")).toHaveTextContent("Hello World");
    expect(
      screen.getByRole("heading", { name: "Overview" })
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Overview" })).toBeInTheDocument();
    expect(await screen.findByText("Running")).toBeInTheDocument();
  });

  it("keeps command palette closed by default", async () => {
    await act(async () => {
      renderWithProviders(<LayoutWithPage />, { route: "/" });
    });

    expect(
      screen.queryByPlaceholderText(/Jump to page, run action/i)
    ).not.toBeInTheDocument();
  });
});

function Boom(): never {
  throw new Error("page crashed");
}

describe("AppShell error boundary", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.spyOn(console, "error").mockImplementation(() => {});
    useProviders.setState({ items: [], loading: false, error: null });
    useGatewayStatus.setState({
      value: { running: true, host: "127.0.0.1", port: 8080 } as GatewayStatus,
      loading: false,
      error: null,
    });
  });

  it("keeps sidebar and topbar when a routed page throws", async () => {
    await act(async () => {
      renderWithProviders(
        <Routes>
          <Route path="/" element={<AppShell />}>
            <Route path="/tools" element={<Boom />} />
          </Route>
        </Routes>,
        { route: "/tools" }
      );
    });

    expect(screen.getByText("page crashed")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Overview" })).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Back to overview" })
    ).toBeInTheDocument();
  });
});
