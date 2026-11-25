use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::agent_tools::{
    mask_secrets, ApprovalDecision, ApprovalGate, ToolCall, ToolDescriptor, ToolKind, ToolOutput,
    ToolProvider,
};
use crate::memory::db;

use super::servers::{
    connect_and_cache, delete_server, insert_server, list_raw, row_from, update_server, view,
    McpServerRow, RawRow,
};
use super::*;

fn mem() -> Connection {
    db::open(&{
        let dir = std::env::temp_dir().join("bonsai-mcp-admin-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("t-{}-{}.sqlite3", std::process::id(), super::servers::SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)))
    })
    .unwrap()
}

/// Store-backed twin for tests that go through `execute_audited` (it
/// locks the store internally — 2026-11-20 lock-narrowing).
fn mem_store() -> crate::memory::MemoryStore {
    let dir = std::env::temp_dir().join("bonsai-mcp-admin-test");
    std::fs::create_dir_all(&dir).unwrap();
    crate::memory::MemoryStore::open(&dir.join(format!(
        "s-{}-{}.sqlite3",
        std::process::id(),
        super::servers::SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )))
    .unwrap()
}

fn with_conn<T>(s: &crate::memory::MemoryStore, f: impl FnOnce(&Connection) -> T) -> T {
    let conn = s.write_guard().unwrap();
    f(&conn)
}

fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

// cross-module helpers (mcp_discover tests share the temp-db pattern)
pub(crate) fn mem_conn() -> Connection {
    mem()
}

pub(crate) fn next_seq() -> u32 {
    super::servers::SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

pub(crate) fn view_for_test(row: &RawRow, conn: &Connection) -> McpServerRow {
    view(row, conn)
}

#[test]
fn save_list_masks_secrets() {
    let conn = mem();
    let id = insert_server(
        &conn,
        "远程天气",
        "http",
        None,
        &[],
        Some("https://mcp.example.com/v1"),
        &hdrs(&[("Authorization", "Bearer supersecret99")]),
        &[],
    )
    .unwrap();
    let rows = list_raw(&conn).unwrap();
    assert_eq!(rows.len(), 1);
    let v = view(&rows[0], &conn);
    assert!(!v.enabled, "new server starts disabled (RK-04)");
    let rendered = serde_json::to_string(&v).unwrap();
    assert!(!rendered.contains("supersecret99"), "plaintext must not leak");
    assert!(rendered.contains("••••"));
    assert_eq!(v.id, id);
}

#[test]
fn save_validation_rejects_bad_transport_fields() {
    let conn = mem();
    assert_eq!(
        insert_server(&conn, "x", "stdio", None, &[], None, &[], &[]).unwrap_err(),
        "bad_command"
    );
    assert_eq!(
        insert_server(&conn, "x", "http", None, &[], Some("ftp://x"), &[], &[]).unwrap_err(),
        "bad_url"
    );
    assert_eq!(
        insert_server(&conn, "x", "http", None, &[], None, &[], &[]).unwrap_err(),
        "bad_url"
    );
}

#[test]
fn remove_deletes_row_and_reports_missing() {
    let conn = mem();
    let id = insert_server(&conn, "n", "http", None, &[], Some("https://x.com"), &[], &[]).unwrap();
    delete_server(&conn, &id).unwrap();
    assert!(list_raw(&conn).unwrap().is_empty());
    assert_eq!(delete_server(&conn, &id).unwrap_err(), "server_not_found");
}

#[test]
fn update_missing_row_errors_loudly() {
    let conn = mem();
    assert_eq!(
        update_server(&conn, "nope", "n", "http", None, &[], Some("https://x.com"), &[], &[]).unwrap_err(),
        "server_not_found"
    );
}

#[test]
fn toggle_flow_with_mock_client() {
    // No live client in CI: connect fails → enabled must stay 0.
    let conn = mem();
    let id = insert_server(
        &conn,
        "本地",
        "stdio",
        Some("/nonexistent-bonsai-mcp-bin"),
        &[],
        None,
        &[],
        &[],
    )
    .unwrap();
    let row = row_from(&conn, &id).unwrap();
    assert!(connect_and_cache(&conn, &row).is_err());
    let enabled: i64 = conn
        .query_row("SELECT enabled FROM mcp_servers WHERE id=?1", [&id], |r| r.get(0))
        .unwrap();
    assert_eq!(enabled, 0, "failed connect must not flip enabled");
}

#[test]
fn audit_rows_round_trip_masked() {
    let conn = mem();
    // The caller (execute_audited) masks before logging — same here.
    let summary = mask_secrets("args {\"auth\": \"Bearer tokensecret1\"}");
    db::log_agent_tool_call(
        &conn,
        "mcp",
        "weather",
        "ok",
        "auto_allowed",
        Some(&summary),
        Some("trace-1"),
        true,
        Some(42),
        None,
    )
    .unwrap();
    let records = db::tool_call_records(&conn, 10).unwrap();
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert!(r.remote);
    assert_eq!(r.result_status.as_deref(), Some("auto_allowed"));
    assert!(r.params_summary.as_deref().unwrap_or("").contains("••••"));
    assert!(!r.params_summary.as_deref().unwrap_or("").contains("tokensecret1"));
}

#[test]
fn auto_gate_allows_ask_gate_denies() {
    use crate::agent_tools::AutoAllowGate;
    let call = ToolCall { name: "t".into(), args: serde_json::json!({}) };
    assert_eq!(AutoAllowGate.check(&call, "s", false), ApprovalDecision::Allow);
    // No app handle → no one can answer → Deny (never a silent allow).
    let headless = AskFirstGate::new(Arc::new(ApprovalBroker::default()), None, Arc::new(SessionToolAllow::default()));
    assert_eq!(headless.check(&call, "s", false), ApprovalDecision::Deny);
}

#[test]
fn approval_broker_handshake() {
    let broker = Arc::new(ApprovalBroker::default());
    assert!(!broker.reply(Some(1), true), "nothing pending → stale reply");

    let (id, slot) = broker.register("t");
    assert_eq!(broker.peek_tool(Some(id)).as_deref(), Some("t"));
    assert!(!broker.reply(Some(id + 99), true), "id mismatch is ignored");
    assert!(
        broker.has_pending(),
        "mismatched reply must not consume the slot"
    );
    assert!(broker.reply(Some(id), true));
    let (lock, _) = &*slot;
    assert_eq!(*lock.lock().unwrap(), Some(true));
    assert!(!broker.reply(Some(id), true), "second reply is a no-op");
}

#[test]
fn session_allow_skips_ask_and_clears_on_session_switch() {
    let allow = Arc::new(SessionToolAllow::default());
    allow.bind_session("s1");
    allow.grant("fs_write");
    let gate = AskFirstGate::new(Arc::new(ApprovalBroker::default()), None, allow.clone());
    let call = ToolCall {
        name: "fs_write".into(),
        args: serde_json::json!({"path": "/tmp/a.md", "content": "x"}),
    };
    // headless 本应拒；白名单命中则放行
    assert_eq!(gate.check(&call, "fs_write", false), ApprovalDecision::Allow);
    let other = ToolCall {
        name: "fs_copy".into(),
        args: serde_json::json!({"from": "/a", "to": "/b"}),
    };
    assert_eq!(gate.check(&other, "fs_copy", false), ApprovalDecision::Deny);

    allow.bind_session("s2");
    assert!(!allow.allows("fs_write"), "换会话清空白名单");
    assert_eq!(gate.check(&call, "fs_write", false), ApprovalDecision::Deny);
}

#[test]
fn execute_audited_unknown_tool_errors() {
    let s = mem_store();
    let kit = AgentKit::empty();
    let err = execute_audited(&s, &kit, "ghost", serde_json::json!({})).unwrap_err();
    assert_eq!(err, "unknown_tool");
}

#[test]
fn execute_audited_records_ok_and_failure_status() {
    let s = mem_store();

    // ok path: auto_allowed + masked params.
    let mut ok = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
    ok.replies.insert("weather".into(), "杭州多云 18 度".into());
    let kit = AgentKit {
        providers: vec![Box::new(ok)],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None, std::sync::Arc::new(SessionToolAllow::default()))),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    };
    kit.trace("round1:candidates=1".into());
    let out = execute_audited(
        &s,
        &kit,
        "weather",
        serde_json::json!({"auth": "Bearer sk-abcdefgh123"}),
    )
    .unwrap();
    assert_eq!(out.summary, "杭州多云 18 度");

    let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
    assert_eq!(rec.tool, "mcp");
    assert_eq!(rec.action, "weather");
    assert_eq!(rec.outcome, "ok");
    assert_eq!(rec.result_status.as_deref(), Some("auto_allowed"));
    assert!(rec.duration_ms.is_some());
    assert_eq!(rec.prompt_trace.as_deref(), Some("round1:candidates=1"));
    let params = rec.params_summary.as_deref().unwrap_or("");
    assert!(params.contains("••••"), "params masked: {params}");
    assert!(!params.contains("abcdefgh123"));

    // failure path: execute Err → outcome=error + specific result_status.
    let mut broken = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
    broken.fail_for.push("weather".into());
    let kit_broken = AgentKit {
        providers: vec![Box::new(broken)],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None, std::sync::Arc::new(SessionToolAllow::default()))),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    };
    let err = execute_audited(&s, &kit_broken, "weather", serde_json::json!({})).unwrap_err();
    assert_eq!(err, crate::mcp::ERR_TIMEOUT);

    let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
    assert_eq!(rec.outcome, "error");
    assert_eq!(rec.result_status.as_deref(), Some("mcp_timeout"));
}

