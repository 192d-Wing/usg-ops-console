//! Same-origin fetch to the gateway (`credentials: same-origin`; the session
//! is the `__Host-` cookie, never a token in JS storage).

use crate::session::{LoginNotice, LogoutResponse, SessionInfo, login_url};
use console_protocol::session::SessionUser;
use console_protocol::{LaunchResponse, ToolNavItem, ToolSummary, ToolsResponse};
use wasm_bindgen_futures::JsFuture;
use web_sys::wasm_bindgen::{JsCast, JsValue};
use web_sys::{Request, RequestCredentials, RequestInit, Response};

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// 401: the shell is already navigating to the login page.
    Unauthenticated,
    /// Any other non-2xx status (403 CSRF included).
    Status(u16),
    Network,
    Decode,
}

impl ApiError {
    /// User-facing text for the flashbar; `None` when nothing should be shown.
    pub fn message(&self, what: &str) -> Option<String> {
        match self {
            ApiError::Unauthenticated => None,
            ApiError::Status(403) => Some(format!("{what}: request was refused (403)")),
            ApiError::Status(503) => Some(format!(
                "{what}: the console or sign-in service is temporarily unavailable (503)"
            )),
            ApiError::Status(s) => Some(format!("{what}: server error ({s})")),
            ApiError::Network => Some(format!("{what}: the console server is unreachable")),
            ApiError::Decode => Some(format!("{what}: unexpected response from the server")),
        }
    }
}

/// Navigate to `/login?return_to=<current path+query>`, built only from
/// `window.location` (the 401 body's `login_url` is ignored). With a notice,
/// adds `reason=expired|ended`: the gateway then forces a fresh sign-in and
/// appends `console_notice=` to `return_to` (CONTRACT DELTA 8). Uses
/// `location.replace` so Back does not return to the dead session's page.
pub fn redirect_to_login(notice: Option<LoginNotice>) {
    let Some(win) = web_sys::window() else { return };
    let loc = win.location();
    let url = login_url(
        &loc.pathname().unwrap_or_else(|_| "/".into()),
        &loc.search().unwrap_or_default(),
        notice,
    );
    if loc.replace(&url).is_err() {
        let _ = loc.assign(&url);
    }
}

/// Which request got the 401: decides what the user is told afterwards.
#[derive(Clone, Copy, PartialEq, Eq)]
enum On401 {
    /// Page load / tab focus: an expired session is explained, a missing
    /// one (first visit, signed out elsewhere) is not.
    Load,
    /// "Stay signed in": any 401 means the session could not be kept
    /// (revoked at the IdP, de-roled, or expired); NIST-001.
    Keepalive,
}

/// Notice for a 401 from the advisory `reason` in the body (UI copy only).
fn notice_for(on: On401, reason: Option<&str>) -> Option<LoginNotice> {
    match (reason, on) {
        (Some("idle_timeout" | "absolute_timeout"), _) => Some(LoginNotice::Expired),
        (Some("revoked"), _) => Some(LoginNotice::Ended),
        (_, On401::Keepalive) => Some(LoginNotice::Ended),
        (_, On401::Load) => None,
    }
}

/// Decodes with the browser's `JSON` and reads fields via `Reflect` instead
/// of serde_json, which would add about 30 KB gz to the bundle.
trait FromJs: Sized {
    fn from_js(v: &JsValue) -> Option<Self>;
}

fn field(v: &JsValue, k: &str) -> Option<JsValue> {
    js_sys::Reflect::get(v, &JsValue::from_str(k))
        .ok()
        .filter(|f| !f.is_undefined() && !f.is_null())
}

fn string(v: &JsValue, k: &str) -> Option<String> {
    field(v, k)?.as_string()
}

impl FromJs for SessionInfo {
    fn from_js(v: &JsValue) -> Option<Self> {
        let u = field(v, "user")?;
        let roles = match field(v, "roles") {
            None => Vec::new(),
            // `Array.from` would turn a string into its characters (INT-002).
            Some(r) if !js_sys::Array::is_array(&r) => return None,
            Some(r) => js_sys::Array::from(&r)
                .iter()
                .map(|x| x.as_string())
                .collect::<Option<Vec<_>>>()?,
        };
        let idle = field(v, "idle_timeout_secs")?.as_f64()?;
        Some(SessionInfo {
            user: SessionUser {
                id: string(&u, "id")?,
                username: string(&u, "username")?,
                display_name: string(&u, "display_name")?,
            },
            roles,
            idle_expires_at: string(v, "idle_expires_at")?,
            absolute_expires_at: string(v, "absolute_expires_at")?,
            idle_timeout_secs: (idle >= 0.0 && idle.fract() == 0.0).then_some(idle as u64)?,
            server_time: string(v, "server_time")?,
            csrf_token: string(v, "csrf_token")?,
        })
    }
}

