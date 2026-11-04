//! Small browser helpers shared by the hello and evil views.

use console_ui::{AlertKind, FlashId, FlashItem, flash_dismiss, flash_push};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::Response;
use yew::prelude::*;

/// `(tool id, tool-relative path)` from `/t/<id><path>`. Outside `/t/` the
/// id is empty and the path is `/`.
pub fn location_tool() -> (String, String) {
    let path = web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default();
    split_tool_path(&path)
}

pub fn split_tool_path(path: &str) -> (String, String) {
    let Some(rest) = path.strip_prefix("/t/") else {
        return (String::new(), "/".into());
    };
    let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
    (id.to_owned(), format!("/{tail}"))
}

/// Read a `Response` body as JSON, mapping non-2xx to an error string that
/// includes the gateway/tool `error` code when present.
pub async fn response_json(resp: Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let text = match resp.text() {
        Ok(p) => JsFuture::from(p)
            .await
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        let code = body["error"].as_str().unwrap_or("error");
        Err(format!("HTTP {status} ({code})"))
    }
}

/// Plain same-origin `fetch`, for code paths that must not depend on the SDK.
pub async fn fetch(req: &web_sys::Request) -> Result<Response, String> {
    let w = web_sys::window().ok_or("no window")?;
    let v = JsFuture::from(w.fetch_with_request(req))
        .await
        .map_err(js_err)?;
    v.dyn_into::<Response>()
        .map_err(|_| "not a Response".into())
}

/// Readable text for a thrown JS value (`SecurityError: ...`).
pub fn js_err(v: wasm_bindgen::JsValue) -> String {
    if let Some(e) = v.dyn_ref::<js_sys::Error>() {
        format!("{}: {}", String::from(e.name()), String::from(e.message()))
    } else {
        v.as_string().unwrap_or_else(|| format!("{v:?}"))
    }
}

/// Local flashbar state: errors are always shown to the user.
#[derive(Clone, PartialEq)]
pub struct Flash {
    items: UseStateHandle<Vec<FlashItem>>,
    next: UseStateHandle<u64>,
}

#[hook]
pub fn use_flash() -> Flash {
    Flash {
        items: use_state(Vec::new),
        next: use_state(|| 1),
    }
}

impl Flash {
    pub fn push(&self, kind: AlertKind, msg: impl Into<String>) {
        let id = *self.next;
        self.next.set(id + 1);
        let mut v = (*self.items).clone();
        flash_push(
            &mut v,
            FlashItem {
                id: FlashId(id),
                kind,
                header: None,
                content: AttrValue::from(msg.into()),
                dismissible: true,
            },
        );
        self.items.set(v);
    }

    pub fn error(&self, msg: impl Into<String>) {
        self.push(AlertKind::Error, msg);
    }

    pub fn view(&self) -> Html {
        let items = self.items.clone();
        let on_dismiss = Callback::from(move |id: FlashId| {
            let mut v = (*items).clone();
            flash_dismiss(&mut v, id);
            items.set(v);
        });
        html! {
            <div data-testid="hello-flashbar">
                <console_ui::Flashbar items={(*self.items).clone()} {on_dismiss} />
            </div>
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split_tool_path;

    #[test]
    fn splits_tool_paths() {
        assert_eq!(split_tool_path("/t/hello/"), ("hello".into(), "/".into()));
        assert_eq!(
            split_tool_path("/t/hello/headers"),
            ("hello".into(), "/headers".into())
        );
        assert_eq!(split_tool_path("/t/evil"), ("evil".into(), "/".into()));
        assert_eq!(split_tool_path("/x"), (String::new(), "/".into()));
    }
}
