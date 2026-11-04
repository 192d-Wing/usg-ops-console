//! Audit events: one append-only row per auth event (via the restricted
//! audit pool) plus the same fields as a `tracing` JSON line (`target=audit`).

use std::net::IpAddr;

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::PgPool;

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
        }
    }
}

#[derive(Clone)]
pub struct AuditSink {
    pool: PgPool,
}

impl AuditSink {
    /// `pool` must connect as the `console_audit` role (INSERT+SELECT only).
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
             session_ref, client_ip, user_agent, reason, request_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7::inet, $8, $9, $10)",
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
        if evs.is_empty() {
            return Ok(());
        }
        let mut at = Vec::with_capacity(evs.len());
        let mut ty = Vec::with_capacity(evs.len());
        let mut out = Vec::with_capacity(evs.len());
        let mut sub = Vec::with_capacity(evs.len());
        let mut user = Vec::with_capacity(evs.len());
        let mut sref = Vec::with_capacity(evs.len());
        let mut ip = Vec::with_capacity(evs.len());
        let mut ua = Vec::with_capacity(evs.len());
        let mut reason = Vec::with_capacity(evs.len());
        let mut rid = Vec::with_capacity(evs.len());
        for ev in evs {
            ip.push(emit(ev));
            at.push(ev.occurred_at);
            ty.push(ev.event_type.as_str());
            out.push(ev.outcome.as_str());
            sub.push(ev.user_sub.clone());
            user.push(ev.username.clone());
            sref.push(ev.session_ref.clone());
            ua.push(ev.user_agent.clone());
            reason.push(ev.reason);
            rid.push(ev.request_id.clone());
        }
        let res = sqlx::query(
            "INSERT INTO audit_events (occurred_at, event_type, outcome, user_sub, username, \
             session_ref, client_ip, user_agent, reason, request_id) \
             SELECT t.at, t.ty, t.out, t.sub, t.usr, t.sref, t.ip::inet, t.ua, t.reason, t.rid \
             FROM UNNEST($1::timestamptz[], $2::text[], $3::text[], $4::text[], $5::text[], \
                         $6::text[], $7::text[], $8::text[], $9::text[], $10::text[]) \
                  AS t(at, ty, out, sub, usr, sref, ip, ua, reason, rid)",
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
        .execute(&self.pool)
        .await;
        if let Err(e) = &res {
            write_failed(evs[0].event_type, evs.len(), e);
        }
        res.map(|_| ())
    }
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
            "audit event"
        );
    }
    ip
}
