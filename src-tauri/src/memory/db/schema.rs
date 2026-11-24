//! Schema, open/migrate, SCHEMA_VERSION (memory/db split).
//!
//! Plans: docs/zh/plan-memory.md §3–§4 and schema changelog on SCHEMA_VERSION.

use rusqlite::{params, Connection};

use crate::memory::clock;

/// SCHEMA_VERSION = 18（2026-11-24）：Reliability Phase 1
/// （docs/zh/plan-reliability-phase1/v1）——
/// `memory_evidence`（派生记忆统一溯源）+ `proactive_daily_usage`（主动搭话
/// 本地日配额）。Forget Session/Event 经 evidence 级联；无源历史 preference
/// 不假装已溯源。
/// SCHEMA_VERSION = 17（2026-11-24）：失败观测列（docs/zh/plan-retry.md §6）——
/// turn_trace 加 `error_kind`（这一轮为什么失败，可 GROUP BY 出失败分布）
/// 与 `attempts`（重试了几次，含成功前的失败次数）；consolidate_runs 加
/// `error`（夜间作业失败原因，status='error' 从此有话可说）。
/// settings 新 key `job_backoff:*` 存退避状态，不建表。
/// SCHEMA_VERSION = 16（2026-11-23）：检索索引（S2，见 `memory/index.rs`）——
/// `search_docs`（自建 2-gram 的检索文档）+ `search_fts`（FTS5 unicode61，
/// 外部内容指向 search_docs）+ `search_sync`（增量水位线）+ 源表删除级联
/// 触发器。**不用 trigram**：实测对 2 字中文零命中，而中文检索单位就是 2 字词。
///
/// SCHEMA_VERSION = 15（2026-11-23）：上下文管理（docs/zh/plan-context.md §5）——
/// `sessions`（会话索引：标题/时间/归档/摘要指针）+ `events(session_id,id)`
/// 索引（多会话后每个会话都要查，从「可有可无」变成「必须有」）+
/// `turn_trace` 三列观测（估算/实际 prompt tokens、被丢弃轮数）+
/// `tasks.session_id`（遗忘按会话级联时用来定位任务行）。
///
/// SCHEMA_VERSION = 14（2026-11-23）：规划能力（docs/zh/plan-planning.md §4）——
/// `tasks` 增四列：`plan_json`（结构化计划 JSON）、`plan_mode`（none/planned）、
/// `cancel_reason`（user/budget）、`confirm_mode`（auto/manual，D1 默认 auto 直放）。
/// 不新建 plans/plan_steps 表——步骤明细仍由 tool_calls.task_id 承载，
/// 进度状态只存 plan_json + 从 tool_calls 派生，不双写。
///
/// SCHEMA_VERSION = 13（2026-11-23）：用户画像（docs/zh/plan-user-profile.md §4）——
/// `user_profile`（身份 KV，key 白名单在 memory/profile.rs）+
/// `user_profile_history`（被覆盖/删除的旧值归档）。画像 ≠ 事实：facts 是
/// 事件性留痕，画像是可覆盖的身份快照，每轮固定注入 system prompt。
///
/// SCHEMA_VERSION = 12（2026-11-22）：任务卡外壳（plan-task-card v0.2 §8）——
/// `tasks` 表（一次云端工具循环一行）+ `tool_calls.task_id` 关联列。
/// 审计行不进 ACTIVITY_KINDS（自己陪自己防线的延伸）。
/// SCHEMA_VERSION = 11（2026-11-21）：trace_steps 补 user_text 列——当前
/// 输入作为 user message 传给模型（不在 system prompt 里），step 级 trace
/// 此前看不到它（turn 级 turn_trace.user_text 一直有）。
/// SCHEMA_VERSION = 10（2026-11-21）：turn_trace 补 user_text/reply_text 列。
/// 上一版 v9 build 已在旧库建过 turn_trace（无这两列），CREATE TABLE IF NOT
/// EXISTS 不会给已有表加列；v9→v9 不触发 migrate，INSERT 带 user_text 直接
/// 报 "table turn_trace has no column named user_text"。教训：改 SCHEMA 必
/// 须同时 bump 此版本号。
pub(crate) const SCHEMA_VERSION: i64 = 18;

/// 不进会话列表的保留 session id：主动搭话（bubble 照旧实时显示，但不占
/// 用户能切换的对话窗口）。sessions 里**永不建行**（plan-context D6）。
/// 唯一定义在这里，dialogue 侧以 `PROACTIVE_SESSION` 别名引用。
pub const PROACTIVE_SESSION_ID: &str = "proactive";

