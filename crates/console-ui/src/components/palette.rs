use super::icon::{Icon, IconName};
use super::modal::{active_element, set_inert};
use crate::id::use_id;
use crate::logic::{FoldedItem, fold_palette, highlight_runs, rank_folded};
use std::rc::Rc;
use web_sys::wasm_bindgen::JsCast;
use web_sys::{HtmlElement, HtmlInputElement};
use yew::prelude::*;

/// Maximum query length (NIST 800-53 SI-10: bounded input).
pub const PALETTE_MAX_QUERY: u32 = 256;
/// DOM id of the results listbox (referenced by the combobox).
pub const PALETTE_LIST_ID: &str = "palette-list";

/// Result groups, in tie-break / display order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
pub enum PaletteGroup {
    Navigation,
    Tools,
    Scope,
    Preferences,
    Account,
}

impl PaletteGroup {
    pub fn label(self) -> &'static str {
        match self {
            PaletteGroup::Navigation => "Navigation",
            PaletteGroup::Tools => "Tools",
            PaletteGroup::Scope => "Scope",
            PaletteGroup::Preferences => "Preferences",
            PaletteGroup::Account => "Account",
        }
    }

    fn order(self) -> u8 {
        self as u8
    }
}

/// One palette entry. `id` is opaque to the component and is emitted by
/// `on_select`; the host maps it to a closed set of actions.
#[derive(Clone, PartialEq, Debug)]
pub struct PaletteItem {
    pub id: AttrValue,
    pub label: AttrValue,
    pub group: PaletteGroup,
    pub keywords: Vec<AttrValue>,
    pub hint: Option<AttrValue>,
    /// Informational row (e.g. "Tools are loading"): shown, never selectable.
    pub disabled: bool,
}

impl PaletteItem {
    pub fn new(id: impl Into<AttrValue>, label: impl Into<AttrValue>, group: PaletteGroup) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            group,
            keywords: Vec::new(),
            hint: None,
            disabled: false,
        }
    }

    pub fn keywords<I, S>(mut self, kw: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<AttrValue>,
    {
        self.keywords = kw.into_iter().map(Into::into).collect();
        self
    }

    pub fn hint(mut self, hint: impl Into<AttrValue>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Properties, PartialEq)]
pub struct CommandPaletteProps {
    /// Nothing is rendered while false.
    pub open: bool,
    pub items: Rc<Vec<PaletteItem>>,
    /// Item id. The host closes the palette (sets `open = false`).
    pub on_select: Callback<AttrValue>,
    /// Escape or scrim click.
    pub on_close: Callback<()>,
    /// Background region to mark `inert` while open (as [`crate::Modal`]).
    #[prop_or_default]
    pub inert_root: Option<AttrValue>,
    /// Element ids tried in order when the opener is gone on close.
    #[prop_or_default]
    pub fallback_focus: Vec<AttrValue>,
    #[prop_or_default]
    pub on_error: Callback<AttrValue>,
}

/// Case-folded labels/keywords, computed once per `items` (PERF-003).
fn fold_items(items: &[PaletteItem]) -> Vec<FoldedItem> {
    let kws: Vec<Vec<&str>> = items
        .iter()
        .map(|i| i.keywords.iter().map(|k| k.as_str()).collect())
        .collect();
    let input: Vec<(&str, &[&str], u8)> = items
        .iter()
        .zip(&kws)
        .map(|(i, k)| (i.label.as_str(), k.as_slice(), i.group.order()))
        .collect();
    fold_palette(&input)
}

