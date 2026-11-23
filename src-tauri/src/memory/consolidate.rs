//! Nightly consolidation job v2 (plan-memory.md §5 — report §4.3).
//!
//! Pipeline per run, single transaction, idempotent by construction:
//!   ② extract    window's unconsumed events → candidate facts (extractor)
//!   ③ merge      normalized duplicates bump confidence (plan §5.1)
//!   ④ contradict correction candidates stamp `invalid_at` on the old fact;
//!                targets resolve at APPLY time so same-run facts count
//!   ⑤ promote    confidence ≥ 0.8 → preference (idempotent via dedup guard);
//!                stale low-confidence facts are invalidated (defensive sweep)
//!   finish       mark window events consumed; write the audit row
//!
//! Stage ⑥ (outside the transaction, plan-context §6.5): L2 digest rewrite for
//! the sessions touched yesterday — **默认关**（`settings.ctx_digest_model`），
//! 失败保留 L1 文本。
//!
//! Stage ① (session summaries) is deliberately absent in P0 — see the plan's
//! open-question recommendation.
//!
//! Double idempotency (plan §5.4): events carry `consumed` and the audit row
//! records the window, so a retried run can never double-extract. On any
//! error the transaction rolls back: no loss, no duplication.

use rusqlite::Connection;
use serde::Serialize;

use super::db;
use super::extract::{CandidateFact, Category, FactExtractor, RuleExtractor, SessionDigest};

/// L2 摘要重写（plan-context §6.5）：默认关。值只认 `local`，其它一律当关。
pub const SETTING_DIGEST_MODEL: &str = "ctx_digest_model";
/// 一次夜间最多重写几个会话（§6.5：最多 3 个）。
pub const DIGEST_L2_MAX_SESSIONS: usize = 3;
/// L2 输出上限（字）：超了就截，不重试。
pub const DIGEST_L2_MAX_CHARS: usize = 300;
/// L2 单次推理超时（秒）：2B 冷加载也不该要更久，超了就留 L1 文本。
pub const DIGEST_L2_TIMEOUT_SECS: u64 = 10;

/// L2 的结果。`enabled=false` 表示开关没开（不是失败）。
#[derive(Debug, Default, serde::Serialize)]
pub struct DigestReport {
    pub enabled: bool,
    pub rewritten: usize,
    pub skipped: usize,
}

const DIGEST_L2_PROMPT: &str = r#"你是对话摘要压缩器。把用户给的「较早对话摘要」压成一段不超过 300 字的纯文本：
- 只保留用户在做什么、关心什么、已经解决的事，以及还没定下来的事；
- 不评价、不寒暄、不加标题、不用列表、不用 markdown；
- 直接输出正文，不要说明你在做什么，也不要复述这条要求。"#;

/// Facts at/above this confidence are promoted into the preferences layer.
const PROMOTION_THRESHOLD: f64 = 0.8;
/// Merge bump applied when a candidate repeats an existing active fact.
const REPEAT_BUMP: f64 = 0.1;
/// Deletion sweep: confidence below this AND older than 30 days → invalidate.
const DELETE_MAX_CONFIDENCE: f64 = 0.5;
const DELETE_AGE_DAYS: i64 = 30;

#[derive(Debug, Serialize)]
pub struct Report {
    pub trigger: String,
    pub window_start: String,
    pub window_end: String,
    pub events_in_window: usize,
    pub extracted: usize,
    pub merged: usize,
    pub contradicted: usize,
    pub promoted: usize,
    /// q4 interception (plan-onboarding.md §3.2): user events whose content
    /// matches a "不准记录：" boundary — counted, never extracted.
    pub blocked: usize,
    pub deleted: usize,
    pub consumed: usize,
}

pub struct Options<'a> {
    pub trigger: &'a str, // 'nightly' | 'catchup' | 'manual'
    pub window_start: &'a str,
    pub window_end: &'a str,
}

/// Run one consolidation pass (direct-connection form: tests + eval
/// harness). Phase-split so the extractor's model round runs conn-less:
///   read (short) → extract (NO lock/tx) → write (one tx).
/// On a write-phase error the transaction rolls back: events stay
/// unconsumed, the next run re-extracts (double idempotency preserved).
pub fn run(
    conn: &mut Connection,
    extractor: &dyn FactExtractor,
    opts: &Options,
) -> rusqlite::Result<Report> {
    let (events, user_events, blocked, facts) = read_phase(conn, opts)?;
    let mut report = report_skeleton(opts, events.len(), blocked);
    let candidates = if events.is_empty() {
        Vec::new()
    } else {
        extractor.extract(&SessionDigest { user_utterances: &user_events, active_facts: &facts })
    };
    report.extracted = candidates.len();
    write_phase(conn, &mut report, &candidates)?;
    Ok(report)
}