/// Test-only: materialise the full schema on a bare in-memory connection.
/// Other modules' tests build their own `Connection::open_in_memory()` and
/// need the schema without going through `open()` (which wants a path).
#[cfg(test)]
pub fn init_schema_for_test(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

/// Writer / migrator connection (plan-store-lock T1)：WAL + FK + busy_timeout。
/// `busy_timeout` 管跨连接文件锁（读池 vs 写者），不是进程内 Mutex 的替代。
pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    apply_common_pragmas(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// 只读旁路连接（plan-store-lock T3）：同路径、同 pragma，另开 `query_only`，
/// **不**跑 migrate（migrate 只属于 writer）。
pub fn open_reader(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    apply_common_pragmas(&conn)?;
    // SQLite 3.8+：阻止这条连接上的写；误写在 API 层失败而非脏写。
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}

fn apply_common_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // 5s：与 plan-store-lock §6.4 / A2 对齐。
    conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
    Ok(())
}

/// Full schema for a fresh database (v2).
pub(crate) const SCHEMA: &str = r#"
-- Layer 1: raw events. The ground truth everything else traces back to.
CREATE TABLE IF NOT EXISTS events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         TEXT NOT NULL,
    kind       TEXT NOT NULL,
    content    TEXT NOT NULL,
    role       TEXT,                        -- 'user' | 'assistant' | 'system'; NULL = non-dialogue event
    session_id TEXT,                        -- same dialogue window share one id
    consumed   INTEGER NOT NULL DEFAULT 0   -- consumed by a consolidate run
);

-- Conversation windows (plan-context §5.1, v15). 会话只是「对话窗口的索引」：
-- 记忆（facts / preferences / 画像）仍然全局，切会话不切记忆。
-- `title` 为空串 = 还没被首条用户消息命名（前端显示「新对话」）；这样
-- 自动命名不需要中文字符串哨兵，也不会覆盖用户手动改过的标题。
-- `digest` / `digest_upto_event_id` 是 L1/L2 摘要的落点（T6）：
-- upto = 已合并进 digest 的最大 events.id，防重复合并。
-- `proactive` 不在此表建行（D6）。
CREATE TABLE IF NOT EXISTS sessions (
    id                   TEXT PRIMARY KEY,          -- 'default' | uuid
    title                TEXT NOT NULL DEFAULT '',  -- 首条用户消息前 24 字；'' = 未命名
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL,             -- 每次落对话事件 touch，列表按它倒序
    archived             INTEGER NOT NULL DEFAULT 0,
    digest               TEXT,
    digest_upto_event_id INTEGER,
    message_count        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(archived, updated_at DESC);

-- Layer 2: extracted facts. Superseded facts are stamped invalid_at (never
-- hard-deleted by the system; hard delete only on user request).
CREATE TABLE IF NOT EXISTS facts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              TEXT NOT NULL,
    content         TEXT NOT NULL,
    source_event_id INTEGER REFERENCES events(id),
    invalid_at      TEXT,
    confidence      REAL NOT NULL DEFAULT 0.6,  -- rises on repeated mention, gate for preference promotion
    importance      INTEGER NOT NULL DEFAULT 3  -- 1..5; P0 keeps the column only (plan: no logic yet)
);

-- Layer 3a: user preferences (report §4.1: four categories).
CREATE TABLE IF NOT EXISTS preferences (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         TEXT NOT NULL,
    category   TEXT NOT NULL,               -- 'like' | 'habit' | 'context' | 'boundary'
    content    TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    invalid_at TEXT
);

-- Layer 3b: boundaries — "what it must never do/say". Most valuable kind:
-- respect > recording. Exempt from time decay in retrieval.
CREATE TABLE IF NOT EXISTS boundaries (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    ts             TEXT NOT NULL,
    content        TEXT NOT NULL,
    source_fact_id INTEGER REFERENCES facts(id)
);

-- Consolidation audit (plan §4): what ran, over which window, what it did.
CREATE TABLE IF NOT EXISTS consolidate_runs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    trigger       TEXT NOT NULL,   -- 'nightly' | 'catchup' | 'manual'
    window_start  TEXT NOT NULL,
    window_end    TEXT NOT NULL,
    extracted     INTEGER NOT NULL DEFAULT 0,
    merged        INTEGER NOT NULL DEFAULT 0,
    contradicted  INTEGER NOT NULL DEFAULT 0,
    promoted      INTEGER NOT NULL DEFAULT 0,
    deleted       INTEGER NOT NULL DEFAULT 0,
    consumed      INTEGER NOT NULL DEFAULT 0,
    status        TEXT NOT NULL DEFAULT 'running',  -- 'running' | 'done' | 'failed' | 'error'
    error         TEXT            -- v17：失败原因（plan-retry §6；成功行 NULL）
);

