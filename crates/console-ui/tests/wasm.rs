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
        DropdownItem::Note {
            id: "n".into(),
            text: "Nothing else".into(),
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
    assert_eq!(items.length(), 3, "Note is not an interactive item");
    let note = q(&host, "[data-testid=dd-item-n]");
    assert_eq!(note.tag_name(), "SPAN");
    assert!(!note.has_attribute("tabindex"));
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
#[allow(deprecated)] // SearchInput is deprecated (INT-004) but still shipped.
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

/// A11Y-1/NIST-003: `dismiss_on_overlay=false` ignores backdrop clicks;
/// `close_button=false` renders no X; Esc still dismisses.
#[wasm_bindgen_test]
async fn modal_overlay_click_can_be_disabled() {
    thread_local! { static DISMISSED: Cell<u32> = const { Cell::new(0) }; }
    #[function_component]
    fn T() -> Html {
        let on_dismiss = Callback::from(|_| DISMISSED.with(|d| d.set(d.get() + 1)));
        html! {
            <Modal visible=true header="Hi" {on_dismiss} dismiss_on_overlay=false
                close_button=false>{ "Body" }</Modal>
        }
    }
    let host = mount::<T>(());
    tick().await;
    q(&host, ".cui-modal__overlay")
        .dyn_into::<HtmlElement>()
        .unwrap()
        .click();
    tick().await;
    assert_eq!(DISMISSED.with(Cell::get), 0);
    assert!(host.query_selector(".cui-icon-btn").unwrap().is_none());
    key(&q(&host, "[role=dialog]"), "Escape");
    tick().await;
    assert_eq!(DISMISSED.with(Cell::get), 1);
}

/// Default Modal: overlay click dismisses and the X has the given name.
#[wasm_bindgen_test]
async fn modal_overlay_click_dismisses_by_default() {
    thread_local! { static DISMISSED: Cell<u32> = const { Cell::new(0) }; }
    #[function_component]
    fn T() -> Html {
        let on_dismiss = Callback::from(|_| DISMISSED.with(|d| d.set(d.get() + 1)));
        html! {
            <Modal visible=true header="Hi" {on_dismiss}
                close_label={AttrValue::Static("Stay signed in")}>{ "Body" }</Modal>
        }
    }
    let host = mount::<T>(());
    tick().await;
    assert_eq!(
        q(&host, ".cui-icon-btn")
            .get_attribute("aria-label")
            .as_deref(),
        Some("Stay signed in")
    );
    q(&host, ".cui-modal__overlay")
        .dyn_into::<HtmlElement>()
        .unwrap()
        .click();
    tick().await;
    assert_eq!(DISMISSED.with(Cell::get), 1);
}

/// A11Y-2: `keep_focus` buttons stay focusable while loading and ignore clicks.
#[wasm_bindgen_test]
async fn button_keep_focus_loading_stays_focusable() {
    thread_local! { static CLICKS: Cell<u32> = const { Cell::new(0) }; }
    #[function_component]
    fn T() -> Html {
        let onclick = Callback::from(|_: MouseEvent| CLICKS.with(|c| c.set(c.get() + 1)));
        html! { <Button loading=true keep_focus=true {onclick}>{ "Stay" }</Button> }
    }
    let host = mount::<T>(());
    tick().await;
    let b = q(&host, "button").dyn_into::<HtmlElement>().unwrap();
    assert!(!b.has_attribute("disabled"));
    assert_eq!(b.get_attribute("aria-disabled").as_deref(), Some("true"));
    assert_eq!(b.get_attribute("aria-busy").as_deref(), Some("true"));
    b.focus().unwrap();
    let doc = web_sys::window().unwrap().document().unwrap();
    assert_eq!(
        doc.active_element(),
        Some(b.clone().unchecked_into::<Element>())
    );
    b.click();
    tick().await;
    assert_eq!(CLICKS.with(Cell::get), 0);
}

// ---- M4.5: icon, rail, palette, popover, top bar ----

fn input_text(el: &Element, v: &str) {
    let i: web_sys::HtmlInputElement = el.clone().dyn_into().unwrap();
    i.set_value(v);
    let init = web_sys::EventInit::new();
    init.set_bubbles(true);
    // Yew's oninput only reads the target, so a plain bubbling Event works.
    let ev = web_sys::Event::new_with_event_init_dict("input", &init).unwrap();
    i.dispatch_event(&ev).unwrap();
}

#[wasm_bindgen_test]
async fn icon_is_decorative_svg() {
    #[function_component]
    fn T() -> Html {
        html! { <Icon name={IconName::Home} /> }
    }
    let host = mount::<T>(());
    tick().await;
    let s = q(&host, "svg.cui-icon");
    assert_eq!(s.get_attribute("aria-hidden").as_deref(), Some("true"));
    assert_eq!(s.get_attribute("focusable").as_deref(), Some("false"));
    assert!(!s.has_attribute("style"));
    assert_eq!(
        s.namespace_uri().as_deref(),
        Some("http://www.w3.org/2000/svg")
    );
}

#[wasm_bindgen_test]
async fn rail_collapsed_keeps_text_names() {
    #[function_component]
    fn T() -> Html {
        html! {
            <Rail collapsed=true active_href={Some(AttrValue::from("/"))} items={vec![
                RailEntry::Link(RailItem::new("Overview", "/").icon(IconName::Home).testid("rail-item-home")),
                RailEntry::Section { text: "Tools".into(), items: vec![
                    RailItem::new("Hello tool", "/t/hello/").testid("rail-item-tool-hello"),
                ]},
            ]} />
        }
    }
    let host = mount::<T>(());
    tick().await;
    let nav = q(&host, "nav[data-testid=side-navigation]");
    assert_eq!(nav.get_attribute("aria-label").as_deref(), Some("Console"));
    assert!(
        nav.get_attribute("class")
            .unwrap_or_default()
            .split_whitespace()
            .any(|c| c == "cui-rail--collapsed")
    );
    let home = q(&host, "[data-testid=rail-item-home]");
    assert_eq!(
        q(&home, ".cui-rail__label").text_content().unwrap().trim(),
        "Overview"
    );
    // Tooltip is a real element, hidden from the accessibility tree, so the
    // name stays exactly the label (A11Y-001/005).
    let tip = q(&home, ".cui-rail__tip");
    assert_eq!(tip.text_content().as_deref(), Some("Overview"));
    assert_eq!(tip.get_attribute("aria-hidden").as_deref(), Some("true"));
    // Escape anywhere dismisses the tooltip; pointer leave restores it.
    // R2-D1: the real stylesheet must hide the tip after Escape. Pointer
    // hover and :focus-visible cannot be forced from script, so the shipped
    // CSS is injected with those pseudo-classes mapped to equal-specificity
    // test classes, and computed style is asserted.
    let doc = web_sys::window().unwrap().document().unwrap();
    let style = doc.create_element("style").unwrap();
    style.set_text_content(Some(
        &include_str!("../assets/components.css")
            .replace(":hover", ".t-hover")
            .replace(":focus-visible", ".t-focus"),
    ));
    doc.head().unwrap().append_child(&style).unwrap();
    let shown = || {
        let cs = web_sys::window()
            .unwrap()
            .get_computed_style(&tip)
            .unwrap()
            .unwrap();
        cs.get_property_value("visibility").unwrap() == "visible"
            && cs.get_property_value("display").unwrap() != "none"
    };
    for state in ["t-hover", "t-focus"] {
        assert!(!shown(), "tip hidden at rest");
        home.class_list().add_1(state).unwrap();
        assert!(shown(), "tip shown on {state}");
        key(&host, "Escape");
        tick().await;
        assert!(nav.has_attribute("data-tip-dismissed"));
        assert!(!shown(), "Escape hides tip on {state}");
        home.class_list().remove_1(state).unwrap();
        home.dispatch_event(&web_sys::MouseEvent::new("mouseleave").unwrap())
            .unwrap();
        tick().await;
        assert!(!nav.has_attribute("data-tip-dismissed"));
    }
    style.remove();
    key(&host, "Escape");
    tick().await;
    assert!(nav.has_attribute("data-tip-dismissed"));
    home.dispatch_event(&web_sys::MouseEvent::new("mouseleave").unwrap())
        .unwrap();
    tick().await;
    assert!(!nav.has_attribute("data-tip-dismissed"));
    assert_eq!(home.get_attribute("aria-current").as_deref(), Some("page"));
    assert_eq!(
        home.get_attribute("data-label").as_deref(),
        Some("Overview")
    );
    assert!(home.get_attribute("aria-label").is_none());
    assert!(
        q(&home, ".cui-rail__label")
            .get_attribute("class")
            .unwrap_or_default()
            .split_whitespace()
            .any(|c| c == "cui-visually-hidden")
    );
    let tool = q(&host, "[data-testid=rail-item-tool-hello]");
    let mono = q(&tool, ".cui-rail__monogram");
    assert_eq!(mono.text_content().as_deref(), Some("HT"));
    assert_eq!(mono.get_attribute("aria-hidden").as_deref(), Some("true"));
    // Every child of a rail list is an <li> (axe `list`).
    let kids = host.query_selector_all(".cui-rail__list > *").unwrap();
    for i in 0..kids.length() {
        let el: Element = kids.item(i).unwrap().dyn_into().unwrap();
        assert_eq!(el.tag_name(), "LI");
    }
}

#[derive(Properties, PartialEq)]
struct PalHostProps {
    picked: Rc<std::cell::RefCell<Vec<String>>>,
}

#[function_component]
fn PalHost(p: &PalHostProps) -> Html {
    let open = use_state(|| false);
    let items = use_memo((), |_| {
        vec![
            PaletteItem::new("home", "Overview", PaletteGroup::Navigation),
            PaletteItem::new("tools-loading", "Tools are loading", PaletteGroup::Tools)
                .disabled(true),
            PaletteItem::new("theme-dark", "Theme: dark", PaletteGroup::Preferences),
            PaletteItem::new("signout", "Sign out", PaletteGroup::Account),
        ]
    });
    let on_select = {
        let open = open.clone();
        let picked = p.picked.clone();
        Callback::from(move |id: AttrValue| {
            picked.borrow_mut().push(id.to_string());
            open.set(false);
        })
    };
    let on_close = {
        let open = open.clone();
        Callback::from(move |_| open.set(false))
    };
    let onclick = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(true))
    };
    html! {
        <>
            <button id="pal-trigger" data-testid="palette-trigger" {onclick}>{ "Search…" }</button>
            <CommandPalette open={*open} items={Rc::new((*items).clone())} {on_select} {on_close} />
        </>
    }
}

