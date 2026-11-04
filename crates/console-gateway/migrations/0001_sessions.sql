-- M3 server-side sessions and pending OIDC authorization requests.
-- COMPLIANCE MAPPING:
-- - NIST 800-53: SC-23 (Session Authenticity) — only sha256(session id) is stored
-- - NIST 800-53: AC-12 (Session Termination) — absolute expiry per row

CREATE TABLE sessions (
    id_hash             bytea PRIMARY KEY,           -- sha256(cookie value); raw id never stored
    user_sub            text NOT NULL,
    username            text NOT NULL,
    display_name        text NOT NULL,
    roles               text[] NOT NULL,
    csrf_token          bytea NOT NULL,
    id_token            text NOT NULL,               -- id_token_hint for RP logout; never sent in /api/session, never logged
    created_at          timestamptz NOT NULL,
    last_seen_at        timestamptz NOT NULL,        -- moved only by keepalive (strict AC-11)
    absolute_expires_at timestamptz NOT NULL,
    client_ip           inet,
    user_agent          text                         -- truncated to 256 bytes
);
CREATE INDEX sessions_user_sub_idx ON sessions (user_sub);
CREATE INDEX sessions_absolute_expires_idx ON sessions (absolute_expires_at);
CREATE INDEX sessions_last_seen_idx ON sessions (last_seen_at);

CREATE TABLE auth_requests (
    id_hash       bytea PRIMARY KEY,                 -- sha256(__Host-console_auth value)
    state         text NOT NULL,
    nonce         text NOT NULL,
    pkce_verifier text NOT NULL,
    return_to     text NOT NULL,
    expires_at    timestamptz NOT NULL
);
CREATE INDEX auth_requests_expires_idx ON auth_requests (expires_at);
