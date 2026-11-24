//! P2 item 2 — calendar / mail tool calls (docs/zh/plan-calendar.md).
//!
//! Read-only first. This module owns the **only** path from the machine's
//! calendar into bonsai, and it is deliberately narrow:
//!
//! Invariants (plan §1 / §3.4):
//! - **Denied ≠ empty.** A missing permission returns an error, never an
//!   empty list. "No meetings today" and "I can't see your calendar" are
//!   different sentences and must never be conflated.
//! - **Titles live only in the cache.** `calendar_cache` is disposable
//!   (TTL 5 min); titles never reach `events` or `facts`, and the audit
//!   table only records counts and outcomes.
//! - **No writes.** There is no create/update path in the trait, on purpose.
//! - **Must run inside this process.** TCC attributes permission to the
//!   responsible process; a helper binary would own a second identity and
//!   the app would still see nothing (report §3.3).
//! - **Only `fullAccess` counts.** macOS 14 returns `writeOnly` for "add
//!   only" — treating any `granted` boolean as success yields a permanently
//!   empty calendar.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDate, TimeZone, Utc};
use rusqlite::Connection;
use serde::Serialize;

use crate::memory::{clock, db, MemoryStore};

// ---------------------------------------------------------------- constants

/// Cache freshness. The reminder loop ticks every 60s but only hits EventKit
/// when this expires.
pub const CACHE_TTL_SECS: i64 = 300;
/// Reminder lead time (plan §3.6: target −10 min, tolerance ±2).
pub const LEAD_MINUTES: i64 = 10;
pub const WINDOW_SLACK_MINUTES: i64 = 2;
/// Hard cap per day: past three, a reminder becomes a notification centre.
pub const DAILY_REMIND_LIMIT: usize = 3;
/// Two meetings back to back must not produce two bubbles in a row.
pub const MIN_GAP_MINUTES: i64 = 10;
/// Quiet hours (persona: 深夜话变少).
pub const QUIET_START_HOUR: u32 = 22;
pub const QUIET_END_HOUR: u32 = 8;
/// How long the permission dialog may sit unanswered before we give the
/// command thread back (a blocked command thread would freeze the app).
pub const REQUEST_TIMEOUT_SECS: u64 = 60;
/// Calendar poll cadence (same shape as the form-line loop).
pub const POLL_SECS: u64 = 60;

pub const SETTING_ENABLED: &str = "tools.calendar.enabled";
pub const SETTING_REMIND: &str = "tools.calendar.remind";
pub const SETTING_MUTED_DATE: &str = "tools.calendar.muted_date";

// ---------------------------------------------------------------- types

/// The five states macOS 14 reports. `Full` is the only readable one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarAccess {
    NotDetermined,
    Restricted,
    Denied,
    WriteOnly,
    Full,
}

impl CalendarAccess {
    pub fn as_str(&self) -> &'static str {
        match self {
            CalendarAccess::NotDetermined => "not_determined",
            CalendarAccess::Restricted => "restricted",
            CalendarAccess::Denied => "denied",
            CalendarAccess::WriteOnly => "write_only",
            CalendarAccess::Full => "full",
        }
    }

    /// Only full access can read events. `WriteOnly` is NOT enough — this
    /// single line is the difference between working and silently empty.
    pub fn can_read(&self) -> bool {
        matches!(self, CalendarAccess::Full)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CalendarError {
    Access(CalendarAccess),
    Unavailable(String),
    Timeout,
}

impl CalendarError {
    /// Stable string codes for the frontend (it switches on these).
    pub fn code(&self) -> &'static str {
        match self {
            CalendarError::Access(CalendarAccess::Denied) => "calendar_denied",
            CalendarError::Access(CalendarAccess::Restricted) => "calendar_restricted",
            CalendarError::Access(CalendarAccess::WriteOnly) => "calendar_write_only",
            CalendarError::Access(CalendarAccess::NotDetermined) => "calendar_not_determined",
            CalendarError::Access(CalendarAccess::Full) => "calendar_unavailable",
            CalendarError::Unavailable(_) => "calendar_unavailable",
            CalendarError::Timeout => "calendar_timeout",
        }
    }
}

/// One occurrence, as the rest of the app sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventItem {
    pub external_id: String,
    pub start_at: String,
    pub end_at: String,
    pub title: String,
    pub calendar_name: Option<String>,
    pub all_day: bool,
    pub attendee_count: i64,
    pub declined: bool,
    pub canceled: bool,
    pub availability: Option<String>,
    pub reminded: bool,
}

impl EventItem {
    pub fn from_row(r: &db::CalendarRow) -> Self {
        EventItem {
            external_id: r.external_id.clone(),
            start_at: r.start_at.clone(),
            end_at: r.end_at.clone(),
            title: r.title.clone(),
            calendar_name: r.calendar_name.clone(),
            all_day: r.all_day,
            attendee_count: r.attendee_count,
            declined: r.declined,
            canceled: r.canceled,
            availability: r.availability.clone(),
            reminded: r.reminded_at.is_some(),
        }
    }

    pub fn start_local(&self) -> Option<DateTime<Local>> {
        DateTime::parse_from_rfc3339(&self.start_at)
            .ok()
            .map(|t| t.with_timezone(&Local))
    }

