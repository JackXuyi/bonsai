"use client";

// 模型卡数据编排（spec-m1 §3）：进页加载 + 订阅 llm:pull:progress。
// 终态（done/error/cancelled）统一在订阅侧处理：done 刷新已装列表后清快照。
import { useEffect } from "react";
import { llmModelListInstalled } from "@/lib/commands";
import { useTauriEvent } from "@/lib/tauriListen";
import { toast } from "@/lib/stores/uiStore";
import { useLlmStore } from "@/lib/stores/llmStore";
import type { PullProgress } from "@/lib/types";

export function useModels() {
  const load = useLlmStore((s) => s.load);

  useEffect(() => {
    void load();
  }, [load]);

  useTauriEvent<PullProgress>("llm:pull:progress", (p) => {
    const st = useLlmStore.getState();
    st.applyPull(p);
    if (p.status === "done") {
      st.clearPull(p.tag);
      llmModelListInstalled()
        .then((installed) => useLlmStore.setState({ installed, sidecarUp: true }))
        .catch(() => st.refreshInstalled());
      toast(`下好了 ${p.tag}`);
    } else if (p.status === "error") {
      toast(`下载失败：${p.error ?? p.tag}`);
    } else if (p.status === "cancelled") {
      toast("下载已取消");
    } else if (p.status === "warn_disk") {
      toast("磁盘可能不够，还是继续下了");
    }
  });
}
