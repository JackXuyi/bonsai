"use client";

// 设置：索引钻取 / 分区详情（plan-chat-first-ui v2）。
import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { SettingsIndex } from "@/components/settings/SettingsIndex";
import { SettingsSectionView } from "@/components/settings/SettingsSectionView";
import { sectionById } from "@/lib/settingsSections";

function SettingsBody() {
  const params = useSearchParams();
  const raw = params.get("section");
  const section = sectionById(raw);
  if (section) return <SettingsSectionView sectionId={section.id} />;
  return <SettingsIndex />;
}

export default function SettingsPage() {
  return (
    <Suspense fallback={<SettingsIndex />}>
      <SettingsBody />
    </Suspense>
  );
}
