//! Browser tests: `wasm-pack test --headless --firefox crates/console-ui`.
#![cfg(target_arch = "wasm32")]

use console_ui::*;
use std::cell::Cell;
use std::rc::Rc;
use wasm_bindgen_test::*;
use web_sys::wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, KeyboardEvent, KeyboardEventInit};
use yew::prelude::*;

wasm_bindgen_test_configure!(run_in_browser);

fn mount<C: BaseComponent>(props: C::Properties) -> Element {
    let doc = web_sys::window().unwrap().document().unwrap();
    let host = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&host).unwrap();
    yew::Renderer::<C>::with_root_and_props(host.clone(), props).render();
    host
}

async fn tick() {
    yew::platform::time::sleep(std::time::Duration::from_millis(20)).await;
}

fn q(root: &Element, sel: &str) -> Element {
    root.query_selector(sel)
        .unwrap()
        .unwrap_or_else(|| panic!("missing {sel}"))
}

fn key(target: &Element, k: &str) {
    let init = KeyboardEventInit::new();
    init.set_key(k);
    init.set_bubbles(true);
    init.set_cancelable(true);
    let ev = KeyboardEvent::new_with_keyboard_event_init_dict("keydown", &init).unwrap();
    target.dispatch_event(&ev).unwrap();
}

#[wasm_bindgen_test]
async fn button_loading_is_busy_and_disabled() {
    #[function_component]
    fn T() -> Html {
        html! { <Button loading=true>{ "Save" }</Button> }
    }
    let host = mount::<T>(());
    tick().await;
    let b = q(&host, "button");
    assert_eq!(b.get_attribute("aria-busy").as_deref(), Some("true"));
    assert!(b.has_attribute("disabled"));
}

#[wasm_bindgen_test]
async fn form_field_wires_describedby() {
    #[function_component]
    fn T() -> Html {
        html! {
            <FormField label="Name" description="Help" error="Required"
                children={Callback::from(|ids: FieldIds| html! {
                    <Input value="" id={ids.control_id} aria_describedby={ids.describedby} invalid={ids.invalid} />
                })} />
        }
    }
    let host = mount::<T>(());
    tick().await;
    let input = q(&host, "input");
    let by = input.get_attribute("aria-describedby").unwrap();
    for id in by.split(' ') {
        assert!(
            host.query_selector(&format!("#{id}")).unwrap().is_some(),
            "{id}"
        );
    }
    assert_eq!(by.split(' ').count(), 2);
    let label = q(&host, "label");
    assert_eq!(label.get_attribute("for"), input.get_attribute("id"));
    assert_eq!(input.get_attribute("aria-invalid").as_deref(), Some("true"));
}

#[wasm_bindgen_test]
async fn tabs_arrow_moves_selection_and_focus() {
    #[function_component]
    fn T() -> Html {
        let active = use_state(|| AttrValue::from("a"));
        let tabs = vec![
            TabDef::new("a", "A", html! {"a"}),
            TabDef::new("b", "B", html! {"b"}),
        ];
        let onchange = {
            let active = active.clone();
            Callback::from(move |id| active.set(id))
        };
        html! { <Tabs {tabs} active={(*active).clone()} {onchange} /> }
    }
    let host = mount::<T>(());
    tick().await;
    let first = q(&host, "[data-testid=tab-a]");
    key(&first, "ArrowRight");
    tick().await;
    let second = q(&host, "[data-testid=tab-b]");
    assert_eq!(
        second.get_attribute("aria-selected").as_deref(),
        Some("true")
    );
    assert_eq!(second.get_attribute("tabindex").as_deref(), Some("0"));
    let doc = web_sys::window().unwrap().document().unwrap();
    assert_eq!(doc.active_element(), Some(second.clone()));
    key(&second, "ArrowRight");
    tick().await;
    assert_eq!(
        q(&host, "[data-testid=tab-a]")
            .get_attribute("aria-selected")
            .as_deref(),
        Some("true")
    );
}

#[wasm_bindgen_test]
async fn modal_escape_dismisses_and_returns_focus() {
    #[function_component]
    fn T() -> Html {
        let open = use_state(|| false);
        let show = {
            let open = open.clone();
            Callback::from(move |_| open.set(true))
        };
        let hide = {
            let open = open.clone();
            Callback::from(move |_| open.set(false))
        };
        html! {
            <>
                <Button testid="opener" onclick={show}>{ "Open" }</Button>
                <Modal visible={*open} header="Hi" on_dismiss={hide}>{ "Body" }</Modal>
            </>
        }
    }
    let host = mount::<T>(());
    tick().await;
    let opener = q(&host, "[data-testid=opener]")
        .dyn_into::<HtmlElement>()
        .unwrap();
    opener.focus().unwrap();
    opener.click();
    tick().await;
    let dialog = q(&host, "[role=dialog]");
    let doc = web_sys::window().unwrap().document().unwrap();
    assert_eq!(doc.active_element(), Some(dialog.clone()));
    key(&dialog, "Escape");
    tick().await;
    assert!(host.query_selector("[role=dialog]").unwrap().is_none());
    assert_eq!(
        doc.active_element(),
        Some(opener.unchecked_into::<Element>())
    );
}

