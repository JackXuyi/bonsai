// 成长/关系播报 drain（旧 main.js 的 drainGrowthAnnouncements /
// drainRelationshipAnnouncements）：在开窗断点统一消费 pending 队列，
// Rust 侧保证 exactly-once（notified_at 在 ack 时落库），这里只渲染。
import {
  abilityAck,
  abilityStatus,
  growthAck,
  growthStatus,
  relationshipAck,
  relationshipStatus,
} from "./commands";
import { useChatStore } from "./stores/chatStore";
import { toast, useUiStore } from "./stores/uiStore";
import type { AbilityStatus, RelationshipStatus } from "./types";

export const STAGE_NAMES = ["破土", "抽枝", "成形", "成景"];
const RELATIONSHIP_LABELS = ["拘谨", "熟络", "会顶嘴"];
const ABILITY_LABELS = ["陪聊", "能办事", "真帮手"];

export async function drainGrowthAnnouncements() {
  const st = await growthStatus();
  useUiStore.getState().setGrowth(st);
  const pending = st.pending || [];
  for (const p of pending) {
    addAnnouncement(p.note, `它长大了 · ${STAGE_NAMES[p.stage] ?? "成长"}`);
    await growthAck(p.id).catch(() => {});
  }
  if (pending.length) toast("阿盆长大了点");
}

export async function drainRelationshipAnnouncements() {
  const st = await relationshipStatus();
  applyRelationship(st);
  const pending = st.pending || [];
  for (const p of pending) {
    const label =
      p.kind === "first_refusal"
        ? "第一次没顺着你"
        : `关系 · ${RELATIONSHIP_LABELS[p.stage] ?? ""}`;
    addAnnouncement(p.note, label);
    await relationshipAck(p.id).catch(() => {});
  }
  if (pending.length) toast("阿盆和你熟了点");
}

export function applyRelationship(st: RelationshipStatus | null) {
  if (!st) return;
  useUiStore.getState().setRelationship(st);
}

export async function drainAbilityAnnouncements() {
  const st = await abilityStatus();
  applyAbility(st);
  const pending = st.pending || [];
  for (const p of pending) {
    addAnnouncement(p.note, `本事 · ${ABILITY_LABELS[p.stage] ?? ""}`);
    await abilityAck(p.id).catch(() => {});
  }
  if (pending.length) toast("阿盆办事更稳了");
}

export function applyAbility(st: AbilityStatus | null) {
  if (!st) return;
  useUiStore.getState().setAbility(st);
}

function addAnnouncement(text: string, label: string) {
  useChatStore.getState().add({ kind: "announcement", text, note: label });
}
