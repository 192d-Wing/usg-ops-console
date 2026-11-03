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

// ---- M2 chrome primitives ----

fn dd_items() -> Vec<DropdownItem> {
    vec![
        DropdownItem::Link {
            id: "a".into(),
            text: "Alpha".into(),
            href: "#alpha".into(),
            description: Some("first".into()),
        },
        DropdownItem::Divider,
        DropdownItem::Group {
            text: "Group".into(),
            items: vec![DropdownItem::Action {
                id: "b".into(),
                text: "Beta".into(),
            }],
        },
        DropdownItem::Link {
            id: "c".into(),
            text: "Bad".into(),
            href: "javascript:alert(1)".into(),
            description: None,
        },
    ]
}

#[derive(Properties, PartialEq)]
struct DdP {
    hits: Rc<Cell<u32>>,
}

#[function_component]
fn Dd(p: &DdP) -> Html {
    let hits = p.hits.clone();
    let on_action = Callback::from(move |id: AttrValue| {
        assert_eq!(id.as_str(), "b");
        hits.set(hits.get() + 1);
    });
    html! {
        <>
            <button data-testid="outside">{ "outside" }</button>
            <Dropdown label="Services" items={dd_items()} {on_action} test_id="dd" />
        </>
    }
}

fn active() -> Option<Element> {
    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .active_element()
}

fn pointerdown(target: &Element) {
    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    let ev = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &init).unwrap();
    target.dispatch_event(&ev).unwrap();
}

#[wasm_bindgen_test]
async fn dropdown_keyboard_and_escape() {
    let hits = Rc::new(Cell::new(0));
    let host = mount::<Dd>(DdP { hits: hits.clone() });
    tick().await;
    let button = q(&host, "[data-testid=dd-button]");
    let panel = q(&host, "[data-testid=dd-panel]");
    assert_eq!(
        button.get_attribute("aria-expanded").as_deref(),
        Some("false")
    );
    assert_eq!(
        button.get_attribute("aria-controls"),
        panel.get_attribute("id")
    );
    assert!(panel.has_attribute("hidden"));
    assert!(panel.query_selector("a,button").unwrap().is_none());

    button.dyn_ref::<HtmlElement>().unwrap().click();
    tick().await;
    assert_eq!(
        button.get_attribute("aria-expanded").as_deref(),
        Some("true")
    );
    assert!(!panel.has_attribute("hidden"));
    let items = panel.query_selector_all(".cui-dropdown__item").unwrap();
    assert_eq!(items.length(), 3);
    let item = |i| items.item(i).unwrap().unchecked_into::<Element>();
    assert_eq!(active(), Some(item(0)), "first item focused on open");
    // Unsafe href sanitised.
    assert_eq!(item(2).get_attribute("href").as_deref(), Some("#"));

    key(&item(0), "ArrowDown");
    tick().await;
    assert_eq!(active(), Some(item(1)));
    key(&item(1), "ArrowDown");
    key(&item(2), "ArrowDown");
    tick().await;
    assert_eq!(active(), Some(item(0)), "wraps forward");
    key(&item(0), "ArrowUp");
    tick().await;
    assert_eq!(active(), Some(item(2)), "wraps backward");
    key(&item(2), "Home");
    tick().await;
    assert_eq!(active(), Some(item(0)));
    key(&item(0), "End");
    tick().await;
    assert_eq!(active(), Some(item(2)));

    key(&item(2), "Escape");
    tick().await;
    assert_eq!(
        button.get_attribute("aria-expanded").as_deref(),
        Some("false")
    );
    assert_eq!(active(), Some(button.clone()), "Escape returns focus");

    // Action item emits and closes.
    button.dyn_ref::<HtmlElement>().unwrap().click();
    tick().await;
    q(&host, "[data-testid=dd-item-b]")
        .dyn_into::<HtmlElement>()
        .unwrap()
        .click();
    tick().await;
    assert_eq!(hits.get(), 1);
    assert!(q(&host, "[data-testid=dd-panel]").has_attribute("hidden"));
}

