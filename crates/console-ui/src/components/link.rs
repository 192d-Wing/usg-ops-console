use crate::logic::safe_href;
use yew::prelude::*;

/// Client-side navigation hook: receives the click event and the href. The
/// component has already called `prevent_default` when this fires.
pub type OnFollow = Callback<(MouseEvent, AttrValue)>;

/// True for a plain primary-button click without modifier keys, i.e. one the
/// app may intercept for client-side navigation.
pub(crate) fn is_plain_click(e: &MouseEvent) -> bool {
    e.button() == 0 && !e.ctrl_key() && !e.meta_key() && !e.shift_key() && !e.alt_key()
}

/// Builds an `onclick` that diverts plain clicks to `onfollow`.
pub(crate) fn follow_handler(
    href: AttrValue,
    onfollow: Option<OnFollow>,
) -> Option<Callback<MouseEvent>> {
    onfollow.map(|cb| {
        Callback::from(move |e: MouseEvent| {
            if is_plain_click(&e) {
                e.prevent_default();
                cb.emit((e, href.clone()));
            }
        })
    })
}

/// Sanitised href (see [`safe_href`]).
pub(crate) fn sanitize(href: &AttrValue) -> AttrValue {
    let safe = safe_href(href);
    if safe.len() == href.len() {
        href.clone()
    } else {
        AttrValue::from(safe.to_owned())
    }
}

#[derive(Properties, PartialEq)]
pub struct LinkProps {
    pub href: AttrValue,
    /// Opens in a new tab with `rel="noopener noreferrer"`.
    #[prop_or_default]
    pub external: bool,
    #[prop_or_default]
    pub onfollow: Option<OnFollow>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
    #[prop_or_default]
    pub children: Html,
}

#[function_component]
pub fn Link(props: &LinkProps) -> Html {
    let href = sanitize(&props.href);
    let onclick = if props.external {
        None
    } else {
        follow_handler(href.clone(), props.onfollow.clone())
    };
    html! {
        <a
            class="cui-link"
            href={href}
            target={props.external.then_some("_blank")}
            rel={props.external.then_some("noopener noreferrer")}
            data-testid={props.testid.clone()}
            {onclick}
        >
            { props.children.clone() }
            if props.external {
                <span class="cui-link__external" aria-hidden="true">{ "\u{2197}" }</span>
                <span class="cui-visually-hidden">{ " (opens in a new tab)" }</span>
            }
        </a>
    }
}
