"use client";

// 边界清单卡（旧 boundaries 段）：点 × 删除，删除后重拉。
import { useCallback, useEffect, useState } from "react";
import { boundaryDelete, boundaryList } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast } from "@/lib/stores/uiStore";
import type { Boundary } from "@/lib/types";

const DEMO = ["不要主动提：我妈", "不要说：你应该冷静"];

export function BoundaryCard({ active }: { active: boolean }) {
  const [rows, setRows] = useState<Boundary[] | null>(null);
  const [error, setError] = useState(false);

  const load = useCallback(async () => {
    if (!inTauri) {
      setError(false);
      return;
    }
    try {
      setRows(await boundaryList());
      setError(false);
    } catch {
      setRows([]);
      setError(true);
    }
  }, []);

  useEffect(() => {
    if (active) load();
  }, [active, load]);

  // 同 PageHeader：inTauri 在预渲染期是 false，水合期可能是 true，
  // DEMO 数据必须等挂载后再上，否则预渲染 HTML（空列表）和首帧不一致。
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);

  const list = rows ?? (mounted && !inTauri ? DEMO.map((c, i) => ({ id: -1 - i, content: c })) : []);

  async function del(id: number) {
    try {
      await boundaryDelete(id);
      toast("边界已删除");
    } catch {
      toast("删除失败");
    }
    load();
  }

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">边界清单</p>
          <p className="card-sub">它不许自己碰的线 · 点 × 删除</p>
        </div>
      </div>
      <div className="subs">
        {list.map((r) => (
          <div className="boundary-row" key={r.id}>
            <span className="row-label">{r.content}</span>
            {r.id > 0 && (
              <button
                className="boundary-del"
                aria-label="删除这条边界"
                onClick={() => del(r.id)}
              >
                ×
              </button>
            )}
          </div>
        ))}
      </div>
      {error ? (
        <p className="status-line">边界清单读取失败</p>
      ) : (
        list.length === 0 && <p className="status-line">还没有任何边界记录</p>
      )}
    </section>
  );
}
