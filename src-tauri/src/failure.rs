//! 通用重试与失败恢复（plan-retry.md §4）+ 后台循环守卫
//! （plan-loop-supervisor.md）。
//!
//! 一句话：失败不是异常分支，是**每层都有名字、有预算、有出口**的正常状态。
//!
//! 本模块做判定与算术：把 `LlmError` / 工具错误串 / `rusqlite::Error`
//! 收敛成同一个 [`Failure`]，并提供确定性的 [`Backoff`]。另含后台 loop 的
//! [`PanicGuard`] / [`spawn_guarded`] / [`LoopRegistry`]（D5 同文件）。
//!
//! 三条硬规矩（对应 plan-retry §3 不变量）：
//! - 只有 [`FailureKind::retryable`] 为真的 kind 才允许重试；
//! - 分类**只此一处**（字符串匹配在这退役，别在调用点再写 `starts_with`）；
//! - 退避必须可复现：同样的 attempt 得到同样的毫秒数，测试才能断言确切值。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::llm::LlmError;
use crate::memory::MemoryStore;

/// 11 类失败。新增一类要同时给它 hint、retryable 与 from_* 的归类分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// 连接被拒 / DNS / 断流。
    Transport,
    /// 请求超时。
    Timeout,
    /// 429。
    RateLimit,
    /// 5xx。
    Server,
    /// 401 / 403。
    Auth,
    /// 其它 4xx（含参数不合法、上下文超限）。
    BadRequest,
    /// 协议 / JSON 解析失败。
    Decode,
    /// 流式中途断（已产出部分正文）。
    Interrupted,
    /// 审批拒绝 / 权限不足。
    Denied,
    /// 工具或服务不存在 / 未启用。
    Unavailable,
    /// DB / Mutex 中毒 / panic。
    Internal,
}

impl FailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            FailureKind::Transport => "transport",
            FailureKind::Timeout => "timeout",
            FailureKind::RateLimit => "rate_limit",
            FailureKind::Server => "server",
            FailureKind::Auth => "auth",
            FailureKind::BadRequest => "bad_request",
            FailureKind::Decode => "decode",
            FailureKind::Interrupted => "interrupted",
            FailureKind::Denied => "denied",
            FailureKind::Unavailable => "unavailable",
            FailureKind::Internal => "internal",
        }
    }

    /// trace / DB 里存的是短串，读回来要能还原（`/trace` 与诊断列表用）。
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "transport" => FailureKind::Transport,
            "timeout" => FailureKind::Timeout,
            "rate_limit" => FailureKind::RateLimit,
            "server" => FailureKind::Server,
            "auth" => FailureKind::Auth,
            "bad_request" => FailureKind::BadRequest,
            "decode" => FailureKind::Decode,
            "interrupted" => FailureKind::Interrupted,
            "denied" => FailureKind::Denied,
            "unavailable" => FailureKind::Unavailable,
            "internal" => FailureKind::Internal,
            _ => return None,
        })
    }

    /// 该 kind 在**没有额外上下文**时是否允许重试。调用点可以再收紧
    /// （流式已产出、Write 档、web 语义失败），但不能放宽。
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            FailureKind::Transport
                | FailureKind::Timeout
                | FailureKind::RateLimit
                | FailureKind::Server
        )
    }

    /// 给用户的一句话（中文，不带技术术语）。`degraded` 用于「已流出一半」
    /// 这类需要补一句现状的场景，由调用点决定加不加。
    pub const fn hint(self) -> &'static str {
        match self {
            FailureKind::Transport => "网络没连上，等一下再试",
            FailureKind::Timeout => "它想太久了，没等到结果",
            FailureKind::RateLimit => "云端限流了，歇一会儿",
            FailureKind::Server => "云端出故障了（不是你的问题）",
            FailureKind::Auth => "API Key 或额度不对，去设置里看一下",
            FailureKind::BadRequest => "请求被拒了（配置或上下文超限）",
            FailureKind::Decode => "回复格式不对，已丢弃",
            FailureKind::Interrupted => "说到一半断了",
            FailureKind::Denied => "这次调用被拒绝了",
            FailureKind::Unavailable => "这个工具暂时用不了",
            FailureKind::Internal => "它内部出错了，重启一下试试",
        }
    }
}

/// 唯一的失败形状。`retryable` 由 kind 推出但不设 `pub` 赋值口：
/// 想禁重试只能走 [`Failure::downgrade`]，这样「谁把它变成不可重试」永远
/// 在调用点看得见。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub retryable: bool,
    pub hint: &'static str,
    pub detail: String,
    /// 这次失败一共发起过几次请求（含重试）；`trace.attempts` 的来源。
    pub attempts: u32,
}

