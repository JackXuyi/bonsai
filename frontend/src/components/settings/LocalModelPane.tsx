"use client";

// 本地模型 pane（spec-m1 §3，从 ModelSection 抽出）：状态行 + 已装卡片 grid
// + 可下载列表；点卡片即切换，右键删除走两段式确认。
import { useState } from "react";
import { useLlmStore } from "@/lib/stores/llmStore";
import { InstalledModelGrid } from "./InstalledModelGrid";
import { DownloadList } from "./DownloadList";

export function LocalModelPane() {
  const {
    loaded,
    sidecarUp,
    installed,
    catalog,
    localModel,
    provider,
    testingId,
    pulls,
    lastError,
    activate,
    download,
    cancel,
    remove,
  } = useLlmStore();
  const [confirmTag, setConfirmTag] = useState<string | null>(null);

  async function confirmRemove() {
    if (!confirmTag) return;
    const tag = confirmTag;
    setConfirmTag(null);
    await remove(tag);
  }

  return (
    <>
      <div className={`model-status${sidecarUp ? "" : " off"}`}>
        <span className="dot" aria-hidden="true" />
        {sidecarUp
          ? `本地服务就绪 · 127.0.0.1:11435 · ${installed.length} 个模型`
          : "本地服务未就绪（模型不可切换）"}
      </div>
      {provider === "cloud" && (
        <p className="hint-line">
          当前生效的是云端模型；点下面任一卡片会切回本地。
        </p>
      )}

      {lastError && <p className="model-error">{lastError}</p>}

      {!loaded ? (
        <p className="hint-line">正在读模型列表…</p>
      ) : (
        <>
          {!sidecarUp && (
            <p className="hint-line">下面是推荐目录（只读）。本地服务起来后才能点选。</p>
          )}
          <p className="section-label">已装</p>
          <InstalledModelGrid
            installed={installed}
            catalog={catalog}
            activeName={provider === "ollama" ? localModel : ""}
            testingId={testingId}
            fallback={!sidecarUp}
            onPick={(tag) => void activate(tag)}
            onRemove={setConfirmTag}
          />

          {sidecarUp && (
            <>
              <p className="section-label">可下载（并发 1 · 下载走 Ollama registry）</p>
              <DownloadList
                catalog={catalog}
                installedNames={installed.map((m) => m.name)}
                pulls={pulls}
                onDownload={download}
                onCancel={cancel}
              />
            </>
          )}
        </>
      )}

      {confirmTag && (
        <div className="mconfirm" role="dialog" aria-modal="true">
          <div className="mconfirm-box">
            <p className="mconfirm-title">删掉 {confirmTag}？</p>
            <p className="mconfirm-sub">模型文件会从磁盘删掉，下次要用得重新下。</p>
            <div className="mconfirm-actions">
              <button type="button" className="btn-ghost" onClick={() => setConfirmTag(null)}>
                算了
              </button>
              <button type="button" className="btn-danger" onClick={confirmRemove}>
                删除
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
