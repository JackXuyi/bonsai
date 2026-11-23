import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

// jsdom 没有 Tauri internals，convertFileSrc mock 成同构的 asset URL。
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
import { resolveSkinImage } from "@/components/face/skins";
import { useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import { useUiStore } from "@/lib/stores/uiStore";
import type { FaceSkinManifest, GrowthStatus } from "@/lib/types";

beforeEach(() => {
  // persist 落 localStorage，测试间还原默认（deepseek）
  useFaceSkinStore.setState({ skinId: "deepseek", customSkins: [], baseDir: null });
  localStorage.removeItem("bonsai-face-skin");
  useUiStore.setState({ faceState: "calm" });
  for (const m of [createMock, pickMock, clearMock, deleteMock]) m.mockReset();
});

describe("内置皮肤", () => {
  it("PokeFace 默认渲染鲸鱼图片，dormant / frame 落到 img 属性", () => {
    const { container } = render(<PokeFace size="sm" frame={1} />);
    const button = container.querySelector("button.face")!;
    expect(button.getAttribute("data-skin")).toBe("deepseek");

    const img = container.querySelector("img")!;
    expect(img.getAttribute("src")).toBe("/skins/deepseek-whale.png");
    expect(img.getAttribute("data-skin")).toBe("deepseek");
    expect(img.getAttribute("data-frame")).toBe("1");
    expect(img.getAttribute("data-dormant")).toBe("false");
  });

  it("dormant → img[data-dormant=true]（CSS 灰度降饱和）", () => {
    const growth: GrowthStatus = { stage: 2, dormant: true, day_index: 5, micro_seed: 1 };
    useUiStore.setState({ growth });
    const { container } = render(<PokeFace size="lg" frame={0} />);
    expect(container.querySelector("img")!.getAttribute("data-dormant")).toBe("true");
  });

  it("FaceSkinCard 默认选中 deepseek，预览含鲸鱼图片", () => {
    render(<FaceSkinCard />);
    expect(screen.getByTestId("skin-deepseek").getAttribute("aria-checked")).toBe("true");
    expect(screen.getByTestId("skin-bonsai").getAttribute("aria-checked")).toBe("false");
    const previewImg = screen.getByTestId("skin-deepseek").querySelector("img");
    expect(previewImg?.getAttribute("src")).toBe("/skins/deepseek-whale.png");
  });

  it("切到盆栽经典 → store 更新且 PokeFace 跟随换肤（svg 盆栽）", () => {
    render(<FaceSkinCard />);
    const bonsaiBtn = screen.getByTestId("skin-bonsai");
    fireEvent.click(bonsaiBtn);
    expect(useFaceSkinStore.getState().skinId).toBe("bonsai");
    expect(bonsaiBtn.getAttribute("aria-checked")).toBe("true");

    const { container } = render(<PokeFace size="sm" frame={0} />);
    expect(container.querySelector("button.face")!.getAttribute("data-skin")).toBe("bonsai");
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg[data-stage]")).toBeTruthy();
  });
});

describe("自定义皮肤（plan-face-custom-skins.md §3.2 渲染优先级）", () => {
  const manifest = (slots: Record<string, string>): FaceSkinManifest => ({
    id: "c-12345678",
    name: "我的猫",
    created_at: 1,
    slots,
  });

  it("resolveSkinImage：缺槽回退 calm，calm 也缺 → null（外层走 FALLBACK_IMAGE）", () => {
    const skin = {
      kind: "custom" as const,
      id: "c-12345678",
      name: "猫",
      desc: "",
      manifest: manifest({ calm: "calm.png", greeted: "greeted.webp" }),
    };
    const resolve = (_sid: string, file: string) => `asset://${file}`;
    expect(resolveSkinImage(skin, "calm", resolve)!.src).toContain("calm.png");
    expect(resolveSkinImage(skin, "greeted", resolve)!.src).toContain("greeted.webp");
    // sleepy 缺槽 → calm 图
    expect(resolveSkinImage(skin, "sleepy", resolve)!.src).toContain("calm.png");
    // resolveCustom 未就绪 → null
    expect(resolveSkinImage(skin, "calm")).toBeNull();
    // 全空 manifest → null
    const empty = { ...skin, manifest: manifest({}) };
    expect(resolveSkinImage(empty, "calm", resolve)).toBeNull();
  });

  it("选中自定义皮肤：PokeFace 渲染槽位图（asset 协议 URL）", () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png" })],
      baseDir: "/Users/x/Library/Application Support/app.bonsai.desktop",
    });
    const { container } = render(<PokeFace size="sm" frame={0} />);
    const img = container.querySelector("img")!;
    expect(img.getAttribute("data-skin")).toBe("c-12345678");
    const src = img.getAttribute("src")!;
    expect(src).toContain("face_skins");
    expect(src).toContain("c-12345678");
    expect(src).toContain("calm.png");
  });

  it("calm 未上传 → 渲染回退内置小鲸图，不白屏", () => {
    useFaceSkinStore.setState({ skinId: "c-12345678", customSkins: [manifest({})], baseDir: "/tmp" });
    const { container } = render(<PokeFace size="sm" frame={0} />);
    expect(container.querySelector("img")!.getAttribute("src")).toBe("/skins/deepseek-whale.png");
  });

  it("表情状态切换 → 对应槽位图优先于 calm 兜底", () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png", greeted: "greeted.webp" })],
      baseDir: "/tmp",
    });
    const { container } = render(<PokeFace size="sm" frame={0} />);
    expect(container.querySelector("img")!.getAttribute("src")).toContain("calm.png");

    useUiStore.setState({ faceState: "greeted" });
    const { container: c2 } = render(<PokeFace size="sm" frame={0} />);
    expect(c2.querySelector("img")!.getAttribute("src")).toContain("greeted.webp");
  });

  it("选中的自定义皮肤已删除（不在列表）→ 兜底回内置默认", () => {
    useFaceSkinStore.setState({ skinId: "c-99999999", customSkins: [] });
    const { container } = render(<PokeFace size="sm" frame={0} />);
    expect(container.querySelector("button.face")!.getAttribute("data-skin")).toBe("deepseek");
    expect(container.querySelector("img")!.getAttribute("src")).toBe("/skins/deepseek-whale.png");
  });

  it("FaceSkinCard 渲染自定义皮肤项；点击切换生效", () => {
    useFaceSkinStore.setState({ customSkins: [manifest({ calm: "calm.png" })] });
    render(<FaceSkinCard />);
    const btn = screen.getByTestId("skin-c-12345678");
    expect(screen.getByText("我的猫")).toBeTruthy();
    fireEvent.click(btn);
    expect(useFaceSkinStore.getState().skinId).toBe("c-12345678");
    expect(btn.getAttribute("aria-checked")).toBe("true");
  });
});

