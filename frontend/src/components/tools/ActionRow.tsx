"use client";

// 工具动作行（旧 tools 三按钮：日报 / 整理记忆 / 主动说一句）+ busy 态。
import { useState } from "react";
import { useRouter } from "next/navigation";
import { dailyReport, dialogueProactiveNow, memoryConsolidate } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { useChatStore } from "@/lib/stores/chatStore";
import { toast, useUiStore, type ToolResult } from "@/lib/stores/uiStore";
import type { ConsolidateResult } from "@/lib/types";

const ICONS = {
  report: (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <polyline points="14 2 14 8 20 8" />
      <line x1="16" y1="13" x2="8" y2="13" />
      <line x1="16" y1="17" x2="8" y2="17" />
    </svg>
  ),
  consolidate: (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12 2 2 7l10 5 10-5-10-5z" />
      <path d="m2 17 10 5 10-5" />
      <path d="m2 12 10 5 10-5" />
    </svg>
  ),
  proactive: (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M21 11.5a8.4 8.4 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.4 8.4 0 0 1-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.4 8.4 0 0 1 3.8-.9h.5a8.5 8.5 0 0 1 8 8v.5z" />
    </svg>
  ),
};

export function ActionRow({ onReport }: { onReport: (md: string) => void }) {
  const [busy, setBusy] = useState<string | null>(null);
  const setToolResult = useUiStore((s) => s.setToolResult);
  const router = useRouter();

  async function run(key: string, fn: () => Promise<void>) {
    if (!inTauri) {
      toast("浏览器预览：后端没连上");
      return;
    }
    if (busy) return;
    setBusy(key);
    try {
      await fn();
    } catch (err) {
      setToolResult({ title: "出错了", text: String(err) });
      toast("操作失败");
    } finally {
      setBusy(null);
    }
  }

  const pairs = (r: ConsolidateResult): [string, string][] => [
    ["触发", r.trigger],
    ["窗口事件", String(r.events_in_window)],
    ["抽取", String(r.extracted)],
    ["合并", String(r.merged)],
    ["矛盾消解", String(r.contradicted)],
    ["升格", String(r.promoted)],
    ["删除", String(r.deleted)],
    ["消费", String(r.consumed)],
  ];

  return (
    <div className="tools">
      <button
        className={`tool${busy === "report" ? " busy" : ""}`}
        disabled={!!busy}
        onClick={() =>
          run("report", async () => {
            const md = await dailyReport();
            onReport(md);
          })
        }
      >
        <span className="tool-ico">{ICONS.report}</span>
        <span className="tool-name">今日日报</span>
        <span className="tool-sub">今天聊了什么</span>
      </button>
      <button
        className={`tool${busy === "consolidate" ? " busy" : ""}`}
        disabled={!!busy}
        onClick={() =>
          run("consolidate", async () => {
            const r = await memoryConsolidate();
            const stats: ToolResult = { title: "记忆整理", pairs: pairs(r) };
            setToolResult(stats);
            toast("记忆整理完成");
          })
        }
      >
        <span className="tool-ico">{ICONS.consolidate}</span>
        <span className="tool-name">整理记忆</span>
        <span className="tool-sub">抽取 · 合并 · 消解</span>
      </button>
      <button
        className={`tool${busy === "proactive" ? " busy" : ""}`}
        disabled={!!busy}
        onClick={() =>
          run("proactive", async () => {
            const reply = await dialogueProactiveNow();
            if (reply) {
              useChatStore.getState().add({
                kind: "bot",
                text: reply.text,
                proactive: true,
                turnId: reply.turnId,
                note: "主动搭话 · 你手动要的",
              });
              router.push("/");
              toast("它开口了");
            } else {
              toast("它没说话：本地模型没起来");
            }
          })
        }
      >
        <span className="tool-ico">{ICONS.proactive}</span>
        <span className="tool-name">主动说一句</span>
        <span className="tool-sub">看它想说什么</span>
      </button>
    </div>
  );
}