#[wasm_bindgen_test]
async fn dropdown_outside_click_and_focus_out_close() {
    let host = mount::<Dd>(DdP {
        hits: Rc::new(Cell::new(0)),
    });
    tick().await;
    let button = q(&host, "[data-testid=dd-button]");
    let outside = q(&host, "[data-testid=outside]");
    button.dyn_ref::<HtmlElement>().unwrap().click();
    tick().await;
    // Pointerdown inside does not close.
    pointerdown(&q(&host, ".cui-dropdown__item"));
    tick().await;
    assert_eq!(
        button.get_attribute("aria-expanded").as_deref(),
        Some("true")
    );
    pointerdown(&outside);
    tick().await;
    assert_eq!(
        button.get_attribute("aria-expanded").as_deref(),
        Some("false")
    );

    // Focus moving outside (Tab) closes.
    button.dyn_ref::<HtmlElement>().unwrap().click();
    tick().await;
    assert_eq!(
        button.get_attribute("aria-expanded").as_deref(),
        Some("true")
    );
    outside.dyn_ref::<HtmlElement>().unwrap().focus().unwrap();
    tick().await;
    assert_eq!(
        button.get_attribute("aria-expanded").as_deref(),
        Some("false")
    );
}

#[wasm_bindgen_test]
async fn dropdown_listener_removed_on_unmount() {
    let doc = web_sys::window().unwrap().document().unwrap();
    let host = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&host).unwrap();
    let app = yew::Renderer::<Dd>::with_root_and_props(
        host.clone(),
        DdP {
            hits: Rc::new(Cell::new(0)),
        },
    )
    .render();
    tick().await;
    q(&host, "[data-testid=dd-button]")
        .dyn_into::<HtmlElement>()
        .unwrap()
        .click();
    tick().await;
    app.destroy();
    tick().await;
    // A listener left behind would call a dropped closure and throw here.
    pointerdown(&doc.body().unwrap().unchecked_into::<Element>());
    tick().await;
}

#[wasm_bindgen_test]
async fn nav_toggle_reports_state() {
    #[function_component]
    fn T() -> Html {
        let open = use_state(|| false);
        let on_toggle = {
            let open = open.clone();
            Callback::from(move |()| open.set(!*open))
        };
        html! { <NavToggle expanded={*open} controls="side-nav-region" {on_toggle} /> }
    }
    let host = mount::<T>(());
    tick().await;
    let b = q(&host, "[data-testid=nav-toggle]");
    assert_eq!(
        b.get_attribute("aria-controls").as_deref(),
        Some("side-nav-region")
    );
    assert_eq!(b.get_attribute("aria-label").as_deref(), Some("Navigation"));
    assert_eq!(b.get_attribute("aria-expanded").as_deref(), Some("false"));
    b.dyn_ref::<HtmlElement>().unwrap().click();
    tick().await;
    assert_eq!(b.get_attribute("aria-expanded").as_deref(), Some("true"));
}

#[wasm_bindgen_test]
async fn search_input_maxlength_and_submit() {
    #[derive(Properties, PartialEq)]
    struct P {
        got: Rc<std::cell::RefCell<String>>,
    }
    #[function_component]
    fn T(p: &P) -> Html {
        let got = p.got.clone();
        let on_submit = Callback::from(move |v: AttrValue| *got.borrow_mut() = v.to_string());
        html! { <SearchInput label="Search" max_len={8} {on_submit} test_id="s" /> }
    }
    let got = Rc::new(std::cell::RefCell::new(String::new()));
    let host = mount::<T>(P { got: got.clone() });
    tick().await;
    let form = q(&host, "[data-testid=s]");
    assert_eq!(form.get_attribute("role").as_deref(), Some("search"));
    let input = q(&host, "input")
        .dyn_into::<web_sys::HtmlInputElement>()
        .unwrap();
    assert_eq!(input.get_attribute("maxlength").as_deref(), Some("8"));
    assert_eq!(input.get_attribute("autocomplete").as_deref(), Some("off"));
    let label = q(&host, "label");
    assert_eq!(
        label.get_attribute("for").as_deref(),
        Some(input.id().as_str())
    );
    input.set_value("hello");
    let init = web_sys::EventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    let ev = web_sys::Event::new_with_event_init_dict("submit", &init).unwrap();
    form.dispatch_event(&ev).unwrap();
    tick().await;
    assert!(ev.default_prevented(), "submit must not navigate");
    assert_eq!(got.borrow().as_str(), "hello");
}

