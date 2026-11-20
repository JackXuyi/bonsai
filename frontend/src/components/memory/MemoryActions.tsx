"use client";

// 记忆页动作（spec-m3 §2）：「整理记忆」跑一次 consolidate；「忘记全部」
// 两段式确认（首次点击变「确认忘掉一切？」，4 秒未确认自动回弹）。
//
// 回调可注入：默认直连命令，测试里换成 mock——组件本身不碰 IPC。
import { useEffect, useRef, useState } from "react";
import { memoryConsolidate, memoryForgetAll } from "@/lib/commands";
import { toast } from "@/lib/stores/uiStore";
import type { ConsolidateResult } from "@/lib/types";

/** 二段确认的窗口：够看清，不够误触。 */
export const FORGET_CONFIRM_MS = 4000;

interface MemoryActionsProps {
  onChanged: () => void;
  runConsolidate?: () => Promise<ConsolidateResult>;
  runForgetAll?: () => Promise<void>;
}

export function MemoryActions({
  onChanged,
  runConsolidate = memoryConsolidate,
  runForgetAll = memoryForgetAll,
}: MemoryActionsProps) {
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState<null | "consolidate" | "forget">(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    []
  );

  function disarm() {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    setArmed(false);
  }

  function forgetClick() {
    if (busy) return;
    if (!armed) {
      setArmed(true);
      timer.current = setTimeout(() => setArmed(false), FORGET_CONFIRM_MS);
      return;
    }
    disarm();
    setBusy("forget");
    void (async () => {
      try {
        await runForgetAll();
        toast("已忘记一切");
        onChanged();
      } catch (err) {
        toast(`没忘掉：${err}`);
      } finally {
        setBusy(null);
      }
    })();
  }

  function consolidateClick() {
    if (busy) return;
    setBusy("consolidate");
    void (async () => {
      try {
        const r = await runConsolidate();
        toast(`整理完了：抽取 ${r.extracted} · 合并 ${r.merged} · 升级 ${r.promoted}`);
        onChanged();
      } catch (err) {
        toast(`整理失败：${err}`);
      } finally {
        setBusy(null);
      }
    })();
  }

  return (
    <div className="mem-actions">
      <button
        type="button"
        className="btn"
        onClick={consolidateClick}
        disabled={busy !== null}
        aria-label="整理记忆"
      >
        {busy === "consolidate" ? "整理中…" : "整理记忆"}
      </button>
      <button
        type="button"
        className={`btn ${armed ? "btn-danger" : "btn-ghost"}`}
        onClick={forgetClick}
        disabled={busy !== null}
        aria-label="忘记全部"
      >
        {busy === "forget" ? "清空中…" : armed ? "确认忘掉一切？" : "忘记全部…"}
      </button>
      <p className="mem-hint">
        记忆只读：能看不能改。想删单条边界去工具页；「忘记全部」会连同人格成长一起清空。
      </p>
    </div>
  );
}
