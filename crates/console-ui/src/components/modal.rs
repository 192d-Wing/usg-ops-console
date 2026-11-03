use crate::id::use_id;
use crate::logic::trap_index;
use web_sys::wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement};
use yew::prelude::*;

const FOCUSABLE: &str = "a[href], button:not([disabled]), input:not([disabled]), \
    select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex=\"-1\"])";

fn focusables(root: &Element) -> Vec<HtmlElement> {
    let Ok(list) = root.query_selector_all(FOCUSABLE) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i))
        .filter_map(|n| n.dyn_into::<HtmlElement>().ok())
        .collect()
}

fn active_element() -> Option<Element> {
    web_sys::window()?.document()?.active_element()
}

/// Sets or clears `inert` on the element with `id`. Errors are reported to the
/// caller so they can be surfaced (never swallowed).
fn set_inert(id: &str, on: bool) -> Result<(), AttrValue> {
    let el = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .ok_or_else(|| AttrValue::from(format!("Modal: inert root #{id} not found")))?;
    let r = if on {
        el.set_attribute("inert", "")
    } else {
        el.remove_attribute("inert")
    };
    r.map_err(|_| AttrValue::from("Modal: could not update inert state"))
}

/// Returns focus to `el`. If it is still inside an `inert` subtree (e.g. a
/// parent clears inert in its own effect cleanup, whose order relative to ours
/// is not guaranteed), retry once in a microtask after that cleanup has run.
fn restore_focus(el: HtmlElement) {
    let inert = el.closest("[inert]").ok().flatten().is_some();
    if !inert {
        let _ = el.focus();
        return;
    }
    yew::platform::spawn_local(async move {
        let _ = el.focus();
    });
}

#[derive(Properties, PartialEq)]
pub struct ModalProps {
    pub visible: bool,
    pub header: AttrValue,
    #[prop_or_default]
    pub footer: Option<Html>,
    /// Fired on Esc, the close button, or an overlay click.
    pub on_dismiss: Callback<()>,
    /// Id of the background region (outside the modal's DOM subtree) to mark
    /// `inert` while visible. The Modal clears it *before* restoring focus to
    /// the opener, so focus restoration is deterministic.
    #[prop_or_default]
    pub inert_root: Option<AttrValue>,
    /// Receives errors from inert handling so the host can show them in the
    /// flashbar.
    #[prop_or_default]
    pub on_error: Callback<AttrValue>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
    #[prop_or_default]
    pub children: Html,
}

/// Modal dialog. While visible it traps Tab/Shift+Tab, closes on Esc, and
/// returns focus to the previously focused element when hidden. Pass
/// `inert_root` so the Modal marks the background region `inert` while open
/// and clears it before focus is restored.
#[function_component]
pub fn Modal(props: &ModalProps) -> Html {
    let dialog = use_node_ref();
    let opener = use_mut_ref(|| None::<HtmlElement>);
    let title_id = use_id("modal-title");

    {
        let dialog = dialog.clone();
        let opener = opener.clone();
        let inert_root = props.inert_root.clone();
        let on_error = props.on_error.clone();
        use_effect_with(props.visible, move |visible| {
            let visible = *visible;
            if visible {
                if let Some(id) = &inert_root
                    && let Err(e) = set_inert(id, true)
                {
                    on_error.emit(e);
                }
                *opener.borrow_mut() =
                    active_element().and_then(|e| e.dyn_into::<HtmlElement>().ok());
                if let Some(d) = dialog.cast::<HtmlElement>() {
                    let _ = d.focus();
                }
            }
            move || {
                if !visible {
                    return;
                }
                // Clear inert first: focus() on an inert node is a no-op.
                if let Some(id) = &inert_root
                    && let Err(e) = set_inert(id, false)
                {
                    on_error.emit(e);
                }
                if let Some(el) = opener.borrow_mut().take() {
                    restore_focus(el);
                }
            }
        });
    }

    if !props.visible {
        return html! {};
    }

    let onkeydown = {
        let dialog = dialog.clone();
        let on_dismiss = props.on_dismiss.clone();
        Callback::from(move |e: KeyboardEvent| match e.key().as_str() {
            "Escape" => {
                e.prevent_default();
                e.stop_propagation();
                on_dismiss.emit(());
            }
            "Tab" => {
                let Some(root) = dialog.cast::<Element>() else {
                    return;
                };
                let items = focusables(&root);
                let active = active_element();
                let cur = items
                    .iter()
                    .position(|el| active.as_ref() == Some(el.unchecked_ref::<Element>()));
                if let Some(next) = trap_index(cur, items.len(), e.shift_key()) {
                    e.prevent_default();
                    let _ = items[next].focus();
                } else {
                    e.prevent_default();
                }
            }
            _ => {}
        })
    };
    let close = props.on_dismiss.reform(|_: MouseEvent| ());
    let overlay = props.on_dismiss.reform(|_: MouseEvent| ());

    html! {
        <div class="cui-modal" data-testid={props.testid.clone()}>
            <div class="cui-modal__overlay" aria-hidden="true" onclick={overlay}></div>
            <div
                class="cui-modal__dialog"
                role="dialog"
                aria-modal="true"
                aria-labelledby={title_id.clone()}
                tabindex="-1"
                ref={dialog}
                {onkeydown}
            >
                <div class="cui-modal__header">
                    <h2 class="cui-modal__title" id={title_id}>{ props.header.clone() }</h2>
                    <button type="button" class="cui-icon-btn" aria-label="Close dialog" onclick={close}>
                        <span aria-hidden="true">{ "\u{2715}" }</span>
                    </button>
                </div>
                <div class="cui-modal__body">{ props.children.clone() }</div>
                if let Some(f) = &props.footer {
                    <div class="cui-modal__footer">{ f.clone() }</div>
                }
            </div>
        </div>
    }
}
