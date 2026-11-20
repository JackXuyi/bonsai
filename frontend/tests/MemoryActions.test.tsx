import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { MemoryActions, FORGET_CONFIRM_MS } from "@/components/memory/MemoryActions";
import type { ConsolidateResult } from "@/lib/types";

const RESULT: ConsolidateResult = {
  trigger: "manual",
  events_in_window: 3,
  extracted: 2,
  merged: 1,
  contradicted: 0,
  promoted: 1,
  deleted: 0,
  consumed: 3,
};

describe("MemoryActions", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("忘记全部两段式：第一下只 arm，第二下才真删", async () => {
    const runForgetAll = vi.fn().mockResolvedValue(undefined);
    const onChanged = vi.fn();
    render(
      <MemoryActions onChanged={onChanged} runForgetAll={runForgetAll} runConsolidate={vi.fn()} />
    );

    fireEvent.click(screen.getByRole("button", { name: "忘记全部" }));
    expect(runForgetAll).not.toHaveBeenCalled();
    expect(screen.getByText("确认忘掉一切？")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "忘记全部" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(runForgetAll).toHaveBeenCalledTimes(1);
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it("4 秒内不确认就回弹，不会误删", async () => {
    const runForgetAll = vi.fn().mockResolvedValue(undefined);
    render(
      <MemoryActions onChanged={vi.fn()} runForgetAll={runForgetAll} runConsolidate={vi.fn()} />
    );

    fireEvent.click(screen.getByRole("button", { name: "忘记全部" }));
    expect(screen.getByText("确认忘掉一切？")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(FORGET_CONFIRM_MS + 10);
    });

    expect(screen.getByText("忘记全部…")).toBeTruthy();
    expect(runForgetAll).not.toHaveBeenCalled();
  });

  it("整理记忆：跑一次并刷新，失败也不炸", async () => {
    const runConsolidate = vi.fn().mockResolvedValue(RESULT);
    const onChanged = vi.fn();
    render(
      <MemoryActions
        onChanged={onChanged}
        runConsolidate={runConsolidate}
        runForgetAll={vi.fn()}
      />
    );

    fireEvent.click(screen.getByRole("button", { name: "整理记忆" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(runConsolidate).toHaveBeenCalledTimes(1);
    expect(onChanged).toHaveBeenCalledTimes(1);

    runConsolidate.mockRejectedValueOnce(new Error("模型挂了"));
    fireEvent.click(screen.getByRole("button", { name: "整理记忆" }));
    await act(async () => {
      await Promise.resolve();
    });
    // 失败不抛到渲染层：按钮回到可用状态
    expect(screen.getByRole("button", { name: "整理记忆" })).toBeTruthy();
  });
});
