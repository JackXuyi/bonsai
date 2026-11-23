"use client";

// 多会话切换（plan-context.md A11 / A12 / A13 / A14）：金丝雀是「切换后不串味」。
// 用真实 ChatPane + 真实 sessionStore，只把 IPC 命令换成 mock——测的是
// 「切了之后消息流到底是谁的」这条链，不是某个函数被调了几次。
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import { ChatPane } from "@/components/chat/ChatPane";
import { SessionDrawer } from "@/components/chat/SessionDrawer";
import { useChatStore } from "@/lib/stores/chatStore";
import { useSessionStore, DEFAULT_SESSION_ID } from "@/lib/stores/sessionStore";
import type { SessionRow } from "@/lib/types";

const h = vi.hoisted(() => ({ tauri: true }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), back: vi.fn() }),
}));

vi.mock("@/lib/tauriInvoke", () => ({
  get inTauri() {
    return h.tauri;
  },
  invokeWithTimeout: vi.fn().mockRejectedValue(new Error("ipc not under test")),
}));

const historyMock = vi.fn();
const listMock = vi.fn();
const createMock = vi.fn();
const renameMock = vi.fn();
const archiveMock = vi.fn();
const deleteMock = vi.fn();

vi.mock("@/lib/commands", () => ({
  dialogueHistory: (sid: string, limit?: number, beforeId?: number) =>
    historyMock(sid, limit, beforeId),
  dialogueChatStream: vi.fn().mockRejectedValue(new Error("not under test")),
  sessionList: () => listMock(),
  sessionCreate: (title?: string) => createMock(title),
  sessionRename: (id: string, title: string) => renameMock(id, title),
  sessionArchive: (id: string, archived: boolean) => archiveMock(id, archived),
  sessionDelete: (id: string) => deleteMock(id),
}));

const ts = new Date().toISOString();
const row = (id: string, title: string, extra: Partial<SessionRow> = {}): SessionRow => ({
  id,
  title,
  createdAt: ts,
  updatedAt: ts,
  archived: false,
  messageCount: 2,
  digest: null,
  digestUptoEventId: null,
  ...extra,
});

const texts = () => useChatStore.getState().messages.map((m) => m.text);

beforeEach(() => {
  h.tauri = true;
  useChatStore.setState({ messages: [] });
  useSessionStore.setState({
    list: [row(DEFAULT_SESSION_ID, "默认会话"), row("s-a", "会话 A"), row("s-b", "会话 B")],
    currentId: DEFAULT_SESSION_ID,
    loaded: true,
    pending: false,
  });
  historyMock.mockReset().mockResolvedValue([]);
  // 假后端：抽屉每次打开都会重拉列表，回「库里的三行」而不是空数组。
  listMock
    .mockReset()
    .mockResolvedValue([
      row(DEFAULT_SESSION_ID, "默认会话"),
      row("s-a", "会话 A"),
      row("s-b", "会话 B"),
    ]);
  createMock.mockReset();
  renameMock.mockReset().mockResolvedValue(undefined);
  archiveMock.mockReset().mockResolvedValue(undefined);
  deleteMock.mockReset().mockResolvedValue(0);
});

