# Plan of Action and Milestones (POA&M)

Status as of M2 (shell chrome and Playwright/axe E2E), 2026-11-03. Owner and
milestone fields are placeholders until the team assigns them.

Status values used below:

- **Open**: the weakness exists and is not fixed.
- **Pending CI verification**: a fix or test exists and passed locally, but
  the CI job that is meant to prove it has never had a green run. The item
  stays open until a green CI run is recorded here, with its run ID and
  artifacts.
- **Deferred**: accepted for now, with a stated reason.

## Items from `POAM:` markers in code

There are two `POAM:` markers in the repository: one in `deny.toml` and one
`// POAM:` in Rust source (`crates/console-shell/src/demo.rs:14`).

| ID | Weakness | Source | Control | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-001 | RUSTSEC-2024-0370: `proc-macro-error` is unmaintained (not a known vulnerability). It comes in through `yew-macro` and is used at build time only. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | Open | TBD | TBD (re-check on each yew release) |
| POAM-002 | RUSTSEC-2025-0141: `bincode` 1.x is unmaintained (not a known vulnerability). It comes in through `gloo-worker`. The console does not use yew workers. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | Open | TBD | TBD (re-check on each yew release) |
| POAM-010 | NIST-004: the service menu list (`SERVICES`) is static demo data and is not filtered by role. There is no access enforcement behind it. Marker text: `// POAM: AC-3/AC-6 - service list is static and unfiltered; server-side role filtering arrives with the M4 registry.` | `crates/console-shell/src/demo.rs:14` | AC-3, AC-6 | Open | TBD | M4 (tool registry) |

`yew-router` 0.20 also depends on both crates in POAM-001/002. `cargo audit`
reports them as 2 allowed warnings (M2 QA run).

## Verification gaps and deferred findings

### POAM-003 to POAM-006: pending CI verification (NIST-002)

These four items have local evidence, but they are **not closed**. The CI
`wasm-test` and `e2e` jobs in `.github/workflows/ci.yml` were added in M2 and
have never run. Firefox (wasm tests and E2E) has not been run anywhere. Close
each item only after a green CI run of `wasm-test` and `e2e` (all three
browsers), and record the run ID and the `wasm-pack-log` and
`playwright-report` artifacts here.

| ID | Weakness | Local evidence (M2) | Control / reference | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-003 | wasm-bindgen browser tests in `crates/console-ui/tests/wasm.rs` had never been run. | `make wasm-test WASM_BROWSER=chrome`: 16 passed, 0 failed (local headless Chrome). The CI `wasm-test` job runs headless Firefox and has not run yet. | SA-11; WCAG 2.1 | Pending CI verification | TBD | First green CI `wasm-test` run |
| POAM-004 | No axe accessibility scan had been run (M1 A11Y-009). | `e2e/tests/a11y.spec.ts`: axe (wcag2a/aa, wcag21a/aa) on `/` and `/gallery`, light and dark, at 1280, 600 and 320 px wide; 0 serious/critical on Chromium. **Closure scope: axe runs on Chromium only.** WebKit and Firefox get keyboard coverage (`keyboard.spec.ts`, `reflow.spec.ts`), not axe (see POAM-012). Keyboard specs pass on Chromium and WebKit locally; Firefox not run. | WCAG 2.1 AA | Pending CI verification | TBD | First green CI `e2e` run |
| POAM-005 | M1 A11Y-007: fixed px sizes instead of tokens; checkbox smaller than the target minimum. | Sizes moved to tokens (`--cui-size-control-check` is 24px, plus border-width and component-size tokens). Test `components_css_has_no_raw_px` (`crates/console-ui/tests/qa_logic_css.rs`) passes. The a11y reviewer marked it ready to close. | WCAG 2.1 SC 1.4.4, 2.5.8 | Pending CI verification | TBD | First green CI `e2e` run |
| POAM-006 | M1 A11Y-008: side navigation could not be collapsed on narrow screens. | Below 45rem the nav is hidden behind a `NavToggle` disclosure. `keyboard.spec.ts` "side nav at narrow width" and `reflow.spec.ts` (320x640, no horizontal scroll, dropdown panels inside the viewport) pass on Chromium and WebKit. Not run on Firefox. | WCAG 2.1 SC 1.4.10 | Pending CI verification | TBD | First green CI `e2e` run |

### Other open items

| ID | Weakness | Control / reference | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- |
| POAM-007 | M1 PERF-002 (partial fix): `Table` still runs `item_key` on every item on each render. Selection lookup is O(1); key memoisation is deferred until a real tool uses Table. | Performance | Open (deferred) | TBD | TBD |
| POAM-008 | M1 PERF-003: three render-blocking CSS requests (tokens, components, shell), about 5 KB gz in total. Not changed in M2. | Performance | Open (optional) | TBD | TBD |
| POAM-009 | M1 SEC-003: the shell's theme storage DOM path (`theme::dom::load/apply/save` in `crates/console-shell/src/theme.rs`) has no test. Only the parse allow-list it relies on is tested. Not changed in M2. | SI-10 | Open | TBD | TBD |
| POAM-011 | NIST-003 / SEC-002: known benign CSP reports on WebKit. WebKit reports a `style-src-attr` violation each time an `HTMLSelectElement` is created (reproduced with a bare `document.createElement('select')`, no app code). The E2E tests allow, **on WebKit only**, reports that match exactly `{violatedDirective: "style-src-attr", blockedURI: "inline", sample: ""}` with a target of `SELECT` or `null`, capped at the number of `<select>` elements plus 1 per page (`e2e/tests/csp-helpers.ts`, `csp.spec.ts`, `dist-smoke.spec.ts`). **Residual risk:** a first-party inline-style violation with a `null` (document-level) target on WebKit would also match; it is bounded by the cap and by the non-empty `[style]` DOM check. Adding `'report-sample'` to `style-src` (SEC-002(b)) is deferred. Upstream WebKit bug link: TBD. Any future CSP report collector must filter this triple. | SC-18, SA-11 | Open | TBD | TBD (expire when WebKit fixes it, or re-review at M3) |
| POAM-012 | A11Y-006: axe scans run on Chromium only. Firefox E2E has not been run locally. | WCAG 2.1 AA; SA-11 | Open (deferred) | TBD | TBD |
| POAM-013 | M2 PERF-002: optional bundle trims (about 1-3 KB gz) in the scope picker (`crates/console-shell/src/chrome.rs`). Skipped in M2 as not simpler. | Performance | Deferred | TBD | TBD (optional) |
