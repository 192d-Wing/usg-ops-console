//! Console chrome pieces composed into the top navigation: service menu,
//! global search, user menu and the cascading scope picker.

use crate::ctx::ScopeCtx;
use crate::demo::{self, DEMO_USER, SERVICES, SITES};
use crate::scope::{self, find_site, find_tenant};
use console_ui::{
    AlertKind, Dropdown, DropdownItem, FieldIds, FormField, OnFollow, SearchInput, Select,
    SelectOption,
};
use std::cell::Cell;
use std::rc::Rc;
use yew::prelude::*;

type Flash = Callback<(AlertKind, AttrValue)>;

fn service_items() -> Vec<DropdownItem> {
    let mut cats: Vec<&'static str> = Vec::new();
    for s in SERVICES {
        if !cats.contains(&s.category) {
            cats.push(s.category);
        }
    }
    cats.into_iter()
        .map(|c| DropdownItem::Group {
            text: AttrValue::Static(c),
            items: SERVICES
                .iter()
                .filter(|s| s.category == c)
                .map(|s| DropdownItem::Link {
                    id: AttrValue::Static(s.id),
                    text: AttrValue::Static(s.title),
                    href: AttrValue::Static(s.href),
                    description: Some(AttrValue::Static(s.description)),
                })
                .collect(),
        })
        .collect()
}

#[derive(Properties, PartialEq)]
pub struct ServiceMenuProps {
    pub onfollow: OnFollow,
}

/// Tool switcher (demo registry in M2; real, server-filtered registry in M4).
#[function_component]
pub fn ServiceMenu(p: &ServiceMenuProps) -> Html {
    let items = use_memo((), |_| service_items());
    html! {
        <Dropdown label="Services" items={(*items).clone()}
            onfollow={Some(p.onfollow.clone())} test_id="service-menu" />
    }
}

#[derive(Properties, PartialEq)]
pub struct FlashProps {
    pub flash: Flash,
}

/// Static demo identity until M3; labelled "(demo)" so it is not mistaken for
/// a real session (AC-14).
#[function_component]
pub fn UserMenu(p: &FlashProps) -> Html {
    let on_action = {
        let flash = p.flash.clone();
        Callback::from(move |id: AttrValue| {
            let msg = match id.as_str() {
                "sign-out" => "Sign-in arrives in M3",
                _ => "Profile is not available yet",
            };
            flash.emit((AlertKind::Info, AttrValue::Static(msg)));
        })
    };
    let label = AttrValue::from(format!("{} (demo)", DEMO_USER.display_name));
    html! {
        <Dropdown {label} align_end=true {on_action} test_id="user-menu"
            items={vec![
                DropdownItem::Action { id: "profile".into(), text: "Profile (not available yet)".into() },
                DropdownItem::Divider,
                DropdownItem::Action { id: "sign-out".into(), text: "Sign out".into() },
            ]} />
    }
}

/// Display-only search box. The query is never echoed, stored or put in the URL.
#[function_component]
pub fn GlobalSearch(p: &FlashProps) -> Html {
    let value = use_state(AttrValue::default);
    let on_change = {
        let value = value.clone();
        Callback::from(move |v: AttrValue| value.set(v))
    };
    let on_submit = {
        let flash = p.flash.clone();
        Callback::from(move |_: AttrValue| {
            flash.emit((
                AlertKind::Info,
                AttrValue::Static("Search is not available yet"),
            ));
        })
    };
    html! {
        <SearchInput label="Search the console" placeholder="Search" value={(*value).clone()}
            {on_change} {on_submit} test_id="global-search" />
    }
}

fn all_option() -> SelectOption {
    SelectOption::new("", "All")
}

fn opt(v: &AttrValue) -> Option<&str> {
    (!v.is_empty()).then_some(v.as_str())
}

/// Set the live-region text on the next tick (after the clear has rendered).
/// Falls back to setting it immediately if no timer is available.
fn announce_later(announce: UseStateHandle<String>, msg: String, gen_ref: Rc<Cell<u32>>) {
    use web_sys::wasm_bindgen::{JsCast, closure::Closure};
    let generation = gen_ref.get();
    let Some(win) = web_sys::window() else {
        announce.set(msg);
        return;
    };
    let fallback = (announce.clone(), msg.clone());
    // A later picker change bumps the generation; a stale timer then no-ops.
    let cb = Closure::once_into_js(move || {
        if gen_ref.get() == generation {
            announce.set(msg);
        }
    });
    if win
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            ANNOUNCE_DELAY_MS,
        )
        .is_err()
    {
        fallback.0.set(fallback.1);
    }
}

/// Gap between clearing and re-setting the live region.
const ANNOUNCE_DELAY_MS: i32 = 100;

