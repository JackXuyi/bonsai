"use client";

// MCP server 管理卡（spec-s6 §1）：列表 + 添加/编辑弹窗 + 启停 + 测试连接。
// http 传输强制显示隐私提示（plan v0.4 D12）。
import { useCallback, useEffect, useState } from "react";
import {
  agentApprovalGet,
  agentApprovalSet,
  humanizeAgentError,
  mcpServerList,
  mcpServerRemove,
  mcpServerSave,
  mcpServerTest,
  mcpServerToggle,
} from "@/lib/commands";
import type { McpServerRow } from "@/lib/types";

/** null = 关闭；"new" = 添加；id = 编辑 */
type FormState = null | "new" | string;

export function McpSection() {
  const [rows, setRows] = useState<McpServerRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState<FormState>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmId, setConfirmId] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [approval, setApproval] = useState<"auto" | "ask">("auto");

  const refresh = useCallback(async () => {
    try {
      setRows(await mcpServerList());
      setError(null);
    } catch (e) {
      setError(humanizeAgentError(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    void agentApprovalGet()
      .then(setApproval)
      .catch(() => setApproval("auto"));
  }, [refresh]);

  async function switchApproval(next: "auto" | "ask") {
    setApproval(next);
    try {
      await agentApprovalSet(next);
      setError(null);
    } catch (e) {
      setApproval(next === "ask" ? "auto" : "ask"); // 回滚到实际值
      setError(humanizeAgentError(e));
    }
  }

  async function toggle(id: string, enable: boolean) {
    setBusyId(id);
    try {
      await mcpServerToggle(id, enable);
      setError(null);
      await refresh();
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setBusyId(null);
    }
  }

  async function test(id: string) {
    setBusyId(id);
    try {
      const count = await mcpServerTest(id);
      setError(null);
      await refresh();
      setExpanded(id);
      return count;
    } catch (e) {
      setError(humanizeAgentError(e));
      return 0;
    } finally {
      setBusyId(null);
    }
  }

  const editing = typeof form === "string" && form !== "new" ? rows.find((r) => r.id === form) ?? null : null;

  return (
    <div className="card agent-card">
      <p className="card-title">MCP 工具</p>
      {error && <p className="model-error">{error}</p>}
      <div className="agent-approval" data-testid="approval-mode-row">
        <span className="hint-line">调用确认</span>
        <button
          type="button"
          className={approval === "auto" ? "btn" : "btn-ghost"}
          onClick={() => void switchApproval("auto")}
        >
          自动放行
        </button>
        <button
          type="button"
          className={approval === "ask" ? "btn" : "btn-ghost"}
          onClick={() => void switchApproval("ask")}
        >
          每次询问
        </button>
      </div>
      {rows.length === 0 ? (
        <p className="hint-line">还没有接 MCP server。接上后盆栽可以在对话里调用它的工具。</p>
      ) : (
        <div className="agent-list">
          {rows.map((r) => (
            <div key={r.id} className="agent-row" data-testid="mcp-row">
              <div className="agent-row-main">
                <span className="agent-transport" data-transport={r.transport}>
                  {r.transport}
                </span>
                <span className="agent-name">{r.name}</span>
                <span className="hint-line">
                  {r.toolCount > 0 ? `${r.toolCount} 个工具` : "未取到工具"}
                </span>
              </div>
              <div className="agent-row-actions">
                <button
                  type="button"
                  className="btn-ghost"
                  disabled={busyId === r.id}
                  onClick={() => void test(r.id)}
                >
                  {busyId === r.id ? "…" : "测试"}
                </button>
                <button
                  type="button"
                  className={r.enabled ? "btn-danger" : "btn"}
                  disabled={busyId === r.id}
                  onClick={() => void toggle(r.id, !r.enabled)}
                >
                  {r.enabled ? "停用" : "启用"}
                </button>
                <button type="button" className="btn-ghost" onClick={() => setForm(r.id)}>
                  编辑
                </button>
                <button type="button" className="btn-ghost" onClick={() => setConfirmId(r.id)}>
                  删
                </button>
              </div>
              {expanded === r.id && r.toolCount > 0 && (
                <p className="hint-line agent-tools-hint">工具清单见下方详情（测试连接后自动刷新）</p>
              )}
            </div>
          ))}
        </div>
      )}
      <button type="button" className="btn cloud-add" onClick={() => setForm("new")}>
        ＋ 接入 MCP server
      </button>
      <p className="hint-line" style={{ marginTop: 8 }}>
        支持 stdio（本机子进程）与 Streamable HTTP（远程）两种传输。新接入一律默认停用，手动启用后生效；
        {approval === "ask"
          ? "当前每次调用都会先弹确认卡，30 秒不回应视为拒绝。"
          : "当前调用直接执行（可在上方切到「每次询问」）。"}
      </p>

      {form !== null && (
        <McpFormModal
          row={editing}
          onClose={() => setForm(null)}
          onSaved={async () => {
            setForm(null);
            await refresh();
          }}
        />
      )}

      {confirmId && (
        <div className="mconfirm" role="dialog" aria-modal="true">
          <div className="mconfirm-box">
            <p className="mconfirm-title">删除这条 MCP 配置？</p>
            <p className="mconfirm-sub">只删配置与连接，不删本机文件。</p>
            <div className="mconfirm-actions">
              <button type="button" className="btn-ghost" onClick={() => setConfirmId(null)}>
                算了
              </button>
              <button
                type="button"
                className="btn-danger"
                onClick={async () => {
                  const id = confirmId;
                  setConfirmId(null);
                  try {
                    await mcpServerRemove(id);
                    await refresh();
                  } catch (e) {
                    setError(humanizeAgentError(e));
                  }
                }}
              >
                删除
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- form

function McpFormModal({
  row,
  onClose,
  onSaved,
}: {
  row: McpServerRow | null;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [name, setName] = useState(row?.name ?? "");
  const [transport, setTransport] = useState<"stdio" | "http">(row?.transport ?? "stdio");
  const [command, setCommand] = useState(row?.command ?? "");
  const [args, setArgs] = useState(row?.args.join(" ") ?? "");
  const [url, setUrl] = useState(row?.url ?? "");
  const [auth, setAuth] = useState(
    row?.headers.find(([k]) => k.toLowerCase() === "authorization")?.[1] ?? "",
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function save() {
    setSaving(true);
    setError(null);
    try {
      const { mcpServerSave } = await import("@/lib/commands");
      await mcpServerSave({
        id: row?.id,
        name: name.trim(),
        transport,
        command: transport === "stdio" ? command.trim() : null,
        args: transport === "stdio" ? args.trim().split(/\s+/).filter(Boolean) : [],
        url: transport === "http" ? url.trim() : null,
        headers: transport === "http" && auth.trim() ? [["Authorization", auth.trim()]] : [],
        env: [],
      });
      await onSaved();
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="mconfirm" role="dialog" aria-modal="true">
      <div className="mconfirm-box agent-form">
        <p className="mconfirm-title">{row ? "编辑 MCP server" : "接入 MCP server"}</p>

        <label className="agent-field">
          <span>名称</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="例：文件检索" />
        </label>

        <div className="agent-field">
          <span>传输</span>
          <div className="agent-transport-pick">
            <button
              type="button"
              className={transport === "stdio" ? "btn" : "btn-ghost"}
              onClick={() => setTransport("stdio")}
            >
              stdio（本机）
            </button>
            <button
              type="button"
              className={transport === "http" ? "btn" : "btn-ghost"}
              onClick={() => setTransport("http")}
            >
              http（远程）
            </button>
          </div>
        </div>

        {transport === "stdio" ? (
          <>
            <label className="agent-field">
              <span>可执行文件路径</span>
              <input
                value={command}
                onChange={(e) => setCommand(e.target.value)}
                placeholder="例：/usr/local/bin/mcp-server-fs"
              />
            </label>
            <label className="agent-field">
              <span>启动参数（空格分隔）</span>
              <input value={args} onChange={(e) => setArgs(e.target.value)} placeholder="--root ~/docs" />
            </label>
          </>
        ) : (
          <>
            <label className="agent-field">
              <span>端点 URL</span>
              <input
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://mcp.example.com/v1"
              />
            </label>
            <label className="agent-field">
              <span>Authorization（可留空）</span>
              <input
                value={auth}
                onChange={(e) => setAuth(e.target.value)}
                placeholder="Bearer sk-…"
              />
            </label>
          </>
        )}

        {transport === "http" && (
          <p className="agent-privacy" data-testid="privacy-hint">
            ⚠ 远程工具由外部服务处理：对话参数会发往 {url || "该地址"}。记忆与人格上下文不会出设备。
          </p>
        )}

        {error && <p className="model-error">{error}</p>}

        <div className="mconfirm-actions">
          <button type="button" className="btn-ghost" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn" disabled={saving || !name.trim()} onClick={() => void save()}>
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
