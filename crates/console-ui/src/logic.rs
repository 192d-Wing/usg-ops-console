//! Pure, Yew-free logic behind the components. Everything here is unit-tested
//! natively so behaviour does not depend on a browser.

/// Keyboard intent for roving-focus widgets (tabs, menus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Prev,
    Next,
    First,
    Last,
}

/// Maps a `KeyboardEvent.key` value to a horizontal roving-focus intent.
pub fn nav_key(key: &str) -> Option<NavKey> {
    match key {
        "ArrowLeft" => Some(NavKey::Prev),
        "ArrowRight" => Some(NavKey::Next),
        "Home" => Some(NavKey::First),
        "End" => Some(NavKey::Last),
        _ => None,
    }
}

/// Next index for a roving-focus widget. `enabled[i]` is false for disabled
/// entries, which are skipped. Prev/Next wrap around. Returns `None` when no
/// entry is enabled.
pub fn roving_index(current: usize, enabled: &[bool], key: NavKey) -> Option<usize> {
    let len = enabled.len();
    if !enabled.iter().any(|e| *e) {
        return None;
    }
    match key {
        NavKey::First => enabled.iter().position(|e| *e),
        NavKey::Last => enabled.iter().rposition(|e| *e),
        NavKey::Next | NavKey::Prev => {
            let start = current.min(len - 1);
            (1..=len)
                .map(|step| {
                    if key == NavKey::Next {
                        (start + step) % len
                    } else {
                        (start + len - step % len) % len
                    }
                })
                .find(|i| enabled[*i])
        }
    }
}

/// Index of the dropdown item to focus after moving `delta` steps from `cur`
/// in a list of `len` items, wrapping at both ends. Returns 0 for an empty
/// list (callers check `len` before focusing). An out-of-range `cur` is
/// clamped to the last item first.
pub fn dropdown_next_index(len: usize, cur: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let cur = cur.min(len - 1) as isize;
    (cur + delta).rem_euclid(len as isize) as usize
}

/// Focus-trap target when Tab (or Shift+Tab when `backwards`) is pressed.
/// `current` is the index of the focused element within the trap's focusable
/// list, or `None` if focus is on the container itself or outside it.
pub fn trap_index(current: Option<usize>, len: usize, backwards: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (current, backwards) {
        (None, false) => 0,
        (None, true) => len - 1,
        (Some(i), false) => (i + 1) % len,
        (Some(i), true) => (i + len - 1) % len,
    })
}

/// Pushes `item` onto a notification queue. An existing item with the same
/// key is replaced in place (dedupe). The queue keeps at most `max` items,
/// dropping the oldest.
pub fn queue_push<T, K: PartialEq>(queue: &mut Vec<T>, item: T, key: impl Fn(&T) -> K, max: usize) {
    let k = key(&item);
    if let Some(slot) = queue.iter_mut().find(|q| key(q) == k) {
        *slot = item;
        return;
    }
    queue.push(item);
    if max > 0 && queue.len() > max {
        let excess = queue.len() - max;
        queue.drain(..excess);
    }
}

/// Removes every item with the given key. Returns whether anything was removed.
pub fn queue_remove<T, K: PartialEq>(queue: &mut Vec<T>, k: &K, key: impl Fn(&T) -> K) -> bool {
    let before = queue.len();
    queue.retain(|q| key(q) != *k);
    queue.len() != before
}

/// Tri-state of a "select all" checkbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriState {
    Unchecked,
    Mixed,
    Checked,
}

/// Where focus goes when the control at `idx` of `len` siblings is removed:
/// the next one, else the previous one, else `None` (use a fallback target).
pub fn removal_focus_index(idx: usize, len: usize) -> Option<usize> {
    if idx + 1 < len {
        Some(idx + 1)
    } else if idx > 0 && idx < len {
        Some(idx - 1)
    } else {
        None
    }
}

pub fn select_all_state(selected: usize, total: usize) -> TriState {
    if total == 0 || selected == 0 {
        TriState::Unchecked
    } else if selected >= total {
        TriState::Checked
    } else {
        TriState::Mixed
    }
}

/// New selection after the "select all" checkbox is activated: anything other
/// than fully checked selects all, fully checked clears.
pub fn toggle_all<K: Clone + PartialEq>(all: &[K], selected: &[K]) -> Vec<K> {
    let count = all.iter().filter(|k| selected.contains(k)).count();
    match select_all_state(count, all.len()) {
        TriState::Checked => Vec::new(),
        _ => all.to_vec(),
    }
}