#[test]
fn execute_audited_validates_args_before_gate() {
    // plan §3.3: strict inputSchema check BEFORE the approval gate —
    // a small model's hallucinated args never reach execution, and the
    // audit row records the exact reason.
    let s = mem_store();
    let schema = r#"{"type":"object","properties":{"city":{"type":"string"},"days":{"type":"integer"}},"required":["city"]}"#;
    let mut provider = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", schema);
    provider.replies.insert("weather".into(), "杭州多云 18 度".into());
    let kit = AgentKit {
        providers: vec![Box::new(provider)],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None, std::sync::Arc::new(SessionToolAllow::default()))),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    };

    // missing required → rejected, provider never called.
    let err =
        execute_audited(&s, &kit, "weather", serde_json::json!({"days": 3})).unwrap_err();
    assert_eq!(err, "bad_args:missing:city");

    // wrong type → rejected.
    let err = execute_audited(&s, &kit, "weather", serde_json::json!({"city": "杭州", "days": "三天"}))
        .unwrap_err();
    assert_eq!(err, "bad_args:type:days");

    // not an object → rejected.
    let err = execute_audited(&s, &kit, "weather", serde_json::json!("杭州")).unwrap_err();
    assert_eq!(err, "bad_args:not-an-object");

    // valid args (incl. unknown extra field) → passes, executes.
    let out = execute_audited(&s, &kit, "weather", serde_json::json!({"city": "杭州", "unit": "c"})).unwrap();
    assert_eq!(out.summary, "杭州多云 18 度");

    // audit: only the rejected rows carry bad_args; the good one auto_allowed.
    let recs = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
    let bad = recs.iter().filter(|r| r.result_status.as_deref().unwrap_or("").starts_with("bad_args"));
    assert_eq!(bad.count(), 3);
    let good = &recs[0];
    assert_eq!(good.result_status.as_deref(), Some("auto_allowed"));
    assert_eq!(good.outcome, "ok");
}

