import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";

/**
 * DEV ONLY test helpers that read/poke the compose Postgres directly, via the
 * container engine (no extra npm dependency, no DB port/creds in test code).
 * The container is found by compose labels, so this works for podman-compose
 * (local) and docker compose (CI). Override the engine with
 * E2E_CONTAINER_ENGINE=docker.
 *
 * Used only to (a) read audit_events for "was it audited" checks and (b) move
 * a session's server-side clock (last_seen_at) back so idle expiry can be
 * exercised without a real 15 min wait. Never touches audit rows.
 */
const ENGINE = process.env.E2E_CONTAINER_ENGINE ?? "podman";
const PROJECT = process.env.E2E_COMPOSE_PROJECT ?? "usg-ops-console";
let cached: string | undefined;

function container(): string {
  if (cached) return cached;
  const out = execFileSync(
    ENGINE,
    [
      "ps",
      "-q",
      "--filter",
      `label=com.docker.compose.project=${PROJECT}`,
      "--filter",
      "label=com.docker.compose.service=postgres",
    ],
    { encoding: "utf8" },
  ).trim();
  const id = out.split("\n")[0];
  if (!id) throw new Error(`no running compose postgres (${ENGINE}, project ${PROJECT}); run make up`);
  cached = id;
  return id;
}

/** Run one SQL statement as the postgres superuser in the console DB; rows as `a|b` lines. */
export function sql(query: string): string[] {
  const out = execFileSync(
    ENGINE,
    ["exec", "-i", container(), "psql", "-U", "postgres", "-d", "console", "-v", "ON_ERROR_STOP=1", "-At", "-c", query],
    { encoding: "utf8" },
  );
  return out.split("\n").filter((l) => l.length > 0);
}

/** bytea literal for sha256(raw session id) given the base64url cookie value. */
export function sidHashLiteral(cookieValue: string): string {
  const raw = Buffer.from(cookieValue, "base64url");
  return `'\\x${createHash("sha256").update(raw).digest("hex")}'::bytea`;
}

/** Correlation ref stored in audit_events.session_ref (first 8 hex of id_hash). */
export function sessionRef(cookieValue: string): string {
  return createHash("sha256").update(Buffer.from(cookieValue, "base64url")).digest("hex").slice(0, 8);
}

/** last_seen_at of the session (ISO), or undefined when the row is gone. */
export function lastSeen(cookieValue: string): string | undefined {
  return sql(`SELECT to_char(last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US') FROM sessions WHERE id_hash = ${sidHashLiteral(cookieValue)}`)[0];
}

/** Push the session's idle clock into the past so the next request finds it idle-expired. */
export function forceIdleExpiry(cookieValue: string): void {
  const n = sql(
    `WITH u AS (UPDATE sessions SET last_seen_at = now() - interval '1 day' WHERE id_hash = ${sidHashLiteral(cookieValue)} RETURNING 1) SELECT count(*) FROM u`,
  );
  if (n[0] !== "1") throw new Error("session row not found to expire");
}

export interface AuditRow {
  event_type: string;
  outcome: string;
  username: string;
  reason: string;
  session_ref: string;
}

/** Audit rows since `sinceIso` (UTC ISO string from Date#toISOString), oldest first. */
export function auditSince(sinceIso: string, where = "true"): AuditRow[] {
  return sql(
    `SELECT event_type, outcome, coalesce(username,''), coalesce(reason,''), coalesce(session_ref,'') FROM audit_events WHERE occurred_at >= '${sinceIso}'::timestamptz AND (${where}) ORDER BY id`,
  ).map((l) => {
    const [event_type = "", outcome = "", username = "", reason = "", session_ref = ""] = l.split("|");
    return { event_type, outcome, username, reason, session_ref };
  });
}

/** DB clock now (ISO) so "since" filters do not depend on host/VM clock skew. */
export function dbNow(): string {
  return sql(`SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')`)[0] ?? new Date().toISOString();
}
