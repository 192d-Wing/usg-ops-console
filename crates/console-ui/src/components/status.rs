use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BadgeColor {
    #[default]
    Grey,
    Blue,
    Green,
    Red,
    Yellow,
}

#[derive(Properties, PartialEq)]
pub struct BadgeProps {
    #[prop_or_default]
    pub color: BadgeColor,
    #[prop_or_default]
    pub children: Html,
}

#[function_component]
pub fn Badge(props: &BadgeProps) -> Html {
    let m = match props.color {
        BadgeColor::Grey => "cui-badge--grey",
        BadgeColor::Blue => "cui-badge--blue",
        BadgeColor::Green => "cui-badge--green",
        BadgeColor::Red => "cui-badge--red",
        BadgeColor::Yellow => "cui-badge--yellow",
    };
    html! { <span class={classes!("cui-badge", m)}>{ props.children.clone() }</span> }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StatusKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
    Pending,
    Stopped,
}

impl StatusKind {
    pub(crate) fn modifier(self) -> &'static str {
        match self {
            StatusKind::Info => "info",
            StatusKind::Success => "success",
            StatusKind::Warning => "warning",
            StatusKind::Error => "error",
            StatusKind::Pending => "pending",
            StatusKind::Stopped => "stopped",
        }
    }

    /// Decorative glyph; meaning is always also carried by the text label.
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            StatusKind::Info => "\u{2139}",
            StatusKind::Success => "\u{2714}",
            StatusKind::Warning => "\u{26A0}",
            StatusKind::Error => "\u{2716}",
            StatusKind::Pending => "\u{25F7}",
            StatusKind::Stopped => "\u{25A0}",
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct StatusIndicatorProps {
    pub kind: StatusKind,
    #[prop_or_default]
    pub children: Html,
}

/// Icon + text status. Colour is never the only signal (WCAG 1.4.1).
#[function_component]
pub fn StatusIndicator(props: &StatusIndicatorProps) -> Html {
    let class = classes!(
        "cui-status",
        format!("cui-status--{}", props.kind.modifier())
    );
    html! {
        <span {class}>
            <span class="cui-status__icon" aria-hidden="true">{ props.kind.glyph() }</span>
            <span class="cui-status__text">{ props.children.clone() }</span>
        </span>
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SpinnerSize {
    #[default]
    Normal,
    Large,
}

#[derive(Properties, PartialEq)]
pub struct SpinnerProps {
    #[prop_or_default]
    pub size: SpinnerSize,
    /// Accessible text announced to screen readers.
    #[prop_or(AttrValue::Static("Loading"))]
    pub label: AttrValue,
}

#[function_component]
pub fn Spinner(props: &SpinnerProps) -> Html {
    let class = classes!(
        "cui-spinner",
        (props.size == SpinnerSize::Large).then_some("cui-spinner--large")
    );
    html! {
        <span class="cui-spinner-wrap" role="status">
            <span {class} aria-hidden="true"></span>
            <span class="cui-visually-hidden">{ props.label.clone() }</span>
        </span>
    }
}
