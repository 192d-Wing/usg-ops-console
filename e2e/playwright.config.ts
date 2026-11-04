import { readFileSync } from "node:fs";
import { defineConfig, devices } from "@playwright/test";

// M3: every shell route requires a session. Browsers sign in through the
// compose Keycloak (`make up`) and reach the host-run gateways only via the
// compose Caddy TLS proxy: WebKit drops `__Host-`/Secure cookies on
// http://localhost (verified 2026-11-03), and the cookie is never weakened.
//
// Ports come from deploy/compose/ports.env (single source shared with the
// Makefile and compose; process env overrides). Defaults:
//   gateway (host)      browser origin            purpose
//   127.0.0.1:8081  ->  https://localhost:9443    dist-gallery, main suite
//   127.0.0.1:8082  ->  https://localhost:9444    release dist smoke
//   127.0.0.1:8083  ->  https://localhost:9445    idle specs (IDLE_SECS=150)
//   127.0.0.1:8084  ->  https://localhost:9446    acr-mismatch gateway
//
// Gateway env comes from deploy/compose/gateway.env.example (single source).
const ENV_FILE = "../deploy/compose/gateway.env.example";
const PORTS_FILE = "../deploy/compose/ports.env";
function loadEnv(path: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const m = /^\s*([A-Z0-9_]+)=(.*)$/.exec(line);
    if (m && m[1] && m[2] !== undefined) out[m[1]] = m[2];
  }
  return out;
}
const BASE_ENV = loadEnv(ENV_FILE);
const PORTS_FILE_ENV = loadEnv(PORTS_FILE);
function port(name: string): number {
  const v = process.env[name] ?? PORTS_FILE_ENV[name];
  const n = Number(v);
  if (!v || !Number.isInteger(n)) throw new Error(`${name} missing/invalid (see ${PORTS_FILE})`);
  return n;
}
const tls = (k: string) => `https://localhost:${port(`CONSOLE_TLS_PORT_${k}`)}`;
const gw = (k: string) => port(`CONSOLE_GW_PORT_${k}`);
/** acr the dev realm never issues: the ACR gateway must deny every login. */
export const UNMET_ACR = "urn:usg-ops-console:e2e:unmet-piv";

const GW_HOST = process.env.E2E_GATEWAY_HOST ?? "127.0.0.1";
export const ORIGINS = {
  main: process.env.E2E_BASE_URL ?? tls("MAIN"),
  dist: process.env.E2E_DIST_URL ?? tls("DIST"),
  idle: process.env.E2E_IDLE_URL ?? tls("IDLE"),
  acr: process.env.E2E_ACR_URL ?? tls("ACR"),
  /** Direct http to the rate-limit gateway (no browser cookies involved). */
  rl: `http://${process.env.E2E_GATEWAY_HOST ?? "127.0.0.1"}:${port("CONSOLE_GW_PORT_RL")}`,
  /** Direct http to a rate-limit gateway that trusts NO proxy (spoofed XFF must be ignored). */
  rlUntrusted: `http://${process.env.E2E_GATEWAY_HOST ?? "127.0.0.1"}:${port("CONSOLE_GW_PORT_RL_UNTRUSTED")}`,
} as const;
export const IDLE_SECS = 150;
const rnd = () => 1 + Math.floor(Math.random() * 253);
export const E2E_CLIENT_IP = `198.18.${rnd()}.${rnd()}`;
const SMOKE = /dist-smoke\.spec\.ts$/;
const IDLE = /idle[^/]*\.spec\.ts$/;
const SETUP = /auth\.setup\.ts$/;
export const STORAGE_STATE = ".auth/console_user.json";
const isCI = Boolean(process.env.CI);

function gateway(port: number, publicUrl: string, dist: string, extra: Record<string, string> = {}) {
  return {
    command: "../target/release/console-gateway",
    // /healthz is public; probe the gateway directly (not through Caddy).
    url: `http://${GW_HOST}:${port}/healthz`,
    env: {
      ...BASE_ENV,
      CONSOLE_LISTEN: `${GW_HOST}:${port}`,
      CONSOLE_DIST: dist,
      CONSOLE_PUBLIC_URL: publicUrl,
      // Main suite: max idle so the shared storageState session survives a
      // full multi-browser run. Idle behaviour is covered by idle.spec.ts.
      CONSOLE_SESSION_IDLE_SECS: "3600",
      // Caddy's peer is 127.0.0.1 (Linux hostnet) or the podman gvproxy range
      // (10.89.x on macOS); trusting both lets the per-test X-Forwarded-For
      // (E2E_CLIENT_IP below) reach the per-IP limiter. DEV/E2E only.
      CONSOLE_TRUSTED_PROXIES: "127.0.0.1/32,10.0.0.0/8",
      ...extra,
    },
    reuseExistingServer: !isCI,
    timeout: 60_000,
    stdout: "pipe" as const,
    stderr: "pipe" as const,
  };
}

const authed = { storageState: STORAGE_STATE };
export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: isCI,
  retries: isCI ? 1 : 0,
  ...(isCI ? { workers: 2 } : {}),
  reporter: isCI
    ? [["list"], ["html", { open: "never" }], ["json", { outputFile: "test-results/results.json" }]]
    : [["list"], ["html", { open: "never" }]],
  use: {
    baseURL: ORIGINS.main,
    // Per-worker source IP (TEST-NET 198.18/15); loginVia re-randomises per login.
    extraHTTPHeaders: { "X-Forwarded-For": E2E_CLIENT_IP },
    // Caddy `tls internal` self-signed CA (dev only).
    ignoreHTTPSErrors: true,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "setup", testMatch: SETUP, use: { ...devices["Desktop Chrome"] } },
    ...(["chromium", "firefox", "webkit"] as const).flatMap((b) => {
      const dev = devices[b === "chromium" ? "Desktop Chrome" : b === "firefox" ? "Desktop Firefox" : "Desktop Safari"];
      return [
        { name: b, testIgnore: [SMOKE, IDLE, SETUP], dependencies: ["setup"], use: { ...dev, ...authed } },
        { name: `idle-${b}`, testMatch: IDLE, use: { ...dev, baseURL: ORIGINS.idle } },
        { name: `dist-${b}`, testMatch: SMOKE, dependencies: ["setup"], use: { ...dev, ...authed, baseURL: ORIGINS.dist } },
      ];
    }),
  ],
  webServer: [
    gateway(gw("MAIN"), ORIGINS.main, "../crates/console-shell/dist-gallery"),
    gateway(gw("DIST"), ORIGINS.dist, "../crates/console-shell/dist"),
    gateway(gw("IDLE"), ORIGINS.idle, "../crates/console-shell/dist-gallery", { CONSOLE_SESSION_IDLE_SECS: String(IDLE_SECS) }),
    gateway(gw("ACR"), ORIGINS.acr, "../crates/console-shell/dist-gallery", { CONSOLE_REQUIRED_ACR: UNMET_ACR }),
    // SEC NIT-1: limiter gateways use the production trust list (gateway.env.example:
    // 127.0.0.1/32), not the dev/E2E 10/8 widening. RL_UNTRUSTED trusts no proxy, so
    // its loopback peer is untrusted and any X-Forwarded-For must be ignored.
    gateway(gw("RL"), ORIGINS.main, "../crates/console-shell/dist-gallery", { CONSOLE_TRUSTED_PROXIES: "127.0.0.1/32" }),
    gateway(gw("RL_UNTRUSTED"), ORIGINS.main, "../crates/console-shell/dist-gallery", { CONSOLE_TRUSTED_PROXIES: "" }),
  ],
});
