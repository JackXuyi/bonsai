"use client";

// 设置页：M1 模型配置 + M4 日志（spec-m1/m4）。
import { LogsCard } from "@/components/settings/LogsCard";
import { ModelSection } from "@/components/settings/ModelSection";

export default function SettingsPage() {
  return (
    <div className="tab-body">
      <ModelSection />
      <LogsCard />
    </div>
  );
}