/// Adds or removes one key from a selection, preserving order.
pub fn toggle_one<K: Clone + PartialEq>(selected: &[K], key: &K) -> Vec<K> {
    if selected.contains(key) {
        selected.iter().filter(|k| *k != key).cloned().collect()
    } else {
        let mut v = selected.to_vec();
        v.push(key.clone());
        v
    }
}

/// Returns `href` if its scheme is allow-listed (http, https, mailto, or no
/// scheme i.e. relative), otherwise `"#"`. Browsers ignore ASCII tab/newline
/// and leading control/space characters in URLs, so those are stripped before
/// the scheme is inspected.
// NIST 800-53: SI-10 - scheme allow-list for rendered hrefs; protocol-relative
// and backslash forms (which browsers resolve cross-origin) are rejected.
pub fn safe_href(href: &str) -> &str {
    let cleaned: String = href
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let cleaned = cleaned.trim_start_matches(|c: char| c <= ' ');
    // `//host`, `\\host`, `/\host`, `\/host` all resolve to another origin
    // (CWE-601); only single-slash paths count as same-origin relative.
    let mut lead = cleaned.chars();
    if matches!(
        (lead.next(), lead.next()),
        (Some('/' | '\\'), Some('/' | '\\'))
    ) {
        return "#";
    }
    let scheme_end = cleaned.find(':');
    let delim = cleaned.find(['/', '?', '#']);
    let scheme = match (scheme_end, delim) {
        (Some(c), Some(d)) if c < d => Some(&cleaned[..c]),
        (Some(c), None) => Some(&cleaned[..c]),
        _ => None,
    };
    match scheme.map(str::to_ascii_lowercase).as_deref() {
        None | Some("http") | Some("https") | Some("mailto") => href,
        _ => "#",
    }
}

/// Theme preference persisted by the shell (`localStorage["cui-theme"]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

impl Theme {
    /// Parses an allow-listed stored value; anything else is `None`.
    // NIST 800-53: SI-10 - only allow-listed theme values from localStorage
    // are accepted; anything else is ignored.
    pub fn parse(s: &str) -> Option<Theme> {
        match s {
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            "system" => Some(Theme::System),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::System => "system",
        }
    }

    /// Value for `<html data-theme>`; `None` means remove the attribute and
    /// follow `prefers-color-scheme`.
    pub fn data_theme(self) -> Option<&'static str> {
        match self {
            Theme::Light => Some("light"),
            Theme::Dark => Some("dark"),
            Theme::System => None,
        }
    }
}

/// Supported grid column counts (each has a `cui-grid--cols-N` class).
pub const GRID_COLS: [u8; 6] = [1, 2, 3, 4, 6, 12];

/// Clamps a requested column count to the largest supported value <= `n`
/// (minimum 1).
pub fn grid_cols(n: u8) -> u8 {
    GRID_COLS
        .iter()
        .rev()
        .copied()
        .find(|c| *c <= n)
        .unwrap_or(1)
}

/// localStorage key for the rail preference (per browser profile).
pub const RAIL_STORAGE_KEY: &str = "usg-console.rail";

/// Wide-viewport rail presentation. Narrow viewports always start hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RailMode {
    #[default]
    Expanded,
    Collapsed,
}

impl RailMode {
    /// Parses an allow-listed stored value; anything else is `None`.
    // NIST 800-53: SI-10 - only allow-listed rail values from localStorage
    // are accepted; anything else falls back to the default.
    pub fn parse(s: &str) -> Option<RailMode> {
        match s {
            "expanded" => Some(RailMode::Expanded),
            "collapsed" => Some(RailMode::Collapsed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RailMode::Expanded => "expanded",
            RailMode::Collapsed => "collapsed",
        }
    }

    pub fn toggled(self) -> RailMode {
        match self {
            RailMode::Expanded => RailMode::Collapsed,
            RailMode::Collapsed => RailMode::Expanded,
        }
    }
}

/// Maximum number of command-palette results shown.
pub const PALETTE_MAX_RESULTS: usize = 50;

/// How a palette item matched the query; lower is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchTier {
    Exact,
    Prefix,
    WordPrefix,
    Substring,
    Keyword,
    Subsequence,
}

/// One ranked result: index into the input, tier, and the byte range of the
/// label to highlight (contiguous label matches only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub tier: Option<MatchTier>,
    pub highlight: Option<(usize, usize)>,
}

