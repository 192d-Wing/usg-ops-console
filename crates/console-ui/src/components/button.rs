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
    /// When disabled or loading, stay focusable: render `aria-disabled="true"`
    /// instead of `disabled` and ignore clicks. Use where the button may hold
    /// focus while it becomes busy (e.g. inside a modal), so focus is not lost
    /// to `<body>` (WCAG 2.4.3). Never set on submit buttons.
    #[prop_or_default]
    pub keep_focus: bool,
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
    let inactive = props.disabled || props.loading;
    let soft = props.keep_focus && !props.submit;
    let onclick = if soft && inactive {
        Callback::from(|e: MouseEvent| e.prevent_default())
    } else {
        props.onclick.clone()
    };
    html! {
        <button
            type={ty}
            {class}
            disabled={inactive && !soft}
            aria-disabled={(inactive && soft).then_some("true")}
            aria-busy={props.loading.then_some("true")}
            aria-label={props.aria_label.clone()}
            data-testid={props.testid.clone()}
            {onclick}
        >
            if props.loading {
                <span class="cui-spinner cui-spinner--inline" aria-hidden="true"></span>
            }
            { props.children.clone() }
        </button>
    }
}
