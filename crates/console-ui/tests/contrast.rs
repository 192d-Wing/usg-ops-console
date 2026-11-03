//! WCAG 2.1 contrast check of `assets/tokens.css` in both themes.
//! Run with `cargo test -p console-ui --test contrast -- --nocapture` to print
//! the evidence table.

use std::collections::BTreeMap;

const TOKENS: &str = include_str!("../assets/tokens.css");

/// (foreground, background, minimum ratio). 4.5 = normal text, 3.0 = UI
/// component boundaries / focus indicators (WCAG 1.4.11).
const PAIRS: &[(&str, &str, f64)] = &[
    ("text", "bg", 4.5),
    ("text", "surface", 4.5),
    ("text", "surface-raised", 4.5),
    ("text", "surface-hover", 4.5),
    ("text", "accent-subtle", 4.5),
    ("text-muted", "bg", 4.5),
    ("text-muted", "surface", 4.5),
    ("text-muted", "surface-raised", 4.5),
    ("accent-text", "bg", 4.5),
    ("accent-text", "surface", 4.5),
    ("accent-text", "surface-raised", 4.5),
    ("accent-text", "accent-subtle", 4.5),
    ("on-accent", "accent", 4.5),
    ("on-accent", "accent-hover", 4.5),
    ("topbar-text", "topbar-bg", 4.5),
    ("info", "info-bg", 4.5),
    ("success", "success-bg", 4.5),
    ("warning", "warning-bg", 4.5),
    ("error", "error-bg", 4.5),
    ("text", "info-bg", 4.5),
    ("text", "success-bg", 4.5),
    ("text", "warning-bg", 4.5),
    ("text", "error-bg", 4.5),
    ("info", "surface", 4.5),
    ("success", "surface", 4.5),
    ("warning", "surface", 4.5),
    ("error", "surface", 4.5),
    ("error", "bg", 4.5),
    ("pending", "surface", 4.5),
    ("stopped", "surface", 4.5),
    ("border", "bg", 3.0),
    ("border", "surface", 3.0),
    ("border", "surface-raised", 3.0),
    ("border-disabled", "surface", 3.0),
    ("accent", "surface", 3.0),
    ("accent", "bg", 3.0),
    ("focus", "bg", 3.0),
    ("focus", "surface", 3.0),
    ("focus", "surface-raised", 3.0),
    ("focus", "topbar-bg", 3.0),
    // M2 chrome: dropdown panel (surface-raised, hover rows), top navigation,
    // search field on the top bar (text/surface-raised, topbar-text/topbar-bg
    // and text-muted/surface are already listed above).
    ("text-muted", "surface-hover", 4.5),
    ("accent-text", "surface-hover", 4.5),
    ("border", "topbar-bg", 3.0),
];

/// Extracts `--cui-color-*: #hex;` declarations from the first block that
/// follows `selector`.
fn block(css: &str, selector: &str) -> BTreeMap<String, String> {
    let start = css
        .find(selector)
        .unwrap_or_else(|| panic!("selector {selector} not found"));
    let open = start + css[start..].find('{').expect("open brace");
    let close = open + css[open..].find('}').expect("close brace");
    css[open + 1..close]
        .split(';')
        .filter_map(|decl| {
            // Drop a leading `/* comment */` before the property name.
            let decl = decl.rsplit("*/").next().unwrap_or(decl);
            let (k, v) = decl.split_once(':')?;
            let k = k.trim().strip_prefix("--cui-color-")?;
            Some((k.to_owned(), v.trim().to_owned()))
        })
        .collect()
}

fn hex(v: &str) -> Option<[f64; 3]> {
    let h = v.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let c = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|b| b as f64 / 255.0)
    };
    Some([c(0)?, c(2)?, c(4)?])
}

fn luminance(rgb: [f64; 3]) -> f64 {
    let lin = |c: f64| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

fn ratio(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[test]
fn ratio_reference_values() {
    assert!((ratio([0.0; 3], [1.0; 3]) - 21.0).abs() < 1e-9);
    assert!((ratio([1.0; 3], [1.0; 3]) - 1.0).abs() < 1e-9);
}

#[test]
fn dark_blocks_are_identical() {
    let media = block(TOKENS, ":root:not([data-theme=\"light\"])");
    let forced = block(TOKENS, ":root[data-theme=\"dark\"]");
    assert_eq!(
        media, forced,
        "media-query dark block and data-theme=dark block differ"
    );
}

#[test]
fn every_dark_color_overrides_a_light_color() {
    let light = block(TOKENS, ":root {");
    let dark = block(TOKENS, ":root[data-theme=\"dark\"]");
    for k in light.keys() {
        assert!(dark.contains_key(k), "dark theme missing --cui-color-{k}");
    }
}

#[test]
fn token_pairs_meet_wcag_aa() {
    let light = block(TOKENS, ":root {");
    let mut dark = light.clone();
    dark.extend(block(TOKENS, ":root[data-theme=\"dark\"]"));
    let mut failures = Vec::new();
    println!("| theme | foreground | background | ratio | min | result |");
    println!("|---|---|---|---|---|---|");
    for (theme, map) in [("light", &light), ("dark", &dark)] {
        for (fg, bg, min) in PAIRS {
            let get = |k: &str| {
                let v = map
                    .get(k)
                    .unwrap_or_else(|| panic!("{theme}: --cui-color-{k} missing"));
                hex(v).unwrap_or_else(|| panic!("{theme}: --cui-color-{k} is not #rrggbb: {v}"))
            };
            let r = ratio(get(fg), get(bg));
            let ok = r >= *min;
            println!(
                "| {theme} | {fg} ({}) | {bg} ({}) | {r:.2} | {min} | {} |",
                map[*fg],
                map[*bg],
                if ok { "pass" } else { "FAIL" }
            );
            if !ok {
                failures.push(format!("{theme}: {fg}/{bg} = {r:.2} < {min}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "contrast failures:\n{}",
        failures.join("\n")
    );
}