/// Store-backed form (scheduler tick / manual command): the store lock is
/// held ONLY for the read and write phases — the extractor's LLM calls
/// (seconds to minutes) run with ZERO locks, so a catchup consolidation can
/// never freeze chat turns (2026-11-20 lock-narrowing, M-C).
pub fn run_with_store(
    store: &super::MemoryStore,
    extractor: &dyn FactExtractor,
    opts: &Options,
) -> rusqlite::Result<Report> {
    let (events, user_events, blocked, facts) = {
        let conn = lock_store(store)?;
        read_phase(&conn, opts)?
    };
    let mut report = report_skeleton(opts, events.len(), blocked);
    let candidates = if events.is_empty() {
        Vec::new()
    } else {
        extractor.extract(&SessionDigest { user_utterances: &user_events, active_facts: &facts })
    };
    report.extracted = candidates.len();
    let mut conn = lock_store(store)?;
    write_phase(&mut conn, &mut report, &candidates)?;
    Ok(report)
}

fn lock_store(store: &super::MemoryStore) -> rusqlite::Result<std::sync::MutexGuard<'_, Connection>> {
    store.0.lock().map_err(|_| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some("memory store mutex poisoned".into()),
        )
    })
}

/// Phase 1: read everything the run needs as OWNED data, so the lock can be
/// released before extraction.
fn read_phase(
    conn: &Connection,
    opts: &Options,
) -> rusqlite::Result<(Vec<db::EventRow>, Vec<db::EventRow>, usize, Vec<db::ActiveFact>)> {
    // Window events (unconsumed only — the second idempotency leg).
    let events = db::unconsumed_events(conn, opts.window_start, opts.window_end)?;

    // q4 interception (plan-onboarding.md §3.2): content matching a
    // "不准记录：" boundary stops at the extraction door. The raw event
    // row stays (it is ground truth, and the forget path needs it) — it
    // just never becomes a fact, preference, or boundary.
    let no_record = crate::onboarding::no_record_items(conn)?;
    let mut blocked = 0usize;
    let user_events: Vec<db::EventRow> = events
        .iter()
        .filter(|e| e.role.as_deref() != Some("assistant"))
        .filter(|e| {
            let hit = no_record.iter().any(|item| e.content.contains(item));
            if hit {
                blocked += 1;
            }
            !hit
        })
        .cloned()
        .collect();

    let facts = db::active_facts(conn)?;
    Ok((events, user_events, blocked, facts))
}

fn report_skeleton(opts: &Options, events_in_window: usize, blocked: usize) -> Report {
    Report {
        trigger: opts.trigger.to_string(),
        window_start: opts.window_start.to_string(),
        window_end: opts.window_end.to_string(),
        events_in_window,
        extracted: 0,
        merged: 0,
        contradicted: 0,
        promoted: 0,
        blocked,
        deleted: 0,
        consumed: 0,
    }
}

/// Phase 3: one transaction — apply candidates, deletion sweep, mark
/// consumed, audit row. `report` must be pre-filled (skeleton + extracted).
fn write_phase(
    conn: &mut Connection,
    report: &mut Report,
    candidates: &[CandidateFact],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    if !candidates.is_empty() {
        apply_candidates(&tx, candidates, report)?;
    }
    // Deletion sweep runs every pass, window or not (defensive hygiene).
    report.deleted = delete_stale(&tx)?;
    // Finish: mark consumed (only within the window — never touches events
    // outside it) and write the audit row.
    report.consumed = db::mark_events_consumed(&tx, &report.window_start, &report.window_end)?;
    write_audit(&tx, report)?;
    tx.commit()?; // on Err above, tx drops → rollback: events stay unconsumed
    Ok(())
}