-- Predict-verify loop audit (plan-predict.md §3.4). One row per LOCAL day
-- max (UNIQUE(date)); the row is created on the first scheduling decision of
-- the day and later mutated only by ADDING feedback / asked_at — history is
-- never rewritten (plan invariant 1).
CREATE TABLE IF NOT EXISTS predictions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    date           TEXT NOT NULL UNIQUE,    -- local calendar day YYYY-MM-DD
    guess          TEXT NOT NULL,
    basis          TEXT NOT NULL,           -- JSON: [{"type":"preference"|"event","id":N}]
    ask_after_hour INTEGER NOT NULL,        -- intended delivery hour (local)
    asked_at       TEXT,                    -- actual delivery; NULL = not asked
    skip_reason    TEXT,                    -- 'no_candidate'|'busy'|'spoke_today'|'backoff'
    feedback       TEXT,                    -- 'hit'|'miss'|'shut_up' (NULL = no reply yet)
    feedback_at    TEXT,
    resolved_at    TEXT                     -- 48h window close (auto-resolve)
);

-- Form-line growth (plan-morph.md §3.2). Single-row state (id = 1):
-- born_at anchors the day count (backfilled from the earliest event so
-- existing users don't restart), stage only ever increases (invariant 1),
-- last_tick_date makes the daily advance idempotent, dormant_since is the
-- reversible "sleeping" marker (report §6.3 decision 1: no death, only rest).
CREATE TABLE IF NOT EXISTS growth (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    born_at        TEXT NOT NULL,
    stage          INTEGER NOT NULL DEFAULT 0,
    last_tick_date TEXT,
    dormant_since  TEXT
);

-- Growth audit: one row per stage unlock. notified_at makes the
-- window-open announcement idempotent (announced exactly once).
CREATE TABLE IF NOT EXISTS growth_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    stage       INTEGER NOT NULL,
    unlocked_at TEXT NOT NULL,
    notified_at TEXT,
    note        TEXT
);

-- Onboarding (plan-onboarding.md §3.1). `settings` is a tiny KV for
-- app-level state (onboarding_done / age_group / crisis_ack). The answers
-- table is the audit trail — SKIPPED answers are recorded too (skipped=1
-- is itself a signal for tuning the questions later).
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS onboarding_answers (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    question TEXT NOT NULL,                 -- 'q1'..'q5' | 'age' | 'crisis'
    answer   TEXT NOT NULL,
    skipped  INTEGER NOT NULL DEFAULT 0,    -- 1 = user skipped
    ts       TEXT NOT NULL
);

-- Relationship line (plan-relationship.md §3.3). Single-row state (id = 1).
-- No born_at here: the day count reuses growth.born_at so the two timelines
-- share one origin. stage only ever increases; cold_since is the reversible
-- "rusty" marker (never rewinds the stage).
CREATE TABLE IF NOT EXISTS relationship (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    stage          INTEGER NOT NULL DEFAULT 0,   -- 0 拘谨 | 1 熟络 | 2 会顶嘴
    stage_since    TEXT,                          -- current stage start (7-day "first refusal" window)
    cold_since     TEXT,                          -- NULL = warm
    last_tick_date TEXT                           -- makes the daily advance idempotent
);

-- Daily evidence, RECOMPUTED not accumulated: one UPSERT per day derived from
-- predictions + events. Same source data → same score (replayable, testable).
-- The numeric score is internal only — it never leaves this process.
CREATE TABLE IF NOT EXISTS relationship_progress (
    date          TEXT PRIMARY KEY,               -- local day YYYY-MM-DD
    present       INTEGER NOT NULL DEFAULT 0,
    hits          INTEGER NOT NULL DEFAULT 0,
    misses        INTEGER NOT NULL DEFAULT 0,
    disagreements INTEGER NOT NULL DEFAULT 0,
    shut_ups      INTEGER NOT NULL DEFAULT 0,
    score         INTEGER NOT NULL DEFAULT 0,
    computed_at   TEXT NOT NULL
);

-- Relationship audit: one row per stage unlock (notified_at = announced once).
-- `kind` separates stage promotions from the one-off "first refusal" moment.
CREATE TABLE IF NOT EXISTS relationship_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL DEFAULT 'stage',   -- 'stage' | 'first_refusal'
    stage       INTEGER NOT NULL,
    unlocked_at TEXT NOT NULL,
    notified_at TEXT,
    note        TEXT
);

CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_unconsumed ON events(consumed, ts);
-- v15：按会话取历史（dialogue_history / 回放 / 遗忘）从「扫 ts 再过滤」
-- 变成区间扫描；多会话之后每个会话都要查，这条索引是硬需求不是装饰。
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id, id);
CREATE INDEX IF NOT EXISTS idx_facts_active ON facts(ts) WHERE invalid_at IS NULL;

-- 用户画像（plan-user-profile.md §4，v13）。身份 KV，key 白名单在
-- memory/profile.rs；覆盖/删除的旧值进 user_profile_history，不静默消失。
-- 与 facts 并存不重复：facts 留事件痕，本表是每轮固定注入的身份快照。
CREATE TABLE IF NOT EXISTS user_profile (
    key             TEXT PRIMARY KEY,           -- 白名单枚举：name/occupation/tools/city/projects/schedule
    value           TEXT NOT NULL,
    source          TEXT NOT NULL,              -- 'rule' | 'model' | 'manual'（manual 不被自动抽取覆盖）
    source_event_id INTEGER REFERENCES events(id),
    confidence      REAL NOT NULL DEFAULT 0.7,
    updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS user_profile_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    source      TEXT NOT NULL,
    valid_until TEXT NOT NULL                   -- 被替换/删除的时间
);

-- Calendar read cache (plan-calendar.md §3.2). PURE CACHE: whole table is
-- disposable — TTL 5 min, cleared on switch-off / permission loss / forget.
-- Titles live here and ONLY here: they never reach events or facts.
CREATE TABLE IF NOT EXISTS calendar_cache (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    external_id    TEXT NOT NULL,                -- EKEvent.eventIdentifier
    start_at       TEXT NOT NULL,                -- RFC3339 UTC (same clock as events.ts)
    end_at         TEXT NOT NULL,
    title          TEXT NOT NULL,
    calendar_name  TEXT,
    all_day        INTEGER NOT NULL DEFAULT 0,
    attendee_count INTEGER NOT NULL DEFAULT 0,
    declined       INTEGER NOT NULL DEFAULT 0,   -- the current user declined it
    canceled       INTEGER NOT NULL DEFAULT 0,
    availability   TEXT,                         -- 'busy' | 'free' | 'tentative' | 'unavailable'
    reminded_at    TEXT,                         -- one reminder per occurrence
    fetched_at     TEXT NOT NULL,
    -- Recurring instances share an eventIdentifier, so the start time is part
    -- of the identity — without it every occurrence overwrites the last one.
    UNIQUE(external_id, start_at)
);

-- "What did it do" audit (plan-calendar.md §3.2): action + outcome + timing
-- only. No titles, no bodies — this table is safe to show the user.
CREATE TABLE IF NOT EXISTS tool_calls (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL,
    tool        TEXT NOT NULL,     -- 'calendar' | 'mail' | 'mcp' | 'skill'
    action      TEXT NOT NULL,     -- 'agenda' | 'next' | 'search' | 'remind' | 'digest' | <descriptor name>
    outcome     TEXT NOT NULL,     -- 'ok' | 'denied' | 'unavailable' | 'error'
    detail      TEXT,              -- counts / latency, never content
    duration_ms INTEGER,
    -- agent-pipeline audit columns (spec-s4; v7→v8 upgrades get them via
    -- add_column_if_missing — fresh DBs get them here).
    params_summary TEXT,
    result_status  TEXT,
    prompt_trace   TEXT,
    remote         INTEGER DEFAULT 0,
    -- 任务卡外壳（v12）：本行属于哪次云端工具循环（tasks.id，TEXT uuid）
    task_id        TEXT
);

-- 任务卡外壳（plan-task-card v0.2 §8，v12）：一次云端工具循环一行；
-- 步骤明细在 tool_calls（task_id 关联）。审计不进 ACTIVITY_KINDS。
CREATE TABLE IF NOT EXISTS tasks (
    id            TEXT PRIMARY KEY,   -- uuid
    created_at    TEXT NOT NULL,      -- clock::now_iso()
    goal          TEXT NOT NULL,      -- 用户原话前 80 字（人工可读）
    status        TEXT NOT NULL,      -- running / done / failed / canceled
    tool_calls    INTEGER NOT NULL DEFAULT 0,
    final_reply   TEXT,
    -- 规划能力（v14，docs/zh/plan-planning.md §4）：NULL = 单步任务，无额外开销
    plan_json     TEXT,                         -- 结构化计划 JSON（steps[] 状态）
    plan_mode     TEXT NOT NULL DEFAULT 'none', -- 'none' | 'planned'
    cancel_reason TEXT,                         -- 'user' | 'budget' | NULL
    confirm_mode  TEXT NOT NULL DEFAULT 'auto', -- D1: 'auto' 直放 | 'manual' 扩展位（本版不阻塞）
    -- 会话归属（v15）：forget_session 要按会话连带清掉任务行（goal 存的是用户原话）
    session_id    TEXT
);

