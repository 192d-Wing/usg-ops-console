//! Postgres integration: the full login -> session -> keepalive -> logout
//! flow against a stub IdP, plus fixation, expiry, sweeper, CSRF, audit,
//! re-authentication, assurance, refresh revocation and the role model.
//!
//! Gated on `CONSOLE_TEST_DATABASE_URL` (a superuser URL that may
//! CREATE DATABASE / CREATE ROLE). Unset: each test prints `NOT RUN` and
//! returns. Each test gets its own fresh database, provisioned like the
//! compose contract (schema owned by `console_owner`, migrations applied by
//! `console_gateway::migrate` under `SET ROLE console_owner`, runtime pool as
//! a `console_app` member, audit pool as a `console_audit` member).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use base64::Engine;
use common::*;
use console_gateway::audit::AuditSink;
use console_gateway::config::Secret;
use console_gateway::oidc::OidcClient;
use console_gateway::rate_limit::RateLimiter;
use console_gateway::routes::AppState;
use console_gateway::session::{Clock, SessionStore};
use console_gateway::{app, sweeper};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, Connection, Executor, PgConnection, PgPool, Row};
use tower::ServiceExt;
use tracing::subscriber::set_default;

const AUDIT_TEST_ROLE: &str = "console_audit_test";
const AUDIT_TEST_PW: &str = "test-only-audit-pw";
const APP_TEST_ROLE: &str = "console_app_test";
const APP_TEST_PW: &str = "test-only-app-pw";

struct Env {
    router: Router,
    state: Arc<AppState>,
    clock: Arc<ManualClock>,
    stub: StubIdp,
    /// Superuser connection to the test DB (inspection only).
    owner: PgPool,
    /// The runtime role (`console_app` member), as the gateway uses it.
    app: PgPool,
    audit: PgPool,
}

async fn setup(extra: &[(&str, &str)]) -> Option<Env> {
    setup_with(extra, 1000).await
}

async fn setup_with(extra: &[(&str, &str)], login_per_minute: u32) -> Option<Env> {
    let Ok(admin) = std::env::var("CONSOLE_TEST_DATABASE_URL") else {
        println!("NOT RUN: CONSOLE_TEST_DATABASE_URL unset");
        return None;
    };
    let admin_opts: PgConnectOptions = admin.parse().expect("CONSOLE_TEST_DATABASE_URL");
    let mut conn = PgConnection::connect_with(&admin_opts)
        .await
        .expect("admin connect");
    let suffix: String = console_gateway::session::random_bytes::<6>()
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let db = format!("console_test_{suffix}");
    conn.execute(format!("CREATE DATABASE {db}").as_str())
        .await
        .unwrap();
    // Roles are cluster-wide; tolerate concurrent creation by parallel tests.
    let create = |role: &str, attrs: &str| {
        format!(
            "DO $$ BEGIN CREATE ROLE {role} {attrs}; \
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
        )
    };
    for stmt in [
        create("console_owner", "NOLOGIN"),
        create("console_app", "NOLOGIN"),
        create("console_audit", "NOLOGIN"),
        create(
            AUDIT_TEST_ROLE,
            &format!("LOGIN PASSWORD '{AUDIT_TEST_PW}' NOCREATEDB NOCREATEROLE"),
        ),
        create(
            APP_TEST_ROLE,
            &format!("LOGIN PASSWORD '{APP_TEST_PW}' NOCREATEDB NOCREATEROLE"),
        ),
        format!("GRANT console_audit TO {AUDIT_TEST_ROLE}"),
        format!("GRANT console_app TO {APP_TEST_ROLE}"),
    ] {
        let _ = conn.execute(stmt.as_str()).await;
    }
    conn.close().await.ok();

    let owner = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(admin_opts.clone().database(&db).disable_statement_logging())
        .await
        .unwrap();
    // The compose/CNPG schema contract (see fixes-round-1-gateway.md).
    for stmt in [
        "ALTER SCHEMA public OWNER TO console_owner",
        "REVOKE ALL ON SCHEMA public FROM PUBLIC",
        "GRANT USAGE ON SCHEMA public TO console_app, console_audit",
    ] {
        owner.execute(stmt).await.expect(stmt);
    }
    let mut migrator_url = url::Url::parse(&admin).expect("admin URL");
    migrator_url.set_path(&db);
    console_gateway::migrate(&Secret::new(migrator_url.into()))
        .await
        .expect("migrations as console_owner");
    let app_pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(
            admin_opts
                .clone()
                .database(&db)
                .username(APP_TEST_ROLE)
                .password(APP_TEST_PW)
                .disable_statement_logging(),
        )
        .await
        .expect("app role connect");
    let audit = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            admin_opts
                .clone()
                .database(&db)
                .username(AUDIT_TEST_ROLE)
                .password(AUDIT_TEST_PW)
                .disable_statement_logging(),
        )
        .await
        .expect("audit role connect");

    let stub = start_stub_idp().await;
    let cfg = config(&stub.issuer, extra);
    let clock = ManualClock::at(t0());
    *stub.clock.lock().unwrap() = Some(clock.clone());
    let oidc = OidcClient::discover(&cfg, clock.clone()).await.unwrap();
    let store = SessionStore::new(app_pool.clone());
    assert!(store.schema_ready().await, "runtime role sees the schema");
    let state = Arc::new(AppState {
        cfg: Arc::new(cfg),
        store,
        audit: AuditSink::new(audit.clone()),
        oidc: Arc::new(oidc),
        clock: clock.clone(),
        login_limiter: RateLimiter::per_minute(login_per_minute),
    });
    Some(Env {
        router: app(state.clone(), ""),
        state,
        clock,
        stub,
        owner,
        app: app_pool,
        audit,
    })
}

