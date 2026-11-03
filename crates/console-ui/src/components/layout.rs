use crate::logic::grid_cols;
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SpaceSize {
    Xxs,
    Xs,
    S,
    #[default]
    M,
    L,
    Xl,
}

impl SpaceSize {
    fn as_str(self) -> &'static str {
        match self {
            SpaceSize::Xxs => "xxs",
            SpaceSize::Xs => "xs",
            SpaceSize::S => "s",
            SpaceSize::M => "m",
            SpaceSize::L => "l",
            SpaceSize::Xl => "xl",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Direction {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Properties, PartialEq)]
pub struct SpaceBetweenProps {
    #[prop_or_default]
    pub size: SpaceSize,
    #[prop_or_default]
    pub direction: Direction,
    #[prop_or_default]
    pub children: Html,
}

/// Stacks children with a token-based gap (flex `gap`, no inline styles).
#[function_component]
pub fn SpaceBetween(props: &SpaceBetweenProps) -> Html {
    let class = classes!(
        "cui-space",
        format!("cui-space--{}", props.size.as_str()),
        (props.direction == Direction::Horizontal).then_some("cui-space--horizontal"),
    );
    html! { <div {class}>{ props.children.clone() }</div> }
}

#[derive(Properties, PartialEq)]
pub struct GridProps {
    /// Columns at wide widths; clamped to 1, 2, 3, 4, 6 or 12. Collapses to
    /// one column on narrow viewports.
    #[prop_or(1)]
    pub cols: u8,
    #[prop_or_default]
    pub children: Html,
}

#[function_component]
pub fn Grid(props: &GridProps) -> Html {
    let class = classes!(
        "cui-grid",
        format!("cui-grid--cols-{}", grid_cols(props.cols))
    );
    html! { <div {class}>{ props.children.clone() }</div> }
}
