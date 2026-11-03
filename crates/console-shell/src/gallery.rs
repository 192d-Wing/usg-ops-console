//! Component gallery (cargo feature `gallery`, off in release builds — CM-7).
//! Static demo content only: no data, no API calls. Every console-ui component
//! is shown in each of its states so reviewers and E2E tests have one page to
//! exercise in both themes.

use crate::{APP_CHROME_ID, PageMeta, ShellCtx, modal_root, use_page_meta};
use console_ui::*;
use yew::prelude::*;

fn section(id: &'static str, title: &'static str, body: Html) -> Html {
    html! {
        <section class="gallery-section" aria-labelledby={id} data-testid={format!("gallery-{id}")}>
            <Container header={html! {
                <Header variant={HeaderVariant::H2} id={AttrValue::Static(id)}>{ title }</Header>
            }}>
                { body }
            </Container>
        </section>
    }
}

#[derive(Clone, PartialEq)]
struct Row {
    id: &'static str,
    name: &'static str,
    status: StatusKind,
    owner: &'static str,
}

const ROWS: [Row; 3] = [
    Row {
        id: "svc-1",
        name: "ingest-api",
        status: StatusKind::Success,
        owner: "platform",
    },
    Row {
        id: "svc-2",
        name: "report-worker",
        status: StatusKind::Pending,
        owner: "analytics",
    },
    Row {
        id: "svc-3",
        name: "legacy-ftp",
        status: StatusKind::Stopped,
        owner: "ops",
    },
];

fn table_columns() -> Vec<TableColumn<Row>> {
    vec![
        TableColumn {
            id: "name".into(),
            header: "Name".into(),
            cell: Callback::from(|r: Row| html! { r.name }),
            width: Some(ColWidth::M),
        },
        TableColumn {
            id: "status".into(),
            header: "Status".into(),
            cell: Callback::from(|r: Row| {
                let text = match r.status {
                    StatusKind::Success => "Running",
                    StatusKind::Pending => "Starting",
                    _ => "Stopped",
                };
                html! { <StatusIndicator kind={r.status}>{ text }</StatusIndicator> }
            }),
            width: Some(ColWidth::S),
        },
        TableColumn {
            id: "owner".into(),
            header: "Owner".into(),
            cell: Callback::from(|r: Row| html! { r.owner }),
            width: None,
        },
    ]
}

