"use client";

// 邮件卡（P3-1，plan-mail/v1）：只读未读 + 草稿；默认关；仅 Mail.app。
import { useCallback, useEffect, useState } from "react";
import { mailProbe, mailSetEnabled, mailStatus } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type { MailStatus } from "@/lib/types";

export function MailCard({ active }: { active: boolean }) {
  const [st, setSt] = useState<MailStatus | null>(null);
  const [line, setLine] = useState("邮件未开启");
  const [probing, setProbing] = useState(false);
  const windowShownAt = useUiStore((s) => s.windowShownAt);

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setLine("浏览器预览模式：后端未连接");
      return;
    }
    try {
      setSt(await mailStatus());
    } catch {
      setLine("邮件状态读取失败");
    }
  }, []);

  useEffect(() => {
    if (!st) return;
    if (!st.supported) {
      setLine("仅支持 macOS Mail.app");
      return;
    }
    if (!st.enabled) {
      setLine("邮件未开启（默认关闭）");
      return;
    }
    setLine(
      `已开启 · 今日读 ${st.today_reads} 次 · 草稿 ${st.today_drafts} 次 · ${st.note}`
    );
  }, [st]);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh, windowShownAt]);

  async function toggleEnabled(on: boolean) {
    if (!inTauri) return;
    try {
      setSt(await mailSetEnabled(on));
    } catch {
      toast("邮件设置失败");
    }
  }

  async function probe() {
    if (!inTauri || probing) return;
    setProbing(true);
    try {
      const r = await mailProbe();
      toast(`读到 ${r.count} 封未读（未读正文）`);
      await refresh();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e));
    } finally {
      setProbing(false);
    }
  }

  const enabled = !!st?.enabled;

  return (
    <section className="card" data-testid="mail-card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">邮件（实验）</p>
          <p className="card-sub">只读未读 · 草稿不发送 · 仅 Mail.app</p>
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
          <span className="sr-only">开启邮件</span>
        </label>
      </div>

      <div className={`subs${enabled ? "" : " off"}`}>
        <p className="row-label">
          ⓘ 需要 Mail.app 正在运行，并授权「自动化」。主题与发件人仅当次可见，不进长期记忆。
        </p>
        <p className="row-label">草稿只会打开编辑窗口，绝不会代你发送。</p>
        {enabled && st?.supported && (
          <div className="row">
            <button
              className="ob-btn cal-btn"
              onClick={probe}
              disabled={probing}
            >
              {probing ? "读取中…" : "试读未读"}
            </button>
          </div>
        )}
      </div>

      <p className="status-line">{line}</p>
    </section>
  );
}
