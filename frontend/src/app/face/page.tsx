"use client";

// 表情页（spec-m0b §4）：大脸 150px 常驻内容区（不再靠 hero 变形放大），
// 瞬态表情 / 形态演示按钮排其下。
import { PokeFace } from "@/components/face/PokeFace";
import { PreviewPane } from "@/components/preview/PreviewPane";
import { useFaceFrame } from "@/hooks/useFaceFrame";

export default function FacePage() {
  const frame = useFaceFrame();

  return (
    <div className="tab-body face-page">
      <PokeFace size="lg" frame={frame} />
      <PreviewPane />
    </div>
  );
}
