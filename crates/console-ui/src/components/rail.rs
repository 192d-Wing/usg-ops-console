use super::icon::{Icon, IconName};
use super::link::{OnFollow, follow_handler, sanitize};
use super::nav::NavLink;
use web_sys::wasm_bindgen::JsCast;
use web_sys::wasm_bindgen::closure::Closure;
use yew::prelude::*;

/// One rail link. `icon: None` renders a two-letter monogram of `text`.
#[derive(Clone, PartialEq, Debug)]
pub struct RailItem {
    pub text: AttrValue,
    pub href: AttrValue,
    pub icon: Option<IconName>,
    pub testid: Option<AttrValue>,
    /// A sub-page of the current tool (shown as a child, not a peer tool,
    /// in the collapsed rail; A11Y-003).
    pub sub: bool,
}

impl RailItem {
    pub fn new(text: impl Into<AttrValue>, href: impl Into<AttrValue>) -> Self {
        Self {
            text: text.into(),
            href: href.into(),
            icon: None,
            testid: None,
            sub: false,
        }
    }

    pub fn sub_page(mut self) -> Self {
        self.sub = true;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn testid(mut self, id: impl Into<AttrValue>) -> Self {
        self.testid = Some(id.into());
        self
    }
}

impl From<NavLink> for RailItem {
    fn from(l: NavLink) -> Self {
        RailItem::new(l.text, l.href)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum RailEntry {
    Link(RailItem),
    Section {
        text: AttrValue,
        items: Vec<RailItem>,
    },
    Divider,
}

#[derive(Properties, PartialEq)]
pub struct RailProps {
    pub items: Vec<RailEntry>,
    #[prop_or_default]
    pub active_href: Option<AttrValue>,
    #[prop_or_default]
    pub onfollow: Option<OnFollow>,
    /// Icon-only presentation (wide viewport, user collapsed the rail).
    #[prop_or_default]
    pub collapsed: bool,
    #[prop_or(AttrValue::Static("Console"))]
    pub aria_label: AttrValue,
}

/// Two-letter monogram: initials of the first two words, else the first two
/// characters. Rendered as a text node (escaped by Yew).
pub fn monogram(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let chars: String = if words.len() >= 2 {
        words
            .iter()
            .take(2)
            .filter_map(|w| w.chars().next())
            .collect()
    } else {
        text.trim().chars().take(2).collect()
    };
    chars.to_uppercase()
}

fn rail_link(
    item: &RailItem,
    active: Option<&AttrValue>,
    p: &RailProps,
    undismiss: &Callback<MouseEvent>,
) -> Html {
    let href = sanitize(&item.href);
    let is_active = active == Some(&item.href);
    let onclick = follow_handler(href.clone(), p.onfollow.clone());
    let glyph = match item.icon {
        Some(name) => html! { <Icon {name} /> },
        None => html! {
            <span class="cui-rail__monogram" aria-hidden="true">{ monogram(&item.text) }</span>
        },
    };
    html! {
        <li class="cui-rail__item" key={item.href.to_string()}>
            <a
                class={classes!("cui-rail__link", is_active.then_some("cui-rail__link--active"),
                    item.sub.then_some("cui-rail__link--sub"))}
                {href}
                aria-current={is_active.then_some("page")}
                data-label={item.text.clone()}
                data-testid={item.testid.clone()}
                {onclick}
                onmouseleave={undismiss.clone()}
            >
                { glyph }
                <span class={classes!("cui-rail__label", p.collapsed.then_some("cui-visually-hidden"))}>
                    { item.text.clone() }
                </span>
                if p.collapsed {
                    <span class="cui-rail__tip" aria-hidden="true">{ item.text.clone() }</span>
                }
            </a>
        </li>
    }
}

/// Collapsible icon rail. Expanded: icon + label. Collapsed: icon only, the
/// label stays in the DOM (visually hidden) as the accessible name, and an
/// `aria-hidden` tooltip shows on hover/focus. Escape anywhere in the
/// document hides it until the pointer leaves the link or focus moves
/// (WCAG 1.4.13).
#[function_component]
pub fn Rail(p: &RailProps) -> Html {
    let dismissed = use_state_eq(|| false);
    {
        let set = dismissed.setter();
        use_effect_with(p.collapsed, move |collapsed| {
            set.set(false);
            let doc = web_sys::window().and_then(|w| w.document());
            let listener = (*collapsed).then(|| {
                Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                    if e.key() == "Escape" {
                        set.set(true);
                    }
                })
            });
            if let (Some(d), Some(l)) = (&doc, &listener) {
                let _ = d.add_event_listener_with_callback("keydown", l.as_ref().unchecked_ref());
            }
            move || {
                if let (Some(d), Some(l)) = (doc, listener) {
                    let _ = d
                        .remove_event_listener_with_callback("keydown", l.as_ref().unchecked_ref());
                }
            }
        });
    }
    let undismiss = {
        let set = dismissed.setter();
        Callback::from(move |_: MouseEvent| set.set(false))
    };
    let onfocusout = {
        let set = dismissed.setter();
        Callback::from(move |_: FocusEvent| set.set(false))
    };
    let active = p.active_href.as_ref();
    let class = classes!("cui-rail", p.collapsed.then_some("cui-rail--collapsed"));
    html! {
        <nav {class} aria-label={p.aria_label.clone()} data-testid="side-navigation"
            data-tip-dismissed={(p.collapsed && *dismissed).then_some("")} {onfocusout}>
            <ul class="cui-rail__list">
                { for p.items.iter().enumerate().map(|(i, e)| match e {
                    RailEntry::Link(item) => rail_link(item, active, p, &undismiss),
                    RailEntry::Section { text, items } => html! {
                        <li class="cui-rail__section" key={format!("s{i}")}>
                            <span class={classes!("cui-rail__section-title",
                                p.collapsed.then_some("cui-visually-hidden"))}>{ text.clone() }</span>
                            <ul class="cui-rail__list">
                                { for items.iter().map(|it| rail_link(it, active, p, &undismiss)) }
                            </ul>
                        </li>
                    },
                    RailEntry::Divider => html! {
                        <li class="cui-rail__divider" aria-hidden="true" key={format!("d{i}")}></li>
                    },
                }) }
            </ul>
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use super::monogram;

    #[test]
    fn monograms() {
        assert_eq!(monogram("Hello tool"), "HT");
        assert_eq!(monogram("hello"), "HE");
        assert_eq!(monogram("x"), "X");
        assert_eq!(monogram("  "), "");
        assert_eq!(monogram("<b> x"), "<X");
        assert_eq!(monogram("élan vital"), "ÉV");
    }
}
