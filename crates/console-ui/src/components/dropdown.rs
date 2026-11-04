//! Disclosure dropdown: a `<button aria-expanded>` controlling a panel of
//! plain links/buttons. Deliberately not an ARIA `menu` (see plan M2 §1).

use super::link::{OnFollow, is_plain_click, sanitize};
use crate::id::use_id;
use crate::logic::dropdown_next_index;
use web_sys::wasm_bindgen::JsCast;
use web_sys::wasm_bindgen::closure::Closure;
use web_sys::{Element, HtmlElement, Node};
use yew::prelude::*;

#[derive(Clone, PartialEq, Debug)]
pub enum DropdownItem {
    Link {
        id: AttrValue,
        text: AttrValue,
        href: AttrValue,
        description: Option<AttrValue>,
    },
    /// Emits `on_action(id)`.
    Action {
        id: AttrValue,
        text: AttrValue,
    },
    /// Non-interactive status text (e.g. "Loading…", "Nothing here"). Not
    /// focusable and skipped by arrow-key navigation (WCAG 4.1.2).
    Note {
        id: AttrValue,
        text: AttrValue,
    },
    /// One level only; a nested `Group` inside a group is ignored.
    Group {
        text: AttrValue,
        items: Vec<DropdownItem>,
    },
    Divider,
}

#[derive(Properties, PartialEq)]
pub struct DropdownProps {
    /// Visible button text (accessible name unless `aria_label` is set).
    pub label: AttrValue,
    #[prop_or_default]
    pub aria_label: Option<AttrValue>,
    pub items: Vec<DropdownItem>,
    #[prop_or_default]
    pub on_action: Callback<AttrValue>,
    #[prop_or_default]
    pub onfollow: Option<OnFollow>,
    /// Align the panel to the inline-end edge of the button.
    #[prop_or_default]
    pub align_end: bool,
    /// `data-testid` on the root; button gets `{id}-button`, panel `{id}-panel`.
    #[prop_or_default]
    pub test_id: Option<AttrValue>,
}

const ITEM_SEL: &str = ".cui-dropdown__item";

fn items_of(panel: &NodeRef) -> Vec<HtmlElement> {
    let Some(p) = panel.cast::<Element>() else {
        return Vec::new();
    };
    let Ok(list) = p.query_selector_all(ITEM_SEL) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i))
        .filter_map(|n| n.dyn_into::<HtmlElement>().ok())
        .collect()
}

fn contains(root: &NodeRef, node: Option<&Node>) -> bool {
    match (root.cast::<Node>(), node) {
        (Some(r), Some(n)) => r.contains(Some(n)),
        _ => false,
    }
}

struct Ctx<'a> {
    on_action: &'a Callback<AttrValue>,
    onfollow: &'a Option<OnFollow>,
    close: &'a Callback<()>,
    test_id: &'a Option<AttrValue>,
}

fn item_testid(ctx: &Ctx<'_>, id: &AttrValue) -> Option<AttrValue> {
    ctx.test_id
        .as_ref()
        .map(|t| AttrValue::from(format!("{t}-item-{id}")))
}

fn render_item(item: &DropdownItem, key: String, ctx: &Ctx<'_>, nested: bool) -> Html {
    match item {
        DropdownItem::Link {
            id,
            text,
            href,
            description,
        } => {
            let href = sanitize(href);
            let onclick = {
                let href = href.clone();
                let onfollow = ctx.onfollow.clone();
                let close = ctx.close.clone();
                Callback::from(move |e: MouseEvent| {
                    if let Some(cb) = &onfollow
                        && is_plain_click(&e)
                    {
                        e.prevent_default();
                        close.emit(());
                        cb.emit((e, href.clone()));
                    } else {
                        close.emit(());
                    }
                })
            };
            html! {
                <li class="cui-dropdown__entry" {key}>
                    <a class="cui-dropdown__item" {href} {onclick} data-testid={item_testid(ctx, id)}>
                        <span class="cui-dropdown__text">{ text.clone() }</span>
                        if let Some(d) = description {
                            <span class="cui-dropdown__description">{ d.clone() }</span>
                        }
                    </a>
                </li>
            }
        }
        DropdownItem::Action { id, text } => {
            let onclick = {
                let id = id.clone();
                let on_action = ctx.on_action.clone();
                let close = ctx.close.clone();
                Callback::from(move |_: MouseEvent| {
                    close.emit(());
                    on_action.emit(id.clone());
                })
            };
            html! {
                <li class="cui-dropdown__entry" {key}>
                    <button type="button" class="cui-dropdown__item" {onclick}
                        data-testid={item_testid(ctx, id)}>
                        <span class="cui-dropdown__text">{ text.clone() }</span>
                    </button>
                </li>
            }
        }
        DropdownItem::Note { id, text } => html! {
            <li class="cui-dropdown__entry" {key}>
                <span class="cui-dropdown__note" data-testid={item_testid(ctx, id)}>
                    { text.clone() }
                </span>
            </li>
        },
        DropdownItem::Group { text, items } => {
            if nested {
                return html! {};
            }
            html! {
                <li class="cui-dropdown__group" {key}>
                    <span class="cui-dropdown__group-title" aria-hidden="true">{ text.clone() }</span>
                    <ul class="cui-dropdown__list" aria-label={text.clone()}>
                        { for items.iter().enumerate().map(|(i, it)| render_item(it, format!("g{i}"), ctx, true)) }
                    </ul>
                </li>
            }
        }
        DropdownItem::Divider => html! {
            <li class="cui-dropdown__divider" aria-hidden="true" {key}></li>
        },
    }
}

