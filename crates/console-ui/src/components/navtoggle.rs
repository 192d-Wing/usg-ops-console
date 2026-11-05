use super::icon::{Icon, IconName};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct NavToggleProps {
    pub expanded: bool,
    /// Id of the navigation region this button shows/hides.
    pub controls: AttrValue,
    pub on_toggle: Callback<()>,
    /// Accessible name (the button shows only an icon).
    #[prop_or(AttrValue::Static("Navigation"))]
    pub label: AttrValue,
    /// Glyph (e.g. `Collapse`/`Expand` for the wide rail, `Menu` for the drawer).
    #[prop_or(IconName::Menu)]
    pub icon: IconName,
}

/// Icon button that toggles a navigation region. Visibility of the button and
/// the region at each breakpoint is the host layout's job.
#[function_component]
pub fn NavToggle(p: &NavToggleProps) -> Html {
    let onclick = p.on_toggle.reform(|_: MouseEvent| ());
    html! {
        <button
            type="button"
            class="cui-navtoggle"
            aria-expanded={if p.expanded { "true" } else { "false" }}
            aria-controls={p.controls.clone()}
            aria-label={p.label.clone()}
            data-testid="nav-toggle"
            {onclick}
        >
            <Icon name={p.icon} />
        </button>
    }
}
