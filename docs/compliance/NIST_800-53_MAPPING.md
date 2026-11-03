# NIST SP 800-53 Rev. 5 control mapping

Status as of M2 (shell chrome, global scope picker, Playwright/axe E2E),
2026-11-03.
This file describes only what the code in this repository does today.

Status values:

- **Implemented**: the code does it, and a test or gate in this repo proves it.
- **Partial**: some of the control is in code, or the code is there but no test
  proves all of it.
- **Planned**: nothing in code yet. The milestone is from `docs/PLAN.md`.
- **Inherited**: provided by an external system. No control is marked
  Inherited yet, because none of those integrations exist in code.

## Summary

| Control | Status | Milestone |
| --- | --- | --- |
| CM-7 Least functionality (gallery excluded from release) | Implemented | M1 |
| SI-10 Input validation (`safe_href`, theme allow-list, scope URL query, search length) | Implemented | M1-M2 |
| SI-11 Error handling (scope errors name the key only) | Implemented | M2 |
| SC-8 Transmission confidentiality (HSTS header) | Partial | M0 |
| SC-18 / SI-10 Content Security Policy (hash-based) | Partial | M0-M2 |
| SA-11 Developer testing (wasm-bindgen and E2E jobs) | Implemented | M2 |
| SA-11(1) Static code analysis (clippy) | Implemented | M0 |
| SR-3, SR-11, CM-14 Supply chain (pinned actions, images, npm) | Partial | M2 |
| RA-5 / SI-2 Dependency vulnerability scanning | Partial | M0 |
| PL-8 Security architecture (scope is a UI filter only) | Partial (documented design rule) | M2 |
| AC-2, AC-3, AC-6 Account management, access enforcement | Planned (POAM-010) | M3-M4 |
| AC-11, AC-12 Session lock, session termination | Planned | M3 |
| AU-2, AU-3, AU-12 Audit events | Planned | M3 |
| IA-2, IA-2(12) Identification, PIV/CAC | Planned (to be Inherited from Keycloak) | M3 |
| SC-23 Session authenticity | Planned | M3 |

## CM-7 Least functionality

**Status: Implemented.**

The component gallery (`/gallery`) is a development aid. It is compiled only
when the `gallery` Cargo feature is on. The feature is off by default, so
release builds leave out both the route and the module.

Code:

- `crates/console-shell/Cargo.toml`: `[features] default = []`, `gallery = []`.
- `crates/console-shell/src/main.rs`: the `Gallery` route and the
  `gallery` module are `#[cfg(feature = "gallery")]`.
- `Makefile` target `web` builds the release bundle into
  `crates/console-shell/dist` without the feature. Target `web-gallery` builds
  into the separate, gitignored `crates/console-shell/dist-gallery`.
- `deploy/Containerfile` runs plain `trunk build --release`, so the image has
  no gallery.

Evidence:

- Test `gallery_not_default_feature` in `crates/console-ui/tests/guards.rs`
  fails if `gallery` appears in the shell's default features.
- Make target `check-no-gallery` (part of `make check`) rebuilds `dist`. It
  fails if no release wasm is present, or if the wasm contains the
  gallery-only string `Component gallery`.
- CI job step "Release bundle is gallery-free (CM-7)" in
  `.github/workflows/ci.yml` runs the same grep on the CI release build.

- E2E `e2e/tests/dist-smoke.spec.ts` test "release dist: /gallery is NotFound
  (gallery not compiled in)" runs against the release `dist` (gateway on
  127.0.0.1:8082, `dist-*` Playwright projects). Passed on all three engines
  in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231) (see SA-11).

Limits: the gallery has no authentication. That is acceptable only while it
stays compiled out of release builds. `make run` serves the gallery bundle and
is for local development only.

## SI-10 Information input validation

**Status: Implemented** for the four input points listed here. Other input
points (manifests, SDK messages, OIDC) do not exist yet.

### `safe_href`: rendered link targets