fn cookie_attrs(res: &Response<Body>, name: &str) -> Option<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

async fn body_json(res: Response<Body>) -> serde_json::Value {
    let body = axum::body::to_bytes(res.into_body(), 16 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap_or_default()
}

async fn body_text(res: Response<Body>) -> String {
    let body = axum::body::to_bytes(res.into_body(), 16 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&body).into_owned()
}

/// In-memory log sink for the "no secrets in logs" checks (SEC-008).
#[derive(Clone, Default)]
struct LogBuf(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Knobs for [`Env::login_with`].
#[derive(Default)]
struct LoginOpts<'a> {
    return_to: &'a str,
    reason: Option<&'a str>,
    roles: Option<&'a [&'a str]>,
    prev_sid: Option<&'a str>,
    /// Send `__Host-console_reauth=1` with `/login`.
    reauth_cookie: bool,
    acr: Option<Option<&'a str>>,
    /// `auth_time` offset from now (seconds, negative = in the past).
    auth_time_offset: Option<i64>,
}

struct LoginResult {
    res: Response<Body>,
    sid: Option<String>,
    authorize: HashMap<String, String>,
}

fn set_cookies(res: &Response<Body>) -> HashMap<String, String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| {
            let (nv, _) = c.split_once(';').unwrap_or((c, ""));
            let (n, v) = nv.split_once('=')?;
            Some((n.to_owned(), v.to_owned()))
        })
        .collect()
}

fn location(res: &Response<Body>) -> String {
    res.headers()[header::LOCATION].to_str().unwrap().to_owned()
}

impl Env {
    async fn call(&self, req: Request<Body>) -> Response<Body> {
        self.router.clone().oneshot(req).await.unwrap()
    }

    /// Runs /login + /callback; returns the callback response and new sid.
    async fn login(
        &self,
        return_to: &str,
        roles: &[&str],
        prev_sid: Option<&str>,
    ) -> (Response<Body>, Option<String>) {
        let r = self
            .login_with(LoginOpts {
                return_to,
                roles: Some(roles),
                prev_sid,
                ..Default::default()
            })
            .await;
        (r.res, r.sid)
    }

