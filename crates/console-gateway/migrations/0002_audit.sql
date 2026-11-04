-- M3 append-only audit log.
-- COMPLIANCE MAPPING:
-- - NIST 800-53: AU-2/AU-3/AU-12 (Audit Events, Content, Generation) — who/what/when/where/outcome
-- - NIST 800-53: AU-9 (Protection of Audit Information) — append-only trigger + restricted writer role
-- - NIST 800-53: AU-8 (Time Stamps) — timestamptz, written in UTC by the gateway

-- The gateway's audit pool connects as (a member of) role `console_audit`,
-- which must be provisioned before migrations run (compose / DBA / CNPG
-- managed roles). Fail loudly instead of silently skipping the grant.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_audit') THEN
        RAISE EXCEPTION 'role console_audit must exist before migrations (audit writer role, INSERT+SELECT on audit_events only)';
    END IF;
END
$$;

CREATE TABLE audit_events (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at timestamptz NOT NULL,
    event_type  text NOT NULL,      -- login.success|login.failure|login.denied|logout|
                                    -- session.expired.idle|session.expired.absolute|session.refresh
    outcome     text NOT NULL CHECK (outcome IN ('success', 'failure', 'denied')),
    user_sub    text,               -- null when unknown (failure before token)
    username    text,
    session_ref text,               -- first 8 hex of id_hash (correlation, not the secret)
    client_ip   inet,
    user_agent  text,
    reason      text,               -- machine reason code; never token text
    request_id  text
);
CREATE INDEX audit_events_occurred_idx ON audit_events (occurred_at);
CREATE INDEX audit_events_user_sub_idx ON audit_events (user_sub);

CREATE FUNCTION audit_events_no_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit_events is append-only (% rejected)', TG_OP;
END
$$;

CREATE TRIGGER audit_events_no_update_delete
    BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_no_mutation();
CREATE TRIGGER audit_events_no_truncate
    BEFORE TRUNCATE ON audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION audit_events_no_mutation();

REVOKE ALL ON audit_events FROM PUBLIC;
GRANT INSERT, SELECT ON audit_events TO console_audit;
