import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, waitFor, fireEvent, cleanup } from "@testing-library/react";

vi.mock("@/lib/api");

import * as api from "@/lib/api";
import { renderWithProviders } from "@/components/test-utils";
import { AgentGateway } from "./AgentGateway";

afterEach(() => cleanup());

function agent(
  id: "codex-native" | "claude-code",
  overrides: Partial<api.AgentGatewayAgentStatus> = {}
): api.AgentGatewayAgentStatus {
  return {
    id,
    name: id === "codex-native" ? "Codex" : "Claude Code",
    installed: true,
    executable:
      id === "codex-native"
        ? "/opt/homebrew/bin/codex"
        : "/Users/me/.local/bin/claude",
    version: id === "codex-native" ? "codex-cli 0.162.1" : "2.1.302 (Claude Code)",
    state: "available",
    available: true,
    auth_state: "authenticated",
    auth_detail: "Logged in",
    source: id === "codex-native" ? "OpenAI Native" : "DeepSeek",
    model: id === "codex-native" ? "gpt-6.1-sol" : "deepseek-flash[1m]",
    config_path:
      id === "codex-native"
        ? "/Users/me/.codex/config.toml"
        : "/Users/me/.claude/settings.json",
    configured_path: null,
    detection: "common",
    warnings: [],
    error: null,
    ...overrides,
  };
}

function client(
  id: "codeleveler" | "pi",
  overrides: Partial<api.AgentGatewayClientStatus> = {}
): api.AgentGatewayClientStatus {
  return {
    id,
    name: id === "pi" ? "Pi Agent" : "CodeLeveler",
    installed: true,
    version: id === "pi" ? "1.3.0" : "1.0.16",
    executable: id === "pi" ? "/usr/local/bin/pi" : "/opt/homebrew/bin/leveler",
    config_path:
      id === "pi" ? "/Users/me/.pi/agent/mcp.json" : "/Users/me/.leveler/config.toml",
    config_exists: true,
    mcp_name: "muxlayer",
    config_status: "not_configured",
    configured: false,
    managed: false,
    matching: false,
    command: null,
    args: [],
    copy_command: "/opt/homebrew/bin/agentgate-serve",
    copy_args: ["mcp-agent"],
    copy_json: '{"mcpServers":{"muxlayer":{"command":"/opt/homebrew/bin/agentgate-serve","args":["mcp-agent"]}}}',
    copy_toml: '[[mcp_servers]]\nname = "muxlayer"',
    compatibility: "old",
    requires_upgrade: id === "codeleveler",
    warning:
      id === "codeleveler"
        ? "This version predates the MCP isError fix."
        : null,
    error: null,
    live_status_available: false,
    live_status_note: "Configuration presence is not an online status.",
    ...overrides,
  };
}

function overview(
  overrides: Partial<api.AgentGatewayOverview> = {}
): api.AgentGatewayOverview {
  return {
    cli: {
      installed: true,
      path: "/opt/homebrew/bin/agentgate-serve",
      version: "agentgate 2.0.8",
      detection: "common",
      error: null,
      install_hint: "install",
      download_url: "https://example.test",
    },
    data_dir: "/Users/me/.muxlayer",
    agents: [agent("codex-native"), agent("claude-code")],
    service: {
      installed: true,
      cli_path: "/opt/homebrew/bin/agentgate-serve",
      cli_version: "agentgate 2.0.8",
      data_dir: "/Users/me/.muxlayer",
      running: false,
      service_version: null,
      running_tasks: [],
      agents: [],
      last_error: null,
      error: null,
      install_hint: "install",
      download_url: "https://example.test",
    },
    clients: [client("codeleveler"), client("pi")],
    ...overrides,
  };
}