#[wasm_bindgen_test]
async fn top_navigation_slots() {
    #[function_component]
    fn T() -> Html {
        html! {
            <TopNavigation brand={html!{ <a href="/">{ "Brand" }</a> }}
                utilities={html!{ <span data-testid="u">{ "u" }</span> }} />
        }
    }
    let host = mount::<T>(());
    tick().await;
    let h = q(&host, "[data-testid=top-navigation]");
    assert_eq!(h.tag_name(), "HEADER");
    assert!(
        h.query_selector(".cui-topnav__utilities [data-testid=u]")
            .unwrap()
            .is_some()
    );
}

// F1 guard (axe `list`): every child of a side-nav list is a plain <li>.
#[wasm_bindgen_test]
async fn side_nav_lists_contain_only_listitems() {
    #[function_component]
    fn T() -> Html {
        html! {
            <SideNavigation
                header={Some(NavLink::new("Service", "/svc"))}
                items={vec![
                    SideNavItem::Link(NavLink::new("A", "/a")),
                    SideNavItem::Divider,
                    SideNavItem::Section {
                        text: "S".into(),
                        items: vec![NavLink::new("B", "/b")],
                    },
                ]}
            />
        }
    }
    let host = mount::<T>(());
    tick().await;
    let lists = host.query_selector_all("ul, ol").unwrap();
    assert!(lists.length() >= 2);
    for i in 0..lists.length() {
        let list = lists.item(i).unwrap().unchecked_into::<Element>();
        let kids = list.children();
        for j in 0..kids.length() {
            let k = kids.item(j).unwrap();
            assert_eq!(k.tag_name(), "LI", "non-li child in {}", list.class_name());
            assert!(k.get_attribute("role").is_none(), "li with role override");
        }
    }
}

// F2: a controlled <select> follows `selected` even after the user changed it
// (option `selected` attributes stop applying once selectedness is dirty).
#[wasm_bindgen_test]
async fn select_value_follows_selected_after_user_change() {
    #[derive(Properties, PartialEq)]
    struct P {
        sel: Rc<std::cell::RefCell<Option<UseStateHandle<Option<AttrValue>>>>>,
    }
    #[function_component]
    fn T(p: &P) -> Html {
        let s = use_state(|| Some(AttrValue::from("a")));
        *p.sel.borrow_mut() = Some(s.clone());
        html! {
            <Select testid="s" selected={(*s).clone()} options={vec![
                SelectOption::new("", "All"),
                SelectOption::new("a", "A"),
                SelectOption::new("b", "B"),
            ]} />
        }
    }
    let handle = Rc::new(std::cell::RefCell::new(None));
    let host = mount::<T>(P {
        sel: handle.clone(),
    });
    tick().await;
    let sel = q(&host, "[data-testid=s]").unchecked_into::<web_sys::HtmlSelectElement>();
    assert_eq!(sel.value(), "a");
    // User picks "b" (makes selectedness dirty).
    sel.set_value("b");
    let st = handle.borrow_mut().take().unwrap();
    st.set(Some("b".into()));
    tick().await;
    // Back: state returns to "a".
    let st = handle.borrow_mut().take().unwrap();
    st.set(Some("a".into()));
    tick().await;
    assert_eq!(sel.value(), "a");
    // Forward: state goes to "b".
    let st = handle.borrow_mut().take().unwrap();
    st.set(Some("b".into()));
    tick().await;
    assert_eq!(sel.value(), "b");
    // Cleared.
    let st = handle.borrow_mut().take().unwrap();
    st.set(Some("".into()));
    tick().await;
    assert_eq!(sel.value(), "");
}
