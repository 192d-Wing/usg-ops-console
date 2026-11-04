//! Audit events: one append-only row per auth event (via the restricted
//! audit pool) plus the same fields as a `tracing` JSON line (`target=audit`).

use std::net::IpAddr;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::PgPool;
use tokio::sync::{mpsc, oneshot};

/// Bound of the background audit queue (PERF-002). Overflow drops the DB row
/// (the tracing line is always written first) and counts it.
pub const QUEUE_CAPACITY: usize = 1024;
/// Max rows per background `INSERT`.
const BATCH: usize = 64;

enum Msg {
    Event(Box<AuditEvent>),
    Flush(oneshot::Sender<()>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    LoginSuccess,
    LoginFailure,
    LoginDenied,
    Logout,
    ExpiredIdle,
    ExpiredAbsolute,
    Refresh,
    Revoked,
    AccessDenied,
    /// `POST /api/tools/{id}/launch` succeeded (a role denial is `ToolDenied`).
    ToolLaunch,
    /// A proxied document navigation on a tool origin.
    ToolAccess,
    /// Tool-origin refusal: role, bad launch code, misdirected, spoofing.
    ToolDenied,
    /// A proxied state-changing request (registry `audit_writes`).
    ToolRequest,
    /// Start-up: the tool registry was loaded (one event per gateway start).
    RegistryLoaded,
    /// Start-up: a tool uses `insecure_http_upstream` (NIST-003, CM-6
    /// deviation); one event per such tool.
    RegistryInsecureUpstream,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoginSuccess => "login.success",
            Self::LoginFailure => "login.failure",
            Self::LoginDenied => "login.denied",
            Self::Logout => "logout",
            Self::ExpiredIdle => "session.expired.idle",
            Self::ExpiredAbsolute => "session.expired.absolute",
            Self::Refresh => "session.refresh",
            Self::Revoked => "session.revoked",
            Self::AccessDenied => "access.denied",
            Self::ToolLaunch => "tool.launch",
            Self::ToolAccess => "tool.access",
            Self::ToolDenied => "tool.denied",
            Self::ToolRequest => "tool.request",
            Self::RegistryLoaded => "registry.loaded",
            Self::RegistryInsecureUpstream => "registry.insecure_upstream",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    Denied,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }
}

/// Who/what/when/where/outcome. `reason` is a fixed machine code; no field
/// ever carries token text.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub occurred_at: DateTime<Utc>,
    pub event_type: EventType,
    pub outcome: Outcome,
    pub user_sub: Option<String>,
    pub username: Option<String>,
    pub session_ref: Option<String>,
    pub client_ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub reason: Option<&'static str>,
    pub request_id: Option<String>,
    /// Registry tool id (tool events only).
    pub tool_id: Option<String>,
    /// Request path without the query string (tool events only).
    pub path: Option<String>,
    /// HTTP status answered to the browser (tool events only).
    pub http_status: Option<u16>,
    /// Upstream time to response headers (proxied tool events only).
    pub latency_ms: Option<u64>,
}

impl AuditEvent {
    pub fn new(occurred_at: DateTime<Utc>, event_type: EventType, outcome: Outcome) -> Self {
        Self {
            occurred_at,
            event_type,
            outcome,
            user_sub: None,
            username: None,
            session_ref: None,
            client_ip: None,
            user_agent: None,
            reason: None,
            request_id: None,
            tool_id: None,
            path: None,
            http_status: None,
            latency_ms: None,
        }
    }
}

#[derive(Clone)]
pub struct AuditSink {
    pool: PgPool,
    queue: Option<mpsc::Sender<Msg>>,
    dropped: Arc<AtomicU64>,
    drop_log: Arc<DropLog>,
}

/// Rate limit for the "row dropped" error line: the first drop is logged at
/// once, later drops at most every [`DROP_LOG_INTERVAL`] as one summary.
struct DropLog {
    base: std::time::Instant,
    /// Millis since `base` of the last line; `u64::MAX` = never logged.
    last_ms: AtomicU64,
    /// Drops since the last line.
    pending: AtomicU64,
}

