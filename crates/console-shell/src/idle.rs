//! Idle-timeout warning (AC-11/AC-12). Only the explicit "Stay signed in"
//! extends the session; user activity does not (brief Q6).

use crate::session::{LoginNotice, SessionInfo, parse_rfc3339_ms};
use crate::{APP_CHROME_ID, focus_selector, modal_root};
use console_ui::{Alert, AlertKind, Button, Direction, Modal, SpaceBetween, SpaceSize, Variant};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use yew::prelude::*;

/// Upper bound on how long before the deadline the dialog opens.
pub const WARN_BEFORE_MAX_MS: i64 = 120_000;

/// Warning lead = `min(120 s, idle_timeout / 2)` (plan section 8). The idle
/// length comes from `SessionInfo::idle_timeout_secs` (the configured value),
/// so it is not re-derived from timestamps.
pub fn warn_lead_ms(idle_timeout_secs: u64) -> i64 {
    let half = i64::try_from(idle_timeout_secs.saturating_mul(500)).unwrap_or(i64::MAX);
    WARN_BEFORE_MAX_MS.min(half)
}

/// When the session ends, on the client clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    pub at_ms: i64,
    /// The absolute lifetime ends first; keepalive cannot extend it.
    pub absolute: bool,
}

/// `min(idle_expires_at, absolute_expires_at)` shifted by the skew between
/// `server_time` and the client clock when the response was received.
pub fn deadline(s: &SessionInfo, client_now_ms: i64) -> Option<Deadline> {
    let idle = parse_rfc3339_ms(&s.idle_expires_at)?;
    let abs = parse_rfc3339_ms(&s.absolute_expires_at)?;
    let skew = client_now_ms - parse_rfc3339_ms(&s.server_time)?;
    Some(Deadline {
        at_ms: idle.min(abs) + skew,
        absolute: abs <= idle,
    })
}

/// Whole seconds left, rounded up so "0:00" only shows at the deadline.
pub fn remaining_secs(at_ms: i64, now_ms: i64) -> i64 {
    ((at_ms - now_ms).max(0) + 999) / 1000
}

/// `m:ss` for the visible countdown.
pub fn format_remaining(secs: i64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Polite announcement when the countdown crosses 60 s or 30 s, and only
/// then, so the per-second text update never floods a screen reader. A
/// threshold is only announced if it lies strictly below the lead (`lead_ms`),
/// so a short lead does not announce "1 minute" as the dialog opens.
pub fn announcement(lead_ms: i64, prev_secs: i64, secs: i64) -> Option<&'static str> {
    let lead_secs = lead_ms / 1000;
    if lead_secs > 60 && prev_secs > 60 && secs <= 60 && secs > 30 {
        Some("About 1 minute left before you are signed out.")
    } else if lead_secs > 30 && prev_secs > 30 && secs <= 30 {
        Some("About 30 seconds left before you are signed out.")
    } else {
        None
    }
}

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Hidden,
    Open,
    /// "Continue" on the absolute-expiry variant: closed, deadline still runs.
    Dismissed,
}

#[derive(Properties, PartialEq)]
pub struct IdleWatchProps {
    pub session: Rc<SessionInfo>,
    /// Client clock (ms) when `session` was received.
    pub received_at: i64,
    pub busy: bool,
    /// Last keepalive failure, shown inline in the dialog (the flashbar is
    /// inert behind the modal; A11Y-2).
    #[prop_or_default]
    pub keepalive_error: Option<AttrValue>,
    pub on_keepalive: Callback<()>,
    pub on_sign_out: Callback<()>,
    pub flash: Callback<(AlertKind, AttrValue)>,
}