/// Startup reconnect (2026-11-20 getHardwareInfo regression): disabled
/// rows must be skipped, an enabled-but-dead server must fail CLEANLY
/// (logged, pool stays empty, count reflects reality).
#[test]
fn reconnect_enabled_servers_skips_dead_and_disabled() {
    let dir = std::env::temp_dir().join("bonsai-mcp-reconnect-test");
    std::fs::create_dir_all(&dir).unwrap();
    let store = crate::memory::MemoryStore::open(
        &dir.join(format!("r-{}-{}.sqlite3", std::process::id(), super::servers::SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst))),
    )
    .unwrap();
    let agent = AgentState {
        manager: crate::mcp::McpManager::default(),
        skills_dir: std::env::temp_dir(),
        approvals: Arc::new(ApprovalBroker::default()),
        session_allow: Arc::new(SessionToolAllow::default()),
        app: None,
    };
    let conn = store.write_guard().unwrap();
    let id = insert_server(&conn, "死掉的", "stdio", Some("/bin/true"), &[], None, &[], &[]).unwrap();
    drop(conn);

    // Disabled row: skipped without any connection attempt.
    assert_eq!(reconnect_enabled_servers(&agent, &store), 0);
    assert!(agent.manager.get(&id).is_none(), "disabled row must not enter the pool");

    // Enabled but not a real MCP server (/bin/true exits immediately →
    // initialize fails): clean skip, pool stays empty.
    {
        let conn = store.write_guard().unwrap();
        conn.execute("UPDATE mcp_servers SET enabled=1 WHERE id=?1", [&id]).unwrap();
    }
    assert_eq!(reconnect_enabled_servers(&agent, &store), 0);
    assert!(agent.manager.get(&id).is_none(), "dead server must not enter the pool");
}

