"use client";

// 预测-验证卡（旧 addPrediction 的三个 verdict 按钮；反馈直达记忆层）。
import { useState } from "react";
import { predictionFeedback } from "@/lib/commands";

const VERDICTS = [
  { key: "hit", label: "猜对了" },
  { key: "miss", label: "猜错了" },
  { key: "shut_up", label: "别烦我" },
];
const VERDICT_NOTES: Record<string, string> = {
  hit: "已记下：猜对了。它会把这条学得更牢。",
  miss: "已记下：猜错了。它会降低这类猜测。",
  shut_up: "已记下。这类猜测先停 3 天。",
};

export function PredictionCard({ id, onStick }: { id: number; onStick?: () => void }) {
  const [badge, setBadge] = useState("它猜的 · 预测验证");
  const [disabled, setDisabled] = useState(false);

  async function vote(key: string) {
    setDisabled(true);
    try {
      const applied = await predictionFeedback(id, key);
      setBadge(
        applied ? VERDICT_NOTES[key] : "这条已经过了反馈窗口（48 小时），没有记。"
      );
    } catch (err) {
      setBadge(`反馈失败：${err}`);
      setDisabled(false);
    }
    onStick?.();
  }

  return (
    <>
      <br />
      <span className="badge">{badge}</span>
      <div className="predict-row">
        {VERDICTS.map((v) => (
          <button
            key={v.key}
            className="chip"
            disabled={disabled}
            onClick={() => vote(v.key)}
          >
            {v.label}
          </button>
        ))}
      </div>
    </>
  );
}
