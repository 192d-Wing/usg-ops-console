-- M4 POAM-031: the idle limit is stored on each session row, so gateways that
-- share one database with different CONSOLE_SESSION_IDLE_SECS never expire
-- (or sweep) each other's sessions early. Legacy rows (NULL) fall back to the
-- reading gateway's configured idle limit.
-- Also stores the id_token `acr` so the tool assertion can carry it (IA-2(12)).
-- COMPLIANCE MAPPING:
-- - NIST 800-53: AC-11 (Device Lock), AC-12 (Session Termination) — per-session idle limit and deadline

ALTER TABLE sessions ADD COLUMN idle_timeout_secs integer CHECK (idle_timeout_secs > 0);
ALTER TABLE sessions ADD COLUMN idle_expires_at timestamptz;
ALTER TABLE sessions ADD COLUMN acr text;
CREATE INDEX sessions_idle_expires_idx ON sessions (idle_expires_at);