    /// A meeting is something with another human in it. Solo blocks
    /// ("专注时间", "写周报") must never silence the companion — that is
    /// exactly when it should be allowed to talk (plan §3.7).
    pub fn is_meeting(&self) -> bool {
        self.attendee_count >= 2 && !self.declined && !self.canceled
    }
}

/// Read-only source. There is intentionally no write method.
pub trait CalendarSource: Send + Sync {
    fn access(&self) -> CalendarAccess;
    /// Ask the system (or, in tests, pretend to). Must not block forever.
    fn request_access(&self, timeout: Duration) -> Result<CalendarAccess, CalendarError>;
    fn events_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<EventItem>, CalendarError>;
}

// ---------------------------------------------------------------- mock

/// Test / non-macOS / unsupported fallback source. Uses the same code path
/// as the real one so the permission ladder is exercised in CI.
pub struct MockCalendar {
    pub access: Mutex<CalendarAccess>,
    pub items: Mutex<Vec<EventItem>>,
    /// What `request_access` returns (None = keep current access).
    pub grant: Mutex<Option<CalendarAccess>>,
}

impl MockCalendar {
    pub fn new(access: CalendarAccess) -> Self {
        MockCalendar {
            access: Mutex::new(access),
            items: Mutex::new(Vec::new()),
            grant: Mutex::new(None),
        }
    }

    pub fn with_items(access: CalendarAccess, items: Vec<EventItem>) -> Self {
        let m = MockCalendar::new(access);
        *m.items.lock().unwrap() = items;
        m
    }

    pub fn set_grant(&self, access: CalendarAccess) {
        *self.grant.lock().unwrap() = Some(access);
    }
}

impl CalendarSource for MockCalendar {
    fn access(&self) -> CalendarAccess {
        *self.access.lock().unwrap()
    }

    fn request_access(&self, _timeout: Duration) -> Result<CalendarAccess, CalendarError> {
        if let Some(g) = *self.grant.lock().unwrap() {
            *self.access.lock().unwrap() = g;
        }
        Ok(self.access())
    }

    fn events_between(
        &self,
        _start: DateTime<Utc>,
        _end: DateTime<Utc>,
    ) -> Result<Vec<EventItem>, CalendarError> {
        let access = self.access();
        if !access.can_read() {
            return Err(CalendarError::Access(access));
        }
        Ok(self.items.lock().unwrap().clone())
    }
}

// ---------------------------------------------------------------- macOS bridge

#[cfg(target_os = "macos")]
mod ek {
    use super::*;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_event_kit::{
        EKAuthorizationStatus, EKCalendar, EKEntityType, EKEvent, EKEventStatus, EKEventStore,
        EKParticipantStatus,
    };
    use objc2_foundation::{NSArray, NSDate, NSError};

    fn map_status(s: EKAuthorizationStatus) -> CalendarAccess {
        if s == EKAuthorizationStatus::FullAccess {
            CalendarAccess::Full
        } else if s == EKAuthorizationStatus::WriteOnly {
            CalendarAccess::WriteOnly
        } else if s == EKAuthorizationStatus::Denied {
            CalendarAccess::Denied
        } else if s == EKAuthorizationStatus::Restricted {
            CalendarAccess::Restricted
        } else {
            CalendarAccess::NotDetermined
        }
    }

    pub fn current_access() -> CalendarAccess {
        // Class method, thread-safe, no store needed.
        let s = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        map_status(s)
    }

    /// Holds the app handle so the permission prompt can be dispatched to the
    /// main thread — Tauri command handlers run on a worker thread, and
    /// `requestFullAccessToEvents` only shows UI when called on the main one.
    pub struct EkCalendarSource {
        pub app: tauri::AppHandle,
    }

    impl CalendarSource for EkCalendarSource {
        fn access(&self) -> CalendarAccess {
            current_access()
        }

        fn request_access(&self, timeout: Duration) -> Result<CalendarAccess, CalendarError> {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let dispatched = self.app.run_on_main_thread(move || unsafe {
                let store = EKEventStore::new();
                let tx = tx.clone();
                let block = RcBlock::new(move |_granted: Bool, _err: *mut NSError| {
                    let _ = tx.send(());
                });
                // Apple copies the handler, so dropping our RcBlock right
                // after the call is correct.
                store.requestFullAccessToEventsWithCompletion(&block as *const _ as *mut _);
            });
            if dispatched.is_err() {
                return Err(CalendarError::Unavailable("main thread unavailable".into()));
            }
            match rx.recv_timeout(timeout) {
                Ok(()) => Ok(current_access()),
                Err(_) => Err(CalendarError::Timeout),
            }
        }

        fn events_between(
            &self,
            start: DateTime<Utc>,
            end: DateTime<Utc>,
        ) -> Result<Vec<EventItem>, CalendarError> {
            let access = current_access();
            if !access.can_read() {
                return Err(CalendarError::Access(access));
            }
            // A fresh store per query is cheap and documented behaviour:
            // later instantiations reuse the existing permission.
            unsafe {
                let store = EKEventStore::new();
                let from = NSDate::dateWithTimeIntervalSince1970(start.timestamp() as f64);
                let to = NSDate::dateWithTimeIntervalSince1970(end.timestamp() as f64);
                let predicate = store.predicateForEventsWithStartDate_endDate_calendars(
                    &from, &to, None,
                );
                let events = store.eventsMatchingPredicate(&predicate);
                Ok(collect(&events))
            }
        }
    }