/// Lower-cases `s` and returns it with a map from each folded byte offset to
/// the original byte offset (so highlights index the original label).
fn fold(s: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len() + 1);
    for (i, c) in s.char_indices() {
        for l in c.to_lowercase() {
            for _ in 0..l.len_utf8() {
                map.push(i);
            }
            out.push(l);
        }
    }
    map.push(s.len());
    (out, map)
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut h = hay.chars();
    needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|n| h.any(|c| c == n))
}

/// A palette item with its label and keywords case-folded once (PERF-003):
/// build with [`fold_palette`] when the item list changes, then rank each
/// keystroke with [`rank_folded`] without re-allocating per item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldedItem {
    label: String,
    map: Vec<usize>,
    keywords: Vec<String>,
    group: u8,
}

/// Folds `(label, keywords, group_order)` items for [`rank_folded`].
pub fn fold_palette(items: &[(&str, &[&str], u8)]) -> Vec<FoldedItem> {
    items
        .iter()
        .map(|(label, kw, group)| {
            let (label, map) = fold(label);
            FoldedItem {
                label,
                map,
                keywords: kw.iter().map(|k| k.to_lowercase()).collect(),
                group: *group,
            }
        })
        .collect()
}

/// Scores one folded item against an already folded, trimmed, non-empty
/// query. Kept out of line so the ranking stays one small function (PERF-002).
#[inline(never)]
fn tier_for(q: &str, it: &FoldedItem) -> (Option<MatchTier>, Option<(usize, usize)>) {
    let (fl, map) = (it.label.as_str(), it.map.as_slice());
    let span = |start: usize| Some((map[start], map[start + q.len()]));
    if fl == q {
        return (Some(MatchTier::Exact), span(0));
    }
    if fl.starts_with(q) {
        return (Some(MatchTier::Prefix), span(0));
    }
    let word = fl.match_indices(q).find(|(i, _)| {
        fl[..*i]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_alphanumeric())
    });
    if let Some((i, _)) = word {
        return (Some(MatchTier::WordPrefix), span(i));
    }
    if let Some(i) = fl.find(q) {
        return (Some(MatchTier::Substring), span(i));
    }
    let token_hit = |t: &str| fl.contains(t) || it.keywords.iter().any(|k| k.contains(t));
    if q.split_whitespace().all(token_hit) {
        return (Some(MatchTier::Keyword), None);
    }
    if is_subsequence(q, fl) {
        return (Some(MatchTier::Subsequence), None);
    }
    (None, None)
}

/// Ranks palette items `(label, keywords, group_order)` for `query`.
///
/// Order: tier (exact > prefix > word-boundary prefix > substring > keyword >
/// subsequence), then group order, then label (case-folded), then input
/// order. An empty query returns every item sorted by group, keeping input
/// order within a group. Results are capped at [`PALETTE_MAX_RESULTS`].
pub fn rank_palette(query: &str, items: &[(&str, &[&str], u8)]) -> Vec<Ranked> {
    rank_folded(query, &fold_palette(items))
}

/// [`rank_palette`] over items already folded with [`fold_palette`].
pub fn rank_folded(query: &str, items: &[FoldedItem]) -> Vec<Ranked> {
    let q = fold(query.trim()).0;
    let mut out: Vec<Ranked> = items
        .iter()
        .enumerate()
        .filter_map(|(index, it)| {
            if q.is_empty() {
                return Some(Ranked {
                    index,
                    tier: None,
                    highlight: None,
                });
            }
            let (tier, highlight) = tier_for(&q, it);
            tier.map(|t| Ranked {
                index,
                tier: Some(t),
                highlight,
            })
        })
        .collect();
    // Index tie-break makes the order total, so an unstable sort is
    // deterministic and keeps input order for equal keys (PERF-002).
    let label = |r: &Ranked| {
        if q.is_empty() {
            ""
        } else {
            items[r.index].label.as_str()
        }
    };
    out.sort_unstable_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(items[a.index].group.cmp(&items[b.index].group))
            .then_with(|| label(a).cmp(label(b)))
            .then(a.index.cmp(&b.index))
    });
    out.truncate(PALETTE_MAX_RESULTS);
    out
}

