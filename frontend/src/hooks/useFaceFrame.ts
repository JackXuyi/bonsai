"use client";

// 600ms 帧翻转（旧 Hero/main.js boot 段的 setInterval，两处脸共用）。
import { useEffect, useState } from "react";

const FRAME_INTERVAL_MS = 600;

export function useFaceFrame() {
  const [frame, setFrame] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setFrame((f) => f ^ 1), FRAME_INTERVAL_MS);
    return () => clearInterval(t);
  }, []);
  return frame;
}
