import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  act,
  waitFor,
  screen,
  fireEvent,
  cleanup,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");

import * as api from "@/lib/api";
import { Mcp } from "./Mcp";

afterEach(() => cleanup());

function server(id: string): any {
  return {
    id,
    name: "filesystem",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-filesystem"],
    env: [],
    sources: [{ client: "codex", config_path: "/tmp/codex/mcp.json" }],
    enabled_clients: ["codex"],
    validation: { status: "valid", issues: [] },
  };
}

describe("Mcp", () => {
  beforeEach(() => {
    vi.mocked(api.listMcpServers).mockResolvedValue([server("s1")]);
    vi.mocked(api.upsertMcpServer).mockResolvedValue(server("s1"));
    vi.mocked(api.deleteMcpServer).mockResolvedValue(true);
    vi.mocked(api.syncMcpServer).mockResolvedValue([server("s1")]);
    vi.mocked(api.exportMcpServers).mockResolvedValue("{}");
    vi.mocked(api.importMcpServers).mockResolvedValue([server("s1")]);
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });

  it("renders server list", async () => {
    render(
      <MemoryRouter>
        <Mcp />
      </MemoryRouter>
    );

    await waitFor(() => expect(api.listMcpServers).toHaveBeenCalled());
    expect(screen.getByText("mcp.title")).toBeInTheDocument();
    expect(screen.getByText("mcp.server_matrix")).toBeInTheDocument();
    expect(screen.getByText("filesystem")).toBeInTheDocument();
  });

  it("adds a server", async () => {
    render(
      <MemoryRouter>
        <Mcp />
      </MemoryRouter>
    );

    await screen.findByText("mcp.title");

    const add = screen.getAllByText("mcp.add_server")[0];
    await act(async () => add.click());

    const nameInput = screen.getByLabelText("name") as HTMLInputElement;
    const commandInput = screen.getByLabelText("command") as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: "fetch" } });
    fireEvent.change(commandInput, { target: { value: "uvx" } });

    const save = screen.getByText("common.save");
    await act(async () => save.click());

    await waitFor(() =>
      expect(api.upsertMcpServer).toHaveBeenCalledWith(
        expect.objectContaining({ name: "fetch", command: "uvx" })
      )
    );
  });

  it("renders empty state when there are no servers", async () => {
    vi.mocked(api.listMcpServers).mockResolvedValue([]);

    render(
      <MemoryRouter>
        <Mcp />
      </MemoryRouter>
    );

    expect(await screen.findByText("mcp.empty_title")).toBeInTheDocument();
    expect(screen.queryByText("mcp.server_matrix")).toBeNull();
  });

  it("edits and deletes a server", async () => {
    render(
      <MemoryRouter>
        <Mcp />
      </MemoryRouter>
    );

    await screen.findByText("filesystem");
    await act(async () => screen.getByTitle("common.edit").click());

    const commandInput = screen.getByLabelText("command") as HTMLInputElement;
    fireEvent.change(commandInput, { target: { value: "uvx" } });
    await act(async () => screen.getByText("common.save").click());

    await waitFor(() =>
      expect(api.upsertMcpServer).toHaveBeenCalledWith(
        expect.objectContaining({ name: "filesystem", command: "uvx" })
      )
    );

    await act(async () => screen.getByTitle("common.delete").click());
    await waitFor(() =>
      expect(api.deleteMcpServer).toHaveBeenCalledWith("codex", "filesystem")
    );
  });
});