/// Disclosure dropdown. Opening focuses the first item; ArrowUp/ArrowDown
/// (wrapping), Home and End move between items; Escape closes and returns
/// focus to the button; Tab out or a click outside closes.
#[function_component]
pub fn Dropdown(props: &DropdownProps) -> Html {
    let open = use_state_eq(|| false);
    let button_id = use_id("dropdown-button");
    let panel_id = use_id("dropdown-panel");
    let root = use_node_ref();
    let button = use_node_ref();
    let panel = use_node_ref();

    // Focus the first item on open; document pointerdown listener only while open.
    {
        let root = root.clone();
        let panel = panel.clone();
        let open_h = open.clone();
        use_effect_with(*open, move |is_open| {
            let mut cleanup: Option<Box<dyn FnOnce()>> = None;
            if *is_open {
                if let Some(first) = items_of(&panel).first() {
                    let _ = first.focus();
                }
                let doc = web_sys::window().and_then(|w| w.document());
                if let Some(doc) = doc {
                    let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |e: web_sys::Event| {
                        let target = e.target().and_then(|t| t.dyn_into::<Node>().ok());
                        if !contains(&root, target.as_ref()) {
                            open_h.set(false);
                        }
                    });
                    if doc
                        .add_event_listener_with_callback(
                            "pointerdown",
                            cb.as_ref().unchecked_ref(),
                        )
                        .is_ok()
                    {
                        cleanup = Some(Box::new(move || {
                            let _ = doc.remove_event_listener_with_callback(
                                "pointerdown",
                                cb.as_ref().unchecked_ref(),
                            );
                            drop(cb);
                        }));
                    }
                }
            }
            move || {
                if let Some(c) = cleanup {
                    c();
                }
            }
        });
    }

    let toggle = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(!*open))
    };
    let close = {
        let open = open.clone();
        Callback::from(move |()| open.set(false))
    };
    let close_and_return = {
        let open = open.clone();
        let button = button.clone();
        move || {
            open.set(false);
            if let Some(b) = button.cast::<HtmlElement>() {
                let _ = b.focus();
            }
        }
    };

    let on_button_key = {
        let open = open.clone();
        let close_and_return = close_and_return.clone();
        Callback::from(move |e: KeyboardEvent| match e.key().as_str() {
            "ArrowDown" if !*open => {
                e.prevent_default();
                open.set(true);
            }
            "Escape" if *open => {
                e.prevent_default();
                e.stop_propagation();
                close_and_return();
            }
            _ => {}
        })
    };

    let on_panel_key = {
        let panel = panel.clone();
        Callback::from(move |e: KeyboardEvent| {
            let key = e.key();
            if key == "Escape" {
                e.prevent_default();
                e.stop_propagation();
                close_and_return();
                return;
            }
            let delta = match key.as_str() {
                "ArrowDown" => Some(1),
                "ArrowUp" => Some(-1),
                "Home" | "End" => None,
                _ => return,
            };
            let items = items_of(&panel);
            if items.is_empty() {
                return;
            }
            e.prevent_default();
            let active = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.active_element());
            let cur = items
                .iter()
                .position(|el| active.as_ref() == Some(el.unchecked_ref::<Element>()));
            let next = match (delta, key.as_str()) {
                (Some(d), _) => match cur {
                    Some(c) => dropdown_next_index(items.len(), c, d),
                    None if d > 0 => 0,
                    None => items.len() - 1,
                },
                (None, "Home") => 0,
                _ => items.len() - 1,
            };
            let _ = items[next].focus();
        })
    };

    // Tab (or any focus move) out of the dropdown closes it. A null
    // relatedTarget (e.g. Safari not focusing clicked buttons) is ignored;
    // outside clicks are handled by the pointerdown listener.
    let onfocusout = {
        let open = open.clone();
        let root = root.clone();
        Callback::from(move |e: FocusEvent| {
            if let Some(t) = e.related_target().and_then(|t| t.dyn_into::<Node>().ok())
                && !contains(&root, Some(&t))
            {
                open.set(false);
            }
        })
    };

    let ctx = Ctx {
        on_action: &props.on_action,
        onfollow: &props.onfollow,
        close: &close,
        test_id: &props.test_id,
    };
    let sub = |suffix: &str| {
        props
            .test_id
            .as_ref()
            .map(|t| AttrValue::from(format!("{t}-{suffix}")))
    };

    html! {
        <div class="cui-dropdown" ref={root} data-testid={props.test_id.clone()} {onfocusout}>
            <button
                type="button"
                class="cui-dropdown__button"
                id={button_id.clone()}
                aria-expanded={if *open { "true" } else { "false" }}
                aria-controls={panel_id.clone()}
                aria-label={props.aria_label.clone()}
                data-testid={sub("button")}
                ref={button}
                onclick={toggle}
                onkeydown={on_button_key}
            >
                <span class="cui-dropdown__label">{ props.label.clone() }</span>
                <span class="cui-dropdown__caret" aria-hidden="true"></span>
            </button>
            <ul
                class={classes!("cui-dropdown__panel", props.align_end.then_some("cui-dropdown__panel--end"))}
                id={panel_id}
                aria-labelledby={button_id}
                hidden={!*open}
                data-testid={sub("panel")}
                ref={panel}
                onkeydown={on_panel_key}
            >
                if *open {
                    { for props.items.iter().enumerate().map(|(i, it)| render_item(it, format!("i{i}"), &ctx, false)) }
                }
            </ul>
        </div>
    }
}
