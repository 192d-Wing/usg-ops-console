//! Tool-origin sessions (migrations 0005, 0006): single-use launch codes minted on
//! the console origin and exchanged on a tool origin for `__Host-tool_sid`.
//! Both rows reference the console session (`ON DELETE CASCADE`), so every
//! way a console session ends also ends its tool sessions. Tool requests
//! read the console row's deadlines and never extend them. Up to
//! [`MAX_TOOL_SESSIONS`] rows per (console session, tool) coexist.

use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tracing::Instrument;

use crate::session::{SESSION_COLS, Session, SessionStore};

/// Launch code lifetime (plan.md section 4.4).
pub const LAUNCH_CODE_TTL: Duration = Duration::from_secs(30);
/// Max stored launch path (incl. query).
pub const MAX_LAUNCH_PATH: usize = 2048;
/// Tool sessions kept per (console session, tool); the oldest launch is
/// evicted beyond this (INT-005: parallel tabs).
pub const MAX_TOOL_SESSIONS: i64 = 8;

/// Per-request lookup SQL, built once (PERF-005: one static string, so the
/// sqlx statement cache sees the same query every time).
static TOOL_SESSION_SQL: LazyLock<String> = LazyLock::new(|| {
    let cols: Vec<String> = SESSION_COLS
        .split(',')
        .map(|c| format!("s.{}", c.trim()))
        .collect();
    format!(
        "SELECT {} FROM tool_sessions t JOIN sessions s ON s.id_hash = t.session_hash \
         WHERE t.id_hash = $1 AND t.tool_id = $2",
        cols.join(", ")
    )
});

#[derive(Debug, sqlx::FromRow)]
pub struct LaunchCode {
    pub session_hash: Vec<u8>,
    pub tool_id: String,
    pub path: String,
    pub expires_at: DateTime<Utc>,
}

impl SessionStore {
    /// Stores `sha256(code)` bound to the console session and tool.
    // COMPLIANCE MAPPING:
    // - NIST 800-53: SC-23 (Session Authenticity) — 256-bit single-use code, 30 s, bound to session + tool
    pub async fn insert_launch_code(
        &self,
        code_hash: &[u8],
        session_hash: &[u8],
        tool_id: &str,
        path: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO tool_launch_codes (code_hash, session_hash, tool_id, path, expires_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(code_hash)
        .bind(session_hash)
        .bind(tool_id)
        .bind(path)
        .bind(expires_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Atomic single use: the row is deleted whether or not it is valid.
    pub async fn take_launch_code(
        &self,
        code_hash: &[u8],
        now: DateTime<Utc>,
    ) -> Result<Option<LaunchCode>, sqlx::Error> {
        let row: Option<LaunchCode> = sqlx::query_as(
            "DELETE FROM tool_launch_codes WHERE code_hash = $1 \
             RETURNING session_hash, tool_id, path, expires_at",
        )
        .bind(code_hash)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.filter(|c| c.expires_at > now))
    }

    /// Adds a tool session for this console session and tool, then evicts
    /// the oldest rows beyond [`MAX_TOOL_SESSIONS`] (INT-005: each tab's
    /// launch gets its own row). `Ok(false)` when the console session no
    /// longer exists. The console row is locked so concurrent launches
    /// cannot exceed the cap.
    // COMPLIANCE MAPPING:
    // - NIST 800-53: AC-10 (Concurrent Session Control) — at most 8 tool sessions per console session and tool
    pub async fn insert_tool_session(
        &self,
        id_hash: &[u8],
        session_hash: &[u8],
        tool_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let live: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM sessions WHERE id_hash = $1 FOR UPDATE")
                .bind(session_hash)
                .fetch_optional(&mut *tx)
                .await?;
        if live.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO tool_sessions (id_hash, session_hash, tool_id, created_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id_hash)
        .bind(session_hash)
        .bind(tool_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM tool_sessions WHERE id_hash IN ( \
               SELECT id_hash FROM tool_sessions WHERE session_hash = $1 AND tool_id = $2 \
               ORDER BY created_at DESC, id_hash DESC OFFSET $3)",
        )
        .bind(session_hash)
        .bind(tool_id)
        .bind(MAX_TOOL_SESSIONS)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Per-request tool lookup: one indexed join to the console row (whose
    /// deadlines and roles apply). Timed under `session.lookup` like
    /// [`SessionStore::load`].
    pub async fn load_tool_session(
        &self,
        id_hash: &[u8],
        tool_id: &str,
    ) -> Result<Option<Session>, sqlx::Error> {
        // Read-only: one retry on a dead pooled connection (see `load`).
        match self.load_tool_session_once(id_hash, tool_id).await {
            Err(e) if crate::session::is_connection_error(&e) => {
                tracing::warn!(error = %e, "tool session lookup hit a dead connection; retrying once");
                self.load_tool_session_once(id_hash, tool_id).await
            }
            r => r,
        }
    }

    async fn load_tool_session_once(
        &self,
        id_hash: &[u8],
        tool_id: &str,
    ) -> Result<Option<Session>, sqlx::Error> {
        async {
            let t0 = std::time::Instant::now();
            let mut conn = self.pool().acquire().await?;
            let acquired = t0.elapsed();
            let row = sqlx::query_as(TOOL_SESSION_SQL.as_str())
                .bind(id_hash)
                .bind(tool_id)
                .fetch_optional(&mut *conn)
                .await;
            tracing::debug!(
                acquire_us = u64::try_from(acquired.as_micros()).unwrap_or(u64::MAX),
                total_us = u64::try_from(t0.elapsed().as_micros()).unwrap_or(u64::MAX),
                "tool session lookup"
            );
            row
        }
        .instrument(tracing::debug_span!("session.lookup", kind = "tool"))
        .await
    }
}