    unsafe fn collect(events: &NSArray<EKEvent>) -> Vec<EventItem> {
        let mut out = Vec::new();
        for ev in events.iter() {
            let external_id = match ev.eventIdentifier() {
                Some(id) => id.to_string(),
                None => continue,
            };
            let start = NSDate::timeIntervalSince1970(&ev.startDate());
            let end = NSDate::timeIntervalSince1970(&ev.endDate());
            let start_at = DateTime::<Utc>::from_timestamp(start as i64, 0)
                .map(|t| t.to_rfc3339())
                .unwrap_or_default();
            let end_at = DateTime::<Utc>::from_timestamp(end as i64, 0)
                .map(|t| t.to_rfc3339())
                .unwrap_or_default();

            let (attendee_count, declined) = match ev.attendees() {
                Some(list) => {
                    let mut declined = false;
                    for p in list.iter() {
                        if p.isCurrentUser() && p.participantStatus() == EKParticipantStatus::Declined
                        {
                            declined = true;
                        }
                    }
                    (list.count() as i64, declined)
                }
                None => (0, false),
            };

            let availability = {
                let a = ev.availability();
                if a == objc2_event_kit::EKEventAvailability::Free {
                    Some("free".to_string())
                } else if a == objc2_event_kit::EKEventAvailability::Tentative {
                    Some("tentative".to_string())
                } else if a == objc2_event_kit::EKEventAvailability::Unavailable {
                    Some("unavailable".to_string())
                } else {
                    Some("busy".to_string())
                }
            };

            let calendar_name = ev
                .calendar()
                .map(|c: objc2::rc::Retained<EKCalendar>| c.title().to_string());

            out.push(EventItem {
                external_id,
                start_at,
                end_at,
                title: ev.title().to_string(),
                calendar_name,
                all_day: ev.isAllDay(),
                attendee_count,
                declined,
                canceled: ev.status() == EKEventStatus::Canceled,
                availability,
                reminded: false,
            });
        }
        out
    }
}

#[cfg(target_os = "macos")]
pub use ek::{current_access, EkCalendarSource};

#[cfg(not(target_os = "macos"))]
pub fn current_access() -> CalendarAccess {
    CalendarAccess::NotDetermined
}

// ---------------------------------------------------------------- managed state

pub struct CalendarState {
    pub source: Box<dyn CalendarSource>,
}

impl CalendarState {
    pub fn source(&self) -> &dyn CalendarSource {
        self.source.as_ref()
    }
}

// ---------------------------------------------------------------- config

fn setting_bool(conn: &Connection, key: &str, default: bool) -> bool {
    match db::get_setting(conn, key) {
        Ok(Some(v)) => v == "1" || v == "true",
        Ok(None) => default,
        Err(_) => default,
    }
}

pub fn is_enabled(conn: &Connection) -> bool {
    setting_bool(conn, SETTING_ENABLED, false)
}

/// Reminders default ON once the calendar itself is enabled — a user who
/// switched the calendar on wants the one thing it is for.
pub fn is_remind_on(conn: &Connection) -> bool {
    setting_bool(conn, SETTING_REMIND, true)
}

pub fn muted_today(conn: &Connection, today: NaiveDate) -> bool {
    matches!(
        db::get_setting(conn, SETTING_MUTED_DATE),
        Ok(Some(v)) if v == today.to_string()
    )
}

// ---------------------------------------------------------------- windows

fn local_midnight(day: NaiveDate) -> Option<DateTime<Local>> {
    Local.from_local_datetime(&day.and_hms_opt(0, 0, 0)?).single()
}

/// Read window: today 00:00 → day after tomorrow 00:00 (plan §1: never
/// past events, never next week).
pub fn horizon(now: DateTime<Local>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let start = local_midnight(now.date_naive())?;
    let end = local_midnight(now.date_naive() + ChronoDuration::days(2))?;
    Some((start.with_timezone(&Utc), end.with_timezone(&Utc)))
}

// ---------------------------------------------------------------- cache

fn row_from_item(item: &EventItem) -> db::CalendarRow {
    db::CalendarRow {
        external_id: item.external_id.clone(),
        start_at: item.start_at.clone(),
        end_at: item.end_at.clone(),
        title: item.title.clone(),
        calendar_name: item.calendar_name.clone(),
        all_day: item.all_day,
        attendee_count: item.attendee_count,
        declined: item.declined,
        canceled: item.canceled,
        availability: item.availability.clone(),
        reminded_at: None,
        fetched_at: String::new(),
    }
}

/// Is the cache older than the TTL? Missing/играunparsable stamps always
/// mean "stale" — a cache we cannot date is a cache we cannot trust.
pub fn cache_is_stale(conn: &Connection, now: DateTime<Local>) -> bool {
    let last = match db::calendar_last_fetch(conn) {
        Ok(Some(v)) => v,
        _ => return true,
    };
    match DateTime::parse_from_rfc3339(&last) {
        Ok(t) => (now.with_timezone(&Utc) - t.with_timezone(&Utc)).num_seconds() >= CACHE_TTL_SECS,
        Err(_) => true,
    }
}