impl Failure {
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Failure {
            kind,
            retryable: kind.retryable(),
            hint: kind.hint(),
            detail: detail.into(),
            attempts: 1,
        }
    }

    /// 重试循环里把真实尝试次数写回（trace 观测用）。
    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts.max(1);
        self
    }

    /// 调用点知道这次不能重试（流式已产出 / Write 档 / 已重试过一轮）。
    pub fn downgrade(mut self) -> Self {
        self.retryable = false;
        self
    }

    /// 模型层：`LlmError` 三变体在这里第一次被当作信息，而不是被抹平。
    pub fn from_llm(err: &LlmError) -> Self {
        match err {
            LlmError::Transport(raw) => {
                let lower = raw.to_ascii_lowercase();
                if lower.contains("timed out") || lower.contains("timeout") {
                    // ureq 的超时走 Transport：对用户是两件不同的事
                    // （网络没连上 vs 它想太久），别混成一句。
                    Failure::new(FailureKind::Timeout, raw.clone())
                } else {
                    Failure::new(FailureKind::Transport, raw.clone())
                }
            }
            LlmError::Status(code, body) => {
                let mut detail = format!("http {code}");
                if !body.trim().is_empty() {
                    detail.push_str(": ");
                    detail.push_str(body);
                }
                Failure::new(status_kind(*code), detail)
            }
            LlmError::Decode(raw) => Failure::new(FailureKind::Decode, raw.clone()),
        }
    }

    /// 工具层：现有错误串 → 语义的唯一翻译点。
    ///
    /// 约定（§5.4）：
    /// - `bad_args*` / `unknown_tool` / `unsupported` → `BadRequest`（重试无意义）；
    /// - `denied` / `forbidden` → `Denied`；
    /// - `mcp_timeout` / "timed out" → `Timeout`；
    /// - `mcp_unreachable` → `Transport`：这是连接层失败，Read 档允许 1 次
    ///   重试（与 T4 现状一致）；`Unavailable` 只留给「工具/服务不存在」，
    ///   那种错误重试一百次也一样；
    /// - 其余未知错误 → `Internal`（不重试：陌生错误不该被当成抖动）。
    pub fn from_tool(err: &str) -> Self {
        let raw = err.trim();
        let lower = raw.to_ascii_lowercase();
        let kind = if raw.starts_with("bad_args") || raw == "unknown_tool" || raw == "unsupported"
        {
            FailureKind::BadRequest
        } else if raw == "denied" || raw == "forbidden" {
            FailureKind::Denied
        } else if raw.starts_with("mcp_timeout") || lower.contains("timed out") {
            FailureKind::Timeout
        } else if raw.starts_with("mcp_unreachable")
            || lower.contains("connection refused")
            || lower.contains("could not connect")
        {
            FailureKind::Transport
        } else if raw.starts_with("tool_not_found") || raw.starts_with("not_configured") {
            FailureKind::Unavailable
        } else {
            FailureKind::Internal
        };
        Failure::new(kind, raw.to_string())
    }

    /// DB 层：一律 `Internal`（DB 出问题不是「再试一次就好了」的那种事），
    /// 但 detail 保留 sqlite 原文，且是否走自愈由 [`is_corrupt_db`] 判——
    /// 分类与处置分开，避免「BUSY 也被当成坏库」。
    pub fn from_db(err: &rusqlite::Error) -> Self {
        Failure::new(FailureKind::Internal, err.to_string())
    }
}

fn status_kind(code: u16) -> FailureKind {
    match code {
        401 | 403 => FailureKind::Auth,
        429 => FailureKind::RateLimit,
        408 => FailureKind::Timeout,
        500..=599 => FailureKind::Server,
        _ => FailureKind::BadRequest,
    }
}

fn sqlite_code(err: &rusqlite::Error) -> Option<rusqlite::ErrorCode> {
    match err {
        rusqlite::Error::SqliteFailure(e, _) => Some(e.code),
        _ => None,
    }
}

/// 只有这两种才允许走「改名备份 + 建空库」（§11 风险矩阵：`SQLITE_BUSY`
/// 是文件锁，误判会把好好的库搬走）。
pub fn is_corrupt_db(err: &rusqlite::Error) -> bool {
    matches!(
        sqlite_code(err),
        Some(rusqlite::ErrorCode::DatabaseCorrupt) | Some(rusqlite::ErrorCode::NotADatabase)
    )
}

/// 文件锁：先重试（1s/2s/4s）再报错，绝不搬库。
pub fn is_busy_db(err: &rusqlite::Error) -> bool {
    matches!(
        sqlite_code(err),
        Some(rusqlite::ErrorCode::DatabaseBusy) | Some(rusqlite::ErrorCode::DatabaseLocked)
    )
}

// ------------------------------------------------------------------ backoff

/// 指数退避 + 上限 + 确定性抖动。
///
/// `delay(attempt)` 的 attempt 从 1 开始（第 1 次重试前等多久）。
/// 抖动不是为「错开惊群」（桌面单用户没有惊群），只为避免每次都在同一秒
/// 撞上限流窗口；确定性哈希让测试能断言确切毫秒数。
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    base_ms: u64,
    cap_ms: u64,
    jitter_pct: u64,
    seed: u64,
}

