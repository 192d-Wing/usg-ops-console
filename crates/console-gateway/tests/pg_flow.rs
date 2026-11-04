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
    setup_full(extra, login_per_minute, None).await
}

/// Env with the test registry against a fresh echo upstream.
async fn setup_tools(extra: &[(&str, &str)]) -> Option<(Env, Echo)> {
    if std::env::var("CONSOLE_TEST_DATABASE_URL").is_err() {
        println!("NOT RUN: CONSOLE_TEST_DATABASE_URL unset");
        return None;
    }
    let echo = start_echo().await;
    let env = setup_full(extra, 1000, Some(registry_toml(echo.port))).await?;
    Some((env, echo))
}

async fn setup_full(
    extra: &[(&str, &str)],
    login_per_minute: u32,
    registry: Option<String>,
) -> Option<Env> {
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
    let cfg_for_tools = cfg.clone();
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
        tools: Arc::new(match &registry {
            Some(r) => tools_state(&cfg_for_tools, r),
            None => console_gateway::registry::ToolsState::empty(),
        }),
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

// ---------------------------------------------------------------------------
// M4: tools (launch handoff, proxy, assertion, lifecycle, limits, POAM-031)
// ---------------------------------------------------------------------------

const TIGHT_HOST: &str = "tight.tools.test";

impl Env {
    async fn csrf(&self, sid: &str) -> String {
        let (st, info) = self.session_json(sid).await;
        assert_eq!(st, StatusCode::OK);
        info["csrf_token"].as_str().unwrap().to_owned()
    }

    async fn launch(
        &self,
        sid: &str,
        csrf: Option<&str>,
        tool: &str,
        body: &str,
    ) -> Response<Body> {
        let mut b = Request::post(format!("/api/tools/{tool}/launch"))
            .header(header::COOKIE, format!("__Host-console_sid={sid}"))
            .header(header::ORIGIN, PUBLIC_URL)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(c) = csrf {
            b = b.header("x-csrf-token", c);
        }
        self.call(b.body(Body::from(body.to_owned())).unwrap())
            .await
    }

    /// Launch URL for `tool` (asserts 200).
    async fn launch_url(&self, sid: &str, tool: &str, body: &str) -> String {
        let csrf = self.csrf(sid).await;
        let res = self.launch(sid, Some(&csrf), tool, body).await;
        assert_eq!(res.status(), StatusCode::OK);
        body_json(res).await["url"].as_str().unwrap().to_owned()
    }

    /// GETs a launch URL on `host` the way the shell's tool iframe does
    /// (Fetch Metadata `iframe` / `same-site`, SEC-001).
    async fn exchange(&self, host: &str, url: &str) -> Response<Body> {
        self.exchange_with(
            host,
            url,
            &[
                ("sec-fetch-dest", "iframe"),
                ("sec-fetch-site", "same-site"),
            ],
        )
        .await
    }

    /// GETs a launch URL on `host` with exactly `headers`.
    async fn exchange_with(
        &self,
        host: &str,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Response<Body> {
        let u = url::Url::parse(url).unwrap();
        let pq = format!("{}?{}", u.path(), u.query().unwrap_or_default());
        let mut b = Request::get(pq).header(header::HOST, host);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        self.call(b.body(Body::empty()).unwrap()).await
    }

    /// Launch + exchange; returns the `__Host-tool_sid` value.
    async fn open_tool(&self, sid: &str, tool: &str, host: &str) -> String {
        let url = self.launch_url(sid, tool, "{}").await;
        let res = self.exchange(host, &url).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        set_cookies(&res)["__Host-tool_sid"].clone()
    }

    fn tool_req(&self, host: &str, path: &str, tool_sid: &str) -> axum::http::request::Builder {
        Request::get(path)
            .header(header::HOST, host)
            .header(header::COOKIE, format!("__Host-tool_sid={tool_sid}"))
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.owner)
            .await
            .unwrap()
    }

    async fn tool_audit(
        &self,
    ) -> Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i32>,
    )> {
        // Proxy-path rows are written by the background queue (PERF-002).
        self.state.audit.flush().await;
        sqlx::query(
            "SELECT event_type, outcome, reason, tool_id, path, http_status FROM audit_events \
             WHERE event_type LIKE 'tool.%' ORDER BY id",
        )
        .fetch_all(&self.audit)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5)))
        .collect()
    }
}