describe("ChatPane 会话切换", () => {
  it("A12：切到另一个会话，旧气泡一个不留，消息流换成目标会话内容", async () => {
    historyMock.mockImplementation(async (sid: string) => {
      if (sid === DEFAULT_SESSION_ID)
        return [
          { id: 1, ts, role: "user", content: "默认会话里的话" },
          { id: 2, ts, role: "assistant", content: "默认会话里的回" },
        ];
      if (sid === "s-a") return [{ id: 11, ts, role: "user", content: "会话 A 的话" }];
      return [];
    });

    render(<ChatPane />);
    await waitFor(() => expect(texts()).toEqual(["默认会话里的话", "默认会话里的回"]));

    act(() => useSessionStore.getState().switchTo("s-a"));

    // 旧会话的正文一个字都不能留在 DOM 里
    await waitFor(() => expect(texts()).toEqual(["会话 A 的话"]));
    expect(screen.queryByText("默认会话里的话")).toBeNull();
    expect(screen.queryByText("默认会话里的回")).toBeNull();
    expect(screen.getByText("会话 A 的话")).toBeTruthy();
  });

  it("A11：新建会话后立刻是空窗口，不继承上一个会话的上下文", async () => {
    historyMock.mockImplementation(async (sid: string) =>
      sid === DEFAULT_SESSION_ID
        ? [{ id: 1, ts, role: "user", content: "默认会话里的话" }]
        : []
    );
    createMock.mockResolvedValue(row("s-new", ""));

    render(<ChatPane />);
    await waitFor(() => expect(texts()).toEqual(["默认会话里的话"]));

    await act(async () => {
      await useSessionStore.getState().create();
    });

    expect(useSessionStore.getState().currentId).toBe("s-new");
    await waitFor(() => expect(texts()).toEqual([]));
    expect(screen.queryByText("默认会话里的话")).toBeNull();
    // 空态回来了：新会话没有历史
    expect(screen.getByText("还没聊过")).toBeTruthy();
  });

  it("切换时请求带的是目标会话 id（不是写死的 default）", async () => {
    render(<ChatPane />);
    await waitFor(() => expect(historyMock).toHaveBeenCalled());

    act(() => useSessionStore.getState().switchTo("s-b"));

    await waitFor(() =>
      expect(historyMock.mock.calls.some((c) => c[0] === "s-b")).toBe(true)
    );
  });
});

describe("SessionDrawer", () => {
  /** 抽屉打开即重拉列表：等这次 IPC 落定再断言，否则会撞 act 警告。 */
  async function mountDrawer(busy: boolean, onClose: () => void = () => {}) {
    await act(async () => {
      render(<SessionDrawer open busy={busy} onClose={onClose} />);
    });
  }

  it("A13：一轮在飞时切/建都拒绝，提示等它说完", async () => {
    await mountDrawer(true);

    expect(screen.getByText("等它说完再切换。")).toBeTruthy();
    const create = screen.getByText("＋ 新对话") as HTMLButtonElement;
    expect(create.disabled).toBe(true);

    const target = screen.getByText("会话 A").closest("button") as HTMLButtonElement;
    expect(target.disabled).toBe(true);
    target.click();
    expect(useSessionStore.getState().currentId).toBe(DEFAULT_SESSION_ID);
  });

  it("不在飞时点一行就切过去并关抽屉", async () => {
    const onClose = vi.fn();
    await mountDrawer(false, onClose);

    const target = screen.getByText("会话 B").closest("button") as HTMLButtonElement;
    expect(target.disabled).toBe(false);
    act(() => target.click());

    expect(useSessionStore.getState().currentId).toBe("s-b");
    expect(onClose).toHaveBeenCalled();
  });

  it("A14：删掉当前会话 → 后端级联删、列表同步、回到默认会话", async () => {
    useSessionStore.setState({ currentId: "s-b" });
    deleteMock.mockResolvedValue(7);
    await mountDrawer(false);

    await act(async () => {
      await useSessionStore.getState().remove("s-b");
    });

    expect(deleteMock).toHaveBeenCalledWith("s-b");
    expect(useSessionStore.getState().currentId).toBe(DEFAULT_SESSION_ID);
    expect(useSessionStore.getState().list.map((r) => r.id)).toEqual([
      DEFAULT_SESSION_ID,
      "s-a",
    ]);
  });

  it("默认会话不给归档与删除入口（老数据锚点）", async () => {
    await mountDrawer(false);

    const more = screen.getAllByLabelText("会话操作")[0];
    act(() => more.click());

    expect(screen.getByText("重命名")).toBeTruthy();
    expect(screen.queryByText("归档")).toBeNull();
    expect(screen.queryByText("删除")).toBeNull();
    expect(screen.getByText(/默认会话是历史锚点/)).toBeTruthy();
  });

  it("归档不等于删除：数据不动，行移进折叠区", async () => {
    await mountDrawer(false);

    await act(async () => {
      await useSessionStore.getState().archive("s-a", true);
    });

    expect(archiveMock).toHaveBeenCalledWith("s-a", true);
    expect(useSessionStore.getState().list.find((r) => r.id === "s-a")?.archived).toBe(true);
    expect(screen.getByText("已归档（1）")).toBeTruthy();
    // 归档的会话仍在列表里（可恢复），不是被删掉
    expect(screen.getByText("会话 A")).toBeTruthy();
  });
});