#[wasm_bindgen_test]
async fn toggle_reports_aria_checked() {
    #[derive(Properties, PartialEq)]
    struct P {
        hits: Rc<Cell<u32>>,
    }
    #[function_component]
    fn T(p: &P) -> Html {
        let on = use_state(|| false);
        let hits = p.hits.clone();
        let onchange = {
            let on = on.clone();
            Callback::from(move |v| {
                hits.set(hits.get() + 1);
                on.set(v)
            })
        };
        html! { <Toggle checked={*on} {onchange}>{ "Dark" }</Toggle> }
    }
    let hits = Rc::new(Cell::new(0));
    let host = mount::<T>(P { hits: hits.clone() });
    tick().await;
    let sw = q(&host, "[role=switch]");
    assert_eq!(sw.get_attribute("aria-checked").as_deref(), Some("false"));
    sw.dyn_ref::<HtmlElement>().unwrap().click();
    tick().await;
    assert_eq!(
        q(&host, "[role=switch]")
            .get_attribute("aria-checked")
            .as_deref(),
        Some("true")
    );
    assert_eq!(hits.get(), 1);
}

async fn open_then_escape(host: &Element, chrome_id: &str) {
    let opener = q(host, "[data-testid=opener]")
        .dyn_into::<HtmlElement>()
        .unwrap();
    opener.focus().unwrap();
    opener.click();
    tick().await;
    let doc = web_sys::window().unwrap().document().unwrap();
    let chrome = doc.get_element_by_id(chrome_id).unwrap();
    assert!(chrome.has_attribute("inert"), "chrome inert while open");
    let dialog = q(host, "[role=dialog]");
    key(&dialog, "Escape");
    tick().await;
    assert!(host.query_selector("[role=dialog]").unwrap().is_none());
    assert!(!chrome.has_attribute("inert"), "inert cleared on close");
    assert_eq!(
        doc.active_element(),
        Some(opener.unchecked_into::<Element>()),
        "focus returned to opener"
    );
}

/// ORCH-1: with `inert_root`, the Modal clears inert before restoring focus.
#[wasm_bindgen_test]
async fn modal_inert_root_returns_focus_to_opener() {
    #[function_component]
    fn T() -> Html {
        let open = use_state(|| false);
        let show = {
            let open = open.clone();
            Callback::from(move |_| open.set(true))
        };
        let hide = {
            let open = open.clone();
            Callback::from(move |_| open.set(false))
        };
        html! {
            <>
                <div id="t-chrome-a">
                    <Button testid="opener" onclick={show}>{ "Open" }</Button>
                </div>
                <Modal visible={*open} header="Hi" on_dismiss={hide}
                    inert_root={AttrValue::Static("t-chrome-a")}>{ "Body" }</Modal>
            </>
        }
    }
    let host = mount::<T>(());
    tick().await;
    open_then_escape(&host, "t-chrome-a").await;
}

/// ORCH-1: if a parent manages inert in its own effect (cleanup order not
/// guaranteed), the Modal still restores focus once inert is cleared.
#[wasm_bindgen_test]
async fn modal_parent_managed_inert_returns_focus_to_opener() {
    fn set(on: bool) {
        let el = web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("t-chrome-b")
            .unwrap();
        if on {
            el.set_attribute("inert", "").unwrap();
        } else {
            el.remove_attribute("inert").unwrap();
        }
    }
    #[function_component]
    fn T() -> Html {
        let open = use_state(|| false);
        use_effect_with(*open, |o| {
            set(*o);
            || set(false)
        });
        let show = {
            let open = open.clone();
            Callback::from(move |_| open.set(true))
        };
        let hide = {
            let open = open.clone();
            Callback::from(move |_| open.set(false))
        };
        html! {
            <>
                <div id="t-chrome-b">
                    <Button testid="opener" onclick={show}>{ "Open" }</Button>
                </div>
                <Modal visible={*open} header="Hi" on_dismiss={hide}>{ "Body" }</Modal>
            </>
        }
    }
    let host = mount::<T>(());
    tick().await;
    open_then_escape(&host, "t-chrome-b").await;
}

/// A11Y-002: dismissing a flash moves focus to the remaining flash's button.
#[wasm_bindgen_test]
async fn flash_dismiss_moves_focus_to_sibling() {
    #[function_component]
    fn T() -> Html {
        let items = use_state(|| {
            (1..=2)
                .map(|i| FlashItem {
                    id: FlashId(i),
                    kind: AlertKind::Info,
                    header: None,
                    content: AttrValue::Static("x"),
                    dismissible: true,
                })
                .collect::<Vec<_>>()
        });
        let on_dismiss = {
            let items = items.clone();
            Callback::from(move |id| {
                let mut v = (*items).clone();
                flash_dismiss(&mut v, id);
                items.set(v);
            })
        };
        html! { <Flashbar items={(*items).clone()} {on_dismiss} /> }
    }
    let host = mount::<T>(());
    tick().await;
    let first = q(&host, ".cui-flash__dismiss")
        .dyn_into::<HtmlElement>()
        .unwrap();
    first.focus().unwrap();
    first.click();
    tick().await;
    let rest = host.query_selector_all(".cui-flash__dismiss").unwrap();
    assert_eq!(rest.length(), 1);
    let doc = web_sys::window().unwrap().document().unwrap();
    assert_eq!(
        doc.active_element()
            .map(|e| e.unchecked_into::<web_sys::Node>()),
        rest.item(0)
    );
}
