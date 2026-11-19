"use client";

// 会前提醒气泡（旧 showReminder）：mute 按钮当天全静，Rust 侧已去重限频。
import { useState } from "react";
import { calendarMuteToday } from "@/lib/commands";
import { toast } from "@/lib/stores/uiStore";
import type { ChatMessage } from "@/lib/stores/chatStore";

export function ReminderBubble({
  message,
  onStick,
}: {
  message: ChatMessage;
  onStick?: () => void;
}) {
  const [muted, setMuted] = useState(false);
  const r = message.reminder;

  async function mute() {
    try {
      await calendarMuteToday();
      toast("今天安静了");
    } catch {
      toast("操作失败");
    }
    setMuted(true);
    onStick?.();
  }

  return (
    <div className="bubble bot prediction">
      <span className="body">
        {r?.title}，{r?.minutes_left} 分钟后开始。
      </span>
      <br />
      <span className="badge">会前提醒</span>
      {!muted && (
        <>
          <br />
          <button className="ob-btn cal-mute" onClick={mute}>
            今天别再提醒
          </button>
        </>
      )}
    </div>
  );
}
