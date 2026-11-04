// DEV/E2E ONLY forward HTTP proxy for the KA503 gateway (brief Q6, plan §8).
//
// The KA503 gateway runs with HTTP_PROXY pointing here (reqwest honours it for
// its plain-http Keycloak calls). Discovery, JWKS and the login code exchange
// pass through unchanged, so sign-in works normally. Any token-endpoint POST
// with grant_type=refresh_token has its connection destroyed: to the gateway
// the IdP token endpoint is unreachable, so POST /api/session/keepalive must
// answer 503 `unavailable` (retryable; session neither extended nor ended).
//
// Usage: node servers/idp-refresh-blackhole.mjs <listen-port>
import http from "node:http";

const port = Number(process.argv[2] ?? process.env.CONSOLE_PORT_KA503_PROXY);
if (!Number.isInteger(port)) throw new Error("listen port required");
let blackholed = 0;

const server = http.createServer((req, res) => {
  if (req.url === "/healthz") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true, blackholed }));
    return;
  }
  let target;
  try {
    target = new URL(req.url ?? "");
  } catch {
    res.writeHead(400).end();
    return;
  }
  // Only ever forward to the loopback Keycloak (no open proxy).
  if (target.protocol !== "http:" || target.hostname !== "127.0.0.1") {
    res.writeHead(403).end();
    return;
  }
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const body = Buffer.concat(chunks);
    if (req.method === "POST" && target.pathname.endsWith("/protocol/openid-connect/token") && /(^|&)grant_type=refresh_token(&|$)/.test(body.toString())) {
      blackholed++;
      req.socket.destroy();
      return;
    }
    const headers = { ...req.headers };
    delete headers["proxy-connection"];
    const up = http.request(target, { method: req.method, headers }, (ur) => {
      res.writeHead(ur.statusCode ?? 502, ur.headers);
      ur.pipe(res);
    });
    up.on("error", () => res.writeHead(502).end());
    up.end(body);
  });
});
server.listen(port, "127.0.0.1", () => console.log(`idp-refresh-blackhole on 127.0.0.1:${port}`));
