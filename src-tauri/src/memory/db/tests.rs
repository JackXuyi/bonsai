//! memory/db integration tests (kept as one module after the split).

use rusqlite::{params, Connection};

use super::*;
use crate::memory::clock;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    fn all_events(conn: &Connection) -> Vec<EventRow> {
        unconsumed_events(conn, "1970-01-01T00:00:00Z", "2999-01-01T00:00:00Z").unwrap()
    }

    #[test]
    fn event_fact_search_invalidate_forget() {
        let conn = mem();
        let ev = add_event(&conn, "chat", "user said they love rainy days").unwrap();
        let fact = add_fact(&conn, "user loves rainy days", Some(ev)).unwrap();

        let hits = search(&conn, "rainy", 10, true).unwrap();
        assert_eq!(hits.len(), 2); // fact first (higher score), then event

        invalidate_fact(&conn, fact).unwrap();
        let hits = search(&conn, "rainy", 10, true).unwrap();
        // Invalidated fact sinks (0.1 penalty): the active event ranks first,
        // the superseded fact trails.
        assert_eq!(hits[0].kind, "event");
        assert!(hits[1].invalidated);

        forget_event(&conn, ev).unwrap();
        let hits = search(&conn, "rainy", 10, true).unwrap();
        assert!(hits.is_empty()); // truly deleted — nothing left to trace
    }

    #[test]
    fn bump_rises_and_caps() {
        let conn = mem();
        let a = add_fact(&conn, "user loves rainy days", None).unwrap();
        bump_fact(&conn, a, 0.1).unwrap();
        bump_fact(&conn, a, 10.0).unwrap();
        let facts = active_facts(&conn).unwrap();
        assert!((facts[0].confidence - 0.95).abs() < 1e-9);
    }

    #[test]
    fn v1_database_migrates_losslessly() {
        // Recreate exactly the scaffold-era v1 tables (no role/session/consumed,
        // no confidence/importance, no consolidate_runs), then migrate.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 kind TEXT NOT NULL, content TEXT NOT NULL);
             CREATE TABLE facts (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL, source_event_id INTEGER REFERENCES events(id),
                 invalid_at TEXT);
             CREATE TABLE preferences (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 category TEXT NOT NULL, content TEXT NOT NULL,
                 confidence REAL NOT NULL DEFAULT 0.5, invalid_at TEXT);
             CREATE TABLE boundaries (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES ('2026-11-01T00:00:00Z', 'chat', 'legacy row')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);

        // Legacy row intact with new columns at defaults; second run is a no-op.
        let rows = all_events(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "legacy row");
        migrate(&conn).unwrap();
    }

    #[test]
    fn v13_tasks_upgrade_to_v14_with_plan_columns() {
        // v13 老库：tasks 是 v12 的六列外壳，user_version 已停在 13。
        // migrate 必须补齐四列、保留老行，并写入默认值（v10 教训）。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 kind TEXT NOT NULL, content TEXT NOT NULL, role TEXT, session_id TEXT,
                 consumed INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE tasks (
                 id TEXT PRIMARY KEY, created_at TEXT NOT NULL, goal TEXT NOT NULL,
                 status TEXT NOT NULL, tool_calls INTEGER NOT NULL DEFAULT 0,
                 final_reply TEXT);
             INSERT INTO tasks (id, created_at, goal, status, tool_calls)
                 VALUES ('legacy-1', '2026-11-23T00:00:00Z', '老任务', 'done', 2);
             PRAGMA user_version = 13;",
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);

        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(tasks)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        for c in ["plan_json", "plan_mode", "cancel_reason", "confirm_mode"] {
            assert!(cols.iter().any(|x| x == c), "v14 缺列: {c}");
        }

        let rows = task_list(&conn, 10).unwrap();
        assert_eq!(rows.len(), 1, "老行必须保留");
        assert_eq!(rows[0].id, "legacy-1");
        assert_eq!(rows[0].goal, "老任务");
        assert!(rows[0].plan.is_none());
        assert_eq!(rows[0].plan_mode, "none");
        assert_eq!(rows[0].confirm_mode, "auto", "D1 默认确认");
        assert!(rows[0].cancel_reason.is_none());

        // 二次迁移幂等：不重复加列、不丢行。
        migrate(&conn).unwrap();
        assert_eq!(task_list(&conn, 10).unwrap().len(), 1);
    }

    #[test]
    fn plan_put_get_and_task_list_carry_plan() {
        let conn = mem();
        let id = task_start(&conn, "整理文件并写摘要").unwrap();
        let plan = PlanJson {
            version: 1,
            goal: "整理文件并写摘要".into(),
            steps: vec![
                PlanStep {
                    seq: 0,
                    intent: "读第一个文件".into(),
                    tool_hint: "fs_read".into(),
                    status: "done".into(),
                    attempts: 1,
                    tool_call_id: Some(12),
                },
                PlanStep {
                    seq: 1,
                    intent: "写入摘要".into(),
                    tool_hint: "fs_write".into(),
                    status: "pending".into(),
                    attempts: 0,
                    tool_call_id: None,
                },
            ],
            replans: 0,
        };

        plan_put(&conn, &id, &plan).unwrap();
        assert_eq!(plan_get(&conn, &id).unwrap(), Some(plan.clone()));
        task_set_confirm_mode(&conn, &id, "manual").unwrap();
        task_set_cancel_reason(&conn, &id, "budget").unwrap();
        task_finish(&conn, &id, "done", 2, None).unwrap();

        let rows = task_list(&conn, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].plan, Some(plan));
        assert_eq!(rows[0].plan_mode, "planned");
        assert_eq!(rows[0].confirm_mode, "manual");
        assert_eq!(rows[0].cancel_reason.as_deref(), Some("budget"));

        // 脏 JSON 降级：plan_get 返回 None、列表不炸（一条脏数据不打挂面板）。
        conn.execute(
            "UPDATE tasks SET plan_json = '{not json' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        assert!(plan_get(&conn, &id).unwrap().is_none());
        assert!(task_list(&conn, 10).unwrap()[0].plan.is_none());
    }

    #[test]
    fn migrate_heals_a_db_stamped_at_current_version_without_tables() {
        // 真机事故（2026-11-23）：user_version 已经 checkpoint 到当前版本（当初 13，
        // 现为 14），但建表帧随 WAL 截断丢失 → 旧实现「版本够就 return」永久卡死，
        // 前端报 "no such table: user_profile"。migrate 必须自己把表补回来。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 kind TEXT NOT NULL, content TEXT NOT NULL, role TEXT, session_id TEXT,
                 consumed INTEGER NOT NULL DEFAULT 0);
             PRAGMA user_version = 14;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES ('2026-11-23T00:00:00Z', 'chat', 'kept')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        for table in ["user_profile", "user_profile_history"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table} 必须被自愈补出来");
        }
        assert_eq!(all_events(&conn).len(), 1, "旧数据不动");
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn v7_database_upgrades_to_v8_idempotently() {
        // v7-era shape: tool_calls exists WITHOUT the agent audit columns and
        // mcp_servers does not exist yet (spec-s2 §1).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 kind TEXT NOT NULL, content TEXT NOT NULL, role TEXT, session_id TEXT,
                 consumed INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE facts (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL, source_event_id INTEGER,
                 confidence REAL NOT NULL DEFAULT 0.6,
                 importance INTEGER NOT NULL DEFAULT 3, invalid_at TEXT);
             CREATE TABLE preferences (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 category TEXT NOT NULL, content TEXT NOT NULL,
                 confidence REAL NOT NULL DEFAULT 0.5, invalid_at TEXT);
             CREATE TABLE boundaries (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL, source_fact_id INTEGER);
             CREATE TABLE tool_calls (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 tool TEXT NOT NULL, action TEXT NOT NULL, outcome TEXT NOT NULL,
                 detail TEXT, duration_ms INTEGER);
             PRAGMA user_version = 7;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES ('2026-11-10T00:00:00Z', 'chat', 'v7 row')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tool_calls (ts, tool, action, outcome)
             VALUES ('2026-11-10T00:00:00Z', 'calendar', 'agenda', 'ok')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);

        // New table appeared, legacy audit row survived with NULL defaults.
        let servers: i64 = conn
            .query_row("SELECT COUNT(*) FROM mcp_servers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(servers, 0);
        let records = tool_call_records(&conn, 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].action, "agenda");
        assert!(records[0].params_summary.is_none());
        assert!(!records[0].remote);
        assert_eq!(all_events(&conn).len(), 1);

        // Second run is a no-op — no duplicate columns, no data loss.
        migrate(&conn).unwrap();
        assert_eq!(all_events(&conn).len(), 1);
        assert_eq!(tool_call_records(&conn, 10).unwrap().len(), 1);
    }

    #[test]
    fn tool_call_records_newest_first_with_limit() {
        let conn = mem();
        for n in 1..=3 {
            log_agent_tool_call(
                &conn,
                "mcp",
                &format!("tool{n}"),
                "ok",
                "auto_allowed",
                None,
                None,
                false,
                Some(n),
                None,
            )
            .unwrap();
        }
        let all = tool_call_records(&conn, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].action, "tool3", "newest first");
        assert_eq!(all[2].action, "tool1");

        let capped = tool_call_records(&conn, 2).unwrap();
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].action, "tool3");
    }

    #[test]
    fn dialogue_bubbles_flat_ordered_and_session_scoped() {
        let conn = mem();
        add_event_meta(&conn, "dialogue", "早上好", Some("user"), Some("default")).unwrap();
        add_event_meta(&conn, "dialogue", "早。今天有雨。", Some("assistant"), Some("default")).unwrap();
        add_event_meta(&conn, "dialogue", "明天呢？", Some("user"), Some("default")).unwrap();
        // noise: non-dialogue + other session — must be excluded.
        add_event_meta(&conn, "perception", "窗口打开", None, None).unwrap();
        add_event_meta(&conn, "dialogue", "别的会话", Some("user"), Some("other")).unwrap();

        let rows = dialogue_bubbles(&conn, "default", 50, None).unwrap();
        assert_eq!(rows.len(), 3, "flat rows: proactive/reply-less kept, noise dropped");
        assert_eq!(rows[0].role, "user");
        assert_eq!(rows[0].content, "早上好");
        assert_eq!(rows[2].role, "user");
        assert!(rows[1].ts.contains("20"), "ts carried from events");

        let capped = dialogue_bubbles(&conn, "default", 2, None).unwrap();
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].content, "早。今天有雨。", "limit takes the newest, oldest first out");

        // Pagination cursor: capped = [id2, id3]; page before id2 → only id1.
        let page2 = dialogue_bubbles(&conn, "default", 2, Some(capped[0].id)).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].content, "早上好", "cursor returns strictly older rows");
        let page1_only = dialogue_bubbles(&conn, "default", 1, Some(rows[2].id)).unwrap();
        assert_eq!(page1_only.len(), 1);
        assert_eq!(page1_only[0].content, "早。今天有雨。", "cursor excludes itself and everything after");
    }

    #[test]
    fn dialogue_turns_meta_carries_ids_and_days_for_the_digest() {
        let conn = mem();
        add_event_meta(&conn, "dialogue", "问一", Some("user"), Some("s1")).unwrap();
        let a1 = add_event_meta(&conn, "dialogue", "答一", Some("assistant"), Some("s1")).unwrap();
        add_event_meta(&conn, "dialogue", "问二", Some("user"), Some("s1")).unwrap();
        let a2 = add_event_meta(&conn, "dialogue", "答二", Some("assistant"), Some("s1")).unwrap();

        let turns = dialogue_turns_meta(&conn, "s1", 12).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].upto_id, a1, "水位线取该轮的 assistant 行");
        assert_eq!(turns[1].upto_id, a2);
        assert_eq!(turns[0].day.chars().count(), 5, "本地 MM-DD");

        // 纯文本视图与旧契约一致：T6 把配对逻辑收敛成一份实现。
        let plain = dialogue_history(&conn, "s1", 12).unwrap();
        assert_eq!(plain.len(), turns.len());
        assert_eq!(plain[1].assistant, "答二");
        assert_eq!(plain[0].user, "问一");
    }

    #[test]
    fn session_digest_pointer_only_moves_forward() {
        let conn = mem();
        session_create(&conn, "s1", "会话").unwrap();
        assert_eq!(session_digest(&conn, "s1").unwrap(), (None, None));

        session_set_digest(&conn, "s1", "- [11-22] 甲 → 乙", 30).unwrap();
        assert_eq!(
            session_set_digest(&conn, "s1", "- [11-21] 旧 → 旧", 10).unwrap(),
            1
        );
        let (text, upto) = session_digest(&conn, "s1").unwrap();
        assert_eq!(text.as_deref(), Some("- [11-21] 旧 → 旧"), "正文按最后一次写入");
        assert_eq!(upto, Some(30), "水位线不许被旧数据拖回去");

        // 行不存在（proactive）：静默 0 行，不报错、不建行。
        assert_eq!(session_set_digest(&conn, "proactive", "x", 1).unwrap(), 0);
        assert!(session_get(&conn, "proactive").unwrap().is_none());
    }

    #[test]
    fn session_set_digest_if_refuses_to_clobber_a_newer_merge() {
        let conn = mem();
        session_create(&conn, "s1", "会话").unwrap();
        session_set_digest(&conn, "s1", "L1 v1", 5).unwrap();

        // L2 看到的也是 v1 → 允许覆盖。
        assert_eq!(session_set_digest_if(&conn, "s1", "L1 v1", "L2 人话").unwrap(), 1);
        // 但若库里的已经变了（L1 又追加过），就不许覆盖。
        assert_eq!(session_set_digest_if(&conn, "s1", "L1 v1", "过期的 L2").unwrap(), 0);
        assert_eq!(
            session_digest(&conn, "s1").unwrap().0.as_deref(),
            Some("L2 人话")
        );
    }

    #[test]
    fn digest_candidates_skip_archived_and_empty() {
        let conn = mem();
        session_create(&conn, "with-digest", "有摘要").unwrap();
        session_set_digest(&conn, "with-digest", "- [11-22] 甲", 5).unwrap();
        session_create(&conn, "empty", "没摘要").unwrap();
        session_create(&conn, "archived", "归档").unwrap();
        session_set_digest(&conn, "archived", "- [11-22] 乙", 6).unwrap();
        session_set_archived(&conn, "archived", true).unwrap();

        let rows = session_digest_candidates(&conn, "1970-01-01T00:00:00Z", 3).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"with-digest"));
        assert!(!ids.contains(&"empty"), "没摘要就没有可重写的东西");
        assert!(!ids.contains(&"archived"), "归档会话不该被夜间任务翻出来");
    }

    #[test]
    fn forget_session_and_all_cascade() {
        let conn = mem();
        let s1 = add_event_meta(&conn, "chat", "alpha", Some("user"), Some("sess-1")).unwrap();
        add_fact(&conn, "fact from sess-1", Some(s1)).unwrap();
        let s2 = add_event_meta(&conn, "chat", "beta", Some("user"), Some("sess-2")).unwrap();
        add_fact(&conn, "fact from sess-2", Some(s2)).unwrap();
        add_boundary(&conn, "never interrupt", None).unwrap();

        forget_session(&conn, "sess-1").unwrap();
        let rows = all_events(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("sess-2"));

        forget_all(&conn).unwrap();
        assert!(all_events(&conn).is_empty());
        assert!(active_facts(&conn).unwrap().is_empty());
    }

    #[test]
    fn v14_database_backfills_default_session_and_observability_columns() {
        // v14 老库：events 已有会话列，sessions 还不存在，turn_trace 没有
        // 观测三列，tasks 没有 session_id。迁移必须补齐且保留老数据。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 kind TEXT NOT NULL, content TEXT NOT NULL, role TEXT, session_id TEXT,
                 consumed INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE tasks (id TEXT PRIMARY KEY, created_at TEXT NOT NULL,
                 goal TEXT NOT NULL, status TEXT NOT NULL,
                 tool_calls INTEGER NOT NULL DEFAULT 0, final_reply TEXT,
                 plan_json TEXT, plan_mode TEXT NOT NULL DEFAULT 'none', cancel_reason TEXT,
                 confirm_mode TEXT NOT NULL DEFAULT 'auto');
             CREATE TABLE turn_trace (id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL, user_event_id INTEGER, assistant_event_id INTEGER,
                 kind TEXT NOT NULL, model_used INTEGER NOT NULL DEFAULT 0, tool_label TEXT,
                 filter_report TEXT, duration_ms INTEGER, user_text TEXT, reply_text TEXT,
                 ts TEXT NOT NULL);
             INSERT INTO events (ts, kind, content, role, session_id) VALUES
                 ('2026-11-20T08:00:00Z','dialogue','第一句用户消息
第二行','user','default'),
                 ('2026-11-20T08:00:01Z','dialogue','回复','assistant','default'),
                 ('2026-11-20T09:00:00Z','dialogue','早','assistant','proactive');
             INSERT INTO tasks (id, created_at, goal, status) VALUES
                 ('legacy-task','2026-11-20T08:00:02Z','老任务','done');
             PRAGMA user_version = 14;",
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);

        let s = session_get(&conn, "default").unwrap().expect("老库要回填 default 行");
        assert_eq!(s.title, "第一句用户消息 第二行", "标题取首条用户消息，换行折空格");
        assert_eq!(s.message_count, 2, "只数对话事件");
        assert!(s.digest.is_none(), "回填不造摘要");
        assert!(session_get(&conn, "proactive").unwrap().is_none(), "主动搭话不建行");
        assert_eq!(session_list(&conn, true).unwrap().len(), 1);

        let trace_cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(turn_trace)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        for c in ["prompt_tokens_est", "prompt_tokens_actual", "dropped_turns"] {
            assert!(trace_cols.iter().any(|x| x == c), "v15 缺列: {c}");
        }
        let task_cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(tasks)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert!(task_cols.iter().any(|x| x == "session_id"), "tasks 缺 session_id");
        assert_eq!(task_list(&conn, 10).unwrap().len(), 1, "老任务保留");

        // 二次迁移幂等：不重复回填、不重复加列。
        migrate(&conn).unwrap();
        assert_eq!(session_list(&conn, true).unwrap().len(), 1);
    }

    #[test]
    fn dialogue_events_touch_sessions_but_proactive_stays_out() {
        let conn = mem();
        add_event_meta(&conn, "dialogue", "在吗", Some("user"), Some("s1")).unwrap();
        add_event_meta(&conn, "dialogue", "在", Some("assistant"), Some("s1")).unwrap();
        let s = session_get(&conn, "s1").unwrap().unwrap();
        assert_eq!(s.title, "在吗");
        assert_eq!(s.message_count, 2);

        // 非对话事件（心跳、感知）不建会话。
        add_event_meta(&conn, "clipboard", "复制了一段", None, Some("s1")).unwrap();
        assert_eq!(session_get(&conn, "s1").unwrap().unwrap().message_count, 2);

        add_event_meta(&conn, "dialogue", "早", Some("assistant"), Some(PROACTIVE_SESSION_ID))
            .unwrap();
        assert!(
            session_get(&conn, PROACTIVE_SESSION_ID).unwrap().is_none(),
            "proactive 不进会话列表"
        );
    }

    #[test]
    fn forget_session_clears_trace_tasks_and_the_session_row() {
        let conn = mem();
        let u1 = add_event_meta(&conn, "dialogue", "帮我整理文件", Some("user"), Some("s1")).unwrap();
        add_event_meta(&conn, "dialogue", "好了", Some("assistant"), Some("s1")).unwrap();
        add_event_meta(&conn, "dialogue", "另一个话题", Some("user"), Some("s2")).unwrap();
        add_fact(&conn, "s1 的事实", Some(u1)).unwrap();
        let task_s1 = task_start_in(&conn, "帮我整理文件", Some("s1")).unwrap();
        let task_s2 = task_start_in(&conn, "另一个话题", Some("s2")).unwrap();
        conn.execute(
            "INSERT INTO turn_trace (session_id, user_event_id, kind, user_text, reply_text, ts)
             VALUES ('s1', ?1, 'model', '帮我整理文件', '好了', '2026-11-23T00:00:00Z')",
            params![u1],
        )
        .unwrap();
        let turn_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO trace_steps (turn_id, seq, step_kind, title, prompt, ts)
             VALUES (?1, 0, 'llm_round', '模型作答', 'SYSTEM 全文+用户原话', '2026-11-23T00:00:00Z')",
            params![turn_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turn_trace (session_id, kind, user_text, ts)
             VALUES ('s2', 'model', '另一个话题', '2026-11-23T00:00:01Z')",
            [],
        )
        .unwrap();

        forget_session(&conn, "s1").unwrap();

        assert!(session_get(&conn, "s1").unwrap().is_none(), "会话行要删");
        assert!(session_get(&conn, "s2").unwrap().is_some(), "别的会话不动");
        assert!(
            all_events(&conn).iter().all(|e| e.session_id.as_deref() != Some("s1")),
            "事件要删"
        );
        // trace 里存着未截断的 system 与用户原话：必须一起走。
        let traces: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_trace WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(traces, 0);
        let steps: i64 = conn
            .query_row("SELECT COUNT(*) FROM trace_steps", [], |r| r.get(0))
            .unwrap();
        assert_eq!(steps, 0, "trace_steps 随 turn 级联删除");
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tasks, 0, "任务行（goal 存原话）要删");
        let keep: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks WHERE id = ?1", params![task_s2], |r| r.get(0))
            .unwrap();
        assert_eq!(keep, 1);
        let _ = task_s1;
    }

    #[test]
    fn forget_all_clears_trace_tasks_and_sessions() {
        let conn = mem();
        add_event_meta(&conn, "dialogue", "随便聊聊", Some("user"), Some("s1")).unwrap();
        task_start_in(&conn, "随便聊聊", Some("s1")).unwrap();
        conn.execute(
            "INSERT INTO turn_trace (session_id, kind, ts) VALUES ('s1','model','2026-11-23T00:00:00Z')",
            [],
        )
        .unwrap();
        let tid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO trace_steps (turn_id, seq, step_kind, title, ts)
             VALUES (?1, 0, 'llm_round', '模型作答', '2026-11-23T00:00:00Z')",
            params![tid],
        )
        .unwrap();

        forget_all(&conn).unwrap();
        for (table, sql) in [
            ("sessions", "SELECT COUNT(*) FROM sessions"),
            ("tasks", "SELECT COUNT(*) FROM tasks"),
            ("turn_trace", "SELECT COUNT(*) FROM turn_trace"),
            ("trace_steps", "SELECT COUNT(*) FROM trace_steps"),
        ] {
            let n: i64 = conn.query_row(sql, [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "{table} 未清空");
        }
    }

    #[test]
    fn promotion_is_idempotent() {
        let conn = mem();
        assert!(promote_fact(&conn, "like", "user loves rainy days", 0.9).unwrap());
        assert!(!promote_fact(&conn, "like", "user loves rainy days", 0.9).unwrap());
    }

    #[test]
    fn growth_anchor_backfills_from_earliest_event_and_never_moves() {
        let conn = mem();
        let _clock = clock::pinned("2026-11-01T00:00:00Z");
        add_event(&conn, "chat", "oldest").unwrap();
        let _clock2 = clock::pinned("2026-11-18T00:00:00Z");
        add_event(&conn, "chat", "newer").unwrap();

        let s = ensure_growth(&conn).unwrap();
        assert_eq!(s.born_at, "2026-11-01T00:00:00Z", "anchor = earliest event");
        assert_eq!(s.stage, 0);

        // The anchor is written once; later calls never move it.
        let s2 = ensure_growth(&conn).unwrap();
        assert_eq!(s2.born_at, s.born_at);
    }

    #[test]
    fn growth_stage_monotonic_and_announcement_idempotent() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        set_growth_stage(&conn, 2).unwrap();
        // A smaller stage must not rewind the state (invariant 1).
        set_growth_stage(&conn, 0).unwrap();
        assert_eq!(growth_state(&conn).unwrap().unwrap().stage, 2);

        let id = add_growth_log(&conn, 2, "成形了。").unwrap();
        assert_eq!(pending_growth_logs(&conn).unwrap().len(), 1);
        assert!(mark_growth_notified(&conn, id).unwrap());
        // Second window open: the row is already announced → no-op.
        assert!(!mark_growth_notified(&conn, id).unwrap());
        assert!(pending_growth_logs(&conn).unwrap().is_empty());
    }

    #[test]
    fn dormancy_flips_are_guards_against_double_write() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        assert_eq!(set_dormant(&conn, Some("2026-11-19T00:00:00Z")).unwrap(), 1);
        // Already dormant: the second sleep write is a no-op.
        assert_eq!(set_dormant(&conn, Some("2026-11-20T00:00:00Z")).unwrap(), 0);
        let s = growth_state(&conn).unwrap().unwrap();
        assert_eq!(s.dormant_since.as_deref(), Some("2026-11-19T00:00:00Z"));
        assert_eq!(set_dormant(&conn, None).unwrap(), 1);
        // Awake: a second wake is a no-op.
        assert_eq!(set_dormant(&conn, None).unwrap(), 0);
    }

    #[test]
    fn forget_all_cascades_growth_tables() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        set_growth_stage(&conn, 1).unwrap();
        add_growth_log(&conn, 1, "抽枝了。").unwrap();
        forget_all(&conn).unwrap();
        assert!(growth_state(&conn).unwrap().is_none());
        // A later ensure re-anchors from scratch (events are gone too).
        let s = ensure_growth(&conn).unwrap();
        assert_eq!(s.stage, 0);
    }

    #[test]
    fn relationship_stage_is_monotonic_and_cold_flips_once() {
        let conn = mem();
        assert!(relationship_state(&conn).unwrap().is_none());
        let s = ensure_relationship(&conn).unwrap();
        assert_eq!(s.stage, 0);
        // Second ensure is a no-op (single row, no duplicate insert).
        ensure_relationship(&conn).unwrap();

        set_relationship_stage(&conn, 1).unwrap();
        let s = relationship_state(&conn).unwrap().unwrap();
        assert_eq!(s.stage, 1);
        assert!(s.stage_since.is_some(), "promotion stamps stage_since");

        // Going backwards is a silent no-op — invariant 1 (never rewinds).
        set_relationship_stage(&conn, 0).unwrap();
        assert_eq!(relationship_state(&conn).unwrap().unwrap().stage, 1);
        set_relationship_stage(&conn, 2).unwrap();
        assert_eq!(relationship_state(&conn).unwrap().unwrap().stage, 2);

        // Cold stamps once, thaws once (double writes are no-ops).
        assert_eq!(set_relationship_cold(&conn, Some("2026-11-19T00:00:00Z")).unwrap(), 1);
        assert_eq!(set_relationship_cold(&conn, Some("2026-11-20T00:00:00Z")).unwrap(), 0);
        assert_eq!(set_relationship_cold(&conn, None).unwrap(), 1);
        assert_eq!(set_relationship_cold(&conn, None).unwrap(), 0);
        assert_eq!(relationship_state(&conn).unwrap().unwrap().stage, 2, "cold never rewinds stage");
    }

    #[test]
    fn progress_upsert_is_recompute_not_accumulate() {
        let conn = mem();
        let ev = DayEvidence {
            present: 1,
            hits: 1,
            misses: 0,
            disagreements: 1,
            shut_ups: 0,
            score: 3,
        };
        upsert_progress(&conn, "2026-11-19", &ev).unwrap();
        // Same source data recomputed → the row is replaced, not summed.
        upsert_progress(&conn, "2026-11-19", &ev).unwrap();
        let got = progress_on_day(&conn, "2026-11-19").unwrap().unwrap();
        assert_eq!(got, ev, "double recompute must not double the score");

        // A changed source (one more miss) overwrites the day honestly.
        let ev2 = DayEvidence { misses: 1, score: 2, ..ev.clone() };
        upsert_progress(&conn, "2026-11-19", &ev2).unwrap();
        assert_eq!(progress_on_day(&conn, "2026-11-19").unwrap().unwrap().score, 2);
    }

    #[test]
    fn progress_sum_before_excludes_today() {
        let conn = mem();
        for (d, score) in [("2026-11-17", 2), ("2026-11-18", 1), ("2026-11-19", 3)] {
            upsert_progress(&conn, d, &DayEvidence { score, ..Default::default() }).unwrap();
        }
        // Today (11-19) never counts toward promotion — no mid-day jump.
        assert_eq!(progress_sum_before(&conn, "2026-11-19").unwrap(), 3);
        assert_eq!(progress_sum_before(&conn, "2026-11-20").unwrap(), 6);
        assert_eq!(progress_sum_before(&conn, "2026-11-17").unwrap(), 0);
    }

    #[test]
    fn relationship_reset_keeps_memory_and_growth() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        set_growth_stage(&conn, 2).unwrap();
        add_event(&conn, "dialogue", "hi").unwrap();
        ensure_relationship(&conn).unwrap();
        set_relationship_stage(&conn, 2).unwrap();
        add_relationship_log(&conn, 2, "会顶嘴了。").unwrap();
        upsert_progress(&conn, "2026-11-19", &DayEvidence { score: 2, ..Default::default() }).unwrap();

        forget_relationship(&conn).unwrap();
        assert!(relationship_state(&conn).unwrap().is_none());
        assert!(pending_relationship_logs(&conn).unwrap().is_empty());
        assert!(progress_on_day(&conn, "2026-11-19").unwrap().is_none());
        // Memory and the plant are untouched — "start over socially" only.
        assert_eq!(growth_state(&conn).unwrap().unwrap().stage, 2);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        // forget_all still cascades the relationship tables.
        ensure_relationship(&conn).unwrap();
        add_relationship_log(&conn, 1, "熟络了。").unwrap();
        forget_all(&conn).unwrap();
        assert!(relationship_state(&conn).unwrap().is_none());
        assert!(pending_relationship_logs(&conn).unwrap().is_empty());
    }

    #[test]
    fn settings_upsert_remove_and_onboarding_cascade() {
        let conn = mem();
        // Missing key reads as None — that's the "old user" signal.
        assert!(get_setting(&conn, "onboarding_done").unwrap().is_none());
        set_setting(&conn, "onboarding_done", "1").unwrap();
        // Upsert overwrites, never duplicates.
        set_setting(&conn, "onboarding_done", "0").unwrap();
        assert_eq!(get_setting(&conn, "onboarding_done").unwrap().as_deref(), Some("0"));
        assert_eq!(remove_setting(&conn, "onboarding_done").unwrap(), 1);
        assert!(get_setting(&conn, "onboarding_done").unwrap().is_none());

        // Skipped answers are recorded too (skipped=1 is a signal).
        record_onboarding_answer(&conn, "q1", "我妈", false).unwrap();
        record_onboarding_answer(&conn, "q4", "", true).unwrap();
        let answers = onboarding_answers(&conn).unwrap();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0], ("q1".into(), "我妈".into(), false));
        assert_eq!(answers[1], ("q4".into(), "".into(), true));

        // Nuclear forget clears the onboarding state: the honest reading of
        // "forget everything" includes "re-introduce yourself".
        set_setting(&conn, "age_group", "adult").unwrap();
        forget_all(&conn).unwrap();
        assert!(get_setting(&conn, "age_group").unwrap().is_none());
        assert!(onboarding_answers(&conn).unwrap().is_empty());
    }

    #[test]
    fn pinned_clock_is_used_for_writes() {
        let _guard = clock::pinned("2026-11-18T00:00:00Z");
        let conn = mem();
        add_event(&conn, "chat", "x").unwrap();
        let rows = all_events(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ts, "2026-11-18T00:00:00Z");
    }

    fn cal_row(external_id: &str, start: &str, title: &str) -> CalendarRow {
        CalendarRow {
            external_id: external_id.into(),
            start_at: start.into(),
            end_at: start.into(),
            title: title.into(),
            calendar_name: Some("工作".into()),
            all_day: false,
            attendee_count: 3,
            declined: false,
            canceled: false,
            availability: Some("busy".into()),
            reminded_at: None,
            fetched_at: String::new(),
        }
    }

    #[test]
    fn calendar_replace_preserves_reminded_at_and_sweeps_cancelled() {
        let mut conn = mem();
        let t0 = "2026-11-19T06:00:00+00:00";
        calendar_replace(&mut conn, &[cal_row("a", t0, "评审"), cal_row("b", t0, "一对一")], "2026-11-19T05:00:00+00:00").unwrap();
        assert_eq!(calendar_between(&conn, "2026-11-19T00:00:00+00:00", "2026-11-20T00:00:00+00:00").unwrap().len(), 2);

        // Reminder fired, then a refresh arrives: the flag must survive,
        // otherwise the same meeting is announced again 5 minutes later.
        assert!(mark_calendar_reminded(&conn, "a", t0).unwrap());
        assert!(!mark_calendar_reminded(&conn, "a", t0).unwrap()); // idempotent
        // "b" was cancelled upstream: absent from the next fetch.
        calendar_replace(&mut conn, &[cal_row("a", t0, "评审")], "2026-11-19T05:05:00+00:00").unwrap();
        let rows = calendar_between(&conn, "2026-11-19T00:00:00+00:00", "2026-11-20T00:00:00+00:00").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].external_id, "a");
        assert!(rows[0].reminded_at.is_some());
        assert_eq!(calendar_last_fetch(&conn).unwrap().as_deref(), Some("2026-11-19T05:05:00+00:00"));
    }

    #[test]
    fn calendar_next_after_picks_the_earliest_upcoming() {
        let mut conn = mem();
        calendar_replace(
            &mut conn,
            &[
                cal_row("late", "2026-11-19T09:00:00+00:00", "下午"),
                cal_row("early", "2026-11-19T06:00:00+00:00", "上午"),
            ],
            "2026-11-19T05:00:00+00:00",
        )
        .unwrap();
        let next = calendar_next_after(&conn, "2026-11-19T05:00:00+00:00").unwrap().unwrap();
        assert_eq!(next.external_id, "early");
        assert!(calendar_next_after(&conn, "2026-11-19T10:00:00+00:00").unwrap().is_none());
    }

    #[test]
    fn tool_audit_counts_and_cascades_on_forget() {
        let conn = mem();
        log_tool_call(&conn, "calendar", "agenda", "ok", Some("2 events, 40ms"), Some(40)).unwrap();
        log_tool_call(&conn, "calendar", "next", "denied", None, None).unwrap();
        let (ok, total) = tool_calls_summary(
            &conn,
            "calendar",
            "1970-01-01T00:00:00Z",
            "2999-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!((ok, total), (1, 2));
        assert_eq!(tool_calls_recent(&conn, 5).unwrap().len(), 2);
        forget_all(&conn).unwrap();
        assert!(tool_calls_recent(&conn, 5).unwrap().is_empty());
        assert!(calendar_last_fetch(&conn).unwrap().is_none());
    }

    #[test]
    fn v9_creates_trace_tables() {
        let _conn = mem();
        // Fresh schema carries the v9 tables; migrate on a legacy v1 db too.
        let legacy = Connection::open_in_memory().unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                     kind TEXT NOT NULL, content TEXT NOT NULL);
                 CREATE TABLE facts (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                     content TEXT NOT NULL, source_event_id INTEGER, invalid_at TEXT);
                 CREATE TABLE boundaries (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     content TEXT NOT NULL)",
            )
            .unwrap();
        migrate(&legacy).unwrap();
        assert_eq!(user_version(&legacy).unwrap(), SCHEMA_VERSION);
        let n: i64 = legacy
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
                 AND name IN ('turn_trace','trace_steps')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "migrated db must carry both trace tables");
    }

    #[test]
    fn trace_turn_persist_get_and_find_by_event_roundtrip() {
        let conn = mem();
        let draft = TraceTurnDraft {
            session_id: "s1".into(),
            kind: "model".into(),
            model_used: true,
            error_kind: None,
            attempts: None,
            tool_label: Some("builtin:memory_search".into()),
            filter_report: Some(r#"{"explicit":false,"hedged":false}"#.into()),
            duration_ms: Some(1234),
            user_text: Some("帮我看看明天安排".into()),
            reply_text: Some("明天上午有两条".into()),
            steps: vec![
                TraceStepDraft {
                    step_kind: "crisis_check".into(),
                    title: "危机拦截检查".into(),
                    prompt: None,
                    output: None,
                    args_json: None,
                    summary: None,
                    outcome: Some("miss".into()),
                    duration_ms: None,
                    user_text: None,
                },
                TraceStepDraft {
                    step_kind: "llm_round".into(),
                    title: "直接对话".into(),
                    prompt: Some("SYS".into()),
                    output: Some("REPLY".into()),
                    args_json: None,
                    summary: None,
                    outcome: Some("answered".into()),
                    duration_ms: Some(900),
                    user_text: Some("在吗".into()),
                },
            ],
        };
        let uid = add_event(&conn, "dialogue", "user text").unwrap();
        let aid = add_event(&conn, "dialogue", "assistant reply").unwrap();
        let turn_id = trace_turn_persist(&conn, &draft, Some(uid), Some(aid)).unwrap();

        // Full readback: master + ordered steps.
        let trace = turn_trace_get(&conn, turn_id).unwrap().expect("turn exists");
        assert_eq!(trace.kind, "model");
        assert!(trace.model_used);
        assert_eq!(trace.user_event_id, Some(uid));
        assert_eq!(trace.assistant_event_id, Some(aid));
        assert_eq!(trace.steps.len(), 2);
        assert_eq!(trace.steps[0].seq, 1);
        assert_eq!(trace.steps[0].step_kind, "crisis_check");
        assert_eq!(trace.steps[1].seq, 2);
        assert_eq!(trace.steps[1].prompt.as_deref(), Some("SYS"));

        // Both sides of the dialogue resolve to the same turn; a stranger id
        // resolves to nothing.
        assert_eq!(turn_trace_find_by_event(&conn, uid).unwrap(), Some(turn_id));
        assert_eq!(turn_trace_find_by_event(&conn, aid).unwrap(), Some(turn_id));
        assert_eq!(turn_trace_find_by_event(&conn, 999_999).unwrap(), None);
        assert!(turn_trace_get(&conn, 999_999).unwrap().is_none());
    }

    #[test]
    fn window_stats_rolls_up_tools_predictions_and_relationship() {
        let conn = mem();
        // In-window tool calls: same action twice, another once.
        log_tool_call(&conn, "calendar", "agenda_today", "ok", None, None).unwrap();
        log_tool_call(&conn, "calendar", "agenda_today", "ok", None, None).unwrap();
        log_tool_call(&conn, "memory_search", "search", "ok", None, None).unwrap();
        // Out-of-window call (1990) must not be counted.
        conn.execute(
            "INSERT INTO tool_calls (ts, tool, action, outcome) VALUES ('1990-01-01T00:00:00+00:00','x','y','ok')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO predictions (date, guess, basis, ask_after_hour, asked_at, feedback)
             VALUES ('2026-11-17','g','[]',9,'2026-11-17T09:00:00+00:00','hit'),
                    ('2026-11-18','g','[]',9,'2026-11-18T09:00:00+00:00','miss'),
                    ('2026-11-19','g','[]',9,NULL,NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO relationship_progress (date, present, score, computed_at)
             VALUES ('2026-11-17',1,1,'x'), ('2026-11-18',1,2,'x')",
            [],
        )
        .unwrap();

        let s = window_stats(
            &conn,
            "2020-01-01T00:00:00+00:00",
            "2030-01-01T00:00:00+00:00",
            "2026-11-17",
            "2026-11-19",
        )
        .unwrap();
        assert_eq!(s.tools[0], ("calendar:agenda_today".to_string(), 2));
        assert_eq!(s.tools.len(), 2);
        assert_eq!(s.predictions_asked, 2, "unasked row is not a guess made");
        assert_eq!((s.predictions_hit, s.predictions_miss), (1, 1));
        assert_eq!(s.relationship_score, 3);
        assert_eq!(s.relationship_days, 2);

        // Empty window → zeroed, never an error.
        let empty = window_stats(
            &conn,
            "1995-01-01T00:00:00+00:00",
            "1995-02-01T00:00:00+00:00",
            "1995-01-01",
            "1995-01-31",
        )
        .unwrap();
        assert_eq!(empty, WindowStats::default());
    }
