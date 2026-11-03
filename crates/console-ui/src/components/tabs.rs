use crate::id::use_id;
use crate::logic::{nav_key, roving_index};
use web_sys::HtmlElement;
use web_sys::wasm_bindgen::JsCast;
use yew::prelude::*;

#[derive(Clone, PartialEq)]
pub struct TabDef {
    pub id: AttrValue,
    pub label: AttrValue,
    pub content: Html,
    pub disabled: bool,
}

impl TabDef {
    pub fn new(id: impl Into<AttrValue>, label: impl Into<AttrValue>, content: Html) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            content,
            disabled: false,
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct TabsProps {
    pub tabs: Vec<TabDef>,
    /// Id of the active tab.
    pub active: AttrValue,
    pub onchange: Callback<AttrValue>,
    /// Accessible name of the tab list.
    #[prop_or(AttrValue::Static("Tabs"))]
    pub aria_label: AttrValue,
}

/// WAI-ARIA tabs with automatic activation: Left/Right (wrapping), Home and
/// End move focus and select; only the active tab is in the tab order.
#[function_component]
pub fn Tabs(props: &TabsProps) -> Html {
    let base = use_id("tabs");
    let list = use_node_ref();
    let enabled: Vec<bool> = props.tabs.iter().map(|t| !t.disabled).collect();
    let active_idx = props
        .tabs
        .iter()
        .position(|t| t.id == props.active)
        .unwrap_or(0);

    let onkeydown = {
        let list = list.clone();
        let ids: Vec<AttrValue> = props.tabs.iter().map(|t| t.id.clone()).collect();
        let onchange = props.onchange.clone();
        let enabled = enabled.clone();
        Callback::from(move |e: KeyboardEvent| {
            let Some(k) = nav_key(&e.key()) else { return };
            let Some(next) = roving_index(active_idx, &enabled, k) else {
                return;
            };
            e.prevent_default();
            if let Some(el) = list
                .cast::<web_sys::Element>()
                .and_then(|l| l.query_selector_all("[role=tab]").ok())
                .and_then(|nl| nl.item(next as u32))
                .and_then(|n| n.dyn_into::<HtmlElement>().ok())
            {
                // Focus failure is non-fatal: selection still changes.
                let _ = el.focus();
            }
            if next != active_idx {
                onchange.emit(ids[next].clone());
            }
        })
    };

    html! {
        <div class="cui-tabs">
            <div class="cui-tabs__list" role="tablist" aria-label={props.aria_label.clone()} ref={list} {onkeydown}>
                { for props.tabs.iter().enumerate().map(|(i, t)| {
                    let sel = i == active_idx;
                    let id = t.id.clone();
                    let onclick = props.onchange.reform(move |_: MouseEvent| id.clone());
                    html! {
                        <button
                            key={t.id.to_string()}
                            type="button"
                            role="tab"
                            class={classes!("cui-tabs__tab", sel.then_some("cui-tabs__tab--active"))}
                            id={format!("{base}-tab-{i}")}
                            aria-selected={if sel { "true" } else { "false" }}
                            aria-controls={sel.then(|| format!("{base}-panel-{i}"))}
                            tabindex={if sel { "0" } else { "-1" }}
                            disabled={t.disabled}
                            data-testid={format!("tab-{}", t.id)}
                            {onclick}
                        >{ t.label.clone() }</button>
                    }
                }) }
            </div>
            if let Some(t) = props.tabs.get(active_idx) {
                <div
                    class="cui-tabs__panel"
                    role="tabpanel"
                    id={format!("{base}-panel-{active_idx}")}
                    aria-labelledby={format!("{base}-tab-{active_idx}")}
                    tabindex="0"
                >{ t.content.clone() }</div>
            }
        </div>
    }
}
