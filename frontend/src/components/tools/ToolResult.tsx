"use client";

// 工具动作结果卡（旧 resultShell / showResult / showStats）。
import { useEffect, useRef } from "react";
import { useUiStore } from "@/lib/stores/uiStore";

export function ToolResult() {
  const result = useUiStore((s) => s.toolResult);
  const setToolResult = useUiStore((s) => s.setToolResult);
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    if (result) ref.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [result]);

  if (!result) return null;

  return (
    <section className="result" ref={ref as React.RefObject<HTMLElement>}>
      <div className="result-head">
        <span className="result-title">{result.title}</span>
        <button
          className="result-close"
          aria-label="收起"
          onClick={() => setToolResult(null)}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>
      {result.pairs ? (
        <div className="stats">
          {result.pairs.map(([k, v]) => (
            <div className="stat" key={k}>
              <span>{k}</span>
              <b>{v}</b>
            </div>
          ))}
        </div>
      ) : (
        <div className="result-text">{result.text}</div>
      )}
    </section>
  );
}
