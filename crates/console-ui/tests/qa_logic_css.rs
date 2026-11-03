//! QA additions (M1): logic edge cases and stylesheet/HTML guards.

use console_ui::logic::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const TOKENS: &str = include_str!("../assets/tokens.css");
const COMPONENTS: &str = include_str!("../assets/components.css");

fn repo_file(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn strip_comments(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i..].find("*/").map_or("", |j| &rest[i + j + 2..]);
    }
    out.push_str(rest);
    out
}

// ---- logic edge cases ----

#[test]
fn roving_edge_cases() {
    assert_eq!(roving_index(0, &[], NavKey::Next), None);
    assert_eq!(roving_index(0, &[false, false], NavKey::First), None);
    assert_eq!(roving_index(0, &[true], NavKey::Next), Some(0));
    assert_eq!(roving_index(0, &[true], NavKey::Prev), Some(0));
    // Out-of-range current is clamped, not a panic.
    assert_eq!(roving_index(99, &[true, true, true], NavKey::Next), Some(0));
    assert_eq!(roving_index(99, &[true, true, true], NavKey::Prev), Some(1));
    assert_eq!(nav_key("ArrowUp"), None);
    assert_eq!(nav_key("arrowright"), None);
}

#[test]
fn trap_edge_cases() {
    assert_eq!(trap_index(None, 0, false), None);
    assert_eq!(trap_index(Some(0), 0, true), None);
    assert_eq!(trap_index(Some(0), 1, false), Some(0));
    assert_eq!(trap_index(Some(0), 1, true), Some(0));
    assert_eq!(trap_index(None, 3, true), Some(2));
}

#[test]
fn queue_remove_missing_is_noop() {
    let mut q = vec![1u32, 2];
    assert!(!queue_remove(&mut q, &7, |x| *x));
    assert_eq!(q, vec![1, 2]);
    queue_push(&mut q, 3, |x| *x, 2);
    assert_eq!(q.len(), 2, "cap enforced");
}

#[test]
fn select_all_empty_table() {
    // Must not panic or report "all" for an empty table.
    let s = select_all_state(0, 0);
    assert_ne!(s, select_all_state(3, 3));
    assert!(toggle_all::<&str>(&[], &[]).is_empty());
}

#[test]
fn safe_href_more_vectors() {
    for bad in [
        "\u{0}javascript:alert(1)",
        "\r\njavascript:alert(1)",
        "javascript\t:alert(1)",
        "DATA:text/html;base64,AAAA",
        "ftp://x",
        "chrome://settings",
    ] {
        assert_eq!(safe_href(bad), "#", "{bad:?}");
    }
    // Safe hrefs are returned unchanged (not normalised).
    assert_eq!(safe_href("/t/a?b=c#d"), "/t/a?b=c#d");
}

#[test]
fn theme_allow_list_rejects_near_misses() {
    for bad in [
        "LIGHT",
        " light",
        "light\n",
        "dark;",
        "system\0",
        "null",
        "undefined",
        "auto",
    ] {
        assert_eq!(Theme::parse(bad), None, "{bad:?}");
    }
    // Every data-theme value written to the DOM is one the CSS knows.
    for t in [Theme::Light, Theme::Dark] {
        let v = t.data_theme().unwrap();
        assert!(
            TOKENS.contains(&format!("data-theme=\"{v}\"")),
            "tokens.css has no selector for data-theme={v}"
        );
    }
    assert_eq!(Theme::default(), Theme::System);
}

#[test]
fn grid_cols_always_has_a_class() {
    for n in 0..=255u8 {
        let c = grid_cols(n);
        assert!(
            COMPONENTS.contains(&format!("cui-grid--cols-{c}")),
            "no CSS class for grid cols {c}"
        );
    }
}

// ---- stylesheet / HTML guards ----