describe("自定义皮肤管理（plan-face-custom-skins.md §6.2）", () => {
  const manifest = (slots: Record<string, string>): FaceSkinManifest => ({
    id: "c-12345678",
    name: "我的猫",
    created_at: 1,
    slots,
  });

  it("选中自定义皮肤 → 槽位网格：已传槽位显示缩略图+×，缺槽显示上传", () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png", greeted: "greeted.webp" })],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    expect(screen.getByTestId("skin-manage")).toBeTruthy();
    expect(screen.getByTestId("slot-calm").getAttribute("data-filled")).toBe("true");
    expect(screen.getByTestId("slot-sleepy").getAttribute("data-filled")).toBe("false");
    expect(screen.getByTestId("slot-clear-calm")).toBeTruthy();
    expect(screen.getByTestId("slot-upload-sleepy")).toBeTruthy();
    const thumb = screen.getByTestId("slot-calm").querySelector("img")!;
    expect(thumb.getAttribute("src")).toContain("calm.png");
  });

  it("点上传 → faceSkinPickAndImport(skinId, slot)；取消选择不弹错", async () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({})],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("slot-upload-calm"));
    await waitFor(() => expect(pickMock).toHaveBeenCalledWith("c-12345678", "calm"));

    pickMock.mockRejectedValue("face_pick_cancelled:已取消选择");
    fireEvent.click(screen.getByTestId("slot-upload-calm"));
    await waitFor(() => expect(pickMock).toHaveBeenCalledTimes(2));
    // 无 toast（取消不报错）
  });

  it("上传失败 → 结构化错误冒泡为 toast 说明", async () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({})],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    pickMock.mockRejectedValue("face_bad_image:仅支持 PNG / JPEG / WebP 图片");
    fireEvent.click(screen.getByTestId("slot-upload-calm"));
    await waitFor(() =>
      expect(useUiStore.getState().toasts.some((t) => t.text.includes("仅支持 PNG"))).toBe(true),
    );
  });

  it("清除槽位 → faceSkinClearSlot", async () => {
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png" })],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("slot-clear-calm"));
    await waitFor(() => expect(clearMock).toHaveBeenCalledWith("c-12345678", "calm"));
  });

  it("新建皮肤 → faceSkinCreate + 选中新皮肤", async () => {
    createMock.mockResolvedValue({ id: "c-aaaa1111", name: "小狗", created_at: 2, slots: {} });
    render(<FaceSkinCard />);
    fireEvent.change(screen.getByTestId("skin-create-input"), { target: { value: "小狗" } });
    fireEvent.click(screen.getByTestId("skin-create-btn"));
    await waitFor(() => expect(createMock).toHaveBeenCalledWith("小狗"));
    expect(useFaceSkinStore.getState().skinId).toBe("c-aaaa1111");
  });

  it("删除选中皮肤：confirm 后调 faceSkinDelete 并回退内置默认", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png" })],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("skin-delete"));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith("c-12345678"));
    expect(useFaceSkinStore.getState().skinId).toBe("deepseek");
    confirmSpy.mockRestore();
  });

  it("删除选中皮肤：confirm 取消 → 不调 faceSkinDelete，管理区仍在", () => {
    // 删除成功会回退内置默认并卸载管理区，取消分支必须独立断言
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    useFaceSkinStore.setState({
      skinId: "c-12345678",
      customSkins: [manifest({ calm: "calm.png" })],
      baseDir: "/tmp",
    });
    render(<FaceSkinCard />);
    fireEvent.click(screen.getByTestId("skin-delete"));
    expect(deleteMock).not.toHaveBeenCalled();
    expect(useFaceSkinStore.getState().skinId).toBe("c-12345678");
    expect(screen.getByTestId("skin-delete")).toBeTruthy();
    confirmSpy.mockRestore();
  });

  it("选中内置皮肤时管理区不出现", () => {
    render(<FaceSkinCard />);
    expect(screen.queryByTestId("skin-manage")).toBeNull();
  });
});
