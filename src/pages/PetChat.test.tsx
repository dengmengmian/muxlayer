import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  waitFor,
  screen,
  cleanup,
  fireEvent,
} from "@testing-library/react";

vi.mock("@/lib/api", () => ({
  getPetSettings: vi.fn(() => Promise.resolve({ pet_type: "robot" })),
  getPetChatHistory: vi.fn(() => Promise.resolve("[]")),
  savePetChatHistory: vi.fn((history: string) => Promise.resolve(history)),
  getPetMemory: vi.fn(() => Promise.resolve("{}")),
  savePetMemory: vi.fn(() => Promise.resolve(true)),
  petChat: vi.fn(() => Promise.resolve("hello")),
}));

vi.mock("@/lib/bindings", () => ({
  events: Object.fromEntries(
    ["petSettingsChanged", "petChatUpdated", "petMemoryChanged"].map((key) => [
      key,
      { listen: vi.fn(() => Promise.resolve(() => {})) },
    ])
  ),
}));

import * as api from "@/lib/api";
import { PetChat } from "./PetChat";

describe("PetChat", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Element.prototype.scrollTo = vi.fn();
  });

  afterEach(() => {
    cleanup();
  });

  it("renders pet chat console sections", async () => {
    render(<PetChat />);

    await waitFor(() => expect(api.getPetChatHistory).toHaveBeenCalled());
    expect(screen.getByText("petchat.title")).toBeInTheDocument();
    expect(screen.getByText("petchat.conversation_stream")).toBeInTheDocument();
    expect(screen.getByTestId("pet-memory-panel")).toHaveClass("hidden");
    const memoryButtons = screen.getAllByText("petchat.memory_matrix");
    fireEvent.click(memoryButtons[memoryButtons.length - 1]);
    expect(screen.getByTestId("pet-memory-panel")).not.toHaveClass("hidden");
    expect(screen.getByTestId("pet-chat-page")).toHaveClass("h-full");
  });
});