    async fn login_with(&self, o: LoginOpts<'_>) -> LoginResult {
        let enc: String = url::form_urlencoded::byte_serialize(o.return_to.as_bytes()).collect();
        let mut uri = format!("/login?return_to={enc}");
        if let Some(r) = o.reason {
            uri.push_str(&format!("&reason={r}"));
        }
        let mut b = Request::get(uri);
        if o.reauth_cookie {
            b = b.header(header::COOKIE, "__Host-console_reauth=1");
        }
        let res = self.call(b.body(Body::empty()).unwrap()).await;
        assert_eq!(res.status(), StatusCode::FOUND);
        let auth = set_cookies(&res)["__Host-console_auth"].clone();
        let authz = url::Url::parse(&location(&res)).unwrap();
        let q: HashMap<String, String> = authz.query_pairs().into_owned().collect();
        assert!(
            !q.contains_key("return_to"),
            "return_to never goes to the IdP"
        );
        assert_eq!(q["acr_values"], ACR, "acr_values requested");
        let now = self.clock.now();
        let mut spec = TokenSpec::valid(&self.stub.issuer, &q["nonce"], now);
        if let Some(roles) = o.roles {
            spec.roles = roles.iter().map(|r| (*r).to_owned()).collect();
        } else {
            spec.roles = vec!["console-user".into()];
        }
        if let Some(acr) = o.acr {
            spec.acr = acr.map(str::to_owned);
        }
        if let Some(off) = o.auth_time_offset {
            spec.auth_time = Some(now + chrono::Duration::seconds(off));
        }
        *self.stub.next.lock().unwrap() = Some(spec);

        let mut cookie = format!("__Host-console_auth={auth}");
        if let Some(sid) = o.prev_sid {
            cookie.push_str(&format!("; __Host-console_sid={sid}"));
        }
        let res = self
            .call(
                Request::get(format!("/callback?code=good-code&state={}", q["state"]))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        let sid = set_cookies(&res)
            .get("__Host-console_sid")
            .filter(|v| !v.is_empty())
            .cloned();
        LoginResult {
            res,
            sid,
            authorize: q,
        }
    }

    fn get(&self, path: &str, sid: &str) -> Request<Body> {
        Request::get(path)
            .header(header::COOKIE, format!("__Host-console_sid={sid}"))
            .body(Body::empty())
            .unwrap()
    }

    fn post(
        &self,
        path: &str,
        sid: &str,
        csrf: Option<&str>,
        origin: Option<&str>,
    ) -> Request<Body> {
        let mut b = Request::post(path).header(header::COOKIE, format!("__Host-console_sid={sid}"));
        if let Some(c) = csrf {
            b = b.header("x-csrf-token", c);
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::empty()).unwrap()
    }

    async fn session_json(&self, sid: &str) -> (StatusCode, serde_json::Value) {
        let res = self.call(self.get("/api/session", sid)).await;
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), 16 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or_default())
    }

    async fn audit_types(&self) -> Vec<(String, String, Option<String>)> {
        sqlx::query("SELECT event_type, outcome, reason FROM audit_events ORDER BY id")
            .fetch_all(&self.audit)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect()
    }

    async fn session_rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&self.owner)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn pg_full_flow_and_audit() {
    let Some(env) = setup(&[]).await else { return };
    // Capture everything (TRACE) on this thread for the no-secrets checks.
    let logs = LogBuf::default();
    let sink = logs.clone();
    let _guard = set_default(
        tracing_subscriber::fmt()
            .with_writer(move || sink.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish(),
    );
    let (res, sid) = env.login("/?site=x", &["console-user"], None).await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(location(&res), format!("{PUBLIC_URL}/?site=x"));
    let sid = sid.expect("sid cookie");
    assert!(
        set_cookies(&res)["__Host-console_auth"].is_empty(),
        "auth cookie cleared"
    );

    let (st, info) = env.session_json(&sid).await;
    assert_eq!(st, StatusCode::OK);
    let parsed: console_protocol::SessionInfo = serde_json::from_value(info.clone()).unwrap();
    assert_eq!(parsed.user.username, "jdoe");
    assert_eq!(parsed.roles, vec!["console-user".to_owned()]);
    assert_eq!(parsed.idle_timeout_secs, 900);
    assert!(
        !info.to_string().contains("eyJ"),
        "no token in /api/session"
    );
    let csrf = parsed.csrf_token.clone();

    // CSRF: missing token, wrong token, foreign origin.
    for (tok, origin) in [
        (None, Some(PUBLIC_URL)),
        (Some("AAAA"), Some(PUBLIC_URL)),
        (Some(csrf.as_str()), Some("https://evil.example")),
        (Some(csrf.as_str()), None),
    ] {
        let res = env
            .call(env.post("/api/session/keepalive", &sid, tok, origin))
            .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let res = env.call(env.post("/logout", &sid, tok, origin)).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    // Reads never extend the idle window; keepalive does.
    env.clock.advance(600);
    let (_, before) = env.session_json(&sid).await;
    assert_eq!(before["idle_expires_at"], info["idle_expires_at"]);
    let res = env
        .call(env.post(
            "/api/session/keepalive",
            &sid,
            Some(&csrf),
            Some(PUBLIC_URL),
        ))
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let (_, after) = env.session_json(&sid).await;
    assert_ne!(after["idle_expires_at"], info["idle_expires_at"]);

    // Authenticated unknown API path: honest JSON 404 (INT-003).
    let res = env.call(env.get("/api/session/v2", &sid)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(res).await,
        serde_json::json!({"error": "not_found"})
    );

    let res = env
        .call(env.post("/logout", &sid, Some(&csrf), Some(PUBLIC_URL)))
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(set_cookies(&res)["__Host-console_sid"].is_empty());
    assert!(
        cookie_attrs(&res, "__Host-console_reauth")
            .is_some_and(|c| c.contains("HttpOnly") && c.contains("Max-Age=43200")),
        "logout sets the reauth marker"
    );
    let body = axum::body::to_bytes(res.into_body(), 16 * 1024)
        .await
        .unwrap();
    let out: console_protocol::LogoutResponse = serde_json::from_slice(&body).unwrap();
    assert!(
        out.redirect
            .contains("/protocol/openid-connect/logout?id_token_hint=")
    );
    assert!(out.redirect.contains("client_id=usg-ops-console"));
    let issuer_origin = url::Url::parse(&env.stub.issuer)
        .unwrap()
        .origin()
        .ascii_serialization();
    assert_eq!(
        out.end_session_origin.as_deref(),
        Some(issuer_origin.as_str())
    );
    assert!(out.redirect.starts_with(&issuer_origin));

    let (st, body) = env.session_json(&sid).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "session gone on every tab");
    assert_eq!(body["reason"], "no_session");
    assert_eq!(env.session_rows().await, 0);

    let types = env.audit_types().await;
    let names: Vec<&str> = types.iter().map(|t| t.0.as_str()).collect();
    let mut expected = vec!["login.success"];
    expected.extend(["access.denied"; 8]);
    expected.extend(["session.refresh", "logout"]);
    assert_eq!(names, expected);
    // NIST-009: CSRF denials are audited with reason and outcome.
    let denials: Vec<Option<String>> = types
        .iter()
        .filter(|t| t.0 == "access.denied")
        .map(|t| {
            assert_eq!(t.1, "denied");
            t.2.clone()
        })
        .collect();
    assert_eq!(
        denials
            .iter()
            .filter(|r| r.as_deref() == Some("origin"))
            .count(),
        4
    );
    assert_eq!(
        denials
            .iter()
            .filter(|r| r.as_deref() == Some("csrf_token"))
            .count(),
        4
    );
    // AU-3 fields populated; no token text anywhere in the audit table.
    let row = sqlx::query(
        "SELECT user_sub, username, session_ref, request_id, occurred_at FROM audit_events \
         WHERE event_type = 'login.success'",
    )
    .fetch_one(&env.audit)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "sub-123");
    assert_eq!(row.get::<String, _>(1), "jdoe");
    assert_eq!(row.get::<String, _>(2).len(), 8);
    assert!(row.get::<Option<String>, _>(3).is_some());
    let leak: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE concat_ws(' ', user_sub, username, \
         session_ref, user_agent, reason, request_id) LIKE '%eyJ%'",
    )
    .fetch_one(&env.audit)
    .await
    .unwrap();
    assert_eq!(leak, 0);
    // The token request used client_secret_basic, not a body secret.
    assert!(
        env.stub
            .token_requests
            .lock()
            .unwrap()
            .iter()
            .all(|b| !b.contains(CLIENT_SECRET))
    );
    // The keepalive used the stored refresh token (NIST-001).
    assert!(
        env.stub
            .token_requests
            .lock()
            .unwrap()
            .iter()
            .any(|b| b.contains("grant_type=refresh_token") && b.contains("stub-refresh-1"))
    );
    // SEC-008 / IA-5: no code, state, tokens or secrets in any log line.
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("/callback"), "request spans were captured");
    for needle in [
        "good-code",
        "state=",
        "stub-refresh",
        "eyJ",
        CLIENT_SECRET,
        "code=",
    ] {
        if let Some(line) = text.lines().find(|l| l.contains(needle)) {
            let at = line.find(needle).unwrap_or(0);
            let ctx: String = line.chars().take(at.min(400)).collect();
            panic!("log leaked {needle:?}; line starts: {ctx}");
        }
    }
}

