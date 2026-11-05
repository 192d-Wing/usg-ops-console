# Integrating a tool

Status as of M4 (tool registry, proxy, container, SDK v1), 2026-11-04.
This is the contract between the console and an independently deployed
tool. It describes what the code in this repository does today. The
reference implementation is [examples/hello-tool](../examples/hello-tool/README.md).
Where the example README and this file differ, this file wins.

M4 is verified locally and in CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284)
(all jobs, including E2E on Chromium, Firefox and WebKit).

## Overview

```text
Browser (console tab)                        Gateway                             Tool upstream
 shell https://console.<domain>
  POST /api/tools/<id>/launch  ------------> role check, mint launch code (30 s)
  <- {"url": "https://<id>.tools.<domain>/_console/launch?code=..."}
  iframe src=url  -------------------------> (tool origin) check launch context,
                                             take code, create tool session,
  <- 303 /t/<id><path> + Set-Cookie __Host-tool_sid
  iframe GET /t/<id>/...  -----------------> tool session -> console session check,
                                             rebuild headers, sign assertion -------> X-Console-Assertion
  postMessage (SDK v1)  <====================================================> shell
```

- Each tool has **its own browser origin** (`https://<id>.tools.<domain>`),
  served by the same gateway through `Host` routing. The console origin
  never serves tool content (404 for `/t/*` and `/_console/*`).
