import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SkillAutoCandidate, SkillView } from "@/lib/types";

const skillListMock = vi.fn();
const skillAutoGetEnabledMock = vi.fn();
const skillAutoCandidatesMock = vi.fn();
const skillAutoSetEnabledMock = vi.fn();
const skillExtractNowMock = vi.fn();
const skillAutoDismissMock = vi.fn();
const skillToggleMock = vi.fn();
const skillRemoveMock = vi.fn();

vi.mock("@/lib/commands", () => ({
  humanizeAgentError: (e: unknown) => String(e),
  skillList: (...a: unknown[]) => skillListMock(...a),
  skillAutoGetEnabled: (...a: unknown[]) => skillAutoGetEnabledMock(...a),
  skillAutoCandidates: (...a: unknown[]) => skillAutoCandidatesMock(...a),
  skillAutoSetEnabled: (...a: unknown[]) => skillAutoSetEnabledMock(...a),
  skillExtractNow: (...a: unknown[]) => skillExtractNowMock(...a),
  skillAutoDismiss: (...a: unknown[]) => skillAutoDismissMock(...a),
  skillToggle: (...a: unknown[]) => skillToggleMock(...a),
  skillRemove: (...a: unknown[]) => skillRemoveMock(...a),
}));

vi.mock("@/components/agent/SkillAddDialog", () => ({
  SkillAddDialog: () => null,
}));
vi.mock("@/components/agent/SkillPreviewDialog", () => ({
  SkillPreviewDialog: () => null,
}));

describe("SkillAuto management surface", () => {
  beforeEach(() => {
    skillListMock.mockReset();
    skillAutoGetEnabledMock.mockReset();
    skillAutoCandidatesMock.mockReset();
    skillAutoSetEnabledMock.mockReset();
    skillExtractNowMock.mockReset();
    skillAutoDismissMock.mockReset();
    skillToggleMock.mockReset();
    skillRemoveMock.mockReset();
    skillAutoGetEnabledMock.mockResolvedValue(false);
    skillAutoCandidatesMock.mockResolvedValue([]);
    skillListMock.mockResolvedValue([]);
  });

  it("shows draft badge and source for auto-extracted packs", async () => {
    const draft: SkillView = {
      id: "auto-flow-abc",
      name: "查日程并汇总",
      description: "desc",
      enabled: false,
      valid: true,
      autoExtracted: true,
      autoSource: "来源任务 t1 · 2026-11-24",
      autoConfidence: 0.8,
    };
    skillListMock.mockResolvedValue([draft]);
    const { SkillSection } = await import("@/components/agent/SkillSection");
    render(<SkillSection />);
    await waitFor(() =>
      expect(screen.getByText(/自动提取 · 未启用/)).toBeTruthy(),
    );
    expect(screen.getByText(/来源任务 t1/)).toBeTruthy();
  });

  it("extract button calls skillExtractNow and refreshes", async () => {
    const cand: SkillAutoCandidate = {
      taskId: "task-1",
      goal: "整理下载目录",
      status: "done",
      okTools: 2,
      explicit: false,
      chainKey: "fs:list>fs:read",
    };
    skillAutoCandidatesMock.mockResolvedValue([cand]);
    skillExtractNowMock.mockResolvedValue({ id: "auto-x", name: "整理下载" });
    const { SkillSection } = await import("@/components/agent/SkillSection");
    render(<SkillSection />);
    await waitFor(() => expect(screen.getByText("整理下载目录")).toBeTruthy());
    fireEvent.click(screen.getByTestId("skill-extract-task-1"));
    await waitFor(() => expect(skillExtractNowMock).toHaveBeenCalledWith("task-1"));
  });

  it("auto toggle persists via skillAutoSetEnabled", async () => {
    skillAutoSetEnabledMock.mockResolvedValue(undefined);
    const { SkillSection } = await import("@/components/agent/SkillSection");
    render(<SkillSection />);
    await waitFor(() => expect(screen.getByTestId("skill-auto-enabled")).toBeTruthy());
    fireEvent.click(screen.getByTestId("skill-auto-enabled"));
    await waitFor(() => expect(skillAutoSetEnabledMock).toHaveBeenCalledWith(true));
  });
});