const DROP_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

impl AuditSink {
    /// `pool` must connect as the `console_audit` role (INSERT+SELECT only).
    /// Inside a tokio runtime this also starts the background writer used by
    /// [`AuditSink::record_background`].
    pub fn new(pool: PgPool) -> Self {
        let queue = tokio::runtime::Handle::try_current().ok().map(|rt| {
            let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
            rt.spawn(writer(pool.clone(), rx));
            tx
        });
        Self {
            pool,
            queue,
            dropped: Arc::new(AtomicU64::new(0)),
            drop_log: Arc::new(DropLog {
                base: std::time::Instant::now(),
                last_ms: AtomicU64::new(u64::MAX),
                pending: AtomicU64::new(0),
            }),
        }
    }

    /// DB rows dropped because the background queue was full or closed.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Best-effort audit off the response path (PERF-002): the tracing line
    /// is written now, the DB row by a background writer through a bounded
    /// queue. Overflow drops the row, counts it ([`AuditSink::dropped`]) and
    /// logs at `error` (first drop, then a summary at most every 10 s). Only for events whose loss must not delay or fail
    /// the request (tool navigations/writes/denials on the proxy path);
    /// fail-closed events keep using [`AuditSink::record`].
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AU-2/AU-12 — every event still produces the tracing record synchronously
    // - NIST 800-53: AU-5 — queue overflow is counted and logged at error for alerting
    // POAM: AU-5 - background audit rows can be dropped under sustained overload (counted and logged; no metric export yet).
    pub fn record_background(&self, ev: AuditEvent) {
        let _ = emit(&ev);
        let t = ev.event_type;
        let Some(q) = &self.queue else {
            self.drop_row(t);
            return;
        };
        if q.try_send(Msg::Event(Box::new(ev))).is_err() {
            self.drop_row(t);
        }
    }

    fn drop_row(&self, t: EventType) {
        let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        self.drop_log.pending.fetch_add(1, Ordering::Relaxed);
        if let Some(count) = self.drop_log.take_due() {
            tracing::error!(
                target: "audit",
                event_type = t.as_str(),
                dropped_since_last_log = count,
                dropped_total = total,
                "audit queue full or closed; DB rows dropped"
            );
        }
    }

