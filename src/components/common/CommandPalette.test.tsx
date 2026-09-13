import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, fireEvent, waitFor } from "@testing-library/react";
import { CommandPalette } from "./CommandPalette";
import { ToastContainer } from "./Toast";
import { renderWithProviders } from "@/components/test-utils";
import * as api from "@/lib/api";
import { useGatewayStatus, __resetGlobalStoresForTest } from "@/store/global";
import type { GatewayStatus } from "@/types/gateway";

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    listProviders: vi.fn().mockResolvedValue([]),
    startGateway: vi.fn(),
    stopGateway: vi.fn(),
    restartGateway: vi.fn(),
  };
});

function renderPalette() {
  return renderWithProviders(
    <>
      <CommandPalette open onClose={() => {}} />
      <ToastContainer />
    </>
  );
}

describe("CommandPalette gateway actions", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.clearAllMocks();
  });

  it("shows an error toast when starting the gateway fails", async () => {
    vi.mocked(api.startGateway).mockRejectedValue({
      code: "gateway",
      message: "port 8080 already in use",
    });
    renderPalette();

    fireEvent.click(screen.getByText("Start gateway service"));

    expect(
      await screen.findByText("port 8080 already in use")
    ).toBeInTheDocument();
  });

  it("writes the returned status into the gateway status store on success", async () => {
    const running = {
      running: true,
      host: "127.0.0.1",
      port: 8080,
    } as GatewayStatus;
    vi.mocked(api.restartGateway).mockResolvedValue(running);
    renderPalette();

    fireEvent.click(screen.getByText("Restart gateway service"));

    await waitFor(() =>
      expect(useGatewayStatus.getState().value).toEqual(running)
    );
  });
});