#[wasm_bindgen_test]
async fn palette_combobox_navigation_and_focus_return() {
    let picked = Rc::new(std::cell::RefCell::new(Vec::new()));
    let host = mount::<PalHost>(PalHostProps {
        picked: picked.clone(),
    });
    tick().await;
    let trigger: HtmlElement = q(&host, "#pal-trigger").dyn_into().unwrap();
    trigger.focus().unwrap();
    trigger.click();
    tick().await;
    let dlg = q(&host, "[data-testid=command-palette] [role=dialog]");
    assert_eq!(dlg.get_attribute("aria-modal").as_deref(), Some("true"));
    let input = q(&host, "[data-testid=palette-input]");
    assert_eq!(active().as_ref(), Some(&input), "input focused on open");
    assert_eq!(input.get_attribute("role").as_deref(), Some("combobox"));
    assert_eq!(
        input.get_attribute("aria-controls").as_deref(),
        Some("palette-list")
    );
    assert_eq!(input.get_attribute("maxlength").as_deref(), Some("256"));
    assert_eq!(
        q(&host, "#palette-list").get_attribute("role").as_deref(),
        Some("listbox")
    );
    assert!(input.get_attribute("aria-activedescendant").is_none());

    key(&input, "ArrowDown");
    tick().await;
    let a1 = input
        .get_attribute("aria-activedescendant")
        .expect("active set");
    let opt = q(&host, &format!("#{a1}"));
    assert_eq!(
        opt.get_attribute("data-testid").as_deref(),
        Some("palette-option-home")
    );
    assert_eq!(opt.get_attribute("aria-selected").as_deref(), Some("true"));
    // Disabled row is skipped.
    key(&input, "ArrowDown");
    tick().await;
    let a2 = input.get_attribute("aria-activedescendant").unwrap();
    assert_eq!(
        q(&host, &format!("#{a2}"))
            .get_attribute("data-testid")
            .as_deref(),
        Some("palette-option-theme-dark")
    );
    key(&input, "Enter");
    tick().await;
    assert_eq!(picked.borrow().as_slice(), ["theme-dark"]);
    assert!(
        host.query_selector("[data-testid=command-palette]")
            .unwrap()
            .is_none()
    );
    tick().await;
    assert_eq!(
        active().as_ref(),
        Some(trigger.unchecked_ref::<Element>()),
        "focus back on trigger"
    );

    // Escape closes and returns focus too.
    trigger.click();
    tick().await;
    let input = q(&host, "[data-testid=palette-input]");
    key(&input, "Escape");
    tick().await;
    tick().await;
    assert!(
        host.query_selector("[data-testid=command-palette]")
            .unwrap()
            .is_none()
    );
    assert_eq!(active().as_ref(), Some(trigger.unchecked_ref::<Element>()));
    assert_eq!(picked.borrow().len(), 1, "escape selects nothing");
}

