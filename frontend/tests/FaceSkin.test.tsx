import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset://localhost/${encodeURIComponent(path)}`,
}));

const createMock = vi.fn();
const pickMock = vi.fn();
const clearMock = vi.fn();
const deleteMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    faceSkinCreate: (name: string) => createMock(name),
    faceSkinPickAndImport: (skinId: string, slot: string) => pickMock(skinId, slot),
    faceSkinClearSlot: (skinId: string, slot: string) => clearMock(skinId, slot),
    faceSkinDelete: (skinId: string) => deleteMock(skinId),
  };
});

import { PokeFace } from "@/components/face/PokeFace";
import { FaceSkinCard } from "@/components/settings/FaceSkinCard";
import { DEFAULT_SKIN, resolveSkinImage } from "@/components/face/skins";
import { useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import { useUiStore } from "@/lib/stores/uiStore";
import type { FaceSkinManifest, GrowthStatus } from "@/lib/types";

beforeEach(() => {
  useFaceSkinStore.setState({ skinId: DEFAULT_SKIN, customSkins: [], baseDir: null });
  localStorage.removeItem("bonsai-face-skin");
  useUiStore.setState({ faceState: "calm" });
  for (const m of [createMock, pickMock, clearMock, deleteMock]) m.mockReset();
});

describe("内置皮肤", () => {
  it("PokeFace 默认渲染大肥鱼 calm 槽", () => {
    const { container } = render(<PokeFace size="sm" frame={1} />);
    const button = container.querySelector("button.face")!;
    expect(button.getAttribute("data-skin")).toBe("fatfish");

    const img = container.querySelector("img")!;
    expect(img.getAttribute("src")).toBe("/skins/fatfish/calm.png");
    expect(img.getAttribute("data-skin")).toBe("fatfish");
    expect(img.getAttribute("data-frame")).toBe("1");
  });

  it("dormant → img[data-dormant=true]", () => {
    const growth: GrowthStatus = { stage: 2, dormant: true, day_index: 5, micro_seed: 1 };
    useUiStore.setState({ growth });
    const { container } = render(<PokeFace size="lg" frame={0} />);
    expect(container.querySelector("img")!.getAttribute("data-dormant")).toBe("true");
  });

  it("FaceSkinCard 默认选中 fatfish，无盆栽选项", () => {
    render(<FaceSkinCard />);
    expect(screen.getByTestId("skin-fatfish").getAttribute("aria-checked")).toBe("true");
    expect(screen.queryByTestId("skin-bonsai")).toBeNull();
    const previewImg = screen.getByTestId("skin-fatfish").querySelector("img");
    expect(previewImg?.getAttribute("src")).toBe("/skins/fatfish/calm.png");
  });

  it("切到小鲸 → store 更新且 PokeFace 跟随", () => {
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("skin-deepseek"));
    expect(useFaceSkinStore.getState().skinId).toBe("deepseek");

    const { container } = render(<PokeFace size="sm" frame={0} />);
    expect(container.querySelector("button.face")!.getAttribute("data-skin")).toBe("deepseek");
    expect(container.querySelector("img")!.getAttribute("src")).toBe("/skins/deepseek-whale.png");
  });

  it("persist 残留 bonsai → 迁移到 fatfish", () => {
    useFaceSkinStore.setState({ skinId: "bonsai" });
    useFaceSkinStore.getState().setSkin("bonsai");
    expect(useFaceSkinStore.getState().skinId).toBe("fatfish");
  });
});

describe("自定义皮肤", () => {
  const manifest = (slots: Record<string, string>): FaceSkinManifest => ({
    id: "c-12345678",
    name: "我的猫",
    created_at: 1,
    slots,
  });

  it("resolveSkinImage：缺槽回退 calm", () => {
    const skin = {
      kind: "custom" as const,
      id: "c-12345678",
      name: "猫",
      desc: "",
      manifest: manifest({ calm: "calm.png", greeted: "greeted.webp" }),
    };
    const resolve = (_sid: string, file: string) => `asset://${file}`;
    expect(resolveSkinImage(skin, "calm", resolve)!.src).toContain("calm.png");
    expect(resolveSkinImage(skin, "sleepy", resolve)!.src).toContain("calm.png");
    expect(resolveSkinImage(skin, "calm")).toBeNull();
  });

  it("选中自定义皮肤：PokeFace 渲染槽位图", () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png" })],
      baseDir: "/Users/x/Library/Application Support/app.bonsai.desktop",
    });
    const { container } = render(<PokeFace size="sm" frame={0} />);
    const src = container.querySelector("img")!.getAttribute("src")!;
    expect(src).toContain("face_skins");
    expect(src).toContain("calm.png");
  });

  it("calm 未上传 → 回退小鲸兜底图", () => {
    useFaceSkinStore.setState({ skinId: "c-12345678", customSkins: [manifest({})], baseDir: "/tmp" });
    const { container } = render(<PokeFace size="sm" frame={0} />);
    expect(container.querySelector("img")!.getAttribute("src")).toBe("/skins/deepseek-whale.png");
  });

  it("选中的自定义皮肤已删除 → 兜底默认 fatfish", () => {
    useFaceSkinStore.setState({ skinId: "c-99999999", customSkins: [] });
    const { container } = render(<PokeFace size="sm" frame={0} />);
    expect(container.querySelector("button.face")!.getAttribute("data-skin")).toBe("fatfish");
  });

  it("FaceSkinCard 渲染自定义皮肤项", () => {
    useFaceSkinStore.setState({ customSkins: [manifest({ calm: "calm.png" })] });
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("skin-c-12345678"));
    expect(useFaceSkinStore.getState().skinId).toBe("c-12345678");
  });
});

describe("自定义皮肤管理", () => {
  const manifest = (slots: Record<string, string>): FaceSkinManifest => ({
    id: "c-12345678",
    name: "我的猫",
    created_at: 1,
    slots,
  });

  it("选中自定义皮肤 → 槽位网格", () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png", greeted: "greeted.webp" })],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    expect(screen.getByTestId("slot-calm").getAttribute("data-filled")).toBe("true");
    expect(screen.getByTestId("slot-sleepy").getAttribute("data-filled")).toBe("false");
  });

  it("点上传 → faceSkinPickAndImport", async () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({})],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("slot-upload-calm"));
    await waitFor(() => expect(pickMock).toHaveBeenCalledWith("c-12345678", "calm"));
  });

  it("删除选中皮肤：confirm 后回退默认 fatfish", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png" })],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("skin-delete"));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith("c-12345678"));
    expect(useFaceSkinStore.getState().skinId).toBe("fatfish");
    confirmSpy.mockRestore();
  });

  it("选中内置皮肤时管理区不出现", () => {
    render(<FaceSkinCard />);
    expect(screen.queryByTestId("skin-manage")).toBeNull();
  });
});
