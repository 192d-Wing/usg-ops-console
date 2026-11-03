use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct ContainerProps {
    /// Typically a [`crate::Header`] with `HeaderVariant::H2`.
    #[prop_or_default]
    pub header: Option<Html>,
    #[prop_or_default]
    pub footer: Option<Html>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
    #[prop_or_default]
    pub children: Html,
}

/// Bordered content panel with optional header and footer.
#[function_component]
pub fn Container(props: &ContainerProps) -> Html {
    html! {
        <section class="cui-container" data-testid={props.testid.clone()}>
            if let Some(h) = &props.header {
                <div class="cui-container__header">{ h.clone() }</div>
            }
            <div class="cui-container__body">{ props.children.clone() }</div>
            if let Some(f) = &props.footer {
                <div class="cui-container__footer">{ f.clone() }</div>
            }
        </section>
    }
}
