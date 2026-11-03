# NIST SP 800-53 Rev. 5 control mapping

Status as of M1 (console-ui core components and design tokens), 2026-11-03.
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
| SI-10 Input validation (`safe_href`, theme allow-list) | Implemented | M1 |
| SC-8 Transmission confidentiality (HSTS header) | Partial | M0 |
| SC-18 / SI-10 Content Security Policy (hash-based) | Partial | M0 |
| RA-5 / SI-2 Dependency vulnerability scanning | Partial | M0 |
| AC-2, AC-3, AC-6 Account management, access enforcement | Planned | M3-M4 |
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

Limits: the gallery has no authentication. That is acceptable only while it
stays compiled out of release builds. `make run` serves the gallery bundle and
is for local development only.

## SI-10 Information input validation

**Status: Implemented** for the two M1 input points listed here. Other input
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
POA&M item SEC-003.

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

Why only Partial: `Referrer-Policy` and the full CSP string have no test. A live
check showed no CSP violations on `/gallery`
(`findings-orchestrator-browser.md`, M1 run). That was a manual check, not an
automated test.

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
  registry (M3-M4).
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
theme against 4.5:1 (text) or 3:1 (UI). An axe scan has **not** been run. See
the POA&M.
