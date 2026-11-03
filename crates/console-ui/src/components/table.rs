use super::form::Checkbox;
use crate::logic::{TriState, select_all_state, toggle_all, toggle_one};
use std::collections::HashSet;
use std::rc::Rc;
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColWidth {
    Xs,
    S,
    M,
    L,
    Auto,
}

impl ColWidth {
    fn class(self) -> &'static str {
        match self {
            ColWidth::Xs => "cui-col--xs",
            ColWidth::S => "cui-col--s",
            ColWidth::M => "cui-col--m",
            ColWidth::L => "cui-col--l",
            ColWidth::Auto => "cui-col--auto",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct TableColumn<T: 'static> {
    pub id: AttrValue,
    pub header: AttrValue,
    pub cell: Callback<T, Html>,
    pub width: Option<ColWidth>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SelectionMode {
    #[default]
    None,
    Multi,
}

#[derive(Properties, PartialEq)]
pub struct TableProps<T: Clone + PartialEq + 'static> {
    pub columns: Vec<TableColumn<T>>,
    pub items: Vec<T>,
    /// Stable row key (also the selection key).
    pub item_key: Callback<T, AttrValue>,
    /// Shown in a full-width row when `items` is empty.
    #[prop_or_default]
    pub empty: Html,
    /// Accessible table name (rendered as a visually hidden caption).
    pub caption: AttrValue,
    #[prop_or_default]
    pub selection: SelectionMode,
    #[prop_or_default]
    pub selected: Vec<AttrValue>,
    #[prop_or_default]
    pub on_selection: Callback<Vec<AttrValue>>,
    /// Column whose cell text names a row for the row checkbox label.
    #[prop_or_default]
    pub row_label: Option<Callback<T, AttrValue>>,
    #[prop_or_default]
    pub testid: Option<AttrValue>,
}

/// Simple data table. No virtualization: intended for up to ~500 rows.
#[function_component]
pub fn Table<T>(props: &TableProps<T>) -> Html
where
    T: Clone + PartialEq + 'static,
{
    let multi = props.selection == SelectionMode::Multi;
    let keys: Rc<Vec<AttrValue>> = Rc::new(
        props
            .items
            .iter()
            .map(|i| props.item_key.emit(i.clone()))
            .collect(),
    );
    // O(1) membership instead of a linear scan per row.
    let selected_set: HashSet<&AttrValue> = props.selected.iter().collect();
    let count = keys.iter().filter(|k| selected_set.contains(k)).count();
    let all_state = select_all_state(count, keys.len());
    let ncols = props.columns.len() + usize::from(multi);

    let on_all = {
        let keys = keys.clone();
        let selected = props.selected.clone();
        props
            .on_selection
            .reform(move |_: bool| toggle_all(&keys, &selected))
    };

    html! {
        <div class="cui-table-wrap" data-testid={props.testid.clone()}>
            <table class="cui-table">
                <caption class="cui-visually-hidden">{ props.caption.clone() }</caption>
                <thead>
                    <tr>
                        if multi {
                            <th scope="col" class="cui-table__select">
                                <Checkbox
                                    checked={all_state == TriState::Checked}
                                    indeterminate={all_state == TriState::Mixed}
                                    disabled={keys.is_empty()}
                                    aria_label="Select all rows"
                                    onchange={on_all}
                                />
                            </th>
                        }
                        { for props.columns.iter().map(|c| html! {
                            <th key={c.id.to_string()} scope="col" class={c.width.map(ColWidth::class)}>
                                { c.header.clone() }
                            </th>
                        }) }
                    </tr>
                </thead>
                <tbody>
                    if props.items.is_empty() {
                        <tr class="cui-table__empty">
                            <td colspan={ncols.to_string()}>{ props.empty.clone() }</td>
                        </tr>
                    } else {
                        { for props.items.iter().zip(keys.iter()).map(|(item, key)| {
                            let is_sel = selected_set.contains(key);
                            let label = props
                                .row_label
                                .as_ref()
                                .map(|f| f.emit(item.clone()))
                                .unwrap_or_else(|| key.clone());
                            let on_row = {
                                let selected = props.selected.clone();
                                let key = key.clone();
                                props.on_selection.reform(move |_: bool| toggle_one(&selected, &key))
                            };
                            html! {
                                <tr key={key.to_string()}
                                    class={classes!(is_sel.then_some("cui-table__row--selected"))}>
                                    if multi {
                                        <td class="cui-table__select">
                                            <Checkbox
                                                checked={is_sel}
                                                aria_label={AttrValue::from(format!("Select {label}"))}
                                                onchange={on_row}
                                            />
                                        </td>
                                    }
                                    { for props.columns.iter().map(|c| html! {
                                        <td key={c.id.to_string()}>{ c.cell.emit(item.clone()) }</td>
                                    }) }
                                </tr>
                            }
                        }) }
                    }
                </tbody>
            </table>
        </div>
    }
}
