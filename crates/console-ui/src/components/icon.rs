use yew::prelude::*;

/// Fixed inline-SVG icon set (24×24 grid, 1.75 stroke). Colour comes from
/// `currentColor` via `.cui-icon`, so icons follow text colour, themes and
/// forced-colors. No `style` attribute is ever emitted (CSP `style-src 'self'`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum IconName {
    Home,
    Tool,
    Grid,
    Gallery,
    Settings,
    Search,
    Sun,
    Moon,
    Monitor,
    User,
    ChevronRight,
    ChevronDown,
    /// "Collapse navigation" (panel with arrow pointing left).
    Collapse,
    /// "Expand navigation" (panel with arrow pointing right).
    Expand,
    Menu,
    Close,
    SignOut,
    /// Product mark.
    Mark,
}

impl IconName {
    /// SVG path data (stroked). Static strings keep the wasm small.
    pub fn path(self) -> &'static str {
        match self {
            IconName::Home => "M3 10.5 12 3l9 7.5M5.5 8.5V20h5v-6h3v6h5V8.5",
            IconName::Tool => {
                "M14.5 5.5a4 4 0 0 0 5 5L12 18l-1.5 1.5a2.1 2.1 0 0 1-3-3L9 15l7.5-7.5a4 4 0 0 0-2-2Z"
            }
            IconName::Grid => "M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z",
            IconName::Gallery => "M4 5h16v14H4zM4 15l4.5-4.5 4 4 2.5-2.5L20 17M15.5 9.5h.01",
            IconName::Settings => {
                "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6ZM19 12a7 7 0 0 0-.1-1.2l2-1.6-2-3.4-2.4.9a7 7 0 0 0-2-1.2L14 3h-4l-.5 2.5a7 7 0 0 0-2 1.2l-2.4-.9-2 3.4 2 1.6a7 7 0 0 0 0 2.4l-2 1.6 2 3.4 2.4-.9a7 7 0 0 0 2 1.2L10 21h4l.5-2.5a7 7 0 0 0 2-1.2l2.4.9 2-3.4-2-1.6c.07-.4.1-.8.1-1.2Z"
            }
            IconName::Search => "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14ZM16 16l4.5 4.5",
            IconName::Sun => {
                "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"
            }
            IconName::Moon => "M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5Z",
            IconName::Monitor => "M3 4h18v12H3zM8 20h8M12 16v4",
            IconName::User => "M12 4a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM4 20a8 8 0 0 1 16 0",
            IconName::ChevronRight => "M9 6l6 6-6 6",
            IconName::ChevronDown => "M6 9l6 6 6-6",
            IconName::Collapse => "M4 4h16v16H4zM9 4v16M16 9l-3 3 3 3",
            IconName::Expand => "M4 4h16v16H4zM9 4v16M13 9l3 3-3 3",
            IconName::Menu => "M4 7h16M4 12h16M4 17h16",
            IconName::Close => "M6 6l12 12M18 6 6 18",
            IconName::SignOut => "M10 4H5v16h5M15 8l4 4-4 4M19 12H9",
            IconName::Mark => {
                "M12 3 20 7.5v9L12 21l-8-4.5v-9L12 3ZM12 12l8-4.5M12 12v9M12 12 4 7.5"
            }
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct IconProps {
    pub name: IconName,
    /// Extra classes (e.g. `cui-icon--s`).
    #[prop_or_default]
    pub class: Classes,
}

/// Decorative icon. Always `aria-hidden`; give the parent an accessible name.
#[function_component]
pub fn Icon(p: &IconProps) -> Html {
    html! {
        <svg
            class={classes!("cui-icon", p.class.clone())}
            viewBox="0 0 24 24"
            aria-hidden="true"
            focusable="false"
        >
            <path d={p.name.path()} />
        </svg>
    }
}
