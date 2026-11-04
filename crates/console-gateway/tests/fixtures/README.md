Test fixtures for console-gateway.

- `session_v1.json`: the `/api/session` v1 contract, deserialized by the
  gateway, console-protocol, and the shell mirror so the shape cannot drift.
- `test-only-rsa*.pem`: throwaway RSA keys used only to sign id_tokens for a
  stub IdP in tests. They are NOT secrets and are never trusted by any real
  deployment (the gateway only trusts the JWKS of the configured issuer).
- `test-only-p256*.pem`: throwaway P-256 keys used only to sign/verify tool
  identity assertions in tests (`*.pub.pem` exercises the published-keys
  rotation file). NOT secrets; never configured in any deployment.