fn string_array(v: &JsValue, k: &str) -> Option<Vec<String>> {
    match field(v, k) {
        None => Some(Vec::new()),
        Some(a) if !js_sys::Array::is_array(&a) => None,
        // Non-string members are skipped (lenient, like serde's default).
        Some(a) => Some(
            js_sys::Array::from(&a)
                .iter()
                .filter_map(|x| x.as_string())
                .collect(),
        ),
    }
}

impl FromJs for ToolSummary {
    fn from_js(v: &JsValue) -> Option<Self> {
        let nav = match field(v, "nav") {
            None => Vec::new(),
            Some(a) if !js_sys::Array::is_array(&a) => return None,
            Some(a) => js_sys::Array::from(&a)
                .iter()
                .filter_map(|n| {
                    Some(ToolNavItem {
                        label: string(&n, "label")?,
                        path: string(&n, "path")?,
                    })
                })
                .collect(),
        };
        Some(ToolSummary {
            id: string(v, "id")?,
            title: string(v, "title")?,
            category: string(v, "category"),
            icon: string(v, "icon"),
            origin: string(v, "origin")?,
            entry: string(v, "entry").unwrap_or_else(|| "/".to_owned()),
            nav,
            scopes: string_array(v, "scopes")?,
        })
    }
}

impl FromJs for ToolsResponse {
    fn from_js(v: &JsValue) -> Option<Self> {
        let tools = match field(v, "tools") {
            None => Vec::new(),
            Some(a) if !js_sys::Array::is_array(&a) => return None,
            // One bad entry is dropped instead of failing the whole menu.
            Some(a) => js_sys::Array::from(&a)
                .iter()
                .filter_map(|t| ToolSummary::from_js(&t))
                .collect(),
        };
        Some(ToolsResponse { tools })
    }
}

impl FromJs for LaunchResponse {
    fn from_js(v: &JsValue) -> Option<Self> {
        Some(LaunchResponse {
            url: string(v, "url")?,
        })
    }
}

impl FromJs for LogoutResponse {
    fn from_js(v: &JsValue) -> Option<Self> {
        Some(LogoutResponse {
            redirect: string(v, "redirect")?,
            end_session_origin: string(v, "end_session_origin"),
        })
    }
}

async fn fetch_json<T: FromJs>(
    method: &str,
    path: &str,
    csrf: Option<&str>,
    body: Option<&str>,
    on401: On401,
) -> Result<T, ApiError> {
    let win = web_sys::window().ok_or(ApiError::Network)?;
    let init = RequestInit::new();
    init.set_method(method);
    init.set_credentials(RequestCredentials::SameOrigin);
    if let Some(b) = body {
        init.set_body(&JsValue::from_str(b));
    }
    let req = Request::new_with_str_and_init(path, &init).map_err(|_| ApiError::Network)?;
    let h = req.headers();
    h.set("Accept", "application/json")
        .map_err(|_| ApiError::Network)?;
    if body.is_some() {
        h.set("Content-Type", "application/json")
            .map_err(|_| ApiError::Network)?;
    }
    if let Some(t) = csrf {
        h.set("X-CSRF-Token", t).map_err(|_| ApiError::Network)?;
    }
    let resp: Response = JsFuture::from(win.fetch_with_request(&req))
        .await
        .map_err(|_| ApiError::Network)?
        .dyn_into()
        .map_err(|_: JsValue| ApiError::Network)?;
    let status = resp.status();
    if status == 401 {
        let reason = match resp.json() {
            Ok(p) => JsFuture::from(p)
                .await
                .ok()
                .and_then(|v| string(&v, "reason")),
            Err(_) => None,
        };
        redirect_to_login(notice_for(on401, reason.as_deref()));
        return Err(ApiError::Unauthenticated);
    }
    if !resp.ok() {
        return Err(ApiError::Status(status));
    }
    let v = JsFuture::from(resp.json().map_err(|_| ApiError::Decode)?)
        .await
        .map_err(|_| ApiError::Decode)?;
    T::from_js(&v).ok_or(ApiError::Decode)
}

pub async fn get_session() -> Result<SessionInfo, ApiError> {
    fetch_json("GET", "/api/session", None, None, On401::Load).await
}

pub async fn keepalive(csrf: &str) -> Result<SessionInfo, ApiError> {
    fetch_json(
        "POST",
        "/api/session/keepalive",
        Some(csrf),
        None,
        On401::Keepalive,
    )
    .await
}

pub async fn logout(csrf: &str) -> Result<LogoutResponse, ApiError> {
    fetch_json("POST", "/logout", Some(csrf), None, On401::Load).await
}

/// `GET /api/tools`: the registry view, already filtered by role on the
/// server (POAM-010). Entries that fail [`crate::tools::accept_tool`] are
/// dropped, not fatal.
pub async fn get_tools() -> Result<ToolsResponse, ApiError> {
    fetch_json(
        "GET",
        console_protocol::tools::ROUTE_TOOLS,
        None,
        None,
        On401::Load,
    )
    .await
}