    /// Waits until every event queued before this call has been written
    /// (tests, graceful shutdown).
    pub async fn flush(&self) {
        let Some(q) = &self.queue else { return };
        let (tx, rx) = oneshot::channel();
        if q.send(Msg::Flush(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    // COMPLIANCE MAPPING:
    // - NIST 800-53: AU-2 (Event Logging), AU-3 (Content of Audit Records), AU-12 (Audit Record Generation)
    // - NIST 800-53: AU-8 (Time Stamps) — UTC RFC 3339
    // - NIST 800-53: AU-5 (Response to Audit Logging Process Failures) — callers fail closed on Err for grants
    /// Emits the tracing line first (so a DB failure still leaves a record),
    /// then inserts the row.
    pub async fn record(&self, ev: &AuditEvent) -> Result<(), sqlx::Error> {
        let ip = emit(ev);
        let res = sqlx::query(
            "INSERT INTO audit_events (occurred_at, event_type, outcome, user_sub, username, \
             session_ref, client_ip, user_agent, reason, request_id, tool_id, path, \
             http_status, latency_ms) \
             VALUES ($1, $2, $3, $4, $5, $6, $7::inet, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(ev.occurred_at)
        .bind(ev.event_type.as_str())
        .bind(ev.outcome.as_str())
        .bind(ev.user_sub.as_deref())
        .bind(ev.username.as_deref())
        .bind(ev.session_ref.as_deref())
        .bind(ip.as_deref())
        .bind(ev.user_agent.as_deref())
        .bind(ev.reason)
        .bind(ev.request_id.as_deref())
        .bind(ev.tool_id.as_deref())
        .bind(ev.path.as_deref())
        .bind(ev.http_status.map(i32::from))
        .bind(ev.latency_ms.map(clamp_i64))
        .execute(&self.pool)
        .await;
        if let Err(e) = &res {
            write_failed(ev.event_type, 1, e);
        }
        res.map(|_| ())
    }

    /// For events where refusing the action would be worse than a missing
    /// DB row (logout, expiry, denials, refresh): the tracing line is always
    /// emitted and a DB failure is logged at `error` (`target=audit`,
    /// "audit write failed") for alerting.
    // POAM: AU-5 - logout/expiry/denial/refresh/revocation audit writes do not fail closed; DB-write failures are
    // only logged at error level (no metric/alert wired yet). Login success does fail closed.
    pub async fn record_best_effort(&self, ev: &AuditEvent) {
        let _ = self.record(ev).await;
    }

    /// One multi-row `INSERT ... SELECT FROM UNNEST` for a batch (sweeper,
    /// PERF-002). Each event still gets its own tracing line first.
    pub async fn record_many(&self, evs: &[AuditEvent]) -> Result<(), sqlx::Error> {
        let ips: Vec<Option<String>> = evs.iter().map(emit).collect();
        insert_rows(&self.pool, evs, ips).await
    }
}

impl DropLog {
    /// `Some(drops since the last line)` when a line is due now.
    fn take_due(&self) -> Option<u64> {
        let now = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        let last = self.last_ms.load(Ordering::Relaxed);
        let interval = u64::try_from(DROP_LOG_INTERVAL.as_millis()).unwrap_or(u64::MAX);
        let due = last == u64::MAX || now.saturating_sub(last) >= interval;
        if !due {
            return None;
        }
        // One winner per interval under concurrent drops.
        self.last_ms
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .ok()?;
        Some(self.pending.swap(0, Ordering::Relaxed))
    }
}

/// Background writer: drains the queue in batches of up to [`BATCH`].
async fn writer(pool: PgPool, mut rx: mpsc::Receiver<Msg>) {
    let mut batch: Vec<AuditEvent> = Vec::with_capacity(BATCH);
    let mut flushes = Vec::new();
    while let Some(first) = rx.recv().await {
        let mut next = Some(first);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Event(ev) => batch.push(*ev),
                Msg::Flush(done) => flushes.push(done),
            }
            if batch.len() < BATCH {
                next = rx.try_recv().ok();
            }
        }
        if !batch.is_empty() {
            let ips = batch
                .iter()
                .map(|e| e.client_ip.map(|ip| ip.to_string()))
                .collect();
            // Failure is logged by insert_rows ("audit write failed").
            let _ = insert_rows(&pool, &batch, ips).await;
            batch.clear();
        }
        for done in flushes.drain(..) {
            let _ = done.send(());
        }
    }
}

/// The batch `INSERT` without tracing lines (callers emit them).
async fn insert_rows(
    pool: &PgPool,
    evs: &[AuditEvent],
    ip: Vec<Option<String>>,
) -> Result<(), sqlx::Error> {
    if evs.is_empty() {
        return Ok(());
    }
    let mut at = Vec::with_capacity(evs.len());
    let mut ty = Vec::with_capacity(evs.len());
    let mut out = Vec::with_capacity(evs.len());
    let mut sub = Vec::with_capacity(evs.len());
    let mut user = Vec::with_capacity(evs.len());
    let mut sref = Vec::with_capacity(evs.len());
    let mut ua = Vec::with_capacity(evs.len());
    let mut reason = Vec::with_capacity(evs.len());
    let mut rid = Vec::with_capacity(evs.len());
    let mut tool = Vec::with_capacity(evs.len());
    let mut path = Vec::with_capacity(evs.len());
    let mut status = Vec::with_capacity(evs.len());
    let mut latency = Vec::with_capacity(evs.len());
    for ev in evs {
        at.push(ev.occurred_at);
        ty.push(ev.event_type.as_str());
        out.push(ev.outcome.as_str());
        sub.push(ev.user_sub.clone());
        user.push(ev.username.clone());
        sref.push(ev.session_ref.clone());
        ua.push(ev.user_agent.clone());
        reason.push(ev.reason);
        rid.push(ev.request_id.clone());
        tool.push(ev.tool_id.clone());
        path.push(ev.path.clone());
        status.push(ev.http_status.map(i32::from));
        latency.push(ev.latency_ms.map(clamp_i64));
    }
    let res = sqlx::query(
        "INSERT INTO audit_events (occurred_at, event_type, outcome, user_sub, username, \
         session_ref, client_ip, user_agent, reason, request_id, tool_id, path, \
         http_status, latency_ms) \
         SELECT t.at, t.ty, t.out, t.sub, t.usr, t.sref, t.ip::inet, t.ua, t.reason, t.rid, \
                t.tool, t.path, t.status, t.latency \
         FROM UNNEST($1::timestamptz[], $2::text[], $3::text[], $4::text[], $5::text[], \
                     $6::text[], $7::text[], $8::text[], $9::text[], $10::text[], \
                     $11::text[], $12::text[], $13::int4[], $14::int8[]) \
              AS t(at, ty, out, sub, usr, sref, ip, ua, reason, rid, tool, path, status, \
                   latency)",
    )
    .bind(&at)
    .bind(&ty)
    .bind(&out)
    .bind(&sub)
    .bind(&user)
    .bind(&sref)
    .bind(&ip)
    .bind(&ua)
    .bind(&reason)
    .bind(&rid)
    .bind(&tool)
    .bind(&path)
    .bind(&status)
    .bind(&latency)
    .execute(pool)
    .await;
    if let Err(e) = &res {
        write_failed(evs[0].event_type, evs.len(), e);
    }
    res.map(|_| ())
}

fn clamp_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn write_failed(t: EventType, count: usize, e: &sqlx::Error) {
    tracing::error!(
        target: "audit",
        event_type = t.as_str(),
        count,
        error = %e,
        "audit write failed"
    );
}

/// The structured JSON line (`target=audit`); returns the IP as text.
fn emit(ev: &AuditEvent) -> Option<String> {
    let ip = ev.client_ip.map(|ip| ip.to_string());
    {
        tracing::info!(
            target: "audit",
            occurred_at = %ev.occurred_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            event_type = ev.event_type.as_str(),
            outcome = ev.outcome.as_str(),
            user_sub = ev.user_sub.as_deref(),
            username = ev.username.as_deref(),
            session_ref = ev.session_ref.as_deref(),
            client_ip = ip.as_deref(),
            user_agent = ev.user_agent.as_deref(),
            reason = ev.reason,
            request_id = ev.request_id.as_deref(),
            tool_id = ev.tool_id.as_deref(),
            path = ev.path.as_deref(),
            http_status = ev.http_status,
            latency_ms = ev.latency_ms,
            "audit event"
        );
    }
    ip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_log_first_then_rate_limited() {
        let d = DropLog {
            base: std::time::Instant::now(),
            last_ms: AtomicU64::new(u64::MAX),
            pending: AtomicU64::new(0),
        };
        d.pending.fetch_add(1, Ordering::Relaxed);
        assert_eq!(d.take_due(), Some(1), "first drop logs immediately");
        for _ in 0..5 {
            d.pending.fetch_add(1, Ordering::Relaxed);
            assert_eq!(d.take_due(), None, "suppressed within the interval");
        }
        // Pretend the last line was more than an interval ago.
        d.last_ms.store(0, Ordering::Relaxed);
        let d2 = DropLog {
            base: std::time::Instant::now()
                - DROP_LOG_INTERVAL
                - std::time::Duration::from_millis(1),
            last_ms: AtomicU64::new(0),
            pending: AtomicU64::new(d.pending.load(Ordering::Relaxed)),
        };
        assert_eq!(
            d2.take_due(),
            Some(5),
            "summary carries the suppressed count"
        );
    }
}
