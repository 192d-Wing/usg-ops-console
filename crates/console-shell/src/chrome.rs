//! Console chrome pieces composed into the top navigation: the user menu and
//! the breadcrumb scope switcher (M2 cascading picker inside popovers).

use crate::ctx::{ScopeCtx, SessionCtx};
use crate::demo::{self, SITES};
use crate::palette::Seg;
use crate::scope::{self, find_site, find_tenant};
use console_ui::{
    AlertKind, Dropdown, DropdownItem, FieldIds, FormField, Icon, IconName, Popover, PopoverAlign,
    Select, SelectOption,
};
use std::cell::Cell;
use std::rc::Rc;
use yew::prelude::*;

type Flash = Callback<(AlertKind, AttrValue)>;

#[derive(Properties, PartialEq)]
pub struct FlashProps {
    pub flash: Flash,
}

/// Signed-in user from `/api/session`. A skeleton is shown while the session
/// loads so first paint is not blocked on it; a "Session unavailable" menu
/// with Retry and a sign-in link if loading failed (A11Y-3).
#[function_component]
pub fn UserMenu(p: &FlashProps) -> Html {
    let ctx = use_context::<SessionCtx>();
    let on_action = {
        let flash = p.flash.clone();
        let sign_out = ctx.as_ref().map(|c| c.sign_out.clone());
        let retry = ctx.as_ref().map(|c| c.retry.clone());
        Callback::from(
            move |id: AttrValue| match (id.as_str(), &sign_out, &retry) {
                ("sign-out", Some(s), _) => s.emit(()),
                ("retry", _, Some(r)) => r.emit(()),
                _ => flash.emit((
                    AlertKind::Info,
                    AttrValue::Static("Profile is not available yet"),
                )),
            },
        )
    };
    let signing_out = ctx.as_ref().is_some_and(|c| c.signing_out);
    // Always mounted so the change to "Signing out…" is announced (A11Y-8).
    let status = html! {
        <span class="cui-visually-hidden" role="status" aria-live="polite"
            data-testid="user-menu-status">
            if signing_out { { "Signing out\u{2026}" } }
        </span>
    };
    let failed = ctx.as_ref().is_some_and(|c| c.failed);
    let Some(info) = ctx.and_then(|c| c.session) else {
        if failed {
            let (path, search, _) = crate::current_location();
            let login = crate::session::login_url(&path, &search, None);
            return html! {
                <>
                    <Dropdown label="Session unavailable" align_end=true {on_action}
                        test_id="user-menu-failed"
                        items={vec![
                            DropdownItem::Action { id: "retry".into(), text: "Retry".into() },
                            DropdownItem::Link {
                                id: "sign-in".into(),
                                text: "Sign in".into(),
                                href: login.into(),
                                description: None,
                            },
                        ]} />
                    { status }
                </>
            };
        }
        return html! {
            <>
                <span class="user-menu-skeleton" data-testid="user-menu-loading">
                    <span class="cui-visually-hidden">{ "Loading user" }</span>
                </span>
                { status }
            </>
        };
    };
    let label = AttrValue::from(info.user.display_name.clone());
    let aria_label = AttrValue::from(format!("{}, user menu", info.user.display_name));
    html! {
        <>
            <Dropdown {label} {aria_label} align_end=true {on_action} test_id="user-menu"
                items={vec![
                    DropdownItem::Group {
                        text: format!("Signed in as {}", info.user.username).into(),
                        items: vec![
                            DropdownItem::Action { id: "profile".into(), text: "Profile (not available yet)".into() },
                        ],
                    },
                    DropdownItem::Divider,
                    DropdownItem::Action { id: "sign-out".into(), text: "Sign out".into() },
                    DropdownItem::Divider,
                    // Moved from the old scope bar (plan 4.3); text unchanged.
                    DropdownItem::Note {
                        id: "sdk-protocol".into(),
                        text: format!("SDK protocol v{}", console_protocol::PROTOCOL_VERSION).into(),
                    },
                ]} />
            { status }
        </>
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

#[derive(Properties, PartialEq)]
pub struct ScopePickerProps {
    /// Wide layout (>= 45rem): one segment per level. Narrow: one "Scope"
    /// button whose popover holds all three selects.
    pub wide: bool,
    /// Segment whose popover is open (owned by the shell so the command
    /// palette can open one).
    pub open: Option<Seg>,
    pub on_open: Callback<Option<Seg>>,
}

/// Popover ids (`aria-controls`).
fn popover_id(seg: Option<Seg>) -> String {
    match seg {
        Some(s) => format!("scope-{}-popover", s.key()),
        None => "scope-popover".to_owned(),
    }
}

/// Focus target when a scope popover opens: the segment's select, or its
/// hint when the select is disabled (so focus is never lost on open).
fn focus_target(seg: Seg, disabled: bool) -> String {
    let id = match seg {
        Seg::Site => "scope-site",
        Seg::Tenant => "scope-tenant",
        Seg::Env => "scope-env",
    };
    if disabled {
        format!("#{id}-hint")
    } else {
        format!("#{id}")
    }
}

/// Breadcrumb scope switcher (plan 4.3): Site > Tenant > Environment. Each
/// segment opens a popover with the unchanged M2 cascading `<select>`. Option
/// lists come only from the allow-listed demo hierarchy, never from the URL.
/// The popover stays open after a change (brief decision), so keyboard users
/// can Tab on to the next segment.
#[function_component]
pub fn ScopePicker(p: &ScopePickerProps) -> Html {
    let anchors = use_memo((), |_| {
        [
            NodeRef::default(),
            NodeRef::default(),
            NodeRef::default(),
            NodeRef::default(),
        ]
    });
    // Polite announcement of cascaded clears; region always rendered (outside
    // the popovers) so assistive tech picks up changes.
    let announce = use_state(String::new);
    let announce_gen: Rc<Cell<u32>> = (*use_memo((), |_| Rc::new(Cell::new(0u32)))).clone();
    let ctx = use_context::<ScopeCtx>();
    let cur = ctx.as_ref().map(|c| c.scope.clone()).unwrap_or_default();
    let site = cur.site.as_deref().and_then(|s| find_site(SITES, s));
    let tenant = site
        .zip(cur.tenant.as_deref())
        .and_then(|(s, t)| find_tenant(s, t));
    let disabled = |seg: Seg| match seg {
        Seg::Site => false,
        Seg::Tenant => site.is_none(),
        Seg::Env => tenant.is_none(),
    };

    // Move focus into the popover for the requested segment. Runs after the
    // Popover's own open effect (children first), so it wins in the compact
    // layout, where one popover holds all three selects.
    {
        let target = p.open.map(|s| focus_target(s, disabled(s)));
        use_effect_with((target, p.wide), |(t, _)| {
            if let Some(t) = t {
                crate::focus_selector(t);
            }
        });
    }
    let Some(ctx) = ctx else {
        return Html::default();
    };

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
    // The hint takes programmatic focus (tabindex -1) when its select is
    // disabled and the popover opens.
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
                    <p class="scope-picker__hint" id={hint_id} data-testid={format!("{id}-hint")}
                        tabindex={disabled.then_some("-1")}>{ h }</p>
                }
            </div>
        }
    };
    let site_field = field(
        "Site",
        "scope-site",
        site_opts,
        sel(&cur.site),
        on_site,
        site.is_none()
            .then_some("Selecting a site enables Tenant, then Environment"),
        false,
    );
    let tenant_field = field(
        "Tenant",
        "scope-tenant",
        tenant_opts,
        sel(&cur.tenant),
        on_tenant,
        site.is_none().then_some("Select a site first"),
        site.is_none(),
    );
    let env_field = field(
        "Environment",
        "scope-env",
        env_opts,
        sel(&cur.environment),
        on_env,
        tenant.is_none().then_some(if site.is_none() {
            "Select a site first"
        } else {
            "Select a tenant first"
        }),
        tenant.is_none(),
    );

    // Segment text: the current value, or a muted placeholder when unset.
    let value = |seg: Seg| -> (String, bool) {
        match seg {
            Seg::Site => site.map_or(("All sites".into(), true), |s| (s.name.into(), false)),
            Seg::Tenant => tenant.map_or(
                (
                    if site.is_some() {
                        "All tenants"
                    } else {
                        "Select tenant"
                    }
                    .into(),
                    true,
                ),
                |t| (t.name.into(), false),
            ),
            Seg::Env => cur
                .environment
                .as_deref()
                .filter(|_| tenant.is_some())
                .map_or(
                    (
                        if tenant.is_some() {
                            "All environments"
                        } else {
                            "Select environment"
                        }
                        .into(),
                        true,
                    ),
                    |e| (demo::env_label(e).into(), false),
                ),
        }
    };
    let seg_name = |seg: Seg| match seg {
        Seg::Site => "Site",
        Seg::Tenant => "Tenant",
        Seg::Env => "Environment",
    };
    let close = p.on_open.reform(|()| None);
    let toggle = |seg: Seg| {
        let on_open = p.on_open.clone();
        let open = p.open;
        Callback::from(move |_: MouseEvent| {
            on_open.emit(if open == Some(seg) { None } else { Some(seg) })
        })
    };
    let announcer = html! {
        <div class="cui-visually-hidden" role="status" aria-live="polite" aria-atomic="true"
            data-testid="scope-announcer">{ (*announce).clone() }</div>
    };

    // One tree for both widths (D4): the selects are created once and only
    // attributes change across the 45rem switch. Wide: the outer popover is
    // an embedded pass-through and each segment has its own popover.
    // Compact: the outer popover is the dialog and the segment popovers are
    // embedded, so all three selects show together. `open` still carries
    // the requested segment (focus target).
    let wide = p.wide;
    let compact_anchor = anchors[3].clone();
    let expanded = !wide && p.open.is_some();
    let compact_click = {
        let on_open = p.on_open.clone();
        Callback::from(move |_: MouseEvent| {
            on_open.emit(if expanded { None } else { Some(Seg::Site) })
        })
    };
    let summary: Vec<String> = Seg::ALL
        .into_iter()
        .filter_map(|s| {
            let (v, unset) = value(s);
            (!unset).then_some(v)
        })
        .collect();
    let fields = [site_field, tenant_field, env_field];
    html! {
        <nav class={classes!("cui-scope-crumbs", "scope-crumbs", (!wide).then_some("scope-crumbs--compact"))}
            aria-label="Scope" data-testid="scope-picker">
            <button type="button" class="cui-scope-crumbs__seg" ref={compact_anchor.clone()} hidden={wide}
                aria-haspopup="dialog" aria-expanded={if expanded { "true" } else { "false" }}
                aria-controls={popover_id(None)} data-testid="scope-trigger" onclick={compact_click}>
                <span>{ "Scope" }</span>
                <span class="cui-visually-hidden">
                    { if summary.is_empty() { String::new() } else { format!(": {}", summary.join(", ")) } }
                </span>
                <Icon name={IconName::ChevronDown} />
            </button>
            <Popover id={AttrValue::from(popover_id(None))} open={expanded} on_close={close.clone()}
                anchor={compact_anchor} label={AttrValue::Static("Scope")} align={PopoverAlign::End}
                embedded={wide} testid={AttrValue::Static("scope-popover")}>
            <ol class="scope-crumbs__list">
                { for Seg::ALL.into_iter().zip(fields).enumerate().map(|(i, (seg, f))| {
                    let anchor = anchors[i].clone();
                    let open = wide && p.open == Some(seg);
                    let (text, unset) = value(seg);
                    let pid = popover_id(Some(seg));
                    html! {
                        <li class="scope-crumbs__item" key={seg.key()}>
                            if i > 0 {
                                <span class="cui-scope-crumbs__sep" aria-hidden="true" hidden={!wide}>
                                    <Icon name={IconName::ChevronRight} />
                                </span>
                            }
                            <button type="button" ref={anchor.clone()} hidden={!wide}
                                class={classes!("cui-scope-crumbs__seg", unset.then_some("cui-scope-crumbs__seg--unset"))}
                                aria-haspopup="dialog" aria-expanded={if open { "true" } else { "false" }}
                                aria-controls={pid.clone()}
                                data-testid={format!("scope-{}-trigger", seg.key())}
                                onclick={toggle(seg)}>
                                <span class="cui-visually-hidden">{ format!("{}: ", seg_name(seg)) }</span>
                                <span class="cui-scope-crumbs__value">{ text }</span>
                                <Icon name={IconName::ChevronDown} class={classes!("cui-scope-crumbs__chev")} />
                            </button>
                            <Popover id={AttrValue::from(pid)} {open} on_close={close.clone()} {anchor}
                                label={AttrValue::Static(seg_name(seg))} embedded={!wide}
                                testid={AttrValue::from(format!("scope-{}-popover", seg.key()))}>
                                { f }
                            </Popover>
                        </li>
                    }
                }) }
            </ol>
            </Popover>
            { announcer }
        </nav>
    }
}