#[test]
fn ask_mode_gate_denies_when_no_ui_is_attached() {
    // Full flip check: setting=ask → build_agent_kit picks the asking
    // gate → without an app handle it must deny (never silently allow).
    let s = mem_store();
    with_conn(&s, |conn| {
        db::set_setting(conn, "approval_mode", "ask").unwrap();
        let id = insert_server(conn, "本地", "stdio", Some("/bin/true"), &[], None, &[], &[]).unwrap();
        let cache = serde_json::to_string(&vec![ToolDescriptor {
            name: "weather".into(),
            summary: "查天气".into(),
            detail: "{}".into(),
            kind: crate::agent_tools::ToolKind::Mcp,
            remote: false,
                    access: crate::agent_tools::ToolAccess::Read,
        }])
        .unwrap();
        conn.execute(
            "UPDATE mcp_servers SET enabled = 1, tool_cache = ?1 WHERE id = ?2",
            rusqlite::params![cache, id],
        )
        .unwrap();
    });
    let state = AgentState {
        manager: crate::mcp::McpManager::default(),
        skills_dir: std::env::temp_dir(),
        approvals: Arc::new(ApprovalBroker::default()),
        session_allow: Arc::new(SessionToolAllow::default()),
        app: None,
    };
    // Separate scopes: execute_audited locks internally, so no guard may
    // be alive here.
    let kit = with_conn(&s, |conn| build_agent_kit(conn, &state, "今天有什么会", crate::calendar::CalendarAccess::Denied, true));
    assert!(kit.has_capabilities());
    let err = execute_audited(&s, &kit, "weather", serde_json::json!({})).unwrap_err();
    assert_eq!(err, "denied");
    let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
    assert_eq!(rec.outcome, "denied");
    assert_eq!(rec.result_status.as_deref(), Some("denied_by_gate"));
}

#[test]
fn catalog_is_sorted_and_capped() {
    // Catalog order must be stable regardless of provider/enable order:
    // the model picks names from this list (spec-s1 §3 item 2).
    let kit = AgentKit {
        providers: vec![
            Box::new(crate::mcp::MockMcpProvider::with_tool("zebra", "z", "{}")),
            Box::new(crate::mcp::MockMcpProvider::with_tool("alpha", "a", "{}")),
        ],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None, std::sync::Arc::new(SessionToolAllow::default()))),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    };
    let names: Vec<String> = kit.catalog().into_iter().map(|d| d.name).collect();
    assert_eq!(names, vec!["alpha".to_string(), "zebra".to_string()]);
    assert!(kit.has_capabilities());
    assert!(!AgentKit::empty().has_capabilities());
}

// ---- 权限三档（plan-task-card v0.2 §6，roadmap G-10）

/// Always-allow gate, for the ask-branch allow-side assertion.
struct AllowAllGate;
impl ApprovalGate for AllowAllGate {
    fn check(&self, _c: &ToolCall, _s: &str, _r: bool) -> ApprovalDecision {
        ApprovalDecision::Allow
    }
}

