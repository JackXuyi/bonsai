"use client";

// 设置分区详情：按 section 组装既有卡片（plan-chat-first-ui v2）。
import { useState } from "react";
import { SecondaryChrome } from "@/components/layout/SecondaryChrome";
import { FaceSkinCard } from "@/components/settings/FaceSkinCard";
import { ModelSection } from "@/components/settings/ModelSection";
import { McpSection } from "@/components/agent/McpSection";
import { SkillSection } from "@/components/agent/SkillSection";
import { LogsCard } from "@/components/settings/LogsCard";
import { LoopsCard } from "@/components/settings/LoopsCard";
import { DiagnosticsCard } from "@/components/settings/DiagnosticsCard";
import { MemoryOverview } from "@/components/memory/MemoryOverview";
import { ActionRow } from "@/components/tools/ActionRow";
import { PersonaCard } from "@/components/tools/PersonaCard";
import { ProfileCard } from "@/components/tools/ProfileCard";
import { RelationshipCard } from "@/components/tools/RelationshipCard";
import { CalendarCard } from "@/components/tools/CalendarCard";
import { MailCard } from "@/components/tools/MailCard";
import { FileCard } from "@/components/tools/FileCard";
import { WebCard } from "@/components/tools/WebCard";
import { PerceptionCard } from "@/components/tools/PerceptionCard";
import { BoundaryCard } from "@/components/tools/BoundaryCard";
import { PlanProgressCard } from "@/components/tools/PlanProgressCard";
import { AuditTimeline } from "@/components/agent/AuditTimeline";
import { PermissionTiersCard } from "@/components/agent/PermissionTiersCard";
import { ToolCallRecords } from "@/components/agent/ToolCallRecords";
import { ToolResult } from "@/components/tools/ToolResult";
import { ReportDialog } from "@/components/tools/ReportDialog";
import { PreviewPane } from "@/components/preview/PreviewPane";
import { PokeFace } from "@/components/face/PokeFace";
import { useFaceFrame } from "@/hooks/useFaceFrame";
import {
  sectionById,
  settingsHref,
  type SettingsSectionId,
} from "@/lib/settingsSections";

export function SettingsSectionView({ sectionId }: { sectionId: string }) {
  const def = sectionById(sectionId);
  const title = def?.label ?? "设置";
  const id = (def?.id ?? "ops") as SettingsSectionId;

  return (
    <>
      <SecondaryChrome
        title={title}
        backHref={settingsHref()}
        backLabel="返回设置"
      />
      <div className="tab-body">
        <SectionBody id={id} />
      </div>
    </>
  );
}

function SectionBody({ id }: { id: SettingsSectionId }) {
  switch (id) {
    case "presence":
      return <PresenceSection />;
    case "persona":
      return (
        <>
          <PersonaCard />
          <ProfileCard active />
          <RelationshipCard />
        </>
      );
    case "memory":
      return <MemorySection />;
    case "model":
      return <ModelSection />;
    case "agent":
      return (
        <>
          <McpSection />
          <SkillSection />
        </>
      );
    case "access":
      return (
        <>
          <PermissionTiersCard />
          <AuditTimeline variant="full" />
          <CalendarCard active />
          <MailCard active />
          <FileCard active />
          <WebCard active />
          <PerceptionCard active />
          <BoundaryCard active />
          <PlanProgressCard active />
        </>
      );
    case "ops":
      return (
        <>
          <LoopsCard />
          <DiagnosticsCard />
          <LogsCard />
          <ToolCallRecords />
          <ToolResult />
        </>
      );
  }
}

function PresenceSection() {
  const frame = useFaceFrame();
  return (
    <>
      <div className="presence-preview card">
        <PokeFace size="lg" frame={frame} />
        <p className="hint-line">点脸打个招呼；皮肤在下方选择。</p>
      </div>
      <FaceSkinCard />
      <PreviewPane />
    </>
  );
}

function MemorySection() {
  const [report, setReport] = useState<{ open: boolean; md: string | null }>({
    open: false,
    md: null,
  });
  return (
    <>
      <ActionRow
        onReport={(md) => {
          setReport({ open: true, md });
        }}
      />
      <MemoryOverview />
      <ReportDialog
        open={report.open}
        md={report.md}
        onClose={() => setReport({ open: false, md: null })}
      />
    </>
  );
}
