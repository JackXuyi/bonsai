"use client";

// 工具页：动作按钮 + 感知/日历/边界/关系/人格卡 + 结果卡。
import { useState } from "react";
import { ActionRow } from "./ActionRow";
import { PersonaCard } from "./PersonaCard";
import { PerceptionCard } from "./PerceptionCard";
import { CalendarCard } from "./CalendarCard";
import { BoundaryCard } from "./BoundaryCard";
import { RelationshipCard } from "./RelationshipCard";
import { ToolResult } from "./ToolResult";
import { ReportDialog } from "./ReportDialog";

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
      <PerceptionCard active />
      <CalendarCard active />
      <BoundaryCard active />
      <RelationshipCard />
      <ToolResult />
      <ReportDialog
        open={report.open}
        md={report.md}
        onClose={() => setReport({ open: false, md: null })}
      />
    </>
  );
}