#[wasm_bindgen_test]
async fn palette_empty_state_is_text_only() {
    let picked = Rc::new(std::cell::RefCell::new(Vec::new()));
    let host = mount::<PalHost>(PalHostProps { picked });
    tick().await;
    let trigger: HtmlElement = q(&host, "#pal-trigger").dyn_into().unwrap();
    trigger.click();
    tick().await;
    let input = q(&host, "[data-testid=palette-input]");
    input_text(&input, "needle-xyz<b>");
    tick().await;
    let empty = q(&host, "[data-testid=palette-empty]");
    assert!(empty.text_content().unwrap().contains("needle-xyz<b>"));
    assert!(
        empty.query_selector("b").unwrap().is_none(),
        "query must not become markup"
    );
    assert!(q(&host, "#palette-list").has_attribute("hidden"));
    assert_eq!(
        input.get_attribute("aria-expanded").as_deref(),
        Some("false")
    );
    yew::platform::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(
        q(&host, "[data-testid=palette-status]")
            .text_content()
            .as_deref(),
        Some("No results")
    );
    // Highlight is a <mark> vnode, not HTML.
    input_text(&input, "sign");
    tick().await;
    let m = q(&host, "[data-testid=palette-option-signout] mark");
    assert_eq!(m.text_content().as_deref(), Some("Sign"));
    key(&input, "Escape");
    tick().await;
}