`crates/console-ui/src/logic.rs` `safe_href` (annotated
`// NIST 800-53: SI-10`). `Link`, `SideNavigation` and `Breadcrumbs` route every
`href` through it. It removes tab, CR and LF characters and trims leading
control characters and spaces. It then allows only `http:`, `https:`,
`mailto:` and relative URLs, and returns `#` for anything else. It also
returns `#` for protocol-relative and backslash forms (`//`, `\\`, `/\`, `\/`),
which browsers resolve to another origin. External links get
`rel="noopener noreferrer"`.

Tests:

- `logic::tests::safe_href_allow_list` (`crates/console-ui/src/logic.rs`)
- `safe_href_more_vectors` (`crates/console-ui/tests/qa_logic_css.rs`)

### Theme preference from `localStorage`

`Theme::parse` in `crates/console-ui/src/logic.rs` (annotated) accepts only the
listed theme values. `crates/console-shell/src/theme.rs` `load` (annotated)
passes the stored `cui-theme` value through that parse before anything is
written to `data-theme` on `<html>`.

Tests:

- `logic::tests::theme_parse` (`crates/console-ui/src/logic.rs`)
- `theme_allow_list_rejects_near_misses` (`crates/console-ui/tests/qa_logic_css.rs`)
- `tests::parse_allow_list_only`, `tests::system_clears_attribute`
  (`crates/console-shell/src/theme.rs`)

Gap: the wasm-only DOM path (`theme::dom::load/apply/save`) has no test. See
POA&M item POAM-009.

### Scope URL query (`site`, `tenant`, `env`)

`crates/console-shell/src/scope.rs` (annotated `NIST 800-53: SI-10` and
`SI-11`) parses `location.search`:

- The query is ignored as a whole if it is over 2048 bytes or 32 pairs.
- Keys are matched exactly and case-sensitively. A repeated key is rejected.
- No percent-decoding and no `+` handling. A value is accepted only if it
  matches `[a-z0-9][a-z0-9-]{0,62}` (checked byte by byte) **and** exists in
  the demo hierarchy (`crates/console-shell/src/demo.rs`): tenant must exist
  under the site, env under the (site, tenant) pair.
- Rejected keys are dropped and the URL is rewritten with `replaceState`.

`same_origin_url` in `crates/console-shell/src/main.rs` (SEC-001) collapses a
leading run of `/` or `\` to one `/` before every history push/replace, so a
crafted pathname such as `//evil.example/` cannot make `pushState` throw.

Tests (native, `crates/console-shell/src/scope.rs` `tests`): `charset`,
`rejects_bad_values_without_decoding`, `keys_exact_and_case_sensitive`,
`duplicates_rejected`, `oversize_ignored`, `relationship_checks`,
`qa_size_limits_exact_boundaries`,
`qa_uppercase_and_percent_encoding_rejected_at_every_level`,
`qa_repeated_lower_keys_rejected_upper_kept`, `qa_tenant_or_env_without_site`,
`qa_cross_site_pairs`, `qa_cascade_results_are_always_valid`; and
`tests::same_origin_url_collapses_leading_slashes`
(`crates/console-shell/src/main.rs`). E2E: `e2e/tests/scope.spec.ts`
(relationship validation and "QA: URL edge cases", for example "oversize query
(>2048 bytes) ignores all scope with a fixed message", "percent-encoded valid id
is not decoded").

### Global search box

`SearchInput` (`crates/console-ui/src/components/search.rs:33`, annotated
`SI-10 - input length-bounded (maxlength + truncation)`) sets `maxlength`. The
shell's `GlobalSearch` (`crates/console-shell/src/chrome.rs`) truncates to 256
characters and is display-only: submit shows a fixed "Search is not available
yet" flash and the query is not put in the URL or storage.

Tests: wasm `search_input_maxlength_and_submit`
(`crates/console-ui/tests/wasm.rs`, local Chrome only, see POAM-003); E2E
"search submit shows fixed info flash and never echoes the query"
(`e2e/tests/keyboard.spec.ts`).

## SI-11 Error handling

**Status: Implemented** for scope URL errors.

`crates/console-shell/src/scope.rs`: errors carry only the key name, never the
value. The shell shows one Warning flash, for example `Ignored invalid scope in
URL: site, tenant`, or `... : query too long` for oversize queries. Cascade
announcements are built from allow-list labels only. `same_origin_url` (see
SI-10) prevents a crafted path from turning into a wasm panic.

Tests: `errors_never_contain_value` (`scope.rs`); E2E invalid-value cases in
`e2e/tests/scope.spec.ts` check that the value appears in neither the DOM nor
the title.

## SC-8 Transmission confidentiality and integrity

**Status: Partial.**

`crates/console-gateway/src/main.rs` (annotated `// NIST 800-53: SC-8, SI-10`)
sets `Strict-Transport-Security: max-age=31536000; includeSubDomains` on every
response.

Why only Partial:

- The gateway listens on plain HTTP (`CONSOLE_LISTEN`, default
  `0.0.0.0:8080`). No code in this repo terminates TLS. TLS must be provided
  by the deployment and is not documented in code yet.
- No test checks the HSTS header. Test
  `tests::healthz_ok_with_security_headers` checks only CSP and
  `X-Content-Type-Options`.

## SC-18 / SI-10 Content Security Policy and browser hardening

**Status: Partial.** The headers are in code, but tests cover only part of
them.

`crates/console-gateway/src/main.rs` `content_security_policy` builds:

```text
default-src 'self'; script-src 'self' 'wasm-unsafe-eval' <sha256 hashes>;
style-src 'self'; frame-src 'self'; frame-ancestors 'none'; base-uri 'none';
form-action 'self'
```

At startup it computes the hashes from the inline `<script>` bodies in the
served `index.html`. It never uses `'unsafe-inline'`. The gateway also sets
`X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`.

The UI is written so that it does not need `'unsafe-inline'` for styles:

- Test `no_inline_styles` (`crates/console-ui/tests/guards.rs`): no `style=`
  in console-ui or console-shell source.
- Test `no_raw_html` (same file): no raw-HTML injection APIs.
- Test `index_html_is_csp_clean` (`crates/console-ui/tests/qa_logic_css.rs`):
  no `style=`, `<style>` or inline `<script>` in the shell's `index.html`.

Gateway tests (`crates/console-gateway/src/main.rs`):

- `tests::healthz_ok_with_security_headers`: CSP contains
  `frame-ancestors 'none'`, contains no `unsafe-inline`, and `nosniff` is set.
- `tests::hashes_only_inline_scripts`: only scripts without a `src` are
  hashed.

Browser E2E (M2):

- `e2e/tests/csp.spec.ts` "no CSP violations and no non-empty [style]
  attributes across interactions" (gallery bundle, port 8081).
- `e2e/tests/dist-smoke.spec.ts` "release dist: / loads with zero CSP
  violations" (release `dist`, port 8082), annotated `NIST 800-53: SC-18`.
- Both passed locally on Chromium and WebKit. Firefox has not been run, and
  the CI `e2e` job has not had a green run yet.

### Known benign CSP reports

WebKit reports a `style-src-attr` violation every time an `HTMLSelectElement`
is created, even with no app code (minimal repro with
`document.createElement('select')`). It is engine behaviour, not a style the
console sets. The tests allow it **on WebKit only**, and only when the report
matches exactly `{violatedDirective: "style-src-attr", blockedURI: "inline",
sample: ""}` with a target tag of `SELECT` or `null`, up to the number of
`<select>` elements plus 1 per page (`e2e/tests/csp-helpers.ts`). Every other
report fails on every engine, and Chromium must report 0.

Residual risk: a first-party inline-style violation with a `null` target on
WebKit would also match (bounded by the cap and the `[style]` DOM check).
`'report-sample'` is not in the CSP, so the sample is always empty. Upstream
WebKit bug link: TBD. Tracked as POA&M item POAM-011. A future CSP report
collector must filter this triple.

Why only Partial: `Referrer-Policy` and the full CSP string have no unit test.
The browser E2E evidence is local only until the CI `e2e` job runs green.

## SA-11 Developer testing and evaluation

**Status: Implemented.**

`.github/workflows/ci.yml` has two test jobs, first green in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231) on commit `0a6f40e`:

- `wasm-test` (comment `POAM-003 / SA-11`): `wasm-pack test --headless
  --firefox crates/console-ui`, uploads artifact `wasm-pack-log`.
- `e2e`: builds `dist`, `dist-gallery` and the gateway, runs `npm ci`, `tsc`,
  `npm audit --audit-level=high`, then `npx playwright test` on chromium,
  firefox and webkit plus the `dist-*` smoke projects; uploads artifact
  `playwright-report`.

CI evidence: `wasm-test` (headless Firefox) 16 passed; `e2e` on chromium,
firefox, webkit and the three `dist-*` projects 147 passed, 24 skipped (axe is
Chromium-only by design), 0 failed. Local runs (headless Chrome wasm; chromium
and webkit e2e) agree. POAM-003 to POAM-006 are closed; axe engine scope stays
open as POAM-012.

## SA-11(1) Static code analysis

**Status: Implemented.**

`make lint` and the CI `rust` job (comment `SA-11(1)` in
`.github/workflows/ci.yml`) run `cargo clippy ... -D warnings` on the host
workspace (all targets) and on wasm32 for console-shell (with and without
`gallery`) and console-ui (all targets). A warning fails the build.

## SR-3, SR-11, CM-14 Supply chain controls

**Status: Partial.** The build inputs are pinned; signatures are not verified.

- Every third-party GitHub Action in `.github/workflows/ci.yml` is pinned to a
  full commit SHA with a version comment. `trunk-action` pins trunk
  `v0.22.1`, the same version the Containerfile installs.
- `deploy/Containerfile` pins `rust:1.99-bookworm` and
  `distroless/cc-debian12:nonroot` by `@sha256` digest, and installs
  `trunk@0.22.1 --locked`.
- `e2e/.npmrc` sets `save-exact=true`, `ignore-scripts=true` and
  `engine-strict=true`. `e2e/package.json` requires Node `>=24`, CI uses Node
  24, and `npm audit --audit-level=high` fails the CI job.

Why only Partial: these are configuration, not tests. The digest-pinned
container build and the SHA-pinned workflow ran green in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231). There is no signature or provenance verification, and no automated update
process (Dependabot/Renovate) for the pins.

