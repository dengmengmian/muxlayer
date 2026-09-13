import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, waitFor } from "@testing-library/react";

vi.mock("@/lib/api");
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

import * as api from "@/lib/api";
import { PostApplyDialog } from "./PostApplyDialog";
import { ToastContainer } from "@/components/common/Toast";

describe("PostApplyDialog", () => {
  beforeEach(() => {
    vi.mocked(api.killClientProcess).mockResolvedValue(undefined as never);
  });

  it("kills the listed process instead of only copying a command", async () => {
    render(
      <PostApplyDialog
        open
        clientId="deepseek_harness"
        clientName="DeepSeek Harness"
        configPath="/tmp/settings.yaml"
        processes={[{ pid: 58313, command: "dsh" }]}
        onClose={() => {}}
      />
    );

    const btn = screen.getByRole("button", { name: /tools.post_apply.kill/ });
    await act(async () => btn.click());

    await waitFor(() =>
      expect(api.killClientProcess).toHaveBeenCalledWith(
        "deepseek_harness",
        58313
      )
    );
  });

  it("tells the user to restart ChatGPT Desktop themselves after restarting Codex", async () => {
    // 重启 Codex 不再顺手结束 ChatGPT 桌面端;它在跑时要提示用户自己重启。
    vi.mocked(api.codexDesktopAvailable).mockResolvedValue(true);
    vi.mocked(api.restartCodexDesktop).mockResolvedValue({
      supported: true,
      platform: "macos",
      was_running: true,
      killed: 1,
      relaunched: true,
      chatgpt_needs_manual_restart: true,
    });
    render(
      <>
        <PostApplyDialog
          open
          clientId="codex"
          clientName="Codex"
          configPath="/tmp/config.toml"
          processes={[]}
          onClose={() => {}}
        />
        <ToastContainer />
      </>
    );

    const btn = await screen.findByRole("button", {
      name: /tools.post_apply.restart_codex/,
    });
    await act(async () => btn.click());

    expect(
      await screen.findByText("tools.post_apply.chatgpt_manual_restart")
    ).toBeInTheDocument();
  });

  it("hides the Codex restart button when Codex Desktop isn't installed", async () => {
    // Linux、只装了 CLI、或 Codex 已并入 ChatGPT.app 时,点重启只会报错。
    vi.mocked(api.codexDesktopAvailable).mockResolvedValue(false);
    render(
      <PostApplyDialog
        open
        clientId="codex"
        clientName="Codex"
        configPath="/tmp/config.toml"
        processes={[{ pid: 42, command: "codex" }]}
        onClose={() => {}}
      />
    );

    await waitFor(() => expect(api.codexDesktopAvailable).toHaveBeenCalled());
    expect(
      screen.queryByRole("button", { name: /tools.post_apply.restart_codex/ })
    ).toBeNull();
  });
});