/// Arms one timer per session snapshot (re-armed when keepalive returns a new
/// one), opens the warning, and navigates to login at the deadline.
#[function_component]
pub fn IdleWatch(p: &IdleWatchProps) -> Html {
    let phase = use_state(|| Phase::Hidden);
    let remaining = use_state(|| 0i64);
    let announce = use_state(|| AttrValue::Static(""));
    let generation = use_memo((), |_| Rc::new(Cell::new(0u32)));
    let dl = deadline(&p.session, p.received_at);
    let lead = warn_lead_ms(p.session.idle_timeout_secs);

    {
        let (phase, remaining, announce) = (phase.clone(), remaining.clone(), announce.clone());
        let generation = (*generation).clone();
        let flash = p.flash.clone();
        use_effect_with((dl, lead), move |(dl, lead)| {
            let lead = *lead;
            let my = generation.get().wrapping_add(1);
            generation.set(my);
            phase.set(Phase::Hidden);
            announce.set(AttrValue::Static(""));
            match *dl {
                None => flash.emit((
                    AlertKind::Warning,
                    "Session: expiry time not understood; no timeout warning will be shown".into(),
                )),
                Some(d) => {
                    let g = generation.clone();
                    yew::platform::spawn_local(async move {
                        let wait = (d.at_ms - lead - now_ms()).max(0);
                        yew::platform::time::sleep(Duration::from_millis(wait as u64)).await;
                        if g.get() != my {
                            return;
                        }
                        let mut prev = i64::MAX;
                        loop {
                            let secs = remaining_secs(d.at_ms, now_ms());
                            if g.get() != my {
                                return;
                            }
                            if secs == 0 {
                                crate::api::redirect_to_login(Some(LoginNotice::Expired));
                                return;
                            }
                            if prev == i64::MAX {
                                phase.set(Phase::Open);
                            }
                            remaining.set(secs);
                            if let Some(m) = announcement(lead, prev, secs) {
                                announce.set(AttrValue::Static(m));
                            }
                            prev = secs;
                            // Wake on the next whole-second boundary.
                            let next = (d.at_ms - now_ms()).rem_euclid(1000).max(50);
                            yew::platform::time::sleep(Duration::from_millis(next as u64)).await;
                        }
                    });
                }
            }
            move || generation.set(generation.get().wrapping_add(1))
        });
    }

    let open = *phase == Phase::Open;
    let absolute = dl.is_some_and(|d| d.absolute);
    use_effect_with((open, absolute), |(open, absolute)| {
        if *open {
            // Runs after the Modal's own effect focused the dialog.
            focus_selector(if *absolute {
                "[data-testid=\"idle-continue\"]"
            } else {
                "[data-testid=\"idle-stay\"]"
            });
        }
    });

    // Always mounted, outside the modal and outside the inert chrome, so
    // the 60 s / 30 s announcements continue after "Continue" (A11Y-6).
    let announcer = html! {
        <div class="cui-visually-hidden" role="status" aria-live="polite" aria-atomic="true"
            data-testid="idle-announcer">{ (*announce).clone() }</div>
    };
    if !open {
        return announcer;
    }

    // While a keepalive is in flight the buttons stay focusable
    // (`keep_focus`: aria-disabled, clicks ignored) so focus never drops to
    // <body> outside the dialog (A11Y-2, WCAG 2.4.3).
    let busy = p.busy;
    let guard = move |cb: Callback<()>| {
        Callback::from(move |_| {
            if !busy {
                cb.emit(());
            }
        })
    };
    let stay = guard(p.on_keepalive.clone()).reform(|_: MouseEvent| ());
    let sign_out = guard(p.on_sign_out.clone()).reform(|_: MouseEvent| ());
    let cont = {
        let phase = phase.clone();
        Callback::from(move |_: MouseEvent| phase.set(Phase::Dismissed))
    };
    // Escape = "Stay signed in" (brief Q4), or "Continue" when the absolute
    // lifetime cannot be extended. No close (X) button and no overlay
    // dismissal: a stray click must not extend the session (A11Y-1,
    // NIST-003, AC-11).
    let on_dismiss = if absolute {
        let phase = phase.clone();
        Callback::from(move |_| phase.set(Phase::Dismissed))
    } else {
        guard(p.on_keepalive.clone())
    };
    let on_error = p.flash.reform(|e: AttrValue| (AlertKind::Error, e));
    let footer = html! {
        <SpaceBetween direction={Direction::Horizontal} size={SpaceSize::S}>
            if absolute {
                <Button variant={Variant::Primary} testid="idle-continue" onclick={cont}>{ "Continue" }</Button>
            } else {
                <Button variant={Variant::Primary} testid="idle-stay" loading={busy} keep_focus=true
                    onclick={stay}>
                    { "Stay signed in" }
                </Button>
            }
            <Button testid="idle-sign-out" disabled={busy} keep_focus=true onclick={sign_out}>
                { "Sign out" }
            </Button>
        </SpaceBetween>
    };
    let body = if absolute {
        "Your session has reached its maximum length and cannot be extended. Save your work; you will be signed out in"
    } else {
        "For security, you will be signed out after a period of inactivity. You will be signed out in"
    };
    let dialog = html! {
        <Modal visible=true header="Your session is about to expire" {on_dismiss} {on_error}
            dismiss_on_overlay=false close_button=false
            inert_root={AttrValue::Static(APP_CHROME_ID)} testid="idle-dialog" {footer}>
            <p>
                { body }{ " " }
                <span class="idle-countdown" data-testid="idle-countdown">
                    { format_remaining(*remaining) }
                </span>
                { "." }
            </p>
            <div role="alert" data-testid="idle-error">
                if let Some(e) = &p.keepalive_error {
                    <Alert kind={AlertKind::Error}>{ e.clone() }</Alert>
                }
            </div>
        </Modal>
    };
    let dialog = match modal_root() {
        Some(host) => create_portal(dialog, host),
        None => dialog,
    };
    html! { <>{ announcer }{ dialog }</> }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_protocol::session::SessionUser;

    fn info(idle: &str, abs: &str, server: &str) -> SessionInfo {
        SessionInfo {
            user: SessionUser {
                id: "s".into(),
                username: "u".into(),
                display_name: "U".into(),
            },
            roles: vec![],
            idle_expires_at: idle.into(),
            absolute_expires_at: abs.into(),
            idle_timeout_secs: 900,
            server_time: server.into(),
            csrf_token: "t".into(),
        }
    }

    #[test]
    fn deadline_uses_min_and_skew() {
        let s = info(
            "2026-11-03T18:15:00Z",
            "2026-11-04T06:00:00Z",
            "2026-11-03T18:00:00Z",
        );
        let server = parse_rfc3339_ms("2026-11-03T18:00:00Z").unwrap();
        // Client clock 5 s ahead of the server.
        let d = deadline(&s, server + 5_000).unwrap();
        assert_eq!(
            d.at_ms,
            parse_rfc3339_ms("2026-11-03T18:15:00Z").unwrap() + 5_000
        );
        assert!(!d.absolute);
        let s = info(
            "2026-11-03T18:15:00Z",
            "2026-11-03T18:10:00Z",
            "2026-11-03T18:00:00Z",
        );
        let d = deadline(&s, server).unwrap();
        assert!(d.absolute);
        assert_eq!(d.at_ms, parse_rfc3339_ms("2026-11-03T18:10:00Z").unwrap());
        assert_eq!(deadline(&info("bad", "x", "y"), 0), None);
    }

    #[test]
    fn countdown_helpers() {
        assert_eq!(remaining_secs(10_000, 0), 10);
        assert_eq!(remaining_secs(10_000, 9_001), 1);
        assert_eq!(remaining_secs(10_000, 10_000), 0);
        assert_eq!(remaining_secs(10_000, 20_000), 0);
        assert_eq!(format_remaining(119), "1:59");
        assert_eq!(format_remaining(5), "0:05");
    }

    #[test]
    fn lead_is_min_of_120s_and_half_idle() {
        assert_eq!(warn_lead_ms(900), 120_000);
        assert_eq!(warn_lead_ms(240), 120_000);
        assert_eq!(warn_lead_ms(150), 75_000);
        assert_eq!(warn_lead_ms(90), 45_000);
        assert_eq!(warn_lead_ms(1), 500);
        assert_eq!(warn_lead_ms(0), 0);
        assert_eq!(warn_lead_ms(u64::MAX), 120_000);
    }

    fn said(lead_ms: i64) -> Vec<i64> {
        let mut out = Vec::new();
        let mut prev = i64::MAX;
        for s in (1..=(lead_ms + 999) / 1000).rev() {
            if announcement(lead_ms, prev, s).is_some() {
                out.push(s);
            }
            prev = s;
        }
        out
    }

    #[test]
    fn short_leads_only_announce_lower_thresholds() {
        assert_eq!(said(120_000), vec![60, 30]);
        assert_eq!(said(75_000), vec![60, 30]);
        assert_eq!(said(60_000), vec![30]);
        assert_eq!(said(45_000), vec![30]);
        assert_eq!(said(30_000), Vec::<i64>::new());
        assert_eq!(said(10_000), Vec::<i64>::new());
    }

    #[test]
    fn announces_only_at_thresholds() {
        let lead = WARN_BEFORE_MAX_MS;
        let mut said = Vec::new();
        let mut prev = i64::MAX;
        for s in (1..=120).rev() {
            if let Some(m) = announcement(lead, prev, s) {
                said.push((s, m));
            }
            prev = s;
        }
        assert_eq!(said.len(), 2);
        assert_eq!(said[0].0, 60);
        assert_eq!(said[1].0, 30);
        // Opening late (tab woke at 45 s) announces the 1-minute mark once.
        assert_eq!(
            announcement(lead, i64::MAX, 45),
            Some("About 1 minute left before you are signed out.")
        );
    }
}
