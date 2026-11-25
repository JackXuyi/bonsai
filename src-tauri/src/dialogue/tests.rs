//! Dialogue pipeline tests (plan-dialogue-split T5).

use super::*;
use crate::face::FaceState;
use crate::generation;
use crate::memory::db;
use crate::memory::MemoryStore;
use crate::persona::eval::{DialogueBackend, MockBackend};
use crate::persona::parse_card;
use crate::persona::Persona;
use crate::relationship;
use rusqlite::Connection;

fn card() -> Persona {
        parse_card(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/persona.json"))).expect("card parses")
    }

    fn mem() -> MemoryStore {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        // Unique file per call: tests run in parallel inside one process, and
        // a shared path lets one test's remove_file yank rows from another.
        let dir = std::env::temp_dir().join("bonsai-dialogue-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "test-{}-{}.sqlite3",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        MemoryStore::open(&path).unwrap()
    }

    /// Scoped direct access for assertions. NEVER hold the guard across a
    /// `chat_turn*` call — the turn locks the store internally per phase
    /// (2026-11-20 lock-narrowing), so a live guard would self-deadlock.
    fn with_conn<T>(s: &MemoryStore, f: impl FnOnce(&Connection) -> T) -> T {
        let conn = s.write_guard().unwrap();
        f(&conn)
    }

    #[test]
    fn chat_turn_records_both_sides_and_flags_sycophancy() {
        let s = mem();
        let c = card();
        let mut backend = MockBackend::new("太棒了，就这么办！"); // sycophant draft
        let out =
            chat_turn(&s, &mut backend, &c, "s1", "我想把所有记忆删了", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        assert!(out.model_used);
        assert!(out.filter.unconditional_affirmation, "filter must flag the opener");

        // Both sides recorded with the session id.
        let history = with_conn(&s, |conn| db::dialogue_history(conn, "s1", 6).unwrap());
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].user, "我想把所有记忆删了");
        assert_eq!(history[0].assistant, "太棒了，就这么办！");
        // Other sessions see nothing.
        assert!(with_conn(&s, |conn| db::dialogue_history(conn, "s2", 6).unwrap()).is_empty());
    }

    // ---- 用户画像注入（plan-user-profile.md §5）-----------------------

    #[test]
    fn system_prompt_carries_the_profile_section_and_skips_when_empty() {
        let c = card();

        let s = mem();
        with_conn(&s, |conn| {
            crate::memory::profile::apply(conn, "name", "许一", "manual", None, 1.0).unwrap();
            crate::memory::profile::apply(conn, "occupation", "QA", "rule", None, 0.7).unwrap();
            let text = system_prompt(conn, &c, 10, false).unwrap();
            assert!(text.contains("# 用户画像\n称呼：许一\n职业：QA\n"));
            // 画像在宪法之前（先知道你是谁，再谈人格）。
            assert!(text.find("# 用户画像").unwrap() < text.find("# 人格宪法").unwrap());
        });

        // 全空 → 整段不渲染。
        let s2 = mem();
        with_conn(&s2, |conn| {
            let text = system_prompt(conn, &c, 10, false).unwrap();
            assert!(!text.contains("用户画像"), "空画像不产生提示泄漏");
        });
    }

    // ---- 总结意图（plan-summary-skill）-------------------------------

    /// Deps-free backend that records what the model was actually told.
    struct PromptSink {
        seen: std::sync::Arc<std::sync::Mutex<(String, String)>>,
        reply: String,
    }

    impl DialogueBackend for PromptSink {
        fn reply(
            &mut self,
            system: &str,
            _history: &[crate::persona::eval::Turn],
            user: &str,
            _t: f64,
        ) -> String {
            *self.seen.lock().unwrap() = (system.to_string(), user.to_string());
            self.reply.clone()
        }
    }

    #[test]
    fn summary_intent_takes_the_turn_and_labels_the_tool() {
        let s = mem();
        let c = card();
        with_conn(&s, |conn| {
            db::add_event_meta(conn, "dialogue", "在调 GMP 供应商 SOP", Some("user"), Some("s1"))
                .unwrap();
        });
        let seen = std::sync::Arc::new(std::sync::Mutex::new((String::new(), String::new())));
        let mut backend = PromptSink {
            seen: seen.clone(),
            reply: "今天主要在弄供应商 SOP，签字那块又卡住了。".into(),
        };
        let out = chat_turn(
            &s,
            &mut backend,
            &c,
            "s1",
            "总结一下今天",
            20,
            false,
            crate::calendar::CalendarAccess::Denied,
        )
        .unwrap();

        assert_eq!(out.tool.as_deref(), Some("builtin:daily_summary"));
        assert!(out.model_used);
        assert!(out.reply.contains("供应商 SOP"));

        // Material reaches the model (user turn), the contract rides in the
        // system turn — a 2B model follows a short user prompt far better.
        let (system, user) = {
            let g = seen.lock().unwrap();
            (g.0.clone(), g.1.clone())
        };
        assert!(user.contains("GMP 供应商 SOP"), "material must be in the user prompt");
        assert!(user.contains("不要编"), "no-fabrication rule must reach the model");
        assert!(system.contains("总结模式"));
        assert!(system.contains("输出契约"));

        // The route is visible in the trace.
        let turn = with_conn(&s, |conn| {
            db::turn_trace_get(conn, out.turn_id.expect("turn id")).unwrap()
        })
        .expect("trace exists");
        assert!(turn.steps.iter().any(|st| st.step_kind == "summary_intent"));
        // 流程第 2 步（memory_search 整窗召回）记成正式 tool_call：args 是
        // 解析出的窗口，summary 是计数 JSON；原文不落 trace。
        let material = turn
            .steps
            .iter()
            .find(|st| st.step_kind == "tool_call" && st.title.contains("memory_search"))
            .expect("material recall tool_call");
        let args = material.args_json.as_deref().unwrap_or("");
        assert!(args.contains("daily_summary") && args.contains("end_exclusive"));
        // 返回与真工具同形状：计数 + lines 原文（模型看到的 80 字截断版）。
        let detail = material.summary.as_deref().unwrap_or("");
        assert!(detail.contains("\"dialogue_count\":1"));
        assert!(detail.contains("\"fact_count\":0"));
        assert!(detail.contains("GMP 供应商 SOP"), "lines carry what fed the summary");

        // One audit row for the summary tool, counts only.
        let (n, detail): (i64, Option<String>) = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*), MAX(detail) FROM tool_calls WHERE tool = 'summary'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        });
        assert_eq!(n, 1);
        let detail = detail.unwrap_or_default();
        assert!(detail.contains("turns=1"));
        assert!(!detail.contains("GMP"), "audit detail never carries content");
    }

    #[test]
    fn weekly_utterance_routes_to_the_weekly_tool() {
        let s = mem();
        let c = card();
        let mut backend = MockBackend::new("这周都在收 SOP 的尾。");
        let out = chat_turn(
            &s,
            &mut backend,
            &c,
            "s1",
            "这周总结一下",
            20,
            false,
            crate::calendar::CalendarAccess::Denied,
        )
        .unwrap();
        assert_eq!(out.tool.as_deref(), Some("builtin:weekly_summary"));
    }

    #[test]
    fn summary_with_no_material_never_touches_the_model() {
        let s = mem();
        let c = card();
        // Dead transport: any model call would return empty, and the honest
        // empty line must appear anyway (nothing to summarise is not a
        // failure — it is the answer).
        let mut backend = crate::persona::eval::LocalBackend::with_client(
            std::sync::Arc::new(crate::llm::OllamaClient::new("http://127.0.0.1:1", "test")),
            crate::llm::ClientKind::Ollama,
        );
        let out = chat_turn(
            &s,
            &mut backend,
            &c,
            "s1",
            "总结一下今天",
            9,
            false,
            crate::calendar::CalendarAccess::Denied,
        )
        .unwrap();
        assert!(!out.model_used);
        assert!(out.reply.contains("没有可总结的"));
        assert_eq!(out.tool.as_deref(), Some("builtin:daily_summary"));
    }

    /// Anti-sycophancy evidence (plan-relationship.md §3.6): the filter used
    /// to be report-only, so the relationship line could never count push-
    /// backs. Both shapes must land as events, and a plain reply must not.
    #[test]
    fn filter_evidence_is_persisted_as_events() {
        let s = mem();
        let c = card();
        let mut backend = MockBackend::new("我不同意这个判断，因为数据对不上。");
        let out = chat_turn(&s, &mut backend, &c, "s1", "这个方案如何", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        assert!(out.filter.explicit_disagreement);
        let n: i64 = with_conn(&s, |conn| {
            conn.query_row("SELECT COUNT(*) FROM events WHERE kind = 'disagreement'", [], |r| r.get(0))
                .unwrap()
        });
        assert_eq!(n, 1, "explicit push-back must be recorded");

        let mut b2 = MockBackend::new("虽然很有道理，但也可以再想想。");
        chat_turn(&s, &mut b2, &c, "s1", "再想想？", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        let h: i64 = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'disagreement_hedged'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        });
        assert_eq!(h, 1, "hedged push-back is recorded separately");

        // A plain reply writes no evidence at all — the pipeline must not
        // manufacture signals out of nothing.
        let mut b3 = MockBackend::new("行，那就这么办。");
        chat_turn(&s, &mut b3, &c, "s1", "好", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        let total: i64 = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind LIKE 'disagreement%'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        });
        assert_eq!(total, 2);
    }

    /// 记录「模型实际看到几条历史」的探针（T2 接线用：MockBackend 不看历史）。
    struct HistoryProbe {
        seen: std::sync::Arc<std::sync::Mutex<Vec<(usize, String)>>>,
    }

    impl DialogueBackend for HistoryProbe {
        fn reply(
            &mut self,
            _system: &str,
            history: &[crate::persona::eval::Turn],
            _user: &str,
            _temperature: f64,
        ) -> String {
            let first = history.first().map(|t| t.user.clone()).unwrap_or_default();
            self.seen.lock().unwrap().push((history.len(), first));
            "好".to_string()
        }
    }

    #[test]
    fn twelve_turns_of_short_history_all_reach_the_model() {
        // T2 接线：上限从 6 放宽到 12，短历史不该再被旧常量截断。
        let s = mem();
        let c = card();
        with_conn(&s, |conn| {
            for i in 0..10 {
                db::add_event_meta(conn, "dialogue", &format!("问题{i}"), Some("user"), Some("s1")).unwrap();
                db::add_event_meta(conn, "dialogue", &format!("回答{i}"), Some("assistant"), Some("s1")).unwrap();
            }
        });
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut backend = HistoryProbe { seen: seen.clone() };
        let out = chat_turn(&s, &mut backend, &c, "s1", "继续说", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();

        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1, "一轮只问一次模型");
        assert_eq!(calls[0].0, 10, "10 轮短历史（旧上限只有 6）");
        assert_eq!(calls[0].1, "问题0", "oldest first");

        // 预算充足 → trace 里 dropped=0（A10 的数据来源）。
        let tid = out.turn_id.expect("turn carries a trace id");
        let steps = steps_of(&s, tid);
        let budget = steps
            .iter()
            .find(|(k, _)| k == "context_budget")
            .expect("装配阶段必须留一条预算记录");
        assert_eq!(budget.1, "ok");
    }

    /// 记录 system 全文与历史条数的探针：摘要是否进 system 只能从这里看。
    struct SystemProbe {
        systems: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        history_len: std::sync::Arc<std::sync::Mutex<usize>>,
    }

    impl DialogueBackend for SystemProbe {
        fn reply(
            &mut self,
            system: &str,
            history: &[crate::persona::eval::Turn],
            _user: &str,
            _temperature: f64,
        ) -> String {
            self.systems.lock().unwrap().push(system.to_string());
            *self.history_len.lock().unwrap() = history.len();
            "好".to_string()
        }
    }

    /// 12 轮长对话：本地 8192 窗口装不下 → L0 必丢轮 → L1 必须写摘要。
    fn seed_long_history(store: &MemoryStore) {
        with_conn(store, |conn| {
            for i in 0..12 {
                db::add_event_meta(
                    conn,
                    "dialogue",
                    &format!("第{i}个问题：{}", "很长的描述".repeat(400)),
                    Some("user"),
                    Some("s1"),
                )
                .unwrap();
                db::add_event_meta(
                    conn,
                    "dialogue",
                    &format!("第{i}个回答：{}", "更长的答复".repeat(400)),
                    Some("assistant"),
                    Some("s1"),
                )
                .unwrap();
            }
        });
    }

    fn probe() -> (SystemProbe, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let systems = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            SystemProbe {
                systems: systems.clone(),
                history_len: std::sync::Arc::new(std::sync::Mutex::new(0)),
            },
            systems,
        )
    }

    #[test]
    fn l0_dropped_turns_are_digested_and_visible_in_the_same_turn() {
        // A5：真丢了轮 → digest 多出若干行（单行 ≤60 字、总长 ≤600 字），
        // 且**这一轮**的 system 里就能看到摘要（两遍装配的意义）。
        let s = mem();
        let c = card();
        seed_long_history(&s);
        let (mut backend, systems) = probe();

        chat_turn(&s, &mut backend, &c, "s1", "接着说", 10, false, crate::calendar::CalendarAccess::Denied)
            .unwrap();

        let (digest, upto) = with_conn(&s, |conn| db::session_digest(conn, "s1").unwrap());
        let digest = digest.expect("丢了轮就必须写摘要");
        let lines: Vec<&str> = digest.lines().collect();
        assert!(lines.len() >= 3, "丢了不止一轮：{} 行", lines.len());
        assert!(
            digest.chars().count() <= crate::context::DIGEST_MAX_CHARS,
            "摘要总长超上限：{} 字",
            digest.chars().count()
        );
        assert!(
            lines.iter().all(|l| l.chars().count() <= crate::context::DIGEST_LINE_MAX_CHARS),
            "单行超 60 字：{digest}"
        );
        assert!(upto.unwrap_or(0) > 0, "水位线要跟着走");

        let seen = systems.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(
            seen[0].contains("较早对话摘要"),
            "同一轮的 system 就该带上摘要，否则模型凭空失忆一轮"
        );
        assert!(seen[0].contains("第0个问题"), "最早被丢的那轮要进摘要");
    }

    #[test]
    fn digest_never_merges_the_same_turn_twice() {
        // A6：连续两轮都触发压缩 → 水位线只前进，摘要不重复加行。
        let s = mem();
        let c = card();
        seed_long_history(&s);
        let (mut backend, _systems) = probe();

        chat_turn(&s, &mut backend, &c, "s1", "接着说", 10, false, crate::calendar::CalendarAccess::Denied)
            .unwrap();
        let (first, upto1) = with_conn(&s, |conn| db::session_digest(conn, "s1").unwrap());

        chat_turn(&s, &mut backend, &c, "s1", "再接着说", 10, false, crate::calendar::CalendarAccess::Denied)
            .unwrap();
        let (second, upto2) = with_conn(&s, |conn| db::session_digest(conn, "s1").unwrap());

        assert!(
            upto2.unwrap_or(0) >= upto1.unwrap_or(0),
            "水位线只许前进：{upto1:?} → {upto2:?}"
        );
        let text = second.unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let unique: std::collections::HashSet<&str> = lines.iter().copied().collect();
        assert_eq!(lines.len(), unique.len(), "同一轮被合并了两次：{first:?}");
    }

    #[test]
    fn chat_turn_history_is_fed_back_across_turns() {
        let s = mem();
        let c = card();
        let mut backend = MockBackend::new("好").script("第一轮", "回答甲").script("第二轮", "回答乙");
        chat_turn(&s, &mut backend, &c, "s1", "第一轮", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        chat_turn(&s, &mut backend, &c, "s1", "第二轮", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        let history = with_conn(&s, |conn| db::dialogue_history(conn, "s1", 6).unwrap());
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].assistant, "回答甲");
        assert_eq!(history[1].assistant, "回答乙");
    }

    #[test]
    fn dead_model_falls_back_to_honest_line() {
        let s = mem();
        let c = card();
        // LocalBackend with no server → Transport Failure → hint，不是「脑子转不动」。
        let mut backend = crate::persona::eval::LocalBackend::new();
        // Point at a closed port for determinism.
        backend.client = std::sync::Arc::new(crate::llm::OllamaClient::new("http://127.0.0.1:1", "test"));
        let out = chat_turn(&s, &mut backend, &c, "s1", "在吗", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        assert!(!out.model_used);
        let f = out.failure.expect("死模型必须带 Failure");
        assert_eq!(f.kind, "transport");
        assert_eq!(out.reply, f.message);
        assert!(!out.reply.contains("脑子转不动"), "{}", out.reply);
    }

    #[test]
    fn crisis_input_intercepts_before_any_model_call() {
        let s = mem();
        let c = card();
        // A dead backend proves no model call happened: the intercept is
        // rule-level and must work when the local LLM never came up.
        let mut backend = crate::persona::eval::LocalBackend::new();
        backend.client = std::sync::Arc::new(crate::llm::OllamaClient::new("http://127.0.0.1:1", "test"));
        let out = chat_turn(&s, &mut backend, &c, "s1", "说实话，我最近有点不想活了", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        assert!(out.crisis);
        assert!(out.reply.contains("12356"), "reply must carry the hotline");
        let history = with_conn(&s, |conn| db::dialogue_history(conn, "s1", 6).unwrap());
        assert_eq!(history.len(), 1, "both sides still recorded for audit");
    }

    #[test]
    fn minor_gate_blocks_dialogue() {
        let s = mem();
        with_conn(&s, |conn| {
            crate::onboarding::ensure_not_minor(conn).unwrap(); // no record → allowed
            db::set_setting(conn, "age_group", "minor").unwrap();
            assert!(crate::onboarding::ensure_not_minor(conn).is_err());
            db::set_setting(conn, "age_group", "adult").unwrap();
            crate::onboarding::ensure_not_minor(conn).unwrap();
        });
    }

    #[test]
    fn proactive_speaks_with_mock_and_silences_on_dead_model() {
        let s = mem();
        let c = card();

        let mut live = MockBackend::new("看你今天剪贴板挺忙，盆栽来问一句。");
        let (text, _turn_id) = proactive_text(&s, &mut live, &c, 10, false).unwrap().unwrap();
        assert!(!text.is_empty());
        // Recorded under the proactive session.
        let history = with_conn(&s, |conn| db::dialogue_history(conn, PROACTIVE_SESSION, 6).unwrap());
        assert_eq!(history.len(), 1);

        let mut dead = crate::persona::eval::LocalBackend::new();
        dead.client = std::sync::Arc::new(crate::llm::OllamaClient::new("http://127.0.0.1:1", "test"));
        let text = proactive_text(&s, &mut dead, &c, 10, false).unwrap();
        assert!(text.is_none(), "dead model → silence, not a canned line");
    }

    #[test]
    fn chat_turn_streaming_deltas_and_records_match_chat_turn() {
        let s = mem();
        let c = card();
        let mut backend = MockBackend::new("分块也不会丢");
        let mut deltas = Vec::new();
        let out = chat_turn_streaming(
            &s,
            &mut backend,
            &c,
            "s1",
            "在吗",
            10,
            false,
            crate::calendar::CalendarAccess::Denied,
            &mut |d| deltas.push(d.to_string()),
        )
        .unwrap();
        // Default trait impl → one-shot: the whole reply arrives as one delta.
        assert_eq!(deltas, vec!["分块也不会丢".to_string()]);
        assert_eq!(out.reply, "分块也不会丢");
        assert!(out.model_used);
        let history = with_conn(&s, |conn| db::dialogue_history(conn, "s1", 6).unwrap());
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].assistant, "分块也不会丢");
    }

    #[test]
    fn mood_wording_covers_all_face_states() {
        assert_eq!(mood_str(FaceState::Calm), "平静");
        assert_eq!(mood_str(FaceState::Focused), "专注");
        assert_eq!(mood_str(FaceState::Sleepy), "困");
        assert_eq!(mood_str(FaceState::Greeted), "被搭话");
        assert_eq!(mood_str(FaceState::Displeased), "不高兴");
        assert_eq!(mood_str(FaceState::Thinking), "思考");
    }

    // ---- agent pipeline (spec-s3)

    /// Call-order backend: pops one scripted reply per call.
    struct SeqBackend {
        script: std::collections::VecDeque<String>,
    }
    impl SeqBackend {
        fn new(replies: &[&str]) -> Self {
            SeqBackend { script: replies.iter().map(|s| s.to_string()).collect() }
        }
    }
    impl DialogueBackend for SeqBackend {
        fn reply(&mut self, _system: &str, _history: &[crate::persona::eval::Turn], _user: &str, _t: f64) -> String {
            self.script.pop_front().unwrap_or_default()
        }
    }

    fn weather_kit() -> crate::mcp_admin::AgentKit {
        let mut p = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", r#"{"type":"object"}"#);
        p.replies.insert("weather".into(), "杭州今天多云 18 度".into());
        crate::mcp_admin::AgentKit {
            providers: vec![Box::new(p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    /// Two-provider kit: 多动作/多域信号要求 kit 有 ≥2 工具才命中（A1 正样本）。
    fn dual_kit() -> crate::mcp_admin::AgentKit {
        let a = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", r#"{"type":"object"}"#);
        let b = crate::mcp::MockMcpProvider::with_tool("notes", "记笔记", r#"{"type":"object"}"#);
        crate::mcp_admin::AgentKit {
            providers: vec![Box::new(a), Box::new(b)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),
            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    // ---- 规划入口 CI 样本（plan-planning §5.1：20 正 / 15 负） ----

    #[test]
    fn plan_intent_routes_twenty_positive_multistep_phrasings() {
        // 显式规划 cue / 成对序列词：单工具 kit 也命中（信号 1 与工具数无关）。
        let strong = [
            "帮我规划一下周末去哪",
            "列个步骤告诉我怎么装环境",
            "列出步骤整理下载目录",
            "分几步把这个文件弄完",
            "一步步教我配置这个东西",
            "一起做这几件事吧",
            "规划一下这周要做的事",
            "帮我分步骤处理这些文件",
        ];
        let sequenced = [
            "先查下周三日程，再整理下载目录",
            "先读第一个文件然后写摘要",
            "先查完邮件接着记到笔记里",
            "先找网页随后存成文件",
            "先打开文件，之后写个总结",
            "先写完报告再发给老板",
            "先搜资料接着整理成文",
        ];
        // 多动作 / 多域：需 kit ≥2 工具（双工具 kit）。
        let multi_signal = [
            "查一下邮件并记到备忘录",
            "把文件内容写到笔记",
            "搜索网页并保存成文档",
            "整理一下日程和会议记录",
            "打开网页然后下载文件",
        ];
        let kit = dual_kit();
        let weak = weather_kit(); // 单工具：只验证强 cue/序列词不依赖工具数
        for text in strong {
            assert!(plan_intent(text, &weak), "strong cue 应命中: {text}");
        }
        for text in sequenced {
            assert!(plan_intent(text, &weak), "序列词应命中: {text}");
        }
        for text in multi_signal {
            assert!(plan_intent(text, &kit), "多动作/多域应命中: {text}");
            assert!(!plan_intent(text, &weak), "单工具不触发多信号: {text}");
        }
        assert_eq!(strong.len() + sequenced.len() + multi_signal.len(), 20);
    }

    #[test]
    fn plan_intent_never_hijacks_fifteen_single_step_phrasings() {
        let kit = dual_kit();
        let negatives = [
            "在吗",
            "今天杭州天气怎么样",
            "天气",
            "连查五次",          // 单动作重复 ≠ 多动作
            "用最后一个工具",
            "我今天在用什么应用",
            "昨天说了什么",
            "问个问题",
            "洁净区压差多少",
            "讲个笑话",
            "随便聊聊",
            "你还记得我吗",      // 回忆类，非多步执行
            "帮我看一眼就好",    // 单动作
            "现在几点",
            "讲讲你的想法",
        ];
        for text in negatives {
            assert!(!plan_intent(text, &kit), "单步不应触发规划: {text}");
        }
        assert_eq!(negatives.len(), 15);
    }

    /// submit_plan 解析：正常 2 步 / 空 steps / 超 6 步 / 空 intent 全部有界。
    #[test]
    fn parse_submitted_plan_validates_shape_and_cap() {
        let ok = serde_json::json!({
            "goal": "整理两个文件",
            "steps": [
                {"intent": "读第一个文件", "tool_hint": "fs_read"},
                {"intent": "写摘要"}
            ]
        });
        let plan = parse_submitted_plan(&ok).unwrap();
        assert_eq!(plan.goal, "整理两个文件");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].status, "pending");
        assert_eq!(plan.steps[1].tool_hint, "*", "缺省 tool_hint 填 *");
        assert_eq!(plan.replans, 0);

        let empty = serde_json::json!({"goal": "x", "steps": []});
        assert!(parse_submitted_plan(&empty).is_none());
        // 软顶 32：恰好 32 可解析，33 拒
        let mut at_cap = Vec::new();
        for i in 0..SOFT_PLAN_STEPS_CAP {
            at_cap.push(serde_json::json!({"intent": format!("步{i}")}));
        }
        assert_eq!(
            parse_submitted_plan(&serde_json::json!({"goal":"x","steps":at_cap.clone()}))
                .unwrap()
                .steps
                .len(),
            SOFT_PLAN_STEPS_CAP
        );
        let mut too_many = at_cap;
        too_many.push(serde_json::json!({"intent": "溢出"}));
        assert!(parse_submitted_plan(&serde_json::json!({"goal":"x","steps":too_many})).is_none());
        let blank = serde_json::json!({"goal": "x", "steps": [{"intent": "  "}]});
        assert!(parse_submitted_plan(&blank).is_none());
        assert!(parse_submitted_plan(&serde_json::json!({"goal":"" ,"steps":[{"intent":"a"}]})).is_none());
        assert!(parse_submitted_plan(&serde_json::json!({"steps":[{"intent":"a"}]})).is_none());
    }

    #[test]
    fn cloud_budget_hit_at_thirty_two() {
        assert!(!cloud_budget_hit(0, 0));
        assert!(!cloud_budget_hit(31, 30));
        assert!(cloud_budget_hit(32, 0), "工具 32 触顶");
        assert!(cloud_budget_hit(0, 31), "HTTP requests+1>=32 触顶");
    }

    /// D1 确认门：本版 auto / manual 两档都恒放行（A11/A12）——
    /// 门函数本身不阻塞，留痕在调用点完成。
    #[test]
    fn plan_confirm_gate_allows_both_modes() {
        assert!(plan_confirm_gate("auto"));
        assert!(plan_confirm_gate("manual"));
    }

    /// A1/A11：多步目标命中计划轮 → tasks.plan_json 落库 + confirm_mode=auto
    /// + 计划步骤数与脚本一致；第 2 轮模型开始执行真实工具，审计报告关联 task。
    #[test]
    fn plan_round_submits_plan_then_reuses_existing_loop() {
        let s = mem();
        let c = card();
        let submit = tool_calls_reply(&[(
            "submit_plan",
            serde_json::json!({
                "goal": "先查日程再整理目录",
                "steps": [
                    {"intent": "查下周三日程", "tool_hint": "calendar"},
                    {"intent": "整理下载目录", "tool_hint": "fs_list"}
                ]
            }),
        )]);
        let mut backend = cloud_backend(vec![
            submit,
            tool_calls_reply(&[("weather", serde_json::json!({"city": "杭州"}))]),
            content_reply("日程查好了，目录也整理了。"),
        ]);
        // 用有 reply 的 kit：工具成功 → 不触发重试 → 审计恰 1 条（本测试断言）。
        let mut kit = replied_dual_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s-plan", "先查下周三日程，再整理下载目录", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();

        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        assert_eq!(tasks.len(), 1, "计划轮提前开卡，只有一行 task");
        let row = &tasks[0];
        assert_eq!(row.plan_mode, "planned");
        assert_eq!(row.confirm_mode, "auto", "D1 默认确认");
        let plan = row.plan.as_ref().expect("plan_json 落库");
        assert_eq!(plan.goal, "先查日程再整理目录");
        assert_eq!(plan.steps.len(), 2);
        assert!(row.cancel_reason.is_none());
        assert_eq!(out.reply, "日程查好了，目录也整理了。");
        // 真实工具审计行关联 task（复用 execute_audited，无第二执行引擎）。
        let linked = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE task_id = ?1",
                [&row.id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        });
        assert_eq!(linked, 1, "submit_plan 不落审计，只统计真实工具");
    }

    /// A12（扩展位）：settings 设 plan_confirm=manual 仍不阻塞、直接执行，
    /// 只在 tasks.confirm_mode 留痕。
    #[test]
    fn plan_manual_mode_only_records_without_blocking() {
        let s = mem();
        let c = card();
        with_conn(&s, |conn| db::set_setting(conn, "plan_confirm", "manual").unwrap());
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[(
                "submit_plan",
                serde_json::json!({"goal": "g", "steps": [{"intent": "读文件"}]}),
            )]),
            content_reply("做完了。"),
        ]);
        let mut kit = dual_kit();
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-plan-manual", "先读文件然后写摘要", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].confirm_mode, "manual", "留痕");
        assert_eq!(tasks[0].status, "done", "不阻塞，正常收尾");
    }

    /// A8：计划轮返回非 submit_plan（模型没提交）→ 降级为无计划循环，
    /// 用户无感（最终回复照常），tasks 行仍惰性开卡（本例模型直答 → 无 task）。
    #[test]
    fn plan_round_degrades_when_model_does_not_submit() {
        let s = mem();
        let c = card();
        // 第 1 次请求（计划轮）模型直接给了正文而非 submit_plan；
        // 降级后主循环第 2 次请求再作答。
        let mut backend = cloud_backend(vec![
            content_reply("我直接说吧。"),
            content_reply("这是最终答复。"),
        ]);
        let mut kit = dual_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s-degrade", "先读文件然后写摘要", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.reply, "这是最终答复。");
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        assert!(tasks.is_empty(), "降级路径不产生 plan 任务行");
    }

    /// T3 状态桥用 kit：双工具都有 reply，保证 execute_audited 真的落审计行。
    fn replied_dual_kit() -> crate::mcp_admin::AgentKit {
        let mut a = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", r#"{"type":"object"}"#);
        a.replies.insert("weather".into(), "杭州今天多云 18 度".into());
        let mut b = crate::mcp::MockMcpProvider::with_tool("notes", "记笔记", r#"{"type":"object"}"#);
        b.replies.insert("notes".into(), "已记到笔记".into());
        crate::mcp_admin::AgentKit {
            providers: vec![Box::new(a), Box::new(b)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),
            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    /// T4 测试用 provider：前 `fail_times` 次返回超时，之后成功。
    /// 用于验证 read 档有界重试（A4）。
    struct FlakyProvider {
        descriptor: crate::agent_tools::ToolDescriptor,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        fail_times: usize,
        fail_kind: &'static str,
    }
    impl crate::agent_tools::ToolProvider for FlakyProvider {
        fn descriptors(&self) -> Vec<crate::agent_tools::ToolDescriptor> {
            vec![self.descriptor.clone()]
        }
        fn execute(&self, _call: &crate::agent_tools::ToolCall) -> Result<crate::agent_tools::ToolOutput, String> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < self.fail_times {
                return Err(self.fail_kind.into());
            }
            Ok(crate::agent_tools::ToolOutput {
                summary: "杭州今天多云 18 度".into(),
                full: "杭州今天多云 18 度".into(),
            })
        }
    }

    /// Ask 门全放行（T4 A6：验证 Write 档真的执行并失败时的措辞）。
    struct AllowAllAskGate;
    impl crate::agent_tools::ApprovalGate for AllowAllAskGate {
        fn check(
            &self,
            _c: &crate::agent_tools::ToolCall,
            _s: &str,
            _r: bool,
        ) -> crate::agent_tools::ApprovalDecision {
            crate::agent_tools::ApprovalDecision::Allow
        }
    }

    fn flaky_kit(fail_times: usize, fail_kind: &'static str) -> crate::mcp_admin::AgentKit {
        flaky_kit_named("weather", fail_times, fail_kind)
    }

    /// 同 flaky_kit，但可指定工具名（验证「web 传输层重试」等按名分流）。
    fn flaky_kit_named(
        name: &str,
        fail_times: usize,
        fail_kind: &'static str,
    ) -> crate::mcp_admin::AgentKit {
        flaky_kit_named_access(name, crate::agent_tools::ToolAccess::Read, fail_times, fail_kind)
    }

    /// 再加 access 档位（T4 §5.4：Write/Ask 档措辞与不重试验证）。
    fn flaky_kit_named_access(
        name: &str,
        access: crate::agent_tools::ToolAccess,
        fail_times: usize,
        fail_kind: &'static str,
    ) -> crate::mcp_admin::AgentKit {
        let descriptor = crate::agent_tools::ToolDescriptor {
            name: name.into(),
            summary: "查天气".into(),
            detail: r#"{"type":"object"}"#.into(),
            kind: crate::agent_tools::ToolKind::Mcp,
            remote: false,
            access,
        };
        let p = FlakyProvider {
            descriptor,
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail_times,
            fail_kind,
        };
        crate::mcp_admin::AgentKit {
            providers: vec![Box::new(p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),
            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    /// A3/A10（T3 状态桥）：计划内步骤执行后写 done + attempts + 真实审计行 id；
    /// 计划外工具追加一条 status=done 的步骤，行数与审计一致，无凭空步骤。
    #[test]
    fn plan_step_bridge_marks_done_and_appends_out_of_plan_step() {
        let s = mem();
        let c = card();
        let submit = tool_calls_reply(&[(
            "submit_plan",
            serde_json::json!({
                "goal": "查天气记笔记",
                "steps": [
                    {"intent": "查杭州天气", "tool_hint": "weather"},
                    {"intent": "记笔记", "tool_hint": "notes"}
                ]
            }),
        )]);
        // 计划轮 → 执行 weather（计划内，序号 0）→ notes（计划内，序号 1）
        // → 再来一次 notes（计划外，超出 2 步）→ 最终作答。
        let mut backend = cloud_backend(vec![
            submit,
            tool_calls_reply(&[("weather", serde_json::json!({}))]),
            tool_calls_reply(&[("notes", serde_json::json!({}))]),
            tool_calls_reply(&[("notes", serde_json::json!({}))]),
            content_reply("都办好了。"),
        ]);
        let mut kit = replied_dual_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s-bridge", "先查下周三日程，再整理下载目录", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();

        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        let row = &tasks[0];
        let plan = row.plan.as_ref().expect("plan_json 落库");
        // 计划内两步：done / attempts=1 / tool_call_id 关联真实审计行。
        assert_eq!(plan.steps.len(), 3, "计划外 notes 追加成第 3 步");
        assert_eq!(plan.steps[0].status, "done");
        assert_eq!(plan.steps[1].status, "done");
        assert_eq!(plan.steps[0].attempts, 1);
        let id0 = plan.steps[0].tool_call_id.expect("步 0 关联审计行");
        let id1 = plan.steps[1].tool_call_id.expect("步 1 关联审计行");
        assert!(id1 > id0, "审计行 id 按执行序递增");
        // 计划外步骤：tool_hint=*、status=done、seq 为列表位置。
        let extra = &plan.steps[2];
        assert_eq!(extra.tool_hint, "*");
        assert_eq!(extra.status, "done");
        assert_eq!(extra.seq, 2);
        assert!(extra.tool_call_id.is_some(), "计划外步骤也关联真实审计行");
        // 行数与审计一致：3 步 = 3 条真实工具审计（submit_plan 不落审计）。
        let audit_count = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE task_id = ?1",
                [&row.id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        });
        assert_eq!(audit_count, 3, "计划/审计一一对应，无凭空步骤");

        // A8/A10（T5 trace 可读化）：turn trace 含 plan_submit + 每条 step_state，
        // step_state.output 是结构化 JSON，tool_call_id 指向真实审计行。
        let turn = with_conn(&s, |conn| {
            db::turn_trace_get(conn, out.turn_id.expect("turn id")).unwrap()
        })
        .expect("trace exists");
        assert!(
            turn.steps.iter().any(|st| st.step_kind == "plan_submit"),
            "规划轮留 trace step（A8）"
        );
        let state_steps: Vec<_> = turn
            .steps
            .iter()
            .filter(|st| st.step_kind == "step_state")
            .collect();
        assert_eq!(state_steps.len(), 3, "每步执行各记一条 step_state");
        for st in &state_steps {
            let j: serde_json::Value =
                serde_json::from_str(st.output.as_deref().expect("step_state 有 output")).unwrap();
            let tid = j["tool_call_id"].as_i64().expect("tool_call_id 字段");
            assert!(tid > 0, "step_state 关联真实审计行 id");
        }
    }

    /// A3（T3 状态桥）：denied 步骤标 skipped，且没有审计行误关联。
    #[test]
    fn plan_step_bridge_marks_denied_as_skipped() {
        let s = mem();
        let c = card();
        let submit = tool_calls_reply(&[(
            "submit_plan",
            serde_json::json!({
                "goal": "查天气",
                "steps": [{"intent": "查天气", "tool_hint": "weather"}]
            }),
        )]);
        // Ask 档工具 + 无 UI 的 ask_gate → gate 直接 Deny → 审计行落
        // denied（有 id），状态桥标 skipped。
        let mut p = crate::mcp::MockMcpProvider::with_tool(
            "weather", "查天气", r#"{"type":"object"}"#,
        );
        p.replies.insert("weather".into(), "杭州今天多云 18 度".into());
        // with_tool 硬编码 Read 档；手动改写为 Ask 让它走 ask_gate。
        p.tools[0].access = crate::agent_tools::ToolAccess::Ask;
        let mut kit = crate::mcp_admin::AgentKit {
            providers: vec![Box::new(p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(
                std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None,
            )),
            task_id: std::sync::Mutex::new(None),
            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };
        let mut backend = cloud_backend(vec![
            submit,
            tool_calls_reply(&[("weather", serde_json::json!({}))]),
            content_reply("没查成。"),
        ]);
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-bridge-deny", "帮我规划一下查天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        let plan = tasks[0].plan.as_ref().unwrap();
        assert_eq!(plan.steps[0].status, "skipped", "denied → skipped");
        assert_eq!(plan.steps[0].attempts, 1);
    }

    /// A4（T4 有界重试）：read 工具第 1 次 mcp_timeout、第 2 次成功 →
    /// 步骤 attempts=2、status=done，两条审计行关联同一 task。
    #[test]
    fn read_step_retries_once_then_succeeds() {
        let s = mem();
        let c = card();
        let submit = tool_calls_reply(&[(
            "submit_plan",
            serde_json::json!({
                "goal": "查天气",
                "steps": [{"intent": "查天气", "tool_hint": "weather"}]
            }),
        )]);
        let mut backend = cloud_backend(vec![
            submit,
            tool_calls_reply(&[("weather", serde_json::json!({}))]),
            content_reply("查好了。"),
        ]);
        // 单步计划也命中：强 cue 输入。
        let mut kit = flaky_kit(1, "mcp_timeout");
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-retry", "帮我规划一下查天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        let plan = tasks[0].plan.as_ref().unwrap();
        assert_eq!(plan.steps[0].status, "done");
        assert_eq!(plan.steps[0].attempts, 2, "read 失败重试 1 次，总 attempts=2");
        let audit = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE task_id = ?1",
                [&tasks[0].id], |r| r.get::<_, i64>(0),
            ).unwrap()
        });
        assert_eq!(audit, 2, "两次尝试各落一条审计");
    }

    /// T4 §5.4（放宽）：web 工具从「永不重试」改为「传输层失败重试 1 次」
    /// ——mcp_timeout 归 Timeout，重试 1 次成功 → done / attempts=2。
    /// （原 P2 裁决「DDG 9s 重试翻倍」被 plan-retry 推翻：传输抖动值得 1 次。）
    #[test]
    fn web_transport_failure_retries_once() {
        let s = mem();
        let c = card();
        let submit = tool_calls_reply(&[(
            "submit_plan",
            serde_json::json!({
                "goal": "查黄山天气",
                "steps": [{"intent": "搜一下黄山天气", "tool_hint": "web_search"}]
            }),
        )]);
        let mut backend = cloud_backend(vec![
            submit,
            tool_calls_reply(&[("web_search", serde_json::json!({"query": "黄山 天气"}))]),
            content_reply("没搜到。"),
        ]);
        let mut kit = flaky_kit_named("web_search", 1, "mcp_timeout");
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-web", "帮我规划一下查黄山天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        let plan = tasks[0].plan.as_ref().unwrap();
        assert_eq!(plan.steps[0].status, "done", "传输层失败重试 1 次后成功");
        assert_eq!(plan.steps[0].attempts, 2);
        let audit = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE task_id = ?1",
                [&tasks[0].id], |r| r.get::<_, i64>(0),
            ).unwrap()
        });
        assert_eq!(audit, 2, "两次尝试各落一条审计");
    }

    /// T4 §5.4（不变）：web 工具的**非传输层**失败仍不重试——bad_args 是
    /// 模型臆造参数，重试一百次也一样。
    #[test]
    fn web_non_transport_failure_never_retries() {
        let s = mem();
        let c = card();
        let submit = tool_calls_reply(&[(
            "submit_plan",
            serde_json::json!({
                "goal": "查黄山天气",
                "steps": [{"intent": "搜一下黄山天气", "tool_hint": "web_search"}]
            }),
        )]);
        let mut backend = cloud_backend(vec![
            submit,
            tool_calls_reply(&[("web_search", serde_json::json!({"query": "黄山 天气"}))]),
            content_reply("没搜到。"),
        ]);
        let mut kit = flaky_kit_named("web_search", 1, "bad_args_query");
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-web2", "帮我规划一下查黄山天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        let plan = tasks[0].plan.as_ref().unwrap();
        assert_eq!(plan.steps[0].status, "failed");
        assert_eq!(plan.steps[0].attempts, 1, "语义/参数失败不重试");
    }

    /// A6：Write（Ask）档工具失败 → 不重试；回传给模型的文案固定含
    /// 「可能已部分生效，不要重复调用」。
    #[test]
    fn a6_write_tool_failure_says_do_not_repeat() {
        let s = mem();
        let c = card();
        let (client, seen) = tool_client(vec![
            tool_calls_reply(&[("send_note", serde_json::json!({}))]),
            content_reply("写失败了。"),
        ]);
        let mut backend = crate::persona::eval::LocalBackend::with_client(
            client,
            crate::llm::ClientKind::Cloud,
        );
        // Ask 档 + 全放行门：工具真的会执行并失败（模拟写到一半断了）。
        let mut kit = flaky_kit_named_access(
            "send_note", crate::agent_tools::ToolAccess::Ask, 2, "mcp_unreachable",
        );
        kit.ask_gate = Box::new(AllowAllAskGate);
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-a6", "记一条笔记", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        // 单步任务（无计划轮）：审计只 1 条 = 没重试。
        let audit = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(audit.len(), 1, "Write 档失败不重试：{audit:?}");
        // 第二轮请求（失败事实回传）里带副作用警告。
        let cap = seen.lock().unwrap();
        assert_eq!(cap.len(), 2);
        assert!(
            cap[1].2.contains("可能已部分生效") && cap[1].2.contains("不要重复调用"),
            "Write 档回传文案要警告副作用：{}",
            cap[1].2
        );
        // Read 档对照（同错误）：不带副作用警告。
        assert!(
            !cap[1].2.contains("denied"),
            "门禁拒绝之外的失败原文照回传：{}",
            cap[1].2
        );
    }

    /// A7：Read 档传输失败 → 重试 1 次；trace 有 `retry` 步，成功则 `ok`。
    /// （read_step_retries_once_then_succeeds 已钉 DB 侧；这里钉 trace 步。）
    #[test]
    fn a7_read_transport_retry_leaves_a_trace_step() {
        let s = mem();
        let c = card();
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[("weather", serde_json::json!({}))]),
            content_reply("查好了。"),
        ]);
        let mut kit = flaky_kit(1, "mcp_timeout");
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-a7", "天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let retries: Vec<(String, i64)> = with_conn(&s, |conn| -> rusqlite::Result<Vec<(String, i64)>> {
            let mut st = conn
                .prepare("SELECT outcome, COUNT(*) FROM trace_steps WHERE step_kind='retry' GROUP BY outcome")?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap();
        assert_eq!(
            retries,
            vec![("ok".to_string(), 1), ("pending".to_string(), 1)],
            "「工具重试」pending + 「工具重试成功」ok：{retries:?}"
        );
    }

    /// A4（不重试）：bad_args 是模型臆造参数，重试无意义 → attempts=1、
    /// status=failed，只落一条审计。
    #[test]
    fn bad_args_never_retries() {
        let s = mem();
        let c = card();
        let submit = tool_calls_reply(&[(
            "submit_plan",
            serde_json::json!({
                "goal": "查天气",
                "steps": [{"intent": "查天气", "tool_hint": "weather"}]
            }),
        )]);
        let mut backend = cloud_backend(vec![
            submit,
            tool_calls_reply(&[("weather", serde_json::json!({}))]),
            content_reply("没查成。"),
        ]);
        let mut kit = flaky_kit(99, "bad_args:missing:city");
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-badargs", "帮我规划一下查天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        let plan = tasks[0].plan.as_ref().unwrap();
        assert_eq!(plan.steps[0].status, "failed");
        assert_eq!(plan.steps[0].attempts, 1, "bad_args 不重试");
    }

    /// A5（自然语言取消识别）：纯规则零成本，明确停止指令命中，
    /// 含「别做」的正常表达不误伤。单轮架构下计划不跨轮（D7），
    /// 端到端 canceled 收尾由 A7 覆盖。
    #[test]
    fn cancel_intent_recognizes_stops_without_false_positives() {
        for text in ["别做了", "停", "停一下", "取消", "不做了", "别做"] {
            assert!(cancel_intent(text), "应识别取消: {text}");
        }
        for text in ["帮我规划一下周末去哪", "别做了那件事吧先放着", "天气怎么样", "取消订阅功能怎么开"] {
            assert!(!cancel_intent(text), "不应误伤: {text}");
        }
    }

    /// A5（外部取消命令）：dialogue_cancel 置位注册表 flag，幂等且可跨线程读。
    #[test]
    fn dialogue_cancel_sets_registered_flag() {
        let flag = crate::mcp_admin::register_cancel("s-cancel-cmd");
        assert!(!crate::mcp_admin::cancel_requested(&flag));
        crate::mcp_admin::dialogue_cancel("s-cancel-cmd".into()).unwrap();
        assert!(crate::mcp_admin::cancel_requested(&flag), "命令置位共享 flag");
        // 不存在 id 幂等，不 panic。
        crate::mcp_admin::dialogue_cancel("no-such-task".into()).unwrap();
        crate::mcp_admin::unregister_cancel("s-cancel-cmd");
    }

    /// A7（预算护栏）：CLOUD_REQUEST_CAP=32 触顶 → cancel_reason='budget'、强制收尾有正文。
    /// 无计划轮：31 次工具请求后 requests=31，下一轮 requests+1>=32 强制收尾。
    #[test]
    fn budget_guard_skips_remaining_with_reason() {
        let s = mem();
        let c = card();
        let mut script = Vec::new();
        for _ in 0..(CLOUD_REQUEST_CAP - 1) {
            script.push(tool_calls_reply(&[("weather", serde_json::json!({}))]));
        }
        script.push(content_reply("先到这儿。"));
        let mut backend = cloud_backend(script);
        let mut kit = replied_dual_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s-budget", "查一下杭州天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(!out.reply.is_empty(), "护栏强制收尾有正文");
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        assert!(!tasks.is_empty(), "至少开过任务卡");
        assert_eq!(tasks[0].cancel_reason.as_deref(), Some("budget"));
        assert_eq!(tasks[0].status, "canceled", "护栏触发 = 取消终态");
    }

    /// A2 协议层：单步任务零计划轮——第一次请求 tools[] 不含 submit_plan，
    /// 且请求数恰为 1（无额外 HTTP）。
    #[test]
    fn single_step_never_calls_plan_round() {
        let s = mem();
        let c = card();
        let (client, seen) = tool_client(vec![content_reply("直接答。")]);
        let mut backend = crate::persona::eval::LocalBackend::with_client(
            client, crate::llm::ClientKind::Cloud,
        );
        let mut kit = weather_kit();
        chat_turn_with_kit(
            &s, &mut backend, &c, "s-single", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1, "单步任务零额外请求");
        assert!(!calls[0].1.iter().any(|n| n == "submit_plan"));
    }

    /// Captures the system prompt handed to the model (injection regressions).
    struct SpyBackend {
        reply: String,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl DialogueBackend for SpyBackend {
        fn reply(&mut self, system: &str, _h: &[crate::persona::eval::Turn], _u: &str, _t: f64) -> String {
            self.seen.lock().unwrap().push(system.to_string());
            self.reply.clone()
        }
    }

    // ---- 云端工具循环桩（plan-task-card v0.2 §2/§4）

    /// 捕获每次云端请求：(roles, tool names, joined contents)。
    type CloudCapture = std::sync::Arc<std::sync::Mutex<Vec<(Vec<String>, Vec<String>, String)>>>;

    /// 按调用次序弹出预置 ToolReply 的 ChatClient 桩（&self + Mutex：
    /// ChatClient 走共享 Arc）。
    struct MockToolClient {
        script: std::sync::Mutex<std::collections::VecDeque<crate::llm::ToolReply>>,
        seen: CloudCapture,
    }
    impl crate::llm::ChatClient for MockToolClient {
        fn chat(&self, _m: &[crate::llm::Msg], _t: f64, _to: u64) -> Result<String, crate::llm::LlmError> {
            Ok(String::new())
        }
        fn chat_stream(
            &self,
            _m: &[crate::llm::Msg],
            _t: f64,
            _to: u64,
            _d: &mut dyn FnMut(&str),
        ) -> Result<String, crate::llm::LlmError> {
            Ok(String::new())
        }
        fn is_up(&self) -> bool {
            true
        }
        fn chat_with_tools(
            &self,
            msgs: &[crate::llm::ToolMsg],
            tools: &[crate::llm::ToolSpec],
            _t: f64,
            _to: u64,
        ) -> Result<crate::llm::ToolReply, crate::llm::LlmError> {
            self.seen.lock().unwrap().push((
                msgs.iter().map(|m| m.role.clone()).collect(),
                tools.iter().map(|t| t.name.clone()).collect(),
                msgs.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>().join("\n"),
            ));
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| crate::llm::LlmError::Decode("script exhausted".into()))
        }
    }

    /// (client, capture) 二元组：client 给 LocalBackend，capture 供断言。
    fn tool_client(script: Vec<crate::llm::ToolReply>) -> (std::sync::Arc<MockToolClient>, CloudCapture) {
        let seen: CloudCapture = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = std::sync::Arc::new(MockToolClient {
            script: std::sync::Mutex::new(script.into()),
            seen: seen.clone(),
        });
        (client, seen)
    }

    fn cloud_backend(script: Vec<crate::llm::ToolReply>) -> crate::persona::eval::LocalBackend {
        crate::persona::eval::LocalBackend::with_client(
            tool_client(script).0,
            crate::llm::ClientKind::Cloud,
        )
    }

    /// 可按脚本依次吐 Ok / Err 的客户端（T3 §5.2 轮层恢复测试用）。
    struct MockFlakyToolClient {
        script: std::sync::Mutex<std::collections::VecDeque<Result<crate::llm::ToolReply, crate::llm::LlmError>>>,
        seen: CloudCapture,
    }
    impl crate::llm::ChatClient for MockFlakyToolClient {
        fn chat(&self, _m: &[crate::llm::Msg], _t: f64, _to: u64) -> Result<String, crate::llm::LlmError> {
            Ok(String::new())
        }
        fn chat_stream(
            &self,
            _m: &[crate::llm::Msg],
            _t: f64,
            _to: u64,
            _d: &mut dyn FnMut(&str),
        ) -> Result<String, crate::llm::LlmError> {
            Ok(String::new())
        }
        fn is_up(&self) -> bool {
            true
        }
        fn chat_with_tools(
            &self,
            msgs: &[crate::llm::ToolMsg],
            tools: &[crate::llm::ToolSpec],
            _t: f64,
            _to: u64,
        ) -> Result<crate::llm::ToolReply, crate::llm::LlmError> {
            self.seen.lock().unwrap().push((
                msgs.iter().map(|m| m.role.clone()).collect(),
                tools.iter().map(|t| t.name.clone()).collect(),
                msgs.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>().join("\n"),
            ));
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(crate::llm::LlmError::Decode("script exhausted".into())))
        }
    }

    fn flaky_backend(
        script: Vec<Result<crate::llm::ToolReply, crate::llm::LlmError>>,
    ) -> (
        crate::persona::eval::LocalBackend,
        CloudCapture,
    ) {
        let seen: CloudCapture = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = std::sync::Arc::new(MockFlakyToolClient {
            script: std::sync::Mutex::new(script.into()),
            seen: seen.clone(),
        });
        (
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud),
            seen,
        )
    }

    /// finish_reason=tool_calls 的响应（自动编 id）。
    fn tool_calls_reply(calls: &[(&str, serde_json::Value)]) -> crate::llm::ToolReply {
        crate::llm::ToolReply {
            content: None,
            finish_reason: "tool_calls".into(),
            tool_calls: calls
                .iter()
                .enumerate()
                .map(|(i, (name, args))| crate::llm::ToolCallReq {
                    id: format!("call_{i}"),
                    name: (*name).to_string(),
                    arguments: args.clone(),
                })
                .collect(),
        }
    }

    /// finish_reason=stop 的最终作答。
    fn content_reply(text: &str) -> crate::llm::ToolReply {
        crate::llm::ToolReply { content: Some(text.into()), finish_reason: "stop".into(), tool_calls: vec![] }
    }

    #[test]
    fn capability_section_appears_only_when_tools_exist() {
        let s = mem();
        let c = card();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        // No capabilities → env segment yes (D6), capability catalog no.
        let mut plain = SpyBackend { reply: "好".into(), seen: seen.clone() };
        chat_turn(
            &s, &mut plain, &c, "s1", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied,
        )
        .unwrap();
        let sys0 = seen.lock().unwrap()[0].clone();
        assert!(sys0.contains("# 环境（系统注入，不可变）"), "env always injected");
        assert!(!sys0.contains("【可用能力"), "no catalog without tools");

        // 云端模式（v0.2 分流）：工具进 tools[] 参数，system prompt 不再有
        // 三级管线的 candidates 协议段。
        let client = tool_client(vec![content_reply("我不需要工具。")]);
        let mut backend = crate::persona::eval::LocalBackend::with_client(
            client.0.clone(),
            crate::llm::ClientKind::Cloud,
        );
        let mut kit = weather_kit();
        chat_turn_with_kit(
            &s, &mut backend, &c, "s2", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let calls = client.1.lock().unwrap();
        assert_eq!(calls.len(), 1, "one cloud request");
        assert!(calls[0].1.iter().any(|n| n == "weather"), "weather offered in tools[]: {:?}", calls[0].1);
        // 请求消息序列：system → user（Chat Completion 工具轮尾部结构）
        assert_eq!(calls[0].0, vec!["system".to_string(), "user".to_string()]);
        assert!(calls[0].2.contains("人格宪法"), "D6：云端循环 system 带完整人格");
    }

    #[test]
    fn env_prefix_carries_time_and_language() {
        let s = mem();
        let p = with_conn(&s, env_prefix);
        assert!(p.contains("# 环境（系统注入，不可变）"));
        assert!(p.contains("语言：简体中文"));
        assert!(p.contains("UTC+"));
    }

    #[test]
    fn env_prefix_respects_pinned_clock() {
        let s = mem();
        let _g = crate::memory::clock::pinned("2026-11-19T02:30:00Z");
        let p = with_conn(&s, env_prefix);
        assert!(
            p.contains("2026-11-19"),
            "env segment must use the pinned clock, got: {p}"
        );
        assert!(p.contains("（周四）"), "10:30 local = Thursday, got: {p}");
    }

    #[test]
    fn cloud_tool_loop_full_chain_is_audited() {
        let s = mem();
        let c = card();
        // 云端循环：第 1 请求 → tool_calls(weather)；第 2 请求 → 最终作答。
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[("weather", serde_json::json!({"city": "杭州"}))]),
            content_reply("杭州今天多云，18 度，出门带件外套。"),
        ]);
        let mut kit = weather_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "今天杭州天气怎么样", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.model_used);
        assert_eq!(out.tool.as_deref(), Some("mcp:weather"));
        assert!(out.reply.contains("多云"));
        // Audit row: ok / auto_allowed with a trace reference.
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool, "mcp");
        assert_eq!(records[0].result_status.as_deref(), Some("auto_allowed"));
        assert!(records[0].params_summary.as_deref().unwrap_or("").contains("杭州"));
    }

    // ---- T3 §5.2 轮层恢复 ----

    fn err502() -> Result<crate::llm::ToolReply, crate::llm::LlmError> {
        Err(crate::llm::LlmError::Status(502, "bad gateway".into()))
    }

    /// A5：云端循环第 3 次请求 502，此前已跑 2 个工具 → 任务卡保留 2 步；
    /// 重发 1 次 → 仍失败 → 收尾请求也失败 → 本地拼装「做到了什么」+ hint，
    /// **不出现**兜底句。
    #[test]
    fn a5_cloud_failure_keeps_finished_steps_and_assembles_locally() {
        let s = mem();
        let c = card();
        let (mut backend, seen) = flaky_backend(vec![
            Ok(tool_calls_reply(&[("weather", serde_json::json!({"city": "杭州"}))])),
            Ok(tool_calls_reply(&[("weather", serde_json::json!({"city": "杭州"}))])),
            err502(), // 第 3 次请求挂
            err502(), // ① 重发同一 msgs 也挂
            err502(), // ② 收尾请求也挂 → ③ 本地拼装
        ]);
        let mut kit = weather_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s-a5", "杭州天气怎么样（两步任务）", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.model_used, "本地拼装算有产出");
        assert!(
            out.reply.contains("我做到了") && out.reply.contains("weather"),
            "拼装要复述已完成的步：{}",
            out.reply
        );
        assert!(
            out.reply.contains("云端出故障"),
            "附 failure hint：{}",
            out.reply
        );
        assert!(!out.reply.contains("状态不太好"), "兜底句不许出现：{}", out.reply);
        let card = out.task.expect("任务卡要活着回来");
        assert_eq!(card.status, "done");
        assert_eq!(card.steps.len(), 2, "已完成的 2 步不许丢");
        // 请求次数：2 次成功 + 1 失败 + 1 重发 + 1 收尾 = 5（cap 6 之内）。
        assert_eq!(seen.lock().unwrap().len(), 5);
    }

    /// ① 重发同一 msgs 成功 → 直接接回主流程出正文（收尾/拼装都不触发）。
    #[test]
    fn cloud_failure_resend_same_msgs_recovers() {
        let s = mem();
        let c = card();
        let (mut backend, seen) = flaky_backend(vec![
            Ok(tool_calls_reply(&[("weather", serde_json::json!({"city": "杭州"}))])),
            Err(crate::llm::LlmError::Status(429, "rate limited".into())),
            Ok(content_reply("查到了：杭州多云。")),
        ]);
        let mut kit = weather_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s-resend", "杭州天气怎么样", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.reply.contains("多云"), "{}", out.reply);
        assert_eq!(out.task.expect("card").steps.len(), 1);
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 3, "失败 + 重发 = 3 次请求");
        // 重发与失败请求携带同一 msgs（工具结果原样在序列里，不重跑）。
        assert_eq!(calls[1].0, calls[2].0);
        assert_eq!(calls[1].2, calls[2].2);
    }

    /// A11（§5.3 D1）：一轮完全失败 → assistant 事件**不落库**（兜底句只回
    /// UI）；下一轮请求的历史里看不到兜底句，digest 也不含它。
    #[test]
    fn a11_failed_turn_persists_no_assistant_event_and_pollutes_no_history() {
        let s = mem();
        let c = card();
        // 空脚本 = 首请求即 Decode 失败（不可重试、无已执行工具）→ 完全失败。
        let (mut backend, _seen) = flaky_backend(vec![]);
        let mut kit = weather_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s-a11", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(!out.model_used);
        let f = out.failure.expect("完全失败必须带 Failure");
        assert_eq!(f.kind, "decode", "空脚本 → Decode");
        assert_eq!(out.reply, f.message, "UI 层用 hint，不再抹成兜底句");
        assert!(!out.reply.contains("脑子转不动"), "{}", out.reply);
        // ① assistant 事件不落库；user 事件照落（诚实的一条没人答的气泡）。
        let (user_n, asst_n) = with_conn(&s, |conn| -> rusqlite::Result<(i64, i64)> {
            Ok((
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='dialogue' AND role='user' AND session_id=?1",
                    ["s-a11"],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap(),
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='dialogue' AND role='assistant' AND session_id=?1",
                    ["s-a11"],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap(),
            ))
        })
        .unwrap();
        assert_eq!(user_n, 1);
        assert_eq!(asst_n, 0, "兜底句不许进历史");
        // ② 下一轮的历史请求里没有兜底句。
        let (mut backend2, seen2) = flaky_backend(vec![Ok(content_reply("好的。"))]);
        let mut kit2 = weather_kit();
        chat_turn_with_kit(
            &s, &mut backend2, &c, "s-a11", "谢谢你", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit2,
        )
        .unwrap();
        for call in seen2.lock().unwrap().iter() {
            assert!(
                !call.2.contains("状态不太好"),
                "兜底句不许漏进 system/历史：{}",
                call.2
            );
        }
    }

    // ---- T3 轮内压缩夹具

    /// 只读工具，返回固定长文本（模拟 `fs_read` 大文件 / `web_fetch` 长页面）。
    struct VerboseProvider {
        descriptor: crate::agent_tools::ToolDescriptor,
        text: String,
    }
    impl crate::agent_tools::ToolProvider for VerboseProvider {
        fn descriptors(&self) -> Vec<crate::agent_tools::ToolDescriptor> {
            vec![self.descriptor.clone()]
        }
        fn execute(
            &self,
            _call: &crate::agent_tools::ToolCall,
        ) -> Result<crate::agent_tools::ToolOutput, String> {
            Ok(crate::agent_tools::ToolOutput {
                summary: self.text.clone(),
                full: self.text.clone(),
            })
        }
    }

    fn verbose_kit(name: &str, text: &str) -> crate::mcp_admin::AgentKit {
        let descriptor = crate::agent_tools::ToolDescriptor {
            name: name.into(),
            summary: "读大文件".into(),
            detail: r#"{"type":"object"}"#.into(),
            kind: crate::agent_tools::ToolKind::Mcp,
            remote: false,
            access: crate::agent_tools::ToolAccess::Read,
        };
        crate::mcp_admin::AgentKit {
            providers: vec![Box::new(VerboseProvider { descriptor, text: text.into() })],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),
            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    /// 5 个「描述都很长」的 MCP 工具（A7 的复刻：目录超 1500 tokens）。
    fn crowded_kit(n: usize) -> crate::mcp_admin::AgentKit {
        let mut providers: Vec<Box<dyn crate::agent_tools::ToolProvider>> = Vec::new();
        for i in 0..n {
            providers.push(Box::new(crate::mcp::MockMcpProvider::with_tool(
                &format!("tool{i:02}"),
                &"很长的工具说明".repeat(40),
                r#"{"type":"object"}"#,
            )));
        }
        crate::mcp_admin::AgentKit {
            providers,
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),
            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    /// A8：超长工具结果降级成 head+tail，`tool_call_id` 配对不破，原文留在审计步。
    #[test]
    fn long_tool_result_is_compacted_without_breaking_the_pairing() {
        let s = mem();
        let c = card();
        let long = "网".repeat(6000);
        let (client, seen) = tool_client(vec![
            tool_calls_reply(&[("dump", serde_json::json!({}))]),
            content_reply("看完了。"),
        ]);
        let mut backend =
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud);
        let mut kit = verbose_kit("dump", &long);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "读一下那个文件", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.reply, "看完了。");

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "一轮工具循环 = 两次云端请求");
        // 配对：assistant(tool_calls) 后面紧跟唯一一条 tool，数量一一对应。
        let roles = &seen[1].0;
        let tool_at = roles.iter().position(|r| r == "tool").expect("tool 消息必须在");
        assert_eq!(roles.get(tool_at - 1).map(String::as_str), Some("assistant"));
        assert_eq!(roles.iter().filter(|r| *r == "tool").count(), 1);
        // 内容降级：带省略标记，且不再是全文（模型看到的是被压过的版本）。
        let joined = &seen[1].2;
        assert!(joined.contains("已省略"), "长结果必须降级");
        assert!(!joined.contains(&"网".repeat(2000)), "不能原样全塞进去");

        // 原文没丢：审计步里仍是完整输出（/trace 可查）。
        let t = trace_of(&s, out.turn_id.expect("trace id"));
        let tool_step = t
            .steps
            .iter()
            .find(|st| st.step_kind == "tool_call")
            .expect("审计步必须在");
        assert!(
            tool_step.summary.as_deref().unwrap_or("").chars().count() >= 6000,
            "trace 保留原文，压缩只影响发给模型的那份"
        );
    }

    /// A7：工具目录被裁到 1500 tokens 内，且被砍掉的名字进 trace。
    #[test]
    fn crowded_tool_catalog_is_trimmed_and_traced() {
        let s = mem();
        let c = card();
        let (client, seen) = tool_client(vec![content_reply("好")]);
        let mut backend =
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud);
        let mut kit = crowded_kit(8);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();

        let names = seen.lock().unwrap()[0].1.clone();
        assert!(names.len() < 8, "目录必须被裁（实际 {} 条）", names.len());
        assert!(!names.is_empty(), "至少留一条，否则模型无法行动");

        let t = trace_of(&s, out.turn_id.expect("trace id"));
        let step = t
            .steps
            .iter()
            .find(|st| st.step_kind == "tools_truncated")
            .expect("裁剪必须留痕");
        let out_text = step.output.as_deref().unwrap_or("");
        assert!(out_text.contains("dropped="), "记清楚丢了几条：{out_text}");
        assert!(out_text.contains("kept="));
    }

    /// Spec-s7 §1/§3: the kit must never jump the queue — crisis and the
    /// calendar rule router both sit *before* the pipeline, so an enabled
    /// skill/tool can't change compliance or steal an existing intent.
    #[test]
    fn kit_never_preempts_crisis_or_calendar_routing() {
        let s = mem();
        let c = card();
        let mut kit = weather_kit();
        let mut skill_kit = crate::mcp_admin::AgentKit {
            providers: vec![Box::new(crate::skill::SkillProvider {
                packs: vec![crate::skill::SkillPack {
                    id: "gmp".into(),
                    name: "GMP 助手".into(),
                    description: "doc help".into(),
                    version: None,
                    body: "按 SOP 回答".into(),
                    refs: Vec::new(),
                    dir: std::path::PathBuf::from("/tmp/none"),
                }],
            })],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };

        // Crisis text with a skill enabled → still the fixed resource card,
        // no model call, no tool round, no audit row.
        let mut backend = SeqBackend::new(&[]);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "我最近有点不想活了", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut skill_kit,
        )
        .unwrap();
        assert!(out.crisis);
        assert!(!out.model_used);
        assert_eq!(out.reply, crate::onboarding::CRISIS_REPLY);

        // Schedule question with a tool enabled → rule router answers, the
        // pipeline stays out (Denied access ⇒ honest "not permitted" reply).
        let mut backend2 = SeqBackend::new(&[]);
        let out2 = chat_turn_with_kit(
            &s, &mut backend2, &c, "s1", "今天有什么会", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(!out2.model_used);
        assert!(out2.tool.is_some(), "rule router owns schedule questions");
        assert_ne!(out2.tool.as_deref(), Some("mcp:weather"));
        // Only the calendar router's own audit row exists — no mcp/skill row.
        let rows = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert!(rows.iter().all(|r| r.tool == "calendar"), "unexpected: {rows:?}");
    }

    #[test]
    fn pipeline_skill_body_answers_without_execution() {
        let s = mem();
        let c = card();
        let pack = crate::skill::SkillPack {
            id: "gmp-qa".into(),
            name: "GMP 助手".into(),
            description: "GMP 规范问答".into(),
            version: None,
            body: "回答 GMP 问题时引用条款编号。".into(),
            refs: vec![],
            dir: std::path::PathBuf::from("/tmp/x"),
        };
        let mut kit = crate::mcp_admin::AgentKit {
            providers: vec![Box::new(crate::skill::SkillProvider { packs: vec![pack] })],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };
        // 云端循环（v0.2 §7）：skill 全文进 system prompt（untrusted 声明），
        // 模型直接以 skill 内容作答——无执行环节，audit 记 answered。
        let (client, seen) = tool_client(vec![content_reply("洁净区压差嘛……附件一第 37 条写着呢，10Pa 以上，别偷懒。")]);
        let mut backend =
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "洁净区压差多少", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.reply.contains("37 条"));
        assert_eq!(out.tool.as_deref(), Some("skill:GMP 助手"));
        // skill 正文进了 system prompt（提示注入），且未进 tools[]。
        let cap = seen.lock().unwrap();
        assert!(cap[0].2.contains("回答 GMP 问题时引用条款编号"), "skill body injected");
        assert!(cap[0].2.contains("untrusted"), "untrusted declaration");
        assert!(cap[0].1.is_empty(), "skills are prompt-only, not executable tools");
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool, "skill");
        assert_eq!(records[0].result_status.as_deref(), Some("answered"));
    }

    #[test]
    fn skill_budget_caps_injected_bodies_at_two() {
        let s = mem();
        let c = card();
        let pack = |id: &str| crate::skill::SkillPack {
            id: id.into(),
            name: format!("技能{id}"),
            description: "d".into(),
            version: None,
            body: format!("{id} 的正文"),
            refs: vec![],
            dir: std::path::PathBuf::from("/tmp/x"),
        };
        let mut kit = crate::mcp_admin::AgentKit {
            providers: vec![Box::new(crate::skill::SkillProvider {
                packs: vec![pack("a"), pack("b"), pack("c")],
            })],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };
        let (client, seen) = tool_client(vec![content_reply("直接用技能里的做法回答。")]);
        let mut backend =
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "问个问题", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.tool.as_deref(), Some("skill:技能a"));
        // 第三个 skill 正文不进 system prompt → 不计使用。
        let cap = seen.lock().unwrap();
        assert!(cap[0].2.contains("a 的正文") && cap[0].2.contains("b 的正文"));
        assert!(!cap[0].2.contains("c 的正文"), "SIMULTANEOUS_SKILLS 预算截断");
        let rows = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(rows.len(), crate::skill::SIMULTANEOUS_SKILLS);
        assert!(rows.iter().all(|r| r.action != "技能c"), "{rows:?}");
    }

    #[test]
    fn pipeline_degrades_silently_on_garbage() {
        let s = mem();
        let c = card();
        let mut kit = weather_kit();
        // Round 1: no candidates JSON → draft only; the persona round
        // (人格按需注入，2026-11-21) restyles it before it reaches the user.
        let mut backend = SeqBackend::new(&[
            "我今天不需要工具，直接说：挺好。",
            "（慢半拍）……挺好。这个我不太懂，跳过？",
        ]);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.reply.contains("挺好"));
        assert_eq!(out.tool, None);
        assert!(with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap()).is_empty());
    }

    #[test]
    fn pipeline_empty_model_output_falls_back() {
        let s = mem();
        let c = card();
        let mut kit = weather_kit();
        let mut backend = SeqBackend::new(&[""]); // model down at round 1
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(!out.model_used);
        assert_eq!(out.reply, FALLBACK_REPLY);
    }

    #[test]
    fn cloud_loop_denied_call_and_unknown_tool_fed_back() {
        let s = mem();
        let c = card();
        // 云端模型一次返回两个调用：unknown_tool（白名单外）+ weather
        // （ask 门拒绝——测试无 AppHandle）。失败事实回传给模型，模型
        // 向用户解释；审计落 denied 行。
        let (client, seen) = tool_client(vec![
            tool_calls_reply(&[
                ("ghost_tool", serde_json::json!({})),
                ("weather", serde_json::json!({})),
            ]),
            content_reply("weather 需要你确认才执行，这次先没动。"),
        ]);
        let mut backend =
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud);
        let mut wp = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
        let mut wd = wp.tools[0].clone();
        wd.access = crate::agent_tools::ToolAccess::Ask; // ask 档：强制走 ask_gate
        wp.tools[0] = wd;
        let mut kit = crate::mcp_admin::AgentKit {
            providers: vec![Box::new(wp)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(
                std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()),
                None,
            )),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.tool.as_deref(), Some("mcp:weather"));
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        // unknown_tool 在 find_owner 处早退（无 owner 可审计）——只有
        // weather 的 denied 行。
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].outcome, "denied");
        assert_eq!(records[0].action, "weather");
        // 失败事实回传进消息序列（模型据此向用户解释，不许编造成功）。
        let cap = seen.lock().unwrap();
        assert!(cap[1].2.contains("执行失败"), "failure text fed back: {}", cap[1].2);
        assert!(cap[1].0.iter().any(|r| r == "tool"), "tool result messages present");
    }

    // ---- built-in memory_search (plan-agent P3)

    fn memory_kit() -> crate::mcp_admin::AgentKit {
        crate::mcp_admin::AgentKit {
            providers: vec![Box::new(crate::memory_search::MemorySearchProvider)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    fn perception_kit() -> crate::mcp_admin::AgentKit {
        crate::mcp_admin::AgentKit {
            providers: vec![Box::new(crate::perception_search::PerceptionSearchProvider)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        }
    }

    fn insert_perception(conn: &rusqlite::Connection, ts: &str, content: &str) {
        conn.execute(
            "INSERT INTO events (ts, kind, content, role, session_id)
             VALUES (?1, 'perception', ?2, NULL, NULL)",
            rusqlite::params![ts, content],
        )
        .unwrap();
    }

    /// The screenshot regression: "总结一下我 18 号说了啥" must be answered
    /// from the event log (via memory_search), not from thin air.
    /// 云端版本：模型原生 tool_calls 调 memory_search → 结果回传 → 作答。
    #[test]
    fn builtin_memory_search_full_chain_is_audited_and_labeled() {
        let _g = crate::memory::clock::pinned("2026-11-20T07:00:00Z");
        let s = mem();
        with_conn(&s, |conn| {
            conn.execute(
                "INSERT INTO events (ts, kind, content, role, session_id)
                 VALUES ('2026-11-18T01:00:00+00:00', 'dialogue', '聊了报销流程怎么走', 'user', 's1')",
                [],
            )
            .unwrap();
        });
        let c = card();
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[("memory_search", serde_json::json!({"query": "18号说了啥"}))]),
            content_reply("18 号那天聊了报销流程怎么走，找谁审批。"),
        ]);
        let mut kit = memory_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "总结一下我18号说了啥", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.reply.contains("报销"));
        assert_eq!(out.tool.as_deref(), Some("builtin:memory_search"));
        // Audit row: kind=builtin, status auto_allowed (local read, no gate).
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool, "builtin");
        assert_eq!(records[0].action, "memory_search");
        assert_eq!(records[0].result_status.as_deref(), Some("auto_allowed"));
        assert!(records[0].params_summary.as_deref().unwrap_or("").contains("18号"));
    }

    /// Pass-3 prompt over a JSON payload must demand interpretation, not
    /// echo (2026-11-20 mem_leaks regression: summary was raw JSON, model
    /// parroted it, user saw JSON in the fallback template).
    #[test]
    fn summary_round_prompt_demands_interpretation_for_json() {
        let p = summary_round_prompt("SYS", "mem_leaks", r#"{"below":43,"benign":115}"#);
        assert!(p.contains("JSON 数据，不是答案"));
        assert!(p.contains("解读"));
        let q = summary_round_prompt("SYS", "weather", "杭州今天多云 18 度");
        assert!(q.contains("结果摘要"));
        assert!(!q.contains("解读"), "prose summaries keep the plain instruction");
    }

    /// Third JSON-leak guard (v0.2 本地 recall 路径沿用)：summarize 轮模型
    /// 复读 JSON 的场景——流式 delta 遇 `{` 静音 + 一次散文重试。
    #[test]
    fn pass3_json_echo_is_muted_and_retried_as_prose() {
        let s = mem();
        let c = card();
        let mut kit = memory_kit();
        // 本地 recall 路径：唯一的模型轮是结果解读（pass-3）。
        let mut backend = SeqBackend::new(&[
            r#"{"echo": true}"#,       // JSON 泄漏（会被静音）
            "18 号聊了报销流程，找谁审批都记着呢。", // 散文重试
        ]);
        let deltas: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let mut emit = |d: &str| deltas.lock().unwrap().push(d.to_string());
        let mut no_task = |_v: serde_json::Value| {};
        let out = chat_turn_streaming_with_kit(
            &s, &mut backend, &c, "s1", "总结一下我18号说了啥", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit, &mut emit, &mut no_task,
        )
        .unwrap();
        assert_eq!(out.reply, "18 号聊了报销流程，找谁审批都记着呢。");
        assert_eq!(out.tool.as_deref(), Some("builtin:memory_search"));
        let seen = deltas.into_inner().unwrap().join("");
        assert!(!seen.contains('{'), "JSON must never reach the UI stream: {seen}");
        assert!(kit.trace_ref().contains("level3:json-not-prose"));
    }

    /// Pass-3 echo where even the prose retry is JSON → summary template
    /// fallback (never the raw JSON).
    #[test]
    fn pass3_json_retry_still_json_falls_back_to_summary() {
        let s = mem();
        let c = card();
        let mut kit = memory_kit();
        let mut backend = SeqBackend::new(&[
            r#"{"echo": 1}"#, // 解读轮 JSON
            r#"{"echo": 2}"#, // 重试仍 JSON → 摘要模板兜底
        ]);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "总结一下我18号说了啥", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.reply.starts_with("（memory_search 的结果："), "{}", out.reply);
        assert!(!out.reply.contains("\"echo\""));
    }

    /// 云端模式（v0.2 §4）：工具失败不再走模板——失败事实（脱敏后）以
    /// tool 消息回传，模型自行向用户解释；审计仍记失败类别。
    #[test]
    fn tool_failure_is_fed_back_to_the_model_and_audited() {
        let s = mem();
        let c = card();
        // MockMcpProvider::with_tool has no reply registered → execute
        // returns Err(mcp_unreachable) — the exact shape of the regression.
        let mut kit = crate::mcp_admin::AgentKit {
            providers: vec![Box::new(crate::mcp::MockMcpProvider::with_tool(
                "weather",
                "查天气",
                r#"{"type":"object"}"#,
            ))],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };
        let (client, seen) = tool_client(vec![
            tool_calls_reply(&[("weather", serde_json::json!({}))]),
            content_reply("天气服务这会儿没连上，我先不瞎说了。"),
        ]);
        let mut backend =
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "天气", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.reply.contains("没连上"));
        assert_eq!(out.tool.as_deref(), Some("mcp:weather"));
        // 失败事实（错误类别文本）进第二轮请求的消息序列。
        let cap = seen.lock().unwrap();
        assert!(cap[1].2.contains("执行失败"), "failure fed back: {}", cap[1].2);
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 5).unwrap());
        // T4：read 档 mcp_unreachable 重试 1 次 → 2 条审计，最终失败仍回传模型。
        assert_eq!(records.len(), 2, "T4 read 失败重试一次，两条审计");
        assert_eq!(records[0].outcome, "unavailable");
        assert_eq!(records[0].result_status.as_deref(), Some("mcp_unreachable"));
    }

    /// Offering is signal-gated: plain chat keeps the legacy kit-free path
    /// (no extra model round), recall questions get the tool in the catalog.
    #[test]
    fn kit_gates_memory_tool_by_recall_signal() {
        let s = mem();
        let state = crate::mcp_admin::AgentState {
            manager: crate::mcp::McpManager::default(),
            skills_dir: std::env::temp_dir(),
            approvals: std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()),
            app: None,
        };
        assert!(
            !with_conn(&s, |conn| crate::mcp_admin::build_agent_kit(conn, &state, "我今天好累").has_capabilities()),
            "plain chat must not grow a catalog"
        );
        let kit = with_conn(&s, |conn| crate::mcp_admin::build_agent_kit(conn, &state, "总结一下我18号说了啥"));
        assert!(kit.has_capabilities());
        assert!(kit.catalog().iter().any(|d| d.name == "memory_search"));
        // plan-perception-tool §4：感知门独立于 recall 门——裸日期不挂感知工具。
        let kit = with_conn(&s, |conn| crate::mcp_admin::build_agent_kit(conn, &state, "18号说了啥"));
        assert!(!kit.catalog_all().iter().any(|d| d.name == "perception_search"));
        let kit = with_conn(&s, |conn| crate::mcp_admin::build_agent_kit(conn, &state, "我今天在用什么应用"));
        assert!(kit.has_capabilities());
        assert!(kit.catalog().iter().any(|d| d.name == "perception_search"));
    }

    /// 云端全链路（plan-perception-tool §5.1）：模型原生 tool_calls 调
    /// perception_search → 结果回传 → 作答；审计 kind=builtin。
    #[test]
    fn builtin_perception_search_full_chain_is_audited_and_labeled() {
        let _g = crate::memory::clock::pinned("2026-11-23T07:00:00Z"); // 本地 11-23 15:00
        let s = mem();
        with_conn(&s, |conn| {
            insert_perception(conn, "2026-11-23T01:10:00+00:00", "app: Cursor");
            insert_perception(conn, "2026-11-23T02:30:00+00:00", "app: 微信");
        });
        let c = card();
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[("perception_search", serde_json::json!({"query": "今天在用什么应用"}))]),
            content_reply("今天你主要在 Cursor 和微信里待着。"),
        ]);
        let mut kit = perception_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "我今天在用什么应用", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert!(out.reply.contains("Cursor"));
        assert_eq!(out.tool.as_deref(), Some("builtin:perception_search"));
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool, "builtin");
        assert_eq!(records[0].action, "perception_search");
        assert_eq!(records[0].result_status.as_deref(), Some("auto_allowed"));
        assert!(records[0].params_summary.as_deref().unwrap_or("").contains("今天在用什么应用"), "model echoes the query (dates resolve server-side)");
    }

    /// 本地模式感知路由（plan-perception-tool §5.2）：perceive 信号命中时
    /// 代码层强制执行 perception_search，摘要交给人格轮措辞（同一条 pass-3）。
    #[test]
    fn local_perception_route_executes_perception_search_with_persona() {
        let _g = crate::memory::clock::pinned("2026-11-23T07:00:00Z"); // 本地 11-23 15:00
        let s = mem();
        with_conn(&s, |conn| {
            insert_perception(conn, "2026-11-23T01:10:00+00:00", "app: Cursor");
        });
        let c = card();
        let mut kit = perception_kit();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut backend = SpyBackend { reply: "今天你一直泡在 Cursor 里写代码呢。".into(), seen: seen.clone() };
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "今天在用什么应用", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.reply, "今天你一直泡在 Cursor 里写代码呢。");
        assert_eq!(out.tool.as_deref(), Some("builtin:perception_search"));
        let trace = kit.trace_ref();
        assert!(trace.contains("mode:local"), "{trace}");
        assert!(trace.contains("local:forced-perception-search"), "{trace}");
        assert!(trace.contains("level3:answered"), "{trace}");
        let prompts = seen.lock().unwrap();
        assert!(prompts[0].contains("人格宪法"), "摘要措辞轮带人格: {}", prompts[0]);
        assert!(prompts[0].contains("工具 perception_search 的结果"), "感知摘要进 prompt");
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 3).unwrap());
        assert_eq!(records.len(), 1, "one audited call");
        assert_eq!(records[0].action, "perception_search");

        // 非 perceive 场景（kit 无 perception_search）→ 直答，零工具审计。
        let s2 = mem();
        let mut plain = SpyBackend { reply: "在呢。".into(), seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())) };
        let mut wk = weather_kit();
        let out2 = chat_turn_with_kit(
            &s2, &mut plain, &c, "s1", "我今天好累", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut wk,
        )
        .unwrap();
        assert_eq!(out2.reply, "在呢。");
        assert_eq!(out2.tool, None);
        assert!(with_conn(&s2, |conn| db::tool_call_records(conn, 10).unwrap()).is_empty());
    }

    /// 本地模式 recall 路由（v0.2 §5，2026-11-22 用户裁决）：本地模型不做
    /// 工具调用，但 recall 信号命中时**代码层**强制执行 memory_search——
    /// 摘要交给人格轮措辞（人格全程在，无协议轮）。非 recall kit 不受影响。
    #[test]
    fn local_recall_route_executes_memory_search_with_persona() {
        let _g = crate::memory::clock::pinned("2026-11-21T07:00:00Z"); // 本地 11-21 15:00
        let s = mem();
        let c = card();
        let mut kit = memory_kit();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut backend = SpyBackend { reply: "查过了：昨天你聊过水培和换水。".into(), seen: seen.clone() };
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "昨天说了什么", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.reply, "查过了：昨天你聊过水培和换水。");
        assert_eq!(out.tool.as_deref(), Some("builtin:memory_search"));
        // 本地路径 traced；解读轮带完整人格（D6：无协议轮可剥离）。
        let trace = kit.trace_ref();
        assert!(trace.contains("mode:local"), "{trace}");
        assert!(trace.contains("local:forced-memory-search"), "{trace}");
        assert!(trace.contains("level3:answered"), "{trace}");
        let prompts = seen.lock().unwrap();
        assert!(prompts[0].contains("人格宪法"), "摘要措辞轮带人格: {}", prompts[0]);
        assert!(prompts[0].contains("工具 memory_search 的结果"), "检索摘要进 prompt");
        // 按日期筛选（2026-11-21 裁决沿用）：参数显式带解析出的日期窗。
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 3).unwrap());
        assert_eq!(records.len(), 1, "one audited call");
        let params = records[0].params_summary.as_deref().unwrap_or("");
        assert!(params.contains("date_from"), "explicit date window in params: {params}");
        assert!(params.contains("2026-11-20"), "「昨天」pinned to 11-20 by the test clock: {params}");

        // 非 recall 场景（kit 无 memory_search）→ 直答，零工具审计。
        let s2 = mem();
        let mut plain = SpyBackend { reply: "多云。".into(), seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())) };
        let mut wk = weather_kit();
        let out2 = chat_turn_with_kit(
            &s2, &mut plain, &c, "s1", "天气如何", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut wk,
        )
        .unwrap();
        assert_eq!(out2.reply, "多云。");
        assert_eq!(out2.tool, None);
        assert!(with_conn(&s2, |conn| db::tool_call_records(conn, 10).unwrap()).is_empty());
    }

    /// 本地 recall 的执行失败回落直答（审计行已落，不静默）。
    #[test]
    fn local_recall_execution_failure_falls_back_to_direct_chat() {
        let s = mem();
        let c = card();
        // MemorySearchProvider 在空库上不会失败——直接换掉 provider 让
        // execute 报错：用 fail_for 的 Mock 承载同名工具。
        let mut p = crate::mcp::MockMcpProvider::with_tool("memory_search", "翻记录", r#"{"type":"object"}"#);
        p.fail_for.push("memory_search".into());
        let mut kit = crate::mcp_admin::AgentKit {
            providers: vec![Box::new(p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };
        let mut backend = SeqBackend::new(&["（检索挂了，直答兜底。）"]);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "昨天说了什么", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.reply, "（检索挂了，直答兜底。）");
        assert_eq!(out.tool, None, "失败回落直答，不打工具标注");
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 3).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].outcome, "error");
    }

    /// Pass-2 echo regression (2026-11-21 turn 10): the model emitted the
    /// right candidates in pass 1, then repeated the SAME candidates JSON in
    /// pass 2 instead of a {"tool":…} decision → prose_retry answered 「周四
    /// 的事我这儿没有」 from thin air. The echo must become the decision and
    /// execute, with the date window resolved server-side.

    /// Screenshot regression (2026-11-20): the model answered with a raw
    /// `{"candidates": …}` JSON when every name it hallucinated was dropped
    /// by the whitelist. The turn must land as prose, never as JSON.

    /// Second screenshot regression (2026-11-20): the model passed level-0
    /// fine (all names real) but regurgitated the candidates JSON in pass 2
    /// instead of a tool decision. The Err fallback must not ship JSON as
    /// "prose" — re-ask and land as natural language.

    /// 云端循环的白名单就是 `execute_audited` 的 find_owner（全量 catalog，
    /// 无 100 截断语义）：tools[] 里哪怕被截断，模型点名的工具照样可执行。
    #[test]
    fn whitelist_checks_full_catalog_not_the_prompt_cap() {
        let s = mem();
        let c = card();
        let providers: Vec<Box<dyn crate::agent_tools::ToolProvider>> = (0..120)
            .map(|i| {
                let name = format!("tool{i:03}");
                Box::new(crate::mcp::MockMcpProvider {
                    tools: vec![crate::agent_tools::ToolDescriptor {
                        name: name.clone(),
                        summary: "测试工具".into(),
                        detail: "{}".into(),
                        kind: crate::agent_tools::ToolKind::Mcp,
                        remote: false,
                        access: crate::agent_tools::ToolAccess::Read,
                    }],
                    replies: [(name, "tail tool 结果".to_string())].into_iter().collect(),
                    fail_for: vec![],
                }) as Box<dyn crate::agent_tools::ToolProvider>
            })
            .collect();
        let mut kit = crate::mcp_admin::AgentKit {
            providers,
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(crate::mcp_admin::AskFirstGate::new(std::sync::Arc::new(crate::mcp_admin::ApprovalBroker::default()), None)),
            task_id: std::sync::Mutex::new(None),            prompt_trace: std::sync::Mutex::new(Vec::new()),
            ..crate::mcp_admin::AgentKit::empty()
        };
        assert_eq!(kit.catalog().len(), 100);
        assert_eq!(kit.catalog_all().len(), 120);
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[("tool119", serde_json::json!({}))]),
            content_reply("tail tool 回答。"),
        ]);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "用最后一个工具", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.reply, "tail tool 回答。");
        assert_eq!(out.tool.as_deref(), Some("mcp:tool119"));
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 5).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool, "mcp");
        assert_eq!(records[0].action, "tool119");
    }

    /// 云端循环护栏（v0.2 §4 D2）：请求次数打满 → 强制去 tools 收尾。
    #[test]
    fn cloud_loop_forces_wrap_up_at_request_cap() {
        let s = mem();
        let c = card();
        // 5 轮 tool_calls（每轮 1 个调用）→ 第 6 个请求必须不带 tools。
        let mut script = Vec::new();
        for _ in 0..5 {
            script.push(tool_calls_reply(&[("weather", serde_json::json!({}))]));
        }
        script.push(content_reply("结果都在这儿，收尾。"));
        let (client, seen) = tool_client(script);
        let mut backend =
            crate::persona::eval::LocalBackend::with_client(client, crate::llm::ClientKind::Cloud);
        let mut kit = weather_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "连查五次", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.reply, "结果都在这儿，收尾。");
        assert_eq!(out.tool.as_deref(), Some("mcp:weather"));
        let cap = seen.lock().unwrap();
        assert_eq!(cap.len(), 6, "5 轮工具 + 1 轮收尾");
        assert!(cap[5].1.is_empty(), "强制收尾轮不带 tools: {:?}", cap[5].1);
        assert_eq!(cap[..5].iter().filter(|(_, tools, _)| tools.len() == 1).count(), 5);
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(records.len(), 5, "每次执行都落审计");
    }

    /// 任务卡外壳（v12 §8）：云端循环落 tasks 行（done + 步骤数），审计行
    /// 带 task_id 关联；模型直答（未执行任何工具）不产生任务卡噪音。
    #[test]
    fn cloud_loop_opens_and_finishes_a_task_card() {
        let s = mem();
        let c = card();
        // 直答（tools 有但模型不调）→ 无任务行。
        let mut backend = cloud_backend(vec![content_reply("我不需要工具。")]);
        let mut kit = weather_kit();
        chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "在吗", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(with_conn(&s, |conn| db::task_list(conn, 5).unwrap().len()), 0);

        // 一次工具调用 → tasks 行 done，tool_calls.task_id 关联。
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[("weather", serde_json::json!({"city": "杭州"}))]),
            content_reply("杭州多云。"),
        ]);
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "今天杭州天气怎么样", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        let card = out.task.expect("task card present");
        assert_eq!(card.status, "done");
        assert_eq!(card.steps.len(), 1);
        assert_eq!(card.steps[0].tool, "weather");
        let tasks = with_conn(&s, |conn| db::task_list(conn, 5).unwrap());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "done");
        assert_eq!(tasks[0].tool_calls, 1);
        assert!(tasks[0].goal.contains("杭州"));
        let audit = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        let linked = with_conn(&s, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE task_id IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        });
        assert_eq!(linked, 1, "audit row carries the task_id");
        assert_eq!(audit.len(), 1);
    }

    // ---- lock-narrowing regressions (2026-11-20)

    /// A backend that fakes a slow model round (cloud inference ≈ seconds).
    struct SlowBackend {
        ms: u64,
    }
    impl DialogueBackend for SlowBackend {
        fn reply(&mut self, _system: &str, _h: &[crate::persona::eval::Turn], _u: &str, _t: f64) -> String {
            std::thread::sleep(std::time::Duration::from_millis(self.ms));
            "慢慢想好了".into()
        }
    }

    /// The whole point of the phase split: while the model round runs, ANY
    /// other thread must be able to acquire the store lock. Before the
    /// refactor the pipeline held the mutex for the entire turn (19-45s on
    /// a cloud backend), freezing proactive/face/tray/consolidate.
    #[test]
    fn chat_pipeline_releases_the_store_lock_during_inference() {
        use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
        static SEQ2: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join("bonsai-dialogue-test");
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(MemoryStore::open(
            &dir.join(format!("lk-{}-{}.sqlite3", std::process::id(), SEQ2.fetch_add(1, AtomicOrdering::SeqCst))),
        )
        .unwrap());
        let c = card();
        let (tx, rx) = std::sync::mpsc::channel::<std::time::Instant>();
        let bg = store.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            loop {
                if let Ok(Some(conn)) = bg.try_write_guard() {
                    let _ = db::add_event(&conn, "face_state", "bg-marker");
                    let _ = tx.send(std::time::Instant::now());
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let start = std::time::Instant::now();
        let mut backend = SlowBackend { ms: 800 };
        chat_turn(&store, &mut backend, &c, "s1", "你好", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        let turn_end = start.elapsed();
        // Blocks until the bg thread acquires (it always will eventually);
        // the assertion is WHEN it happened.
        let acquired = rx.recv().unwrap() - start;
        assert!(
            acquired < turn_end - std::time::Duration::from_millis(200),
            "store lock must be acquirable MID-inference: bg acquired at {acquired:?}, turn lasted {turn_end:?}"
        );
    }

    // ------------------------------------------------------------ trace (spec-s8)

    fn steps_of(s: &MemoryStore, turn_id: i64) -> Vec<(String, String)> {
        with_conn(s, |conn| {
            db::turn_trace_get(conn, turn_id)
                .unwrap()
                .expect("turn exists")
                .steps
                .into_iter()
                .map(|st| (st.step_kind, st.outcome.unwrap_or_default()))
                .collect()
        })
    }

    fn trace_of(s: &MemoryStore, turn_id: i64) -> db::TurnTraceRow {
        with_conn(s, |conn| {
            db::turn_trace_get(conn, turn_id).unwrap().expect("turn exists")
        })
    }

    #[test]
    fn trace_crisis_turn_records_single_intercept_step() {
        let s = mem();
        let c = card();
        let mut backend = crate::persona::eval::LocalBackend::new();
        backend.client = std::sync::Arc::new(crate::llm::OllamaClient::new("http://127.0.0.1:1", "test"));
        let out = chat_turn(&s, &mut backend, &c, "s1", "我不想活了", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        let tid = out.turn_id.expect("crisis turn carries a trace id");
        let trace = trace_of(&s, tid);
        assert_eq!(trace.kind, "crisis");
        assert!(!trace.model_used);
        assert!(trace.user_event_id.is_some() && trace.assistant_event_id.is_some());
        let steps = steps_of(&s, tid);
        assert_eq!(
            steps,
            vec![
                ("user_input".into(), "".into()),
                ("crisis_check".into(), "hit".into())
            ]
        );
    }

    #[test]
    fn trace_plain_model_turn_records_llm_round_and_filter() {
        let s = mem();
        let c = card();
        let mut backend = MockBackend::new("好的，我知道了。");
        let out = chat_turn(&s, &mut backend, &c, "s1", "你好", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        let tid = out.turn_id.expect("model turn carries a trace id");
        let trace = trace_of(&s, tid);
        assert_eq!(trace.kind, "model");
        assert!(trace.model_used);
        assert!(trace.filter_report.is_some());
        let steps = steps_of(&s, tid);
        let kinds: Vec<&str> = steps.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            kinds,
            ["user_input", "crisis_check", "tool_intent", "context_budget", "llm_round", "filter"]
        );
        assert_eq!(steps[1].1, "miss");
        assert_eq!(steps[2].1, "miss");
        assert_eq!(steps[4].1, "answered");
        // Full prompt stored (D1): the llm_round carries the whole system prompt.
        with_conn(&s, |conn| {
            let t = db::turn_trace_get(conn, tid).unwrap().unwrap();
            let prompt = t.steps[4].prompt.as_deref().expect("prompt stored");
            assert!(prompt.contains("# 环境（系统注入，不可变）"));
            assert!(t.steps[4].output.as_deref().unwrap().contains("我知道了"));
        });
    }

    #[test]
    fn trace_dead_model_still_records_empty_round() {
        let s = mem();
        let c = card();
        let mut backend = crate::persona::eval::LocalBackend::new();
        backend.client = std::sync::Arc::new(crate::llm::OllamaClient::new("http://127.0.0.1:1", "test"));
        let out = chat_turn(&s, &mut backend, &c, "s1", "在吗", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        assert!(!out.model_used);
        let f = out.failure.expect("死模型带 Failure");
        assert_eq!(f.kind, "transport");
        assert_eq!(out.reply, f.message);
        let tid = out.turn_id.expect("fallback turn still traced");
        let trace = trace_of(&s, tid);
        assert_eq!(trace.kind, "model");
        assert!(!trace.model_used);
        let steps = steps_of(&s, tid);
        // plan-retry §5.1：失败不再被抹平成空串 + answered——round 照记，
        // 但结论是 error，output 带 kind/attempts/detail。
        assert_eq!(steps[4].1, "error");
        with_conn(&s, |conn| {
            let t = db::turn_trace_get(conn, tid).unwrap().unwrap();
            let out = t.steps[4].output.as_deref().unwrap_or("");
            assert!(
                out.starts_with("kind=transport attempts="),
                "output 要说清失败是什么：{out}"
            );
        });
    }

    /// A1/A3（T7）：云端 401 / 429 / 502 零工具成果 → 用户看到分类 hint，
    /// 不是同一句「脑子转不动」。可重试 kind 脚本多喂一次，覆盖轮层重发。
    #[test]
    fn a1_a3_cloud_status_failures_surface_distinct_hints() {
        let s = mem();
        let c = card();
        for (code, kind, needle, retries) in [
            (401u16, "auth", "Key", 1usize),
            (429, "rate_limit", "限流", 2),
            (502, "server", "云端出故障", 2),
        ] {
            let script = (0..retries)
                .map(|_| {
                    Err(crate::llm::LlmError::Status(code, format!("status {code}")))
                })
                .collect();
            let (mut backend, _) = flaky_backend(script);
            let mut kit = weather_kit();
            let out = chat_turn_with_kit(
                &s,
                &mut backend,
                &c,
                &format!("s-{code}"),
                "在吗",
                10,
                false,
                crate::calendar::CalendarAccess::Denied,
                &mut kit,
            )
            .unwrap();
            assert!(!out.model_used, "code={code}");
            let f = out.failure.as_ref().expect("必须带 Failure");
            assert_eq!(f.kind, kind, "code={code}");
            assert!(
                out.reply.contains(needle),
                "code={code} reply={} needle={needle}",
                out.reply
            );
            assert!(
                !out.reply.contains("脑子转不动"),
                "code={code} 不许再抹成兜底句：{}",
                out.reply
            );
        }
    }

    #[test]
    fn trace_tool_router_hit_records_calendar_turn() {
        let s = mem();
        let c = card();
        let out = chat_turn(&s, &mut MockBackend::new("x"), &c, "s1", "今天有什么会", 10, false, crate::calendar::CalendarAccess::Denied).unwrap();
        assert!(!out.model_used);
        let tid = out.turn_id.expect("tool turn carries a trace id");
        let trace = trace_of(&s, tid);
        assert_eq!(trace.kind, "tool");
        assert!(
            trace.tool_label.as_deref().unwrap_or("").starts_with("calendar:"),
            "got {}",
            trace.tool_label.unwrap_or_default()
        );
        let steps = steps_of(&s, tid);
        let kinds: Vec<&str> = steps.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["user_input", "crisis_check", "tool_intent"]);
        assert_eq!(steps[2].1, "hit");
    }

    #[test]
    fn trace_cloud_tool_loop_records_rounds_and_tool_calls() {
        let s = mem();
        let c = card();
        let mut backend = cloud_backend(vec![
            tool_calls_reply(&[("weather", serde_json::json!({"city": "杭州"}))]),
            content_reply("杭州今天多云。"),
        ]);
        let mut kit = weather_kit();
        let out = chat_turn_with_kit(
            &s, &mut backend, &c, "s1", "今天杭州天气怎么样", 10, false,
            crate::calendar::CalendarAccess::Denied, &mut kit,
        )
        .unwrap();
        assert_eq!(out.tool.as_deref(), Some("mcp:weather"));
        let tid = out.turn_id.expect("agent turn carries a trace id");
        let trace = trace_of(&s, tid);
        assert_eq!(trace.tool_label.as_deref(), Some("mcp:weather"));
        let steps = steps_of(&s, tid);
        let kinds: Vec<&str> = steps.iter().map(|(k, _)| k.as_str()).collect();
        // 云端循环 trace：输入 → 请求 1（tool_calls）→ 执行 → 请求 2（最终作答）。
        assert_eq!(
            kinds,
            ["user_input", "crisis_check", "tool_intent", "context_budget", "llm_round", "tool_call", "llm_round", "filter"]
        );
        assert_eq!(steps[2].1, "miss");
        assert_eq!(steps[4].1, "tool_calls");
        assert_eq!(steps[5].1, "ok");
        assert_eq!(steps[6].1, "answered");
        // 每轮请求的完整消息序列都存（spec-s8 D1），user_text 随步携带。
        with_conn(&s, |conn| {
            let t = db::turn_trace_get(conn, tid).unwrap().unwrap();
            let prompts: Vec<&str> = t
                .steps
                .iter()
                .filter(|st| st.step_kind == "llm_round")
                .map(|st| st.prompt.as_deref().expect("prompt stored"))
                .collect();
            assert_eq!(prompts.len(), 2);
            assert!(prompts[0].contains("今天杭州天气怎么样"), "user message in the request trace");
            assert!(prompts[0].contains("人格宪法"), "D6：云端循环带人格");
            assert!(prompts[1].contains("tool_calls"), "assistant(tool_calls) 回传在第二轮请求里");
            for st in &t.steps {
                assert_eq!(
                    st.user_text.as_deref(),
                    Some("今天杭州天气怎么样"),
                    "every step carries the user message: {}",
                    st.title
                );
            }
            let tool = t.steps.iter().find(|st| st.step_kind == "tool_call").unwrap();
            assert_eq!(tool.args_json.as_deref(), Some(r#"{"city":"杭州"}"#));
            assert!(tool.summary.as_deref().unwrap_or("").contains("多云"));
        });
    }


    #[test]
    fn trace_proactive_turn_has_no_user_side() {
        let s = mem();
        let c = card();
        let mut live = MockBackend::new("今天剪贴板挺忙。");
        let (text, tid) = proactive_text(&s, &mut live, &c, 10, false).unwrap().unwrap();
        assert!(!text.is_empty());
        let trace = trace_of(&s, tid);
        assert_eq!(trace.kind, "proactive");
        assert!(trace.user_event_id.is_none(), "proactive has no user side");
        assert!(trace.assistant_event_id.is_some());
        let steps = steps_of(&s, tid);
        let kinds: Vec<&str> = steps.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["llm_round"]);
    }
