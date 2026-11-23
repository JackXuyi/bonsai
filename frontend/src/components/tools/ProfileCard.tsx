"use client";

// 用户画像卡（plan-user-profile.md §6.2）：工具页展示 + 行内编辑。
// 与 BoundaryCard 同款交互范式（active 触发 load / 删除重拉），但刻意
// 不放 DEMO 假数据——画像是事实数据，假数据会误导。
// key→中文标签与后端 profile::label 一一对应（Rust 侧渲染 prompt 用
// Rust 侧的，前端展示用这份，两处都在各自模块的单一位置）。
import { useCallback, useEffect, useState } from "react";
import { profileDelete, profileList, profileSet } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast } from "@/lib/stores/uiStore";
import type { ProfileEntry } from "@/lib/types";

const KEY_LABELS: Record<string, string> = {
  name: "称呼",
  occupation: "职业",
  tools: "常用工具",
  city: "城市",
  projects: "在做的事",
  schedule: "作息",
};

function sourceBadge(source: string): string {
  return source === "manual" ? "你写的" : "聊天里记的";
}

export function ProfileCard({ active }: { active: boolean }) {
  const [rows, setRows] = useState<ProfileEntry[] | null>(null);
  const [error, setError] = useState(false);
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [editValue, setEditValue] = useState("");

  const load = useCallback(async () => {
    if (!inTauri) {
      setError(false);
      return;
    }
    try {
      setRows(await profileList());
      setError(false);
    } catch {
      setRows([]);
      setError(true);
    }
  }, []);

  useEffect(() => {
    if (active) load();
  }, [active, load]);

  async function save(key: string) {
    const value = editValue.trim();
    setEditingKey(null);
    if (!value) return;
    try {
      await profileSet(key, value);
      toast("画像已更新");
    } catch {
      toast("保存失败");
    }
    load();
  }

  async function clear(key: string) {
    try {
      await profileDelete(key);
      toast("已清除这条画像");
    } catch {
      toast("删除失败");
    }
    load();
  }

  // 只展示白名单 key（后端本就只写白名单，前端再兜一层未知 key 丢弃）。
  const list = (rows ?? []).filter((r) => r.key in KEY_LABELS);

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">用户画像</p>
          <p className="card-sub">它记住了这些 · 点「改」修正 · 记忆可看 = 信任</p>
        </div>
      </div>
      <div className="subs">
        {list.map((r) =>
          editingKey === r.key ? (
            <div className="profile-row profile-editing" key={r.key}>
              <span className="row-label">{KEY_LABELS[r.key]}</span>
              <input
                className="profile-input"
                aria-label={`修改${KEY_LABELS[r.key]}`}
                value={editValue}
                autoFocus
                onChange={(e) => setEditValue(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void save(r.key);
                  if (e.key === "Escape") setEditingKey(null);
                }}
              />
              <button className="profile-op" onClick={() => void save(r.key)}>
                保存
              </button>
              <button
                className="profile-op"
                onClick={() => setEditingKey(null)}
              >
                取消
              </button>
            </div>
          ) : (
            <div className="profile-row" key={r.key}>
              <span className="profile-key">{KEY_LABELS[r.key]}</span>
              <span className="row-label">{r.value}</span>
              <span className="profile-badge">{sourceBadge(r.source)}</span>
              <button
                className="profile-op"
                aria-label={`修改${KEY_LABELS[r.key]}`}
                onClick={() => {
                  setEditingKey(r.key);
                  setEditValue(r.value);
                }}
              >
                改
              </button>
              <button
                className="profile-op profile-clear"
                aria-label={`清除${KEY_LABELS[r.key]}`}
                onClick={() => void clear(r.key)}
              >
                清
              </button>
            </div>
          ),
        )}
      </div>
      {error ? (
        <p className="status-line">用户画像读取失败</p>
      ) : (
        rows !== null && list.length === 0 && (
          <p className="status-line">还没有任何画像——聊聊天它就慢慢认识你了</p>
        )
      )}
    </section>
  );
}