#[tokio::test]
async fn pg_session_fixation_rotates_id() {
    let Some(env) = setup(&[]).await else { return };
    let (_, a) = env.login("/", &["console-user"], None).await;
    let a = a.unwrap();
    let (_, b) = env.login("/", &["console-user"], Some(&a)).await;
    let b = b.unwrap();
    assert_ne!(a, b);
    assert_eq!(
        env.session_json(&a).await.0,
        StatusCode::UNAUTHORIZED,
        "old id is dead"
    );
    assert_eq!(env.session_json(&b).await.0, StatusCode::OK);
    assert_eq!(env.session_rows().await, 1);

    // A client-chosen (well-formed) id is never adopted.
    let chosen = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 32]);
    let (_, c) = env.login("/", &["console-user"], Some(&chosen)).await;
    assert_ne!(c.unwrap(), chosen);
}

#[tokio::test]
async fn pg_idle_and_absolute_expiry() {
    let Some(env) = setup(&[
        ("CONSOLE_SESSION_IDLE_SECS", "600"),
        ("CONSOLE_SESSION_ABSOLUTE_SECS", "900"),
    ])
    .await
    else {
        return;
    };
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let sid = sid.unwrap();
    env.clock.advance(599);
    assert_eq!(env.session_json(&sid).await.0, StatusCode::OK);
    env.clock.advance(1);
    let (st, body) = env.session_json(&sid).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["reason"], "idle_timeout");
    assert_eq!(
        env.session_json(&sid).await.1["reason"],
        "no_session",
        "reported once"
    );

    let (_, sid) = env.login("/", &["console-user"], None).await;
    let sid = sid.unwrap();
    let (_, info) = env.session_json(&sid).await;
    let csrf = info["csrf_token"].as_str().unwrap().to_owned();
    env.clock.advance(500);
    let res = env
        .call(env.post(
            "/api/session/keepalive",
            &sid,
            Some(&csrf),
            Some(PUBLIC_URL),
        ))
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 16 * 1024)
        .await
        .unwrap();
    let ka: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        ka["idle_expires_at"], ka["absolute_expires_at"],
        "capped by absolute"
    );
    env.clock.advance(400);
    let (st, body) = env.session_json(&sid).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["reason"], "absolute_timeout");

    let types = env.audit_types().await;
    assert!(types.contains(&(
        "session.expired.idle".into(),
        "success".into(),
        Some("on_access".into())
    )));
    assert!(types.contains(&(
        "session.expired.absolute".into(),
        "success".into(),
        Some("on_access".into())
    )));
}