-- MCP servers (plan-agent-mcp-skill.md §3.2, spec-s2). Secrets in
-- headers_json/env_json are plaintext at rest (local SQLite, same policy as
-- cloud keys); every read path handed to the frontend must mask them.
CREATE TABLE IF NOT EXISTS mcp_servers (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    transport     TEXT NOT NULL DEFAULT 'stdio',  -- 'stdio' | 'http'
    command       TEXT,                           -- stdio: executable path (NULL for http)
    args_json     TEXT NOT NULL DEFAULT '[]',
    url           TEXT,                           -- http: remote endpoint (NULL for stdio)
    headers_json  TEXT NOT NULL DEFAULT '{}',
    env_json      TEXT NOT NULL DEFAULT '{}',
    enabled       INTEGER NOT NULL DEFAULT 0,     -- new servers start disabled (RK-04)
    tool_cache    TEXT,
    tool_cache_at TEXT,
    created_at    TEXT NOT NULL
);

-- Turn trace (spec-s8): how one reply was produced. Sidecar record —
-- NEVER part of ACTIVITY_KINDS and never feeds morph/relationship/growth:
-- tracing our own work must not count as "being talked to".
-- D1 (2026-11-20): prompts/outputs stored FULL TEXT, no pruning, no TTL —
-- the trace is a debugging & trust product; truncating it destroys the
-- evidence. Storage cost is accepted on local SQLite.
CREATE TABLE IF NOT EXISTS turn_trace (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id         TEXT NOT NULL,
    user_event_id      INTEGER,        -- events.id of the user side (NULL for proactive)
    assistant_event_id INTEGER,        -- events.id of the assistant side
    kind               TEXT NOT NULL,  -- 'model' | 'tool' | 'crisis' | 'proactive'
    model_used         INTEGER NOT NULL DEFAULT 0,
    tool_label         TEXT,           -- 'builtin:x' | 'mcp:x' | 'skill:x' | 'calendar:action'
    filter_report      TEXT,           -- JSON: {"unconditional_affirmation","hedged","explicit"}
    duration_ms        INTEGER,
    user_text          TEXT,           -- 用户原文（摘要卡显示；proactive 为 NULL）
    reply_text         TEXT,           -- 过滤后的最终回复（摘要卡显示）
    prompt_tokens_est    INTEGER,      -- v15：本轮最后一次请求的估算输入 tokens
    prompt_tokens_actual INTEGER,      -- v15：provider usage.prompt_tokens（没有就 NULL）
    dropped_turns        INTEGER,      -- v15：L0 丢掉的历史轮数（0/未记为 NULL）
    error_kind           TEXT,         -- v17：本轮失败类别（plan-retry §6；正常轮 NULL）
    attempts             INTEGER,      -- v17：重试次数（含成功前的失败；正常轮 NULL）
    ts                 TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_turn_trace_user ON turn_trace(user_event_id);
CREATE INDEX IF NOT EXISTS idx_turn_trace_asst ON turn_trace(assistant_event_id);

-- One row per pipeline step inside a turn (spec-s8 §3.2). seq orders the
-- timeline; step_kind drives the frontend's node coloring.
CREATE TABLE IF NOT EXISTS trace_steps (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id     INTEGER NOT NULL REFERENCES turn_trace(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    step_kind   TEXT NOT NULL,   -- crisis_check|tool_intent|summary_intent|llm_round|tool_call|json_retry|prose_retry|filter|user_input|plan_submit|step_state|retry (plan-* 三枚举见 docs/zh/plan-planning.md T5)
    title       TEXT NOT NULL,   -- display heading (Chinese, user-facing)
    prompt      TEXT,            -- llm_round: full system prompt of THIS round (D1: unclipped)
    output      TEXT,            -- llm_round: raw model output; tool_call: reply text / error
    args_json   TEXT,            -- tool_call: args JSON verbatim
    summary     TEXT,            -- tool_call: ToolOutput.summary
    outcome     TEXT,            -- ok|denied|error|hit|miss|pass|answered|direct|none
    duration_ms INTEGER,
    user_text   TEXT,           -- v11: the current user message sent THIS round
    ts          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trace_steps_turn ON trace_steps(turn_id, seq);

-- 检索索引（v16，S2，见 memory/index.rs）：正文 → 空格分隔 2-gram 存 bigrams，
-- 再用 unicode61（按空格切）建 FTS5。**不是 trigram**——trigram 对 2 字中文
-- 零命中（index.rs 的用例把这个结论钉住了），而中文的检索单位就是 2 字词。
-- body 存原文，方便 /trace 与调试；检索只走 bigrams。
CREATE TABLE IF NOT EXISTS search_docs (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    kind    TEXT NOT NULL,      -- 'event' | 'fact' | 'preference' | 'boundary'
    ref_id  INTEGER NOT NULL,   -- 源表主键
    ts      TEXT NOT NULL,
    body    TEXT NOT NULL,
    bigrams TEXT NOT NULL,
    UNIQUE(kind, ref_id)
);
CREATE INDEX IF NOT EXISTS idx_search_docs_kind ON search_docs(kind, ref_id);
CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
    bigrams, content='search_docs', content_rowid='id', tokenize='unicode61'
);
-- FTS 与 search_docs 的同步（外部内容表的 delete 命令需要原始 token，故用触发器）
CREATE TRIGGER IF NOT EXISTS search_docs_ai AFTER INSERT ON search_docs BEGIN
    INSERT INTO search_fts(rowid, bigrams) VALUES (new.id, new.bigrams);
END;
CREATE TRIGGER IF NOT EXISTS search_docs_ad AFTER DELETE ON search_docs BEGIN
    INSERT INTO search_fts(search_fts, rowid, bigrams)
        VALUES ('delete', old.id, old.bigrams);
END;
-- 增量同步水位线：插入要 Rust 分词（触发器做不到），所以按 id 追赶。
CREATE TABLE IF NOT EXISTS search_sync (
    kind    TEXT PRIMARY KEY,
    last_id INTEGER NOT NULL
);
-- 用户「忘掉」必须连索引一起忘：源表删除 → 级联删 search_docs → 级联删 FTS。
-- 这四条触发器是 forget_event / forget_session / forget_all 的唯一保证，
-- 不依赖应用层记得调（漏一处就是「删了还能被搜出来」的合规事故）。
CREATE TRIGGER IF NOT EXISTS events_search_ad AFTER DELETE ON events BEGIN
    DELETE FROM search_docs WHERE kind = 'event' AND ref_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS facts_search_ad AFTER DELETE ON facts BEGIN
    DELETE FROM search_docs WHERE kind = 'fact' AND ref_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS preferences_search_ad AFTER DELETE ON preferences BEGIN
    DELETE FROM search_docs WHERE kind = 'preference' AND ref_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS boundaries_search_ad AFTER DELETE ON boundaries BEGIN
    DELETE FROM search_docs WHERE kind = 'boundary' AND ref_id = old.id;
END;

-- vector leg 仍未做：G-13/P5-1 的触发条件是「S2 之后近义改写仍 <75%」，
-- 基线与跑法见 memory/eval_recall.rs。

-- v18：派生记忆统一溯源（plan-reliability-phase1 R1）。
-- memory_id 用 TEXT：fact/preference/boundary 存十进制 id，profile 存 key。
CREATE TABLE IF NOT EXISTS memory_evidence (
    memory_type TEXT NOT NULL,   -- 'fact' | 'preference' | 'boundary' | 'profile'
    memory_id   TEXT NOT NULL,
    event_id    INTEGER NOT NULL REFERENCES events(id),
    relation    TEXT NOT NULL,   -- 'extracted' | 'promoted' | 'derived' | 'manual_link'
    created_at  TEXT NOT NULL,
    PRIMARY KEY (memory_type, memory_id, event_id, relation)
);
CREATE INDEX IF NOT EXISTS idx_evidence_event ON memory_evidence(event_id);
CREATE INDEX IF NOT EXISTS idx_evidence_memory ON memory_evidence(memory_type, memory_id);

-- v18：主动搭话本地日配额（plan-reliability-phase1 R4）。
CREATE TABLE IF NOT EXISTS proactive_daily_usage (
    local_date     TEXT PRIMARY KEY,  -- YYYY-MM-DD（本地日历日）
    used_count     INTEGER NOT NULL DEFAULT 0,
    reserved       INTEGER NOT NULL DEFAULT 0,
    last_spoken_at TEXT
);
"#;

pub(super) fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

fn set_user_version(conn: &Connection, v: i64) -> rusqlite::Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {v};"))
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    let mut table_exists = false;
    while let Some(row) = rows.next()? {
        table_exists = true;
        let name: String = row.get(1)?;
        if name == column {
            return Ok(());
        }
    }
    // 表还不存在（PRAGMA 对缺失表返回零行）：跳过。紧接着的 SCHEMA 会按最新
    // 定义把它建出来，列天然就在；硬 ALTER 只会报 "no such table"。
    if !table_exists {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))
}

