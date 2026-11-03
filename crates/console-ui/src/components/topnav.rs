use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct TopNavigationProps {
    /// e.g. `<a href="/">USG Ops Console</a>`.
    pub brand: Html,
    /// After the brand (nav toggle, service menu).
    #[prop_or_default]
    pub start: Html,
    /// Centre slot (global search).
    #[prop_or_default]
    pub search: Html,
    /// End slot (scope, theme, user).
    #[prop_or_default]
    pub utilities: Html,
}

/// Console banner frame. Layout only; the slots carry the behaviour.
#[function_component]
pub fn TopNavigation(p: &TopNavigationProps) -> Html {
    html! {
        <header class="cui-topnav" role="banner" data-testid="top-navigation">
            <div class="cui-topnav__start">
                <div class="cui-topnav__brand">{ p.brand.clone() }</div>
                { p.start.clone() }
            </div>
            <div class="cui-topnav__search">{ p.search.clone() }</div>
            <div class="cui-topnav__utilities">{ p.utilities.clone() }</div>
        </header>
    }
}
