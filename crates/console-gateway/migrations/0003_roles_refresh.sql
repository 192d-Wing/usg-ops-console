-- M3 review round 1: least-privilege ownership (SEC-006) and the
-- server-side refresh token (NIST-001). 0001/0002 are left byte-identical
-- (sqlx checksums); the AU-9 annotation in 0002 is superseded by this file.
-- COMPLIANCE MAPPING:
-- - NIST 800-53: AU-9 (Protection of Audit Information) — audit_events owned by NOLOGIN console_owner;
--   the runtime role (console_app) has no rights on it; console_audit may only INSERT/SELECT
-- - NIST 800-53: AC-6 (Least Privilege) — runtime role limited to DML on sessions/auth_requests
-- POAM: AU-9 - console_owner (assumable by the migrator login) and superusers can still disable the
--   append-only triggers or drop audit_events; an external, write-once audit store (AU-4(1)/AU-9(2)) is M7.
-- POAM: SC-28 - sessions.id_token and sessions.refresh_token are stored in plaintext at rest
--   (accepted for M3, brief Q2/NIST-001); app-level or pgcrypto encryption is M7.

-- Roles are cluster objects provisioned outside migrations (compose init,
-- DBA, CNPG managed roles). Fail loudly rather than skip a grant.
DO $$
DECLARE r text;
BEGIN
    FOREACH r IN ARRAY ARRAY['console_owner', 'console_app', 'console_audit'] LOOP
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) THEN
            RAISE EXCEPTION 'role % must exist before migrations (see impl notes: DB roles)', r;
        END IF;
    END LOOP;
END
$$;

-- No-ops when the migrator already runs as console_owner (SET ROLE); moves
-- ownership when an earlier run created the objects as another role.
ALTER TABLE sessions OWNER TO console_owner;
ALTER TABLE auth_requests OWNER TO console_owner;
ALTER TABLE audit_events OWNER TO console_owner;
ALTER FUNCTION audit_events_no_mutation() OWNER TO console_owner;

REVOKE ALL ON sessions, auth_requests, audit_events FROM PUBLIC;
REVOKE ALL ON audit_events FROM console_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON sessions, auth_requests TO console_app;
GRANT INSERT, SELECT ON audit_events TO console_audit;

-- Refresh token for the keepalive refresh grant (never sent to the browser,
-- never logged). Nullable: a session without one cannot be extended.
ALTER TABLE sessions ADD COLUMN refresh_token text;

-- SEC-001: a login started after expiry/sign-out must prove fresh
-- authentication (auth_time >= issued_at).
ALTER TABLE auth_requests ADD COLUMN reauth boolean NOT NULL DEFAULT false;
ALTER TABLE auth_requests ADD COLUMN issued_at timestamptz;
