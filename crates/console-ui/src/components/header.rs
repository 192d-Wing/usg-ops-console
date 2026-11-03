use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HeaderVariant {
    H1,
    #[default]
    H2,
}

#[derive(Properties, PartialEq)]
pub struct HeaderProps {
    #[prop_or_default]
    pub variant: HeaderVariant,
    #[prop_or_default]
    pub description: Option<AttrValue>,
    #[prop_or_default]
    pub actions: Option<Html>,
    /// Optional id on the heading element, for `aria-labelledby`.
    #[prop_or_default]
    pub id: Option<AttrValue>,
    #[prop_or_default]
    pub children: Html,
}

/// Page (h1) or section (h2) heading with description and action slot.
#[function_component]
pub fn Header(props: &HeaderProps) -> Html {
    let title = props.children.clone();
    let id = props.id.clone();
    let heading = match props.variant {
        HeaderVariant::H1 => {
            html! { <h1 class="cui-header__title cui-header__title--h1" {id}>{ title }</h1> }
        }
        HeaderVariant::H2 => {
            html! { <h2 class="cui-header__title cui-header__title--h2" {id}>{ title }</h2> }
        }
    };
    html! {
        <div class="cui-header">
            <div class="cui-header__main">
                { heading }
                if let Some(d) = &props.description {
                    <p class="cui-header__description">{ d.clone() }</p>
                }
            </div>
            if let Some(a) = &props.actions {
                <div class="cui-header__actions">{ a.clone() }</div>
            }
        </div>
    }
}