#[function_component]
fn PopHost() -> Html {
    let open = use_state(|| false);
    let anchor = use_node_ref();
    let onclick = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(!*open))
    };
    let on_close = {
        let open = open.clone();
        Callback::from(move |_| open.set(false))
    };
    html! {
        <div>
            <button id="seg" ref={anchor.clone()} aria-controls="pop" aria-expanded={(*open).to_string()} {onclick}>{ "Site" }</button>
            <Popover id="pop" open={*open} {on_close} {anchor} label="Site" testid="scope-site-popover">
                <select id="sel" data-testid="scope-site"><option>{ "a" }</option></select>
            </Popover>
            <button id="outside">{ "x" }</button>
        </div>
    }
}

#[wasm_bindgen_test]
async fn popover_closes_on_focus_out_without_moving_focus() {
    let host = mount::<PopHost>(());
    tick().await;
    let pop = q(&host, "#pop");
    let seg: HtmlElement = q(&host, "#seg").dyn_into().unwrap();
    seg.click();
    tick().await;
    assert!(!pop.has_attribute("hidden"));
    // Back to the anchor keeps it open (A11Y-002).
    seg.focus().unwrap();
    tick().await;
    assert!(!pop.has_attribute("hidden"), "anchor is inside");
    q(&host, "#sel")
        .dyn_into::<HtmlElement>()
        .unwrap()
        .focus()
        .unwrap();
    let outside: HtmlElement = q(&host, "#outside").dyn_into().unwrap();
    outside.focus().unwrap();
    tick().await;
    assert!(pop.has_attribute("hidden"), "focus left panel and anchor");
    assert_eq!(seg.get_attribute("aria-expanded").as_deref(), Some("false"));
    assert_eq!(
        active().map(|e| e.id()).as_deref(),
        Some("outside"),
        "focus not moved"
    );
}

