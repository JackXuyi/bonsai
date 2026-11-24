"use client";

// 工具页：动作按钮 + 感知/日历/文件/边界/关系/人格/画像卡 + 调用记录卡 + 结果卡。
import { useState } from "react";
import { ActionRow } from "./ActionRow";
import { PersonaCard } from "./PersonaCard";
import { ProfileCard } from "./ProfileCard";
import { PerceptionCard } from "./PerceptionCard";
import { CalendarCard } from "./CalendarCard";
import { FileCard } from "./FileCard";
import { WebCard } from "./WebCard";
import { MailCard } from "./MailCard";
import { BoundaryCard } from "./BoundaryCard";
import { RelationshipCard } from "./RelationshipCard";
import { AbilityCard } from "./AbilityCard";
import { PlanProgressCard } from "./PlanProgressCard";
import { ToolResult } from "./ToolResult";
import { ReportDialog } from "./ReportDialog";
import { ToolCallRecords } from "@/components/agent/ToolCallRecords";

export function ToolsPane() {
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
      <PersonaCard />
      <ProfileCard active />
      <PerceptionCard active />
      <CalendarCard active />
      <MailCard active />
      <WebCard active />
      <FileCard active />
      <BoundaryCard active />
      <RelationshipCard />
      <AbilityCard />
      <PlanProgressCard active />
      <ToolCallRecords />
      <ToolResult />
      <ReportDialog
        open={report.open}
        md={report.md}
        onClose={() => setReport({ open: false, md: null })}
      />
    </>
  );
}
