//! hello-tool web UI: the template tool for the console SDK.
//!
//! One build serves two registry ids from the same server:
//! - `hello`: the normal tool (SDK hooks, identity from its backend, scope,
//!   theme, breadcrumbs, notifications, own-origin storage);
//! - `evil`: a hostile fixture that tries to escape the sandbox so E2E can
//!   prove each attempt fails (see `evil.rs`).
//!
//! The mode is chosen from `location.pathname` (`/t/<id>/...`).

mod evil;
mod hello;
mod util;

fn main() {
    let (id, _) = util::location_tool();
    if id == "evil" {
        yew::Renderer::<evil::Evil>::new().render();
    } else {
        yew::Renderer::<hello::Root>::new().render();
    }
}
