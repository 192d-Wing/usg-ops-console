use super::modal::{active_element, focusables};
use web_sys::wasm_bindgen::JsCast;
use web_sys::wasm_bindgen::closure::Closure;
use web_sys::{Element, HtmlElement, Node};
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PopoverAlign {
    #[default]
    Start,
    End,
}

#[derive(Properties, PartialEq)]
pub struct PopoverProps {
    /// DOM id; reference it from the trigger's `aria-controls`.
    pub id: AttrValue,
    pub open: bool,
    /// Fired on Escape, on a pointerdown outside the panel and anchor, and
    /// when focus leaves the panel for anything but the anchor.
    pub on_close: Callback<()>,
    /// The trigger. Excluded from outside-click detection; focus returns here.
    pub anchor: NodeRef,
    /// Accessible name of the `role="dialog"` panel.
    #[prop_or_default]
    pub label: Option<AttrValue>,
    /// Element focused on open; defaults to the first focusable descendant.
    #[prop_or_default]
    pub initial_focus: Option<NodeRef>,
    #[prop_or_default]
    pub align: PopoverAlign,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
    /// Render the panel as a transparent pass-through (`display: contents`,
    /// no dialog role, no focus/close handling). The element itself stays,
    /// so hosts can switch layouts without recreating the children (D4).
    #[prop_or_default]
    pub embedded: bool,
    #[prop_or_default]
    pub children: Html,
}

fn contains(root: &NodeRef, target: Option<&Node>) -> bool {
    match (root.cast::<Node>(), target) {
        (Some(r), Some(t)) => r.contains(Some(t)),
        _ => false,
    }
}

/// Non-modal anchored panel. Always in the DOM (`hidden` while closed) so its
/// controls keep stable ids/testids. The host owns `open` and the trigger's
/// `aria-expanded`.
#[function_component]
pub fn Popover(p: &PopoverProps) -> Html {
    let panel = use_node_ref();

    // Focus on open (synchronous, so a host-opened popover wins over any
    // deferred focus restoration elsewhere, e.g. the command palette).
    {
        let panel = panel.clone();
        let initial = p.initial_focus.clone();
        use_effect_with(p.open && !p.embedded, move |open| {
            if *open {
                let target = initial
                    .as_ref()
                    .and_then(|r| r.cast::<HtmlElement>())
                    .or_else(|| {
                        panel
                            .cast::<Element>()
                            .and_then(|root| focusables(&root).into_iter().next())
                    });
                if let Some(el) = target {
                    let _ = el.focus();
                }
            }
        });
    }

    // Outside pointerdown closes; listener only while open.
    {
        let panel = panel.clone();
        let anchor = p.anchor.clone();
        let on_close = p.on_close.clone();
        use_effect_with(p.open && !p.embedded, move |open| {
            let doc = web_sys::window().and_then(|w| w.document());
            let listener = (*open).then(|| {
                let anchor_f = anchor.clone();
                Closure::<dyn Fn(web_sys::Event)>::new(move |e: web_sys::Event| {
                    let target = e.target().and_then(|t| t.dyn_into::<Node>().ok());
                    if contains(&panel, target.as_ref()) || contains(&anchor_f, target.as_ref()) {
                        return;
                    }
                    let focus_inside = active_element()
                        .map(|a| contains(&panel, Some(a.unchecked_ref::<Node>())))
                        .unwrap_or(false);
                    on_close.emit(());
                    if focus_inside && let Some(a) = anchor_f.cast::<HtmlElement>() {
                        let _ = a.focus();
                    }
                })
            });
            if let (Some(d), Some(l)) = (&doc, &listener) {
                let _ = d.add_event_listener_with_callback_and_bool(
                    "pointerdown",
                    l.as_ref().unchecked_ref(),
                    true,
                );
            }
            move || {
                if let (Some(d), Some(l)) = (doc, listener) {
                    let _ = d.remove_event_listener_with_callback_and_bool(
                        "pointerdown",
                        l.as_ref().unchecked_ref(),
                        true,
                    );
                }
            }
        });
    }

    let onkeydown = {
        let on_close = p.on_close.clone();
        let anchor = p.anchor.clone();
        let embedded = p.embedded;
        Callback::from(move |e: KeyboardEvent| {
            if !embedded && e.key() == "Escape" {
                e.prevent_default();
                e.stop_propagation();
                on_close.emit(());
                if let Some(a) = anchor.cast::<HtmlElement>() {
                    let _ = a.focus();
                }
            }
        })
    };

    // Tabbing out of the panel to anything but the anchor closes it without
    // moving focus (A11Y-002). A null `relatedTarget` (window blur, click on
    // nothing focusable) is left to the pointerdown handler.
    let onfocusout = {
        let on_close = p.on_close.clone();
        let anchor = p.anchor.clone();
        let panel = panel.clone();
        let active = p.open && !p.embedded;
        Callback::from(move |e: FocusEvent| {
            if !active {
                return;
            }
            let Some(next) = e.related_target().and_then(|t| t.dyn_into::<Node>().ok()) else {
                return;
            };
            if !contains(&panel, Some(&next)) && !contains(&anchor, Some(&next)) {
                on_close.emit(());
            }
        })
    };

    let class = classes!(
        "cui-popover",
        (p.align == PopoverAlign::End).then_some("cui-popover--end"),
        p.embedded.then_some("cui-popover--embedded")
    );
    let embedded = p.embedded;
    html! {
        <div
            {class}
            id={p.id.clone()}
            role={(!embedded).then_some("dialog")}
            aria-label={p.label.clone().filter(|_| !embedded)}
            hidden={!embedded && !p.open}
            data-testid={p.testid.clone().filter(|_| !embedded)}
            ref={panel}
            {onkeydown}
            {onfocusout}
        >
            { p.children.clone() }
        </div>
    }
}
