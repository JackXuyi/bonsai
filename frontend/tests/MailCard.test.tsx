import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { MailCard } from "@/components/tools/MailCard";
import type { MailStatus } from "@/lib/types";

const statusMock = vi.fn();
const setEnabledMock = vi.fn();
const probeMock = vi.fn();
const toastMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    mailStatus: () => statusMock(),
    mailSetEnabled: (on: boolean) => setEnabledMock(on),
    mailProbe: () => probeMock(),
  };
});

vi.mock("@/lib/stores/uiStore", () => ({
  toast: (...args: unknown[]) => toastMock(...args),
  useUiStore: (sel: (s: { windowShownAt: number }) => unknown) =>
    sel({ windowShownAt: 0 }),
}));

function fixture(over: Partial<MailStatus> = {}): MailStatus {
  return {
    enabled: false,
    supported: true,
    today_reads: 0,
    today_drafts: 0,
    note: "默认关闭",
    ...over,
  };
}

beforeEach(() => {
  for (const m of [statusMock, setEnabledMock, probeMock, toastMock]) {
    m.mockReset();
  }
  statusMock.mockResolvedValue(fixture());
  setEnabledMock.mockImplementation(async (on: boolean) =>
    fixture({ enabled: on })
  );
  probeMock.mockResolvedValue({ ok: true, count: 0 });
});

describe("MailCard", () => {
  it("默认关：实验标注与不发送说明可见", async () => {
    render(<MailCard active />);
    await waitFor(() => expect(statusMock).toHaveBeenCalled());
    const card = screen.getByTestId("mail-card");
    await waitFor(() =>
      expect(card.textContent).toContain("邮件未开启（默认关闭）")
    );
    expect(card.textContent).toContain("绝不会代你发送");
    expect(card.textContent).toContain("Mail.app");
  });

  it("开启后可试读；开关透传 setEnabled", async () => {
    statusMock.mockResolvedValue(fixture({ enabled: true }));
    render(<MailCard active />);
    await waitFor(() =>
      expect(screen.getByTestId("mail-card").textContent).toContain("已开启")
    );
    fireEvent.click(screen.getByText("试读未读"));
    await waitFor(() => expect(probeMock).toHaveBeenCalled());

    const toggle = screen
      .getByTestId("mail-card")
      .querySelector('input[type="checkbox"]') as HTMLInputElement;
    expect(toggle.checked).toBe(true);
    fireEvent.click(toggle);
    await waitFor(() => expect(setEnabledMock).toHaveBeenCalledWith(false));
  });
});