fn raw_color_hits(css: &str) -> Vec<String> {
    let css = strip_comments(css);
    let mut hits = Vec::new();
    for (n, line) in css.lines().enumerate() {
        let l = line.trim();
        if l.starts_with("--") {
            continue; // token definitions are allowed (tokens.css only)
        }
        let lower = l.to_ascii_lowercase();
        let has_hex = lower.match_indices('#').any(|(i, _)| {
            let h: String = lower[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            // ignore id selectors like `#app-chrome` (followed by non-hex)
            matches!(h.len(), 3 | 4 | 6 | 8)
                && lower[i + 1 + h.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                && l.contains(':')
        });
        if has_hex || lower.contains("rgb(") || lower.contains("rgba(") || lower.contains("hsl(") {
            hits.push(format!("{}: {l}", n + 1));
        }
    }
    hits
}

#[test]
fn component_and_shell_css_use_tokens_only() {
    let shell = repo_file("console-shell/assets/shell.css");
    for (name, css) in [
        ("components.css", COMPONENTS),
        ("shell.css", shell.as_str()),
    ] {
        let hits = raw_color_hits(css);
        assert!(hits.is_empty(), "{name} raw colours:\n{}", hits.join("\n"));
    }
}

fn defined_tokens() -> BTreeSet<String> {
    strip_comments(TOKENS)
        .split(['{', ';', '}'])
        .filter_map(|d| d.trim().split_once(':'))
        .map(|(k, _)| k.trim().to_owned())
        .filter(|k| k.starts_with("--cui-"))
        .collect()
}

#[test]
fn every_var_reference_is_defined() {
    let defined = defined_tokens();
    assert!(defined.len() > 20, "suspiciously few tokens parsed");
    let shell = repo_file("console-shell/assets/shell.css");
    let mut missing = Vec::new();
    for (name, css) in [
        ("tokens.css", TOKENS),
        ("components.css", COMPONENTS),
        ("shell.css", shell.as_str()),
    ] {
        let css = strip_comments(css);
        for (i, _) in css.match_indices("var(--") {
            let rest = &css[i + 4..];
            let end = rest.find([',', ')']).unwrap_or(rest.len());
            let name_ = rest[..end].trim();
            if name_.starts_with("--cui-") && !defined.contains(name_) {
                missing.push(format!("{name}: {name_}"));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "undefined tokens:\n{}",
        missing.join("\n")
    );
}

#[test]
fn reduced_motion_is_honoured() {
    assert!(
        TOKENS.contains("prefers-reduced-motion") || COMPONENTS.contains("prefers-reduced-motion"),
        "no prefers-reduced-motion handling"
    );
}

#[test]
fn index_html_is_csp_clean() {
    let html = repo_file("console-shell/index.html").to_ascii_lowercase();
    assert!(
        !html.contains(" style="),
        "inline style attribute in index.html"
    );
    assert!(!html.contains("<style"), "<style> element in index.html");
    // Trunk injects the wasm loader itself; no hand-written inline scripts.
    for (i, _) in html.match_indices("<script") {
        let tag_end = html[i..].find('>').map_or(html.len(), |j| i + j);
        assert!(
            html[i..tag_end].contains("src=") || html[i..tag_end].contains("data-trunk"),
            "inline <script> in index.html"
        );
    }
    assert!(html.contains("console-ui/assets/tokens.css"));
    assert!(html.contains("console-ui/assets/components.css"));
    assert!(
        !Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../console-shell/assets/tokens.css")
            .exists(),
        "stale shell tokens.css still present"
    );
}

#[test]
fn exported_css_consts_match_files() {
    assert_eq!(console_ui::TOKENS_CSS, TOKENS);
    assert_eq!(console_ui::COMPONENTS_CSS, COMPONENTS);
    assert!(!console_ui::COMPONENTS_CSS.contains("@import url(http"));
}

// ---- M2: A11Y-007 px guard and dropdown index ----

/// Lines of `css` containing a raw `<digits>px` length, ignoring comments and
/// lines marked `/* px-ok */` (a documented exception).
fn raw_px_hits(css: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (n, line) in css.lines().enumerate() {
        if line.contains("px-ok") {
            continue;
        }
        let code = strip_comments(line);
        let b = code.as_bytes();
        let found = code
            .match_indices("px")
            .any(|(i, _)| i > 0 && b[i - 1].is_ascii_digit());
        if found {
            hits.push(format!("{}: {}", n + 1, line.trim()));
        }
    }
    hits
}

#[test]
fn components_css_has_no_raw_px() {
    let hits = raw_px_hits(COMPONENTS);
    assert!(
        hits.is_empty(),
        "raw px in components.css (use a token or mark /* px-ok */):\n{}",
        hits.join("\n")
    );
    // The guard itself must catch a raw value.
    assert_eq!(raw_px_hits("a { width: 18px; }").len(), 1);
    assert!(raw_px_hits("a { width: 1px; /* px-ok: why */ }").is_empty());
    assert!(raw_px_hits("/* 24px in a comment */").is_empty());
}

#[test]
fn px_ok_exceptions_are_few_and_explained() {
    let n = COMPONENTS
        .lines()
        .filter(|l| l.contains("px-ok"))
        .inspect(|l| assert!(l.contains("px-ok:"), "px-ok without reason: {l}"))
        .count();
    assert!(n <= 6, "too many px-ok exceptions: {n}");
}

#[test]
fn a11y007_tokens_present() {
    for t in [
        "--cui-border-width-s: 1px",
        "--cui-border-width-m: 2px",
        "--cui-border-width-l: 4px",
        "--cui-size-control-check: 1.5rem",
        "--cui-size-target-min: 1.5rem",
        "--cui-size-toggle-w: 2.25rem",
        "--cui-size-toggle-h: 1.25rem",
        "--cui-size-toggle-knob: 0.75rem",
        "--cui-size-modal-w: 40rem",
        "--cui-size-dropdown-w: 18rem",
        "--cui-size-arrow: 0.3125rem",
        "--cui-z-dropdown: 900",
    ] {
        assert!(TOKENS.contains(t), "missing token {t}");
    }
    let checkbox = COMPONENTS
        .split(".cui-checkbox__input {")
        .nth(1)
        .and_then(|s| s.split('}').next())
        .expect("checkbox rule");
    assert!(checkbox.contains("var(--cui-size-control-check)"));
}

#[test]
fn dropdown_below_modal_and_flash() {
    let z = |name: &str| -> u32 {
        let i = TOKENS.find(name).expect(name) + name.len();
        TOKENS[i..]
            .trim_start_matches([':', ' '])
            .split(';')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    assert!(z("--cui-z-dropdown") < z("--cui-z-modal"));
    assert!(z("--cui-z-modal") < z("--cui-z-flash"));
}

#[test]
fn dropdown_next_index_edges() {
    assert_eq!(dropdown_next_index(0, 0, 1), 0);
    assert_eq!(dropdown_next_index(0, 0, -1), 0);
    assert_eq!(dropdown_next_index(4, 3, 1), 0);
    assert_eq!(dropdown_next_index(4, 0, -1), 3);
    assert_eq!(dropdown_next_index(4, 1, 5), 2);
    assert_eq!(dropdown_next_index(4, 1, -5), 0);
    assert_eq!(dropdown_next_index(4, 100, -1), 2);
}

#[test]
fn new_components_have_css() {
    for c in [
        ".cui-topnav {",
        ".cui-dropdown__panel {",
        ".cui-dropdown__item {",
        ".cui-search {",
        ".cui-navtoggle {",
    ] {
        assert!(COMPONENTS.contains(c), "missing rule {c}");
    }
    // Panel visibility relies on the `hidden` attribute, never inline style.
    assert!(COMPONENTS.contains(".cui-dropdown__panel[hidden]"));
}
