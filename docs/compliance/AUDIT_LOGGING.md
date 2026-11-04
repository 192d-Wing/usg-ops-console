# Audit logging

Status as of M4 (tool events, registry events, background audit queue),
2026-11-04. This file describes what the code does today. M4 is green in
CI run 41892456284; M4 statements also cite local runs (`make
test-integration` 99 passed; Playwright macOS 294 passed, Linux container
428 passed / 3 flaky, POAM-044).

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
| `tool_id` | text, nullable (M4) | Registry tool id for `tool.*` and `registry.insecure_upstream` events |
| `path` | text, nullable (M4) | Request or launch path **without the query** (`path_only`) |
| `http_status` | integer, nullable (M4) | Status returned for the tool request or denial |
| `latency_ms` | bigint, nullable (M4) | Upstream latency for proxied requests |

The M4 columns are added by `migrations/0005_tool_sessions.sql`. Test:
`tools_api::tests::audit_paths_drop_query`.

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

M4 events:

| `event_type` | `outcome` | `reason` values | Emitted when | Write mode |
| --- | --- | --- | --- | --- |
| `tool.launch` | success | `launch` | `POST /api/tools/{id}/launch` minted a launch code (with `tool_id`, `path`) | Best effort (awaited) |
| `tool.access` | success, or failure on 5xx | `navigation` | Each proxied document navigation (`Sec-Fetch-Mode: navigate`, or no Fetch Metadata), not each asset. Carries `path`, `http_status`, `latency_ms` | Background queue |
| `tool.request` | success, or failure on 5xx | `write` | Each state-changing (unsafe-method) proxied request, only for tools with registry `audit_writes = true` | Background queue |
| `tool.denied` | denied | `missing_role` (launch 404, code exchange 404, proxy 403 after a mid-session role revocation), `launch_context` (launch URL not opened from the console iframe), `bad_code` (missing, unknown, used or expired launch code), `misdirected` (421: path for another tool), `identity_header_stripped` (client sent identity headers that were removed; at most once per tool session per minute), `rate_limited` (added when the per-IP denial audit limiter applies) | Denials on the launch API and on tool origins | Background queue (launch API and exchange role denials too, SEC-011); unauthenticated tool-origin denials are rate-bounded per IP |
| `registry.loaded` | success | `startup` | Once per gateway start after the registry validated | **Fail closed** (boot stops) |
| `registry.insecure_upstream` | success | `insecure_http_upstream` | At start, once per tool that sets `insecure_http_upstream = true`; the operator's reason is in the log line | **Fail closed** (boot stops) |

Every tool event carries the tool id, `session_ref` when a session is
known, client IP and request id. Tool upstreams keep their own business
audit; the console records access and launch (AU-2 split). Launch codes,
assertions and query strings never appear in records.

Code: `audit::EventType`, `tools_api.rs`, `tool_origin.rs`, `main.rs`
(registry events), `routes.rs` (callback, `require_session`,
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

M4 tests: `pg_tool_launch_proxy_and_assertion` (`tool.access`,
`tool.request`, `identity_header_stripped`), `pg_tool_role_denied`
(`tool.denied` `missing_role`, 404), `pg_tool_session_lifecycle`
(`launch_context`), `tool_origin::tests::navigation_audit_classification`,
`router::forbidden_tool_launch_matches_unknown_id_without_awaiting_db`;
E2E `tools.spec.ts` (rows for `tool.launch`, `tool.access`, `tool.request`
with path and `http_status`; `identity_header_stripped`),
`tools-norole.spec.ts` and `tools.spec.ts` (`missing_role`),
`tool-isolation.spec.ts` (`launch_context`, 6 or more rows). The
`registry.*` events have no dedicated automated test; they are written on
every E2E gateway start (fail-closed path).

Not audited: `/healthz`, ordinary authenticated console requests, proxied
tool assets (non-navigation GETs), 401s for console requests without a
session, and console 404s.

## Example records

M4 tool row (illustrative):

```text
id=97 occurred_at=2026-11-04 15:02:11+00 event_type=tool.access outcome=success
user_sub=3f1c2a9e-... username=jdoe session_ref=9a1b2c3d client_ip=198.51.100.7
reason=navigation tool_id=hello path=/t/hello/headers http_status=200 latency_ms=3
request_id=Pq2...
```

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
- **Fail closed at startup (M4):** `registry.loaded` and
  `registry.insecure_upstream`. If the audit DB write fails, the gateway
  does not start.
- **Background queue (M4, PERF-002):** `AuditSink::record_background`
  writes the JSON line synchronously, then `try_send`s the row into a
  bounded queue (`QUEUE_CAPACITY` 1024). A writer task inserts batches of
  at most 64 rows (`INSERT ... UNNEST`). If the queue is full or closed
  the row is dropped and counted (`AuditSink::dropped`). Drop logging is
  rate-limited (`DropLog`): the first drop is logged at `error` at once,
  then at most one summary per 10 s with `dropped_since_last_log` and
  `dropped_total`. Drops suppressed after the last summary appear only in
  the counter. On shutdown the gateway flushes the queue with a 5 s bound.
  Used for `tool.access`, `tool.request`, `tool.denied` and rate-bounded
  unauthenticated denials. Marker `// POAM: AU-5` at `audit.rs:190`. **No
  metric export or alert for `dropped()` exists yet: required before
  production (POAM-036).** Test:
  `audit::tests::drop_log_first_then_rate_limited`.

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

**Planned.** M4 adds no export. There is no retention policy, export job or SIEM forwarder in
this repository. The JSON lines on stdout (`target=audit`) are the
intended collection point for a log shipper; the deployment must provide
one. Until then, the `audit_events` table is the system of record.