fn kit_with_access(name: &str, access: crate::agent_tools::ToolAccess) -> AgentKit {
    let mut p = crate::mcp::MockMcpProvider::with_tool(name, "测试工具", "{}");
    p.replies.insert(name.to_string(), "执行结果".into());
    let mut d = p.tools[0].clone();
    d.access = access;
    p.tools[0] = d;
    AgentKit {
        providers: vec![Box::new(p)],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None, std::sync::Arc::new(SessionToolAllow::default()))),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    }
}

#[test]
fn forbidden_tool_never_executes_and_audits_rejection() {
    let s = crate::memory::MemoryStore::open(&std::env::temp_dir().join("mcp-admin-t3a.sqlite3")).unwrap();
    let kit = kit_with_access("rm_rf", crate::agent_tools::ToolAccess::Forbidden);
    let err = execute_audited(&s, &kit, "rm_rf", serde_json::json!({})).unwrap_err();
    assert_eq!(err, "forbidden");
    // 沉默不得变成执行：审计记录拒绝，状态 forbidden_tool。
    let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
    assert_eq!(rec.outcome, "denied");
    assert_eq!(rec.result_status.as_deref(), Some("forbidden_tool"));
}

#[test]
fn ask_access_forces_the_ask_gate_even_in_auto_mode() {
    let s = crate::memory::MemoryStore::open(&std::env::temp_dir().join("mcp-admin-t3b.sqlite3")).unwrap();
    // kit.gate 是 AutoAllowGate（approval_mode=auto 的形状），但 ask 档
    // 工具必须走 ask_gate——headless 无 AppHandle ⇒ 立即拒绝。
    let kit = kit_with_access("send_mail", crate::agent_tools::ToolAccess::Ask);
    let err = execute_audited(&s, &kit, "send_mail", serde_json::json!({})).unwrap_err();
    assert_eq!(err, "denied");
    let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
    assert_eq!(rec.result_status.as_deref(), Some("denied_by_gate"));

    // ask 门放行（用户点了允许）→ 执行 + asked_allowed 审计。
    let mut p = crate::mcp::MockMcpProvider::with_tool("send_mail", "测试工具", "{}");
    p.replies.insert("send_mail".into(), "已执行".into());
    let mut d = p.tools[0].clone();
    d.access = crate::agent_tools::ToolAccess::Ask;
    p.tools[0] = d;
    let kit = AgentKit {
        providers: vec![Box::new(p)],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        ask_gate: Box::new(AllowAllGate),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    };
    let out = execute_audited(&s, &kit, "send_mail", serde_json::json!({})).unwrap();
    assert_eq!(out.summary, "已执行");
    let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())
        .into_iter()
        .find(|r| r.result_status.as_deref() == Some("asked_allowed"))
        .expect("asked_allowed audit row");
    assert_eq!(rec.outcome, "ok");
}

#[test]
fn access_defaults_to_read_for_legacy_caches() {
    // 旧 tool_cache 反序列化（无 access 字段）→ Read，行为零回归。
    let old: crate::agent_tools::ToolDescriptor = serde_json::from_str(
        r#"{"name":"weather","summary":"查天气","detail":"{}","kind":"mcp","remote":false}"#,
    )
    .unwrap();
    assert_eq!(old.access, crate::agent_tools::ToolAccess::Read);
    assert_eq!(old.access.as_str(), "read");
}

// ---- 轮内工具缓存（plan-tool-cache v2）

#[test]
fn execute_audited_memoizes_read_tools_within_one_kit() {
    let s = mem_store();
    let mut p = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
    p.replies.insert("weather".into(), "杭州多云 18 度".into());
    let kit = AgentKit {
        providers: vec![Box::new(p)],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None, std::sync::Arc::new(SessionToolAllow::default()))),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    };
    let args = serde_json::json!({"city": "杭州", "unit": "c"});
    let out1 = execute_audited(&s, &kit, "weather", args.clone()).unwrap();
    // 键序不同的同参数对象 → 同一缓存键 → 命中，provider 不再执行。
    let out2 = execute_audited(&s, &kit, "weather", serde_json::json!({"unit": "c", "city": "杭州"})).unwrap();
    assert_eq!(out1.summary, out2.summary);

    // 审计诚实：一次真执行 + 一次 cache_hit（新行在前）。
    let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
    assert_eq!(records[0].result_status.as_deref(), Some("cache_hit"));
    assert_eq!(records[0].outcome, "ok");
    assert_eq!(records[1].result_status.as_deref(), Some("auto_allowed"));
}

