"use client";

// 文件卡（P3-2，plan-files.md §3.6）：授权目录逐个添加（系统选择器，禁手输），
// 默认零授权——roots 为空时 provider 不挂载，模型根本看不到 fs 工具。
// 写入走 ask 档每次弹卡询问；cloud_read 关闭时文件内容不出机器。
import { useCallback, useEffect, useState } from "react";
import {
  fsPickDirectory,
  fsRootAdd,
  fsRootRemove,
  fsSetConfig,
  fsStatus,
} from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type { FsStatus } from "@/lib/types";

/** 后端 MAX_ROOTS = 5（plan §7-1）；前端提前拦，少一次无效往返。 */
const MAX_ROOTS = 5;

/** 错误形如 "fs_roots_max:最多 5 个目录" —— 剥掉 code 只留人话。 */
function errText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  return raw.includes(":") ? raw.split(":").slice(1).join(":") : raw;
}

export function FileCard({ active }: { active: boolean }) {
  const [st, setSt] = useState<FsStatus | null>(null);
  const [line, setLine] = useState("文件工具未开启");
  const windowShownAt = useUiStore((s) => s.windowShownAt);

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setLine("浏览器预览模式：后端未连接");
      return;
    }
    try {
      setSt(await fsStatus());
    } catch {
      setLine("文件工具状态读取失败");
    }
  }, []);

  useEffect(() => {
    if (!st) return;
    if (!st.enabled) {
      setLine("文件工具未开启（默认关闭）");
      return;
    }
    if (st.roots.length === 0) {
      setLine("还没授权任何目录——点「添加目录」开始");
      return;
    }
    setLine(
      `已授权 ${st.roots.length} 个目录 · 写/复制/移动只新建不覆盖 · 变更前询问`
    );
  }, [st]);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh, windowShownAt]);

  async function toggleEnabled(on: boolean) {
    if (!inTauri) return;
    try {
      setSt(await fsSetConfig({ enabled: on }));
    } catch {
      toast("文件工具设置失败");
    }
  }

  async function toggleCloudRead(on: boolean) {
    if (!inTauri) return;
    try {
      setSt(await fsSetConfig({ cloud_read: on }));
    } catch {
      toast("设置失败");
    }
  }

  async function addRoot() {
    if (!inTauri) return;
    let picked: string | null = null;
    try {
      picked = await fsPickDirectory();
    } catch {
      toast("目录选择器打不开");
      return;
    }
    if (!picked) return; // 用户取消
    try {
      setSt(await fsRootAdd(picked));
    } catch (e) {
      toast(errText(e));
    }
  }

  async function removeRoot(path: string) {
    if (!inTauri) return;
    try {
      setSt(await fsRootRemove(path));
    } catch (e) {
      toast(errText(e));
    }
  }

  const enabled = !!st?.enabled;
  const roots = st?.roots ?? [];
  const canAdd = enabled && roots.length < MAX_ROOTS;

  return (
    <section className="card" data-testid="file-card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">文件</p>
          <p className="card-sub">只在授权目录内找/读/写/复制/移动 · 可读 PDF 文本层 · 删除永远不做</p>
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
          <span className="sr-only">开启文件工具</span>
        </label>
      </div>

      <div className={`subs${enabled ? "" : " off"}`}>
        <p className="row-label">
          允许读写的目录（逐个添加，随时可删）
        </p>
        {roots.map((path) => (
          <div className="row" key={path}>
            <span className="row-label truncate max-w-full" title={path}>
              {path}
            </span>
            <button
              className="ob-btn cal-btn"
              onClick={() => removeRoot(path)}
              data-testid={`remove-root-${roots.indexOf(path)}`}
            >
              移除
            </button>
          </div>
        ))}
        {canAdd && (
          <div className="row">
            <button
              className="ob-btn cal-btn"
              onClick={addRoot}
              data-testid="add-root"
            >
              + 添加目录…
            </button>
          </div>
        )}
        <p className="row-label">
          ⚠️ 范围越大的目录风险越大，建议只给子目录
        </p>
        <label className="row">
          <span className="row-label">读取内容发送云端总结</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!st?.cloud_read}
              disabled={!enabled}
              onChange={(e) => toggleCloudRead(e.target.checked)}
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>
        <p className="row-label">
          关闭后：模型只知道文件存在，正文只在界面里看，不出这台电脑。可读文本与
          PDF 文本层；扫描件不支持 OCR
        </p>
      </div>

      <p className="status-line">{line}</p>
    </section>
  );
}
