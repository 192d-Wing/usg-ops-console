use crate::id::use_id;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InputKind {
    #[default]
    Text,
    Search,
    Email,
    Password,
    Number,
    Url,
}

impl InputKind {
    fn as_str(self) -> &'static str {
        match self {
            InputKind::Text => "text",
            InputKind::Search => "search",
            InputKind::Email => "email",
            InputKind::Password => "password",
            InputKind::Number => "number",
            InputKind::Url => "url",
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct InputProps {
    pub value: AttrValue,
    #[prop_or_default]
    pub oninput: Callback<String>,
    #[prop_or_default]
    pub kind: InputKind,
    #[prop_or_default]
    pub disabled: bool,
    #[prop_or_default]
    pub invalid: bool,
    #[prop_or_default]
    pub id: Option<AttrValue>,
    #[prop_or_default]
    pub aria_describedby: Option<AttrValue>,
    /// Use when there is no visible `<label>` (e.g. a search box).
    #[prop_or_default]
    pub aria_label: Option<AttrValue>,
    #[prop_or_default]
    pub placeholder: Option<AttrValue>,
    #[prop_or_default]
    pub name: Option<AttrValue>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
}

#[function_component]
pub fn Input(props: &InputProps) -> Html {
    let oninput = props
        .oninput
        .reform(|e: InputEvent| e.target_unchecked_into::<HtmlInputElement>().value());
    html! {
        <input
            class={classes!("cui-input", props.invalid.then_some("cui-input--invalid"))}
            type={props.kind.as_str()}
            value={props.value.clone()}
            disabled={props.disabled}
            aria-invalid={props.invalid.then_some("true")}
            id={props.id.clone()}
            name={props.name.clone()}
            aria-describedby={props.aria_describedby.clone()}
            aria-label={props.aria_label.clone()}
            placeholder={props.placeholder.clone()}
            data-testid={props.testid.clone()}
            {oninput}
        />
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct SelectOption {
    pub value: AttrValue,
    pub label: AttrValue,
    pub disabled: bool,
}

impl SelectOption {
    pub fn new(value: impl Into<AttrValue>, label: impl Into<AttrValue>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct SelectProps {
    pub options: Vec<SelectOption>,
    #[prop_or_default]
    pub selected: Option<AttrValue>,
    #[prop_or_default]
    pub onchange: Callback<AttrValue>,
    /// Shown as a first, empty-valued option when nothing is selected.
    #[prop_or_default]
    pub placeholder: Option<AttrValue>,
    #[prop_or_default]
    pub disabled: bool,
    #[prop_or_default]
    pub invalid: bool,
    #[prop_or_default]
    pub id: Option<AttrValue>,
    #[prop_or_default]
    pub aria_describedby: Option<AttrValue>,
    #[prop_or_default]
    pub aria_label: Option<AttrValue>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
}

/// Native `<select>`; keyboard and screen-reader behaviour come from the browser.
#[function_component]
pub fn Select(props: &SelectProps) -> Html {
    let onchange = props
        .onchange
        .reform(|e: Event| AttrValue::from(e.target_unchecked_into::<HtmlSelectElement>().value()));
    let sel = props.selected.clone();
    // Controlled value: option `selected` attributes only set the *default*
    // selectedness and stop applying once the user has changed the control,
    // so after every render (options are in the DOM by then) set the `value`
    // property from props. Fixes stale pickers on history Back/Forward.
    let node = use_node_ref();
    {
        let node = node.clone();
        let want = sel
            .clone()
            .or_else(|| props.placeholder.as_ref().map(|_| AttrValue::Static("")));
        use_effect(move || {
            if let (Some(el), Some(v)) = (node.cast::<HtmlSelectElement>(), want)
                && el.value() != v.as_str()
            {
                el.set_value(&v);
            }
        });
    }
    html! {
        <span class="cui-select">
            <select
                ref={node}
                class={classes!("cui-select__control", props.invalid.then_some("cui-input--invalid"))}
                disabled={props.disabled}
                aria-invalid={props.invalid.then_some("true")}
                id={props.id.clone()}
                aria-describedby={props.aria_describedby.clone()}
                aria-label={props.aria_label.clone()}
                data-testid={props.testid.clone()}
                {onchange}
            >
                if let Some(p) = &props.placeholder {
                    <option value="" selected={sel.is_none()} disabled=true>{ p.clone() }</option>
                }
                { for props.options.iter().map(|o| html! {
                    <option
                        key={o.value.to_string()}
                        value={o.value.clone()}
                        disabled={o.disabled}
                        selected={sel.as_ref() == Some(&o.value)}
                    >{ o.label.clone() }</option>
                }) }
            </select>
            <span class="cui-select__arrow" aria-hidden="true"></span>
        </span>
    }
}

#[derive(Properties, PartialEq)]
pub struct CheckboxProps {
    pub checked: bool,
    /// Mixed state (set via the DOM property, which has no HTML attribute).
    #[prop_or_default]
    pub indeterminate: bool,
    #[prop_or_default]
    pub onchange: Callback<bool>,
    #[prop_or_default]
    pub disabled: bool,
    /// Accessible name when there is no visible label (e.g. table rows).
    #[prop_or_default]
    pub aria_label: Option<AttrValue>,
    #[prop_or_default]
    pub aria_describedby: Option<AttrValue>,
    #[prop_or_default]
    pub id: Option<AttrValue>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
    /// Visible label text.
    #[prop_or_default]
    pub children: Html,
}

#[function_component]
pub fn Checkbox(props: &CheckboxProps) -> Html {
    let node = use_node_ref();
    let auto_id = use_id("cb");
    let id = props.id.clone().unwrap_or(auto_id);
    {
        let node = node.clone();
        use_effect_with(props.indeterminate, move |ind| {
            if let Some(el) = node.cast::<HtmlInputElement>() {
                el.set_indeterminate(*ind);
            }
        });
    }
    let onchange = props
        .onchange
        .reform(|e: Event| e.target_unchecked_into::<HtmlInputElement>().checked());
    html! {
        <span class={classes!("cui-checkbox", props.disabled.then_some("cui-checkbox--disabled"))}>
            <input
                ref={node}
                type="checkbox"
                class="cui-checkbox__input"
                id={id.clone()}
                checked={props.checked}
                disabled={props.disabled}
                aria-label={props.aria_label.clone()}
                aria-describedby={props.aria_describedby.clone()}
                data-testid={props.testid.clone()}
                {onchange}
            />
            if props.children != Html::default() {
                <label class="cui-checkbox__label" for={id}>{ props.children.clone() }</label>
            }
        </span>
    }
}

#[derive(Properties, PartialEq)]
pub struct ToggleProps {
    pub checked: bool,
    #[prop_or_default]
    pub onchange: Callback<bool>,
    #[prop_or_default]
    pub disabled: bool,
    #[prop_or_default]
    pub aria_describedby: Option<AttrValue>,
    #[prop_or_default]
    pub id: Option<AttrValue>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
    /// Visible label; it is the switch's accessible name.
    #[prop_or_default]
    pub children: Html,
}

/// On/off switch (`role="switch"`), operable with Space/Enter as a button.
#[function_component]
pub fn Toggle(props: &ToggleProps) -> Html {
    let auto_id = use_id("toggle");
    let id = props.id.clone().unwrap_or(auto_id);
    let label_id = AttrValue::from(format!("{id}-label"));
    let checked = props.checked;
    let onclick = props.onchange.reform(move |_: MouseEvent| !checked);
    html! {
        <span class={classes!("cui-toggle", props.disabled.then_some("cui-toggle--disabled"))}>
            <button
                type="button"
                role="switch"
                class="cui-toggle__switch"
                id={id.clone()}
                aria-checked={if checked { "true" } else { "false" }}
                aria-labelledby={label_id.clone()}
                aria-describedby={props.aria_describedby.clone()}
                disabled={props.disabled}
                data-testid={props.testid.clone()}
                {onclick}
            >
                <span class="cui-toggle__thumb" aria-hidden="true"></span>
            </button>
            <label class="cui-toggle__label" id={label_id} for={id}>{ props.children.clone() }</label>
        </span>
    }
}

/// Ids handed to a [`FormField`]'s render-prop so the control can wire its
/// `id`, `aria-describedby` and `aria-invalid`.
#[derive(Clone, PartialEq, Debug)]
pub struct FieldIds {
    pub control_id: AttrValue,
    /// Space-separated ids of description and error text, if any.
    pub describedby: Option<AttrValue>,
    pub invalid: bool,
}

#[derive(Properties, PartialEq)]
pub struct FormFieldProps {
    pub label: AttrValue,
    #[prop_or_default]
    pub description: Option<AttrValue>,
    #[prop_or_default]
    pub error: Option<AttrValue>,
    /// Optional explicit control id (otherwise generated).
    #[prop_or_default]
    pub id: Option<AttrValue>,
    /// Render-prop: receives ids and returns the control.
    pub children: Callback<FieldIds, Html>,
}

/// Computes the `aria-describedby` value for a field.
pub fn describedby(desc_id: Option<&str>, err_id: Option<&str>) -> Option<AttrValue> {
    let parts: Vec<&str> = [desc_id, err_id].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| AttrValue::from(parts.join(" ")))
}

#[function_component]
pub fn FormField(props: &FormFieldProps) -> Html {
    let auto_id = use_id("field");
    let control_id = props.id.clone().unwrap_or(auto_id);
    let desc_id = format!("{control_id}-desc");
    let err_id = format!("{control_id}-error");
    let ids = FieldIds {
        control_id: control_id.clone(),
        describedby: describedby(
            props.description.as_ref().map(|_| desc_id.as_str()),
            props.error.as_ref().map(|_| err_id.as_str()),
        ),
        invalid: props.error.is_some(),
    };
    html! {
        <div class={classes!("cui-field", props.error.is_some().then_some("cui-field--invalid"))}>
            <label class="cui-field__label" for={control_id}>{ props.label.clone() }</label>
            if let Some(d) = &props.description {
                <p class="cui-field__description" id={desc_id.clone()}>{ d.clone() }</p>
            }
            <div class="cui-field__control">{ props.children.emit(ids) }</div>
            if let Some(e) = &props.error {
                <p class="cui-field__error" id={err_id.clone()}>
                    <span aria-hidden="true">{ "\u{2716} " }</span>
                    <span class="cui-visually-hidden">{ "Error: " }</span>
                    { e.clone() }
                </p>
            }
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describedby_joins() {
        assert_eq!(describedby(None, None), None);
        assert_eq!(describedby(Some("d"), None).as_deref(), Some("d"));
        assert_eq!(describedby(None, Some("e")).as_deref(), Some("e"));
        assert_eq!(describedby(Some("d"), Some("e")).as_deref(), Some("d e"));
    }
}