#[test]
fn write_tool_success_clears_the_turn_cache() {
    let s = mem_store();
    let mut read_p = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
    read_p.replies.insert("weather".into(), "执行结果".into());
    let mut write_p = crate::mcp::MockMcpProvider::with_tool("send_mail", "发邮件", "{}");
    write_p.replies.insert("send_mail".into(), "已执行".into());
    let mut d = write_p.tools[0].clone();
    d.access = crate::agent_tools::ToolAccess::Ask;
    write_p.tools[0] = d;
    let kit = AgentKit {
        providers: vec![Box::new(read_p), Box::new(write_p)],
        gate: Box::new(crate::agent_tools::AutoAllowGate),
        // ask 门放行（模拟用户点了允许）→ 写成功 → 必须清缓存。
        ask_gate: Box::new(AllowAllGate),
        task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
        ..AgentKit::empty()
    };
    let args = serde_json::json!({"city": "杭州"});
    execute_audited(&s, &kit, "weather", args.clone()).unwrap();
    // 命中一次，证明缓存确实在工作。
    let hit = execute_audited(&s, &kit, "weather", args.clone()).unwrap();
    assert_eq!(hit.summary, "执行结果");
    // 写成功 → 清空 → 第三次 weather 重新真执行（非 cache_hit）。
    execute_audited(&s, &kit, "send_mail", serde_json::json!({})).unwrap();
    let out3 = execute_audited(&s, &kit, "weather", args.clone()).unwrap();
    assert_eq!(out3.summary, "执行结果");
    let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
    assert_eq!(records[0].result_status.as_deref(), Some("auto_allowed"));
    assert_eq!(records[0].action, "weather");
    assert_eq!(records[1].result_status.as_deref(), Some("asked_allowed"));
    assert_eq!(records[2].result_status.as_deref(), Some("cache_hit"));
}

#[test]
fn mail_provider_mounts_only_when_enabled() {
    let s = mem_store();
    let state = AgentState {
        manager: crate::mcp::McpManager::default(),
        skills_dir: std::env::temp_dir(),
        approvals: Arc::new(ApprovalBroker::default()),
        session_allow: Arc::new(SessionToolAllow::default()),
        app: None,
    };
    let off = with_conn(&s, |conn| build_agent_kit(conn, &state, "今天有什么邮件", crate::calendar::CalendarAccess::Denied, true));
    assert!(
        !off.catalog_all().iter().any(|d| d.name.starts_with("mail_")),
        "默认关时 kit 不得出现 mail_*"
    );
    with_conn(&s, |conn| {
        crate::mail::set_enabled(conn, true).unwrap();
    });
    let on = with_conn(&s, |conn| build_agent_kit(conn, &state, "今天有什么邮件", crate::calendar::CalendarAccess::Denied, true));
    let names: Vec<_> = on
        .catalog_all()
        .into_iter()
        .map(|d| d.name)
        .filter(|n| n.starts_with("mail_"))
        .collect();
    assert!(names.contains(&"mail_unread".to_string()));
    assert!(names.contains(&"mail_draft".to_string()));

    // 本地（cloud=false）即使开关开也不挂 mail / calendar。
    with_conn(&s, |conn| {
        db::set_setting(conn, crate::calendar::SETTING_ENABLED, "1").unwrap();
    });
    let local = with_conn(&s, |conn| {
        build_agent_kit(
            conn,
            &state,
            "今天有什么邮件",
            crate::calendar::CalendarAccess::Denied,
            false,
        )
    });
    assert!(
        !local
            .catalog_all()
            .iter()
            .any(|d| d.name.starts_with("mail_") || d.name.starts_with("calendar_")),
        "本地不得挂 mail/calendar"
    );
}
