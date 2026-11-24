import { beforeEach, describe, expect, it } from "vitest";
import { useUiStore } from "@/lib/stores/uiStore";

beforeEach(() => {
  useUiStore.setState({
    toasts: [],
    faceState: "calm",
    growth: null,
    stagePreview: null,
  });
});

describe("uiStore", () => {
  it("pushToast 队列有上限（4）且后进保留", () => {
    for (const t of ["a", "b", "c", "d", "e", "f"]) {
      useUiStore.getState().pushToast(t);
    }
    const texts = useUiStore.getState().toasts.map((t) => t.text);
    expect(texts).toEqual(["c", "d", "e", "f"]);
    expect(useUiStore.getState().toasts).toHaveLength(4);
  });

  it("dismissToast 移除对应项；setFaceState 校验非法值", () => {
    const { pushToast, dismissToast } = useUiStore.getState();
    pushToast("hello");
    const id = useUiStore.getState().toasts[0].id;
    dismissToast(id);
    expect(useUiStore.getState().toasts).toHaveLength(0);

    useUiStore.getState().setFaceState("not-a-state");
    expect(useUiStore.getState().faceState).toBe("calm");
    useUiStore.getState().setFaceState("proud");
    expect(useUiStore.getState().faceState).toBe("proud");
  });

  it("sessionDrawerOpen 可设可翻", () => {
    useUiStore.getState().setSessionDrawerOpen(true);
    expect(useUiStore.getState().sessionDrawerOpen).toBe(true);
    useUiStore.getState().toggleSessionDrawer();
    expect(useUiStore.getState().sessionDrawerOpen).toBe(false);
  });
});
