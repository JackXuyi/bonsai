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
