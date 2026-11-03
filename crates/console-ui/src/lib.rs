//! Console component library. Styling lives in CSS custom properties
//! (`assets/tokens.css` in console-shell) so tools in iframes can share it.

use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Variant {
    #[default]
    Normal,
    Primary,
    Link,
}

#[derive(Properties, PartialEq)]
pub struct ButtonProps {
    #[prop_or_default]
    pub variant: Variant,
    #[prop_or_default]
    pub disabled: bool,
    #[prop_or_default]
    pub onclick: Callback<MouseEvent>,
    pub children: Html,
}

#[function_component]
pub fn Button(props: &ButtonProps) -> Html {
    let class = match props.variant {
        Variant::Normal => "cui-btn",
        Variant::Primary => "cui-btn cui-btn--primary",
        Variant::Link => "cui-btn cui-btn--link",
    };
    html! {
        <button type="button" {class} disabled={props.disabled} onclick={props.onclick.clone()}>
            { props.children.clone() }
        </button>
    }
}

#[derive(Properties, PartialEq)]
pub struct ContainerProps {
    #[prop_or_default]
    pub header: Option<AttrValue>,
    pub children: Html,
}

/// Bordered content panel with an optional header.
#[function_component]
pub fn Container(props: &ContainerProps) -> Html {
    html! {
        <section class="cui-container">
            if let Some(h) = &props.header {
                <h2 class="cui-container__header">{ h }</h2>
            }
            <div class="cui-container__body">{ props.children.clone() }</div>
        </section>
    }
}