/// Verifies an assertion the way a tool must: JWKS from the console origin,
/// key by `kid`, ES256 pinned, `aud` = own id, `iss` = console.
async fn verify_assertion(env: &Env, token: &str, aud: &str) -> console_protocol::AssertionClaims {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
    let res = env
        .call(
            Request::get("/.well-known/jwks.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let set: JwkSet = serde_json::from_value(body_json(res).await).unwrap();
    let h = decode_header(token).unwrap();
    assert_eq!(h.alg, Algorithm::ES256);
    assert_eq!(h.typ.as_deref(), Some("console-assertion+jwt"));
    let jwk = set.find(h.kid.as_deref().unwrap()).expect("kid in JWKS");
    let mut v = Validation::new(Algorithm::ES256);
    v.set_audience(&[aud]);
    v.set_issuer(&[PUBLIC_URL]);
    decode::<console_protocol::AssertionClaims>(token, &DecodingKey::from_jwk(jwk).unwrap(), &v)
        .expect("assertion verifies with the published JWKS")
        .claims
}

#[tokio::test]
async fn pg_tool_launch_proxy_and_assertion() {
    let Some((env, echo)) = setup_tools(&[]).await else {
        return;
    };
    let (_, sid) = env
        .login(
            "/",
            &["console-user", "hello:user", "hello:admin", "other:x"],
            None,
        )
        .await;
    let sid = sid.unwrap();
    let (_, info) = env.session_json(&sid).await;
    assert_eq!(
        info["roles"],
        serde_json::json!(["console-user", "hello:admin", "hello:user"]),
        "registry-used roles kept; unrelated roles dropped"
    );

    // /api/tools: filtered by roles, browser-safe shape.
    let res = env.call(env.get("/api/tools", &sid)).await;
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    let tools = body_json(res).await;
    let ids: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["hello", "tight", "dead"], "`denied` hidden");
    assert_eq!(tools["tools"][0]["origin"], HELLO_ORIGIN);
    assert!(
        !tools.to_string().contains("127.0.0.1"),
        "no upstream leaked"
    );

    // Launch: CSRF, unknown tool, bad path.
    let csrf = env.csrf(&sid).await;
    assert_eq!(
        env.launch(&sid, None, "hello", "{}").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        env.launch(&sid, Some(&csrf), "nope", "{}").await.status(),
        StatusCode::NOT_FOUND
    );
    for bad in [
        r#"{"path":"//evil.example"}"#,
        r#"{"path":"https://x"}"#,
        r#"{"path":"/../x"}"#,
        "nope",
    ] {
        let res = env.launch(&sid, Some(&csrf), "hello", bad).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    let url = env.launch_url(&sid, "hello", r#"{"path":"/x?y=1"}"#).await;
    assert!(
        url.starts_with(&format!("{HELLO_ORIGIN}/_console/launch?code=")),
        "{url}"
    );

    // Exchange: 303 into /t/hello, strict host-only cookie, single use.
    let res = env.exchange(HELLO_HOST, &url).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&res), "/t/hello/x?y=1");
    assert_eq!(res.headers()[header::REFERRER_POLICY], "no-referrer");
    let attrs = cookie_attrs(&res, "__Host-tool_sid").unwrap();
    for flag in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(attrs.contains(flag), "{attrs}");
    }
    assert!(!attrs.contains("Domain") && !attrs.contains("Max-Age"));
    let tool_sid = set_cookies(&res)["__Host-tool_sid"].clone();
    assert_eq!(
        env.exchange(HELLO_HOST, &url).await.status(),
        StatusCode::UNAUTHORIZED,
        "single use"
    );

    let last_seen = |env: &Env| {
        let pool = env.owner.clone();
        async move {
            sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
                "SELECT last_seen_at FROM sessions",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let before = last_seen(&env).await;
    env.clock.advance(5);

    // Spoofed identity never reaches the upstream.
    let spoofed = |path: &str| {
        env.tool_req(HELLO_HOST, path, &tool_sid)
            .header(
                header::COOKIE,
                format!("__Host-console_sid={sid}; app=1; __Host-console_auth=x"),
            )
            .header("x-console-user", "admin")
            .header("X-Console-Roles", "admin")
            .header("x-console-assertion", "forged")
            .header("x-forwarded-user", "admin")
            .header("x-forwarded-for", "6.6.6.6")
            .header("forwarded", "for=6.6.6.6")
            .header("x-real-ip", "6.6.6.6")
            .header("authorization", "Bearer stolen")
            .header("x-console-scope", "site=dc1;tenant=t1")
            .header("x-hello-csrf", "c")
            .header("connection", "x-hello-csrf")
            .header("sec-fetch-mode", "navigate")
            .body(Body::empty())
            .unwrap()
    };
    let res = env.call(spoofed("/t/hello/echo?q=1")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let seen = echo.last();
    assert_eq!(
        seen.uri, "/t/hello/echo?q=1",
        "path + query forwarded verbatim"
    );
    let h = &seen.headers;
    for gone in [
        "x-console-user",
        "x-console-roles",
        "x-forwarded-user",
        "forwarded",
        "x-real-ip",
        "authorization",
        "x-console-scope",
        "x-hello-csrf",
    ] {
        assert!(!h.contains_key(gone), "{gone} reached the upstream");
    }
    assert_eq!(
        h["cookie"], "app=1",
        "console/tool session cookies never forwarded"
    );
    assert_ne!(h["x-console-assertion"], "forged");
    assert_eq!(h.get_all("x-console-assertion").iter().count(), 1);
    assert!(h.get("x-forwarded-for").is_none_or(|v| v != "6.6.6.6"));
    assert_eq!(h["x-forwarded-proto"], "https");
    assert_eq!(h["x-forwarded-host"], HELLO_HOST);
    assert!(h.contains_key("x-request-id"));
    assert_eq!(h["host"], format!("127.0.0.1:{}", echo.port));

    // Response filtering.
    let cookies: Vec<String> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(cookies, vec!["pref=dark; Path=/"]);
    assert!(res.headers().get(header::SERVER).is_none());
    assert_eq!(
        res.headers()[header::STRICT_TRANSPORT_SECURITY],
        "max-age=31536000; includeSubDomains"
    );
    assert!(
        res.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .starts_with(&format!("frame-ancestors {PUBLIC_URL};"))
    );

    // The assertion verifies against the JWKS and carries the agreed claims.
    let token = h["x-console-assertion"].to_str().unwrap().to_owned();
    let c = verify_assertion(&env, &token, "hello").await;
    assert_eq!(c.sub, "sub-123");
    assert_eq!(c.preferred_username, "jdoe");
    assert_eq!(c.name, "Jane Doe");
    assert_eq!(
        c.roles,
        ["hello:admin", "hello:user"],
        "forward_roles prefixes only"
    );
    assert_eq!(c.ctx.site.as_deref(), Some("dc1"));
    assert_eq!(c.ctx.tenant, None, "tenant not in the registry scopes");
    assert_eq!(c.exp - c.iat, 60);
    assert_eq!(c.nbf, c.iat);
    assert_eq!(c.acr.as_deref(), Some(ACR));
    assert_eq!(c.sid.len(), 22);
    let session_ref: String = info["user"]["id"].as_str().unwrap().to_owned();
    assert_ne!(c.sid, session_ref);
    let sref = console_gateway::session::session_ref(
        &console_gateway::session::hash_cookie_value(&sid).unwrap(),
    );
    assert!(!c.sid.contains(&sref), "sid is not the console session ref");
    assert!(verify_assertion_fails_for_other_aud(&env, &token).await);

    // A second request: same pseudonym, fresh jti; spoof audited once/min.
    let res = env.call(spoofed("/t/hello/echo")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let c2 = verify_assertion(
        &env,
        echo.last().headers["x-console-assertion"].to_str().unwrap(),
        "hello",
    )
    .await;
    assert_eq!(c2.sid, c.sid);
    assert_ne!(c2.jti, c.jti);

    // Location on the upstream authority is rewritten to the tool origin.
    let res = env
        .call(
            env.tool_req(HELLO_HOST, "/t/hello/redirect", &tool_sid)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(location(&res), format!("{HELLO_ORIGIN}/t/hello/next"));
    // SEC-004: a Location off the tool origin is refused with 502.
    let res = env
        .call(
            env.tool_req(HELLO_HOST, "/t/hello/foreign-redirect", &tool_sid)
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    assert!(res.headers().get(header::LOCATION).is_none());
    assert_eq!(body_json(res).await["error"], "bad_gateway");

    // State-changing request on an audit_writes tool.
    let res = env
        .call(
            Request::post("/t/hello/echo")
                .header(header::HOST, HELLO_HOST)
                .header(header::COOKIE, format!("__Host-tool_sid={tool_sid}"))
                .header("sec-fetch-mode", "cors")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from("hello"))
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(echo.last().body_len, 5, "request body streamed through");

    // SDK context.
    let res = env
        .call(
            env.tool_req(HELLO_HOST, "/_console/context", &tool_sid)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let ctx: console_protocol::ToolContext = serde_json::from_value(body_json(res).await).unwrap();
    assert_eq!(ctx.tool_id, "hello");
    assert_eq!(ctx.console_origin, PUBLIC_URL);
    assert_eq!(ctx.protocol, console_protocol::PROTOCOL_VERSION);

    // The tool session cannot be used on another tool's origin.
    let res = env
        .call(
            env.tool_req(TIGHT_HOST, "/t/tight/x", &tool_sid)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    assert_eq!(
        last_seen(&env).await,
        before,
        "tool traffic never extends the console idle window"
    );

    let audit = env.tool_audit().await;
    let has = |t: &str, o: &str, r: &str| {
        audit
            .iter()
            .any(|a| a.0 == t && a.1 == o && a.2.as_deref() == Some(r))
    };
    assert!(has("tool.launch", "success", "launch"), "{audit:?}");
    assert!(has("tool.access", "success", "navigation"), "{audit:?}");
    assert!(has("tool.request", "success", "write"), "{audit:?}");
    assert!(
        has("tool.denied", "denied", "bad_code"),
        "reused code: {audit:?}"
    );
    assert_eq!(
        audit
            .iter()
            .filter(|a| a.2.as_deref() == Some("identity_header_stripped"))
            .count(),
        1,
        "spoofing audited once per tool session per minute: {audit:?}"
    );
    let launch = audit.iter().find(|a| a.0 == "tool.launch").unwrap();
    assert_eq!(launch.3.as_deref(), Some("hello"));
    assert_eq!(launch.4.as_deref(), Some("/x"), "no query in audit paths");
    let access = audit.iter().find(|a| a.0 == "tool.access").unwrap();
    assert_eq!(access.4.as_deref(), Some("/t/hello/echo"));
    assert_eq!(access.5, Some(200));
}

async fn verify_assertion_fails_for_other_aud(env: &Env, token: &str) -> bool {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, jwk::JwkSet};
    let res = env
        .call(
            Request::get("/.well-known/jwks.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let set: JwkSet = serde_json::from_value(body_json(res).await).unwrap();
    let mut v = Validation::new(Algorithm::ES256);
    v.set_audience(&["evil"]);
    decode::<console_protocol::AssertionClaims>(
        token,
        &DecodingKey::from_jwk(&set.keys[0]).unwrap(),
        &v,
    )
    .is_err()
}

#[tokio::test]
async fn pg_tool_role_denied() {
    let Some((env, echo)) = setup_tools(&[]).await else {
        return;
    };
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let sid = sid.unwrap();
    let tools = body_json(env.call(env.get("/api/tools", &sid)).await).await;
    let ids: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["tight", "dead"]);
    let csrf = env.csrf(&sid).await;
    // INT-009: identical to an unknown id, decided before the body is
    // parsed (a malformed body still gets 404, not 400).
    let unknown = env.launch(&sid, Some(&csrf), "nope", "{}").await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let unknown = body_json(unknown).await;
    for body in ["{}", "not json"] {
        let res = env.launch(&sid, Some(&csrf), "hello", body).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body_json(res).await, unknown);
    }
    assert_eq!(
        env.count("tool_launch_codes").await,
        0,
        "no code for a denied launch"
    );

    // Roles removed after the tool session exists: the proxy re-checks.
    let (_, sid2) = env.login("/", &["console-user", "hello:user"], None).await;
    let sid2 = sid2.unwrap();
    let tool_sid = env.open_tool(&sid2, "hello", HELLO_HOST).await;
    sqlx::query("UPDATE sessions SET roles = ARRAY['console-user']")
        .execute(&env.owner)
        .await
        .unwrap();
    let calls = echo.count();
    let res = env
        .call(
            env.tool_req(HELLO_HOST, "/t/hello/echo", &tool_sid)
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(res).await["error"], "forbidden");
    assert_eq!(
        echo.count(),
        calls,
        "denied requests never reach the upstream"
    );
    let audit = env.tool_audit().await;
    assert!(audit.iter().any(|a| a.0 == "tool.denied"
        && a.1 == "denied"
        && a.2.as_deref() == Some("missing_role")
        && a.5 == Some(404)));
    assert!(
        !audit
            .iter()
            .any(|a| a.0 == "tool.launch" && a.1 == "denied"),
        "a denial is tool.denied, not tool.launch"
    );
    assert!(audit.iter().any(|a| a.0 == "tool.denied"
        && a.2.as_deref() == Some("missing_role")
        && a.5 == Some(403)));
}

#[tokio::test]
async fn pg_tool_session_lifecycle() {
    let Some((env, _echo)) = setup_tools(&[]).await else {
        return;
    };
    let roles = ["console-user", "hello:user"];
    let (_, sid) = env.login("/", &roles, None).await;
    let sid = sid.unwrap();

    // INT-005: each launch (tab) gets its own tool session; up to 8 per
    // console session and tool, the oldest launch evicted beyond that.
    let get = |t: &str| {
        env.tool_req(HELLO_HOST, "/t/hello/echo", t)
            .body(Body::empty())
            .unwrap()
    };
    let mut tabs = Vec::new();
    for _ in 0..8 {
        tabs.push(env.open_tool(&sid, "hello", HELLO_HOST).await);
        env.clock.advance(1);
    }
    assert_eq!(env.count("tool_sessions").await, 8);
    for t in &tabs {
        assert_eq!(
            env.call(get(t)).await.status(),
            StatusCode::OK,
            "parallel tabs"
        );
    }
    let tight = env.open_tool(&sid, "tight", TIGHT_HOST).await;
    let tool_sid = env.open_tool(&sid, "hello", HELLO_HOST).await;
    assert_eq!(env.count("tool_sessions").await, 9, "8 hello + 1 tight");
    assert_eq!(
        env.call(get(&tabs[0])).await.status(),
        StatusCode::UNAUTHORIZED,
        "oldest launch evicted"
    );
    for t in tabs.iter().skip(1).chain([&tool_sid]) {
        assert_eq!(env.call(get(t)).await.status(), StatusCode::OK);
    }
    let tight_req = env
        .tool_req(TIGHT_HOST, "/t/tight/echo", &tight)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        env.call(tight_req).await.status(),
        StatusCode::OK,
        "other tool's sessions untouched"
    );

    // SEC-001: a launch URL opened outside the console's tool iframe (a
    // top-level link, a cross-site frame) gets 401, no cookie, and the code
    // is not consumed.
    let url = env.launch_url(&sid, "hello", "{}").await;
    for ctx in [
        &[("sec-fetch-dest", "document"), ("sec-fetch-site", "none")][..],
        &[
            ("sec-fetch-dest", "document"),
            ("sec-fetch-site", "cross-site"),
        ][..],
        &[
            ("sec-fetch-dest", "iframe"),
            ("sec-fetch-site", "cross-site"),
        ][..],
        &[][..],
        &[("referer", "https://evil.example/")][..],
    ] {
        let res = env.exchange_with(HELLO_HOST, &url, ctx).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{ctx:?}");
        assert!(
            cookie_attrs(&res, "__Host-tool_sid").is_none(),
            "no cookie: {ctx:?}"
        );
    }
    let audit = env.tool_audit().await;
    assert!(
        audit
            .iter()
            .any(|a| a.0 == "tool.denied" && a.2.as_deref() == Some("launch_context")),
        "{audit:?}"
    );
    let res = env
        .exchange_with(
            HELLO_HOST,
            &url,
            &[("referer", &format!("{PUBLIC_URL}/tools/hello/"))],
        )
        .await;
    assert_eq!(
        res.status(),
        StatusCode::SEE_OTHER,
        "no Fetch Metadata: console Referer accepted; code still valid"
    );

    // Codes: expiry, wrong origin, single use.
    let url = env.launch_url(&sid, "hello", "{}").await;
    env.clock.advance(31);
    assert_eq!(
        env.exchange(HELLO_HOST, &url).await.status(),
        StatusCode::UNAUTHORIZED,
        "expired"
    );
    let url = env.launch_url(&sid, "hello", "{}").await;
    assert_eq!(
        env.exchange(TIGHT_HOST, &url).await.status(),
        StatusCode::UNAUTHORIZED,
        "wrong tool origin"
    );
    assert_eq!(
        env.exchange(HELLO_HOST, &url).await.status(),
        StatusCode::UNAUTHORIZED,
        "consumed"
    );

    // Sweeper removes expired codes.
    let _ = env.launch_url(&sid, "hello", "{}").await;
    assert_eq!(env.count("tool_launch_codes").await, 1);
    env.clock.advance(31);
    sweeper::sweep_once(&env.state).await.unwrap();
    assert_eq!(env.count("tool_launch_codes").await, 0);

    // Logout kills the tool session and any outstanding code.
    let url = env.launch_url(&sid, "hello", "{}").await;
    let csrf = env.csrf(&sid).await;
    let res = env
        .call(env.post("/logout", &sid, Some(&csrf), Some(PUBLIC_URL)))
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(env.count("tool_sessions").await, 0);
    assert_eq!(env.count("tool_launch_codes").await, 0);
    let res = env.call(get(&tool_sid)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(
        cookie_attrs(&res, "__Host-tool_sid")
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(
        env.exchange(HELLO_HOST, &url).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // Console idle expiry blocks tool requests (and ends the console session).
    let (_, sid) = env.login("/", &roles, None).await;
    let sid = sid.unwrap();
    let tool_sid = env.open_tool(&sid, "hello", HELLO_HOST).await;
    env.clock.advance(901);
    let res = env
        .call(
            env.tool_req(HELLO_HOST, "/t/hello/echo", &tool_sid)
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(res).await["reason"], "idle_timeout");
    assert_eq!(env.session_rows().await, 0);
    assert!(
        env.audit_types()
            .await
            .iter()
            .any(|(t, _, r)| t == "session.expired.idle" && r.as_deref() == Some("on_access"))
    );
}

#[tokio::test]
async fn pg_tool_proxy_limits() {
    let Some((env, echo)) = setup_tools(&[]).await else {
        return;
    };
    let (_, sid) = env.login("/", &["console-user", "hello:user"], None).await;
    let sid = sid.unwrap();
    let hello = env.open_tool(&sid, "hello", HELLO_HOST).await;
    let tight = env.open_tool(&sid, "tight", TIGHT_HOST).await;
    let dead = env.open_tool(&sid, "dead", "dead.tools.test").await;
    let json = |b: axum::http::request::Builder| b.header(header::ACCEPT, "application/json");

    // WebSocket upgrade: 501, never proxied.
    let calls = echo.count();
    let res = env
        .call(
            json(env.tool_req(HELLO_HOST, "/t/hello/ws", &hello))
                .header("connection", "Upgrade")
                .header("upgrade", "websocket")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body_json(res).await["error"], "not_implemented");
    assert_eq!(echo.count(), calls);

    // Body limit: by Content-Length up front, and while streaming.
    let res = env
        .call(
            json(Request::post("/t/tight/up").header(header::HOST, TIGHT_HOST))
                .header(header::COOKIE, format!("__Host-tool_sid={tight}"))
                .header(header::CONTENT_LENGTH, "100")
                .body(Body::from(vec![b'x'; 100]))
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let res = env
        .call(
            json(Request::post("/t/tight/up").header(header::HOST, TIGHT_HOST))
                .header(header::COOKIE, format!("__Host-tool_sid={tight}"))
                .body(Body::from(vec![b'x'; 100]))
                .unwrap(),
        )
        .await;
    assert!(
        matches!(
            res.status(),
            StatusCode::PAYLOAD_TOO_LARGE | StatusCode::BAD_GATEWAY
        ),
        "streaming over-limit body is cut off: {}",
        res.status()
    );

    // Response-header timeout → 504; unreachable upstream → 502.
    let res = env
        .call(
            json(env.tool_req(TIGHT_HOST, "/t/tight/slow", &tight))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(body_json(res).await["error"], "gateway_timeout");
    let res = env
        .call(
            json(env.tool_req("dead.tools.test", "/t/dead/", &dead))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(body_json(res).await["error"], "bad_gateway");

    // Streaming: the first chunk arrives before the upstream finishes, and
    // the in-flight body holds tight's only concurrency permit.
    let res = env
        .call(
            env.tool_req(TIGHT_HOST, "/t/tight/stream", &tight)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let mut body = res.into_body();
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        http_body_util::BodyExt::frame(&mut body),
    )
    .await
    .expect("first byte before the upstream finished")
    .unwrap()
    .unwrap();
    assert_eq!(first.into_data().unwrap(), "first;");
    let busy = env
        .call(
            json(env.tool_req(TIGHT_HOST, "/t/tight/x", &tight))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy.headers()[header::RETRY_AFTER], "1");
    echo.release.notify_one();
    let rest = http_body_util::BodyExt::collect(body)
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(&rest[..], b"last");
    let res = env
        .call(
            env.tool_req(TIGHT_HOST, "/t/tight/x", &tight)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "permit released with the body"
    );
}

/// POAM-031: two gateways with different idle limits on one database never
/// expire each other's sessions early.
#[tokio::test]
async fn pg_per_session_idle_limit() {
    let Some(env_a) = setup(&[("CONSOLE_SESSION_IDLE_SECS", "150")]).await else {
        return;
    };
    let mut cfg_b = (*env_a.state.cfg).clone();
    cfg_b.session_idle = std::time::Duration::from_secs(900);
    let state_b = Arc::new(AppState {
        cfg: Arc::new(cfg_b),
        store: env_a.state.store.clone(),
        audit: env_a.state.audit.clone(),
        oidc: env_a.state.oidc.clone(),
        clock: env_a.clock.clone(),
        login_limiter: RateLimiter::per_minute(1000),
        tools: Arc::new(console_gateway::registry::ToolsState::empty()),
    });
    let env_b = Env {
        router: app(state_b.clone(), ""),
        state: state_b.clone(),
        clock: env_a.clock.clone(),
        stub: env_a.stub.clone(),
        owner: env_a.owner.clone(),
        app: env_a.app.clone(),
        audit: env_a.audit.clone(),
    };
    let (_, long) = env_b.login("/", &["console-user"], None).await;
    let long = long.unwrap();
    let (_, short) = env_a.login("/", &["console-user"], None).await;
    let short = short.unwrap();
    let (_, info) = env_a.session_json(&long).await;
    assert_eq!(
        info["idle_timeout_secs"], 900,
        "row value, not the reading gateway's"
    );
    let idle_secs: Vec<Option<i32>> =
        sqlx::query_scalar("SELECT idle_timeout_secs FROM sessions ORDER BY created_at")
            .fetch_all(&env_a.owner)
            .await
            .unwrap();
    assert_eq!(idle_secs.len(), 2);

    env_a.clock.advance(200);
    assert_eq!(
        sweeper::sweep_once(&env_a.state).await.unwrap(),
        1,
        "only the 150 s session"
    );
    assert_eq!(
        env_a.session_json(&long).await.0,
        StatusCode::OK,
        "the 900 s session survives gateway A"
    );
    assert_eq!(env_b.session_json(&short).await.0, StatusCode::UNAUTHORIZED);
    // Gateway B's sweeper also honours rows: nothing else to sweep.
    assert_eq!(sweeper::sweep_once(&state_b).await.unwrap(), 0);
    env_a.clock.advance(701);
    assert_eq!(
        env_b.session_json(&long).await.0,
        StatusCode::UNAUTHORIZED,
        "900 s limit applies"
    );
}

/// PERF-008: a slow IdP token endpoint yields a quick retryable 503; the
/// session is neither extended nor ended.
#[tokio::test]
async fn pg_refresh_timeout_is_retryable_503() {
    let Some(env) = setup(&[("CONSOLE_OIDC_REFRESH_TIMEOUT_SECS", "1")]).await else {
        return;
    };
    let (_, sid) = env.login("/", &["console-user"], None).await;
    let sid = sid.unwrap();
    let csrf = env.csrf(&sid).await;
    *env.stub.refresh.lock().unwrap() = RefreshMode::Slow(3);
    let t = std::time::Instant::now();
    let res = env
        .call(env.post(
            "/api/session/keepalive",
            &sid,
            Some(&csrf),
            Some(PUBLIC_URL),
        ))
        .await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        t.elapsed() < std::time::Duration::from_millis(2500),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(body_json(res).await["error"], "unavailable");
    assert_eq!(env.session_json(&sid).await.0, StatusCode::OK);
}
