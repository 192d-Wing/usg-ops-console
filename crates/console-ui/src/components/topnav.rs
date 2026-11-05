use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct TopNavigationProps {
    /// e.g. `<a href="/">USG Ops Console</a>`.
    pub brand: Html,
    /// Before the brand (nav / rail toggle).
    #[prop_or_default]
    pub start: Html,
    /// Grows to fill (command palette trigger).
    #[prop_or_default]
    pub search: Html,
    /// Scope switcher (breadcrumb segments).
    #[prop_or_default]
    pub scope: Html,
    /// End slot (theme, user).
    #[prop_or_default]
    pub utilities: Html,
}

/// Console banner frame: `[start][brand][search ...][scope][utilities]`. Layout only; the slots carry the behaviour.
#[function_component]
pub fn TopNavigation(p: &TopNavigationProps) -> Html {
    html! {
        <header class="cui-topnav" role="banner" data-testid="top-navigation">
            <div class="cui-topnav__start">
                { p.start.clone() }
                <div class="cui-topnav__brand">{ p.brand.clone() }</div>
            </div>
            <div class="cui-topnav__search">{ p.search.clone() }</div>
            <div class="cui-topnav__scope">{ p.scope.clone() }</div>
            <div class="cui-topnav__utilities">{ p.utilities.clone() }</div>
        </header>
    }
}
