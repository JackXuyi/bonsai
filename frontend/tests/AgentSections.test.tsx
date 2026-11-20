import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { McpSection } from "@/components/agent/McpSection";
import type { McpServerRow, SkillView, ToolCallRecord } from "@/lib/types";

const row: McpServerRow = {
  id: "srv1",
  name: "文件检索",
  transport: "http",
  command: null,
  args: [],
  url: "https://mcp.example.com/v1",
  headers: [["Authorization", "••••9f3a"]],
  env: [],
  enabled: false,
  toolCount: 3,
  toolCacheAt: "2026-11-20T10:00:00Z",
};

const listMock = vi.fn().mockResolvedValue<McpServerRow[]>([row]);
const toggleMock = vi.fn().mockResolvedValue(undefined);
const saveMock = vi.fn().mockResolvedValue("srv1");
const skillListMock = vi.fn().mockResolvedValue<SkillView[]>([]);
const recordsMock = vi.fn().mockResolvedValue<ToolCallRecord[]>([]);
const approvalGetMock = vi.fn().mockResolvedValue<"auto" | "ask">("auto");
const approvalSetMock = vi.fn().mockResolvedValue(undefined);

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    mcpServerList: (...args: unknown[]) => listMock(...args),
    mcpServerToggle: (...args: unknown[]) => toggleMock(...args),
    mcpServerSave: (...args: unknown[]) => saveMock(...args),
    mcpServerRemove: vi.fn().mockResolvedValue(undefined),
    mcpServerTest: vi.fn().mockResolvedValue(3),
    mcpServerTools: vi.fn().mockResolvedValue([]),
    skillList: (...args: unknown[]) => skillListMock(...args),
    skillToggle: vi.fn().mockResolvedValue(undefined),
    toolCallRecords: (...args: unknown[]) => recordsMock(...args),
    agentApprovalGet: (...args: unknown[]) => approvalGetMock(...args),
    agentApprovalSet: (...args: unknown[]) => approvalSetMock(...args),
  };
});

beforeEach(() => {
  listMock.mockClear().mockResolvedValue([row]);
  toggleMock.mockClear();
  saveMock.mockClear();
  approvalGetMock.mockClear().mockResolvedValue("auto");
  approvalSetMock.mockClear();
});

describe("McpSection", () => {
  it("renders rows with transport badge and actions", async () => {
    render(<McpSection />);
    await waitFor(() => expect(screen.getByText("文件检索")).toBeTruthy());
    expect(screen.getByText("3 个工具")).toBeTruthy();
    expect(screen.getByText("启用")).toBeTruthy();
  });

  it("toggling enable calls the command", async () => {
    render(<McpSection />);
    await waitFor(() => screen.getByText("启用"));
    fireEvent.click(screen.getByText("启用"));
    await waitFor(() => expect(toggleMock).toHaveBeenCalledWith("srv1", true));
  });

  it("approval mode switch persists the choice", async () => {
    render(<McpSection />);
    await waitFor(() => expect(approvalGetMock).toHaveBeenCalled());
    // 默认 auto：提示语说明调用直接执行
    await waitFor(() => expect(screen.getByTestId("approval-mode-row")).toBeTruthy());
    fireEvent.click(screen.getByText("每次询问"));
    await waitFor(() => expect(approvalSetMock).toHaveBeenCalledWith("ask"));
    await waitFor(() =>
      expect(screen.getByTestId("approval-mode-row").parentElement?.textContent).toContain(
        "30 秒不回应视为拒绝"
      )
    );
  });

  it("form shows the privacy hint for http transport", async () => {
    render(<McpSection />);
    await waitFor(() => screen.getByText("文件检索")); // flush the initial load inside act
    fireEvent.click(screen.getByText("＋ 接入 MCP server"));
    fireEvent.click(screen.getByText("http（远程）"));
    const hint = screen.getByTestId("privacy-hint");
    expect(hint.textContent).toContain("外部服务");
  });

  it("form switches fields between stdio and http", async () => {
    render(<McpSection />);
    await waitFor(() => screen.getByText("文件检索"));
    fireEvent.click(screen.getByText("＋ 接入 MCP server"));
    expect(screen.getByPlaceholderText(/mcp-server-fs/)).toBeTruthy();
    fireEvent.click(screen.getByText("http（远程）"));
    expect(screen.getByPlaceholderText(/mcp\.example\.com/)).toBeTruthy();
    expect(screen.queryByPlaceholderText(/mcp-server-fs/)).toBeNull();
  });
});

describe("SkillSection + ToolCallRecords (light checks)", () => {
  it("invalid skill shows the rejection reason", async () => {
    const { SkillSection } = await import("@/components/agent/SkillSection");
    const invalid: SkillView = {
      id: "bad",
      name: "bad",
      description: "",
      enabled: false,
      valid: false,
      errorCode: "skill_injection",
    };
    skillListMock.mockResolvedValue([invalid]);
    render(<SkillSection />);
    await waitFor(() => expect(screen.getByText("试图改写人格边界，已拦截")).toBeTruthy());
  });

  it("records card renders outcome dots and details", async () => {
    const { ToolCallRecords } = await import("@/components/agent/ToolCallRecords");
    const rec: ToolCallRecord = {
      id: 1,
      ts: "2026-11-20T10:30:00Z",
      tool: "mcp",
      action: "weather",
      outcome: "ok",
      resultStatus: "auto_allowed",
      paramsSummary: '{"city":"杭州"}',
      promptTrace: "level0:weather | level1:weather | level2:call:weather",
      remote: false,
      durationMs: 42,
    };
    recordsMock.mockResolvedValue([rec]);
    render(<ToolCallRecords />);
    await waitFor(() => expect(screen.getByText("mcp:weather")).toBeTruthy());
    fireEvent.click(screen.getByText("mcp:weather"));
    expect(screen.getByText(/auto_allowed/)).toBeTruthy();
    expect(screen.getByText(/level0:weather/)).toBeTruthy();
  });
});
