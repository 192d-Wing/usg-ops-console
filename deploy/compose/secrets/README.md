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
| `dev-only-<username>` | test users `console_user`, `console_admin`, `no_role` | E2E login |

To exercise the `*_FILE` path locally, put files here (they are git- and
container-ignored) and point `CONSOLE_OIDC_CLIENT_SECRET_FILE`,
`CONSOLE_DATABASE_URL_FILE`, `CONSOLE_AUDIT_DATABASE_URL_FILE` at them.
Real icam credentials never go in this repository.