describe("AgentGateway", () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem("agentgate_locale", "en");
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(overview());
    vi.mocked(api.getGatewayManagerStatus).mockResolvedValue({
      model: null,
      model_error: null,
      model_auto_start: true,
      agent: overview().service,
      agent_auto_start: true,
      bundled_cli: "/Users/me/.muxlayer/bin/agentgate-serve",
    });
    vi.mocked(api.setAgentGatewayAgentPath).mockResolvedValue({
      status: agent("codex-native", { configured_path: "/new/codex" }),
      requires_service_restart: false,
    });
    vi.mocked(api.setAgentGatewayServePath).mockResolvedValue(
      overview().cli
    );
    vi.mocked(api.setAgentGatewayAutoStart).mockResolvedValue(true);
    vi.mocked(api.testAgentGatewayAgent).mockResolvedValue({
      success: true,
      agent: "codex-native",
      state: "completed",
      task_id: "task-1",
      session_id: "session-1",
      final_answer: "AGENTGATE_OK",
      error: null,
      duration_ms: 42,
      started_service: false,
    });
    vi.mocked(api.getAgentGatewayServiceStatus).mockResolvedValue(
      overview().service
    );
    vi.mocked(api.startAgentGatewayService).mockResolvedValue(
      overview().service
    );
    vi.mocked(api.stopAgentGatewayService).mockResolvedValue(
      overview().service
    );
    vi.mocked(api.restartAgentGatewayService).mockResolvedValue(
      overview().service
    );
    vi.mocked(api.configureAgentGatewayMcp).mockResolvedValue({
      status: client("codeleveler", { config_status: "configured", matching: true }),
      message: "saved",
    });
    vi.mocked(api.removeAgentGatewayMcp).mockResolvedValue({
      status: client("codeleveler"),
      message: "removed",
    });
    vi.mocked(api.getAgentGatewayDiagnostics).mockResolvedValue("diag text");
    vi.mocked(api.openAgentGatewayDataDir).mockResolvedValue(true);
  });

  it("renders real agent and client status", async () => {
    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await waitFor(() =>
      expect(api.getAgentGatewayOverview).toHaveBeenCalled()
    );
    expect((await screen.findAllByText("Codex")).length).toBeGreaterThan(0);
    expect(screen.getByText("codex-cli 0.162.1")).toBeInTheDocument();
    expect(screen.getByText("OpenAI Native")).toBeInTheDocument();
    expect(screen.getByText("DeepSeek")).toBeInTheDocument();
    expect(screen.getByText("deepseek-flash[1m]")).toBeInTheDocument();
  });

  it("shows install guidance when the Agent Gateway CLI is missing", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        cli: {
          installed: false,
          path: null,
          version: null,
          detection: "auto",
          error: "not found",
          install_hint: "Download the CLI",
          download_url: "https://example.test",
        },
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    expect(
      (await screen.findAllByText("Agent Gateway CLI not found")).length
    ).toBeGreaterThan(0);
    expect(screen.getByText("Download the CLI")).toBeInTheDocument();
  });

  it("saves a manual agent path", async () => {
    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("codex-cli 0.162.1");
    fireEvent.click(screen.getAllByText("Edit path")[0]);
    const input = screen.getByPlaceholderText("Absolute path to the CLI (empty = auto-detect)");
    fireEvent.change(input, { target: { value: "/new/codex" } });
    fireEvent.click(screen.getByText("Save path"));

    await waitFor(() =>
      expect(api.setAgentGatewayAgentPath).toHaveBeenCalledWith(
        "codex-native",
        "/new/codex"
      )
    );
  });

  it("configures CodeLeveler after confirmation", async () => {
    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("codex-cli 0.162.1");
    fireEvent.click(screen.getByText("Connections"));
    fireEvent.click(screen.getAllByText("Configure MCP")[0]);
    await waitFor(() =>
      expect(screen.getByText("Configure MCP?")).toBeInTheDocument()
    );
    const buttons = screen.getAllByText("Configure MCP");
    fireEvent.click(buttons[buttons.length - 1]);

    await waitFor(() =>
      expect(api.configureAgentGatewayMcp).toHaveBeenCalledWith(
        "codeleveler",
        true
      )
    );
  });

  it("shows Model Gateway and Agent Gateway status independently", async () => {
    vi.mocked(api.getGatewayManagerStatus).mockResolvedValue({
      model: {
        running: true,
        host: "127.0.0.1",
        port: 9090,
        active_provider: "DeepSeek",
        input_protocol: "responses",
        output_protocol: "chat",
        started_at: "2026-10-18T00:00:00Z",
      },
      model_error: null,
      model_auto_start: true,
      agent: {
        ...overview().service,
        running: true,
        error: null,
      },
      agent_auto_start: false,
      bundled_cli: "/Users/me/.muxlayer/bin/agentgate-serve",
    });

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    expect(await screen.findByText("Gateway")).toBeInTheDocument();
    expect(screen.getByText("Model Gateway")).toBeInTheDocument();
    expect(screen.getAllByText("Agent Gateway").length).toBeGreaterThan(0);
    expect(screen.getByText("127.0.0.1:9090")).toBeInTheDocument();
    const autoStart = screen.getByLabelText(
      "Start Agent Gateway with Desktop"
    );
    expect(autoStart).not.toBeChecked();
    fireEvent.click(autoStart);
    await waitFor(() =>
      expect(api.setAgentGatewayAutoStart).toHaveBeenCalledWith(true)
    );
  });

  it("shows unverified Codex auth without claiming it is available", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        agents: [
          agent("codex-native", {
            state: "unverified",
            available: true,
            auth_state: "unknown",
            auth_detail: "",
          }),
          agent("claude-code"),
        ],
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    expect(
      await screen.findByText("Verification required")
    ).toBeInTheDocument();
    expect(screen.getAllByText("Test connection")[0]).toBeEnabled();
  });

  it("distinguishes an installed but non-executable client", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        clients: [
          client("codeleveler"),
          client("pi", {
            installed: false,
            version: null,
            executable: "/Users/me/.nvm/versions/node/v25.2.1/bin/pi",
            error: "env: node: No such file or directory",
          }),
        ],
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("codex-cli 0.162.1");
    fireEvent.click(screen.getByText("Connections"));

    expect(await screen.findByText("Client unavailable")).toBeInTheDocument();
    expect(
      screen.getByText("env: node: No such file or directory")
    ).toBeInTheDocument();
    expect(screen.queryByText("Client not installed")).not.toBeInTheDocument();
  });

  it("allows updating and removing a stale managed MCP entry", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        clients: [
          client("codeleveler"),
          client("pi", {
            config_status: "stale",
            configured: true,
            managed: true,
            matching: false,
            command: "/old/agentgate-serve",
            warning:
              "This MuxLayer entry points to a different agentgate-serve path.",
          }),
        ],
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("codex-cli 0.162.1");
    fireEvent.click(screen.getByText("Connections"));

    expect(await screen.findByText("Update required")).toBeInTheDocument();
    const removeButtons = screen.getAllByText("Remove MCP");
    expect(removeButtons[removeButtons.length - 1]).not.toBeDisabled();
    const configureButtons = screen.getAllByText("Configure MCP");
    expect(configureButtons[configureButtons.length - 1]).not.toBeDisabled();
  });

  it("refreshes service status and warns before stopping running tasks", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        service: {
          ...overview().service,
          running: true,
          service_version: "2.0.8",
        },
      })
    );
    vi.mocked(api.getAgentGatewayServiceStatus).mockResolvedValue({
      ...overview().service,
      running: true,
      service_version: "2.0.8",
      running_tasks: ["task-9"],
    });

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("codex-cli 0.162.1");
    fireEvent.click(screen.getByText("Settings"));
    await waitFor(() =>
      expect(api.getAgentGatewayServiceStatus).toHaveBeenCalled()
    );
    fireEvent.click(screen.getByText("Stop service"));
    expect(
      await screen.findByText(/1 task\(s\) are currently running/)
    ).toBeInTheDocument();
    const stopButtons = screen.getAllByText("Stop service");
    fireEvent.click(stopButtons[stopButtons.length - 1]);

    await waitFor(() =>
      expect(api.stopAgentGatewayService).toHaveBeenCalledWith(true)
    );
  });
});
