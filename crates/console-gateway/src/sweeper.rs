//! Periodic deletion of expired sessions and auth requests, with one audit
//! record per swept session.

use std::sync::Arc;

use crate::audit::{AuditEvent, EventType, Outcome};
use crate::routes::AppState;
use crate::session;

/// Sessions deleted per sweep pass (bounded; the next tick continues).
const BATCH: i64 = 1000;
/// Audit rows per batched INSERT.
const CHUNK: usize = 200;

// COMPLIANCE MAPPING:
// - NIST 800-53: AC-12 (Session Termination) — expired sessions removed even if never revisited
/// One pass; returns how many sessions were swept.
pub async fn sweep_once(s: &AppState) -> Result<usize, sqlx::Error> {
    let now = s.clock.now();
    let swept = s.store.sweep(now, s.cfg.session_idle, BATCH).await?;
    let events: Vec<AuditEvent> = swept
        .iter()
        .map(|row| {
            let t = if row.absolute {
                EventType::ExpiredAbsolute
            } else {
                EventType::ExpiredIdle
            };
            let mut ev = AuditEvent::new(now, t, Outcome::Success);
            ev.user_sub = Some(row.user_sub.clone());
            ev.username = Some(row.username.clone());
            ev.session_ref = Some(session::session_ref(&row.id_hash));
            ev.client_ip = row.client_ip.as_deref().and_then(|ip| ip.parse().ok());
            ev.reason = Some("swept");
            ev
        })
        .collect();
    // One INSERT per CHUNK rows (PERF-002); tracing lines are per event.
    // POAM: AU-5 - a failed sweeper audit write is logged at error and not retried (the rows are already deleted).
    for chunk in events.chunks(CHUNK) {
        let _ = s.audit.record_many(chunk).await;
    }
    Ok(swept.len())
}

pub fn spawn(s: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(s.cfg.sweep_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match sweep_once(&s).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(swept = n, "expired sessions swept"),
                Err(e) => tracing::error!(error = %e, "session sweep failed"),
            }
        }
    })
}
