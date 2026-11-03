# Audit logging

**Status: Planned (M3).** The console does not emit audit events yet.

## What exists today

`crates/console-gateway/src/main.rs` writes JSON logs through
`tracing_subscriber` (`.json()`, level from `RUST_LOG`, default `info`) and
uses `tower_http::trace::TraceLayer` for HTTP request spans. These are
operational logs. They are not audit records: they have no user identity, no
defined event types, and no SIEM export. Do not cite them as AU-2, AU-3 or
AU-12 evidence.

## Planned

- Audit event types, record fields and example records
- SIEM export

These will be documented here when they are implemented (M3, `docs/PLAN.md`).
