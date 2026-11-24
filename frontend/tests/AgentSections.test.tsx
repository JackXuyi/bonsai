import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { McpSection } from "@/components/agent/McpSection";
import type { DiscoveredServer, McpServerRow, SkillView, ToolCallRecord } from "@/lib/types";

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
const scanMock = vi.fn().mockResolvedValue<DiscoveredServer[]>([]);
const importMock = vi.fn().mockResolvedValue({ inserted: 0, skipped: 0 });
const toolsMock = vi.fn().mockResolvedValue<
  { name: string; summary: string; detail: string; kind: "mcp" | "skill" | "builtin"; remote: boolean }[]
>([]);
const builtinMock = vi.fn().mockResolvedValue<
  { name: string; summary: string; detail: string; kind: "mcp" | "skill" | "builtin"; remote: boolean }[]
>([]);

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    mcpServerList: (...args: unknown[]) => listMock(...args),
    mcpServerToggle: (...args: unknown[]) => toggleMock(...args),
    mcpServerSave: (...args: unknown[]) => saveMock(...args),
    mcpServerRemove: vi.fn().mockResolvedValue(undefined),
    mcpServerTest: vi.fn().mockResolvedValue(3),
    mcpServerTools: (...args: unknown[]) => toolsMock(...args),
    builtinToolsList: (...args: unknown[]) => builtinMock(...args),
    skillList: (...args: unknown[]) => skillListMock(...args),
    skillToggle: vi.fn().mockResolvedValue(undefined),
    skillRemove: vi.fn().mockResolvedValue(undefined),
    skillAutoGetEnabled: vi.fn().mockResolvedValue(false),
    skillAutoCandidates: vi.fn().mockResolvedValue([]),
    skillAutoSetEnabled: vi.fn().mockResolvedValue(undefined),
    skillExtractNow: vi.fn().mockResolvedValue({ id: "x", name: "x" }),
    skillAutoDismiss: vi.fn().mockResolvedValue(undefined),
    toolCallRecords: (...args: unknown[]) => recordsMock(...args),
    agentApprovalGet: (...args: unknown[]) => approvalGetMock(...args),
    agentApprovalSet: (...args: unknown[]) => approvalSetMock(...args),
    mcpDiscoverScan: (...args: unknown[]) => scanMock(...args),
    mcpDiscoverImport: (...args: unknown[]) => importMock(...args),
  };
});

beforeEach(() => {
  listMock.mockClear().mockResolvedValue([row]);
  toggleMock.mockClear();
  saveMock.mockClear();
  approvalGetMock.mockClear().mockResolvedValue("auto");
  approvalSetMock.mockClear();
  scanMock.mockClear().mockResolvedValue<DiscoveredServer[]>([]);
  importMock.mockClear().mockResolvedValue({ inserted: 0, skipped: 0 });
  toolsMock.mockClear().mockResolvedValue([
    { name: "read_file", summary: "读文件", detail: "{}", kind: "mcp", remote: false, access: "read" },
    { name: "list_dir", summary: "列目录", detail: "{}", kind: "mcp", remote: false, access: "read" },
  ]);
  builtinMock.mockClear().mockResolvedValue([
    {
      name: "memory_search",
      summary: "按日期或关键词翻找过往聊天记录与记忆（本地）",
      detail: "{}",
      kind: "builtin",
      remote: false,
      access: "read",
    },
    {
      name: "fs_write",
      summary: "在授权目录内新建文本文件",
      detail: "{}",
      kind: "builtin",
      remote: false,
      access: "ask",
    },
  ]);
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

  it("scan modal lists candidates and imports selected signatures", async () => {
    const candNew: DiscoveredServer = {
      name: "prune",
      transport: "stdio",
      command: "/bin/prune",
      args: ["mcp"],
      url: null,
      headers: [],
      env: [],
      source: "cursor",
      signature: "stdio|/bin/prune|mcp",
      imported: false,
    };
    const candDone: DiscoveredServer = {
      name: "followup",
      transport: "http",
      command: null,
      args: [],
      url: "https://api.followup.us/api/v1/mcp",
      headers: [],
      env: [],
      source: "claude-code",
      signature: "http|https://api.followup.us/api/v1/mcp",
      imported: true,
    };
    scanMock.mockResolvedValue([candNew, candDone]);
    importMock.mockResolvedValue({ inserted: 1, skipped: 0 });
    render(<McpSection />);
    await waitFor(() => screen.getByText("文件检索")); // flush initial load inside act
    fireEvent.click(screen.getByText("扫描本机"));
    await waitFor(() => expect(screen.getByTestId("scan-modal")).toBeTruthy());
    expect(scanMock).toHaveBeenCalledTimes(1);
    expect(screen.getAllByTestId("scan-row").length).toBe(2);
    // 已导入的候选勾选框禁用且标了「已导入」
    const doneBox = screen.getByLabelText("选择 followup") as HTMLInputElement;
    expect(doneBox.disabled).toBe(true);
    expect(screen.getByText(/已导入/)).toBeTruthy();
    // 导入只带未导入的 signature
    fireEvent.click(screen.getByText(/导入选中/));
    await waitFor(() =>
      expect(importMock).toHaveBeenCalledWith(["stdio|/bin/prune|mcp"])
    );
    // 导入成功后重扫刷新 imported 标记
    await waitFor(() => expect(scanMock).toHaveBeenCalledTimes(2));
    expect(screen.getByText(/已导入 1 条/)).toBeTruthy();
  });

  it("expanding a server row lists its cached tools with access badges", async () => {
    render(<McpSection />);
    await waitFor(() => screen.getByText("文件检索"));
    // 收起态不显示工具
    expect(screen.queryByTestId("tools-of-srv1")).toBeNull();
    // 点行头展开 → 懒拉工具清单并渲染（字母序）
    fireEvent.click(screen.getByText("文件检索"));
    await waitFor(() => expect(screen.getByTestId("tools-of-srv1")).toBeTruthy());
    expect(toolsMock).toHaveBeenCalledWith("srv1");
    expect(screen.getByText("read_file")).toBeTruthy();
    expect(screen.getByText("读文件")).toBeTruthy();
    const badges = screen.getAllByTestId("tool-access-badge");
    expect(badges.length).toBeGreaterThanOrEqual(2);
    expect(badges[0].getAttribute("data-access")).toBe("read");
    // 再点一次收起
    fireEvent.click(screen.getByText("文件检索"));
    await waitFor(() => expect(screen.queryByTestId("tools-of-srv1")).toBeNull());
    // 缓存复用：收起再展开不再发第二次请求
    fireEvent.click(screen.getByText("文件检索"));
    await waitFor(() => expect(screen.getByTestId("tools-of-srv1")).toBeTruthy());
    expect(toolsMock).toHaveBeenCalledTimes(1);
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
    await waitFor(() => expect(screen.getByText("weather")).toBeTruthy());
    fireEvent.click(screen.getByText("weather"));
    expect(screen.getByText(/auto_allowed/)).toBeTruthy();
    expect(screen.getByText(/level0:weather/)).toBeTruthy();
  });
});

