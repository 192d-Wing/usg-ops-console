//! Source guards: no inline styles (CSP `style-src 'self'`) and no raw HTML
//! injection in the console's WASM crates.

use std::fs;
use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn scan(banned: &[&str]) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in ["console-ui/src", "console-shell/src"] {
        rs_files(&root.join(c), &mut files);
    }
    assert!(!files.is_empty(), "no sources found");
    let mut hits = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).expect("read source");
        for (n, line) in text.lines().enumerate() {
            for b in banned {
                if line.contains(b) {
                    hits.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
                }
            }
        }
    }
    hits
}

#[test]
fn no_inline_styles() {
    // Split so this file does not match itself if it is ever moved into src.
    let pat = ["style", "="].concat();
    let pats = [pat.as_str(), ".set_property(\"", "style()"];
    let hits = scan(&pats);
    assert!(
        hits.is_empty(),
        "inline style usage found:\n{}",
        hits.join("\n")
    );
}

#[test]
fn no_raw_html() {
    let pats = [
        ["inner", "_html"].concat(),
        ["from_html", "_unchecked"].concat(),
        ["set_inner", "_html"].concat(),
    ];
    let refs: Vec<&str> = pats.iter().map(String::as_str).collect();
    let hits = scan(&refs);
    assert!(
        hits.is_empty(),
        "raw HTML injection found:\n{}",
        hits.join("\n")
    );
}

/// NIST 800-53: CM-7 - the gallery must never be a default feature of the shell.
#[test]
fn gallery_not_default_feature() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let toml = fs::read_to_string(root.join("console-shell/Cargo.toml")).expect("read toml");
    let default = toml
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("default") && l.contains('='))
        .expect("console-shell declares [features] default");
    assert!(
        !default.contains("gallery"),
        "gallery must not be a default feature: {default}"
    );
}

/// Hex colours (`#rrggbb`, lower-cased) appearing in `text`.
fn hexes(text: &str) -> std::collections::BTreeSet<String> {
    let b = text.as_bytes();
    let mut out = std::collections::BTreeSet::new();
    for (i, _) in text.match_indices('#') {
        let h = &b[i + 1..];
        let n = h.iter().take_while(|c| c.is_ascii_hexdigit()).count();
        let next_ok = h
            .get(n)
            .is_none_or(|c| !c.is_ascii_alphanumeric() && *c != b'_');
        if n == 6 && next_ok {
            out.insert(text[i..i + 7].to_ascii_lowercase());
        }
    }
    out
}

/// The gateway's static AUTH_CSS duplicates token values literally (the
/// gateway must not depend on console-ui); every hex must be a real token.
#[test]
fn auth_css_hexes_are_tokens() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let routes =
        fs::read_to_string(root.join("console-gateway/src/routes.rs")).expect("read routes.rs");
    let start = routes
        .find("const AUTH_CSS")
        .expect("const AUTH_CSS in routes.rs");
    let body = &routes[start..];
    // End = first line ending in an unescaped `";` (inner quotes are `\"`).
    let end = body
        .match_indices("\";\n")
        .map(|(i, _)| i)
        .find(|i| *i > 0 && body.as_bytes()[i - 1] != b'\\')
        .expect("end of AUTH_CSS");
    let auth = &body[..end];
    let tokens = hexes(&console_ui::TOKENS_CSS.to_ascii_lowercase());
    let found = hexes(auth);
    assert!(!found.is_empty(), "no hex colours found in AUTH_CSS");
    let stray: Vec<_> = found.difference(&tokens).collect();
    assert!(
        stray.is_empty(),
        "AUTH_CSS hexes not in tokens.css: {stray:?}"
    );
}
