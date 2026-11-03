use super::link::{OnFollow, follow_handler, sanitize};
use yew::prelude::*;

#[derive(Clone, PartialEq, Debug)]
pub struct NavLink {
    pub text: AttrValue,
    pub href: AttrValue,
}

impl NavLink {
    pub fn new(text: impl Into<AttrValue>, href: impl Into<AttrValue>) -> Self {
        Self {
            text: text.into(),
            href: href.into(),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum SideNavItem {
    Link(NavLink),
    Section {
        text: AttrValue,
        items: Vec<NavLink>,
    },
    Divider,
}

#[derive(Properties, PartialEq)]
pub struct SideNavigationProps {
    #[prop_or_default]
    pub header: Option<NavLink>,
    pub items: Vec<SideNavItem>,
    #[prop_or_default]
    pub active_href: Option<AttrValue>,
    #[prop_or_default]
    pub onfollow: Option<OnFollow>,
    #[prop_or(AttrValue::Static("Side navigation"))]
    pub aria_label: AttrValue,
}

fn nav_link(l: &NavLink, active: Option<&AttrValue>, onfollow: &Option<OnFollow>) -> Html {
    let href = sanitize(&l.href);
    let is_active = active == Some(&l.href);
    let onclick = follow_handler(href.clone(), onfollow.clone());
    html! {
        <li class="cui-sidenav__item" key={l.href.to_string()}>
            <a
                class={classes!("cui-sidenav__link", is_active.then_some("cui-sidenav__link--active"))}
                {href}
                aria-current={is_active.then_some("page")}
                {onclick}
            >{ l.text.clone() }</a>
        </li>
    }
}

#[function_component]
pub fn SideNavigation(props: &SideNavigationProps) -> Html {
    let active = props.active_href.as_ref();
    html! {
        <nav class="cui-sidenav" aria-label={props.aria_label.clone()} data-testid="side-navigation">
            if let Some(h) = &props.header {
                <div class="cui-sidenav__header">
                    <a class="cui-sidenav__header-link" href={sanitize(&h.href)}
                       onclick={follow_handler(sanitize(&h.href), props.onfollow.clone())}>
                        { h.text.clone() }
                    </a>
                </div>
            }
            <ul class="cui-sidenav__list">
                { for props.items.iter().enumerate().map(|(i, item)| match item {
                    SideNavItem::Link(l) => nav_link(l, active, &props.onfollow),
                    SideNavItem::Section { text, items } => html! {
                        <li class="cui-sidenav__section" key={format!("s{i}")}>
                            <span class="cui-sidenav__section-title">{ text.clone() }</span>
                            <ul class="cui-sidenav__list">
                                { for items.iter().map(|l| nav_link(l, active, &props.onfollow)) }
                            </ul>
                        </li>
                    },
                    // Decorative: a `role` on an <li> would make it a non-listitem
                    // child of the <ul> (axe `list`, WCAG 1.3.1).
                    SideNavItem::Divider => html! {
                        <li class="cui-sidenav__divider" aria-hidden="true" key={format!("d{i}")}></li>
                    },
                }) }
            </ul>
        </nav>
    }
}

#[derive(Properties, PartialEq)]
pub struct BreadcrumbsProps {
    pub items: Vec<NavLink>,
    #[prop_or_default]
    pub onfollow: Option<OnFollow>,
}

/// Breadcrumb trail; the last item is the current page and is not a link.
/// Items with an empty `href` render as plain text (e.g. a section name).
#[function_component]
pub fn Breadcrumbs(props: &BreadcrumbsProps) -> Html {
    let last = props.items.len().saturating_sub(1);
    html! {
        <nav class="cui-breadcrumbs" aria-label="Breadcrumbs">
            <ol class="cui-breadcrumbs__list">
                { for props.items.iter().enumerate().map(|(i, item)| {
                    if i == last {
                        html! {
                            <li class="cui-breadcrumbs__item" key={i}>
                                <span aria-current="page">{ item.text.clone() }</span>
                            </li>
                        }
                    } else if item.href.is_empty() {
                        // Grouping crumb with no page of its own: plain text.
                        html! {
                            <li class="cui-breadcrumbs__item" key={i}>
                                <span>{ item.text.clone() }</span>
                                <span class="cui-breadcrumbs__sep" aria-hidden="true">{ "/" }</span>
                            </li>
                        }
                    } else {
                        let href = sanitize(&item.href);
                        let onclick = follow_handler(href.clone(), props.onfollow.clone());
                        html! {
                            <li class="cui-breadcrumbs__item" key={i}>
                                <a class="cui-link" {href} {onclick}>{ item.text.clone() }</a>
                                <span class="cui-breadcrumbs__sep" aria-hidden="true">{ "/" }</span>
                            </li>
                        }
                    }
                }) }
            </ol>
        </nav>
    }
}
