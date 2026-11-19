"use client";

// composer：自适应高 + IME guard + 发送按钮（旧 autosize / keydown / submit）。
import { useRef, useState } from "react";
import { shouldBlockSubmit } from "@/lib/ime";

export function Composer({
  onSend,
  chatBusy,
  inputRef,
}: {
  onSend: (text: string) => void;
  chatBusy: boolean;
  inputRef?: React.RefObject<HTMLTextAreaElement>;
}) {
  const innerRef = useRef<HTMLTextAreaElement>(null);
  const ref = inputRef ?? innerRef;
  const [value, setValue] = useState("");
  const canSend = value.trim().length > 0 && !chatBusy;

  function autosize() {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 66)}px`;
  }

  function submit() {
    const text = value;
    setValue("");
    autosize();
    onSend(text);
  }

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        if (value.trim()) submit();
      }}
    >
      <textarea
        ref={ref}
        rows={1}
        placeholder="说点什么…"
        autoComplete="off"
        spellCheck={false}
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          autosize();
        }}
        onKeyDown={(e) => {
          if (shouldBlockSubmit(e.nativeEvent.isComposing ?? false, e.key, e.shiftKey))
            return;
          e.preventDefault();
          submit();
        }}
      />
      <button type="submit" className="send" aria-label="发送" disabled={!canSend}>
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <line x1="12" y1="19" x2="12" y2="6" />
          <polyline points="6 12 12 6 18 12" />
        </svg>
      </button>
    </form>
  );
}
