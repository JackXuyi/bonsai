"use client";

// MCP server 管理卡（spec-s6 §1）：列表 + 添加/编辑弹窗 + 启停 + 测试连接。
// http 传输强制显示隐私提示（plan v0.4 D12）。
import { useCallback, useEffect, useState } from "react";
import { ToolAccessBadge } from "@/components/agent/ToolAccessBadge";
import {
  humanizeAgentError,
  mcpDiscoverImport,
  mcpDiscoverScan,
  mcpServerList,
  mcpServerRemove,
  mcpServerSave,
  mcpServerTest,
  mcpServerToggle,
  mcpServerTools,
} from "@/lib/commands";
import type { DiscoveredServer, McpServerRow, ToolDescriptor } from "@/lib/types";

/** null = 关闭；"new" = 添加；id = 编辑 */
type FormState = null | "new" | string;

export function McpSection() {
  const [rows, setRows] = useState<McpServerRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState<FormState>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmId, setConfirmId] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  /** 展开时懒加载的工具清单缓存（id → descriptors），一次拉取终身复用。 */
  const [tools, setTools] = useState<Record<string, ToolDescriptor[]>>({});
  const [toolsLoading, setToolsLoading] = useState(false);
  // 本机扫描（spec-s2 §6）
  const [scanOpen, setScanOpen] = useState(false);
  const [scanRows, setScanRows] = useState<DiscoveredServer[]>([]);
  const [scanning, setScanning] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [importing, setImporting] = useState(false);
  const [scanMsg, setScanMsg] = useState<string | null>(null);

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
  }, [refresh]);

  // 展开 server 行时懒拉工具清单（测试连接成功也会展开 → 这里统一兜住）。
  useEffect(() => {
    if (!expanded || tools[expanded] || toolsLoading) return;
    setToolsLoading(true);
    mcpServerTools(expanded)
      .then((list) => {
        const sorted = [...list].sort((a, b) => a.name.localeCompare(b.name));
        setTools((prev) => ({ ...prev, [expanded]: sorted }));
      })
      .catch(() => setTools((prev) => ({ ...prev, [expanded]: [] })))
      .finally(() => setToolsLoading(false));
  }, [expanded, tools, toolsLoading]);

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

  function toggleSel(sig: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(sig)) next.delete(sig);
      else next.add(sig);
      return next;
    });
  }

  async function runScan() {
    setScanning(true);
    setScanMsg(null);
    try {
      const found = await mcpDiscoverScan();
      setScanRows(found);
      setSelected(new Set(found.filter((r) => !r.imported).map((r) => r.signature)));
      setScanOpen(true);
      setError(null);
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setScanning(false);
    }
  }

  async function importSelected() {
    setImporting(true);
    try {
      const report = await mcpDiscoverImport([...selected]);
      setScanMsg(`已导入 ${report.inserted} 条，跳过 ${report.skipped} 条。导入后默认停用，逐个「测试/启用」生效。`);
      await refresh();
      // 重扫一次刷新 imported 标记
      const found = await mcpDiscoverScan();
      setScanRows(found);
      setSelected(new Set(found.filter((r) => !r.imported).map((r) => r.signature)));
      setError(null);
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setImporting(false);
    }
  }

  return (
    <>
    <div className="card agent-card">
      <p className="card-title">MCP 工具</p>
      {error && <p className="model-error">{error}</p>}
      {rows.length === 0 ? (
        <p className="hint-line">还没有接 MCP server。接上后盆栽可以在对话里调用它的工具。</p>
      ) : (
        <div className="agent-list">
          {rows.map((r) => (
            <div key={r.id} className="agent-row" data-testid="mcp-row">
              <div
                className="agent-row-main"
                style={{ cursor: r.toolCount > 0 ? "pointer" : "default" }}
                onClick={() => {
                  if (r.toolCount > 0) setExpanded(expanded === r.id ? null : r.id);
                }}
              >
                <span className="agent-transport" data-transport={r.transport}>
                  {r.transport}
                </span>
                <span className="agent-name">{r.name}</span>
                <span className="hint-line">
                  {r.toolCount > 0 ? `${r.toolCount} 个工具` : "未取到工具"}
                </span>
                {r.toolCount > 0 && (
                  <span className="agent-chevron" aria-hidden="true">
                    {expanded === r.id ? "▾" : "▸"}
                  </span>
                )}
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
                <div className="agent-tool-grid" data-testid={`tools-of-${r.id}`}>
                  {toolsLoading && !tools[r.id] && <p className="hint-line">拉取工具清单…</p>}
                  {tools[r.id]?.map((t) => (
                    <div key={t.name} className="agent-tool-line">
                      <ToolAccessBadge access={t.access} compact />
                      <span className="agent-tool-name">{t.name}</span>
                      <span className="agent-tool-summary">{t.summary || "—"}</span>
                    </div>
                  ))}
                  {tools[r.id] && tools[r.id].length === 0 && (
                    <p className="hint-line">工具缓存为空——点「测试」重新连接获取。</p>
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
      <div style={{ display: "flex", gap: 8 }}>
        <button type="button" className="btn cloud-add" style={{ flex: 1 }} onClick={() => setForm("new")}>
          ＋ 接入 MCP server
        </button>
        <button type="button" className="btn-ghost" disabled={scanning} onClick={() => void runScan()}>
          {scanning ? "扫描中…" : "扫描本机"}
        </button>
      </div>
      <p className="hint-line" style={{ marginTop: 8 }}>
        支持 stdio（本机子进程）与 Streamable HTTP（远程）两种传输。新接入一律默认停用，手动启用后生效。
        权限分级与调用确认在「设置 · 权限与工具」。
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

      {scanOpen && (
        <div className="mconfirm" role="dialog" aria-modal="true">
          <div className="mconfirm-box agent-form" data-testid="scan-modal">
            <p className="mconfirm-title">扫描本机 MCP 配置</p>
            <p className="mconfirm-sub">
              来源：Cursor / Claude / VS Code / Windsurf 等客户端配置文件，以及应用自带的 mcp manifest。
              导入后默认停用，手动启用才生效。
            </p>
            {scanRows.length === 0 ? (
              <p className="hint-line">没扫到任何 MCP 配置。</p>
            ) : (
              <div className="agent-list">
                {scanRows.map((r) => (
                  <div key={r.signature} className="agent-row" data-testid="scan-row">
                    <div className="agent-row-main">
                      <input
                        type="checkbox"
                        aria-label={`选择 ${r.name}`}
                        disabled={r.imported}
                        checked={selected.has(r.signature)}
                        onChange={() => toggleSel(r.signature)}
                      />
                      <span className="agent-transport" data-transport={r.transport}>
                        {r.transport}
                      </span>
                      <span className="agent-name">{r.name}</span>
                      <span className="hint-line">
                        {r.source}
                        {r.imported ? " · 已导入" : ""}
                      </span>
                    </div>
                    <div className="agent-row-actions">
                      <span className="hint-line">{r.transport === "stdio" ? r.command : r.url}</span>
                    </div>
                  </div>
                ))}
              </div>
            )}
            {scanMsg && <p className="hint-line">{scanMsg}</p>}
            <div className="mconfirm-actions">
              <button type="button" className="btn-ghost" onClick={() => setScanOpen(false)}>
                关闭
              </button>
              <button
                type="button"
                className="btn"
                disabled={importing || selected.size === 0}
                onClick={() => void importSelected()}
              >
                {importing ? "导入中…" : `导入选中（${selected.size}）`}
              </button>
            </div>
          </div>
        </div>
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
    </>
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
