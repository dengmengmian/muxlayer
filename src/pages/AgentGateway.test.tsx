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
    copy_json:
      '{"mcpServers":{"muxlayer":{"command":"/opt/homebrew/bin/agentgate-serve","args":["mcp-agent"]}}}',
    copy_toml: '[[mcp_servers]]\nname = "muxlayer"',
    compatibility: "Compatibility details",
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
      running: true,
      service_version: "2.0.8",
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

function manager(
  overrides: Partial<api.GatewayManagerStatus> = {}
): api.GatewayManagerStatus {
  return {
    model: null,
    model_error: null,
    model_start_error: null,
    model_starting: false,
    model_auto_start: true,
    agent: overview().service,
    agent_start_error: null,
    agent_starting: false,
    agent_auto_start: true,
    bundled_cli: "/Users/me/.muxlayer/bin/agentgate-serve",
    ...overrides,
  };
}

describe("AgentGateway product flow", () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem("agentgate_locale", "en");
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(overview());
    vi.mocked(api.getGatewayManagerStatus).mockResolvedValue(manager());
    vi.mocked(api.setAgentGatewayAgentPath).mockResolvedValue({
      status: agent("codex-native", { configured_path: "/new/codex" }),
      requires_service_restart: false,
    });
    vi.mocked(api.setAgentGatewayServePath).mockResolvedValue(overview().cli);
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
      status: client("codeleveler", {
        config_status: "configured",
        configured: true,
        managed: true,
        matching: true,
      }),
      message: "saved",
    });
    vi.mocked(api.removeAgentGatewayMcp).mockResolvedValue({
      status: client("codeleveler"),
      message: "removed",
    });
    vi.mocked(api.getAgentGatewayDiagnostics).mockResolvedValue("diag text");
    vi.mocked(api.openAgentGatewayDataDir).mockResolvedValue(true);
  });

  it("explains the purpose and shows the three-step flow", async () => {
    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    expect(
      await screen.findByText("Let your AI assistant use Codex and Claude Code")
    ).toBeInTheDocument();
    expect(screen.getByText("① Choose an available Agent")).toBeInTheDocument();
    expect(screen.getByText("② Connect your app")).toBeInTheDocument();
    expect(screen.getByText("③ Start using it")).toBeInTheDocument();
    expect(screen.getByText(/OpenAI Native/)).toBeInTheDocument();
    expect(screen.getByText(/DeepSeek/)).toBeInTheDocument();
    expect(screen.queryByText("codex-cli 0.162.1")).not.toBeInTheDocument();
  });

  it("shows first-use guidance when no Agent is installed", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        agents: [
          agent("codex-native", {
            installed: false,
            state: "not_installed",
            available: false,
            version: null,
            executable: null,
            source: "Unknown",
            model: null,
          }),
          agent("claude-code", {
            installed: false,
            state: "not_installed",
            available: false,
            version: null,
            executable: null,
            source: "Unknown",
            model: null,
          }),
        ],
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    expect(
      await screen.findByText("No usable Agent detected yet")
    ).toBeInTheDocument();
    expect(screen.getByText("Install Codex")).toBeInTheDocument();
    expect(screen.getByText("Install Claude Code")).toBeInTheDocument();
  });

  it("reveals versions and paths only in details", async () => {
    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("Codex");
    expect(screen.queryByText("codex-cli 0.162.1")).not.toBeInTheDocument();
    fireEvent.click(screen.getAllByText("Details")[0]);
    expect(await screen.findByText("codex-cli 0.162.1")).toBeInTheDocument();
    expect(screen.getAllByText("/opt/homebrew/bin/codex").length).toBeGreaterThan(0);
  });

  it("saves a manual agent path from details", async () => {
    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("Codex");
    fireEvent.click(screen.getAllByText("Edit path")[0]);
    const input = screen.getByPlaceholderText(
      "Absolute path to the CLI (empty = auto-detect)"
    );
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

    await screen.findByText("CodeLeveler");
    fireEvent.click(screen.getAllByText("Configure MCP")[0]);
    await screen.findByText("Configure MCP?");
    const buttons = screen.getAllByText("Configure MCP");
    fireEvent.click(buttons[buttons.length - 1]);

    await waitFor(() =>
      expect(api.configureAgentGatewayMcp).toHaveBeenCalledWith(
        "codeleveler",
        true
      )
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

    expect(await screen.findByText("Verification required")).toBeInTheDocument();
    expect(screen.getAllByText("Test connection")[0]).toBeEnabled();
  });

  it("promotes an unverified Agent after a real successful test", async () => {
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

    await screen.findByText("Verification required");
    fireEvent.click(screen.getAllByText("Test connection")[0]);
    await screen.findByText("Test this agent?");
    const buttons = screen.getAllByText("Test connection");
    fireEvent.click(buttons[buttons.length - 1]);

    expect(
      await screen.findByText("A real call passed; this Agent is ready to use.")
    ).toBeInTheDocument();
    expect(screen.getAllByText("Available").length).toBeGreaterThan(0);
  });

  it("turns a 401 into a user-facing failure while keeping raw details", async () => {
    vi.mocked(api.testAgentGatewayAgent).mockResolvedValue({
      success: false,
      agent: "codex-native",
      state: "failed",
      task_id: "task-401",
      session_id: "session-401",
      final_answer: null,
      error: "unexpected status 401 Unauthorized",
      duration_ms: 100,
      started_service: false,
    });

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("Codex");
    fireEvent.click(screen.getAllByText("Test connection")[0]);
    await screen.findByText("Test this agent?");
    const buttons = screen.getAllByText("Test connection");
    fireEvent.click(buttons[buttons.length - 1]);

    expect(
      await screen.findByText(
        "Authentication failed. Check the Codex / Claude Code login and model configuration."
      )
    ).toBeInTheDocument();
    expect(
      screen.getByText(/This does not necessarily mean the account is invalid/)
    ).toBeInTheDocument();
    expect(screen.getByText("Technical details")).toBeInTheDocument();
    expect(screen.getAllByText(/unexpected status 401/).length).toBeGreaterThan(0);
  });

  it("distinguishes a configured client from a client that cannot run", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        clients: [
          client("codeleveler"),
          client("pi", {
            config_status: "unavailable",
            error: "env: node: No such file or directory",
          }),
        ],
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("Pi Agent");
    expect(screen.getByText("Client unavailable")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Pi was detected, but Node.js is not available in this environment. Make sure Node is installed and resolvable, or set a manual path."
      )
    ).toBeInTheDocument();
    const detailsButtons = screen.getAllByText("Details");
    fireEvent.click(detailsButtons[detailsButtons.length - 1]);
    expect(
      screen.getByText("env: node: No such file or directory")
    ).toBeInTheDocument();
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
          }),
        ],
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("Pi Agent");
    expect(screen.getByText("Update required")).toBeInTheDocument();
    const removeButtons = screen.getAllByText("Remove MCP");
    expect(removeButtons[removeButtons.length - 1]).not.toBeDisabled();
    const configureButtons = screen.getAllByText("Reconfigure");
    expect(configureButtons[configureButtons.length - 1]).not.toBeDisabled();
  });

  it("keeps historical service errors out of the healthy summary", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        service: {
          ...overview().service,
          running: true,
          last_error: "ERROR: database is locked",
        },
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    expect(await screen.findByText("Agent service is healthy")).toBeInTheDocument();
    expect(screen.queryByText("ERROR: database is locked")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Advanced settings and diagnostics"));
    expect(
      screen.getByText("Historical error (current status above takes precedence)")
    ).toBeInTheDocument();
    expect(screen.getByText("ERROR: database is locked")).toBeInTheDocument();
  });

  it("warns before stopping a service with running tasks", async () => {
    vi.mocked(api.getAgentGatewayServiceStatus).mockResolvedValue({
      ...overview().service,
      running: true,
      service_version: "2.0.8",
      running_tasks: ["task-9"],
    });

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("Codex");
    fireEvent.click(screen.getByText("Advanced settings and diagnostics"));
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

  it("offers a copyable instruction only when an Agent and client are ready", async () => {
    vi.mocked(api.getAgentGatewayOverview).mockResolvedValue(
      overview({
        clients: [
          client("codeleveler", {
            config_status: "configured",
            configured: true,
            managed: true,
            matching: true,
          }),
        ],
      })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    expect(
      await screen.findByText(
        "You can now use Agent Gateway from CodeLeveler or Pi. Open the client and enter:"
      )
    ).toBeInTheDocument();
    expect(screen.getByText("Copy instruction")).toBeInTheDocument();
    expect(screen.getAllByText(/agent_run/).length).toBeGreaterThan(0);
  });

  it("toggles Agent auto-start from advanced settings", async () => {
    vi.mocked(api.getGatewayManagerStatus).mockResolvedValue(
      manager({ agent_auto_start: false })
    );

    renderWithProviders(<AgentGateway />, { route: "/agent-gateway" });

    await screen.findByText("Codex");
    fireEvent.click(screen.getByText("Advanced settings and diagnostics"));
    const autoStart = screen.getByLabelText("Start Agent Gateway with Desktop");
    expect(autoStart).not.toBeChecked();
    fireEvent.click(autoStart);
    await waitFor(() =>
      expect(api.setAgentGatewayAutoStart).toHaveBeenCalledWith(true)
    );
  });
});