/// Pull one window into the cache. Errors bubble up — never `unwrap_or_default`.
pub fn refresh(
    conn: &mut Connection,
    source: &dyn CalendarSource,
    now: DateTime<Local>,
) -> Result<usize, CalendarError> {
    let (start, end) = horizon(now).ok_or_else(|| CalendarError::Unavailable("bad clock".into()))?;
    let items = source.events_between(start, end)?;
    let rows: Vec<db::CalendarRow> = items.iter().map(row_from_item).collect();
    let stamp = clock::now_iso();
    db::calendar_replace(conn, &rows, &stamp)
        .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
    Ok(rows.len())
}

/// Refresh only when the TTL expired. Returns `Ok(false)` = served from cache.
pub fn refresh_if_stale(
    conn: &mut Connection,
    source: &dyn CalendarSource,
    now: DateTime<Local>,
) -> Result<bool, CalendarError> {
    if !cache_is_stale(conn, now) {
        return Ok(false);
    }
    refresh(conn, source, now)?;
    Ok(true)
}

pub fn agenda(
    conn: &Connection,
    start: DateTime<Local>,
    end: DateTime<Local>,
) -> Result<Vec<EventItem>, CalendarError> {
    let rows = db::calendar_between(
        conn,
        &start.with_timezone(&Utc).to_rfc3339(),
        &end.with_timezone(&Utc).to_rfc3339(),
    )
    .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
    Ok(rows.iter().map(EventItem::from_row).collect())
}

/// Events overlapping `now` for a given day, oldest first.
pub fn agenda_for_day(
    conn: &Connection,
    day: NaiveDate,
) -> Result<Vec<EventItem>, CalendarError> {
    let start = local_midnight(day).ok_or_else(|| CalendarError::Unavailable("bad day".into()))?;
    let end = local_midnight(day + ChronoDuration::days(1))
        .ok_or_else(|| CalendarError::Unavailable("bad day".into()))?;
    agenda(conn, start, end)
}

