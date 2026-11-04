Test fixtures for console-gateway.

- `session_v1.json`: the `/api/session` v1 contract, deserialized by the
  gateway, console-protocol, and the shell mirror so the shape cannot drift.
- `test-only-rsa*.pem`: throwaway RSA keys used only to sign id_tokens for a
  stub IdP in tests. They are NOT secrets and are never trusted by any real
  deployment (the gateway only trusts the JWKS of the configured issuer).
