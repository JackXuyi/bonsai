"use client";

// Web 检索卡（P3-3，plan-web.md §3.7）：只有总开关一个状态（web 无授权
// 列表、无目录管理）。默认关——搜索词会发给搜索引擎、网页内容会发给云端
// 模型，开启是知情选择；断网/失败一律结构化报错，阿盆不会编造搜索结果。
import { useCallback, useEffect, useState } from "react";
import { webSetConfig, webStatus } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type { WebStatus } from "@/lib/types";

export function WebCard({ active }: { active: boolean }) {
  const [st, setSt] = useState<WebStatus | null>(null);
  const [line, setLine] = useState("Web 检索未开启");
  const windowShownAt = useUiStore((s) => s.windowShownAt);

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setLine("浏览器预览模式：后端未连接");
      return;
    }
    try {
      setSt(await webStatus());
    } catch {
      setLine("Web 检索状态读取失败");
    }
  }, []);

  useEffect(() => {
    if (!st) return;
    setLine(
      st.enabled
        ? "已开启 · 阿盆可以搜索网页、读取网页正文并总结"
        : "Web 检索未开启（默认关闭）"
    );
  }, [st]);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh, windowShownAt]);

  async function toggleEnabled(on: boolean) {
    if (!inTauri) return;
    try {
      setSt(await webSetConfig({ enabled: on }));
    } catch {
      toast("Web 检索设置失败");
    }
  }

  const enabled = !!st?.enabled;

  return (
    <section className="card" data-testid="web-card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">Web 检索</p>
          <p className="card-sub">搜索网页 · 读取正文 · 总结引用</p>
        </div>
        <label className="tgl tgl-master" title="总开关">
          <span className="master-label">总开关</span>
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => toggleEnabled(e.target.checked)}
          />
          <span className="track">
            <span className="knob" />
          </span>
          <span className="sr-only">开启 Web 检索</span>
        </label>
      </div>

      <div className={`subs${enabled ? "" : " off"}`}>
        <p className="row-label">
          ⓘ 搜索词会发送给搜索引擎；网页内容会发送给已配置的云端模型（如
          DeepSeek）用于总结
        </p>
        <p className="row-label">
          断网或搜索失败时阿盆会明说，不会编造搜索结果；网页内容不存进记忆
        </p>
      </div>

      <p className="status-line">{line}</p>
    </section>
  );
}