/// L4 input: is the user in a real meeting right now? Reads the cache only.
/// No permission → `false`: a wrong silence is worse than a wrong word, but
/// a permanent silence is a bug, so the UI must show the missing permission.
pub fn in_meeting_now(conn: &Connection, now: DateTime<Local>) -> bool {
    let iso = now.with_timezone(&Utc).to_rfc3339();
    match db::calendar_between(conn, &iso, &iso) {
        Ok(rows) => rows
            .iter()
            .map(EventItem::from_row)
            .any(|e| !e.all_day && e.is_meeting()),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------- reminders

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reminder {
    /// Not shown to the user — used to mark the cached row as announced.
    #[serde(skip)]
    pub external_id: String,
    pub title: String,
    pub start_at: String,
    pub minutes_left: i64,
    pub calendar_name: Option<String>,
}

/// Everything the pure reminder picker needs (no clock, no DB inside).
pub struct ReminderContext {
    pub today_count: usize,
    pub last_remind_at: Option<DateTime<Local>>,
    pub muted_today: bool,
}

/// Pick at most one occurrence to announce. Pure: the four gates from
/// plan §3.6 are all decided here, so the CI test can drive every branch.
pub fn pick_reminder(
    now: DateTime<Local>,
    rows: &[db::CalendarRow],
    ctx: &ReminderContext,
) -> Option<Reminder> {
    if ctx.muted_today || ctx.today_count >= DAILY_REMIND_LIMIT {
        return None;
    }
    let hour = chrono::Timelike::hour(&now);
    if hour >= QUIET_START_HOUR || hour < QUIET_END_HOUR {
        return None;
    }
    if let Some(last) = ctx.last_remind_at {
        let gap = now.with_timezone(&Utc) - last.with_timezone(&Utc);
        if gap.num_minutes() < MIN_GAP_MINUTES {
            return None;
        }
    }

    for r in rows {
        if r.all_day || r.declined || r.canceled || r.reminded_at.is_some() {
            continue;
        }
        // A "free" block with nobody else in it is not worth a word.
        if r.availability.as_deref() == Some("free") && r.attendee_count == 0 {
            continue;
        }
        let start = match DateTime::parse_from_rfc3339(&r.start_at) {
            Ok(t) => t.with_timezone(&Local),
            Err(_) => continue,
        };
        let minutes_left = (start.with_timezone(&Utc) - now.with_timezone(&Utc)).num_minutes();
        // Already started (or over): never announce a meeting in progress.
        if minutes_left < LEAD_MINUTES - WINDOW_SLACK_MINUTES {
            continue;
        }
        // Too far out: rows are sorted by start, so everything after is later.
        if minutes_left > LEAD_MINUTES + WINDOW_SLACK_MINUTES {
            break;
        }
        return Some(Reminder {
            external_id: r.external_id.clone(),
            title: r.title.clone(),
            start_at: r.start_at.clone(),
            minutes_left,
            calendar_name: r.calendar_name.clone(),
        });
    }
    None
}

/// One reminder tick, testable without a runtime: refresh-if-stale → pick →
/// mark → audit. Returns the reminder that should be announced, if any.
pub fn tick(
    conn: &mut Connection,
    source: &dyn CalendarSource,
    now: DateTime<Local>,
) -> Result<Option<Reminder>, CalendarError> {
    if !is_enabled(conn) || !is_remind_on(conn) {
        return Ok(None);
    }
    let refreshed = match refresh_if_stale(conn, source, now) {
        Ok(v) => v,
        Err(e) => {
            // A revocation must not leave yesterday's titles lying around.
            if matches!(e, CalendarError::Access(_)) {
                let _ = db::clear_calendar_cache(conn);
            }
            return Err(e);
        }
    };
    if refreshed {
        let _ = db::log_tool_call(conn, "calendar", "refresh", "ok", None, None);
    }

    if muted_today(conn, now.date_naive()) {
        return Ok(None);
    }

    let (day_start, day_end) = crate::report::local_day_window(now.date_naive())
        .ok_or_else(|| CalendarError::Unavailable("bad day".into()))?;
    // Only successful "remind" rows count; the refresh rows share the tool name.
    let today_count = count_reminders_today(conn, &day_start, &day_end);

    let last_remind_at = db::last_tool_call_ts(conn, "calendar", "remind")
        .ok()
        .flatten()
        .and_then(|ts| DateTime::parse_from_rfc3339(&ts).ok())
        .map(|t| t.with_timezone(&Local));

    let rows = db::calendar_between(
        conn,
        &now.with_timezone(&Utc).to_rfc3339(),
        &(now.with_timezone(&Utc) + ChronoDuration::hours(12)).to_rfc3339(),
    )
    .map_err(|e| CalendarError::Unavailable(e.to_string()))?;

    let ctx = ReminderContext {
        today_count,
        last_remind_at,
        muted_today: false,
    };
    let Some(reminder) = pick_reminder(now, &rows, &ctx) else {
        return Ok(None);
    };

    if !db::mark_calendar_reminded(conn, &reminder.external_id, &reminder.start_at)
        .unwrap_or(false)
    {
        return Ok(None); // already announced
    }
    let _ = db::log_tool_call(
        conn,
        "calendar",
        "remind",
        "ok",
        Some(&format!("{} min ahead", reminder.minutes_left)),
        None,
    );
    Ok(Some(reminder))
}

/// Count audit rows in `[start_iso, end_iso)` — parsed, not string-compared:
/// RFC3339 strings with different offsets do not sort lexicographically.
fn count_audit(
    conn: &Connection,
    tool: &str,
    action: Option<&str>,
    start_iso: &str,
    end_iso: &str,
) -> i64 {
    let start = match DateTime::parse_from_rfc3339(start_iso) {
        Ok(t) => t.with_timezone(&Utc),
        Err(_) => return 0,
    };
    let end = match DateTime::parse_from_rfc3339(end_iso) {
        Ok(t) => t.with_timezone(&Utc),
        Err(_) => return 0,
    };
    db::tool_calls_recent(conn, 500)
        .map(|rows| {
            rows.into_iter()
                .filter(|(ts, t, a, outcome, _)| {
                    if t != tool || outcome != "ok" {
                        return false;
                    }
                    if let Some(want) = action {
                        if a != want {
                            return false;
                        }
                    }
                    match DateTime::parse_from_rfc3339(ts) {
                        Ok(when) => {
                            let when = when.with_timezone(&Utc);
                            when >= start && when < end
                        }
                        Err(_) => false,
                    }
                })
                .count() as i64
        })
        .unwrap_or(0)
}

fn count_reminders_today(conn: &Connection, start_iso: &str, end_iso: &str) -> usize {
    count_audit(conn, "calendar", Some("remind"), start_iso, end_iso) as usize
}

// ---------------------------------------------------------------- runtime

pub fn spawn(app: tauri::AppHandle) {
    use tauri::Manager;
    // plan-loop-supervisor T1/D3：calendar 计入齐套；双层守卫；CAP 留痕；D1 有界复活。
    crate::failure::spawn_guarded("calendar", move || {
        crate::failure::try_register(&app, "calendar");
        let mut guard = crate::failure::PanicGuard::new("calendar");
        loop {
            std::thread::sleep(Duration::from_secs(POLL_SECS));
            crate::failure::try_heartbeat(&app, "calendar");
            let _ = guard.tick(|| {
                let Some(store) = app.try_state::<MemoryStore>() else {
                    return;
                };
                let Some(state) = app.try_state::<CalendarState>() else {
                    return;
                };
                let Ok(mut conn) = store.inner().0.lock() else {
                    return;
                };
                let now = Local::now();
                match tick(&mut conn, state.inner().source(), now) {
                    Ok(Some(r)) => {
                        // The bubble carries a title, so it goes to the webview only —
                        // never to the events table, never to the tray tooltip.
                        drop(conn);
                        use tauri::Emitter;
                        let _ = app.emit("calendar:remind", r);
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            });
            if guard.stopped() {
                if crate::failure::on_loop_cap(&app, "calendar", "consecutive panic cap") {
                    spawn(app.clone());
                }
                break;
            }
        }
    });
}

// ---------------------------------------------------------------- status

#[derive(Debug, Clone, Serialize)]
pub struct CalendarStatus {
    pub access: &'static str,
    pub can_read: bool,
    pub enabled: bool,
    pub remind: bool,
    pub muted_today: bool,
    pub last_fetch: Option<String>,
    pub cache_count: usize,
    pub today_reads: i64,
    pub today_reminds: i64,
    pub next: Option<EventItem>,
    /// True when the platform has no calendar bridge at all.
    pub supported: bool,
}

pub fn status(conn: &Connection, source: &dyn CalendarSource, now: DateTime<Local>) -> CalendarStatus {
    let access = source.access();
    let (day_start, day_end) = crate::report::local_day_window(now.date_naive())
        .map(|(a, b)| (a, b))
        .unwrap_or_default();
    let today_reads = ["agenda", "agenda_today", "agenda_tomorrow", "next", "search"]
        .iter()
        .map(|a| count_audit(conn, "calendar", Some(a), &day_start, &day_end))
        .sum();
    let today_reminds = count_reminders_today(conn, &day_start, &day_end) as i64;
    let next = db::calendar_next_after(conn, &now.with_timezone(&Utc).to_rfc3339())
        .ok()
        .flatten()
        .map(|r| EventItem::from_row(&r));
    let cache_count = db::calendar_between(
        conn,
        "1970-01-01T00:00:00+00:00",
        "2999-01-01T00:00:00+00:00",
    )
    .map(|r| r.len())
    .unwrap_or(0);
    CalendarStatus {
        access: access.as_str(),
        can_read: access.can_read(),
        enabled: is_enabled(conn),
        remind: is_remind_on(conn),
        muted_today: muted_today(conn, now.date_naive()),
        last_fetch: db::calendar_last_fetch(conn).ok().flatten(),
        cache_count,
        today_reads,
        today_reminds,
        next,
        supported: cfg!(target_os = "macos"),
    }
}

/// Record a read intent (agenda/next/search) so the tools page can show
/// "it read your calendar 3 times today" without storing any content.
pub fn log_read(conn: &Connection, action: &str, outcome: &str, count: usize, ms: u128) {
    let detail = format!("{count} events, {ms}ms");
    let _ = db::log_tool_call(conn, "calendar", action, outcome, Some(&detail), Some(ms as i64));
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn calendar_status(
    store: tauri::State<'_, MemoryStore>,
    state: tauri::State<'_, CalendarState>,
) -> Result<CalendarStatus, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    Ok(status(&conn, state.inner().source(), Local::now()))
}

#[tauri::command]
pub fn calendar_request_access(
    store: tauri::State<'_, MemoryStore>,
    state: tauri::State<'_, CalendarState>,
) -> Result<CalendarStatus, String> {
    let started = Instant::now();
    let result = state
        .inner()
        .source()
        .request_access(Duration::from_secs(REQUEST_TIMEOUT_SECS));
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    match result {
        Ok(access) => {
            let _ = db::log_tool_call(
                &conn,
                "calendar",
                "authorize",
                if access.can_read() { "ok" } else { "denied" },
                Some(access.as_str()),
                Some(started.elapsed().as_millis() as i64),
            );
            // Switching permission off upstream must not leave titles cached.
            if !access.can_read() {
                let _ = db::clear_calendar_cache(&conn);
            }
            Ok(status(&conn, state.inner().source(), Local::now()))
        }
        Err(e) => {
            let _ = db::log_tool_call(
                &conn,
                "calendar",
                "authorize",
                "error",
                Some(e.code()),
                Some(started.elapsed().as_millis() as i64),
            );
            Err(e.code().to_string())
        }
    }
}

/// Turn the calendar on/off and the reminder switch. Turning it off clears
/// the cache immediately (nothing of the calendar survives the switch).
#[tauri::command]
pub fn calendar_set_config(
    store: tauri::State<'_, MemoryStore>,
    state: tauri::State<'_, CalendarState>,
    enabled: Option<bool>,
    remind: Option<bool>,
) -> Result<CalendarStatus, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    if let Some(v) = enabled {
        db::set_setting(&conn, SETTING_ENABLED, if v { "1" } else { "0" })
            .map_err(|e| e.to_string())?;
        if !v {
            let _ = db::clear_calendar_cache(&conn);
        }
    }
    if let Some(v) = remind {
        db::set_setting(&conn, SETTING_REMIND, if v { "1" } else { "0" })
            .map_err(|e| e.to_string())?;
    }
    Ok(status(&conn, state.inner().source(), Local::now()))
}

#[tauri::command]
pub fn calendar_clear_cache(
    store: tauri::State<'_, MemoryStore>,
    state: tauri::State<'_, CalendarState>,
) -> Result<CalendarStatus, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    db::clear_calendar_cache(&conn).map_err(|e| e.to_string())?;
    Ok(status(&conn, state.inner().source(), Local::now()))
}

/// "Don't remind me again today" — the control the user gets on every bubble.
#[tauri::command]
pub fn calendar_mute_today(
    store: tauri::State<'_, MemoryStore>,
    state: tauri::State<'_, CalendarState>,
) -> Result<CalendarStatus, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    db::set_setting(&conn, SETTING_MUTED_DATE, &Local::now().date_naive().to_string())
        .map_err(|e| e.to_string())?;
    Ok(status(&conn, state.inner().source(), Local::now()))
}

/// Deep link into the calendar privacy pane. macOS has no programmatic grant
/// path — after a denial the user MUST flip the switch themselves (report
/// §3.3 #1), and sending them to the wrong pane is a support ticket.
#[tauri::command]
pub fn calendar_open_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars")
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("calendar settings are macOS only".into())
    }
}

