//! Trusted user-presence signal that gates a tool's `RequestSessionRefresh`
//! (SEC-002 / NIST-001; NIST 800-53 AC-11, AC-12; user decision 2026-11-04).
//!
//! A tool may only extend the console session when the shell has seen real
//! user input recently, or the idle-warning dialog is open. "User input" is:
//! - a trusted (`isTrusted`) `pointerdown` or `keydown` in the shell
//!   document, or
//! - focus moving into the tool iframe (shell `document.activeElement`
//!   becomes the iframe: the user clicked or tabbed into the tool), but only
//!   for the first entry since page load or when a trusted shell
//!   `pointerdown`/`keydown` happened after the last observed frame exit
//!   (fix round 3, SEC-012). A tool that focuses itself after the user left
//!   it (or after it moved focus out with `FocusShell`) cannot fake an
//!   entry. Residual: before any exit, a tool's self-focus is
//!   indistinguishable from the user's first click into it (Firefox fires no
//!   shell event for that click), so at most one entry per page load can be
//!   faked.
//!
//! The focus transition is sampled from the shell's own DOM state on window
//! `blur`/`focus` (capture, so element focus changes too), shortly after
//! them, once a second, and when a tool asks for a refresh. Fix round 2
//! (F2): Firefox fires no shell event at all when the user clicks from a
//! focused `<body>` into a cross-origin frame (only `activeElement`
//! changes), and on an element `blur` it may update `activeElement` late,
//! so an event-only check missed it.
//!
//! Anything a tool reports about its own activity is NOT trusted.
//!
//! Known limitation: input inside the cross-origin tool frame is invisible
//! to the shell. A user who works only inside the tool for longer than the
//! idle timeout (without the shell seeing a click, key or focus transition)
//! gets the normal idle warning and must choose "Stay signed in". This is
//! the strict-idle behaviour the user asked for, not a defect.

use crate::tool_host::{frame_has_focus, sync_frame_ring};
use console_ui::AlertKind;
use std::cell::Cell;
use yew::prelude::*;

/// How recent trusted shell input must be for a tool refresh to be honoured.
pub const REFRESH_INPUT_WINDOW_MS: f64 = 60_000.0;

thread_local! {
    static LAST_INPUT_MS: Cell<Option<f64>> = const { Cell::new(None) };
    static IDLE_DIALOG_OPEN: Cell<bool> = const { Cell::new(false) };
    /// Last sampled "tool frame has focus".
    static FRAME_SEEN: Cell<bool> = const { Cell::new(false) };
    /// Focus left the frame (user or `FocusShell`) and no trusted shell
    /// input has happened since (SEC-012).
    static EXIT_UNCONFIRMED: Cell<bool> = const { Cell::new(false) };
    /// Bumped on every observed frame exit and trusted shell input; lets a
    /// pending `FocusShell` retry notice the user acted in between.
    static FOCUS_EPOCH: Cell<u32> = const { Cell::new(0) };
    /// The shell window itself has focus (last window `focus`/`blur`).
    static SHELL_WINDOW_FOCUSED: Cell<bool> = const { Cell::new(false) };
}

/// Pure rule: honour a tool refresh only with an open idle dialog or trusted
/// input within [`REFRESH_INPUT_WINDOW_MS`]. A clock that went backwards
/// does not count as recent.
pub fn user_present(last_input_ms: Option<f64>, idle_dialog_open: bool, now_ms: f64) -> bool {
    idle_dialog_open
        || last_input_ms.is_some_and(|t| now_ms >= t && now_ms - t <= REFRESH_INPUT_WINDOW_MS)
}

/// [`user_present`] with the recorded browser state.
pub fn user_present_now() -> bool {
    observe_frame_focus();
    user_present(
        LAST_INPUT_MS.with(Cell::get),
        IDLE_DIALOG_OPEN.with(Cell::get),
        js_sys::Date::now(),
    )
}

/// Called by the idle warning while it is shown.
pub fn set_idle_dialog_open(open: bool) {
    IDLE_DIALOG_OPEN.with(|c| c.set(open));
}

/// Whether the idle warning is shown (the shell ignores Ctrl/Cmd+K then).
pub fn idle_dialog_open() -> bool {
    IDLE_DIALOG_OPEN.with(Cell::get)
}

fn note_input() {
    LAST_INPUT_MS.with(|c| c.set(Some(js_sys::Date::now())));
}

/// Pure rule: a sampled not-focused -> focused transition of the tool frame
/// counts as user input, unless focus left the frame since the last trusted
/// shell input (`exit_unconfirmed`, SEC-012).
pub fn frame_entry_counts(was_in_frame: bool, now_in_frame: bool, exit_unconfirmed: bool) -> bool {
    now_in_frame && !was_in_frame && !exit_unconfirmed
}

/// Samples whether the tool frame has focus (see module docs) and updates
/// the frame's state-driven focus ring.
pub fn observe_frame_focus() {
    let now = frame_has_focus();
    let was = FRAME_SEEN.with(|c| c.replace(now));
    if frame_entry_counts(was, now, EXIT_UNCONFIRMED.with(Cell::get)) {
        note_input();
    }
    if was && !now {
        EXIT_UNCONFIRMED.with(|c| c.set(true));
        bump_epoch();
    }
    sync_frame_ring(SHELL_WINDOW_FOCUSED.with(Cell::get));
}