/// Stages ③④⑤ applied to the candidate list, in event order.
fn apply_candidates(
    conn: &Connection,
    candidates: &[CandidateFact],
    report: &mut Report,
) -> rusqlite::Result<()> {
    // Local snapshot, refreshed as we insert — later candidates in the same
    // run can bump or correct what earlier ones created.
    let mut facts = db::active_facts(conn)?;

    for cand in candidates {
        // Stage ④: corrections resolve their target against the CURRENT
        // snapshot (extraction time only had the pre-run state).
        if cand.is_correction {
            if let Some(old) = super::extract::correction_target(&facts, &cand.content) {
                let old_id = old.id;
                report.contradicted += db::invalidate_fact(conn, old_id)?;
                facts.retain(|f| f.id != old_id);
                let id =
                    db::add_fact_confident(conn, &cand.content, cand.source_event_id, cand.confidence)?;
                facts.push(new_fact(id, cand));
            }
            continue; // correction with no target: 宁漏勿误, skip
        }

        // 画像出口（plan-user-profile.md §3.4）：带 profile 的 candidate 在
        // facts 照旧落库的同时写 user_profile——一份抽取两个出口。白名单/
        // manual 保护/历史归档都在 profile::apply 里，这里不重复实现。
        if let Some((key, value)) = &cand.profile {
            crate::memory::profile::apply(
                conn,
                key,
                value,
                cand.source,
                cand.source_event_id,
                cand.confidence,
            )?;
        }

        // Stage ③: merge. Normalized exact match bumps confidence instead of
        // inserting a duplicate.
        let norm = super::extract::normalize(&cand.content);
        let fact_id = match facts.iter_mut().find(|f| super::extract::normalize(&f.content) == norm) {
            Some(existing) => {
                db::bump_fact(conn, existing.id, REPEAT_BUMP)?;
                report.merged += 1;
                existing.confidence = ((existing.confidence + REPEAT_BUMP).min(0.95) * 100.0).round() / 100.0;
                existing.id
            }
            None => {
                let id =
                    db::add_fact_confident(conn, &cand.content, cand.source_event_id, cand.confidence)?;
                facts.push(new_fact(id, cand));
                id
            }
        };

        // Boundaries: straight to the boundaries table (plan §5.3), traced to
        // the fact for explainability; normalized dedup keeps the table clean.
        if cand.category == Category::Boundary {
            if !db::boundary_exists(conn, &cand.content)? {
                db::add_boundary(conn, &cand.content, Some(fact_id))?;
            }
            continue;
        }

        // Stage ⑤: promotion gate.
        let conf = facts
            .iter()
            .find(|f| f.id == fact_id)
            .map(|f| f.confidence)
            .unwrap_or(cand.confidence);
        if conf >= PROMOTION_THRESHOLD
            && db::promote_fact(conn, cand.category.as_str(), &cand.content, conf)?
        {
            report.promoted += 1;
        }
    }
    Ok(())
}

fn new_fact(id: i64, cand: &CandidateFact) -> db::ActiveFact {
    db::ActiveFact {
        id,
        ts: super::clock::now_iso(),
        content: cand.content.clone(),
        confidence: cand.confidence,
        importance: 3,
        source_event_id: cand.source_event_id,
    }
}

/// Stage ⑤ deletion sweep: stale low-confidence facts are invalidated, never
/// hard-deleted (system deletes stay soft — plan §3 invariant 2).
fn delete_stale(conn: &Connection) -> rusqlite::Result<usize> {
    let cutoff = chrono::DateTime::parse_from_rfc3339(&super::clock::now_iso())
        .map(|t| (t - chrono::Duration::days(DELETE_AGE_DAYS)).to_rfc3339())
        .unwrap_or_default();
    let ids = db::stale_low_confidence_facts(conn, &cutoff, DELETE_MAX_CONFIDENCE)?;
    let mut n = 0;
    for id in ids {
        n += db::invalidate_fact(conn, id)?;
    }
    Ok(n)
}

fn write_audit(conn: &Connection, report: &Report) -> rusqlite::Result<()> {
    let now = super::clock::now_iso();
    conn.execute(
        "INSERT INTO consolidate_runs
             (started_at, finished_at, trigger, window_start, window_end,
              extracted, merged, contradicted, promoted, deleted, consumed, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'done')",
        rusqlite::params![
            now,
            now, // started == finished for P0; a long-running LocalExtractor run may differ later
            report.trigger,
            report.window_start,
            report.window_end,
            report.extracted as i64,
            report.merged as i64,
            report.contradicted as i64,
            report.promoted as i64,
            report.deleted as i64,
            report.consumed as i64,
        ],
    )?;
    Ok(())
}