/// Structured agenda for the conversation layer. Errors carry stable codes —
/// `"calendar_denied"` is a sentence, `Ok(vec![])` would be a lie.
pub fn agenda_for_intent(
    conn: &Connection,
    access: CalendarAccess,
    action: &str,
    query: Option<&str>,
    now: DateTime<Local>,
) -> Result<Vec<EventItem>, CalendarError> {
    let started = Instant::now();
    if !access.can_read() {
        log_read(conn, action, "denied", 0, started.elapsed().as_millis());
        return Err(CalendarError::Access(access));
    }
    let days: Vec<NaiveDate> = match action {
        "agenda_tomorrow" => vec![now.date_naive() + ChronoDuration::days(1)],
        "agenda_today" | "next" => vec![now.date_naive()],
        _ => vec![now.date_naive(), now.date_naive() + ChronoDuration::days(1)],
    };
    let mut items = Vec::new();
    for day in days {
        match agenda_for_day(conn, day) {
            Ok(list) => items.extend(list),
            Err(e) => {
                log_read(conn, action, e.code(), 0, started.elapsed().as_millis());
                return Err(e);
            }
        }
    }
    if let Some(q) = query {
        let q = q.to_lowercase();
        items.retain(|e| e.title.to_lowercase().contains(&q));
    }
    log_read(conn, action, "ok", items.len(), started.elapsed().as_millis());
    Ok(items)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
       Connection::open_in_memory().unwrap_or_else(|e| panic!("{e}"))
    }

    fn item(id: &str, start_iso: &str, attendees: i64, all_day: bool) -> EventItem {
        EventItem {
            external_id: id.into(),
            start_at: start_iso.into(),
            end_at: start_iso.into(),
            title: format!("会 {id}"),
            calendar_name: None,
            all_day,
            attendee_count: attendees,
            declined: false,
            canceled: false,
            availability: Some("busy".into()),
            reminded: false,
        }
    }

    fn row(id: &str, start_iso: &str, attendees: i64) -> db::CalendarRow {
        db::CalendarRow {
            external_id: id.into(),
            start_at: start_iso.into(),
            end_at: start_iso.into(),
            title: format!("会 {id}"),
            calendar_name: None,
            all_day: false,
            attendee_count: attendees,
            declined: false,
            canceled: false,
            availability: Some("busy".into()),
            reminded_at: None,
            fetched_at: String::new(),
        }
    }

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap()
    }

    fn ctx(today: usize, last: Option<DateTime<Local>>, muted: bool) -> ReminderContext {
        ReminderContext { today_count: today, last_remind_at: last, muted_today: muted }
    }

    #[test]
    fn denied_is_an_error_not_an_empty_list() {
        let source = MockCalendar::new(CalendarAccess::Denied);
        let mut conn = mem();
        db::init_schema_for_test(&conn).unwrap();
        let err = refresh(&mut conn, &source, at(2026, 11, 19, 9, 0)).unwrap_err();
        assert_eq!(err.code(), "calendar_denied");
        // And the write-only trap: "granted" but unreadable.
        let wo = MockCalendar::new(CalendarAccess::WriteOnly);
        assert_eq!(wo.access(), CalendarAccess::WriteOnly);
        assert!(!wo.access().can_read());
    }

    #[test]
    fn write_only_never_reads_even_though_it_was_granted() {
        let src = MockCalendar::new(CalendarAccess::WriteOnly);
        let err = src
            .events_between(Utc::now(), Utc::now())
            .unwrap_err();
        assert_eq!(err.code(), "calendar_write_only");
    }

    #[test]
    fn in_meeting_needs_a_second_human() {
        let mut conn = mem();
        db::init_schema_for_test(&conn).unwrap();
        let now = at(2026, 11, 19, 14, 5);
        let start = (now - ChronoDuration::minutes(5)).with_timezone(&Utc).to_rfc3339();
        let end = (now + ChronoDuration::minutes(25)).with_timezone(&Utc).to_rfc3339();
        let mut r = row("solo", &start, 1);
        r.start_at = start.clone();
        r.end_at = end.clone();
        db::calendar_replace(&mut conn, &[r], "2026-11-19T06:00:00+00:00").unwrap();
        // A solo "focus block" must not silence the companion.
        assert!(!in_meeting_now(&conn, now));

        let mut r2 = row("team", &start, 4);
        r2.start_at = start;
        r2.end_at = end;
        db::calendar_replace(&mut conn, &[r2], "2026-11-19T06:01:00+00:00").unwrap();
        assert!(in_meeting_now(&conn, now));
    }

    #[test]
    fn reminder_window_is_ten_plus_minus_two() {
        let now = at(2026, 11, 19, 9, 0);
        // 16 min out → too early (break, no reminder).
        let rows = vec![row("far", &(now + ChronoDuration::minutes(16)).with_timezone(&Utc).to_rfc3339(), 3)];
        assert!(pick_reminder(now, &rows, &ctx(0, None, false)).is_none());

        // 11 min out → inside the window.
        let rows = vec![row("near", &(now + ChronoDuration::minutes(11)).with_timezone(&Utc).to_rfc3339(), 3)];
        let r = pick_reminder(now, &rows, &ctx(0, None, false)).unwrap();
        assert_eq!(r.minutes_left, 11);

        // 9 min out → still inside (±2 tolerance).
        let rows = vec![row("near", &(now + ChronoDuration::minutes(9)).with_timezone(&Utc).to_rfc3339(), 3)];
        assert!(pick_reminder(now, &rows, &ctx(0, None, false)).is_some());

        // Already started 3 min ago → never announce a meeting in progress.
        let rows = vec![row("started", &(now - ChronoDuration::minutes(3)).with_timezone(&Utc).to_rfc3339(), 3)];
        assert!(pick_reminder(now, &rows, &ctx(0, None, false)).is_none());
    }

    #[test]
    fn reminder_respects_quota_gap_quiet_hours_and_mute() {
        let now = at(2026, 11, 19, 9, 0);
        let soon = (now + ChronoDuration::minutes(10)).with_timezone(&Utc).to_rfc3339();
        let rows = vec![row("a", &soon, 3)];

        assert!(pick_reminder(now, &rows, &ctx(3, None, false)).is_none(), "daily cap");
        assert!(pick_reminder(now, &rows, &ctx(0, None, true)).is_none(), "muted");
        assert!(
            pick_reminder(now, &rows, &ctx(0, Some(now - ChronoDuration::minutes(4)), false)).is_none(),
            "min gap"
        );
        assert!(
            pick_reminder(now, &rows, &ctx(0, Some(now - ChronoDuration::minutes(11)), false)).is_some(),
            "gap cleared"
        );

        // Quiet hours: 23:00 and 07:00 say nothing.
        let late = at(2026, 11, 19, 23, 0);
        let rows_late = vec![row("b", &(late + ChronoDuration::minutes(10)).with_timezone(&Utc).to_rfc3339(), 3)];
        assert!(pick_reminder(late, &rows_late, &ctx(0, None, false)).is_none());
        let early = at(2026, 11, 19, 7, 0);
        let rows_early = vec![row("c", &(early + ChronoDuration::minutes(10)).with_timezone(&Utc).to_rfc3339(), 3)];
        assert!(pick_reminder(early, &rows_early, &ctx(0, None, false)).is_none());
    }

    #[test]
    fn all_day_declined_canceled_and_free_blocks_stay_silent() {
        let now = at(2026, 11, 19, 9, 0);
        let soon = (now + ChronoDuration::minutes(10)).with_timezone(&Utc).to_rfc3339();

        let mut all_day = row("x", &soon, 3);
        all_day.all_day = true;
        assert!(pick_reminder(now, &[all_day], &ctx(0, None, false)).is_none());

        let mut declined = row("x", &soon, 3);
        declined.declined = true;
        assert!(pick_reminder(now, &[declined], &ctx(0, None, false)).is_none());

        let mut canceled = row("x", &soon, 3);
        canceled.canceled = true;
        assert!(pick_reminder(now, &[canceled], &ctx(0, None, false)).is_none());

        let mut free_solo = row("x", &soon, 0);
        free_solo.availability = Some("free".into());
        assert!(pick_reminder(now, &[free_solo], &ctx(0, None, false)).is_none());
    }

    #[test]
    fn tick_reminds_once_and_marks_the_row() {
        let mut conn = mem();
        db::init_schema_for_test(&conn).unwrap();
        let now = at(2026, 11, 19, 9, 0);
        let source = MockCalendar::with_items(
            CalendarAccess::Full,
            vec![item("m1", &(now + ChronoDuration::minutes(10)).with_timezone(&Utc).to_rfc3339(), 3, false)],
        );
        db::set_setting(&conn, SETTING_ENABLED, "1").unwrap();
        db::set_setting(&conn, SETTING_REMIND, "1").unwrap();

        let first = tick(&mut conn, &source, now).unwrap();
        assert!(first.is_some());
        // Second tick in the same minute: the row is marked, nothing fires.
        let second = tick(&mut conn, &source, now).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn tick_stays_silent_when_switched_off() {
        let mut conn = mem();
        db::init_schema_for_test(&conn).unwrap();
        let now = at(2026, 11, 19, 9, 0);
        let source = MockCalendar::with_items(
            CalendarAccess::Full,
            vec![item("m1", &(now + ChronoDuration::minutes(10)).with_timezone(&Utc).to_rfc3339(), 3, false)],
        );
        assert!(tick(&mut conn, &source, now).unwrap().is_none());
    }

    #[test]
    fn horizon_covers_today_and_tomorrow_only() {
        let (start, end) = horizon(at(2026, 11, 19, 15, 30)).unwrap();
        let s = start.with_timezone(&Local);
        let e = end.with_timezone(&Local);
        assert_eq!(s.format("%Y-%m-%d %H:%M").to_string(), "2026-11-19 00:00");
        assert_eq!(e.format("%Y-%m-%d %H:%M").to_string(), "2026-11-21 00:00");
    }
}
