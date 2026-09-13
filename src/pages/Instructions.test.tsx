import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor, screen, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");
vi.mock("@/components/tools/ClientHistoryButton", () => ({
  ClientHistoryButton: () => <button>history</button>,
}));

import * as api from "@/lib/api";
import { Instructions } from "./Instructions";

afterEach(() => cleanup());

function status(scope: api.InstructionsScope): api.InstructionsStatus {
  return {
    scope,
    path:
      scope === "claude_global"
        ? "/Users/test/.claude/CLAUDE.md"
        : "/Users/test/.codex/AGENTS.md",
    exists: true,
    content: "# Instructions",
    size_bytes: 14,
  };
}

describe("Instructions", () => {
  beforeEach(() => {
    vi.mocked(api.readGlobalInstructions).mockImplementation(async (scope) =>
      status(scope)
    );
    vi.mocked(api.listInstructionsTemplates).mockResolvedValue([]);
    vi.mocked(api.writeGlobalInstructions).mockImplementation(async (scope) =>
      status(scope)
    );
  });

  it("renders instruction console and target matrix", async () => {
    render(
      <MemoryRouter>
        <Instructions />
      </MemoryRouter>
    );

    await waitFor(() => expect(api.readGlobalInstructions).toHaveBeenCalled());
    expect(screen.getByText("instructions.title")).toBeInTheDocument();
    expect(screen.getByText("instructions.target_matrix")).toBeInTheDocument();
    expect(screen.getByText("instructions.editor")).toBeInTheDocument();
  });
});
