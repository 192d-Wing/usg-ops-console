# hello-tool

The template console tool and the E2E target.

**Integrating your own tool:** the contract (assertion verification, SDK v1
messages, proxy headers/cookies/CSP, error codes, session model, focus and
keyboard rules) is in [docs/INTEGRATING-A-TOOL.md](../../docs/INTEGRATING-A-TOOL.md).
This README only describes this example and its test fixtures; where the two
differ, INTEGRATING-A-TOOL.md wins.

It has two crates:

- `web/` (`hello-tool-web`): Yew/WASM UI that uses the `console-sdk` hooks (feature `web`).
- `server/` (`hello-tool-server`): axum backend. It verifies `X-Console-Assertion` against the gateway JWKS and serves the web build under `/t/<id>/`.

One process serves two registry ids:

- `hello` is the normal tool.
- `evil` is a hostile fixture with its own origin (`:9448` in dev). It tries to escape the sandbox, and every attempt must fail.

## Build and run (host process, as in the dev stack / E2E)

```bash
(cd examples/hello-tool/web && trunk build --release)
CONSOLE_ISSUER=https://localhost:9443 \
CONSOLE_JWKS_URL=http://127.0.0.1:8080/.well-known/jwks.json \
cargo run --release -p hello-tool-server
```

| env | default | meaning |
|---|---|---|
| `CONSOLE_ISSUER` | required | Console origin. Must equal the assertion `iss`, which is the gateway's `CONSOLE_PUBLIC_URL` origin. |
| `CONSOLE_JWKS_URL` | required | Gateway JWKS. An internal URL is fine. Verification reads a shared snapshot (no lock held across the network). After 5 min one request refreshes the keys (single-flight) while the others keep using the last good keys; a failed refresh keeps them. After 10 min without a successful fetch, verification fails closed with a 503. An unknown `kid` refetches at most once per minute. Fetches have a 5 s timeout and a streamed 64 KiB size limit. |
| `HELLO_LISTEN` | `127.0.0.1:8090` | Listen address. Only the gateway may be able to reach it. |
| `HELLO_TOOL_IDS` | `hello,evil` | Registry ids to serve (`/t/<id>/`). Each id is also the required `aud`. |
| `HELLO_EVIL_ID` | `evil` | Which id renders the hostile page and gets the permissive CSP. Set it empty to disable. |
| `HELLO_WEB_DIST` | `examples/hello-tool/web/dist` | Trunk output. |

Container image (template): `podman build -f examples/hello-tool/Containerfile -t hello-tool .`

Registry entry expected by the server and the manifest test:

```toml
[[tool]]
id = "hello"
# origin, upstream, ... per deployment
required_roles = ["hello:user"]
scopes = ["site", "tenant"]
forward_headers = ["x-hello-csrf"]   # the tool's own CSRF header for POST api/counter
audit_writes = true                  # optional; POST api/counter is the state-changing request
```

## Backend API (under `/t/<id>/`; every request needs a valid assertion with `aud == <id>`)

| route | response |
|---|---|
| `GET api/whoami` | `{tool_id, verified: true, claims: AssertionClaims}` |
| `GET api/headers` | `{headers: [{name, value}]}`: what actually arrived, sorted by name. The assertion value is shown as `<verified>`, and `cookie` is reduced to cookie names. |
| `GET api/ping` | `pong` |
| `GET api/counter` | `{count}` for this user and tool |
| `POST api/counter` | `{count}` after incrementing. Without `X-Hello-CSRF: 1` it returns 403 `{"error":"csrf"}`. |
| anything else | the static web build. An unknown path returns the SPA with `<base href="/t/<id>/">`. |

Errors: 401 `{"error":"invalid_assertion"}` for a missing, invalid, expired, or wrong-`aud` assertion. 503 when the JWKS is unavailable. `GET /healthz` is unauthenticated.

CSP: the server sends its own policy: `default-src 'self'`, inline Trunk bootstrap allowed by hash, and `connect-src`, `form-action`, and `frame-src` set to `'self'`. The gateway adds `frame-ancestors`, and the browser enforces both policies. For `evil` the server opens `connect-src`, `form-action`, and `frame-src` to `*`, so that its attempts are stopped by the console and not by the fixture itself.

## Test ids

hello (`/t/hello/...`):

