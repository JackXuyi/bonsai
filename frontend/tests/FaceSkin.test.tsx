import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

// jsdom 没有 Tauri internals，convertFileSrc mock 成同构的 asset URL。
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset://localhost/${encodeURIComponent(path)}`,
}));

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