impl Backoff {
    pub const fn new(base_ms: u64, cap_ms: u64, jitter_pct: u64) -> Self {
        Backoff { base_ms, cap_ms, jitter_pct, seed: 0x9E37_79B9_7F4A_7C15 }
    }

    pub const fn seeded(base_ms: u64, cap_ms: u64, jitter_pct: u64, seed: u64) -> Self {
        Backoff { base_ms, cap_ms, jitter_pct, seed }
    }

    pub fn delay(&self, attempt: u32) -> Duration {
        let n = attempt.max(1).min(20); // 1 << 20 已经远大于任何 cap
        let grown = self.base_ms.saturating_mul(1u64 << (n - 1));
        let capped = grown.min(self.cap_ms);
        let span = capped * self.jitter_pct / 100;
        let offset = if span == 0 {
            0i64
        } else {
            (mix(n as u64, self.seed) % (span * 2 + 1)) as i64 - span as i64
        };
        // cap 是硬上限：抖动也不许把等待拖过它（否则「30 分钟后重试」
        // 在日志里会变成 37 分钟，读日志的人会以为有第三个参数）。
        Duration::from_millis((capped as i64 + offset).clamp(1, self.cap_ms.max(1) as i64) as u64)
    }
}

/// splitmix64：小、确定、不需要 `rand` 依赖。
fn mix(x: u64, seed: u64) -> u64 {
    let mut z = x.wrapping_add(seed).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 模型请求（非流式）：最多 2 次重试 = 共 3 次请求（§4.2）。
pub const MODEL_RETRIES: u32 = 2;
/// 流式：最多 1 次，且仅在零 delta 时（由调用点判定）。
pub const STREAM_RETRIES: u32 = 1;
/// 工具（Read 档）：1 次重试 = 共 2 次尝试（与 T4 现状一致）。
pub const TOOL_RETRIES: u32 = 1;
/// 夜间作业：当日最多 5 次，之后等明天。
pub const JOB_DAILY_FAIL_CAP: u32 = 5;

pub const MODEL: Backoff = Backoff::seeded(500, 4_000, 25, 0x01);
pub const STREAM: Backoff = Backoff::seeded(500, 1_000, 25, 0x02);
pub const TOOL: Backoff = Backoff::seeded(500, 2_000, 25, 0x03);
pub const JOB: Backoff = Backoff::seeded(60_000, 1_800_000, 0, 0x04);

// --------------------------------------------------------------- job guard

/// 作业退避状态（存 `settings` 的 KV，不建表，§5.5）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JobState {
    pub fails: u32,
    /// RFC3339（本地时区偏移保留），下次允许执行的时刻。
    pub next_at: String,
}