/// Default P0 pipeline entry: rule extractor.
pub fn run_with_rules(conn: &mut Connection, opts: &Options) -> rusqlite::Result<Report> {
    run(conn, &RuleExtractor, opts)
}

/// Model-first entry (plan §5.2): use `LocalExtractor` when the local LLM
/// is reachable, fall back to rules otherwise. One cheap probe decides —
/// a dead server must never cost the pipeline its guarantees.
/// Store-backed: locks per phase (see `run_with_store`).
pub fn run_with_local_or_rules(
    store: &super::MemoryStore,
    opts: &Options,
) -> rusqlite::Result<Report> {
    let local = super::extract::LocalExtractor::new();
    if local.client.is_up() {
        log::info!("consolidate: model leg = LocalExtractor ({})", local.client.label());
        run_with_store(store, &local, opts)
    } else {
        log::info!("consolidate: local LLM unreachable, falling back to RuleExtractor");
        run_with_store(store, &RuleExtractor, opts)
    }
}

/// L2：把 L1 的短行摘要重写成一段人话（§6.5）。
///
/// 三条边界：
/// - **默认关**：开关没开就直接返回，不读候选、不构造 prompt；
/// - **只在夜间/手动巩固之后跑**（调用方是 scheduler），不进热路径（D2）；
/// - **写回用 compare-and-set**：这几秒里 L1 可能又追加了新行，直接覆盖等于
///   把刚记住的东西吞掉。写不进去就算 skipped，L1 文本原样保留。
pub fn digest_pass(
    store: &super::MemoryStore,
    client: &dyn crate::llm::ChatClient,
) -> DigestReport {
    let mut report = DigestReport::default();

    let candidates = {
        let Ok(conn) = lock_store(store) else {
            return report;
        };
        match db::get_setting(&conn, SETTING_DIGEST_MODEL).ok().flatten().as_deref() {
            Some("local") => {}
            _ => return report, // 默认 off（D4）
        }
        report.enabled = true;
        db::session_digest_candidates(&conn, &digest_window_start(), DIGEST_L2_MAX_SESSIONS)
            .unwrap_or_default()
    };

    for row in candidates {
        let Some(old) = row.digest.as_deref().map(str::trim).filter(|d| !d.is_empty()) else {
            continue;
        };
        let Some(new) = rewrite_digest(client, old) else {
            report.skipped += 1;
            continue;
        };
        // 锁只在读候选与写回这两小段里持有，模型推理全程零锁（同巩固管线）。
        let written = lock_store(store)
            .ok()
            .and_then(|conn| db::session_set_digest_if(&conn, &row.id, old, &new).ok())
            .unwrap_or(0);
        if written > 0 {
            report.rewritten += 1;
        } else {
            report.skipped += 1; // L1 抢先写入 / 会话被删：保留库里的版本
        }
    }
    report
}