| `data-testid` | content |
|---|---|
| `hello-root` | `<main>`. `data-page` is `/` or `/headers`. |
| `hello-connection` | `data-connected="true"` after `Init` |
| `hello-nav-overview`, `hello-nav-request-headers` | in-tool links. Each sends `PathChanged`. |
| `hello-verified` | `yes` once `api/whoami` succeeded |
| `hello-user-name`, `hello-user-username`, `hello-user-sub`, `hello-user-roles` | from the verified claims |
| `hello-claim-aud`, `hello-claim-iss`, `hello-claim-ctx` | claims. `ctx` is JSON and is re-fetched on every `ScopeChanged`. |
| `hello-init-user` | `Init.user.display_name` (display only) |
| `hello-scope-site`, `hello-scope-tenant`, `hello-scope-environment` | SDK scope. `(none)` when unset. |
| `hello-theme` | SDK theme. `data-theme` on `<html>` is set by the SDK. |
| `hello-storage-visits` | counter in this tool origin's `localStorage` (`hello-tool.visits`) |
| `hello-notify` | button that sends `Notify{success, "Hello from Hello Tool"}` |
| `hello-counter-increment`, `hello-counter-value` | button for `POST api/counter` (with CSRF header); shows `Count: N` |
| `hello-headers`, `hello-headers-table`, `hello-header-<name>` | headers page. Each row is a lowercase header name. |
| `hello-flashbar` | local error flashbar |

Titles and breadcrumbs: Overview gives `SetTitle("Overview")` and breadcrumbs `[["Overview",""]]`. Headers gives `SetTitle("Request headers")` and breadcrumbs `[["Overview","/"],["Request headers",""]]`.

evil (`/t/evil/...`): `evil-root` (`data-ready="true"` once `/_console/context` has loaded), and `evil-run-<name>` / `evil-result-<name>` for each attempt. A result is one of:

- `blocked: ...`: the browser or console refused the attempt.
- `ESCAPED: ...`: the attempt worked. Tests must fail on this.
- `sent: N` / `sent: opaque` / `submitted` / `no-exception (check shell URL)`: the attempt was fire-and-forget. Assert on the shell or the console session instead.

`evil-shell-messages` (`data-count`) counts every message the shell posts back to the evil frame (source `window.parent`, console origin). Evil never sends `Ready`, so after any probe the count must stay `0`: the probes were delivered (the fixture uses the cross-origin `parent.postMessage`, which really sends) and the shell ignored them.

Unattended refresh probe (SEC-002 / NIST-001): open `/tools/evil/?probe=refresh-loop` and do not click or type. The fixture sends `RequestSessionRefresh` every 5 s; `evil-refresh-loop` shows `data-replies`, `data-ok-true` and `data-idle-expires-at`. With no trusted input in the shell, `data-ok-true` must stay `0`, `data-idle-expires-at` must not move, and the console session must expire on schedule. A click into the frame or shell counts as user input and allows one refresh per 30 s for the next 60 s.

| name | attempt | expected |
|---|---|---|
| `parent-dom` | `parent.document.body` | `blocked: SecurityError...` |
| `parent-storage` | `parent.localStorage` | `blocked: SecurityError...` |
| `frame-element` | `window.frameElement`, then remove `sandbox` | `blocked: frameElement is null` |
| `top-navigation` | `top.location.href = https://example.invalid/evil` | blocked or no-exception. The shell URL must not change. |
| `postmessage-shapes` | non-JSON, array, missing fields, unknown type, `javascript:` breadcrumb, bad level, 17 KiB, non-string object | `sent: 8`. No flash appears; the only visible effect is a plain-text `x` breadcrumb with no link (the `javascript:` path is refused). `evil-shell-messages` stays `0`. |
| `notify-html` | Notify whose message is an `<img onerror>` probe plus `evil-html-probe` | rendered as text only: no `img[data-evil-probe]` in the shell, and the title is not `pwned` |
| `notify-flood` | 20 Notify messages | `sent: 20`. The shell shows at most 5, plus one rate-limit notice. |
| `navigate-console-bad` | `NavigateConsole{tool:"denied"}` | ignored. The shell URL is unchanged. |
| `api-keepalive-cors` | credentialed CORS POST to the console keepalive | `blocked: TypeError...` |
| `api-keepalive-no-cors` | no-cors credentialed POST | `sent: opaque`. The console session must still be valid and its idle deadline unchanged (CSRF/Origin check). |
| `api-session-read` | credentialed CORS GET `/api/session` | `blocked: TypeError...` |
| `logout-form` | form POST to console `/logout` into the hidden `evil-sink` frame | `submitted`. The console session must still be valid. |