impl JobState {
    /// 第 `fails_before + 1` 次失败之后的状态。
    ///
    /// 达到当日上限就不再按指数等，直接推到**明天 03:00**——继续在当天
    /// 重试只会刷日志，用户在睡觉，问题也不会自己好。
    pub fn after_fail(fails_before: u32, now: chrono::DateTime<chrono::Local>) -> Self {
        let fails = fails_before.saturating_add(1);
        let next = if fails >= JOB_DAILY_FAIL_CAP {
            tomorrow_at_three(now)
        } else {
            now + chrono::Duration::from_std(JOB.delay(fails))
                .unwrap_or_else(|_| chrono::Duration::minutes(30))
        };
        JobState { fails, next_at: next.to_rfc3339() }
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    /// 现在能不能跑。解析不了的脏数据当作「没退避」（宁可多跑一次，
    /// 也不要因为一个坏字符串把作业永久卡死）。
    pub fn allows(&self, now: chrono::DateTime<chrono::Local>) -> bool {
        match chrono::DateTime::parse_from_rfc3339(&self.next_at) {
            Ok(t) => now >= t.with_timezone(&chrono::Local),
            Err(_) => true,
        }
    }
}

fn tomorrow_at_three(now: chrono::DateTime<chrono::Local>) -> chrono::DateTime<chrono::Local> {
    let tomorrow = now.date_naive() + chrono::Duration::days(1);
    tomorrow
        .and_hms_opt(3, 0, 0)
        .and_then(|t| t.and_local_timezone(*now.offset()).single())
        .map(|t| t.with_timezone(&chrono::Local))
        .unwrap_or_else(|| now + chrono::Duration::hours(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(iso: &str) -> chrono::DateTime<chrono::Local> {
        chrono::DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .with_timezone(&chrono::Local)
    }

    // ---------------------------------------------------------- 分类

    #[test]
    fn llm_status_maps_to_four_distinct_kinds() {
        // A1：401 不许重试，且 hint 要指向 key/额度。
        let auth = Failure::from_llm(&LlmError::Status(401, "invalid api key".into()));
        assert_eq!(auth.kind, FailureKind::Auth);
        assert!(!auth.retryable);
        assert!(auth.hint.contains("Key") || auth.hint.contains("额度"), "{}", auth.hint);

        // A2 / A3：429 与 502 都可以重试，但用户看到的话必须不同。
        let limited = Failure::from_llm(&LlmError::Status(429, "rate limited".into()));
        assert_eq!(limited.kind, FailureKind::RateLimit);
        assert!(limited.retryable);
        let down = Failure::from_llm(&LlmError::Status(502, "bad gateway".into()));
        assert_eq!(down.kind, FailureKind::Server);
        assert!(down.retryable);
        assert_ne!(limited.hint, down.hint);

        // 其它 4xx（如 400 上下文超限）重试没有意义。
        let bad = Failure::from_llm(&LlmError::Status(400, "context too long".into()));
        assert_eq!(bad.kind, FailureKind::BadRequest);
        assert!(!bad.retryable);
    }

    #[test]
    fn llm_transport_timeout_is_not_confused_with_refused() {
        let refused = Failure::from_llm(&LlmError::Transport("Connection refused (os error 61)".into()));
        assert_eq!(refused.kind, FailureKind::Transport);
        assert!(refused.retryable);

        let slow = Failure::from_llm(&LlmError::Transport("timed out reading response".into()));
        assert_eq!(slow.kind, FailureKind::Timeout, "超时与连不上对用户是两件事");

        let garbled = Failure::from_llm(&LlmError::Decode("expected value at line 1".into()));
        assert_eq!(garbled.kind, FailureKind::Decode);
        assert!(!garbled.retryable);
    }

    #[test]
    fn tool_errors_are_translated_here_and_nowhere_else() {
        assert_eq!(Failure::from_tool("bad_args:missing:path").kind, FailureKind::BadRequest);
        assert_eq!(Failure::from_tool("denied").kind, FailureKind::Denied);
        assert!(!Failure::from_tool("denied").retryable);

        let timeout = Failure::from_tool("mcp_timeout: no response in 30s");
        assert_eq!(timeout.kind, FailureKind::Timeout);
        assert!(timeout.retryable, "超时的 Read 工具允许 1 次重试");

        // 连接层失败 = Transport（与 T4 现状一致：mcp_unreachable 重试 1 次）。
        let unreachable = Failure::from_tool("mcp_unreachable: server exited");
        assert_eq!(unreachable.kind, FailureKind::Transport);
        assert!(unreachable.retryable);

        // Unavailable 只留给「不存在」：重试一百次也一样。
        let missing = Failure::from_tool("tool_not_found: web_x");
        assert_eq!(missing.kind, FailureKind::Unavailable);
        assert!(!missing.retryable);

        // 陌生错误按 Internal：不把未知当抖动。
        let unknown = Failure::from_tool("some new backend exploded");
        assert_eq!(unknown.kind, FailureKind::Internal);
        assert!(!unknown.retryable);
    }

    #[test]
    fn kind_strings_roundtrip_for_trace_and_ui() {
        for k in [
            FailureKind::Transport,
            FailureKind::Timeout,
            FailureKind::RateLimit,
            FailureKind::Server,
            FailureKind::Auth,
            FailureKind::BadRequest,
            FailureKind::Decode,
            FailureKind::Interrupted,
            FailureKind::Denied,
            FailureKind::Unavailable,
            FailureKind::Internal,
        ] {
            assert_eq!(FailureKind::parse(k.as_str()), Some(k));
            assert!(!k.hint().is_empty(), "{k:?} 必须有给用户的话");
        }
        assert_eq!(FailureKind::parse("no_such_kind"), None);
    }

    #[test]
    fn downgrade_keeps_the_kind_but_forbids_retry() {
        let f = Failure::from_llm(&LlmError::Status(503, "maintenance".into())).downgrade();
        assert_eq!(f.kind, FailureKind::Server, "失败事实不许改写");
        assert!(!f.retryable, "调用点可以禁重试");
    }

    // ---------------------------------------------------------- 退避

    #[test]
    fn backoff_grows_then_saturates_and_stays_in_bounds() {
        for b in [MODEL, STREAM, TOOL, JOB] {
            let base = b.base_ms;
            let cap = b.cap_ms;
            for attempt in 1..=8u32 {
                let d = b.delay(attempt).as_millis() as u64;
                let ideal = (base << (attempt.min(20) - 1)).min(cap);
                let span = ideal * b.jitter_pct / 100;
                assert!(
                    d + 1 >= ideal.saturating_sub(span) && d <= ideal + span.max(1),
                    "attempt {attempt}: {d}ms 落在 {ideal}±{span} 之外"
                );
                assert!(d >= 1, "退避不许是零：会在同一毫秒连发");
            }
            assert!(
                b.delay(50).as_millis() as u64 <= cap,
                "再多尝试也不许超过上限"
            );
        }
    }

    #[test]
    fn backoff_is_deterministic_so_tests_can_lock_it() {
        assert_eq!(MODEL.delay(2), MODEL.delay(2));
        assert_eq!(TOOL.delay(1), TOOL.delay(1));
        // attempt 0 与 1 等价（调用点从 1 数，但别因为 0 立刻打脸）。
        assert_eq!(MODEL.delay(0), MODEL.delay(1));
        // 不同层用不同种子：抖动不该在两层之间完全同相。
        let same_shape = MODEL.delay(3);
        let other_seed = Backoff::seeded(500, 4_000, 25, 0x99).delay(3);
        assert_ne!(same_shape, other_seed);
    }

    #[test]
    fn job_backoff_waits_a_minute_then_gives_up_for_the_day() {
        let now = local("2026-11-23T22:50:00+08:00");
        let first = JobState::after_fail(0, now);
        assert_eq!(first.fails, 1);
        assert!(!first.allows(now), "刚失败完不许立刻再跑（现状是每 60s 重跑）");
        assert!(first.allows(now + chrono::Duration::seconds(61)));

        let fifth = JobState::after_fail(4, now);
        assert_eq!(fifth.fails, JOB_DAILY_FAIL_CAP);
        assert!(!fifth.allows(now + chrono::Duration::hours(1)), "第 5 次之后当天不再跑");
        let next = chrono::DateTime::parse_from_rfc3339(&fifth.next_at).unwrap();
        assert_eq!(next.with_timezone(&chrono::Local).date_naive(), now.date_naive() + chrono::Duration::days(1));
        assert_eq!(next.with_timezone(&chrono::Local).format("%H:%M").to_string(), "03:00");
    }

    #[test]
    fn job_state_roundtrips_and_tolerates_garbage() {
        let state = JobState::after_fail(2, local("2026-11-23T22:50:00+08:00"));
        assert_eq!(JobState::parse(&state.encode()), Some(state.clone()));
        assert_eq!(JobState::parse("not json"), None);

        // 脏数据当「没退避」：宁可多跑一次，也别把作业永久卡死。
        let broken = JobState { fails: 3, next_at: "昨天下午".into() };
        assert!(broken.allows(local("2026-11-23T22:50:00+08:00")));
    }

    // ---------------------------------------------------------- DB 分类

    fn sqlite_failure(code: rusqlite::ErrorCode) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code, extended_code: 0 },
            Some("test".into()),
        )
    }

