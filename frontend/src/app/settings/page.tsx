"use client";

// 设置页：M1 模型配置 + M4 日志 + agent 能力（MCP / Skill，spec-s6）。
import { LogsCard } from "@/components/settings/LogsCard";
import { McpSection } from "@/components/agent/McpSection";
import { SkillSection } from "@/components/agent/SkillSection";
import { ModelSection } from "@/components/settings/ModelSection";

export default function SettingsPage() {
  return (
    <div className="tab-body">
      <ModelSection />
      <McpSection />
      <SkillSection />
      <LogsCard />
    </div>
  );
}
