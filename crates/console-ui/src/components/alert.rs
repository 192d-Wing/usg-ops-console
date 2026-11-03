use crate::logic::{queue_push, queue_remove, removal_focus_index};
use web_sys::wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement};
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AlertKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl AlertKind {
    fn modifier(self) -> &'static str {
        match self {
            AlertKind::Info => "info",
            AlertKind::Success => "success",
            AlertKind::Warning => "warning",
            AlertKind::Error => "error",
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            AlertKind::Info => "\u{2139}",
            AlertKind::Success => "\u{2714}",
            AlertKind::Warning => "\u{26A0}",
            AlertKind::Error => "\u{2716}",
        }
    }
    /// Visually hidden prefix so the type is not conveyed by colour alone.
    fn label(self) -> &'static str {
        match self {
            AlertKind::Info => "Info: ",
            AlertKind::Success => "Success: ",
            AlertKind::Warning => "Warning: ",
            AlertKind::Error => "Error: ",
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct AlertProps {
    #[prop_or_default]
    pub kind: AlertKind,
    #[prop_or_default]
    pub header: Option<AttrValue>,
    #[prop_or_default]
    pub dismissible: bool,
    /// Fired when the dismiss button is pressed. The button is removed with
    /// the alert, so the consumer must move focus to a sensible target (e.g.
    /// the next control or the page heading) to avoid dropping it to `<body>`
    /// (WCAG 2.4.3).
    #[prop_or_default]
    pub on_dismiss: Callback<()>,
    #[prop_or_default]
    pub action: Option<Html>,
    #[prop_or_default]
    pub children: Html,
}

/// Inline, in-page message. Not a live region: use [`Flashbar`] for
/// notifications that appear in response to an action.
#[function_component]
pub fn Alert(props: &AlertProps) -> Html {
    let k = props.kind;
    let on_dismiss = props.on_dismiss.reform(|_: MouseEvent| ());
    html! {
        <div class={classes!("cui-alert", format!("cui-alert--{}", k.modifier()))}>
            <span class="cui-alert__icon" aria-hidden="true">{ k.glyph() }</span>
            <div class="cui-alert__content">
                if let Some(h) = &props.header {
                    <div class="cui-alert__header">
                        <span class="cui-visually-hidden">{ k.label() }</span>{ h.clone() }
                    </div>
                } else {
                    <span class="cui-visually-hidden">{ k.label() }</span>
                }
                <div class="cui-alert__body">{ props.children.clone() }</div>
            </div>
            if let Some(a) = &props.action {
                <div class="cui-alert__action">{ a.clone() }</div>
            }
            if props.dismissible {
                <button type="button" class="cui-icon-btn" aria-label="Dismiss" onclick={on_dismiss}>
                    <span aria-hidden="true">{ "\u{2715}" }</span>
                </button>
            }
        </div>
    }
}

/// Identifier of a flash notification; callers allocate these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct FlashId(pub u64);

#[derive(Clone, PartialEq, Debug)]
pub struct FlashItem {
    pub id: FlashId,
    pub kind: AlertKind,
    pub header: Option<AttrValue>,
    pub content: AttrValue,
    pub dismissible: bool,
}

/// Maximum notifications kept by [`flash_push`].
pub const FLASH_MAX: usize = 5;

/// Adds a notification (replacing one with the same id), capped at
/// [`FLASH_MAX`].
pub fn flash_push(items: &mut Vec<FlashItem>, item: FlashItem) {
    queue_push(items, item, |i| i.id, FLASH_MAX);
}

/// Removes a notification; returns whether it existed.
pub fn flash_dismiss(items: &mut Vec<FlashItem>, id: FlashId) -> bool {
    queue_remove(items, &id, |i| i.id)
}

#[derive(Properties, PartialEq)]
pub struct FlashbarProps {
    pub items: Vec<FlashItem>,
    #[prop_or_default]
    pub on_dismiss: Callback<FlashId>,
    /// Id of the element to focus when the last dismissible flash is removed
    /// (e.g. `main-content`). Otherwise focus moves to the next/previous
    /// flash's dismiss button.
    #[prop_or_default]
    pub fallback_focus_id: Option<AttrValue>,
}

/// Moves focus off the dismiss button that is about to be removed.
fn move_focus_before_dismiss(e: &MouseEvent, fallback: Option<&str>) {
    let Some(btn) = e
        .current_target()
        .and_then(|t| t.dyn_into::<Element>().ok())
    else {
        return;
    };
    let Some(root) = btn.closest(".cui-flashbar").ok().flatten() else {
        return;
    };
    let Ok(list) = root.query_selector_all(".cui-flash__dismiss") else {
        return;
    };
    let buttons: Vec<HtmlElement> = (0..list.length())
        .filter_map(|i| list.item(i))
        .filter_map(|n| n.dyn_into::<HtmlElement>().ok())
        .collect();
    let idx = buttons
        .iter()
        .position(|b| b.unchecked_ref::<Element>() == &btn);
    let target = idx
        .and_then(|i| removal_focus_index(i, buttons.len()))
        .map(|i| buttons[i].clone())
        .or_else(|| {
            let id = fallback?;
            web_sys::window()?
                .document()?
                .get_element_by_id(id)?
                .dyn_into::<HtmlElement>()
                .ok()
        });
    if let Some(t) = target {
        let _ = t.focus();
    }
}

/// Stack of page-level notifications. The list is always rendered so the
/// live regions exist before content is added (required for announcements).
#[function_component]
pub fn Flashbar(props: &FlashbarProps) -> Html {
    let polite: Vec<&FlashItem> = props
        .items
        .iter()
        .filter(|i| !matches!(i.kind, AlertKind::Error))
        .collect();
    let assertive: Vec<&FlashItem> = props
        .items
        .iter()
        .filter(|i| matches!(i.kind, AlertKind::Error))
        .collect();
    let render = |item: &FlashItem| {
        let k = item.kind;
        let id = item.id;
        let on_dismiss = props.on_dismiss.clone();
        let fallback = props.fallback_focus_id.clone();
        let cb = Callback::from(move |e: MouseEvent| {
            move_focus_before_dismiss(&e, fallback.as_deref());
            on_dismiss.emit(id);
        });
        html! {
            <li key={id.0.to_string()}
                class={classes!("cui-flash", format!("cui-flash--{}", k.modifier()))}
                data-testid="flash-item">
                <span class="cui-flash__icon" aria-hidden="true">{ k.glyph() }</span>
                <div class="cui-flash__content">
                    <span class="cui-visually-hidden">{ k.label() }</span>
                    if let Some(h) = &item.header {
                        <strong class="cui-flash__header">{ h.clone() }</strong>{ " " }
                    }
                    <span>{ item.content.clone() }</span>
                </div>
                if item.dismissible {
                    <button type="button" class="cui-icon-btn cui-icon-btn--inverse cui-flash__dismiss"
                        aria-label="Dismiss notification" onclick={cb}>
                        <span aria-hidden="true">{ "\u{2715}" }</span>
                    </button>
                }
            </li>
        }
    };
    html! {
        <div class="cui-flashbar" data-testid="flashbar">
            <div role="alert" aria-relevant="additions">
                <ul class="cui-flashbar__list">{ for assertive.into_iter().map(render) }</ul>
            </div>
            <div role="status" aria-relevant="additions">
                <ul class="cui-flashbar__list">{ for polite.into_iter().map(render) }</ul>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64) -> FlashItem {
        FlashItem {
            id: FlashId(id),
            kind: AlertKind::Info,
            header: None,
            content: AttrValue::Static("x"),
            dismissible: true,
        }
    }

    #[test]
    fn flash_queue() {
        let mut v = Vec::new();
        for i in 0..7 {
            flash_push(&mut v, item(i));
        }
        assert_eq!(v.len(), FLASH_MAX);
        assert_eq!(v[0].id, FlashId(2));
        flash_push(&mut v, item(4));
        assert_eq!(v.len(), FLASH_MAX);
        assert!(flash_dismiss(&mut v, FlashId(4)));
        assert!(!flash_dismiss(&mut v, FlashId(4)));
    }
}
