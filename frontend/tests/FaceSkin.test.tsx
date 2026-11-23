import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { PokeFace } from "@/components/face/PokeFace";
import { FaceSkinCard } from "@/components/settings/FaceSkinCard";
import { useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import { useUiStore } from "@/lib/stores/uiStore";
import type { GrowthStatus } from "@/lib/types";

beforeEach(() => {
  // persist 落 localStorage，测试间还原默认（deepseek）
  useFaceSkinStore.setState({ skinId: "deepseek" });
  localStorage.removeItem("bonsai-face-skin");
});

describe("图片皮肤（DeepSeek 小鲸，默认）", () => {
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
});

describe("皮肤配置", () => {
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
