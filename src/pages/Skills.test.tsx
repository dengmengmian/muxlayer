import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  waitFor,
  screen,
  fireEvent,
  cleanup,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");

import * as api from "@/lib/api";
import { Skills } from "./Skills";

afterEach(() => cleanup());

function skill(id: string, source: string): api.Skill {
  return {
    id,
    source,
    name: id,
    description: "Local skill",
    enabled: true,
    path: `/tmp/${source}/${id}`,
  };
}

describe("Skills", () => {
  beforeEach(() => {
    vi.mocked(api.listSkills).mockResolvedValue([
      skill("writer", "claude"),
      skill("reviewer", "codex"),
    ]);
    vi.mocked(api.exportSkills).mockResolvedValue({
      version: 1,
      skills: [],
      skipped_files: [],
    });
    vi.mocked(api.importSkills).mockResolvedValue([]);
    vi.mocked(api.setSkillEnabled).mockImplementation(async (source, id) =>
      skill(id, source)
    );
    vi.mocked(api.deleteSkill).mockResolvedValue(true);
  });

  it("renders skill console and source matrix", async () => {
    render(
      <MemoryRouter>
        <Skills />
      </MemoryRouter>
    );

    await waitFor(() => expect(api.listSkills).toHaveBeenCalled());
    expect(screen.getByText("skills.title")).toBeInTheDocument();
    expect(screen.getByText("skills.source_matrix")).toBeInTheDocument();
    expect(screen.getByText("writer")).toBeInTheDocument();
  });

  it("exposes OpenCode and Gemini as filter and zip-import sources", async () => {
    vi.mocked(api.listSkills).mockResolvedValue([
      skill("writer", "claude"),
      skill("reviewer", "codex"),
      skill("search", "opencode"),
      skill("summarize", "gemini"),
    ]);

    render(
      <MemoryRouter>
        <Skills />
      </MemoryRouter>
    );

    await screen.findByText("search");
    expect(
      screen.getByRole("option", { name: "OpenCode" })
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: "Gemini CLI" })
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /OpenCode/ })
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Gemini CLI/ })
    ).toBeInTheDocument();
  });

  it("filters the list by OpenCode and Gemini sources", async () => {
    vi.mocked(api.listSkills).mockResolvedValue([
      skill("writer", "claude"),
      skill("search", "opencode"),
      skill("summarize", "gemini"),
    ]);

    render(
      <MemoryRouter>
        <Skills />
      </MemoryRouter>
    );

    await screen.findByText("writer");
    fireEvent.click(screen.getByRole("button", { name: /Gemini CLI/ }));
    expect(screen.getByText("summarize")).toBeInTheDocument();
    expect(screen.queryByText("writer")).not.toBeInTheDocument();
    expect(screen.queryByText("search")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /OpenCode/ }));
    expect(screen.getByText("search")).toBeInTheDocument();
    expect(screen.queryByText("summarize")).not.toBeInTheDocument();
    expect(screen.queryByText("writer")).not.toBeInTheDocument();
  });
});
