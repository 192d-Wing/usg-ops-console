//! Console shell entry point. M0: static chrome; tool registry, router, and
//! iframe container arrive in M2/M4.

use console_ui::{Container, Variant};
use yew::prelude::*;

#[function_component]
fn App() -> Html {
    html! {
        <>
            <header class="topbar" role="banner">
                <span class="topbar__brand">{ "USG Ops Console" }</span>
                <console_ui::Button variant={Variant::Link}>{ "Services" }</console_ui::Button>
                <span class="topbar__spacer" />
                <span>{ format!("SDK protocol v{}", console_protocol::PROTOCOL_VERSION) }</span>
            </header>
            <div class="layout">
                <nav class="sidenav" aria-label="Console">
                    <ul>
                        <li>{ "Home" }</li>
                        <li>{ "Tool catalog" }</li>
                        <li>{ "Settings" }</li>
                    </ul>
                </nav>
                <main class="content" id="tool-container">
                    <Container header="Welcome">
                        <p>{ "No tools registered yet." }</p>
                    </Container>
                </main>
            </div>
        </>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