/// 「昨天有摘要更新」的近似：`updated_at` 在最近 24 小时内。
fn digest_window_start() -> String {
    let now = chrono::DateTime::parse_from_rfc3339(&super::clock::now_iso())
        .map(|t| t.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    (now - chrono::Duration::hours(24)).to_rfc3339()
}

/// 一次模型重写。任何失败（超时 / 空输出 / 连接不上）→ `None`，保留 L1 文本。
fn rewrite_digest(client: &dyn crate::llm::ChatClient, digest: &str) -> Option<String> {
    let reply = client
        .chat(
            &[
                crate::llm::Msg::system(DIGEST_L2_PROMPT),
                crate::llm::Msg::user(digest),
            ],
            0.2, // 摘要是「复述」不是「创作」
            DIGEST_L2_TIMEOUT_SECS,
        )
        .ok()?;
    let text = reply
        .trim()
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if text.is_empty() {
        return None;
    }
    Some(crate::context::clip_chars(text, DIGEST_L2_MAX_CHARS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;
    use rusqlite::Connection;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    const START: &str = "2026-11-17T00:00:00Z";
    const END: &str = "2026-11-19T00:00:00Z"; // events land 2026-11-18T12:00Z, strictly inside

    fn say(conn: &Connection, content: &str) -> i64 {
        db::add_event_meta(conn, "chat", content, Some("user"), Some("s1")).unwrap()
    }

    fn nightly() -> Options<'static> {
        Options { trigger: "nightly", window_start: START, window_end: END }
    }

    // ------------------------------------------------------ L2 digest（§6.5）

    struct FakeClient {
        reply: Option<&'static str>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeClient {
        fn new(reply: Option<&'static str>) -> Self {
            FakeClient { reply, calls: std::sync::atomic::AtomicUsize::new(0) }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl crate::llm::ChatClient for FakeClient {
        fn chat(
            &self,
            _messages: &[crate::llm::Msg],
            _temperature: f64,
            _timeout: u64,
        ) -> Result<String, crate::llm::LlmError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.reply
                .map(|s| s.to_string())
                .ok_or_else(|| crate::llm::LlmError::Decode("fake: no reply".into()))
        }
        fn chat_stream(
            &self,
            messages: &[crate::llm::Msg],
            temperature: f64,
            timeout: u64,
            _on_delta: &mut dyn FnMut(&str),
        ) -> Result<String, crate::llm::LlmError> {
            self.chat(messages, temperature, timeout)
        }
        fn is_up(&self) -> bool {
            true
        }
    }

    /// 一个带摘要的会话，`updated_at` 落在 L2 的 24h 窗口内。
    fn store_with_digest(digest: &str) -> (crate::memory::MemoryStore, String) {
        let conn = mem();
        db::session_create(&conn, "s1", "封窗报价").unwrap();
        db::session_set_digest(&conn, "s1", digest, 12).unwrap();
        (crate::memory::MemoryStore(std::sync::Mutex::new(conn)), "s1".to_string())
    }

    fn digest_of(store: &crate::memory::MemoryStore, id: &str) -> Option<String> {
        let conn = store.0.lock().unwrap();
        db::session_digest(&conn, id).unwrap().0
    }

    #[test]
    fn digest_pass_is_off_unless_the_setting_says_local() {
        let _g = clock::pinned("2026-11-23T22:00:00Z");
        let (store, id) = store_with_digest("- [11-22] 问报价 → 给了估算");
        let client = FakeClient::new(Some("用户在看封窗报价。"));

        let report = digest_pass(&store, &client);

        assert!(!report.enabled, "默认关（D4）");
        assert_eq!(client.calls(), 0, "关着就不该花一次推理");
        assert_eq!(digest_of(&store, &id).as_deref(), Some("- [11-22] 问报价 → 给了估算"));
    }

    #[test]
    fn digest_pass_rewrites_touched_sessions_when_enabled() {
        let _g = clock::pinned("2026-11-23T22:00:00Z");
        let (store, id) = store_with_digest("- [11-22] 问报价 → 给了估算");
        {
            let conn = store.0.lock().unwrap();
            db::set_setting(&conn, SETTING_DIGEST_MODEL, "local").unwrap();
        }
        let client = FakeClient::new(Some("  用户在比价封窗，倾向先做客厅阳台。  "));

        let report = digest_pass(&store, &client);

        assert!(report.enabled);
        assert_eq!(report.rewritten, 1);
        assert_eq!(report.skipped, 0);
        assert_eq!(
            digest_of(&store, &id).as_deref(),
            Some("用户在比价封窗，倾向先做客厅阳台。"),
            "L2 覆盖 L1，前后空白去掉"
        );
    }

    #[test]
    fn digest_pass_keeps_the_l1_text_when_the_model_fails() {
        let _g = clock::pinned("2026-11-23T22:00:00Z");
        let (store, id) = store_with_digest("- [11-22] 问报价 → 给了估算");
        {
            let conn = store.0.lock().unwrap();
            db::set_setting(&conn, SETTING_DIGEST_MODEL, "local").unwrap();
        }
        let client = FakeClient::new(None); // 超时 / 连不上

        let report = digest_pass(&store, &client);

        assert_eq!(report.rewritten, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(
            digest_of(&store, &id).as_deref(),
            Some("- [11-22] 问报价 → 给了估算"),
            "失败就留 L1，不重试、不清空"
        );
    }

    #[test]
    fn digest_pass_clips_a_runaway_rewrite() {
        let _g = clock::pinned("2026-11-23T22:00:00Z");
        let (store, id) = store_with_digest("- [11-22] 问报价 → 给了估算");
        {
            let conn = store.0.lock().unwrap();
            db::set_setting(&conn, SETTING_DIGEST_MODEL, "local").unwrap();
        }
        let long: &'static str = Box::leak("很长的一句话。".repeat(80).into_boxed_str());
        let client = FakeClient::new(Some(long));

        let report = digest_pass(&store, &client);

        assert_eq!(report.rewritten, 1);
        let text = digest_of(&store, &id).expect("摘要应当被写回");
        assert!(
            text.chars().count() <= DIGEST_L2_MAX_CHARS,
            "L2 输出上限 300 字：{}",
            text.chars().count()
        );
    }

    #[test]
    fn extracts_merges_contradicts_and_consumes() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();

        say(&conn, "我喜欢雨天");
        say(&conn, "我每天散步半小时");
        say(&conn, "我喜欢雨天。"); // punctuation differs → merge
        say(&conn, "说错了，其实是我不喜欢雨天"); // correction targets the rainy fact

        let report = run_with_rules(&mut conn, &nightly()).unwrap();
        assert_eq!(report.extracted, 4);
        assert_eq!(report.merged, 1);
        assert_eq!(report.contradicted, 1);
        assert_eq!(report.consumed, 4);

        // Re-running the same window is a no-op (double idempotency).
        let again = run_with_rules(
            &mut conn,
            &Options { trigger: "catchup", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(again.extracted, 0);
        assert_eq!(again.consumed, 0);
        assert_eq!(again.events_in_window, 0);
    }

    #[test]
    fn promotion_after_second_mention_is_idempotent() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();

        say(&conn, "我喜欢雨天");
        let r1 = run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(r1.promoted, 0); // 0.7 < 0.8 — not confident enough yet

        say(&conn, "我喜欢雨天");
        let r2 = run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(r2.merged, 1); // 0.7 + 0.1 = 0.8
        assert_eq!(r2.promoted, 1);

        let prefs: i64 = conn
            .query_row("SELECT COUNT(*) FROM preferences WHERE category = 'like'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(prefs, 1); // promote_fact dedup guard held
    }

    #[test]
    fn boundary_goes_straight_to_boundaries_table() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        say(&conn, "别在我专注的时候弹窗");
        say(&conn, "别在我专注的时候弹窗"); // duplicate phrasing → dedup

        let report = run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(report.extracted, 2);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM boundaries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // Boundary rows trace back to a fact (traceability invariant).
        let linked: i64 = conn
            .query_row("SELECT COUNT(*) FROM boundaries WHERE source_fact_id IS NOT NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(linked, 1);
    }

    #[test]
    fn deletion_sweep_invalidates_stale_low_confidence() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        let old_id = db::add_fact_confident(&conn, "stale guess", None, 0.4).unwrap();
        conn.execute("UPDATE facts SET ts = '2026-10-01T00:00:00Z' WHERE id = ?1", [old_id])
            .unwrap();

        let report = run_with_rules(&mut conn, &nightly()).unwrap(); // empty window, sweep still runs
        assert_eq!(report.deleted, 1);
        let active: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts WHERE invalid_at IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(active, 0);
    }

    #[test]
    fn q4_no_record_items_stop_at_the_extraction_door() {
        use crate::onboarding::{apply_answers, SubmitAnswer};

        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        apply_answers(
            &conn,
            &[SubmitAnswer {
                question: "q4".into(),
                answer: "体检报告".into(),
                skipped: false,
            }],
        )
        .unwrap();

        say(&conn, "我喜欢雨天");
        say(&conn, "今天体检报告显示一切正常");
        let report = run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(report.blocked, 1, "the health-report event is counted, not extracted");
        assert_eq!(report.extracted, 1, "only the rainy-day event becomes a candidate");
        let facts: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts WHERE content LIKE '%体检%'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(facts, 0, "no-record content must never become a fact");
        // The raw event stays (ground truth + forget path need it).
        let raw: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE content LIKE '%体检%'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, 1);
    }

    // ---- 用户画像（plan-user-profile.md §3）----

    #[test]
    fn profile_candidates_write_both_facts_and_user_profile() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        say(&conn, "我叫许一");
        say(&conn, "我在用 Ollama");
        run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();

        // 出口一：facts 照旧落库（事件性留痕）。
        let facts: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(facts, 2);
        // 出口二：画像落库。
        let rows = crate::memory::profile::get_all(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "name");
        assert_eq!(rows[0].value, "许一");
        assert_eq!(rows[0].source, "rule");
        assert_eq!(rows[1].key, "tools");
    }

    #[test]
    fn q4_interception_covers_profile_extraction() {
        use crate::onboarding::{apply_answers, SubmitAnswer};

        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        apply_answers(
            &conn,
            &[SubmitAnswer {
                question: "q4".into(),
                answer: "体检报告".into(),
                skipped: false,
            }],
        )
        .unwrap();

        say(&conn, "我叫许一，上周体检报告有问题");
        run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        let profile: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_profile", [], |r| r.get(0))
            .unwrap();
        assert_eq!(profile, 0, "不准记录的内容连画像也不许进");
    }

    #[test]
    fn manual_profile_survives_consolidate_extraction() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        // 用户先在工具页亲口写了称呼。
        crate::memory::profile::apply(&conn, "name", "Jack", "manual", None, 1.0).unwrap();
        say(&conn, "我叫许一");
        run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        let rows = crate::memory::profile::get_all(&conn).unwrap();
        assert_eq!(rows[0].value, "Jack", "manual 值不被自动抽取覆盖");
        // facts 侧不受影响——两个出口是独立的。
        let facts: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(facts, 1);
    }

    #[test]
    fn error_rolls_back_and_keeps_events_unconsumed() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        say(&conn, "我喜欢雨天");

        // Candidate references a non-existent event → FK violation mid-run.
        struct BadSource;
        impl FactExtractor for BadSource {
            fn extract(&self, _d: &SessionDigest) -> Vec<CandidateFact> {
                vec![CandidateFact {
                    content: "ghost fact".into(),
                    category: Category::Like,
                    confidence: 0.7,
                    source_event_id: Some(999_999),
                    is_correction: false,
                    profile: None,
                    source: "rule",
                }]
            }
        }

        let result = run(&mut conn, &BadSource, &nightly());
        assert!(result.is_err());

        // Nothing committed: event still pending, no facts, no audit rows.
        let pending = db::unconsumed_events(&conn, START, END).unwrap();
        assert_eq!(pending.len(), 1);
        let facts: i64 = conn.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0)).unwrap();
        assert_eq!(facts, 0);
        let audits: i64 =
            conn.query_row("SELECT COUNT(*) FROM consolidate_runs", [], |r| r.get(0)).unwrap();
        assert_eq!(audits, 0);
    }

    /// An extractor that fakes a slow model round.
    struct SlowExtractor {
        ms: u64,
    }
    impl FactExtractor for SlowExtractor {
        fn extract(&self, _d: &SessionDigest) -> Vec<CandidateFact> {
            std::thread::sleep(std::time::Duration::from_millis(self.ms));
            Vec::new()
        }
    }

    /// M-C regression: the extraction's model round must run with ZERO store
    /// locks — a catchup consolidate over minutes of events used to freeze
    /// every chat turn racing for the store.
    #[test]
    fn consolidate_extraction_holds_no_store_lock() {
        use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
        static SEQ3: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join("bonsai-consolidate-test");
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(super::super::MemoryStore::open(
            &dir.join(format!("lk-{}-{}.sqlite3", std::process::id(), SEQ3.fetch_add(1, AtomicOrdering::SeqCst))),
        )
        .unwrap());

        // Seed one event so the run has a window to read.
        {
            let conn = store.0.lock().unwrap();
            let _guard = clock::pinned("2026-11-18T12:00:00Z");
            db::add_event_meta(&conn, "chat", "我喜欢雨天", Some("user"), Some("s1")).unwrap();
        }

        let (tx, rx) = std::sync::mpsc::channel::<std::time::Instant>();
        let bg = store.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            loop {
                if let Ok(conn) = bg.0.try_lock() {
                    let _ = db::add_event(&conn, "face_state", "bg-marker");
                    let _ = tx.send(std::time::Instant::now());
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let start = std::time::Instant::now();
        let opts = Options { trigger: "manual", window_start: START, window_end: END };
        let report = run_with_store(&store, &SlowExtractor { ms: 800 }, &opts).unwrap();
        assert_eq!(report.events_in_window, 1);
        let turn_end = start.elapsed();
        let acquired = rx.recv().unwrap() - start;
        assert!(
            acquired < turn_end - std::time::Duration::from_millis(200),
            "store lock must be acquirable MID-extraction: bg acquired at {acquired:?}, run lasted {turn_end:?}"
        );
    }
}