/// Idempotent migrator. Handles:
/// - fresh DB → full current schema;
/// - v1 DB (scaffold-era, user_version 0 but tables exist) → in-place column
///   additions + audit table (plan §4, "v1 旧库打开 → 升级 → 旧数据无损");
/// - **stamped-but-incomplete DB** (user_version 已是最新但缺表) → 自愈。
///
/// 自愈这条是必须的：版本号只证明「迁移跑过」，不证明「表齐全」。2026-11-23
/// 真机上出现过 user_version=13 而 `user_profile` 不存在的库（建表帧随 WAL
/// 截断丢失，版本号已 checkpoint 落盘），旧实现的「版本够就 return」快路径
/// 把它永久卡死，前端报 `no such table: user_profile`。
pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let has_events: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'events'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)?;

    if !has_events {
        conn.execute_batch(SCHEMA)?;
        set_user_version(conn, SCHEMA_VERSION)?;
        return Ok(());
    }

    // 已有库：**先补列再跑 SCHEMA**——SCHEMA 末尾的 idx_events_unconsumed
    // 引用 events.consumed，v1 库里没这列时先建索引会直接报错。
    // 不看版本号决定要不要跑：版本号只证明「迁移跑过」，不证明「表齐全」，
    // 而两步都是幂等的（PRAGMA table_info 探测 / CREATE IF NOT EXISTS），
    // 每次打开跑一遍就是自愈本身，代价只是十几条元数据检查。
    add_column_if_missing(conn, "events", "role", "TEXT")?;
    add_column_if_missing(conn, "events", "session_id", "TEXT")?;
    add_column_if_missing(conn, "events", "consumed", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(conn, "facts", "confidence", "REAL NOT NULL DEFAULT 0.6")?;
    add_column_if_missing(conn, "facts", "importance", "INTEGER NOT NULL DEFAULT 3")?;
    add_column_if_missing(conn, "boundaries", "source_fact_id", "INTEGER REFERENCES facts(id)")?;

    // IF NOT EXISTS 的 SCHEMA：补出所有缺失的表、索引（含 v13 的画像表），
    // 也顺手修好「版本号够但缺表」的死局。位置必须在下面的补列之前——
    // turn_trace/trace_steps 在 v7 库里还不存在，ALTER 会报 no such table。
    conn.execute_batch(SCHEMA)?;

    // v7 → v8: audit columns for the agent tool pipeline (spec-s2).
    add_column_if_missing(conn, "tool_calls", "params_summary", "TEXT")?;
    add_column_if_missing(conn, "tool_calls", "result_status", "TEXT")?;
    add_column_if_missing(conn, "tool_calls", "prompt_trace", "TEXT")?;
    add_column_if_missing(conn, "tool_calls", "remote", "INTEGER DEFAULT 0")?;
    // v9 trace tables + summary columns (spec-s8; additive on DBs that
    // already created the tables in an earlier v9 build).
    add_column_if_missing(conn, "turn_trace", "user_text", "TEXT")?;
    add_column_if_missing(conn, "turn_trace", "reply_text", "TEXT")?;
    // v11: step 级当前输入（user message 不在 system prompt 里，trace 盲点）
    add_column_if_missing(conn, "trace_steps", "user_text", "TEXT")?;
    // v12: 任务卡外壳（tasks 表由 SCHEMA 的 IF NOT EXISTS 建出）。
    add_column_if_missing(conn, "tool_calls", "task_id", "TEXT")?;
    // v14: 规划能力（docs/zh/plan-planning.md §4）。必须在 SCHEMA 之后——
    // v12 老库的 tasks 表已在 SCHEMA 补出但无新列；顺序颠倒会 no such table。
    add_column_if_missing(conn, "tasks", "plan_json", "TEXT")?;
    add_column_if_missing(conn, "tasks", "plan_mode", "TEXT NOT NULL DEFAULT 'none'")?;
    add_column_if_missing(conn, "tasks", "cancel_reason", "TEXT")?;
    add_column_if_missing(conn, "tasks", "confirm_mode", "TEXT NOT NULL DEFAULT 'auto'")?;
    // v15: 上下文管理（docs/zh/plan-context.md §5）。sessions 表由 SCHEMA 的
    // IF NOT EXISTS 建出；这里只补列 + 回填 default 行。
    add_column_if_missing(conn, "tasks", "session_id", "TEXT")?;
    add_column_if_missing(conn, "turn_trace", "prompt_tokens_est", "INTEGER")?;
    add_column_if_missing(conn, "turn_trace", "prompt_tokens_actual", "INTEGER")?;
    add_column_if_missing(conn, "turn_trace", "dropped_turns", "INTEGER")?;
    // v17: 失败观测列（docs/zh/plan-retry.md §6）。改 SCHEMA 必须同时 bump
    // user_version——这条由 SCHEMA_VERSION=17 与 set_user_version_if_behind 共同保证。
    add_column_if_missing(conn, "turn_trace", "error_kind", "TEXT")?;
    add_column_if_missing(conn, "turn_trace", "attempts", "INTEGER")?;
    add_column_if_missing(conn, "consolidate_runs", "error", "TEXT")?;
    backfill_default_session(conn)?;
    // v18：evidence / 配额表由 SCHEMA IF NOT EXISTS 建出；这里只回填可追溯行。
    backfill_memory_evidence(conn)?;
    clear_stale_proactive_reserves(conn)?;

    set_user_version_if_behind(conn)?;
    Ok(())
}