/// Visible rows: indices into `items`, in display order (grouped; groups in
/// order of their best result, items in rank order).
fn visible_rows(
    items: &[PaletteItem],
    folded: &[FoldedItem],
    query: &str,
) -> Vec<(usize, Option<(usize, usize)>)> {
    let ranked = rank_folded(query, folded);
    let mut groups: Vec<PaletteGroup> = Vec::new();
    for r in &ranked {
        let g = items[r.index].group;
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    groups
        .iter()
        .flat_map(|g| {
            ranked
                .iter()
                .filter(move |r| items[r.index].group == *g)
                .map(|r| (r.index, r.highlight))
        })
        .collect()
}

/// Next enabled row from `cur` by `delta` (wrapping), or first/last.
fn step(enabled: &[bool], cur: Option<usize>, key: &str) -> Option<usize> {
    let len = enabled.len();
    let first = enabled.iter().position(|e| *e)?;
    let last = enabled.iter().rposition(|e| *e)?;
    match key {
        "Home" => Some(first),
        "End" => Some(last),
        "ArrowDown" | "ArrowUp" => {
            let down = key == "ArrowDown";
            let Some(c) = cur else {
                return Some(if down { first } else { last });
            };
            (1..=len)
                .map(|s| {
                    if down {
                        (c + s) % len
                    } else {
                        (c + len - s % len) % len
                    }
                })
                .find(|i| enabled[*i])
        }
        _ => cur,
    }
}

fn status_text(n: usize) -> String {
    match n {
        0 => "No results".to_owned(),
        1 => "1 result".to_owned(),
        n => format!("{n} results"),
    }
}

fn focus_by_id(id: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        .map(|e| {
            let _ = e.focus();
            true
        })
        .unwrap_or(false)
}

/// Command palette: modal dialog with an APG combobox + listbox. Filtering is
/// local (no network); labels and the query render as text nodes only.
#[function_component]
pub fn CommandPalette(p: &CommandPaletteProps) -> Html {
    let base = use_id("palette");
    let input = use_node_ref();
    let query = use_state(String::new);
    let active = use_state(|| None::<usize>);
    let status = use_state(String::new);
    let gen_ = use_mut_ref(|| 0u64);
    let opener = use_mut_ref(|| None::<HtmlElement>);

    // Folding runs only when `items` changes; each keystroke just ranks.
    let folded = use_memo(p.items.clone(), |items| fold_items(items));
    let rows = {
        let items = p.items.clone();
        let folded = folded.clone();
        use_memo(((*query).clone(), items), move |(q, items)| {
            visible_rows(items, &folded, q)
        })
    };
    let enabled: Vec<bool> = rows.iter().map(|(i, _)| !p.items[*i].disabled).collect();

    // Open/close lifecycle: reset, inert, focus; restore focus on close.
    {
        let input = input.clone();
        let opener = opener.clone();
        let query = query.clone();
        let active = active.clone();
        let inert_root = p.inert_root.clone();
        let fallback = p.fallback_focus.clone();
        let on_error = p.on_error.clone();
        use_effect_with(p.open, move |open| {
            let open = *open;
            if open {
                query.set(String::new());
                active.set(None);
                // `body` (or nothing) means "no opener": macOS WebKit does not
                // focus buttons on click, so fall back to `fallback_focus` (D5).
                let body = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.body());
                *opener.borrow_mut() = active_element()
                    .filter(|e| {
                        body.as_ref()
                            .is_none_or(|b| e != b.unchecked_ref::<web_sys::Element>())
                    })
                    .and_then(|e| e.dyn_into::<HtmlElement>().ok());
                if let Some(id) = &inert_root
                    && let Err(e) = set_inert(id, true)
                {
                    on_error.emit(e);
                }
                if let Some(el) = input.cast::<HtmlElement>() {
                    let _ = el.focus();
                }
            }
            move || {
                if !open {
                    return;
                }
                if let Some(id) = &inert_root
                    && let Err(e) = set_inert(id, false)
                {
                    on_error.emit(e);
                }
                let opener = opener.borrow_mut().take();
                // Deferred: if the host moved focus on select (e.g. opened a
                // scope popover), leave it there.
                yew::platform::spawn_local(async move {
                    let doc = web_sys::window().and_then(|w| w.document());
                    let body = doc.as_ref().and_then(|d| d.body());
                    let cur = active_element();
                    let taken = match (&cur, &body) {
                        (Some(c), Some(b)) => c != b.unchecked_ref::<web_sys::Element>(),
                        (Some(_), None) => true,
                        (None, _) => false,
                    };
                    if taken {
                        return;
                    }
                    if let Some(el) = opener.filter(|e| e.is_connected()) {
                        let _ = el.focus();
                        return;
                    }
                    for id in &fallback {
                        if focus_by_id(id) {
                            return;
                        }
                    }
                });
            }
        });
    }

    // Debounced (~300 ms) polite result count.
    {
        let status = status.clone();
        let gen_ = gen_.clone();
        let n = rows.len();
        let open = p.open;
        use_effect_with((n, (*query).clone(), open), move |_| {
            *gen_.borrow_mut() += 1;
            let mine = *gen_.borrow();
            if open {
                yew::platform::spawn_local(async move {
                    yew::platform::time::sleep(std::time::Duration::from_millis(300)).await;
                    if *gen_.borrow() == mine {
                        status.set(status_text(n));
                    }
                });
            }
        });
    }

    // Keep the active option scrolled into view.
    {
        let base = base.clone();
        let rows = rows.clone();
        use_effect_with(*active, move |a| {
            if let Some(i) = a
                && let Some((idx, _)) = rows.get(*i)
                && let Some(el) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.get_element_by_id(&format!("{base}-o{idx}")))
            {
                el.scroll_into_view_with_bool(false);
            }
        });
    }

    if !p.open {
        return html! {};
    }

    let oninput = {
        let query = query.clone();
        let active = active.clone();
        Callback::from(move |e: InputEvent| {
            let Some(el) = e.target_dyn_into::<HtmlInputElement>() else {
                return;
            };
            let v = el.value();
            let t: String = v.chars().take(PALETTE_MAX_QUERY as usize).collect();
            if t.len() != v.len() {
                el.set_value(&t);
            }
            active.set(None);
            query.set(t);
        })
    };

    let select = {
        let items = p.items.clone();
        let rows = rows.clone();
        let on_select = p.on_select.clone();
        let inert_root = p.inert_root.clone();
        let on_error = p.on_error.clone();
        Callback::from(move |row: usize| {
            if let Some((idx, _)) = rows.get(row)
                && let Some(it) = items.get(*idx)
                && !it.disabled
            {
                // Lift `inert` before the host acts, so focus the action
                // moves (e.g. into a scope popover) lands and the deferred
                // restore sees it and stays out of the way (D2).
                if let Some(id) = &inert_root
                    && let Err(e) = set_inert(id, false)
                {
                    on_error.emit(e);
                }
                on_select.emit(it.id.clone());
            }
        })
    };

    let onkeydown = {
        let active = active.clone();
        let enabled = enabled.clone();
        let select = select.clone();
        let on_close = p.on_close.clone();
        let query = query.clone();
        Callback::from(move |e: KeyboardEvent| {
            let key = e.key();
            match key.as_str() {
                "Escape" => {
                    e.prevent_default();
                    e.stop_propagation();
                    on_close.emit(());
                }
                "Tab" => e.prevent_default(), // only the input is focusable
                "ArrowDown" | "ArrowUp" => {
                    e.prevent_default();
                    active.set(step(&enabled, *active, &key));
                }
                // Home/End move the option only while one is active, so the
                // caret keys keep working while typing.
                "Home" | "End" if active.is_some() || query.is_empty() => {
                    e.prevent_default();
                    active.set(step(&enabled, *active, &key));
                }
                "Enter" => {
                    e.prevent_default();
                    let target = (*active).or_else(|| step(&enabled, None, "Home"));
                    if let Some(a) = target {
                        select.emit(a);
                    }
                }
                _ => {}
            }
        })
    };

    let on_scrim = p.on_close.reform(|_: MouseEvent| ());
    let keep_focus = Callback::from(|e: MouseEvent| e.prevent_default());
    let active_id = (*active)
        .and_then(|a| rows.get(a))
        .map(|(idx, _)| AttrValue::from(format!("{base}-o{idx}")));
    let empty = rows.is_empty();

    // Build grouped rows.
    let mut body: Vec<Html> = Vec::new();
    let mut row = 0usize;
    while row < rows.len() {
        let group = p.items[rows[row].0].group;
        let gid = format!("{base}-g{}", group.order());
        let mut opts = Vec::new();
        while row < rows.len() && p.items[rows[row].0].group == group {
            let (idx, hl) = rows[row];
            let it = &p.items[idx];
            let is_active = *active == Some(row);
            let onclick = {
                let select = select.clone();
                Callback::from(move |_: MouseEvent| select.emit(row))
            };
            let onmousemove = {
                let active = active.clone();
                let disabled = it.disabled;
                Callback::from(move |_: MouseEvent| {
                    if !disabled && *active != Some(row) {
                        active.set(Some(row));
                    }
                })
            };
            opts.push(html! {
                <div
                    key={it.id.to_string()}
                    id={format!("{base}-o{idx}")}
                    class={classes!("cui-palette__option",
                        is_active.then_some("cui-palette__option--active"),
                        it.disabled.then_some("cui-palette__option--disabled"))}
                    role="option"
                    aria-selected={if is_active { "true" } else { "false" }}
                    aria-disabled={it.disabled.then_some("true")}
                    data-testid={format!("palette-option-{}", it.id)}
                    {onclick}
                    {onmousemove}
                >
                    <span class="cui-palette__label">
                        { for highlight_runs(&it.label, hl).into_iter().map(|(t, m)| {
                            if m { html! { <mark class="cui-palette__mark">{ t.to_owned() }</mark> } }
                            else { html! { { t.to_owned() } } }
                        }) }
                    </span>
                    if let Some(h) = &it.hint {
                        <span class="cui-palette__hint">{ h.clone() }</span>
                    }
                </div>
            });
            row += 1;
        }
        body.push(html! {
            <div class="cui-palette__group" role="group" aria-labelledby={gid.clone()} key={gid.clone()}>
                <div class="cui-palette__group-title" id={gid} role="presentation">{ group.label() }</div>
                { for opts }
            </div>
        });
    }

    html! {
        <div class="cui-palette" data-testid="command-palette">
            <div class="cui-palette__scrim" aria-hidden="true" onclick={on_scrim}></div>
            <div class="cui-palette__dialog" role="dialog" aria-modal="true"
                 aria-label="Command palette" {onkeydown}>
                <div class="cui-palette__field">
                    <Icon name={IconName::Search} />
                    <input
                        ref={input}
                        class="cui-palette__input"
                        type="text"
                        role="combobox"
                        aria-label="Search commands, pages and tools"
                        aria-expanded={if empty { "false" } else { "true" }}
                        aria-controls={PALETTE_LIST_ID}
                        aria-autocomplete="list"
                        aria-activedescendant={active_id}
                        maxlength={PALETTE_MAX_QUERY.to_string()}
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="Search…"
                        value={(*query).clone()}
                        data-testid="palette-input"
                        {oninput}
                    />
                    <kbd class="cui-kbd" aria-hidden="true">{ "Esc" }</kbd>
                </div>
                <div
                    class="cui-palette__list"
                    id={PALETTE_LIST_ID}
                    role="listbox"
                    aria-label="Results"
                    hidden={empty}
                    data-testid="palette-list"
                    onmousedown={keep_focus}
                >
                    { for body }
                </div>
                if empty {
                    <p class="cui-palette__empty" data-testid="palette-empty">
                        { format!("No results for \u{201c}{}\u{201d}", *query) }
                    </p>
                }
                <div class="cui-visually-hidden" role="status" aria-live="polite"
                     data-testid="palette-status">{ (*status).clone() }</div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_wraps_and_skips_disabled() {
        let e = [true, false, true];
        assert_eq!(step(&e, None, "ArrowDown"), Some(0));
        assert_eq!(step(&e, None, "ArrowUp"), Some(2));
        assert_eq!(step(&e, Some(0), "ArrowDown"), Some(2));
        assert_eq!(step(&e, Some(2), "ArrowDown"), Some(0));
        assert_eq!(step(&e, Some(0), "ArrowUp"), Some(2));
        assert_eq!(step(&e, Some(2), "Home"), Some(0));
        assert_eq!(step(&e, Some(0), "End"), Some(2));
        assert_eq!(step(&[false], None, "ArrowDown"), None);
        assert_eq!(step(&[], None, "Home"), None);
    }

    #[test]
    fn rows_grouped_in_order() {
        let items = vec![
            PaletteItem::new("signout", "Sign out", PaletteGroup::Account),
            PaletteItem::new("home", "Home", PaletteGroup::Navigation),
            PaletteItem::new("t-h", "Hello tool", PaletteGroup::Tools),
        ];
        let f = fold_items(&items);
        let r: Vec<usize> = visible_rows(&items, &f, "").iter().map(|x| x.0).collect();
        assert_eq!(r, vec![1, 2, 0]);
        let r: Vec<usize> = visible_rows(&items, &f, "o").iter().map(|x| x.0).collect();
        assert_eq!(r.len(), 3);
        assert!(visible_rows(&items, &f, "zzz").is_empty());
        assert_eq!(status_text(0), "No results");
        assert_eq!(status_text(1), "1 result");
        assert_eq!(status_text(7), "7 results");
    }
}
