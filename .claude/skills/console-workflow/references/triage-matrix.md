# Triage Matrix — which roles run

When in doubt, include the reviewer: a skipped reviewer is a silent gap in ATO
evidence, an extra one costs only tokens.

## Always-on for any code change

- `qa-engineer`
- `security-reviewer`
- `nist-800-53-reviewer`
- `compliance-scribe`

## Path-triggered roles

| Touched path / topic | Add |
| --- | --- |
| `crates/console-protocol/**` (manifest, SDK messages, `PROTOCOL_VERSION`) | `integration-contract-reviewer`, `release-engineer`, `performance-reviewer` (compiled into the WASM bundle) |
| `crates/console-sdk/**`, `sdk-js/**` | `integration-contract-reviewer`, `performance-reviewer` |
| `crates/console-gateway/**` HTTP routes, registry, proxy | `integration-contract-reviewer`, `performance-reviewer` (proxy/session hot path) |
| Gateway OIDC, sessions, logout, timeouts, audit | (always-on; flag AC-11/AC-12/AU-2/AU-3/AU-12/IA-2(12)/SC-23 focus) |
| Security headers, CSP, iframe `sandbox`, `postMessage` handling | (always-on; flag SC-8/CM-7/SI-10 focus), `integration-contract-reviewer` |
| `crates/console-shell/**` | `ux-a11y-reviewer`, `performance-reviewer`, `integration-contract-reviewer` (if it handles protocol messages) |
| `crates/console-ui/**`, `assets/tokens.css`, `assets/*.css` | `ux-a11y-reviewer`, `performance-reviewer` |
| `crates/modules/**` (native modules) | `ux-a11y-reviewer`, `performance-reviewer` |
| `examples/hello-tool/**` | `integration-contract-reviewer`, `ux-a11y-reviewer` |
| `Cargo.toml`/`Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, `sdk-js` package files | `release-engineer` (+ `performance-reviewer` if a WASM crate gains a dependency) |
| `Trunk.toml`, `index.html`, release profile | `release-engineer`, `performance-reviewer` |
| `deploy/**` (Containerfile, Helm, Flux) | `release-engineer` |
| `.github/workflows/**`, `Makefile` | `release-engineer` |
| Tests / Playwright specs only | `qa-engineer` only |

## Dropped roles

`db-migration-reviewer` and `protocol-reviewer` (usg-dcim) are not part of
this team: the console has no database schema and no device protocols. If
sessions or audit move to a Postgres store (PLAN §6 open decision 3), restore
a database/migration reviewer and trigger it on store schema, migrations, and
query changes.

## Phase skips

| Situation | Skip |
| --- | --- |
| Docs / comments only | Phases 1–4; run scribe + `markdownlint` only |
| Bug fix with an obvious one-file cause | Phase 1 may be a short inline plan by the orchestrator |
| Internal refactor, no behaviour/protocol/API change | Phase 6 (but still check the bundle budget if a WASM crate is touched) |
| `review` mode | Phases 1–2 |

Record every skipped role or phase, with the reason, in `brief.md`.
