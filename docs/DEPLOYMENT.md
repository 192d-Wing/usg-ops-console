# Deployment

Status as of M3, 2026-11-03. This covers what a deployment of
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
- A TLS-terminating ingress or reverse proxy in front of the gateway.

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
| `CONSOLE_TRUSTED_PROXIES` | CIDRs of the ingress/proxy hops, so audit records get the real client IP. Without it the peer address is recorded |

Optional, with defaults: `CONSOLE_LISTEN` (`0.0.0.0:8080`), `CONSOLE_DIST`,
`CONSOLE_OIDC_REDIRECT_PATH` (`/callback`), `CONSOLE_OIDC_DISCOVERY_RETRIES`
(10), `CONSOLE_DB_MAX_CONNECTIONS` (10), `CONSOLE_AUDIT_DB_MAX_CONNECTIONS`
(4), `CONSOLE_SESSION_IDLE_SECS` (900), `CONSOLE_SESSION_ABSOLUTE_SECS`
(43200), `CONSOLE_SWEEP_INTERVAL_SECS` (60), `CONSOLE_REQUIRED_ROLE`
(`console-user`), `RUST_LOG` (`info`).

Must **not** be set in production:

- `CONSOLE_DEV_MODE`: it allows an unset `CONSOLE_REQUIRED_ACR` and DB
  connections without verified TLS.
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
  `console-admin`. The gateway keeps only `console-*` roles.
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

3. Stop with `make down`, or `make down-clean` to also delete volumes.
   Recreate volumes (`make down-clean`) after pulling M3 round-1 changes:
   migration 0003 changes table ownership and fails on tables owned by the
   old `console` role.

### Port clashes

Ports come from `deploy/compose/ports.env` (TLS 9442-9446, host gateways
8080-8085, Keycloak 8180, Postgres 5432). `make up` runs `make ports-free`
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

On Linux or in CI, add the host-network override:
`COMPOSE="docker compose" COMPOSE_FILES="-f deploy/compose/compose.yaml -f deploy/compose/compose.hostnet.yaml"`.