/// v18 回填：能从现有列可靠推出的 evidence 写入；preference 历史无源不造假。
fn backfill_memory_evidence(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO memory_evidence (memory_type, memory_id, event_id, relation, created_at)
         SELECT 'fact', CAST(id AS TEXT), source_event_id, 'extracted', ts
           FROM facts WHERE source_event_id IS NOT NULL",
        [],
    )?;
    // 用 f.ts：极老库的 boundaries 可能缺 ts 列，facts.ts 从 v1 起就有。
    conn.execute(
        "INSERT OR IGNORE INTO memory_evidence (memory_type, memory_id, event_id, relation, created_at)
         SELECT 'boundary', CAST(b.id AS TEXT), f.source_event_id, 'derived', f.ts
           FROM boundaries b
           JOIN facts f ON f.id = b.source_fact_id
          WHERE f.source_event_id IS NOT NULL",
        [],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO memory_evidence (memory_type, memory_id, event_id, relation, created_at)
         SELECT 'profile', key, source_event_id, 'extracted', updated_at
           FROM user_profile WHERE source_event_id IS NOT NULL",
        [],
    )?;
    Ok(())
}

/// 启动/迁移时作废未提交的配额预留（崩溃残留），避免吞掉当日额度。
fn clear_stale_proactive_reserves(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("UPDATE proactive_daily_usage SET reserved = 0", [])?;
    Ok(())
}

