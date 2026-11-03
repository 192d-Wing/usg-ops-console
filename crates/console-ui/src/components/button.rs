use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
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
    /// Shows a spinner, sets `aria-busy` and disables the button.
    #[prop_or_default]
    pub loading: bool,
    /// Renders `type="submit"` instead of `type="button"`.
    #[prop_or_default]
    pub submit: bool,
    #[prop_or_default]
    pub onclick: Callback<MouseEvent>,
    /// Required for icon-only buttons.
    #[prop_or_default]
    pub aria_label: Option<AttrValue>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
    #[prop_or_default]
    pub children: Html,
}

#[function_component]
pub fn Button(props: &ButtonProps) -> Html {
    let class = classes!(
        "cui-btn",
        match props.variant {
            Variant::Normal => None,
            Variant::Primary => Some("cui-btn--primary"),
            Variant::Link => Some("cui-btn--link"),
        },
        props.loading.then_some("cui-btn--loading"),
    );
    let ty = if props.submit { "submit" } else { "button" };
    html! {
        <button
            type={ty}
            {class}
            disabled={props.disabled || props.loading}
            aria-busy={props.loading.then_some("true")}
            aria-label={props.aria_label.clone()}
            data-testid={props.testid.clone()}
            onclick={props.onclick.clone()}
        >
            if props.loading {
                <span class="cui-spinner cui-spinner--inline" aria-hidden="true"></span>
            }
            { props.children.clone() }
        </button>
    }
}
