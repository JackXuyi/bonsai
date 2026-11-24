"use client";

// 感知层卡：总开关 + 子源 + P2-4 窗口标题白名单 / 教名词。
import { useCallback, useEffect, useState } from "react";
import { perceptionGetConfig, perceptionSetConfig, perceptionStatus } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type { PerceptionConfig, TaughtNoun } from "@/lib/types";

function fmtIdle(secs: number) {
  if (!Number.isFinite(secs) || secs > 24 * 3600) return "未知";
  if (secs < 60) return "刚刚动过";
  const min = Math.floor(secs / 60);
  if (min < 60) return `空闲 ${min} 分钟`;
  return `空闲 ${Math.floor(min / 60)} 小时 ${min % 60} 分`;
}

function normalizeCfg(raw: PerceptionConfig): PerceptionConfig {
  return {
    enabled: !!raw.enabled,
    clipboard: !!raw.clipboard,
    active_app: !!raw.active_app,
    downloads: !!raw.downloads,
    window_title: !!raw.window_title,
    title_allowlist: raw.title_allowlist ?? [],
    taught_nouns: raw.taught_nouns ?? [],
  };
}

export function PerceptionCard({ active }: { active: boolean }) {
  const setToolResult = useUiStore((s) => s.setToolResult);
  const [cfg, setCfg] = useState<PerceptionConfig | null>(null);
  const [frontApp, setFrontApp] = useState<string | undefined>();
  const [frontTitle, setFrontTitle] = useState<string | null | undefined>();
  const [line, setLine] = useState("感知层未开启");
  const [dotOn, setDotOn] = useState(false);
  const [needle, setNeedle] = useState("");
  const [nounLabel, setNounLabel] = useState("");

  const refreshStatus = useCallback(async () => {
    if (!inTauri) {
      setLine("浏览器预览模式：后端未连接");
      setDotOn(false);
      return;
    }
    try {
      const s = await perceptionStatus();
      setFrontApp(s.front_app);
      setFrontTitle(s.front_title);
      if (!s.enabled) {
        setLine("感知层未开启（默认关闭）");
        setDotOn(false);
        return;
      }
      const parts = [fmtIdle(s.idle_secs ?? NaN)];
      if (s.front_app) parts.push(`前台 ${s.front_app}`);
      if (s.front_title) parts.push(`窗口 ${s.front_title}`);
      setLine(parts.join(" · "));
      setDotOn(true);
    } catch {
      setLine("感知状态读取失败");
      setDotOn(false);
    }
  }, []);

  const loadConfig = useCallback(async () => {
    if (!inTauri) return;
    try {
      setCfg(normalizeCfg(await perceptionGetConfig()));
    } catch {
      /* perception state not ready yet — the poll below will retry */
    }
  }, []);

  useEffect(() => {
    if (active) refreshStatus();
  }, [active, refreshStatus]);

  useEffect(() => {
    loadConfig();
    refreshStatus();
    const t = setInterval(refreshStatus, 15000);
    return () => clearInterval(t);
  }, [loadConfig, refreshStatus]);

  async function save(next: PerceptionConfig, masterFlip?: boolean) {
    const normalized = normalizeCfg(next);
    setCfg(normalized);
    if (masterFlip !== undefined) {
      toast(
        masterFlip ? "感知层已开启 · 原文不出机器" : "感知层已关闭"
      );
    }
    if (!inTauri) return;
    try {
      await perceptionSetConfig(normalized);
    } catch (err) {
      setToolResult({ title: "感知设置保存失败", text: String(err) });
      toast("感知设置没存上");
    }
    refreshStatus();
  }

  const enabled = !!cfg?.enabled;
  const off = !enabled;
  const windowOn = !!cfg?.window_title;
  const allowlist = cfg?.title_allowlist ?? [];
  const nouns = cfg?.taught_nouns ?? [];

  function addFrontToAllowlist() {
    if (!cfg || !frontApp?.trim()) {
      toast("还没有读到前台应用");
      return;
    }
    const name = frontApp.trim();
    if (allowlist.some((a) => a.toLowerCase() === name.toLowerCase())) {
      toast("已在白名单里");
      return;
    }
    save({ ...cfg, title_allowlist: [...allowlist, name] });
  }

  function removeAllowlisted(name: string) {
    if (!cfg) return;
    save({
      ...cfg,
      title_allowlist: allowlist.filter((a) => a !== name),
    });
  }

  function addTaughtNoun() {
    if (!cfg) return;
    const n = needle.trim();
    const l = nounLabel.trim();
    if (!n || !l) {
      toast("填写匹配词和粗标签");
      return;
    }
    const next: TaughtNoun[] = [
      ...nouns.filter((x) => x.needle.toLowerCase() !== n.toLowerCase()),
      { needle: n, label: l },
    ];
    save({ ...cfg, taught_nouns: next });
    setNeedle("");
    setNounLabel("");
  }

  function removeTaught(n: string) {
    if (!cfg) return;
    save({
      ...cfg,
      taught_nouns: nouns.filter((x) => x.needle !== n),
    });
  }

  return (
    <section className="card" data-testid="perception-card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">感知层</p>
          <p className="card-sub">默认关闭 · 原文不出机器</p>
        </div>
        <label className="tgl tgl-master" title="总开关">
          <span className="master-label">总开关</span>
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) =>
              save(
                {
                  enabled: e.target.checked,
                  clipboard: !!cfg?.clipboard,
                  active_app: !!cfg?.active_app,
                  downloads: !!cfg?.downloads,
                  window_title: !!cfg?.window_title,
                  title_allowlist: allowlist,
                  taught_nouns: nouns,
                },
                e.target.checked
              )
            }
          />
          <span className="track">
            <span className="knob" />
          </span>
          <span className="sr-only">开启感知层</span>
        </label>
      </div>

      <div className={`subs${off ? " off" : ""}`}>
        <label className="row">
          <span className="row-label">剪贴板</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!cfg?.clipboard}
              disabled={off}
              onChange={(e) =>
                cfg && save({ ...cfg, clipboard: e.target.checked })
              }
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>

        <label className="row">
          <span className="row-label">前台应用</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!cfg?.active_app}
              disabled={off}
              onChange={(e) =>
                cfg && save({ ...cfg, active_app: e.target.checked })
              }
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>

        <label className="row">
          <span className="row-label">下载目录</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!cfg?.downloads}
              disabled={off}
              onChange={(e) =>
                cfg && save({ ...cfg, downloads: e.target.checked })
              }
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>

        <label className="row">
          <span className="row-label">窗口标题（白名单）</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={windowOn}
              disabled={off}
              onChange={(e) =>
                cfg && save({ ...cfg, window_title: e.target.checked })
              }
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>

        {windowOn && !off && (
          <div className="perc-screen">
            <p className="row-label">
              ⓘ 仅勾选的 App 会记窗口粗标签；未勾选的 App 事件里没有窗口信息。可能需要「屏幕录制」权限才能读到标题。
            </p>
            <div className="row">
              <button
                type="button"
                className="ob-btn cal-btn"
                onClick={addFrontToAllowlist}
              >
                加入当前前台{frontApp ? `（${frontApp}）` : ""}
              </button>
            </div>
            {allowlist.length > 0 && (
              <ul className="row-label perc-screen-list">
                {allowlist.map((name) => (
                  <li key={name}>
                    {name}{" "}
                    <button
                      type="button"
                      className="ob-btn cal-btn"
                      onClick={() => removeAllowlisted(name)}
                    >
                      移除
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <p className="row-label">教名词：标题含匹配词时，只存你给的粗标签</p>
            <div className="perc-screen-form">
              <input
                className="ob-input"
                placeholder="匹配词"
                value={needle}
                onChange={(e) => setNeedle(e.target.value)}
                aria-label="教名词匹配词"
              />
              <input
                className="ob-input"
                placeholder="粗标签"
                value={nounLabel}
                onChange={(e) => setNounLabel(e.target.value)}
                aria-label="教名词粗标签"
              />
              <button type="button" className="ob-btn cal-btn" onClick={addTaughtNoun}>
                教给盆栽
              </button>
            </div>
            {nouns.length > 0 && (
              <ul className="row-label perc-screen-list">
                {nouns.map((n) => (
                  <li key={n.needle}>
                    「{n.needle}」→ {n.label}{" "}
                    <button
                      type="button"
                      className="ob-btn cal-btn"
                      onClick={() => removeTaught(n.needle)}
                    >
                      删除
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {frontTitle ? (
              <p className="row-label">当前窗口粗标签：{frontTitle}</p>
            ) : null}
          </div>
        )}
      </div>

      <p className="status-line">
        <span className={`dot${dotOn ? " on" : ""}`} />
        <span>{line}</span>
      </p>
    </section>
  );
}
