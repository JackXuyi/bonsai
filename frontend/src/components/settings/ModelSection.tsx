"use client";

// 设置页模型区（spec-m1 本地 / spec-m2 云端）：顶部「本地 | 云端」段选，
// 各自 pane 独立渲染（不用 hidden —— display:flex 会压掉 [hidden]）。
// 底部 Auto 开关：后台任务钉本地（plan-structured-routing T5）。
import { useModels } from "@/hooks/useModels";
import { useLlmStore } from "@/lib/stores/llmStore";
import { CloudSection } from "./CloudSection";
import { LocalModelPane } from "./LocalModelPane";

export function ModelSection() {
  useModels();
  const tab = useLlmStore((s) => s.modelTab);
  const setTab = useLlmStore((s) => s.setModelTab);
  const provider = useLlmStore((s) => s.provider);
  const backgroundLocalOnly = useLlmStore((s) => s.backgroundLocalOnly);
  const setBackgroundLocalOnly = useLlmStore((s) => s.setBackgroundLocalOnly);

  return (
    <div className="card">
      <div className="card-head">
        <span className="card-title">模型</span>
        <span className="card-sub">
          {provider === "cloud" ? "云端生效中 · 点卡片切换" : "点卡片即切换 · 即时生效"}
        </span>
      </div>

      <div className="ptabs" role="tablist" aria-label="模型来源">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "local"}
          className={tab === "local" ? "on" : ""}
          onClick={() => setTab("local")}
        >
          本地
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "cloud"}
          className={tab === "cloud" ? "on" : ""}
          onClick={() => setTab("cloud")}
        >
          云端
        </button>
      </div>

      {tab === "local" ? <LocalModelPane /> : <CloudSection />}

      <div className="routing-row" data-testid="routing-toggle">
        <label className="tgl" title="后台任务只用本地模型">
          <span className="master-label">后台任务只用本地模型</span>
          <input
            type="checkbox"
            checked={backgroundLocalOnly}
            onChange={(e) => void setBackgroundLocalOnly(e.target.checked)}
          />
          <span className="track">
            <span className="knob" />
          </span>
        </label>
        <p className="hint-line routing-hint">
          开启后，夜间记忆整理、总结与周报只在你的电脑上运行；对话仍按上方选择。
        </p>
      </div>
    </div>
  );
}