- The registry (operator-owned TOML, see [DEPLOYMENT.md](DEPLOYMENT.md#tool-registry))
  is the only runtime source of tool configuration. Your
  `console-tool.toml` manifest is a presentation template and CI lint
  input; it cannot change the registry and is not fetched at runtime.
- Identity reaches your backend only as a gateway-signed assertion
  header. Never trust any other identity header.

## Integration checklist

1. **Launch context.** `GET <tool origin>/_console/launch?code=` is accepted
   only from the console's tool iframe: `Sec-Fetch-Dest: iframe` and
   `Sec-Fetch-Site: same-site` or `same-origin`; when no `Sec-Fetch-*`
   headers are present, a `Referer` whose origin is the console origin.
   A `Referer` from any other origin is always refused. Refused: 401, no
   cookie, the code is **not** consumed, audit `tool.denied`
   `launch_context`. Do not link users to launch URLs directly.
   (`tool_origin.rs` `launch_context_ok`; test
   `launch_context_requires_console_iframe`, `pg_tool_session_lifecycle`,
   E2E `tool-isolation.spec.ts`.)
2. **404 hiding.** A tool the user lacks roles for is indistinguishable
   from an unknown id: launch answers 404 `{"error":"not_found"}` with an
   identical body and headers, and the shell shows "Tool not found". The
   proxy returns 403 `forbidden` only when a role is revoked mid-session
   (the user already holds a tool session). (Tests
   `pg_tool_role_denied`,
   `router::forbidden_tool_launch_matches_unknown_id_without_awaiting_db`,
   E2E `tools-norole.spec.ts`.)
3. **Sessions.** Up to **8** tool sessions per (console session, tool).
   Each launch adds one; the 9th evicts the oldest by launch time. An
   evicted tab gets 401 on its next request and must be relaunched from
   the console. All tool sessions are deleted with the console session
   (logout, expiry, revocation). (`tool_session.rs` `MAX_TOOL_SESSIONS`;
   migration `0006_tool_sessions_multi.sql`; E2E `tool-isolation.spec.ts`.)
4. **Errors.** `bad_request` is a defined error kind. Tolerate unknown
   `error` and `reason` values (see [Errors](#errors)).
5. **Message decoding.** Non-object JSON (`"x"`, `1`, `null`, `true`,
   `[1]`, `[]`) is `Err(Malformed)`. An object with an unknown, missing or
   non-string `type` is `Ok(None)` and must be ignored.
6. **Focus edges.** See [Keyboard and focus](#keyboard-and-focus).
7. **Refresh gating.** `RequestSessionRefresh` is honoured only with
   recent user presence in the shell; otherwise you get
   `SessionRefreshed{ok:false}` with the current deadline. Your own
   activity reports are ignored. See [Session refresh](#session-refresh).
8. **Reserved query keys.** The shell URL reserves `site`, `tenant` and
   `env`. See [Scope](#scope-ctx-is-a-selection-not-authorization).
9. **`Init.user.roles` is always empty** (reserved in v1). Authorize from
   the assertion `roles` (filtered by the registry `forward_roles`) or from
   your own backend.
10. **Registry** (operator): optional `schema = 1` (any other value fails
    boot); unknown keys fail boot, so add a new key only after every
    replica, including the rollback target, understands it;
    `insecure_http_upstream = true` requires
    `insecure_http_upstream_reason` (1-200 characters) and is audited at
    startup. Details: [DEPLOYMENT.md](DEPLOYMENT.md#tool-registry).
11. **`CONSOLE_TOOL_SESSION_HMAC_KEY_FILE`** is required outside dev mode
    when the registry has tools: at least 32 bytes; raw binary is used as
    is, base64 text is decoded if it yields at least 32 bytes, other text
    is trimmed and used as bytes. Changing the key (including the dev key)
    acts like a rotation: every `sid` pseudonym changes.
12. **Upstream `Location`.** Allowed: path-relative (`/x`, `x`, `?q`,
    `#f`), absolute on your upstream authority (rewritten to the tool
    origin), or absolute on the tool origin. Anything else (another origin,
    `//host`, backslashes, `javascript:`, `data:`, control characters) is
    answered 502 `bad_gateway`. Redirects to an IdP must be done in the
    browser, not as a 30x through the proxy.
13. **Operator capacity:** `CONSOLE_DB_MAX_CONNECTIONS` defaults to 32
    (about 36 connections per replica including the audit pool). For 3 or
    more replicas raise Postgres `max_connections` or use session-mode
    pooling ([DEPLOYMENT.md](DEPLOYMENT.md#database-connection-budget)).

## Identity assertion

Every proxied request, including static assets, carries a fresh
`X-Console-Assertion` header: a compact JWS. Any client-supplied value is
stripped. The assertion is never given to the browser.

JOSE header: `{"alg":"ES256","typ":"console-assertion+jwt","kid":"<RFC 7638 thumbprint>"}`.

Claims (`console_protocol::tools::AssertionClaims`; unknown claims must be
ignored):

| Claim | Value |
| --- | --- |
| `iss` | Console origin (`CONSOLE_PUBLIC_URL` origin) |
| `aud` | Your tool id (single string, not an array) |
| `sub` | IdP subject |
| `preferred_username`, `name` | From the IdP; `""` when absent |
| `roles` | Session roles filtered by the registry `forward_roles` prefixes |
| `ctx` | `{site, tenant, environment}`, each string or `null`; the user's selection (below) |
| `sid` | Per-session, per-tool pseudonym (HMAC, 16 bytes base64url). Stable within one console session for your tool; not linkable across tools; not the console session id |
| `jti` | 128-bit random, unique per request |
| `iat`, `nbf`, `exp` | `nbf = iat`, `exp = iat + 60` |
| `acr` | Authentication context from the session, when present |

### Verification rules

Your backend must:

- Pin `alg = ES256` and `typ = console-assertion+jwt`. Require `kid`.
- Select the key by `kid` from the JWKS at
  `<console origin>/.well-known/jwks.json` (an internal URL to the gateway
  is fine). Use only EC P-256 keys that have a `kid`.
- Require `iss` = the console origin, `aud` = your own id, and `exp`,
  `nbf`, `sub`. Leeway at most 30 s (`ASSERTION_MAX_LEEWAY_SECS`).
- Cache the JWKS for at most 10 minutes (the gateway serves it with
  `Cache-Control: public, max-age=300`). On an unknown `kid`, refetch at
  most once a minute. Serve the last good keys while a refetch runs and
  fail closed (503) if no fetch has succeeded within your cache limit.
  Bound the fetch (timeouts, response size, no redirects).
- Reject any request under `/t/<id>/` without a valid assertion.
- Only accept the header from the gateway's network path (do not expose
  the upstream directly to browsers).

Replay window: a captured assertion is valid for at most `exp` + leeway
(about 90 s) and only at your tool (`aud`). If you need stronger replay
protection for state-changing requests, keep a `jti` cache. The gateway
still does all session checks on every request.

Key rotation (operator): publish the next key in
`CONSOLE_ASSERTION_PUBLISH_KEYS_FILE`, wait at least JWKS max-age plus
your cache time, switch the signing key, and keep the old public key
published for at least 2 x TTL plus cache time. Keys load at startup only.

The hello-tool reference verifier is
`examples/hello-tool/server/src/assertion.rs` (tests
`cold_cache_fetch_is_single_flight`, `serves_last_good_keys_then_fails_closed`,
`oversized_jwks_is_refused`).

**Supply-chain note.** The hello-tool server verifies with `jsonwebtoken`
(`rust_crypto` backend), which pulls in `crypto-bigint` 0.5 with advisory
RUSTSEC-2026-0387 (not constant-time). That is acceptable for public-key
verification only. If you copy hello-server, you inherit this advisory and
must track it in your own POA&M (the console tracks it as POAM-033). Never
sign or handle private keys with that stack.

## Scope: `ctx` is a selection, not authorization

The SDK `console_fetch` helper sends `X-Console-Scope: site=..;tenant=..;environment=..`.
The gateway keeps only the keys listed in your registry `scopes`, checks
syntax (`[A-Za-z0-9._-]{1,64}`) and copies them into `ctx`. A malformed
header is dropped entirely (`ctx` all `null`). `X-Console-Scope` is not
forwarded raw.

`ctx` is the user's **current selection**, taken from a client header. The
gateway does not authorize it (SEC-003). You must check that `sub` /
`roles` may see the selected site, tenant or environment.

Naming (INT-008): the shell URL key is `env`; the scope header, `ctx` and
`Scope` field are `environment`. The shell URL reserves `site`, `tenant`
and `env`: if your tool uses its own query parameters with those names,
they are dropped from the mirrored shell URL. Namespacing these keys
before wider adoption is an open item (POAM-040).

## SDK protocol v1 (postMessage)

`PROTOCOL_VERSION` is 1. Wire format: `postMessage(<JSON string>)`, an
object `{"type":"<Variant>", ...}`. Messages over 16 KiB are dropped by
both sides. Unknown fields are ignored; variants and fields are never
removed or renamed in v1. Frozen fixtures:
`crates/console-protocol/tests/fixtures/sdk_v1/`.

Shell to tool (`ShellToTool`):

| Type | Fields | Meaning |
| --- | --- | --- |
| `Init` | `protocol`, `theme`, `locale`, `scope`, `user`, `path?`, `tool_id?` | Answer to every `Ready`. `theme` is `light`, `dark` or `system`. `user` is display-only; `user.roles` is always `[]` |
| `ScopeChanged` | `scope` | User changed scope (no reload) |
| `ThemeChanged` | `theme` | User changed theme |
| `Navigate` | `path` | Tool-relative path; the user used back/forward or a nav link |
| `SessionRefreshed` | `ok`, `idle_expires_at?` (RFC 3339) | Answer to `RequestSessionRefresh` |
| `Focus` | `edge`: `first` or `last` | Move focus to your first or last tabbable element |

Tool to shell (`ToolToShell`):

| Type | Fields | Shell behaviour |
| --- | --- | --- |
| `Ready` | `protocol` | Handshake; shell replies `Init` with `min(tool, 1)` |
| `SetTitle` | `title` | Document title `<title> - <tool> - USG Ops Console`; at most 120 characters, control characters removed |
| `SetBreadcrumbs` | `items: [[label, path], ...]` | `[Console, <tool>, ...]`; at most 8 crumbs, labels at most 80 characters; `""` or unsafe path = plain text |
| `Notify` | `level` (`info`, `success`, `warning`, `error`), `message` | Flashbar text prefixed with the tool title; plain text, at most 500 characters; 5 per 10 s, then one "too many notifications" notice |
| `SetBadge` | `key`, `count` (number or `null`) | At most 8 keys of at most 32 characters; 0 or `null` removes |
| `NavigateConsole` | `tool`, `path` | Only to a tool in the user's list with a safe path; otherwise ignored |
| `RequestSessionRefresh` | none | Gated (below) |
| `PathChanged` | `path` | Shell `replaceState` to `/tools/<id><path>` |
| `FocusShell` | `reverse` (default `false`) | Focus leaves the tool: first top-bar control, or with `reverse` the last side-nav link |

Origin rules: the shell posts only to your registry origin and accepts
only messages whose `origin` equals it **and** whose `source` is your
iframe. Your SDK must post to the console origin from
`GET /_console/context` (`{tool_id, console_origin, protocol}`) and accept
only that origin with `source === window.parent`. Never use `"*"`, never
use `document.referrer`. A message from any other origin or window is
ignored (E2E `tool-isolation.spec.ts`, NIST-007).

Handshake: the Rust SDK (`console_sdk`, feature `web`) sends `Ready`
immediately and again at 250 ms, 500 ms and 1 s; after 2 s with no `Init`
it reports `Standalone` (a later `Init` still connects). The shell shows a
non-blocking "not connected" warning if no `Ready` arrives within 10 s of
iframe `load`; the iframe stays usable.

The Rust/Yew API (`console_sdk::hooks`): `ConsoleProvider`, `use_console`,
`use_console_status`, `use_console_scope`, `use_console_user`,
`use_console_theme`, `use_console_navigate`, plus `console_fetch`,
`set_title`, `set_breadcrumbs`, `notify`, `set_badge`, `path_changed`,
`request_session_refresh`, `navigate_console`. There is no TypeScript SDK
or JSON Schema yet (INT-002, POAM-038, M5).

## Session refresh

The console uses strict idle timeout (AC-11). A tool cannot keep the
console session alive by itself.

- The shell honours `RequestSessionRefresh` only if it saw trusted user
  presence in the last **60 s** (`REFRESH_INPUT_WINDOW_MS`), or the idle
  warning dialog is open. Presence is a trusted (`isTrusted`)
  `pointerdown`/`keydown` in the shell, or focus moving into your iframe
  (counted only for the first entry per page load, or after a trusted
  shell input following the last observed exit).
- It also applies at most one refresh per 30 s and one in flight.
- Otherwise the reply is `SessionRefreshed{ok:false, idle_expires_at:<current>}`
  and nothing is extended. Activity your tool reports is never trusted.
- **Limitation:** input inside your cross-origin frame is invisible to the
  shell. A user who works only inside your tool for longer than the idle
  timeout gets the normal idle warning and must choose "Stay signed in".
- Residual accepted risk (SEC-012): before the first frame exit, a tool's
  programmatic self-focus cannot be told from the user's first click, so
  at most one faked entry per page load is possible.

Tests: shell `activity::tests::refresh_needs_recent_input_or_open_dialog`,
`only_untainted_entry_into_frame_counts`; E2E `idle-tool.spec.ts`,
`tool-security.spec.ts` (refresh spam: 0 keepalives).

Your tool requests never extend console idle and never move the absolute
deadline; the gateway rechecks both on every proxied request.

## Keyboard and focus

- **Do not call `focus()` on load.** The shell does not move focus into
  the tool on launch, and neither should you.
- **Let the SDK handle F6.** F6 and Ctrl+F6 send `FocusShell{reverse:false}`;
  Shift+F6 sends `reverse:true`. After sending, the SDK blurs your focused
  element so the next entry lands on the requested edge. Do not bind F6
  yourself.
- **Keep a visible first and last tabbable element.** `Focus{first|last}`
  moves to the first or last element that has client rects, is not inside
  `[inert]`, `[hidden]` or `[aria-hidden=true]`, and has `tabIndex >= 0`.
  The SDK applies it only when your `activeElement` is `<body>`, and waits
  (bounded) for `document.hasFocus()`.
- When the shell sends focus edges: skip link "Skip to tool content",
  F6/Ctrl+F6 into the tool (`first`), Shift+F6 (`last`); after a native
  Tab or Shift+Tab into the frame the shell sends `first` or `last` (it
  only takes effect where the browser left focus on your `<body>`, as
  Firefox does). A click into the frame sends nothing.
- **Firefox:** Shift+Tab from your first control stops once on the
  `<iframe>` element in the shell (a platform behaviour the shell cannot
  remove). The stop has the iframe title and a visible focus ring; the
  next Shift+Tab reaches the shell control before the frame. This is
  expected; see [compliance/ACCESSIBILITY.md](compliance/ACCESSIBILITY.md).
- Shell keyboard shortcuts do not fire while focus is inside your frame.
  This includes the console command palette (M4.5): Ctrl+K / Cmd+K
  pressed inside your frame stays with your tool and does not open the
  palette, and there is no SDK message to forward it (protocol v1 is
  unchanged). You may bind Ctrl/Cmd+K inside your tool. Users reach the
  palette by moving focus back to the shell (F6 or Shift+F6) or with the
  "Search…" button in the top bar. E2E: `palette.spec.ts` "theme from the
  palette reaches the tool (ThemeChanged); shortcut inside the tool frame
  does not open the palette".

E2E: `tool-a11y.spec.ts`. SDK wasm test:
`focus_edge_picks_visible_target_only_from_body`.

## Styling with console-ui tokens

Tools may style themselves with the console-ui design tokens
(`console_ui::TOKENS_CSS`, `assets/tokens.css`) and component styles
(`COMPONENTS_CSS`), served as files from the tool's own origin (see
[crates/console-ui/README.md](../crates/console-ui/README.md#using-the-styles-in-a-tool)).
M4.5 restyled the console ("calm neutral", pine teal accent). Token names
were kept and new tokens were only added, so existing tool stylesheets keep
working; tools may use the new tokens (for example
`--cui-color-surface-sunken`, `--cui-font-weight-medium`, `--cui-space-sm`).
`examples/hello-tool/web/assets/hello.css` uses only `--cui-*` tokens and is
the reference.

API and CSS changes for tools that use console-ui components (for example
new `BadgeColor` variants, the deprecated `SearchInput`, and the
`TopNavigation` slot order) are listed in
[crates/console-ui/CHANGELOG.md](../crates/console-ui/CHANGELOG.md).
`ThemeChanged` values (`light`, `dark`, `system`) are unchanged.

## Proxy contract

Base path: your app is served under `/t/<id>/` on your origin and
receives the path verbatim (including `/t/<id>/`). Build with relative
URLs and emit `<base href="/t/<id>/">` (hello-server injects it). `/t/<id>`
without a trailing slash answers 308 to `/t/<id>/`.

Request headers your upstream receives:

- Allowed client headers only: `accept`, `accept-language`,
  `accept-encoding`, `cache-control`, `content-type`, `content-length`,
  `content-encoding`, `if-match`, `if-none-match`, `if-modified-since`,
  `if-unmodified-since`, `if-range`, `range`, `origin`, `referer`,
  `user-agent`, `x-requested-with`, `last-event-id`, `sec-fetch-*`, plus
  your registry `forward_headers` (lowercase tokens, not in the reserved
  set).
- Always dropped: `authorization`, `forwarded`, `x-forwarded-*`,
  `x-real-ip`, `x-console-*`, `x-forwarded-user`, `x-remote-user`,
  `proxy-*` and all hop-by-hop headers.
- `cookie` is rebuilt without `__Host-console_*` and `__Host-tool_sid`;
  your own cookies pass.
- Added: `x-console-assertion`, `x-forwarded-for` (single value, the
  client IP as resolved by the gateway), `x-forwarded-proto: https`,
  `x-forwarded-host` (your tool host), `x-request-id`. `Host` is your
  upstream authority.
- Rejected with 400 `bad_request` before forwarding: `.`/`..` segments,
  encoded `.`, `/`, `\` or NUL, non-UTF-8 paths.

Responses from your upstream:

- Dropped: hop-by-hop, `proxy-*`, `strict-transport-security`, `server`.
- `Set-Cookie` is dropped if the name starts with `__Host-console` or is
  `__Host-tool_sid`, or if it has a `Domain` attribute. Your cookies stay
  host-only on your origin.
- `Location`: see checklist item 12.
- Added: CSP `frame-ancestors <console origin>; base-uri 'self'; object-src 'none'`
  (a second CSP header; your own CSP is kept and the two intersect),
  `X-Content-Type-Options: nosniff`, the gateway's HSTS,
  `Referrer-Policy: no-referrer` and `Cross-Origin-Resource-Policy: same-site`
  if absent, and `Cache-Control: private, no-store` if absent on HTML or on
  non-2xx/3xx.
- **WebSockets are not supported in v1:** `Upgrade` and `CONNECT` get 501
  `not_implemented` (planned for M5, POAM-034). Server-sent events work
  (streamed HTTP).

Limits (per tool, registry): request body `max_body_bytes` (default
10 MiB) else 413; `response_header_timeout_secs` (default 30) else 504;
`max_concurrent` (default 256) else 503 with `Retry-After: 1`; connect 3 s;
body idle 60 s; no total timeout (long downloads stream).

Iframe: `sandbox="allow-scripts allow-same-origin allow-forms allow-downloads allow-popups allow-modals"`,
`allow=""`, `referrerpolicy="no-referrer"`. No top navigation, no popups
escaping the sandbox. `allow-same-origin` means *your* origin.

## Errors

JSON `{"error":"<kind>"}` (some 401s add `reason`) when the request
`Accept` contains `application/json` (substring match, INT-008 M3
contract); otherwise a small HTML page styled by the console's `/auth.css`.
Tool-origin 401 JSON has no `login_url`: the shell owns re-login.

| Status | `error` | When |
| --- | --- | --- |
| 400 | `bad_request` | Invalid launch JSON or path; rejected proxy path |
| 401 | `unauthenticated` (`reason` `no_session`, `idle_timeout`, `absolute_timeout`) | No or expired tool session, bad or used launch code, refused launch context, evicted tab. Relaunch from the console |
| 403 | `forbidden` | Proxy: role revoked mid-session |
| 403 | `csrf` | Launch API CSRF/Origin failure (console origin) |
| 404 | `not_found` | Unknown tool, or a tool you lack roles for (launch); unknown path |
| 413 | `payload_too_large` | Body over `max_body_bytes` (or launch body over 2 KiB) |
| 421 | `misdirected_request` | This tool origin received `/t/<other-id>/...`. An unknown `Host` is served by the console app, never 421 |
| 429 | `rate_limited` | Launch over 60 per minute per session, or launch-code exchange over the per-IP limit |
| 501 | `not_implemented` | WebSocket `Upgrade` or `CONNECT` |
| 502 | `bad_gateway` | Upstream unreachable or failed, or a disallowed `Location` |
| 503 | `unavailable` | Session store down, or `max_concurrent` reached (`Retry-After: 1`) |
| 504 | `gateway_timeout` | Upstream response headers did not arrive in time |

Rules carried over from M3 (INT-007, INT-008, POAM-025):

- `error` and `reason` are open sets: tolerate unknown values (the
  protocol enums fall back to `Unknown`).
- Logout XHR detection on the console is `X-CSRF-Token` present or
  `Accept` containing `application/json`; `Accept: */*` without the header
  gets a 303. Keepalive 503 is retryable; 401 `revoked` is final.
- Pages under `/t/<id>/` may receive query parameters you did not define
  (for example `console_notice`): ignore unknown parameters.

## Launch-code handoff details

- Codes are 256-bit, single use, valid 30 s, bound to the console session
  and tool id. They are bearer secrets; the query is not logged.
- Success: a new `__Host-tool_sid` (`Secure`, `HttpOnly`,
  `SameSite=Strict`, `Path=/`, no `Domain`, no `Max-Age`) and 303 to
  `/t/<id><path>`.
- Residual (accepted LOW, SEC-001/SEC-010): `same-site` covers every host
  under the registrable domain, so console and tool origins must sit under
  a dedicated registrable domain. A malicious *registered* tool could
  frame another tool's launch URL with a code minted for the attacker's own
  session. A console-signed nonce bound to the shell frame is planned for
  M5 (POAM-035).

## Checklist for a new tool

- Verify the assertion as above on every request; return 401 without it.
- Authorize `ctx` and your actions from `sub`/`roles`, not from `Init`.
- Serve under `/t/<id>/` with relative URLs; no redirects to other origins.
- Use the SDK origin pin from `/_console/context`.
- Follow the keyboard rules; no focus on load.
- Ask the operator for a registry entry and, if you need extra request
  headers, list them in `forward_headers`.