fn bump_epoch() {
    FOCUS_EPOCH.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Changes whenever focus left the frame or the user gave trusted shell
/// input.
pub fn focus_epoch() -> u32 {
    FOCUS_EPOCH.with(Cell::get)
}

/// Called when the shell moves focus out of the tool because the tool asked
/// (`FocusShell`): a later re-entry is not user input until the shell sees
/// trusted input again. Closes a loop where a tool alternates `FocusShell`
/// and focusing itself.
pub fn note_tool_moved_focus() {
    EXIT_UNCONFIRMED.with(|c| c.set(true));
    observe_frame_focus();
}

/// Delays (ms) after a window `blur`/`focus` at which focus is re-sampled.
const RESAMPLE_MS: [i32; 2] = [0, 50];
/// Background sampling period.
const SAMPLE_EVERY_MS: i32 = 1_000;

fn on_event(e: &web_sys::Event) {
    match e.type_().as_str() {
        "pointerdown" | "keydown" if e.is_trusted() => {
            EXIT_UNCONFIRMED.with(|c| c.set(false));
            bump_epoch();
            note_input();
            observe_frame_focus();
        }
        t @ ("blur" | "focus") => {
            if e.target().is_some_and(|x| {
                web_sys::wasm_bindgen::JsCast::dyn_ref::<web_sys::Window>(&x).is_some()
            }) {
                SHELL_WINDOW_FOCUSED.with(|c| c.set(t == "focus"));
            } else if t == "focus" {
                // A shell element got focus, so the shell window has it.
                SHELL_WINDOW_FOCUSED.with(|c| c.set(true));
            }
            observe_frame_focus();
            if let Some(w) = web_sys::window() {
                for ms in RESAMPLE_MS {
                    let cb =
                        web_sys::wasm_bindgen::closure::Closure::once_into_js(observe_frame_focus);
                    let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
                        web_sys::wasm_bindgen::JsCast::unchecked_ref(&cb),
                        ms,
                    );
                }
            }
        }
        _ => {}
    }
}

/// Installs the window listeners once for the life of the shell.
#[hook]
pub fn use_activity_tracker(flash: Callback<(AlertKind, AttrValue)>) {
    use_effect_with((), move |_| {
        use web_sys::wasm_bindgen::{JsCast, closure::Closure};
        let win = web_sys::window();
        let has = win
            .as_ref()
            .and_then(|w| w.document())
            .and_then(|d| d.has_focus().ok())
            .unwrap_or(false);
        SHELL_WINDOW_FOCUSED.with(|c| c.set(has));
        let cb = Closure::<dyn Fn(web_sys::Event)>::new(|e: web_sys::Event| on_event(&e));
        let ok = win.as_ref().is_some_and(|w| {
            ["pointerdown", "keydown", "blur", "focus"].iter().all(|t| {
                w.add_event_listener_with_callback_and_bool(t, cb.as_ref().unchecked_ref(), true)
                    .is_ok()
            })
        });
        let tick = Closure::<dyn Fn()>::new(observe_frame_focus);
        let interval = win.as_ref().and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(
                tick.as_ref().unchecked_ref(),
                SAMPLE_EVERY_MS,
            )
            .ok()
        });
        if !ok || interval.is_none() {
            flash.emit((
                AlertKind::Warning,
                "Activity tracking unavailable: tools cannot extend your session; use \"Stay signed in\" when warned.".into(),
            ));
        }
        move || {
            if let Some(w) = win {
                if let Some(id) = interval {
                    w.clear_interval_with_handle(id);
                }
                drop(tick);
                for t in ["pointerdown", "keydown", "blur", "focus"] {
                    let _ = w.remove_event_listener_with_callback_and_bool(
                        t,
                        cb.as_ref().unchecked_ref(),
                        true,
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_untainted_entry_into_frame_counts() {
        assert!(
            frame_entry_counts(false, true, false),
            "user entered the frame"
        );
        assert!(
            !frame_entry_counts(true, true, false),
            "already in the frame"
        );
        assert!(!frame_entry_counts(false, false, false), "not in the frame");
        assert!(!frame_entry_counts(true, false, false), "left the frame");
        assert!(
            !frame_entry_counts(false, true, true),
            "re-entry after an exit with no trusted shell input since (SEC-012)"
        );
    }

    #[test]
    fn refresh_needs_recent_input_or_open_dialog() {
        let now = 1_000_000.0;
        assert!(!user_present(None, false, now), "never any input");
        assert!(user_present(None, true, now), "idle dialog open");
        assert!(user_present(Some(now - 1.0), false, now));
        assert!(user_present(
            Some(now - REFRESH_INPUT_WINDOW_MS),
            false,
            now
        ));
        assert!(!user_present(
            Some(now - REFRESH_INPUT_WINDOW_MS - 1.0),
            false,
            now
        ));
        assert!(
            !user_present(Some(now + 5_000.0), false, now),
            "future stamp (clock moved back) is not recent"
        );
    }
}