#[function_component]
pub fn Gallery() -> Html {
    use_page_meta(PageMeta {
        title: "Component gallery".into(),
        description: Some(
            "Every console-ui component in each state. Use the theme toggle in the top bar.".into(),
        ),
        breadcrumbs: vec![
            NavLink::new("Console", "/"),
            NavLink::new("Developer", ""),
            NavLink::new("Component gallery", "/gallery"),
        ],
    });
    let ctx = use_context::<ShellCtx>();
    let flash = ctx.as_ref().map(|c| c.flash.clone()).unwrap_or_default();

    let alert_open = use_state(|| true);
    let text = use_state(AttrValue::default);
    let email = use_state(|| AttrValue::Static("not-an-email"));
    let region = use_state(|| None::<AttrValue>);
    let checked = use_state(|| true);
    let toggled = use_state(|| false);
    let selected = use_state(|| vec![AttrValue::Static("svc-1")]);
    let tab = use_state(|| AttrValue::Static("overview"));
    let modal = use_state(|| false);
    let loading = use_state(|| false);

    let flash_btn = |kind: AlertKind, label: &'static str, msg: &'static str| {
        let flash = flash.clone();
        let onclick = Callback::from(move |_| flash.emit((kind, AttrValue::Static(msg))));
        html! { <Button {onclick} testid={format!("flash-{label}")}>{ format!("Flash {label}") }</Button> }
    };

    let buttons = html! {
        <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::S}>
            <Button testid="btn-normal">{ "Normal" }</Button>
            <Button variant={Variant::Primary} testid="btn-primary">{ "Primary" }</Button>
            <Button variant={Variant::Link}>{ "Link button" }</Button>
            <Button disabled=true>{ "Disabled" }</Button>
            <Button variant={Variant::Primary} disabled=true>{ "Primary disabled" }</Button>
            <Button variant={Variant::Primary} loading={*loading} testid="btn-loading"
                onclick={{ let l = loading.clone(); Callback::from(move |_| l.set(!*l)) }}>
                { if *loading { "Saving" } else { "Click to load" } }
            </Button>
            <Button loading=true>{ "Loading" }</Button>
            <Button aria_label="Refresh list">{ "\u{21bb}" }</Button>
        </SpaceBetween>
    };

    let headers = html! {
        <SpaceBetween>
            <Header variant={HeaderVariant::H1} description="H1 with description and actions"
                actions={html! { <Button variant={Variant::Primary}>{ "Create" }</Button> }}>
                { "Page title" }
            </Header>
            <Header variant={HeaderVariant::H2} description="Secondary heading">{ "Section title" }</Header>
            <Header>{ "Plain H2" }</Header>
        </SpaceBetween>
    };

    let links = html! {
        <SpaceBetween>
            <Link href="/">{ "Internal link (home)" }</Link>
            <Link href="https://www.section508.gov/" external=true>{ "External link" }</Link>
            <Link href="javascript:alert(1)" testid="link-unsafe">{ "Unsafe href (rendered as #)" }</Link>
        </SpaceBetween>
    };

    let badges = html! {
        <SpaceBetween>
            <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::S}>
                <Badge>{ "Grey" }</Badge>
                <Badge color={BadgeColor::Blue}>{ "Blue" }</Badge>
                <Badge color={BadgeColor::Green}>{ "Green" }</Badge>
                <Badge color={BadgeColor::Red}>{ "Red" }</Badge>
                <Badge color={BadgeColor::Yellow}>{ "Yellow" }</Badge>
            </SpaceBetween>
            <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::M}>
                <StatusIndicator kind={StatusKind::Info}>{ "Info" }</StatusIndicator>
                <StatusIndicator kind={StatusKind::Success}>{ "Success" }</StatusIndicator>
                <StatusIndicator kind={StatusKind::Warning}>{ "Warning" }</StatusIndicator>
                <StatusIndicator kind={StatusKind::Error}>{ "Error" }</StatusIndicator>
                <StatusIndicator kind={StatusKind::Pending}>{ "Pending" }</StatusIndicator>
                <StatusIndicator kind={StatusKind::Stopped}>{ "Stopped" }</StatusIndicator>
            </SpaceBetween>
            <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::M}>
                <Spinner />
                <Spinner size={SpinnerSize::Large} label="Loading services" />
            </SpaceBetween>
        </SpaceBetween>
    };

    let alerts = html! {
        <SpaceBetween>
            <Alert kind={AlertKind::Info} header="Info">{ "Informational alert." }</Alert>
            <Alert kind={AlertKind::Success}>{ "Success alert without header." }</Alert>
            <Alert kind={AlertKind::Warning} header="Warning"
                action={html! { <Button>{ "Review" }</Button> }}>
                { "Warning alert with an action." }
            </Alert>
            if *alert_open {
                <Alert kind={AlertKind::Error} header="Error" dismissible=true
                    on_dismiss={{ let a = alert_open.clone(); Callback::from(move |_| a.set(false)) }}>
                    { "Dismissible error alert." }
                </Alert>
            } else {
                <Button onclick={{ let a = alert_open.clone(); Callback::from(move |_| a.set(true)) }}>
                    { "Restore dismissed alert" }
                </Button>
            }
            <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::S}>
                { flash_btn(AlertKind::Info, "info", "Informational notification.") }
                { flash_btn(AlertKind::Success, "success", "Saved successfully.") }
                { flash_btn(AlertKind::Warning, "warning", "Quota at 90%.") }
                { flash_btn(AlertKind::Error, "error", "Request failed.") }
            </SpaceBetween>
        </SpaceBetween>
    };

    let region_opts = vec![
        SelectOption::new("us-gov-west-1", "US GovCloud (West)"),
        SelectOption::new("us-gov-east-1", "US GovCloud (East)"),
        SelectOption {
            value: "us-east-1".into(),
            label: "US East (unavailable)".into(),
            disabled: true,
        },
    ];
    let email_err: Option<AttrValue> =
        (!email.contains('@')).then_some("Enter a valid email address.".into());

    let forms = html! {
        <Grid cols={2}>
            <FormField label="Name" description="Free text input.">
                { {
                    let text = text.clone();
                    Callback::from(move |ids: FieldIds| {
                        let t = text.clone();
                        html! { <Input id={ids.control_id} aria_describedby={ids.describedby}
                            value={(*text).clone()} testid="input-name"
                            oninput={Callback::from(move |v: String| t.set(v.into()))} /> }
                    })
                } }
            </FormField>
            <FormField label="Email" description="Shows the error state." error={email_err}>
                { {
                    let email = email.clone();
                    Callback::from(move |ids: FieldIds| {
                        let e = email.clone();
                        html! { <Input id={ids.control_id} aria_describedby={ids.describedby}
                            invalid={ids.invalid} kind={InputKind::Email} value={(*email).clone()}
                            oninput={Callback::from(move |v: String| e.set(v.into()))} /> }
                    })
                } }
            </FormField>
            <FormField label="Disabled input">
                { Callback::from(|ids: FieldIds| html! {
                    <Input id={ids.control_id} aria_describedby={ids.describedby} disabled=true value="read only" />
                }) }
            </FormField>
            <FormField label="Region">
                { {
                    let region = region.clone();
                    Callback::from(move |ids: FieldIds| {
                        let r = region.clone();
                        html! { <Select id={ids.control_id} aria_describedby={ids.describedby}
                            options={region_opts.clone()} selected={(*region).clone()}
                            placeholder="Choose a region"
                            onchange={Callback::from(move |v: AttrValue| r.set(Some(v)))} /> }
                    })
                } }
            </FormField>
            <SpaceBetween>
                <Checkbox checked={*checked}
                    onchange={{ let c = checked.clone(); Callback::from(move |v| c.set(v)) }}>
                    { "Checkbox" }
                </Checkbox>
                <Checkbox checked=false indeterminate=true>{ "Indeterminate" }</Checkbox>
                <Checkbox checked=false disabled=true>{ "Disabled checkbox" }</Checkbox>
            </SpaceBetween>
            <SpaceBetween>
                <Toggle checked={*toggled} testid="toggle-demo"
                    onchange={{ let t = toggled.clone(); Callback::from(move |v| t.set(v)) }}>
                    { "Enable feature" }
                </Toggle>
                <Toggle checked=true disabled=true>{ "Disabled toggle" }</Toggle>
            </SpaceBetween>
        </Grid>
    };

    let item_key = Callback::from(|r: Row| AttrValue::Static(r.id));
    let tables = html! {
        <SpaceBetween>
            <Table<Row> caption="Services (selectable)" columns={table_columns()} items={ROWS.to_vec()}
                item_key={item_key.clone()} selection={SelectionMode::Multi}
                selected={(*selected).clone()}
                on_selection={{ let s = selected.clone(); Callback::from(move |v| s.set(v)) }}
                row_label={Some(Callback::from(|r: Row| AttrValue::Static(r.name)))}
                empty={html! { "No services" }} testid="table-demo" />
            <Table<Row> caption="Services (empty)" columns={table_columns()} items={Vec::new()}
                {item_key} empty={html! { <p>{ "No services found." }</p> }} />
        </SpaceBetween>
    };

    let tabs = html! {
        <Tabs aria_label="Service details" active={(*tab).clone()}
            onchange={{ let t = tab.clone(); Callback::from(move |v| t.set(v)) }}
            tabs={vec![
                TabDef::new("overview", "Overview", html! { <p>{ "Overview panel." }</p> }),
                TabDef::new("logs", "Logs", html! { <p>{ "Logs panel." }</p> }),
                TabDef { disabled: true, ..TabDef::new("metrics", "Metrics (disabled)", Html::default()) },
                TabDef::new("settings", "Settings", html! { <p>{ "Settings panel." }</p> }),
            ]} />
    };

    let close = {
        let m = modal.clone();
        Callback::from(move |_: ()| m.set(false))
    };
    let modal_html = html! {
        <Modal visible={*modal} header="Delete service" on_dismiss={close.clone()} testid="modal-demo"
            inert_root={AttrValue::Static(APP_CHROME_ID)}
            on_error={{ let f = flash.clone(); Callback::from(move |e: AttrValue| f.emit((AlertKind::Error, e))) }}
            footer={html! {
                <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::S}>
                    <Button onclick={{ let c = close.clone(); Callback::from(move |_| c.emit(())) }}>{ "Cancel" }</Button>
                    <Button variant={Variant::Primary}
                        onclick={{ let c = close.clone(); Callback::from(move |_| c.emit(())) }}>{ "Delete" }</Button>
                </SpaceBetween>
            }}>
            <p>{ "The rest of the page is inert while this dialog is open. Esc closes it and focus returns to the opener." }</p>
        </Modal>
    };
    // Portal into #modal-root, a sibling of the inert chrome region.
    let modal_portal = match modal_root() {
        Some(host) => create_portal(modal_html, host),
        None => modal_html,
    };
    let modals = html! {
        <Button testid="open-modal" onclick={{ let m = modal.clone(); Callback::from(move |_| m.set(true)) }}>
            { "Open modal" }
        </Button>
    };

    let navigation = html! {
        <Grid cols={2}>
            <SideNavigation aria_label="Gallery sample navigation"
                header={Some(NavLink::new("Service", "/gallery"))}
                active_href={Some(AttrValue::Static("/gallery"))}
                items={vec![
                    SideNavItem::Link(NavLink::new("Gallery (active)", "/gallery")),
                    SideNavItem::Section { text: "Section".into(), items: vec![
                        NavLink::new("Home", "/"),
                        NavLink::new("Docs", "https://www.section508.gov/"),
                    ]},
                    SideNavItem::Divider,
                    SideNavItem::Link(NavLink::new("After divider", "/")),
                ]} />
            <Breadcrumbs items={vec![
                NavLink::new("Console", "/"),
                NavLink::new("Developer", ""),
                NavLink::new("Gallery", "/gallery"),
            ]} />
        </Grid>
    };

    let layout = html! {
        <SpaceBetween>
            <Grid cols={4}>
                <Container>{ "1" }</Container><Container>{ "2" }</Container>
                <Container>{ "3" }</Container><Container>{ "4" }</Container>
            </Grid>
            <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::Xs}>
                <Badge>{ "xs" }</Badge><Badge>{ "gap" }</Badge>
            </SpaceBetween>
            <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::Xl}>
                <Badge>{ "xl" }</Badge><Badge>{ "gap" }</Badge>
            </SpaceBetween>
            <Container header={html! { <h3>{ "Container with footer" }</h3> }}
                footer={html! { <span>{ "Footer content" }</span> }}>
                { "Body" }
            </Container>
        </SpaceBetween>
    };

    html! {
        <div data-testid="gallery">
            <SpaceBetween size={SpaceSize::L}>
                { section("g-buttons", "Buttons", buttons) }
                { section("g-headers", "Headers", headers) }
                { section("g-links", "Links", links) }
                { section("g-status", "Badges, status, spinner", badges) }
                { section("g-alerts", "Alerts and flashbar", alerts) }
                { section("g-forms", "Form controls", forms) }
                { section("g-table", "Table", tables) }
                { section("g-tabs", "Tabs", tabs) }
                { section("g-modal", "Modal", modals) }
                { section("g-nav", "Navigation", navigation) }
                { section("g-layout", "Layout", layout) }
            </SpaceBetween>
            { modal_portal }
        </div>
    }
}
