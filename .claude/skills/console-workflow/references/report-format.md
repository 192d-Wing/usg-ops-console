# Report Formats

## Findings file (`findings-<role>.md`)

```markdown
# Findings — <role>

Scope: <base>...<head> (<n> files), plan: <run>/plan.md
Verdict: APPROVE | APPROVE WITH COMMENTS | CHANGES REQUESTED

## <ROLE-PREFIX>-001 [BLOCKER] <one-line title>

- **Location**: `crates/console-gateway/src/proxy.rs:123`
- **Reference**: NIST 800-53 AC-12 | CWE-601 | WCAG 2.1 SC 2.4.7 | protocol v1 `ToolToShell`
- **Problem**: what is wrong, in one or two sentences.
- **Failure scenario**: concrete request/message/state → concrete bad outcome.
- **Fix**: the smallest change that resolves it.
- **Confidence**: high | medium | low
```

Role prefixes: `SEC` (security), `NIST` (800-53), `INT` (integration
contract), `PERF` (performance), `A11Y` (UX/accessibility), `QA` (testing),
`REL` (release).

### Severity

| Severity | Meaning | Gate effect |
| --- | --- | --- |
| BLOCKER | Auth/session bypass, session fixation, open redirect, identity-header spoofing through the proxy, iframe escape or missing `postMessage` origin check, XSS, secret exposure, audit bypass, breaking protocol/manifest change that blanks deployed tools | Must fix |
| MAJOR | Missing required control/test, fail-open path, breaking gateway API or protocol change without versioning, unbounded buffering/body size, bundle over 1.5 MB gz or > 10% growth, WCAG 2.1 AA failure (keyboard trap, contrast, missing label), clippy/fmt drift | Must fix |
| MINOR | Hardening, missing annotation, weak test, raw value instead of token, unclear error shape, small perf win | Advisory |
| NIT | Style, naming, comments | Advisory |

A finding without a location and a failure scenario is MINOR at most.

## Fix response (`fixes-round-<n>.md`)

```markdown
| Finding | Action | Change |
| --- | --- | --- |
| SEC-001 | Fixed | `proxy.rs:120-131` strip inbound `X-Forwarded-User` before injection |
| PERF-002 | Rebutted | dep is feature-gated off in the wasm build; bundle +4 KB gz |
```

## Gate summary (`gate.md`)

```markdown
# Gate — <task>

Verdict: PASS | PASS WITH POA&M | FAIL

| Gate | Result | Evidence |
| --- | --- | --- |
| fmt / clippy (host + wasm32) | PASS | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| cargo test | PASS (42) | `cargo test --workspace --locked` |
| trunk build + bundle | PASS (612 KB gz) | `trunk build --release`; gzip size |
| E2E + axe | NOT RUN (no Playwright suite yet) | qa-report.md |
| cargo deny / audit | PASS | `cargo deny check`, `cargo audit` |
| security review | APPROVE | findings-security.md |
| NIST 800-53 review | APPROVE WITH COMMENTS | findings-nist.md (2 MINOR) |
| integration contract review | APPROVE | findings-integration.md |
| performance review | APPROVE | findings-perf.md |
| UX/a11y review | SKIPPED (gateway-only change) | brief.md |
| compliance docs | UPDATED | docs-update.md |
| release | image + deny checks | release-notes.md |

## Open items
## POA&M
## Dropped findings
```
