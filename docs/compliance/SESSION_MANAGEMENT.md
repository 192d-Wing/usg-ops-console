# Session management

**Status: Planned (M3).** The console has no sign-in, sessions or cookies
yet.

The gateway serves static files and `/healthz` only. It sets no cookies.

The shell stores one non-sensitive value in browser `localStorage`:
`cui-theme` (the theme preference). It is validated on read (SI-10, see
`NIST_800-53_MAPPING.md`). No tokens or credentials are stored in the
browser.

## Planned (M3)

- Cookie flags
- Idle timeout (AC-11) and absolute timeout
- Logout and session termination (AC-12)