/// Cascading scope picker: site -> tenant -> environment. Option lists come
/// only from the allow-listed demo hierarchy, never from the URL.
#[function_component]
pub fn ScopePicker() -> Html {
    let Some(ctx) = use_context::<ScopeCtx>() else {
        return Html::default();
    };
    // Polite announcement of cascaded clears; region always rendered so
    // assistive tech picks up changes.
    let announce = use_state(String::new);
    let announce_gen: Rc<Cell<u32>> = (*use_memo((), |_| Rc::new(Cell::new(0u32)))).clone();
    let cur = ctx.scope.clone();
    let site = cur.site.as_deref().and_then(|s| find_site(SITES, s));
    let tenant = site
        .zip(cur.tenant.as_deref())
        .and_then(|(s, t)| find_tenant(s, t));

    let site_opts: Vec<SelectOption> = std::iter::once(all_option())
        .chain(SITES.iter().map(|s| SelectOption::new(s.id, s.name)))
        .collect();
    let tenant_opts: Vec<SelectOption> = std::iter::once(all_option())
        .chain(
            site.into_iter()
                .flat_map(|s| s.tenants)
                .map(|t| SelectOption::new(t.id, t.name)),
        )
        .collect();
    let env_opts: Vec<SelectOption> = std::iter::once(all_option())
        .chain(
            tenant
                .into_iter()
                .flat_map(|t| t.envs)
                .map(|e| SelectOption::new(*e, demo::env_label(e))),
        )
        .collect();

    let emit = {
        let set = ctx.set.clone();
        let announce = announce.clone();
        let announce_gen = announce_gen.clone();
        move |next: console_protocol::Scope, reset: Option<scope::Reset>| {
            // Clear first, then set on the next tick, so an identical
            // consecutive reset message is still a DOM change and is
            // re-announced (A11Y-003, SC 4.1.3).
            announce.set(String::new());
            announce_gen.set(announce_gen.get().wrapping_add(1));
            if let Some(msg) = reset.and_then(|r| r.message()) {
                announce_later(announce.clone(), msg, announce_gen.clone());
            }
            set.emit(next);
        }
    };
    let on_site = {
        let cur = cur.clone();
        let emit = emit.clone();
        Callback::from(move |v: AttrValue| {
            let (n, r) = scope::set_site(&cur, opt(&v), SITES);
            emit(n, r);
        })
    };
    let on_tenant = {
        let cur = cur.clone();
        let emit = emit.clone();
        Callback::from(move |v: AttrValue| {
            let (n, r) = scope::set_tenant(&cur, opt(&v), SITES);
            emit(n, r);
        })
    };
    let on_env = {
        let cur = cur.clone();
        Callback::from(move |v: AttrValue| emit(scope::set_env(&cur, opt(&v), SITES), None))
    };

    let sel = |v: &Option<String>| Some(AttrValue::from(v.clone().unwrap_or_default()));
    // `hint` is the field description; `disabled` is separate so the Site
    // field can explain what it enables without being disabled (A11Y-004).
    let field = |label: &'static str,
                 id: &'static str,
                 options: Vec<SelectOption>,
                 selected: Option<AttrValue>,
                 onchange: Callback<AttrValue>,
                 hint: Option<&'static str>,
                 disabled: bool| {
        let hint_id = format!("{id}-hint");
        let describedby = hint.map(|_| AttrValue::from(hint_id.clone()));
        html! {
            <div class="scope-picker__field">
                <FormField {label} id={AttrValue::Static(id)}>
                    { Callback::from(move |ids: FieldIds| html! {
                        <Select id={ids.control_id} aria_describedby={describedby.clone()}
                            options={options.clone()} selected={selected.clone()}
                            onchange={onchange.clone()} {disabled} testid={AttrValue::Static(id)} />
                    }) }
                </FormField>
                if let Some(h) = hint {
                    <p class="scope-picker__hint" id={hint_id} data-testid={format!("{id}-hint")}>{ h }</p>
                }
            </div>
        }
    };

    html! {
        <div class="scope-picker" role="group" aria-label="Scope" data-testid="scope-picker">
            { field("Site", "scope-site", site_opts, sel(&cur.site), on_site,
                site.is_none().then_some("Selecting a site enables Tenant, then Environment"),
                false) }
            { field("Tenant", "scope-tenant", tenant_opts, sel(&cur.tenant), on_tenant,
                site.is_none().then_some("Select a site first"), site.is_none()) }
            { field("Environment", "scope-env", env_opts, sel(&cur.environment), on_env,
                tenant.is_none().then_some(if site.is_none() {
                    "Select a site first"
                } else {
                    "Select a tenant first"
                }), tenant.is_none()) }
            <div class="cui-visually-hidden" role="status" aria-live="polite" aria-atomic="true"
                data-testid="scope-announcer">{ (*announce).clone() }</div>
        </div>
    }
}
