"use client";

// 气泡内 markdown 渲染（2026-11-23 用户裁决：模型输出是 markdown，纯文本
// 渲染会把 **粗体**、列表等标记原样露出）。remark-gfm 支持表格 / 删除线 /
// 任务列表。样式在 styles/chat.css 的 .bubble .md 段。
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

export function MarkdownText({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
