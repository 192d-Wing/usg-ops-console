use crate::id::use_id;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct SearchInputProps {
    /// Visually hidden `<label>` text.
    pub label: AttrValue,
    #[prop_or_default]
    pub value: AttrValue,
    #[prop_or_default]
    pub placeholder: Option<AttrValue>,
    /// `maxlength` attribute; input is also truncated to this many characters.
    #[prop_or(256)]
    pub max_len: u32,
    #[prop_or_default]
    pub on_change: Callback<AttrValue>,
    /// Enter in the search form; the native submit is always prevented.
    #[prop_or_default]
    pub on_submit: Callback<AttrValue>,
    #[prop_or_default]
    pub test_id: Option<AttrValue>,
}

/// Truncates `s` to at most `max` characters (on a char boundary).
fn truncate(s: &str, max: u32) -> &str {
    match s.char_indices().nth(max as usize) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

// NIST 800-53: SI-10 - input length-bounded (maxlength + truncation)
#[function_component]
pub fn SearchInput(p: &SearchInputProps) -> Html {
    let id = use_id("search");
    let input = use_node_ref();
    let max = p.max_len;

    let oninput = {
        let on_change = p.on_change.clone();
        Callback::from(move |e: InputEvent| {
            let Some(el) = e.target_dyn_into::<HtmlInputElement>() else {
                return;
            };
            let v = el.value();
            let t = truncate(&v, max);
            if t.len() != v.len() {
                el.set_value(t);
            }
            on_change.emit(AttrValue::from(t.to_owned()));
        })
    };
    let onsubmit = {
        let on_submit = p.on_submit.clone();
        let input = input.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let v = input
                .cast::<HtmlInputElement>()
                .map(|el| el.value())
                .unwrap_or_default();
            on_submit.emit(AttrValue::from(truncate(&v, max).to_owned()));
        })
    };

    html! {
        <form role="search" class="cui-search" data-testid={p.test_id.clone()} {onsubmit}>
            <label class="cui-visually-hidden" for={id.clone()}>{ p.label.clone() }</label>
            <span class="cui-search__icon" aria-hidden="true"></span>
            <input
                class="cui-input cui-search__input"
                type="search"
                {id}
                ref={input}
                value={p.value.clone()}
                placeholder={p.placeholder.clone()}
                maxlength={max.to_string()}
                autocomplete="off"
                spellcheck="false"
                {oninput}
            />
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 3), "abc");
        assert_eq!(truncate("\u{e9}\u{e9}\u{e9}", 2), "\u{e9}\u{e9}");
        assert_eq!(truncate("", 0), "");
        assert_eq!(truncate("a", 0), "");
    }
}
