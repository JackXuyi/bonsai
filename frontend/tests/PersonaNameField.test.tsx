import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

const setMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    getPersona: async () => ({ name: "大肥鱼" }),
    setPersonaName: (name: string) => setMock(name),
  };
});

import { PersonaNameField } from "@/components/settings/PersonaNameField";
import { usePersonaStore } from "@/lib/stores/personaStore";

beforeEach(() => {
  setMock.mockReset();
  setMock.mockImplementation(async (name: string) => ({ name }));
  usePersonaStore.setState({ name: "大肥鱼", loaded: true, error: null });
});

describe("PersonaNameField", () => {
  it("保存调用 setPersonaName 并更新 store", async () => {
    render(<PersonaNameField />);
    fireEvent.change(screen.getByTestId("persona-name-input"), {
      target: { value: "肥鱼" },
    });
    fireEvent.click(screen.getByTestId("persona-name-save"));
    await waitFor(() => expect(setMock).toHaveBeenCalledWith("肥鱼"));
    expect(usePersonaStore.getState().name).toBe("肥鱼");
  });
});