/// v15 回填：老库里 `events.session_id='default'` 的对话，在 sessions 里
/// 补一行，否则对话页的会话列表看不到历史。标题取该会话首条用户消息前
/// 24 字（换行折成空格），空则「默认会话」；时间取该会话最早/最晚事件。
/// **不写 digest**（§5.4 第 5 条）：第一次真超预算时才生成，不给老用户
/// 凭空造摘要。`proactive` 不建行（D6）。幂等：已有该行就跳过。
fn backfill_default_session(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, updated_at, message_count)
         SELECT 'default',
                COALESCE(
                    NULLIF(TRIM(SUBSTR(
                        REPLACE(REPLACE(
                            (SELECT content FROM events
                              WHERE session_id = 'default' AND role = 'user'
                              ORDER BY id ASC LIMIT 1), char(10), ' '), char(13), ' ')
                    , 1, 24)), ''),
                    '默认会话'),
                COALESCE((SELECT MIN(ts) FROM events WHERE session_id = 'default'), ?1),
                COALESCE((SELECT MAX(ts) FROM events WHERE session_id = 'default'), ?1),
                (SELECT COUNT(*) FROM events
                  WHERE session_id = 'default' AND kind = 'dialogue')
          WHERE EXISTS (SELECT 1 FROM events WHERE session_id = 'default')
            AND NOT EXISTS (SELECT 1 FROM sessions WHERE id = 'default')",
        params![clock::now_iso()],
    )?;
    Ok(())
}

/// 表面上是一个 `if`，实则守住「版本号只增不减」：表结构由幂等步骤补齐，
/// 版本号只是记录，落后时才写回（也避免无谓的 PRAGMA 写入）。
fn set_user_version_if_behind(conn: &Connection) -> rusqlite::Result<()> {
    if user_version(conn)? < SCHEMA_VERSION {
        set_user_version(conn, SCHEMA_VERSION)?;
    }
    Ok(())
}