/// `POST /api/tools/{id}/launch` with `{"path": ..}` (`{}` = registry entry).
/// 403 is a role denial and 404 an unknown tool (`ApiError::Status`).
pub async fn launch_tool(
    id: &str,
    path: Option<&str>,
    csrf: &str,
) -> Result<LaunchResponse, ApiError> {
    // `JSON.stringify` keeps serde_json out of the request path.
    let obj = js_sys::Object::new();
    if let Some(p) = path {
        js_sys::Reflect::set(&obj, &JsValue::from_str("path"), &JsValue::from_str(p))
            .map_err(|_| ApiError::Network)?;
    }
    let body: String = js_sys::JSON::stringify(&obj)
        .map_err(|_| ApiError::Network)?
        .into();
    fetch_json(
        "POST",
        &console_protocol::tools::launch_route(id),
        Some(csrf),
        Some(&body),
        On401::Load,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_from_401_reason() {
        assert_eq!(
            notice_for(On401::Load, Some("idle_timeout")),
            Some(LoginNotice::Expired)
        );
        assert_eq!(
            notice_for(On401::Load, Some("absolute_timeout")),
            Some(LoginNotice::Expired)
        );
        assert_eq!(notice_for(On401::Load, Some("no_session")), None);
        assert_eq!(notice_for(On401::Load, None), None);
        assert_eq!(
            notice_for(On401::Load, Some("revoked")),
            Some(LoginNotice::Ended)
        );
        assert_eq!(
            notice_for(On401::Keepalive, Some("no_session")),
            Some(LoginNotice::Ended)
        );
        assert_eq!(
            notice_for(On401::Keepalive, Some("revoked")),
            Some(LoginNotice::Ended)
        );
        assert_eq!(
            notice_for(On401::Keepalive, Some("idle_timeout")),
            Some(LoginNotice::Expired)
        );
    }
}

/// INT-001: the shipped `FromJs` decoder against the shared v1 fixture,
/// compared with the serde (gateway/protocol) parse. `wasm-pack test
/// --headless --chrome crates/console-shell`.
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    const FIXTURE: &str = include_str!("../../console-gateway/tests/fixtures/session_v1.json");

    fn parse(json: &str) -> JsValue {
        js_sys::JSON::parse(json).expect("valid JSON")
    }

    #[wasm_bindgen_test]
    fn fixture_decodes_like_serde() {
        let want: SessionInfo = serde_json::from_str(FIXTURE).expect("serde");
        let got = SessionInfo::from_js(&parse(FIXTURE)).expect("FromJs");
        assert_eq!(got, want);
    }

    #[wasm_bindgen_test]
    fn roles_must_be_an_array() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        v["roles"] = serde_json::json!("console-user");
        assert_eq!(SessionInfo::from_js(&parse(&v.to_string())), None);
        v["roles"] = serde_json::json!({"0": "console-user"});
        assert_eq!(SessionInfo::from_js(&parse(&v.to_string())), None);
        v["roles"] = serde_json::json!(["a", 1]);
        assert_eq!(SessionInfo::from_js(&parse(&v.to_string())), None);
    }

    #[wasm_bindgen_test]
    fn logout_response_decodes_like_serde() {
        for raw in [
            r#"{"redirect":"https://kc.example/logout","end_session_origin":"https://kc.example"}"#,
            r#"{"redirect":"https://console.example/logged-out"}"#,
        ] {
            let want: LogoutResponse = serde_json::from_str(raw).unwrap();
            assert_eq!(LogoutResponse::from_js(&parse(raw)), Some(want));
        }
    }

    const TOOLS_FIXTURES: &[&str] = &[
        include_str!("../../console-protocol/tests/fixtures/tools_v1/tools_response.json"),
        r#"{"tools":[{"id":"a","title":"A","origin":"https://a.t","future":1}],"next":2}"#,
    ];

    #[wasm_bindgen_test]
    fn tools_fixtures_decode_like_serde() {
        for raw in TOOLS_FIXTURES {
            let want: ToolsResponse = serde_json::from_str(raw).unwrap();
            assert_eq!(ToolsResponse::from_js(&parse(raw)), Some(want));
        }
        let raw =
            include_str!("../../console-protocol/tests/fixtures/tools_v1/launch_response.json");
        let want: LaunchResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(LaunchResponse::from_js(&parse(raw)), Some(want));
    }

    #[wasm_bindgen_test]
    fn bad_tool_entry_is_dropped_not_fatal() {
        let raw = r#"{"tools":[{"id":"a","title":"A","origin":"https://a.example"},{"id":"b"}]}"#;
        let got = ToolsResponse::from_js(&parse(raw)).unwrap();
        assert_eq!(got.tools.len(), 1);
        assert_eq!(ToolsResponse::from_js(&parse(r#"{"tools":"x"}"#)), None);
    }

    #[wasm_bindgen_test]
    fn missing_field_is_rejected() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        v.as_object_mut().unwrap().remove("csrf_token");
        assert_eq!(SessionInfo::from_js(&parse(&v.to_string())), None);
    }
}