    #[test]
    fn only_corruption_qualifies_for_self_healing() {
        let corrupt = sqlite_failure(rusqlite::ErrorCode::DatabaseCorrupt);
        assert!(is_corrupt_db(&corrupt));
        assert_eq!(Failure::from_db(&corrupt).kind, FailureKind::Internal);

        // 文件锁不是坏库：误判会把好好的记忆搬走（§11）。
        let busy = sqlite_failure(rusqlite::ErrorCode::DatabaseBusy);
        assert!(is_busy_db(&busy));
        assert!(!is_corrupt_db(&busy), "BUSY 只重试，绝不搬库");

        assert!(!is_corrupt_db(&rusqlite::Error::QueryReturnedNoRows));
    }
}

// --------------------------------------------------------------- job guard（T5 §5.5）

/// 作业守门（plan-retry §5.5）：退避状态存 `settings` KV（不建新表）。
/// 「该不该跑」是 `decide_trigger` 的纯函数职责；「能不能跑」在这里——
/// 两件事分开，谁也不污染谁的签名。
pub struct JobGuard {
    pub name: &'static str,
}

impl JobGuard {
    fn key(&self) -> String {
        format!("job_backoff:{}", self.name)
    }

    fn read(&self, conn: &rusqlite::Connection) -> Option<JobState> {
        crate::memory::db::get_setting(conn, &self.key())
            .ok()
            .flatten()
            .and_then(|raw| JobState::parse(&raw))
    }

    /// 现在能不能跑。没有退避记录 / 脏数据 → 能跑（宁可多跑一次）。
    pub fn should_run(&self, conn: &rusqlite::Connection, now: chrono::DateTime<chrono::Local>) -> bool {
        self.read(conn).map(|s| s.allows(now)).unwrap_or(true)
    }

    /// 累计失败次数（作业层写审计 / 诊断用）。
    pub fn fails(&self, conn: &rusqlite::Connection) -> u32 {
        self.read(conn).map(|s| s.fails).unwrap_or(0)
    }

    /// 成功 → 清 key，明天从零算起。
    pub fn record_ok(&self, conn: &rusqlite::Connection) {
        let _ = crate::memory::db::remove_setting(conn, &self.key());
    }

