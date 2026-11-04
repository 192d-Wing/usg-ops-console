# Audit logging

Status as of M3, 2026-11-03. This file describes what the code does today.

Evidence status: the `pg_flow` integration tests (15/15, `make
test-integration` against compose Postgres 17) and the Playwright suite on
Chromium and WebKit passed locally. Firefox and the new CI jobs
(`gateway-integration`, compose-based `e2e`) have not run yet (POAM-030).

## Where audit records go

`console-gateway` writes every authentication and session event twice
(`crates/console-gateway/src/audit.rs`):

1. **Structured JSON log line first.** `tracing` event with
   `target=audit`, message `audit event`, and all record fields. The
   gateway's subscriber writes JSON to stdout (`RUST_LOG`, default `info`).
   This line is emitted before the DB write, so it exists even when the DB
   write fails.
2. **One row in the append-only table `audit_events`**, written through a
   separate connection pool (`CONSOLE_AUDIT_DATABASE_URL`) that logs in as
   the restricted `console_audit` role.

Operational request logs (the `TraceLayer` spans) are not audit records.
Their spans record only method and path, never the query string, so the
OIDC `code` and `state` are not logged (SEC-008; asserted by
`pg_full_flow_and_audit`, which captures all TRACE output and checks that
no `code=`, `state=`, refresh token, `eyJ` or client secret appears).

## Record fields (AU-3)

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | bigint identity | Row order (DB only) |
| `occurred_at` | timestamptz | When, UTC; the JSON line uses RFC 3339 with milliseconds and `Z` (AU-8) |
| `event_type` | text | What (catalogue below) |
| `outcome` | text | `success`, `failure` or `denied` (CHECK constraint) |
| `user_sub` | text | Who: the id_token `sub`; null when unknown (for example a failure before a token was verified) |
| `username` | text | Who: preferred username, when known |
| `session_ref` | text | First 8 hex characters of `sha256(session id)`. A correlation handle only; it is not the session secret |
| `client_ip` | inet | Where: the client address. `X-Forwarded-For` is honoured only from peers in `CONSOLE_TRUSTED_PROXIES`, walked right to left (`client_ip.rs`) |
| `user_agent` | text | Truncated to 256 bytes |
| `reason` | text | Fixed machine code from the catalogue; never token text or request input |
| `request_id` | text | Random per-request id for correlating the JSON line and the row |

No field ever carries an id_token, refresh token, authorization code,
client secret or CSRF token. `pg_full_flow_and_audit` asserts that no
audit row contains `eyJ`.

## Event catalogue (AU-2, AU-12)

| `event_type` | `outcome` | `reason` values | Emitted when | Write mode |
| --- | --- | --- | --- | --- |
| `login.success` | success | none, or `reauthenticated` (the sign-in required fresh authentication: reauth marker present, or `/login?reason=expired` or `ended`) | Callback verified and the session was created | **Fail closed** |
| `login.failure` | failure | `no_auth_request`, `state_mismatch`, `idp_error`, `exchange_failed`, `token_invalid`, `stale_auth` | Callback rejected | Best effort |
| `login.denied` | denied | `missing_role`, `insufficient_assurance` | Valid token, but no required role or the `acr` is not accepted. No session is created | Best effort |
| `logout` | success | `user` (explicit sign-out), `superseded` (a session presented to a failed or denied callback was ended) | Session deleted | Best effort |
| `session.expired.idle` | success | `on_access`, `swept` | Idle deadline passed (found on a request, or by the sweeper) | Best effort (sweeper: batch) |
| `session.expired.absolute` | success | `on_access`, `swept` | Absolute deadline passed | Best effort (sweeper: batch) |
| `session.refresh` | success | `keepalive` | Keepalive extended the session. Audited at most once per 60 s per session | Best effort |
| `session.revoked` | success | `idp_rejected`, `missing_role`, `subject_mismatch`, `token_invalid`, `no_refresh_token` | Keepalive revalidation ended the session | Best effort |
| `access.denied` | denied | `csrf_token`, `origin`, `rate_limited` | CSRF failure on keepalive or logout (with `user_sub` and `session_ref`), or `/login` over the per-IP rate limit (once per IP per limiter episode, not per request) | Best effort |

Code: `audit::EventType`, `routes.rs` (callback, `require_session`,
`keepalive`, `logout`, `csrf_denied`, `login`), `sweeper.rs`.

