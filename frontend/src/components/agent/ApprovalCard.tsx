"use client";

// 兼容旧导入名：全屏卡已退役，权威实现见 InlineApprovalBar
// （plan-chat-agent-inline）。
export {
  InlineApprovalBar as ApprovalCard,
  APPROVAL_WAIT_SECS,
} from "./InlineApprovalBar";
export type { InlineApprovalBarProps, InlineApprovalPlacement } from "./InlineApprovalBar";