    /// 失败 → 按 `Backoff{60s, 30min}` 写退避状态，返回新状态
    /// （`next_at` 给审计行；第 5 次起直接推到明天 03:00）。
    pub fn record_fail(
        &self,
        conn: &rusqlite::Connection,
        now: chrono::DateTime<chrono::Local>,
    ) -> JobState {
        let state = JobState::after_fail(self.fails(conn), now);
        let _ = crate::memory::db::set_setting(conn, &self.key(), &state.encode());
        state
    }
}

// --------------------------------------------------------------- panic guard（T5 §5.6）

/// 后台循环的 panic 守卫：每 tick 包一层 `catch_unwind`，panic 记 `error`
/// 日志并计连续次数；同一线程连续 5 次 panic 后 [`PanicGuard::stopped`]
/// 为真，循环体据此自己退出——防 panic 风暴刷日志（A9）。
pub struct PanicGuard {
    name: &'static str,
    consecutive: u32,
}

impl PanicGuard {
    pub const CAP: u32 = 5;

    pub fn new(name: &'static str) -> Self {
        PanicGuard { name, consecutive: 0 }
    }

    pub fn stopped(&self) -> bool {
        self.consecutive >= Self::CAP
    }

    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }

    /// 跑一轮。panic → `None`（日志已记）；cap 已到 → `None`（先查
    /// [`Self::stopped`]）。正常 → `Some(返回值)`，连续计数清零。
    pub fn tick<T>(&mut self, f: impl FnOnce() -> T) -> Option<T> {
        if self.stopped() {
            return None;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(v) => {
                self.consecutive = 0;
                Some(v)
            }
            Err(payload) => {
                self.consecutive += 1;
                log::error!(
                    "后台循环 {} panic（连续 {}/{}）：{}",
                    self.name,
                    self.consecutive,
                    Self::CAP,
                    panic_summary(&payload)
                );
                None
            }
        }
    }
}

/// 后台线程的外层兜底：整个闭包 panic 时记 error 日志（带 payload 摘要），
/// 线程沉默退出而不是把整个进程炸掉。循环内的逐 tick 守卫由 [`PanicGuard`] 负责。
pub fn spawn_guarded(name: &'static str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name(format!("guarded:{name}"))
        .spawn(move || {
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                log::error!("后台线程 {name} panic 退出：{}", panic_summary(&payload));
            }
        });
}

fn panic_summary(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "（非字符串 panic 载荷）".into()
    }
}

// --------------------------------------------------------------- loop registry（plan-loop-supervisor）

/// 齐套验收的 8 条常驻 loop 名（D3）。顺序稳定，给设置页列表用。
pub const LOOP_NAMES: &[&str] = &[
    "scheduler",
    "predict",
    "growth",
    "relationship",
    "perception",
    "face",
    "proactive",
    "calendar",
];

/// `settings` 键：用户是否允许 CAP 后有界自动重启（D1，默认 off）。
pub const SETTING_AUTORESTART: &str = "loop_autorestart";