Tests: `pg_full_flow_and_audit` (login, refresh, logout, 8 CSRF denials),
`pg_callback_failures_and_single_use`, `pg_missing_role_is_denied_without_session`,
`pg_insufficient_assurance_is_denied`, `pg_reauth_after_logout_requires_fresh_auth`
(`stale_auth`), `pg_idle_and_absolute_expiry`, `pg_sweeper_deletes_and_audits`,
`pg_keepalive_revalidates_with_idp`, `pg_failed_callback_ends_presented_session`,
`pg_login_rate_limit_is_audited_once`. E2E (`e2e/tests/db-helpers.ts`
queries the table): after the round-1 run the DB held `session.revoked`
`idp_rejected`/`missing_role`, `login.denied`
`insufficient_assurance`/`missing_role`, `access.denied`
`csrf_token`/`origin`/`rate_limited`, `session.expired.idle` and
`login.success` `reauthenticated`.

Not audited: `/healthz`, ordinary authenticated requests, 401s for
requests without a session, and 404s.

## Example records

Illustrative values (field layout matches the code).

JSON log line:

```json
{"timestamp":"2026-11-03T18:00:01.234Z","level":"INFO","target":"audit","fields":{"message":"audit event","occurred_at":"2026-11-03T18:00:01.230Z","event_type":"login.success","outcome":"success","user_sub":"3f1c2a9e-0000-4000-8000-000000000001","username":"jdoe","session_ref":"9a1b2c3d","client_ip":"198.51.100.7","user_agent":"Mozilla/5.0 ...","request_id":"Zk3aQ1x0b2c"}}
```

Row:

```text
id=42 occurred_at=2026-11-03 18:20:00+00 event_type=access.denied outcome=denied
user_sub=3f1c2a9e-... username=jdoe session_ref=9a1b2c3d client_ip=198.51.100.7
user_agent=Mozilla/5.0 ... reason=origin request_id=Hq8...
```

The exact top-level wrapper of the JSON line (`timestamp`, `level`,
`fields`) is the `tracing_subscriber` JSON formatter's default layout.

## Fail-closed vs best-effort writes (AU-5)

- **Fail closed:** `login.success`. The session insert runs in a DB
  transaction that is committed only after the audit row has been inserted
  (through the audit pool). If the audit insert fails, the transaction is
  rolled back, no session exists, and the user gets 503 (`routes.rs`
  callback, comment `AU-5: no audit record, no session`).
- **Best effort:** every other event (`AuditSink::record_best_effort`). The
  JSON line is always written first. A DB failure is logged at `error`
  with `target=audit`, message `audit write failed`, the `event_type` and a
  `count`. The action (logout, expiry, denial, revocation) still happens,
  because refusing a logout or a denial would be worse. Marked
  `// POAM: AU-5` in `audit.rs` and `sweeper.rs` (POAM-016). No metric or
  alert is wired to `audit write failed` yet.

## Append-only storage and DB roles (AU-9)

`crates/console-gateway/migrations/0002_audit.sql` and `0003_roles_refresh.sql`:

- `BEFORE UPDATE OR DELETE` row trigger and `BEFORE TRUNCATE` statement
  trigger on `audit_events` raise `audit_events is append-only`.
- `REVOKE ALL ... FROM PUBLIC`.
- `audit_events` is owned by `console_owner` (NOLOGIN).
- `console_audit` has only `INSERT, SELECT` on `audit_events`, and no
  rights on `sessions`.
- The runtime role `console_app` (granted to the `console` login) has DML on
  `sessions` and `auth_requests` only and **no** rights on `audit_events`.

Tests: `pg_audit_is_append_only` (the app role cannot SELECT, INSERT,
DISABLE TRIGGER or DROP `audit_events`; the audit role cannot UPDATE,
DELETE, TRUNCATE, DISABLE TRIGGER or DROP; all public tables are owned by
`console_owner`); `deploy/compose/postgres/check-roles.sql` (run locally
after migrations: "SEC-006 role contract OK").

Residual (POAM-017, marker `-- POAM: AU-9` in `0003_roles_refresh.sql`):
`console_owner` (assumable through the `console_migrator` login) and
database superusers can still disable the triggers or drop the table. An
external write-once audit store (AU-4(1), AU-9(2)) is planned for M7.

## Sweeper

`crates/console-gateway/src/sweeper.rs` runs every
`CONSOLE_SWEEP_INTERVAL_SECS` (default 60). Each pass deletes up to 1000
expired sessions (two index range scans joined with `UNION ALL`) and
audits them as `session.expired.idle` or `session.expired.absolute`
reason `swept`, with the session's stored `client_ip`. Audit rows are
inserted in batches of 200 (`AuditSink::record_many`, one
`INSERT ... SELECT FROM UNNEST`). Each event still gets its own JSON line.
A failed batch write is logged and not retried, because the sessions are
already deleted (POAM-016). Test: `pg_sweeper_deletes_and_audits`.

## Retention and SIEM export

**Planned.** There is no retention policy, export job or SIEM forwarder in
this repository. The JSON lines on stdout (`target=audit`) are the
intended collection point for a log shipper; the deployment must provide
one. Until then, the `audit_events` table is the system of record.