/// Splits `label` into `(text, highlighted)` runs for a highlight range.
pub fn highlight_runs(label: &str, hl: Option<(usize, usize)>) -> Vec<(&str, bool)> {
    match hl {
        Some((s, e))
            if s < e
                && e <= label.len()
                && label.is_char_boundary(s)
                && label.is_char_boundary(e) =>
        {
            [
                (&label[..s], false),
                (&label[s..e], true),
                (&label[e..], false),
            ]
            .into_iter()
            .filter(|(t, _)| !t.is_empty())
            .collect()
        }
        _ => vec![(label, false)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_keys() {
        assert_eq!(nav_key("ArrowRight"), Some(NavKey::Next));
        assert_eq!(nav_key("ArrowLeft"), Some(NavKey::Prev));
        assert_eq!(nav_key("Home"), Some(NavKey::First));
        assert_eq!(nav_key("End"), Some(NavKey::Last));
        assert_eq!(nav_key("ArrowDown"), None);
        assert_eq!(nav_key("a"), None);
    }

    #[test]
    fn roving_wraps_and_skips_disabled() {
        let all = [true, true, true];
        assert_eq!(roving_index(0, &all, NavKey::Next), Some(1));
        assert_eq!(roving_index(2, &all, NavKey::Next), Some(0));
        assert_eq!(roving_index(0, &all, NavKey::Prev), Some(2));
        assert_eq!(roving_index(1, &all, NavKey::First), Some(0));
        assert_eq!(roving_index(1, &all, NavKey::Last), Some(2));
        let some = [true, false, true, false];
        assert_eq!(roving_index(0, &some, NavKey::Next), Some(2));
        assert_eq!(roving_index(2, &some, NavKey::Next), Some(0));
        assert_eq!(roving_index(0, &some, NavKey::Prev), Some(2));
        assert_eq!(roving_index(0, &some, NavKey::Last), Some(2));
        let first_off = [false, true];
        assert_eq!(roving_index(1, &first_off, NavKey::First), Some(1));
        assert_eq!(roving_index(1, &first_off, NavKey::Next), Some(1));
        assert_eq!(roving_index(0, &[false, false], NavKey::Next), None);
        assert_eq!(roving_index(0, &[], NavKey::First), None);
        // Out-of-range current is clamped.
        assert_eq!(roving_index(9, &all, NavKey::Next), Some(0));
    }

    #[test]
    fn dropdown_index_wraps() {
        assert_eq!(dropdown_next_index(0, 0, 1), 0);
        assert_eq!(dropdown_next_index(0, 5, -1), 0);
        assert_eq!(dropdown_next_index(3, 0, 1), 1);
        assert_eq!(dropdown_next_index(3, 2, 1), 0);
        assert_eq!(dropdown_next_index(3, 0, -1), 2);
        assert_eq!(dropdown_next_index(1, 0, 1), 0);
        assert_eq!(dropdown_next_index(3, 9, 1), 0);
        assert_eq!(dropdown_next_index(3, 1, 0), 1);
    }

    #[test]
    fn trap_wraps() {
        assert_eq!(trap_index(None, 0, false), None);
        assert_eq!(trap_index(None, 3, false), Some(0));
        assert_eq!(trap_index(None, 3, true), Some(2));
        assert_eq!(trap_index(Some(2), 3, false), Some(0));
        assert_eq!(trap_index(Some(0), 3, true), Some(2));
        assert_eq!(trap_index(Some(1), 3, false), Some(2));
        assert_eq!(trap_index(Some(0), 1, true), Some(0));
    }

    #[test]
    fn queue_add_dedupe_cap_dismiss() {
        let mut q: Vec<(u32, &str)> = Vec::new();
        let key = |x: &(u32, &str)| x.0;
        queue_push(&mut q, (1, "a"), key, 3);
        queue_push(&mut q, (2, "b"), key, 3);
        queue_push(&mut q, (1, "a2"), key, 3);
        assert_eq!(q, vec![(1, "a2"), (2, "b")]);
        queue_push(&mut q, (3, "c"), key, 3);
        queue_push(&mut q, (4, "d"), key, 3);
        assert_eq!(q, vec![(2, "b"), (3, "c"), (4, "d")]);
        assert!(queue_remove(&mut q, &3, key));
        assert!(!queue_remove(&mut q, &3, key));
        assert_eq!(q, vec![(2, "b"), (4, "d")]);
        // max 0 = unbounded
        let mut u: Vec<(u32, &str)> = Vec::new();
        for i in 0..10 {
            queue_push(&mut u, (i, ""), key, 0);
        }
        assert_eq!(u.len(), 10);
    }

    #[test]
    fn select_all_tristate() {
        assert_eq!(select_all_state(0, 0), TriState::Unchecked);
        assert_eq!(select_all_state(0, 3), TriState::Unchecked);
        assert_eq!(select_all_state(1, 3), TriState::Mixed);
        assert_eq!(select_all_state(3, 3), TriState::Checked);
        let all = ["a", "b", "c"];
        assert_eq!(toggle_all(&all, &[]), all.to_vec());
        assert_eq!(toggle_all(&all, &["b"]), all.to_vec());
        assert!(toggle_all(&all, &["a", "b", "c"]).is_empty());
        // Stale keys not in `all` do not count towards "checked".
        assert_eq!(toggle_all(&all, &["a", "b", "x"]), all.to_vec());
        assert_eq!(toggle_one(&["a"], &"b"), vec!["a", "b"]);
        assert_eq!(toggle_one(&["a", "b"], &"a"), vec!["b"]);
    }

    #[test]
    fn removal_focus() {
        assert_eq!(removal_focus_index(0, 3), Some(1));
        assert_eq!(removal_focus_index(2, 3), Some(1));
        assert_eq!(removal_focus_index(0, 1), None);
        assert_eq!(removal_focus_index(5, 3), None);
    }

    #[test]
    fn safe_href_allow_list() {
        for ok in [
            "/gallery",
            "relative/path",
            "#frag",
            "?q=1",
            "https://example.com",
            "HTTP://example.com",
            "mailto:ops@example.com",
            "/a:b",
            "",
        ] {
            assert_eq!(safe_href(ok), ok, "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(1)",
            "java\tscript:alert(1)",
            "java\nscript:alert(1)",
            "\u{1}javascript:alert(1)",
            "data:text/html,<script>",
            "vbscript:x",
            "file:///etc/passwd",
            "blob:https://x/y",
            "//evil.example/x",
            "\\\\evil.example/x",
            "/\\evil.example",
            "\\/evil.example",
            " //evil.example",
            "/\t/evil.example",
        ] {
            assert_eq!(safe_href(bad), "#", "{bad:?}");
        }
    }

    #[test]
    fn theme_parse() {
        assert_eq!(Theme::parse("light"), Some(Theme::Light));
        assert_eq!(Theme::parse("dark"), Some(Theme::Dark));
        assert_eq!(Theme::parse("system"), Some(Theme::System));
        assert_eq!(Theme::parse("Dark"), None);
        assert_eq!(Theme::parse("\"><script>"), None);
        assert_eq!(Theme::parse(""), None);
        for t in [Theme::Light, Theme::Dark, Theme::System] {
            assert_eq!(Theme::parse(t.as_str()), Some(t));
        }
        assert_eq!(Theme::System.data_theme(), None);
        assert_eq!(Theme::Dark.data_theme(), Some("dark"));
    }

    #[test]
    fn grid_cols_clamp() {
        assert_eq!(grid_cols(0), 1);
        assert_eq!(grid_cols(1), 1);
        assert_eq!(grid_cols(4), 4);
        assert_eq!(grid_cols(5), 4);
        assert_eq!(grid_cols(7), 6);
        assert_eq!(grid_cols(12), 12);
        assert_eq!(grid_cols(200), 12);
    }

    #[test]
    fn rail_mode_parse() {
        assert_eq!(RailMode::parse("expanded"), Some(RailMode::Expanded));
        assert_eq!(RailMode::parse("collapsed"), Some(RailMode::Collapsed));
        for bad in ["", "Collapsed", "true", "collapsed ", "\"><x>"] {
            assert_eq!(RailMode::parse(bad), None, "{bad:?}");
        }
        assert_eq!(RailMode::default(), RailMode::Expanded);
        for m in [RailMode::Expanded, RailMode::Collapsed] {
            assert_eq!(RailMode::parse(m.as_str()), Some(m));
            assert_eq!(m.toggled().toggled(), m);
        }
    }

    fn labels<'a>(items: &[(&'a str, &[&str], u8)], r: &[Ranked]) -> Vec<&'a str> {
        r.iter().map(|x| items[x.index].0).collect()
    }

    #[test]
    fn palette_tiers_in_order() {
        let none: &[&str] = &[];
        let items: Vec<(&str, &[&str], u8)> = vec![
            ("Hostname tool", none, 1), // substring "tool"? no: word prefix
            ("Tool", none, 1),          // exact
            ("Toolbox", none, 1),       // prefix
            ("Stooling", none, 1),      // substring
            ("Home", &["tool"], 0),     // keyword
            ("T o o l s", none, 4),     // subsequence
            ("Unrelated", none, 0),
        ];
        let r = rank_palette("TOOL", &items);
        assert_eq!(
            labels(&items, &r),
            vec![
                "Tool",
                "Toolbox",
                "Hostname tool",
                "Stooling",
                "Home",
                "T o o l s"
            ]
        );
        assert_eq!(r[0].tier, Some(MatchTier::Exact));
        assert_eq!(r[2].highlight, Some((9, 13)));
        assert_eq!(r[4].highlight, None);
    }

    #[test]
    fn palette_ties_group_then_alpha_and_empty_query() {
        let none: &[&str] = &[];
        let items: Vec<(&str, &[&str], u8)> = vec![
            ("Sign out", none, 4),
            ("Theme: dark", none, 3),
            ("beta", none, 1),
            ("Alpha", none, 1),
            ("Home", none, 0),
        ];
        assert_eq!(
            labels(&items, &rank_palette("   ", &items)),
            vec!["Home", "beta", "Alpha", "Theme: dark", "Sign out"]
        );
        assert_eq!(
            labels(&items, &rank_palette("a", &items)),
            vec!["Alpha", "beta", "Theme: dark"]
        );
    }

    #[test]
    fn palette_cap_case_and_unicode() {
        let none: &[&str] = &[];
        let names: Vec<String> = (0..120).map(|i| format!("item {i}")).collect();
        let items: Vec<(&str, &[&str], u8)> = names.iter().map(|n| (n.as_str(), none, 1)).collect();
        assert_eq!(rank_palette("", &items).len(), PALETTE_MAX_RESULTS);
        assert_eq!(rank_palette("ITEM", &items).len(), PALETTE_MAX_RESULTS);
        let u: Vec<(&str, &[&str], u8)> = vec![("Ünïcode Größe", none, 0)];
        let r = rank_palette("größe", &u);
        assert_eq!(r[0].tier, Some(MatchTier::WordPrefix));
        let (s, e) = r[0].highlight.unwrap();
        assert_eq!(&"Ünïcode Größe"[s..e], "Größe");
        assert!(rank_palette("zzz", &u).is_empty());
        assert!(rank_palette("<b>", &u).is_empty());
    }

    #[test]
    fn highlight_runs_split() {
        assert_eq!(
            highlight_runs("Hello", Some((1, 3))),
            vec![("H", false), ("el", true), ("lo", false)]
        );
        assert_eq!(highlight_runs("Hello", None), vec![("Hello", false)]);
        assert_eq!(
            highlight_runs("Hello", Some((0, 99))),
            vec![("Hello", false)]
        );
        assert_eq!(highlight_runs("Hello", Some((0, 5))), vec![("Hello", true)]);
    }

    #[test]
    fn palette_ranking_is_fast() {
        let names: Vec<String> = (0..500)
            .map(|i| format!("Tool number {i} for ops"))
            .collect();
        let kw: &[&str] = &["alpha", "beta"];
        let items: Vec<(&str, &[&str], u8)> = names.iter().map(|n| (n.as_str(), kw, 1)).collect();
        let t = std::time::Instant::now();
        for q in ["t", "tool", "ops", "zz", "tnfo"] {
            let _ = rank_palette(q, &items);
        }
        assert!(t.elapsed().as_millis() < 10, "{:?}", t.elapsed()); // 5 queries, < 2ms each
    }

    #[test]
    fn palette_folded_500_items_fast_and_equivalent() {
        let names: Vec<String> = (0..500)
            .map(|i| format!("Tool Número {i} for Ops"))
            .collect();
        let kw: &[&str] = &["Alpha", "beta"];
        let items: Vec<(&str, &[&str], u8)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), kw, (i % 5) as u8))
            .collect();
        let folded = fold_palette(&items);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            for q in ["t", "tool", "ops", "alpha", "zz", "tnfo", "número 4"] {
                let _ = rank_folded(q, &folded);
            }
        }
        // 140 keystrokes over 500 items; generous bound for debug CI hosts.
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        for q in ["", "t", "ALPHA", "número 4"] {
            assert_eq!(rank_folded(q, &folded), rank_palette(q, &items), "{q}");
        }
    }
}
