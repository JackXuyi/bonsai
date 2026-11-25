"use client";

// composer：浮起条 + ModelChip + IME guard + 发送/停（plan-chat-first-ui v2）。
// 授权条挂在 dock 顶部（plan-chat-agent-inline）。
// ChatPet 贴在输入框右上角（注视感）。
import { useCallback, useRef, useState } from "react";
import { shouldBlockSubmit } from "@/lib/ime";
import { ModelChip } from "@/components/chat/ModelChip";
import { ChatPet } from "@/components/chat/ChatPet";
import { InlineApprovalBar } from "@/components/agent/InlineApprovalBar";

export function Composer({
  onSend,
  onCancel,
  chatBusy,
  showPet = true,
  inputRef,
}: {
  onSend: (text: string) => void;
  /** 回合进行中显示「停」：叫停后端，不是撤气泡 */
  onCancel?: () => void;
  chatBusy: boolean;
  /** 空会话不显示输入框右上角偷看脸 */
  showPet?: boolean;
  inputRef?: React.RefObject<HTMLTextAreaElement>;
}) {
  const innerRef = useRef<HTMLTextAreaElement>(null);
  const ref = inputRef ?? innerRef;
  const [value, setValue] = useState("");
  /** 待批时藏偷看脸，避免盖住允许/拒绝 */
  const [approvalActive, setApprovalActive] = useState(false);
  const onApprovalActive = useCallback((active: boolean) => {
    setApprovalActive(active);
  }, []);
  const canSend = value.trim().length > 0 && !chatBusy;
  const petVisible = showPet && !approvalActive;

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
    <div className="composer-dock">
      <InlineApprovalBar placement="dock" onActiveChange={onApprovalActive} />
      <div className="composer-stage">
        {petVisible ? <ChatPet chatting={chatBusy} /> : null}
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
          <ModelChip />
          {chatBusy && onCancel ? (
            <button
              type="button"
              className="send stop"
              aria-label="停止"
              title="叫停这一轮"
              onClick={onCancel}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <rect x="7" y="7" width="10" height="10" rx="2" />
              </svg>
            </button>
          ) : (
            <button type="submit" className="send" aria-label="发送" disabled={!canSend}>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <line x1="12" y1="19" x2="12" y2="6" />
                <polyline points="6 12 12 6 18 12" />
              </svg>
            </button>
          )}
        </form>
      </div>
    </div>
  );
}
