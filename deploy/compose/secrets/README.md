# secrets/ (DEV ONLY)

Nothing secret is committed here. The local stack uses throwaway values
labelled `dev-only-*` in `../gateway.env.example`, `../compose.yaml` and
`../keycloak/realm-dev.json`:

| Value | Where | Used by |
| --- | --- | --- |
| `dev-only-console-secret` | realm client `usg-ops-console` | `CONSOLE_OIDC_CLIENT_SECRET` |
| `dev-only-console-db` | postgres role `console` (app/owner) | `CONSOLE_DATABASE_URL` |
| `dev-only-audit-db` | postgres role `console_audit` (INSERT+SELECT on `audit_events`) | `CONSOLE_AUDIT_DATABASE_URL` |
| `dev-only-postgres` | postgres superuser | init only |
| `dev-only-admin` | Keycloak bootstrap admin | admin console |
| `dev-only-<username>` | test users `console_user`, `console_admin`, `no_role`, `nohello_user` (console-user without `hello:user`) | E2E login |
| `assertion.key` (generated) | ES256 P-256 PKCS#8 PEM, created by `make up` (`make dev-secrets`) with mode 600 on first run; git-ignored | `CONSOLE_ASSERTION_KEY_FILE` (M4 tool assertions) |
| `tool-session-hmac.key` (generated) | 64 base64 chars, decoded by the gateway to 48 random bytes (raw binary files of >= 32 bytes are also accepted), created by `make up` (`make dev-secrets`) with mode 600 on first run; git-ignored | `CONSOLE_TOOL_SESSION_HMAC_KEY_FILE` (per-tool session pseudonym; required with tools outside dev mode) |

To exercise the `*_FILE` path locally, put files here (they are git- and
container-ignored) and point `CONSOLE_OIDC_CLIENT_SECRET_FILE`,
`CONSOLE_DATABASE_URL_FILE`, `CONSOLE_AUDIT_DATABASE_URL_FILE` at them.
The assertion key and the tool session HMAC key are generated per checkout and
never committed; delete either to rotate (the next `make up` makes a new one;
restart the gateways). Rotating the HMAC key changes every tool's `sid`
pseudonym for existing sessions.
Real icam credentials and production signing keys never go in this repository.