#[tokio::test]
async fn pg_sweeper_deletes_and_audits() {
    let Some(env) = setup(&[]).await else { return };
    env.login("/", &["console-user"], None).await;
    env.login("/", &["console-user"], None).await;
    assert_eq!(sweeper::sweep_once(&env.state).await.unwrap(), 0);
    env.clock.advance(900);
    assert_eq!(sweeper::sweep_once(&env.state).await.unwrap(), 2);
    assert_eq!(env.session_rows().await, 0);
    let swept = env
        .audit_types()
        .await
        .into_iter()
        .filter(|t| t.0 == "session.expired.idle" && t.2.as_deref() == Some("swept"))
        .count();
    assert_eq!(swept, 2);
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_requests")
        .fetch_one(&env.owner)
        .await
        .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn pg_missing_role_is_denied_without_session() {
    let Some(env) = setup(&[]).await else { return };
    let (res, sid) = env.login("/", &["offline_access"], None).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(sid.is_none());
    assert_eq!(env.session_rows().await, 0);
    assert_eq!(
        env.audit_types().await,
        vec![(
            "login.denied".into(),
            "denied".into(),
            Some("missing_role".into())
        )]
    );
}

#[tokio::test]
async fn pg_callback_failures_and_single_use() {
    let Some(env) = setup(&[]).await else { return };
    // No auth cookie.
    let res = env
        .call(
            Request::get("/callback?code=good-code&state=x")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Wrong state, then replay of the (now consumed) auth request.
    let res = env
        .call(Request::get("/login").body(Body::empty()).unwrap())
        .await;
    let auth = set_cookies(&res)["__Host-console_auth"].clone();
    let cookie = format!("__Host-console_auth={auth}");
    for _ in 0..2 {
        let res = env
            .call(
                Request::get("/callback?code=good-code&state=wrong")
                    .header(header::COOKIE, cookie.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
    let reasons: Vec<Option<String>> = env.audit_types().await.into_iter().map(|t| t.2).collect();
    assert_eq!(
        reasons,
        vec![
            Some("no_auth_request".into()),
            Some("state_mismatch".into()),
            Some("no_auth_request".into())
        ]
    );

    // Open redirect attempt lands on the console root.
    let (res, _) = env.login("//evil.example", &["console-user"], None).await;
    assert_eq!(location(&res), format!("{PUBLIC_URL}/"));
}

#[tokio::test]
async fn pg_audit_is_append_only() {
    let Some(env) = setup(&[]).await else { return };
    env.login("/", &["console-user"], None).await;
    for pool in [&env.audit, &env.app, &env.owner] {
        for stmt in [
            "UPDATE audit_events SET reason = 'x'",
            "DELETE FROM audit_events",
            "TRUNCATE audit_events",
        ] {
            assert!(pool.execute(stmt).await.is_err(), "{stmt} must fail");
        }
    }
    // The audit role cannot touch sessions.
    assert!(env.audit.execute("SELECT * FROM sessions").await.is_err());
    // SEC-006: the runtime role cannot read, re-own or un-trigger the audit
    // table, nor drop schema objects; everything is owned by console_owner.
    for stmt in [
        "SELECT * FROM audit_events",
        "ALTER TABLE audit_events DISABLE TRIGGER USER",
        "ALTER TABLE audit_events DISABLE TRIGGER audit_events_no_update_delete",
        "DROP TABLE audit_events",
        "DROP TABLE sessions",
        "INSERT INTO audit_events (occurred_at, event_type, outcome) VALUES (now(), 'x', 'success')",
        "CREATE TABLE sneaky (x int)",
    ] {
        assert!(
            env.app.execute(stmt).await.is_err(),
            "app role: {stmt} must fail"
        );
    }
    for stmt in [
        "ALTER TABLE audit_events DISABLE TRIGGER USER",
        "DROP TABLE audit_events",
    ] {
        assert!(
            env.audit.execute(stmt).await.is_err(),
            "audit role: {stmt} must fail"
        );
    }
    let owners: Vec<(String, String)> = sqlx::query_as(
        "SELECT tablename::text, tableowner::text FROM pg_tables WHERE schemaname = 'public' \
         ORDER BY tablename",
    )
    .fetch_all(&env.owner)
    .await
    .unwrap();
    assert!(owners.len() >= 4, "{owners:?}");
    assert!(
        owners.iter().all(|(_, o)| o == "console_owner"),
        "{owners:?}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&env.audit)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn pg_reauth_after_logout_requires_fresh_auth() {
    let Some(env) = setup(&[]).await else { return };
    // First sign-in: SSO reuse is fine, no prompt.
    let r = env
        .login_with(LoginOpts {
            return_to: "/",
            ..Default::default()
        })
        .await;
    assert!(!r.authorize.contains_key("prompt"));
    let sid = r.sid.unwrap();
    let (_, info) = env.session_json(&sid).await;
    let csrf = info["csrf_token"].as_str().unwrap().to_owned();
    let res = env
        .call(env.post("/logout", &sid, Some(&csrf), Some(PUBLIC_URL)))
        .await;
    assert!(cookie_attrs(&res, "__Host-console_reauth").is_some());

    // Re-login with the marker but a reused SSO session (old auth_time).
    let r = env
        .login_with(LoginOpts {
            return_to: "/",
            reauth_cookie: true,
            auth_time_offset: Some(-3600),
            ..Default::default()
        })
        .await;
    assert_eq!(r.authorize["prompt"], "login", "SEC-001: prompt=login");
    assert_eq!(r.res.status(), StatusCode::BAD_REQUEST);
    assert!(r.sid.is_none());
    assert!(
        cookie_attrs(&r.res, "__Host-console_reauth").is_none(),
        "marker kept until a fresh sign-in"
    );
    assert!(body_text(r.res).await.contains("Sign in again"));
    // An auth_time far older than the login request is stale too.
    let mut no_time = env
        .login_with(LoginOpts {
            return_to: "/",
            reauth_cookie: true,
            auth_time_offset: Some(-100_000_000),
            ..Default::default()
        })
        .await;
    assert_eq!(no_time.res.status(), StatusCode::BAD_REQUEST);
    no_time.sid.take();

    // Fresh authentication succeeds and clears the marker.
    let r = env
        .login_with(LoginOpts {
            return_to: "/",
            reauth_cookie: true,
            auth_time_offset: Some(-5),
            ..Default::default()
        })
        .await;
    assert_eq!(r.res.status(), StatusCode::FOUND);
    assert!(r.sid.is_some());
    assert!(cookie_attrs(&r.res, "__Host-console_reauth").is_some_and(|c| c.contains("Max-Age=0")));
    let types = env.audit_types().await;
    assert!(types.contains(&(
        "login.failure".into(),
        "failure".into(),
        Some("stale_auth".into())
    )));
    assert!(types.contains(&(
        "login.success".into(),
        "success".into(),
        Some("reauthenticated".into())
    )));
    let stale_with_user: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE reason = 'stale_auth' AND user_sub = 'sub-123'",
    )
    .fetch_one(&env.audit)
    .await
    .unwrap();
    assert_eq!(stale_with_user, 2, "stale_auth records who");
}

#[tokio::test]
async fn pg_expired_reason_forces_prompt_and_notice() {
    let Some(env) = setup(&[]).await else { return };
    let r = env
        .login_with(LoginOpts {
            return_to: "/gallery?site=hq",
            reason: Some("expired"),
            ..Default::default()
        })
        .await;
    assert_eq!(r.authorize["prompt"], "login");
    assert_eq!(
        location(&r.res),
        format!("{PUBLIC_URL}/gallery?site=hq&console_notice=expired")
    );
    // Unknown reasons are ignored (not reflected, no prompt).
    let r = env
        .login_with(LoginOpts {
            return_to: "/",
            reason: Some("%3Cscript%3E"),
            ..Default::default()
        })
        .await;
    assert!(!r.authorize.contains_key("prompt"));
    assert_eq!(location(&r.res), format!("{PUBLIC_URL}/"));

    // An idle-expired navigation is sent to /login with reason=expired and
    // gets the reauth marker.
    let sid = r.sid.unwrap();
    env.clock.advance(900);
    let res = env
        .call(
            Request::get("/gallery")
                .header(header::COOKIE, format!("__Host-console_sid={sid}"))
                .header("sec-fetch-mode", "navigate")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(location(&res), "/login?return_to=%2Fgallery&reason=expired");
    assert!(cookie_attrs(&res, "__Host-console_reauth").is_some());
}

#[tokio::test]
async fn pg_insufficient_assurance_is_denied() {
    let Some(env) = setup(&[]).await else { return };
    for acr in [Some("1"), Some("0"), None] {
        let r = env
            .login_with(LoginOpts {
                return_to: "/",
                acr: Some(acr),
                ..Default::default()
            })
            .await;
        assert_eq!(r.res.status(), StatusCode::FORBIDDEN, "{acr:?}");
        assert!(r.sid.is_none());
        let html = body_text(r.res).await;
        assert!(html.contains("CAC/PIV sign-in required"));
        assert!(html.contains("Sign out of single sign-on"));
        assert!(
            html.contains("client_id=usg-ops-console&amp;") || html.contains("&amp;client_id=")
        );
    }
    assert_eq!(env.session_rows().await, 0);
    let types = env.audit_types().await;
    assert_eq!(types.len(), 3);
    assert!(types.iter().all(|t| t
        == &(
            "login.denied".into(),
            "denied".into(),
            Some("insufficient_assurance".into())
        )));
}

#[tokio::test]
async fn pg_keepalive_revalidates_with_idp() {
    let Some(env) = setup(&[]).await else { return };
    let keepalive = |sid: String, csrf: String| {
        env.call(env.post(
            "/api/session/keepalive",
            &sid,
            Some(&csrf),
            Some(PUBLIC_URL),
        ))
    };
    let env = &env;
    let sid_csrf = |sid: String| async move {
        let (_, info) = env.session_json(&sid).await;
        (sid, info["csrf_token"].as_str().unwrap().to_owned())
    };

    // OK: roles are re-read from the refreshed id_token.
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let (sid, csrf) = sid_csrf(sid.unwrap()).await;
    env.stub.next.lock().unwrap().as_mut().unwrap().roles =
        vec!["console-user".into(), "console-admin".into()];
    env.clock.advance(120);
    let res = keepalive(sid.clone(), csrf.clone()).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        body_json(res).await["roles"],
        serde_json::json!(["console-admin", "console-user"])
    );
    let stored: Option<String> = sqlx::query_scalar("SELECT refresh_token FROM sessions")
        .fetch_one(&env.owner)
        .await
        .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some("stub-refresh-2"),
        "rotated token stored"
    );

    // IdP unreachable: 503 JSON, not extended, not ended.
    *env.stub.refresh.lock().unwrap() = RefreshMode::Broken;
    let (_, before) = env.session_json(&sid).await;
    env.clock.advance(60);
    let res = keepalive(sid.clone(), csrf.clone()).await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body_json(res).await,
        serde_json::json!({"error": "unavailable"})
    );
    let (st, after) = env.session_json(&sid).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(before["idle_expires_at"], after["idle_expires_at"]);

    // Role removed at the IdP: session ends, audit missing_role.
    *env.stub.refresh.lock().unwrap() = RefreshMode::Ok;
    env.stub.next.lock().unwrap().as_mut().unwrap().roles = vec!["offline_access".into()];
    let res = keepalive(sid.clone(), csrf.clone()).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(cookie_attrs(&res, "__Host-console_reauth").is_some());
    assert_eq!(body_json(res).await["reason"], "revoked");
    assert_eq!(env.session_json(&sid).await.0, StatusCode::UNAUTHORIZED);

    // User disabled / SSO session revoked: invalid_grant.
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let (sid, csrf) = sid_csrf(sid.unwrap()).await;
    *env.stub.refresh.lock().unwrap() = RefreshMode::Reject;
    let res = keepalive(sid.clone(), csrf).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(res).await["reason"], "revoked");
    assert_eq!(env.session_rows().await, 0);

    // Subject swap in the refreshed token.
    *env.stub.refresh.lock().unwrap() = RefreshMode::Ok;
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let (sid, csrf) = sid_csrf(sid.unwrap()).await;
    env.stub.next.lock().unwrap().as_mut().unwrap().sub = "someone-else".into();
    assert_eq!(
        keepalive(sid, csrf).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let revoked: Vec<Option<String>> = env
        .audit_types()
        .await
        .into_iter()
        .filter(|t| t.0 == "session.revoked")
        .map(|t| t.2)
        .collect();
    assert_eq!(
        revoked,
        vec![
            Some("missing_role".into()),
            Some("idp_rejected".into()),
            Some("subject_mismatch".into())
        ]
    );
}

#[tokio::test]
async fn pg_failed_callback_ends_presented_session() {
    let Some(env) = setup(&[]).await else { return };
    let (_, a) = env.login("/", &["console-user"], None).await;
    let a = a.unwrap();
    // Someone else signs in on the same browser and is denied.
    let (res, b) = env.login("/", &["offline_access"], Some(&a)).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(b.is_none());
    assert!(
        cookie_attrs(&res, "__Host-console_sid").is_some_and(|c| c.contains("Max-Age=0")),
        "SEC-007: sid cleared"
    );
    assert_eq!(env.session_json(&a).await.0, StatusCode::UNAUTHORIZED);
    let types = env.audit_types().await;
    assert!(types.contains(&("logout".into(), "success".into(), Some("superseded".into()))));

    // Before state is verified, a presented session is left alone (a
    // cross-site link to /callback cannot log the user out).
    let (_, c) = env.login("/", &["console-user"], None).await;
    let c = c.unwrap();
    let res = env
        .call(
            Request::get("/callback?code=good-code&state=x")
                .header(header::COOKIE, format!("__Host-console_sid={c}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(env.session_json(&c).await.0, StatusCode::OK);
}

#[tokio::test]
async fn pg_login_rate_limit_is_audited_once() {
    let Some(env) = setup_with(&[], 2).await else {
        return;
    };
    let get = |accept: &'static str| {
        env.call(
            Request::get("/login")
                .header(header::ACCEPT, accept)
                .body(Body::empty())
                .unwrap(),
        )
    };
    assert_eq!(get("text/html").await.status(), StatusCode::FOUND);
    assert_eq!(get("text/html").await.status(), StatusCode::FOUND);
    let res = get("text/html").await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers()[header::RETRY_AFTER], "60");
    assert!(body_text(res).await.contains("Too many sign-in attempts"));
    let res = get("application/json").await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body_json(res).await,
        serde_json::json!({"error": "rate_limited"})
    );
    assert_eq!(
        env.audit_types().await,
        vec![(
            "access.denied".into(),
            "denied".into(),
            Some("rate_limited".into())
        )],
        "one audit row per episode"
    );
}

#[tokio::test]
async fn pg_pending_auth_requests_are_capped() {
    let Some(env) = setup(&[]).await else { return };
    sqlx::query(
        "INSERT INTO auth_requests (id_hash, state, nonce, pkce_verifier, return_to, expires_at) \
         SELECT sha256(i::text::bytea), 's', 'n', 'p', '/', $1 FROM generate_series(1, $2) i",
    )
    .bind(env.clock.now() + chrono::Duration::seconds(600))
    .bind(i32::try_from(console_gateway::session::MAX_PENDING_AUTH).unwrap())
    .execute(&env.owner)
    .await
    .unwrap();
    let res = env
        .call(
            Request::get("/login")
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body_json(res).await,
        serde_json::json!({"error": "unavailable"})
    );
    // Once they expire (and are swept), sign-in works again.
    env.clock.advance(601);
    sweeper::sweep_once(&env.state).await.unwrap();
    let res = env
        .call(Request::get("/login").body(Body::empty()).unwrap())
        .await;
    assert_eq!(res.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn pg_form_logout_with_json_accept_and_limits() {
    let Some(env) = setup(&[]).await else { return };
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let sid = sid.unwrap();
    let (_, info) = env.session_json(&sid).await;
    let csrf = info["csrf_token"].as_str().unwrap().to_owned();
    let form = |body: String, accept: &'static str| {
        Request::post("/logout")
            .header(header::COOKIE, format!("__Host-console_sid={sid}"))
            .header(header::ORIGIN, PUBLIC_URL)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT, accept)
            .body(Body::from(body))
            .unwrap()
    };
    let res = env.call(form("x=".repeat(5000), "text/html")).await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body_json(res).await,
        serde_json::json!({"error": "payload_too_large"})
    );
    // INT-005: a fetch() with a form body and Accept: application/json gets JSON.
    let res = env
        .call(form(format!("csrf={csrf}"), "application/json"))
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let out: console_protocol::LogoutResponse =
        serde_json::from_value(body_json(res).await).unwrap();
    assert!(out.end_session_origin.is_some());

    // The no-JS form path: 303 to the IdP, allowed by CSP form-action.
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let sid = sid.unwrap();
    let (_, info) = env.session_json(&sid).await;
    let csrf = info["csrf_token"].as_str().unwrap().to_owned();
    let res = env
        .call(
            Request::post("/logout")
                .header(header::COOKIE, format!("__Host-console_sid={sid}"))
                .header(header::ORIGIN, PUBLIC_URL)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf={csrf}")))
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let issuer_origin = url::Url::parse(&env.stub.issuer)
        .unwrap()
        .origin()
        .ascii_serialization();
    assert!(location(&res).starts_with(&issuer_origin));
    let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        csp.contains(&format!("form-action 'self' {issuer_origin}")),
        "SEC-004: {csp}"
    );
}
