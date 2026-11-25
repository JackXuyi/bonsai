"use client";

// 单条消息气泡（旧 addBubble / addAnnouncement / addPrediction 的渲染部分）。
import type { ChatMessage } from "@/lib/stores/chatStore";
import { PredictionCard } from "./PredictionCard";
import { ReminderBubble } from "./ReminderBubble";
import { PlanInlineBubble } from "./PlanInlineBubble";
import { MessageActions } from "./MessageActions";
import { MarkdownText } from "./MarkdownText";

export function MessageBubble({
  message,
  onStick,
  onRetry,
}: {
  message: ChatMessage;
  onStick?: () => void;
  /** 失败行「重试」（plan-retry T7）：仅 retryable 时出现 */
  onRetry?: (botMessageId: number) => void;
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

  if (message.kind === "plan" && message.planTaskId) {
    return <PlanInlineBubble planTaskId={message.planTaskId} />;
  }

  // 工具原文不进气泡；计划进度见 kind=plan。明细仍走「查看详情」/trace。

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
        {/* bot 侧按 markdown 渲染（模型输出带粗体/列表等标记）；
            用户消息保持原样显示自己打的字。 */}
        {isUser ? <span className="body">{message.text}</span> : <MarkdownText text={message.text} />}
        {message.streaming && <span className="caret" />}
        {message.note ? (
          <>
            <br />
            <span className="badge">{message.note}</span>
          </>
        ) : null}
      </div>
      {message.failure && !message.streaming ? (
        <div className="turn-failure" role="status">
          <span className="turn-failure-text">
            这一轮没等到结果：{message.failure.message}
          </span>
          {message.failure.retryable && onRetry ? (
            <button
              type="button"
              className="turn-failure-retry"
              onClick={() => onRetry(message.id)}
            >
              重试
            </button>
          ) : null}
        </div>
      ) : null}
      {/* 操作条（spec-s8）：时间 / 复制 / 查看详情；流式中不渲染 */}
      {!message.streaming && <MessageActions message={message} />}
    </>
  );
}