fn loop_status_key(name: &str) -> String {
    format!("loop_status:{name}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopState {
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
struct LoopHealth {
    state: LoopState,
    last_beat: Option<String>,
    stopped_at: Option<String>,
    detail: Option<String>,
    restarts: u32,
}

impl Default for LoopHealth {
    fn default() -> Self {
        LoopHealth {
            state: LoopState::Running,
            last_beat: None,
            stopped_at: None,
            detail: None,
            restarts: 0,
        }
    }
}

/// 进程内存活注册表（plan-loop-supervisor §4.1 / D5）。
/// 不持久化存活态；停机细节另写 `settings`。
#[derive(Default)]
pub struct LoopRegistry {
    inner: Mutex<HashMap<&'static str, LoopHealth>>,
}

/// 设置页 / 命令用的只读快照。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LoopStatusDto {
    pub name: String,
    pub state: String,
    pub last_beat: Option<String>,
    pub stopped_at: Option<String>,
    pub detail: Option<String>,
    pub restarts: u32,
}

impl LoopRegistry {
    /// 启动或复活时登记为 running；**保留** `restarts` 计数。
    pub fn register(&self, name: &'static str) {
        let now = crate::memory::clock::now_iso();
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(name).or_default();
        entry.state = LoopState::Running;
        entry.last_beat = Some(now);
        entry.stopped_at = None;
        entry.detail = None;
    }

    pub fn heartbeat(&self, name: &str) {
        let now = crate::memory::clock::now_iso();
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = map.get_mut(name) {
            if h.state == LoopState::Running {
                h.last_beat = Some(now);
            }
        }
    }

    /// CAP 停机：记 stopped；返回当前 `restarts`（尚未因本次复活递增）。
    pub fn mark_stopped(&self, name: &str, summary: &str) -> u32 {
        let now = crate::memory::clock::now_iso();
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // 停机时 name 可能来自 &'static，但 HashMap 键是 &'static——用已知表查找。
        let key = LOOP_NAMES.iter().copied().find(|n| *n == name);
        let Some(key) = key else {
            return 0;
        };
        let entry = map.entry(key).or_default();
        entry.state = LoopState::Stopped;
        entry.stopped_at = Some(now);
        entry.detail = Some(summary.to_string());
        entry.restarts
    }

    /// 尝试占用一次复活额度。已重启过 → false；否则 restarts+=1 并 true。
    pub fn try_consume_restart(&self, name: &str) -> bool {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(key) = LOOP_NAMES.iter().copied().find(|n| *n == name) else {
            return false;
        };
        let entry = map.entry(key).or_default();
        if entry.restarts >= 1 {
            return false;
        }
        entry.restarts += 1;
        true
    }

    pub fn snapshot(&self) -> Vec<LoopStatusDto> {
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        LOOP_NAMES
            .iter()
            .map(|name| {
                let h = map.get(name).cloned().unwrap_or_default();
                LoopStatusDto {
                    name: (*name).to_string(),
                    state: match h.state {
                        LoopState::Running => "running".into(),
                        LoopState::Stopped => "stopped".into(),
                    },
                    last_beat: h.last_beat,
                    stopped_at: h.stopped_at,
                    detail: h.detail,
                    restarts: h.restarts,
                }
            })
            .collect()
    }
}

/// 安全登记（Registry 尚未 manage 时静默跳过——单测 / 早期启动）。
pub fn try_register(app: &AppHandle, name: &'static str) {
    if let Some(reg) = app.try_state::<LoopRegistry>() {
        reg.register(name);
    }
}

pub fn try_heartbeat(app: &AppHandle, name: &str) {
    if let Some(reg) = app.try_state::<LoopRegistry>() {
        reg.heartbeat(name);
    }
}

/// CAP / 外层退出单点留痕（§4.2）：settings + system 事件 + registry + emit。
pub fn record_loop_stop(app: &AppHandle, name: &str, detail: &str) {
    log::error!("后台循环 {name} 停机：{detail}");
    let restarts = app
        .try_state::<LoopRegistry>()
        .map(|r| r.mark_stopped(name, detail))
        .unwrap_or(0);
    let at = crate::memory::clock::now_iso();
    let payload = serde_json::json!({
        "state": "stopped",
        "at": at,
        "detail": detail,
        "restarts": restarts,
    });
    if let Some(store) = app.try_state::<MemoryStore>() {
        if let Ok(conn) = store.0.lock() {
            let _ = crate::memory::db::set_setting(
                &conn,
                &loop_status_key(name),
                &payload.to_string(),
            );
            let content = serde_json::json!({
                "loop": name,
                "event": "stopped",
                "detail": detail,
                "restarts": restarts,
            })
            .to_string();
            let _ = crate::memory::db::add_event(&conn, "system", &content);
        }
    }
    let _ = app.emit(
        "loop:stopped",
        serde_json::json!({ "name": name, "detail": detail, "restarts": restarts }),
    );
}

fn autorestart_enabled(app: &AppHandle) -> bool {
    let Some(store) = app.try_state::<MemoryStore>() else {
        return false;
    };
    let Ok(conn) = store.0.lock() else {
        return false;
    };
    matches!(
        crate::memory::db::get_setting(&conn, SETTING_AUTORESTART)
            .ok()
            .flatten()
            .as_deref(),
        Some("on")
    )
}

/// D1+D2：开关开着且本进程该 name 尚未复活过 → 立即允许再起。
/// 调用方在返回 true 后**立刻**再调一次本模块的 `spawn`（不追溯、无冷却）。
pub fn maybe_autorestart(app: &AppHandle, name: &'static str) -> bool {
    if !autorestart_enabled(app) {
        return false;
    }
    let Some(reg) = app.try_state::<LoopRegistry>() else {
        return false;
    };
    if !reg.try_consume_restart(name) {
        return false;
    }
    let _ = app.emit(
        "loop:restarted",
        serde_json::json!({ "name": name }),
    );
    true
}

/// CAP 停机：留痕；若应复活返回 true（调用方立即再 spawn）。
pub fn on_loop_cap(app: &AppHandle, name: &'static str, detail: &str) -> bool {
    record_loop_stop(app, name, detail);
    maybe_autorestart(app, name)
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn loops_status(registry: tauri::State<'_, LoopRegistry>) -> Vec<LoopStatusDto> {
    registry.snapshot()
}

#[tauri::command]
pub fn loop_autorestart_get(store: tauri::State<'_, MemoryStore>) -> Result<bool, String> {
    let conn = store.0.lock().map_err(|e| e.to_string())?;
    Ok(matches!(
        crate::memory::db::get_setting(&conn, SETTING_AUTORESTART)
            .map_err(|e| e.to_string())?
            .as_deref(),
        Some("on")
    ))
}

#[tauri::command]
pub fn loop_autorestart_set(
    store: tauri::State<'_, MemoryStore>,
    enabled: bool,
) -> Result<bool, String> {
    let conn = store.0.lock().map_err(|e| e.to_string())?;
    let val = if enabled { "on" } else { "off" };
    crate::memory::db::set_setting(&conn, SETTING_AUTORESTART, val).map_err(|e| e.to_string())?;
    Ok(enabled)
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn panic_counts_and_a_success_resets_the_streak() {
        let mut g = PanicGuard::new("test-loop");
        assert!(g.tick(|| 1).is_some());
        assert_eq!(g.consecutive(), 0, "成功清零");
        assert!(g.tick(|| panic!("boom")).is_none());
        assert_eq!(g.consecutive(), 1);
        assert!(g.tick(|| panic!("boom")).is_none());
        assert_eq!(g.consecutive(), 2, "连续计数只增不清");
        assert!(g.tick(|| 2).is_some());
        assert_eq!(g.consecutive(), 0, "恢复即清零");
    }

    #[test]
    fn five_consecutive_panics_stop_the_loop() {
        let mut g = PanicGuard::new("test-loop");
        for _ in 0..PanicGuard::CAP {
            assert!(g.tick(|| panic!("boom")).is_none());
        }
        assert!(g.stopped(), "连续 5 次 panic 后必须停");
        assert!(g.tick(|| 1).is_none(), "停了就不许再跑");
        // 恢复不了的循环就该退出——这是调用点的契约。
    }

    #[test]
    fn job_guard_backoff_and_recovery() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::memory::db::SCHEMA).unwrap();
        let g = JobGuard { name: "consolidate" };
        let now = chrono::Local::now();

        // 没记录 → 能跑。
        assert!(g.should_run(&conn, now));

        // 失败 1 次：60s 内不能跑，过了 next_at 能跑。
        let st = g.record_fail(&conn, now);
        assert_eq!(st.fails, 1);
        assert!(!g.should_run(&conn, now), "刚失败，立刻再跑必须被挡");
        assert!(
            !g.should_run(&conn, now + chrono::Duration::seconds(30)),
            "退避 base 60s：30s 后仍挡"
        );
        let next = chrono::DateTime::parse_from_rfc3339(&st.next_at).unwrap();
        assert!(
            g.should_run(&conn, next.with_timezone(&chrono::Local)),
            "到 next_at 就放行"
        );

        // 成功 → 清零。
        g.record_ok(&conn);
        assert_eq!(g.fails(&conn), 0);
        assert!(g.should_run(&conn, now));
    }

    #[test]
    fn fifth_failure_pushes_to_tomorrow_three() {
        use chrono::Timelike;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::memory::db::SCHEMA).unwrap();
        let g = JobGuard { name: "consolidate" };
        let now = chrono::Local::now();
        for _ in 0..5 {
            g.record_fail(&conn, now);
        }
        let st = g.read(&conn).unwrap();
        assert_eq!(st.fails, 5);
        let next = chrono::DateTime::parse_from_rfc3339(&st.next_at)
            .unwrap()
            .with_timezone(&chrono::Local);
        assert_eq!(next.hour(), 3, "第 5 次失败推到明天 03:00");
        assert_eq!(next.date_naive(), now.date_naive() + chrono::Duration::days(1));
        assert!(!g.should_run(&conn, now + chrono::Duration::hours(1)));
    }

    #[test]
    fn registry_register_heartbeat_and_stop_snapshot() {
        let reg = LoopRegistry::default();
        assert_eq!(reg.snapshot().len(), LOOP_NAMES.len(), "八行齐套");
        reg.register("face");
        reg.heartbeat("face");
        let snap = reg.snapshot();
        let face = snap.iter().find(|r| r.name == "face").unwrap();
        assert_eq!(face.state, "running");
        assert!(face.last_beat.is_some());

        let r = reg.mark_stopped("face", "consecutive panic cap");
        assert_eq!(r, 0);
        let face = reg.snapshot().into_iter().find(|x| x.name == "face").unwrap();
        assert_eq!(face.state, "stopped");
        assert_eq!(face.detail.as_deref(), Some("consecutive panic cap"));
        assert!(face.stopped_at.is_some());
    }

    #[test]
    fn registry_restart_hard_cap_one_per_process() {
        let reg = LoopRegistry::default();
        reg.register("calendar");
        reg.mark_stopped("calendar", "cap");
        assert!(reg.try_consume_restart("calendar"), "第一次复活放行");
        assert!(!reg.try_consume_restart("calendar"), "硬顶：第二次拒绝");
        // 复活后再登记不应清零 restarts。
        reg.register("calendar");
        let cal = reg.snapshot().into_iter().find(|x| x.name == "calendar").unwrap();
        assert_eq!(cal.state, "running");
        assert_eq!(cal.restarts, 1);
        assert!(!reg.try_consume_restart("calendar"));
    }
}
