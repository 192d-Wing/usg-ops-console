-- M4 tool origins: one-time launch codes and tool-scoped sessions, both bound
-- to the console session (ON DELETE CASCADE: logout, expiry, revocation or a
-- sweep of the console session ends every tool session at once).
-- COMPLIANCE MAPPING:
-- - NIST 800-53: SC-23 (Session Authenticity) — only sha256 of codes / tool session ids stored; single-use codes
-- - NIST 800-53: AC-12 (Session Termination) — tool sessions die with the console session
-- - NIST 800-53: AU-3 (Content of Audit Records) — tool id, path, status and upstream latency on tool events
-- - NIST 800-53: AC-6 (Least Privilege) — runtime role gets DML on the new tables only

CREATE TABLE tool_launch_codes (
    code_hash    bytea PRIMARY KEY,                  -- sha256(code); the code itself is never stored
    session_hash bytea NOT NULL REFERENCES sessions (id_hash) ON DELETE CASCADE,
    tool_id      text NOT NULL,
    path         text NOT NULL,                      -- validated tool-relative path (may carry a query)
    expires_at   timestamptz NOT NULL
);
CREATE INDEX tool_launch_codes_expires_idx ON tool_launch_codes (expires_at);
CREATE INDEX tool_launch_codes_session_idx ON tool_launch_codes (session_hash);

CREATE TABLE tool_sessions (
    id_hash      bytea PRIMARY KEY,                  -- sha256(__Host-tool_sid value)
    session_hash bytea NOT NULL REFERENCES sessions (id_hash) ON DELETE CASCADE,
    tool_id      text NOT NULL,
    created_at   timestamptz NOT NULL,
    UNIQUE (session_hash, tool_id)
);

ALTER TABLE tool_launch_codes OWNER TO console_owner;
ALTER TABLE tool_sessions OWNER TO console_owner;
REVOKE ALL ON tool_launch_codes, tool_sessions FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON tool_launch_codes, tool_sessions TO console_app;

-- Tool context on audit rows (nullable; M3 events leave them NULL).
ALTER TABLE audit_events ADD COLUMN tool_id text;
ALTER TABLE audit_events ADD COLUMN path text;
ALTER TABLE audit_events ADD COLUMN http_status integer;
ALTER TABLE audit_events ADD COLUMN latency_ms bigint;