describe("PermissionTiersCard + AuditTimeline", () => {
  it("shows tier legend, approval switch, and builtin access rows", async () => {
    const { PermissionTiersCard } = await import("@/components/agent/PermissionTiersCard");
    render(<PermissionTiersCard />);
    await waitFor(() => expect(screen.getByTestId("permission-tiers-card")).toBeTruthy());
    expect(screen.getByTestId("access-tier-legend")).toBeTruthy();
    await waitFor(() => expect(builtinMock).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText("fs_write")).toBeTruthy());
    const askRow = screen.getByText("fs_write").closest("[data-testid=builtin-access-row]");
    expect(askRow?.querySelector('[data-access="ask"]')).toBeTruthy();

    await waitFor(() => expect(approvalGetMock).toHaveBeenCalled());
    fireEvent.click(screen.getByText("每次询问"));
    await waitFor(() => expect(approvalSetMock).toHaveBeenCalledWith("ask"));
    await waitFor(() =>
      expect(screen.getByTestId("approval-mode-hint").textContent).toContain("30 秒不回应视为拒绝")
    );
  });

  it("full audit timeline filters by outcome and search", async () => {
    const { AuditTimeline } = await import("@/components/agent/AuditTimeline");
    const rows: ToolCallRecord[] = [
      {
        id: 1,
        ts: "2026-11-20T10:30:00Z",
        tool: "mcp",
        action: "weather",
        outcome: "ok",
        resultStatus: "auto_allowed",
        paramsSummary: '{"city":"杭州"}',
        remote: false,
        durationMs: 42,
      },
      {
        id: 2,
        ts: "2026-11-20T11:00:00Z",
        tool: "builtin",
        action: "fs_write",
        outcome: "denied",
        resultStatus: "denied_by_gate",
        paramsSummary: '{"path":"/tmp/a"}',
        remote: false,
        durationMs: 5,
      },
      {
        id: 3,
        ts: "2026-11-20T11:05:00Z",
        tool: "mcp",
        action: "remote_fetch",
        outcome: "ok",
        resultStatus: "auto_allowed_remote",
        paramsSummary: "{}",
        remote: true,
        durationMs: 120,
      },
    ];
    recordsMock.mockResolvedValue(rows);
    render(<AuditTimeline variant="full" />);
    await waitFor(() => expect(screen.getByTestId("audit-timeline")).toBeTruthy());
    expect(recordsMock).toHaveBeenCalledWith(200);
    expect(screen.getByTestId("audit-count").textContent).toContain("3 / 3");

    fireEvent.change(screen.getByLabelText("按结果筛选"), { target: { value: "denied" } });
    expect(screen.getByTestId("audit-count").textContent).toContain("1 / 3");
    expect(screen.getByText("fs_write")).toBeTruthy();
    expect(screen.queryByText("weather")).toBeNull();

    fireEvent.change(screen.getByLabelText("按结果筛选"), { target: { value: "all" } });
    fireEvent.change(screen.getByLabelText("搜索审计记录"), { target: { value: "remote_fetch" } });
    expect(screen.getByTestId("audit-count").textContent).toContain("1 / 3");
    expect(screen.getByText("remote_fetch")).toBeTruthy();
  });
});
