"use client";

// 单条消息气泡（旧 addBubble / addAnnouncement / addPrediction 的渲染部分）。
import type { ChatMessage } from "@/lib/stores/chatStore";
import { PredictionCard } from "./PredictionCard";
import { ReminderBubble } from "./ReminderBubble";
import { MessageActions } from "./MessageActions";

export function MessageBubble({
  message,
  onStick,
}: {
  message: ChatMessage;
  onStick?: () => void;
}) {
  if (message.kind === "typing") {
    return (
      <div className="bubble bot typing">
        <i />
        <i />
        <i />
      </div>
    );
  }

  if (message.kind === "prediction") {
    return (
      <div className="bubble bot prediction">
        <span className="body">{message.prediction?.guess ?? message.text}</span>
        <br />
        <PredictionCard id={message.prediction!.id} onStick={onStick} />
      </div>
    );
  }

  if (message.kind === "reminder") {
    return <ReminderBubble message={message} onStick={onStick} />;
  }

  // 任务卡（v12）：云端工具循环的步骤进度 + 终态。
  if (message.kind === "task" && message.task) {
    const t = message.task;
    const label = t.status === "running" ? "执行中" : t.status === "done" ? "完成" : "失败";
    return (
      <div className={`bubble bot task-card task-${t.status}`}>
        <span className="task-title">🔧 任务 · {label}</span>
        {t.steps.length > 0 ? (
          <ul className="task-steps">
            {t.steps.map((s) => (
              <li key={s.seq}>
                <span className={`task-dot task-dot-${s.status}`} />
                <span className="task-tool">{s.tool}</span>
                <span className="task-status">
                  {s.status === "ok" ? "完成" : s.status === "denied" ? "待确认" : "失败"}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <span className="task-status">正在调度工具…</span>
        )}
      </div>
    );
  }

  const isUser = message.kind === "user";
  const variant =
    message.kind === "announcement"
      ? "prediction"
      : message.proactive
        ? "proactive"
        : "";

  return (
    <>
      <div className={`bubble ${isUser ? "user" : "bot"}${variant ? " " + variant : ""}`}>
        <span className="body">{message.text}</span>
        {message.streaming && <span className="caret" />}
        {message.note ? (
          <>
            <br />
            <span className="badge">{message.note}</span>
          </>
        ) : null}
      </div>
      {/* 操作条（spec-s8）：时间 / 复制 / 查看详情；流式中不渲染 */}
      {!message.streaming && <MessageActions message={message} />}
    </>
  );
}
