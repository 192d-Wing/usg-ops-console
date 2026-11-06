# Deployment

Status as of M4 (tool hosting), 2026-11-04. M4 is green in
CI run 41892456284. This covers what a deployment of
`console-gateway` needs today. The Helm chart is not built yet (M7); the
requirements below are what it must implement. Configuration reference:
[ARCHITECTURE.md](ARCHITECTURE.md#configuration).

## Components

- `console-gateway` (container image from `deploy/Containerfile`: the
  gateway binary plus the release shell `dist` in `/srv/console`).
- PostgreSQL (CNPG in the cluster) for sessions, pending sign-ins and the
  audit log.
- Keycloak realm `icam` (shared with usg-dcim) with a confidential client
  `usg-ops-console`.
- A TLS-terminating ingress or reverse proxy in front of the gateway,
  serving the console origin **and** every tool origin (M4).
- Tool upstreams (independently deployed), reachable from the gateway.

## Environment keys

Required:

| Key | Notes |
| --- | --- |
| `CONSOLE_PUBLIC_URL` | External `https://` origin users browse to |
| `CONSOLE_OIDC_ISSUER` | `https://<keycloak>/realms/icam` |
| `CONSOLE_OIDC_CLIENT_ID` | `usg-ops-console` |
| `CONSOLE_OIDC_CLIENT_SECRET_FILE` | Path to the mounted client secret |
| `CONSOLE_DATABASE_URL_FILE` | Runtime role `console` |
| `CONSOLE_AUDIT_DATABASE_URL_FILE` | Audit role `console_audit` |
| `CONSOLE_REQUIRED_ACR` | The `acr` value(s) icam's CAC/PIV flow emits. Value to be confirmed with the icam admins (POAM-028) |
| `CONSOLE_TRUSTED_PROXIES` | CIDRs of the ingress/proxy hops, so audit records and the `/login` rate limiter get the real client IP. Without it the peer address is recorded. List every address family the proxy actually connects from (for example both `127.0.0.1/32` and `::1/128` for a local proxy): if the proxy connects over IPv6 and only its IPv4 address is listed, every user shares the proxy's address, so one rate-limit bucket and one audit IP |

Optional, with defaults: `CONSOLE_LISTEN` (`0.0.0.0:8080`), `CONSOLE_DIST`,
`CONSOLE_OIDC_REDIRECT_PATH` (`/callback`), `CONSOLE_OIDC_DISCOVERY_RETRIES`
(10), `CONSOLE_DB_MAX_CONNECTIONS` (**32** since M4, was 10; see [Database connection budget](#database-connection-budget)), `CONSOLE_AUDIT_DB_MAX_CONNECTIONS`
(4), `CONSOLE_SESSION_IDLE_SECS` (900), `CONSOLE_SESSION_ABSOLUTE_SECS`
(43200), `CONSOLE_SWEEP_INTERVAL_SECS` (60), `CONSOLE_REQUIRED_ROLE`
(`console-user`), `RUST_LOG` (`info`), `CONSOLE_OIDC_REFRESH_TIMEOUT_SECS`
(4, range 1-10; keepalive refresh-grant timeout, then 503).

Tool hosting (M4):

| Key | Notes |
| --- | --- |
| `CONSOLE_TOOLS_REGISTRY_FILE` | Path to the registry TOML. Unset: no tools (startup warning). Set but unreadable or invalid: boot fails |
| `CONSOLE_ASSERTION_KEY_FILE` | PKCS#8 PEM P-256 private key (`-----BEGIN PRIVATE KEY-----`). Required when the registry has tools |
| `CONSOLE_ASSERTION_PUBLISH_KEYS_FILE` | Optional PEM bundle of SPKI public keys (`-----BEGIN PUBLIC KEY-----`) published in the JWKS next to the active key (rotation overlap) |
| `CONSOLE_TOOL_SESSION_HMAC_KEY_FILE` | Key for the per-tool `sid` pseudonym, at least 32 bytes. **Required when the registry has tools and `CONSOLE_DEV_MODE` is not true** (boot fails otherwise) |

Reference values: `deploy/compose/gateway.env.example` and
`deploy/compose/secrets/README.md` (DEV ONLY).

Must **not** be set in production:

- `CONSOLE_DEV_MODE`: it allows an unset `CONSOLE_REQUIRED_ACR`, DB
  connections without verified TLS, plain-http non-loopback tool
  upstreams, and a `sid` key derived from the assertion key.
- `CONSOLE_MIGRATOR_DATABASE_URL[_FILE]` on the long-running gateway (see
  below, POAM-027).

The gateway fails to start, naming the key, if a required key is missing
or a value is invalid.

## Secrets via `*_FILE`

The client secret and all three DB URLs (which contain passwords) can be
read from files: `CONSOLE_OIDC_CLIENT_SECRET_FILE`,
`CONSOLE_DATABASE_URL_FILE`, `CONSOLE_AUDIT_DATABASE_URL_FILE`,
`CONSOLE_MIGRATOR_DATABASE_URL_FILE`. Mount them from Kubernetes Secrets.
The direct env forms work but log a startup warning naming the `_FILE`
key. Secret values are never logged (`Secret` redacts in `Debug`).

## Tool origins, DNS and certificates

Each registry tool is served on **its own origin**, normally
`https://<id>.tools.<domain>`, a sibling of the console host (for example
console `https://console.ops.example.mil`, tool
`https://hello.tools.ops.example.mil`). The same gateway serves all of
them and routes on the `Host` header.

- Provide **wildcard DNS** for `*.tools.<domain>` pointing at the ingress,
  and a **wildcard certificate** for `*.tools.<domain>` (or one SAN per
  tool). Adding a tool then needs only a registry entry.
- **Dedicated registrable domain (SEC-010, required).** The console origin
  and all tool origins must sit under a registrable domain (eTLD+1) that
  hosts **nothing else**. Launch-code acceptance relies on
  `Sec-Fetch-Site: same-site`, which covers every host under the
  registrable domain; any other application on that domain would count as
  "same site". Accepted residual, POAM-035.
- **The ingress must pass `Host` unchanged** to the gateway for both the
  console and the tool hosts. An unknown `Host` is served by the console
  app; a tool origin that receives another tool's path answers 421.
- The registry validates that every `origin` is `https`, unique, and not
  equal to or a subdomain of the `CONSOLE_PUBLIC_URL` host, and fails boot
  otherwise.
- The console CSP `frame-src` is computed from the registry origins
  (`'none'` with no tools).
- Tool upstreams should be reachable only from the gateway (they trust
  the assertion header).

## Tool registry

Format: TOML, mounted read-only into the gateway (path in
`CONSOLE_TOOLS_REGISTRY_FILE`) and changed only through Git/Helm review.
It is the only runtime source of tool configuration; tool manifests are
not fetched. Validation is fail-closed and every error names the file, the
tool and the field.

```toml
schema = 1                          # optional; any other value fails boot

[[tool]]
id = "hello"                        # 1-32 chars [a-z0-9-], starts with a letter
title = "Hello Tool"
category = "Examples"               # optional
icon = "puzzle"                     # optional
origin = "https://hello.tools.ops.example.mil"
upstream = "https://hello.internal:8443/"   # path must be "/"; no userinfo or query
upstream_ca_file = "/etc/console/ca/hello.pem"   # optional extra trust anchor
required_roles = ["hello:user"]     # any-of; empty = any console user
forward_roles = ["hello:"]          # role prefixes copied into the assertion; default = required_roles
scopes = ["site", "tenant"]         # of site, tenant, environment
entry = "/"
max_body_bytes = 10485760           # default 10 MiB, max 100 MiB
response_header_timeout_secs = 30   # default 30, 1-300
max_concurrent = 256                # default 256
forward_headers = ["x-hello-csrf"]  # extra request headers; reserved names refused
audit_writes = false                # audit unsafe-method requests as tool.request
[[tool.nav]]
label = "Overview"                  # at most 64 chars, at most 32 items
path = "/"
```

Limits: at most 64 tools. Roles named in any `required_roles`, or matching
a `forward_roles` prefix, are kept on the session in addition to
`console-*`; they must reach the id_token `realm_access.roles` claim
(Keycloak realm roles mapped into the id_token). Role changes apply at the
next login or keepalive.

**Schema stability and rollback.** Unknown keys fail boot (a typo must not
silently drop a control). Add a new key to a deployed registry only after
every gateway replica, **including the rollback target**, understands it;
otherwise a rollback cannot start.

**Plain-http upstreams.** Outside dev mode, a non-loopback upstream must be
`https` with full certificate verification. `insecure_http_upstream = true`
allows plain http only together with
`insecure_http_upstream_reason = "<1-200 characters>"` (a reason without
the flag is an error). The gateway logs a warning and writes a
`registry.insecure_upstream` audit event at every start (boot fails if
that write fails). This is a CM-6 deviation: it is acceptable only with a
**compensating control** such as service-mesh mTLS between gateway and
upstream, which the gateway does not verify (POAM-037). Record the control
for each tool that sets it.

Every start also writes `registry.loaded` (fail closed).

## Tool keys

- **Assertion signing key** (`CONSOLE_ASSERTION_KEY_FILE`): P-256, PKCS#8
  PEM. The `kid` is its RFC 7638 thumbprint.
- **Published keys and JWKS.** `GET <console>/.well-known/jwks.json` is
  public and served with `Cache-Control: public, max-age=300`; tools cache
  it for at most 10 minutes. It lists the active key plus
  `CONSOLE_ASSERTION_PUBLISH_KEYS_FILE` keys. Keys load at startup only.
- **Rotation runbook:** add the new public key to the publish bundle and
  restart; wait at least JWKS max-age plus the tool cache (10 min); switch
  `CONSOLE_ASSERTION_KEY_FILE` to the new key (keep the old public key in
  the bundle) and restart; after at least 2 x assertion TTL (60 s) plus
  the cache time, remove the old key.
- **sid HMAC key** (`CONSOLE_TOOL_SESSION_HMAC_KEY_FILE`): at least 32
  bytes after decoding. The file is read as bytes: UTF-8 text whose
  trimmed form is standard base64 and decodes to at least 32 bytes is
  decoded; other UTF-8 text is trimmed and used as bytes; non-UTF-8
  content is used raw. Example: `openssl rand -base64 48 > key` (decodes to
  48 bytes). Changing it changes every `sid` pseudonym (tools see new
  values), the same as a rotation. Use the same file on every replica.
- Mount all three from Kubernetes Secrets; values are never logged.

## Database connection budget

`CONSOLE_DB_MAX_CONNECTIONS` defaults to **32** (range 1-200) and
`CONSOLE_AUDIT_DB_MAX_CONNECTIONS` to 4, so each replica can open **36**
connections. Stock Postgres and CNPG default to `max_connections = 100`
(3 reserved), which fits two replicas with headroom for the migrator and
admin sessions.

- **For 3 or more replicas:** raise Postgres `max_connections`, or lower
  the per-replica pool, or use a **session-mode** pooler.
- **Transaction-mode pooling is not supported** (PgBouncer or CNPG Pooler
  in transaction mode): sqlx uses per-connection prepared statements and
  the gateway has no setting to turn the statement cache off (POAM-042).
- The pool does not ping before use; idle connections close after 300 s
  and live at most 30 min. Set any load balancer or proxy idle timeout
  between gateway and Postgres **above 300 s**. After a DB failover, the
  read-only session lookup retries once on a connection-class error;
  other requests may see 503 until the pool refreshes.
- The proxy path holds one connection per request only for the session
  lookup (one indexed join), never during upstream I/O.

Sizing is tracked as POAM-041.

## Database roles and migrations

Create these roles before running migrations (CNPG managed roles or a
DBA); the migrations fail if `console_owner`, `console_app` or
`console_audit` are missing:

| Role | Attributes | Grants |
| --- | --- | --- |
| `console_owner` | NOLOGIN | Owns the `console` database and schema `public` |
| `console_app` | NOLOGIN | `USAGE` on schema `public` (table DML is granted by migration 0003) |
| `console_audit` | LOGIN, NOCREATEDB, NOCREATEROLE | `CONNECT`; `USAGE` on schema `public` (INSERT, SELECT on `audit_events` granted by migrations) |
| `console_migrator` | LOGIN, NOCREATEDB, NOCREATEROLE | `GRANT console_owner TO console_migrator`; `CONNECT` |
| `console` | LOGIN, NOCREATEDB, NOCREATEROLE | `GRANT console_app TO console`; `CONNECT` |

Also `REVOKE ALL ON DATABASE console FROM PUBLIC` and `REVOKE ALL ON
SCHEMA public FROM PUBLIC`. The reference SQL is
`deploy/compose/postgres/init/01-roles.sh`; verify the result after
migrating with `psql -f deploy/compose/postgres/check-roles.sql` (prints
`SEC-006 role contract OK`).

M4 migrations: `0004_session_idle.sql` (per-session idle limit),
`0005_tool_sessions.sql` (tool sessions, launch codes, audit columns),
`0006_tool_sessions_multi.sql` (up to 8 tool sessions per console
session and tool). Run `console-gateway migrate` before rolling out M4.

Run migrations as a separate one-shot job or init container:

```bash
CONSOLE_MIGRATOR_DATABASE_URL_FILE=/run/secrets/migrator-url console-gateway migrate
```

`migrate` reads only the migrator URL and `RUST_LOG`, runs `SET ROLE
console_owner` on each connection, and exits. Keep the migrator URL off the
long-running gateway: if it is set, `serve` migrates at startup, and the
runtime process could assume `console_owner`, which can bypass the audit
log's append-only protection (POAM-017, POAM-027). Without it, `serve`
checks the schema and stops with `database schema not migrated; run
console-gateway migrate` if it is behind.

DB connections to a non-loopback host must use `sslmode=verify-full` (or
`verify-ca`) in the URL. Use `sslrootcert=` for a private CA.

## Keycloak client requirements

Client `usg-ops-console` in realm `icam`. The dev export
`deploy/compose/keycloak/realm-dev.json` is a working reference; the icam
values marked below must be confirmed with the icam admins (POAM-028,
POAM-029).

- **Confidential client** (`publicClient: false`), standard flow only:
  implicit flow, direct access grants and service accounts off.
  Client authentication `client_secret_basic`.
- **PKCE**: `pkce.code.challenge.method` = `S256`.
- **Redirect URI**: exactly `<CONSOLE_PUBLIC_URL>/callback` (or the
  configured `CONSOLE_OIDC_REDIRECT_PATH`). No wildcards.
- **Post-logout redirect URI**: exactly `<CONSOLE_PUBLIC_URL>/logged-out`.
- **id_token role mapper**: a realm-roles mapper with claim
  `realm_access.roles`, multivalued, **Add to ID token on**
  (`id.token.claim=true`). Keycloak's default puts roles only in the access
  token; the gateway reads roles from the id_token, so without this every
  user is denied with `missing_role` (the usg-dcim gotcha).
- **Audience mapper**: include `usg-ops-console` in the id_token audience.
- **Realm roles**: `console-user` (required for a session), optionally
  `console-admin`. The gateway keeps `console-*` roles plus (M4) roles
  named by the tool registry (`required_roles`, `forward_roles` prefixes),
  for example `hello:user`; they must be in the id_token too.
- **LoA / acr mapping**: an `acr.loa.map` (realm and client) that maps the
  X.509/CAC authentication level to the value in `CONSOLE_REQUIRED_ACR`,
  and a browser flow with a conditional level-of-authentication step, so
  that Keycloak emits that `acr` both on a fresh CAC login and on SSO reuse
  (otherwise Keycloak emits `acr="0"` on SSO reuse and users get 403
  "CAC/PIV sign-in required"). The dev realm does this with flow
  `browser-loa-dev`, mapping password login at LoA 1 to
  `urn:usg-ops-console:dev:password` (dev only).
- **Client session idle / max = console values**:
  `client.session.idle.timeout` = `CONSOLE_SESSION_IDLE_SECS` (900) and
  `client.session.max.lifespan` = `CONSOLE_SESSION_ABSOLUTE_SECS` (43200).
  Not lower, or keepalive's refresh grant fails and sessions are revoked
  early; not higher, or SSO outlives the console session.
- **`revokeRefreshToken` = false** (or `refreshTokenMaxReuse` > 0). Two
  tabs may refresh with the same token; strict rotation would revoke the
  second tab's session.
- **Refresh tokens on** (`use.refresh.tokens=true`), and the refresh
  response must include an id_token (scope `openid`, the default). Every
  keepalive refreshes and re-reads roles from it.
- **Brute-force detection on** for the realm (AC-7 is inherited from
  Keycloak; POAM-018).
- **id_token lifespan** of at most 5 minutes is recommended: the id_token
  is sent as `id_token_hint` in the end-session URL at sign-out
  (POAM-021). Strip the query string from end-session access logs on the
  ingress and in Keycloak.

## TLS

The gateway listens on plain HTTP and does not terminate TLS. A
TLS-terminating ingress or proxy is **required**:

- `CONSOLE_PUBLIC_URL` must be `https` (the gateway refuses `http` except
  for loopback hosts), and all cookies are `__Host-` and `Secure`, so
  sign-in does not work over plain http.
- The ingress must set `X-Forwarded-For`, and its addresses must be in
  `CONSOLE_TRUSTED_PROXIES`.
- Keep the gateway port reachable only from the ingress.
- The ingress serves the console host and `*.tools.<domain>` and must
  pass `Host` unchanged (see [Tool origins](#tool-origins-dns-and-certificates)).
- Ingress-level rate limiting is recommended; the gateway rate-limits only
  `/login` (POAM-020).

## Local development with podman

DEV ONLY. All credentials in `deploy/compose/` are throwaway values, and
none of these files are in the image build context (`.containerignore`).

1. Install podman and `podman-compose` (or use Docker with
   `COMPOSE="docker compose"`). On macOS, create and start the VM once:

   ```bash
   podman machine init --memory 4096
   podman machine start
   ```

   The VM needs at least 4 GiB: with the 2 GiB default, Keycloak is
   OOM-killed during the E2E suite. For an existing VM run
   `podman machine stop && podman machine set --memory 4096 && podman machine start`.

2. Start the stack and sign in:

   ```bash
   make up      # postgres, keycloak, Caddy TLS proxy
   make run     # gateway on the host; browse https://localhost:9442
   ```

   Sign in as `console_user` / `dev-only-console_user` (Caddy uses an
   internal CA, so accept the certificate warning). `make run` loads
   `deploy/compose/gateway.env.example`, which sets
   `CONSOLE_MIGRATOR_DATABASE_URL` so the gateway migrates at startup.
   It serves its own shell build from `crates/console-shell/dist-run`
   (`make web-run`), so `make e2e` or `make check` rebuilding
   `dist-gallery` does not break a running stack. The gateway pins the CSP
   to the inline-script hash in `index.html` at startup, so restart
   `make run` to pick up new shell code.

3. Stop with `make down`, or `make down-clean` to also delete volumes.
   Recreate volumes (`make down-clean`) after pulling M3 round-1 changes:
   migration 0003 changes table ownership and fails on tables owned by the
   old `console` role.

4. M4: `make up` also generates DEV ONLY tool keys in
   `deploy/compose/secrets/` (P-256 assertion key and the sid HMAC key,
   mode 600, git-ignored) and renders the tool registries into
   `target/e2e-tools/`. `make run` starts the hello-tool server and
   serves it at `https://localhost:9449`. Run `make down-clean` once after
   pulling M4 (migration 0006 is new).

5. M5: after pulling M5, run `make down && make up` once. The Keycloak
   realm must be re-imported (new DEV ONLY users `dcim_viewer` and
   `dcim_operator`) and a running Caddy (`tls-proxy`) must be recreated
   to listen on the new DCIM tool ports 9454/9455. `make run` lists the
   DCIM tool for users with `dcim:viewer`, but does **not** start finch
   or otter-go: unless they run on 127.0.0.1:8094 (see
   [DCIM tool](#dcim-tool-usg-dcim-m5)), `/t/dcim/*` returns 502 and the
   console and hello are unaffected.

The example `examples/hello-tool/Containerfile` is a template for tool
images (build from the repo root). It is not part of the release image
and is **not built in CI** (unverified build).

### Clock drift in the podman VM (macOS)

DEV environment only. During M5 E2E the podman VM clock fell 37 minutes
(and later 9m41s) behind the host. Keycloak then issues id_tokens the
gateway sees as expired, and every console sign-in fails with
"id_token rejected: Expired" (this also breaks `make run`). Compare
`date -u` with `podman machine ssh date -u`; resync with
`podman machine ssh sudo chronyc -a makestep`, or
`podman machine stop && podman machine start`.

### Port clashes

Ports come from `deploy/compose/ports.env` (TLS 9442-9453, host gateways
8080-8089, hello-tool upstreams 8090/8092/8093, IdP blackhole proxy 8091,
Keycloak 8180, Postgres 5432). `make up` runs `make ports-free`
first and fails with the busy port numbers. Override any port from the
environment, for example:

```bash
CONSOLE_TLS_PORT_MAIN=9543 make e2e
```

The same variables feed compose, the Caddyfile, the Keycloak redirect URIs
and Playwright. If a port stays busy after `make down` with no container
owning it, podman's `gvproxy` may hold a stale forward from a failed start.
Clear it with:

```bash
podman machine stop && podman machine start
```

M5 adds DCIM ports: tool origins 9454 (main gateway) and 9455 (run
gateway), finch nginx upstream 8094, otter-go 8095, DCIM Postgres 5442.

On Linux or in CI, add the host-network override:
`COMPOSE="docker compose" COMPOSE_FILES="-f deploy/compose/compose.yaml -f deploy/compose/compose.hostnet.yaml"`.

## DCIM tool (usg-dcim, M5)

DCIM is the usg-dcim finch SPA (served by nginx) and its otter-go
backend. Its code is in the usg-dcim repository (branch
`feat/console-embed`, **uncommitted**); this section lists what the
console side and an operator must configure. Evidence is local only.

### Console side (`deploy/compose/`, DEV ONLY values)

| Key / file | Value | Notes |
| --- | --- | --- |
| `CONSOLE_TLS_PORT_TOOL_DCIM` | 9454 | DCIM tool origin on the main (E2E) gateway |
| `CONSOLE_TLS_PORT_TOOL_DCIM_RUN` | 9455 | DCIM tool origin on the `make run` gateway |
| `CONSOLE_PORT_DCIM_UPSTREAM` | 8094 | finch nginx (registry `upstream`, loopback http) |
| `CONSOLE_PORT_DCIM_API` | 8095 | otter-go, behind nginx; never browser-facing |
| `CONSOLE_PORT_DCIM_PG` | 5442 | DCIM's own Postgres (not the console database) |
| `tools/registry.main.toml`, `registry.run.toml` | `id = "dcim"` | `required_roles = ["dcim:viewer"]`, `forward_roles = ["dcim:"]`, `scopes = ["site"]`, `audit_writes = true`, `forward_headers = ["x-dcim-csrf"]`, nav Sites/Racks/IPAM. Test `registry::tests::dev_registries_parse_and_include_dcim` |
| `keycloak/realm-dev.json` | realm roles | `dcim:viewer`; `dcim:operator` (composite of viewer); `dcim:admin` (composite of operator and viewer). Users `dcim_viewer`, `dcim_operator` (password `dev-only-<name>`) |

The finch build must match the tool id: the gateway serves DCIM under
`/t/dcim/`, so build finch with `VITE_BASE=/t/dcim/ npm run build`
(`dist/index.html` must reference `/t/dcim/assets/`). A plain
`npm run build` is the standalone build.

### otter-go keys (console auth mode)

| Key | Default | Notes |
| --- | --- | --- |
| `OTTER_AUTH_MODE` | `standalone` | `standalone`, `console` or `both`; any other value stops startup. `console` refuses standalone login (`/auth/login`, `/logout`, `/refresh`, `/oidc/*` 404); `both` keeps it |
| `OTTER_CONSOLE_JWKS_URL` | required | Console JWKS, e.g. the gateway's internal `/.well-known/jwks.json`. https required unless the host is loopback |
| `OTTER_CONSOLE_ISSUER` | required | Exact console origin (`iss`) |
| `OTTER_CONSOLE_TOOL_ID` | `dcim` | Required `aud`; must equal the registry id |
| `OTTER_CONSOLE_REQUIRED_ACR` | required | Comma-separated exact allow-list; a missing `acr` is rejected. Production value: POAM-028 |
| `OTTER_CONSOLE_LEEWAY_SECS` | 30 | Clamped to 0..30 |
| `OTTER_CONSOLE_JWKS_REFRESH_SECS` | 300 | Max key age is the larger of 10 min and 2 x refresh |
| `OTTER_CONSOLE_UPSTREAM_ISSUER` | "" | The Keycloak issuer behind the console. Required, and must equal `DCIM_OIDC_ISSUER`, when that is set; otherwise startup fails (POAM-054) |
| `OTTER_CONSOLE_DEV_ACR_ALLOWED` | false | **DEV ONLY.** Must be true to allow an `acr` containing `:dev:` (the dev realm's `urn:usg-ops-console:dev:password`). Never set in production |
| `OTTER_CONSOLE_JWKS_ALLOW_HTTP` | false | **DEV ONLY.** Allows an http JWKS URL on a non-loopback host. Never set in production |
| `DCIM_JWT_SECRET` | — | Not used in `console` mode. Required in `both` mode (startup exits without it) |

`db/dev/console_role_mappings.sql` in usg-dcim is **DEV ONLY**: it
creates dev roles mapped to `dcim:viewer`/`dcim:operator`/`dcim:admin`.
Production maps roles through otter-go's OIDC role-mapping admin API; the
migration `00070_console_dcim_admin_mapping.sql` maps only `dcim:admin`.

### Production requirements (not enforced by code)

- Run a single otter-go replica until the `jti` replay cache has a shared
  store (POAM-052).
- Use https or mTLS from the gateway to finch nginx and from nginx to
  otter-go, and a NetworkPolicy that admits ingress to finch nginx only
  from the gateway pods (otter-go only from nginx, plus API-token
  clients if used). finch nginx forwards `X-Console-Assertion`, so
  direct reachability lets a captured assertion be replayed within its
  window (POAM-053).
- Every existing `users.sso_subject` row in DCIM must come from the same
  IdP and realm as the console (POAM-054).

### E2E (local only)

The DCIM specs (`e2e/tests/dcim*.spec.ts`) run only when `DCIM_REPO`
points at a usg-dcim checkout; without it they are ignored and the suite
is unchanged. CI does not run them.

```bash
cd e2e && DCIM_REPO=~/Dev/usg-dcim npx playwright test \
  tests/dcim.spec.ts tests/dcim-security.spec.ts tests/dcim-a11y.spec.ts \
  tests/dcim-ux.spec.ts --project=chromium --project=webkit
```

`e2e/servers/dcim-stack.mjs` starts a digest-pinned TimescaleDB container
on 5442 (tmpfs), runs the otter-go migrations and the DEV ONLY role
mappings, starts otter-go in `console` mode on 8095 against the main
gateway's JWKS (with `OTTER_CONSOLE_DEV_ACR_ALLOWED=true`), builds finch
with `VITE_BASE=/t/dcim/` (skip with `DCIM_SKIP_FINCH_BUILD=1` when
`dist/` is already a console build) and serves it with a digest-pinned
nginx on 8094. Set `CONSOLE_PORT_HELLO_UPSTREAM=8096` if a `make run`
hello server already holds 8090. Firefox: run through the Linux
Playwright container (`make e2e-linux` pattern). After an E2E run, finch's
`dist/` is the console build; run a plain `npm run build` before using
finch standalone.