## PL-8 Security and privacy architecture: scope

**Status: Partial (documented design rule, nothing to enforce yet).**

The global scope (site, tenant, environment) is a **UI filter, not an
authorization claim**. It lives only in the URL query (never in
`localStorage`; E2E "scope is never written to localStorage"). Upstreams must
not trust scope values a tool echoes back, and M4 must re-authorize scope on
the server. No server-side check exists yet.

## RA-5 / SI-2 Dependency vulnerability scanning

**Status: Partial.**

`make deny` and the CI `cargo-deny` job run `cargo deny check`. `make audit`
runs `cargo audit`. Two advisories are waived in `deny.toml`. Both are
recorded in the POA&M.

These are gates, not tests of the system. Flaw remediation (SI-2) has no
process documented in code yet.

## Planned controls

None of the following has any code yet. Do not cite them as implemented.

- **AC-2, AC-3, AC-6**: roles and access enforcement come with OIDC and the tool
  registry (M3-M4). In M2 the user menu shows a static "Demo User (demo)",
  "Sign out" only shows an info flash, and the service menu is unfiltered
  demo data (POAM-010).
- **AC-11, AC-12**: idle timeout and logout (M3). See
  `docs/compliance/SESSION_MANAGEMENT.md`.
- **AU-2, AU-3, AU-12**: audit event stream (M3). See
  `docs/compliance/AUDIT_LOGGING.md`.
- **IA-2, IA-2(12)**: Keycloak OIDC with CAC/PIV (M3). Expected to become
  Inherited from Keycloak once it is integrated.
- **SC-23**: session cookies (M3).

## Related evidence (not a NIST control)

WCAG 2.1 AA colour contrast: test `token_pairs_meet_wcag_aa` in
`crates/console-ui/tests/contrast.rs` checks 40 declared token pairs in each
theme against 4.5:1 (text) or 3:1 (UI). axe scans run in
`e2e/tests/a11y.spec.ts` on Chromium only (0 serious/critical locally and
in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231)). See POAM-004 and POAM-012.