#[wasm_bindgen_test]
async fn popover_open_escape_and_outside_click() {
    let host = mount::<PopHost>(());
    tick().await;
    let pop = q(&host, "#pop");
    assert!(pop.has_attribute("hidden"), "closed popover is hidden");
    assert_eq!(pop.get_attribute("role").as_deref(), Some("dialog"));
    assert!(
        q(&host, "[data-testid=scope-site]").is_connected(),
        "select stays in DOM"
    );
    let seg: HtmlElement = q(&host, "#seg").dyn_into().unwrap();
    seg.click();
    tick().await;
    assert!(!pop.has_attribute("hidden"));
    assert_eq!(
        active().map(|e| e.id()).as_deref(),
        Some("sel"),
        "focus moves to select"
    );
    key(&q(&host, "#sel"), "Escape");
    tick().await;
    assert!(pop.has_attribute("hidden"));
    assert_eq!(
        active().map(|e| e.id()).as_deref(),
        Some("seg"),
        "focus back to segment"
    );

    seg.click();
    tick().await;
    assert!(!pop.has_attribute("hidden"));
    let outside = q(&host, "#outside");
    let ev = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &{
        let i = web_sys::PointerEventInit::new();
        i.set_bubbles(true);
        i
    })
    .unwrap();
    outside.dispatch_event(&ev).unwrap();
    tick().await;
    assert!(pop.has_attribute("hidden"), "outside pointerdown closes");
    assert_eq!(
        active().map(|e| e.id()).as_deref(),
        Some("seg"),
        "focus was inside: returns to segment"
    );
}

#[wasm_bindgen_test]
async fn top_navigation_order_and_scope_slot() {
    #[function_component]
    fn T() -> Html {
        html! {
            <TopNavigation brand={html!{ <a href="/">{ "Brand" }</a> }}
                start={html!{ <span data-testid="st">{ "s" }</span> }}
                scope={html!{ <span data-testid="sc">{ "c" }</span> }} />
        }
    }
    let host = mount::<T>(());
    tick().await;
    let start = q(&host, ".cui-topnav__start");
    let first: Element = start.first_element_child().unwrap();
    assert_eq!(
        first.get_attribute("data-testid").as_deref(),
        Some("st"),
        "toggle before brand"
    );
    assert!(
        host.query_selector(".cui-topnav__scope [data-testid=sc]")
            .unwrap()
            .is_some()
    );
}

#[wasm_bindgen_test]
async fn nav_toggle_renders_icon() {
    #[function_component]
    fn T() -> Html {
        html! { <NavToggle expanded=true controls="r" on_toggle={Callback::noop()} icon={IconName::Collapse} label="Collapse navigation" /> }
    }
    let host = mount::<T>(());
    tick().await;
    let b = q(&host, "[data-testid=nav-toggle]");
    assert_eq!(
        b.get_attribute("aria-label").as_deref(),
        Some("Collapse navigation")
    );
    assert!(b.query_selector("svg.cui-icon").unwrap().is_some());
}

#[function_component]
fn EmbedHost() -> Html {
    let embedded = use_state(|| false);
    let anchor = use_node_ref();
    let onclick = {
        let embedded = embedded.clone();
        Callback::from(move |_: MouseEvent| embedded.set(!*embedded))
    };
    html! {
        <div>
            <button id="flip" ref={anchor.clone()} {onclick}>{ "flip" }</button>
            <Popover id="epop" open={false} on_close={Callback::noop()} {anchor}
                label="Scope" embedded={*embedded} testid="scope-popover">
                <select id="esel"><option>{ "a" }</option></select>
            </Popover>
        </div>
    }
}

/// D4: switching `embedded` keeps the same child nodes (no re-creation) and
/// drops the dialog semantics while embedded.
#[wasm_bindgen_test]
async fn popover_embedded_keeps_children() {
    let host = mount::<EmbedHost>(());
    tick().await;
    let sel = q(&host, "#esel");
    let pop = q(&host, "#epop");
    assert!(pop.has_attribute("hidden"));
    let flip: HtmlElement = q(&host, "#flip").dyn_into().unwrap();
    flip.click();
    tick().await;
    assert!(!pop.has_attribute("hidden"), "embedded is never hidden");
    assert!(!pop.has_attribute("role"), "embedded has no dialog role");
    assert!(!pop.has_attribute("data-testid"));
    assert!(q(&host, "#esel").is_same_node(Some(&sel)), "select reused");
    flip.click();
    tick().await;
    assert_eq!(pop.get_attribute("role").as_deref(), Some("dialog"));
    assert!(q(&host, "#esel").is_same_node(Some(&sel)), "select reused");
}
