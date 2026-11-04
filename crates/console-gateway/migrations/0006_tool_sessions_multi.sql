-- M4 round 1 (INT-005): up to MAX_TOOL_SESSIONS (8) tool sessions per
-- (console session, tool), so two tabs of one tool no longer revoke each
-- other. The gateway evicts the oldest row beyond the cap on each launch.
-- Rows still cascade with the console session.
-- COMPLIANCE MAPPING:
-- - NIST 800-53: AC-10 (Concurrent Session Control) — bounded tool sessions per console session and tool
-- - NIST 800-53: AC-12 (Session Termination) — unchanged cascade with the console session

ALTER TABLE tool_sessions DROP CONSTRAINT tool_sessions_session_hash_tool_id_key;
CREATE INDEX tool_sessions_session_tool_idx ON tool_sessions (session_hash, tool_id, created_at);
