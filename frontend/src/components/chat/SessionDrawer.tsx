"use client";

// 会话抽屉（docs/zh/plan-context.md §8，D7 裁决：抽屉，不加侧栏项）。
//
// 只做四件事：看列表 / 切 / 改名 / 归档 / 删。不做搜索、标签、置顶、拖拽
// ——个人使用里会话很难超过几十条，加了就是负担（§8 末段）。
//
// 两条硬约束：
// - 一轮在飞时 `busy` 为真，切/建/删全部置灰（A13）。**不静默 cancel**：
//   用户可能只是手滑，真要走先点输入框旁的「停」。
// - 删除是合规级联（后端 forget_session），二次确认必须说清「连带清掉
//   生成记录」，不能只写「删除会话？」。
import { useEffect, useState } from "react";
import {
  DEFAULT_SESSION_ID,
  TITLE_MAX_CHARS,
  dayLabel,
  sessionTitle,
  splitSessions,
  useSessionStore,
} from "@/lib/stores/sessionStore";
import type { SessionRow } from "@/lib/types";

interface SessionDrawerProps {
  open: boolean;
  onClose: () => void;
  /** 一轮对话在飞：所有改变当前会话的操作都拒绝（A13） */
  busy: boolean;
}

export function SessionDrawer({ open, onClose, busy }: SessionDrawerProps) {
  const list = useSessionStore((s) => s.list);
  const currentId = useSessionStore((s) => s.currentId);
  const pending = useSessionStore((s) => s.pending);
  const load = useSessionStore((s) => s.load);

  /** 展开「…」的行（一次只开一个，点别处即收） */
  const [menuId, setMenuId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirming, setConfirming] = useState<SessionRow | null>(null);

  // 每次打开都重拉：标题/条数是别的窗口写进去的，缓存久了会显示旧数字。
  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  useEffect(() => {
    if (!open) {
      setMenuId(null);
      setEditingId(null);
      setConfirming(null);
      return;
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !confirming) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, confirming, onClose]);

  const { active, archived } = splitSessions(list);
  const locked = busy || pending;

  function pick(id: string) {
    if (locked) return;
    useSessionStore.getState().switchTo(id);
    onClose();
  }

  async function onCreate() {
    if (locked) return;
    const id = await useSessionStore.getState().create();
    if (id) onClose();
  }

  function startRename(row: SessionRow) {
    setMenuId(null);
    setEditingId(row.id);
    setDraft(row.title);
  }

  async function commitRename() {
    const id = editingId;
    if (!id) return;
    setEditingId(null);
    const title = draft;
    setDraft("");
    await useSessionStore.getState().rename(id, title);
  }

  async function onArchive(row: SessionRow) {
    setMenuId(null);
    await useSessionStore.getState().archive(row.id, !row.archived);
    // 归档的正是当前会话：归档不等于删除，视图留在原地即可（列表里会移到
    // 折叠区），不强行切走——用户可能只是整理，没打算离开。
  }

  async function onDelete() {
    const row = confirming;
    if (!row) return;
    setConfirming(null);
    try {
      // 返回的是删掉的事件条数；删掉的若是当前会话，store 会切回默认会话，
      // ChatPane 的 currentId effect 随即清空消息流并回放默认会话。
      await useSessionStore.getState().remove(row.id);
    } catch {
      // store 已经 toast 过了，这里只负责收弹层。
    }
  }

  return (
    <>
      <div
        className={`sess-scrim ${open ? "on" : ""}`}
        onClick={onClose}
        aria-hidden="true"
      />
      <aside
        className={`sess-drawer ${open ? "on" : ""}`}
        aria-hidden={!open}
        aria-label="会话列表"
      >
        <div className="sess-head">
          <span className="sess-head-title">会话</span>
          <span className="sess-head-count">{active.length} 个</span>
        </div>
        <button
          type="button"
          className="sess-new"
          onClick={() => void onCreate()}
          disabled={locked}
          title={busy ? "等这一轮说完再新建" : "新建会话"}
        >
          ＋ 新对话
        </button>

        <div className="sess-list" onClick={() => setMenuId(null)}>
          {active.length === 0 && (
            <p className="sess-empty">
              {busy ? "等它说完就能开新会话。" : "还没有别的会话。"}
            </p>
          )}
          {active.map((row) => (
            <SessionItem
              key={row.id}
              row={row}
              current={row.id === currentId}
              locked={locked}
              menuOpen={menuId === row.id}
              editing={editingId === row.id}
              draft={draft}
              onDraft={setDraft}
              onPick={() => pick(row.id)}
              onMenu={() => setMenuId(menuId === row.id ? null : row.id)}
              onRename={() => startRename(row)}
              onCommitRename={() => void commitRename()}
              onCancelRename={() => {
                setEditingId(null);
                setDraft("");
              }}
              onArchive={() => void onArchive(row)}
              onDelete={() => {
                setMenuId(null);
                setConfirming(row);
              }}
            />
          ))}
        </div>

        {archived.length > 0 && (
          <details className="sess-arch">
            <summary>已归档（{archived.length}）</summary>
            <div className="sess-list">
              {archived.map((row) => (
                <SessionItem
                  key={row.id}
                  row={row}
                  current={row.id === currentId}
                  locked={locked}
                  menuOpen={menuId === row.id}
                  editing={editingId === row.id}
                  draft={draft}
                  onDraft={setDraft}
                  onPick={() => pick(row.id)}
                  onMenu={() => setMenuId(menuId === row.id ? null : row.id)}
                  onRename={() => startRename(row)}
                  onCommitRename={() => void commitRename()}
                  onCancelRename={() => {
                    setEditingId(null);
                    setDraft("");
                  }}
                  onArchive={() => void onArchive(row)}
                  onDelete={() => {
                    setMenuId(null);
                    setConfirming(row);
                  }}
                />
              ))}
            </div>
          </details>
        )}

        {busy && <p className="sess-hint">等它说完再切换。</p>}
      </aside>

      {confirming && (
        <div className="mconfirm" role="dialog" aria-modal="true" aria-label="删除会话">
          <div className="mconfirm-box">
            <p className="mconfirm-title">删除「{sessionTitle(confirming)}」？</p>
            <p className="mconfirm-sub">
              这会连带清掉这段对话的生成记录（链路追踪、任务卡），删了找不回来。
              只是想收起来的话，选「归档」。
            </p>
            <div className="mconfirm-actions">
              <button type="button" className="btn-ghost" onClick={() => setConfirming(null)}>
                算了
              </button>
              <button type="button" className="btn-danger" onClick={() => void onDelete()}>
                删除
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

interface ItemProps {
  row: SessionRow;
  current: boolean;
  locked: boolean;
  menuOpen: boolean;
  editing: boolean;
  draft: string;
  onDraft: (v: string) => void;
  onPick: () => void;
  onMenu: () => void;
  onRename: () => void;
  onCommitRename: () => void;
  onCancelRename: () => void;
  onArchive: () => void;
  onDelete: () => void;
}

/** 单行：标题 + 时间/条数；行尾「…」展开改名 / 归档 / 删除。
 *  `default` 会话不可删不可归档（它是老数据的锚点，§6.7）。 */
function SessionItem({
  row,
  current,
  locked,
  menuOpen,
  editing,
  draft,
  onDraft,
  onPick,
  onMenu,
  onRename,
  onCommitRename,
  onCancelRename,
  onArchive,
  onDelete,
}: ItemProps) {
  const isDefault = row.id === DEFAULT_SESSION_ID;
  return (
    <div className={`sess-row ${current ? "on" : ""}`}>
      {editing ? (
        <input
          className="sess-edit"
          autoFocus
          value={draft}
          maxLength={TITLE_MAX_CHARS}
          onChange={(e) => onDraft(e.target.value)}
          onBlur={onCommitRename}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              onCommitRename();
            } else if (e.key === "Escape") {
              e.preventDefault();
              onCancelRename();
            }
          }}
        />
      ) : (
        <button
          type="button"
          className="sess-main"
          onClick={onPick}
          disabled={locked}
          title={locked ? "等这一轮说完再切换" : sessionTitle(row)}
        >
          <span className="sess-row-title">{sessionTitle(row)}</span>
          <span className="sess-row-meta">
            {dayLabel(row.updatedAt)} · {row.messageCount} 条
          </span>
        </button>
      )}

      {!editing && (
        <button
          type="button"
          className="sess-more"
          aria-label="会话操作"
          aria-expanded={menuOpen}
          onClick={(e) => {
            e.stopPropagation();
            onMenu();
          }}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="6" cy="12" r="1" />
            <circle cx="12" cy="12" r="1" />
            <circle cx="18" cy="12" r="1" />
          </svg>
        </button>
      )}

      {menuOpen && (
        <div className="sess-menu" onClick={(e) => e.stopPropagation()}>
          <button type="button" onClick={onRename}>
            重命名
          </button>
          {!isDefault && (
            <button type="button" onClick={onArchive}>
              {row.archived ? "取消归档" : "归档"}
            </button>
          )}
          {!isDefault && (
            <button type="button" className="danger" onClick={onDelete}>
              删除
            </button>
          )}
          {isDefault && <p className="sess-menu-note">默认会话是历史锚点，不能归档或删除。</p>}
        </div>
      )}
    </div>
  );
}
