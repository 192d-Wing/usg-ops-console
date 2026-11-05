# console-ui

Yew 0.23 component library shared by the console shell and by tools. It has
design tokens for light and dark themes and targets WCAG 2.1 AA. Since M4.5
the look is an original "calm neutral" design: neutral greys, one pine teal
accent, 1px borders, soft radii, flat containers. API changes per release:
[CHANGELOG.md](CHANGELOG.md).

## Stylesheets

| File | Rust constant | Contents |
| --- | --- | --- |
| `assets/tokens.css` | `console_ui::TOKENS_CSS` | `--cui-*` custom properties only |
| `assets/components.css` | `console_ui::COMPONENTS_CSS` | `.cui-*` rules that use tokens only (no raw colours) |

`components.css` needs `tokens.css` and must be loaded after it. Test
`exported_css_consts_match_files` checks that the constants equal the files.

## Tokens

All tokens use the `--cui-` prefix. Groups:

- Colour (`--cui-color-*`): bg, surface, surface-raised, surface-sunken
  (M4.5), surface-hover, text,
  text-muted, accent, accent-hover, accent-text, accent-subtle, on-accent,
  border, border-disabled, topbar, and status colours info, success, warning,
  error, pending and stopped, each with a matching `-bg` where used. The
  accent is pine teal (`#0e7268` light, `#3fb8a9` dark). M4.5 also adds
  theme-independent primitives `--cui-color-neutral-0` to `-900` (light
  block only). `topbar` tokens alias surface and text (no inverse top bar).
  `border-disabled` currently has the same value as `border` in every
  theme (POAM-049).
- Typography (`--cui-font-*`): family, size scale, weight, line height.
- Spacing (`--cui-space-*`), radius (`--cui-radius-*`), shadow
  (`--cui-shadow-*`), focus ring (`--cui-focus-*`), motion
  (`--cui-motion-duration-*`), minimum target size
  (`--cui-size-target-min`), z-index (`--cui-z-*`; M4.5 adds
  `rail-overlay` 950, `palette` 1050, `critical` 1200).
- M4.5 additions: `--cui-font-size-xxl`, `--cui-font-weight-medium` and
  `-semibold` (`bold` is now 600), `--cui-space-sm`, `--cui-space-xxl`,
  `--cui-size-icon`, `-icon-s`, `-rail-expanded`, `-rail-collapsed`,
  `-topbar`, `-palette-w`, `-popover-w`. Motion is 100/160/240 ms with
  `cubic-bezier(.2,0,0,1)`. No token was removed or renamed.
- Border widths (`--cui-border-width-s|m|indicator|l`) and component sizes
  (`--cui-size-control-check`, `--cui-size-toggle-*`, `--cui-size-modal-w`,
  `--cui-size-dropdown-w`, `--cui-size-arrow`).

`components.css` contains no raw `px` lengths (test
`components_css_has_no_raw_px`). The only exceptions are lines marked
`/* px-ok: reason */` (for example the visually-hidden clip, forced-colors
outline and shrink-to-fit table column; 6 lines in total).

Cloudscape guard: test `no_cloudscape_values` (`tests/contrast.rs`) fails if
`tokens.css` or `components.css` contains any hex from a fixed list of
Cloudscape blue and navy values.

See `assets/tokens.css` for the full list and values.

Themes:

- `:root` is light.
- Dark applies under `prefers-color-scheme: dark` unless `<html
  data-theme="light">` is set, and always under `data-theme="dark"`. The two
  dark blocks must be identical (test `dark_blocks_are_identical`).
- `prefers-reduced-motion: reduce` sets motion durations to zero and slows the
  spinner.
- `console_ui::Theme` (`System`, `Light`, `Dark`) parses stored values against
  an allow-list and maps each value to its `data-theme` attribute.

Contrast: test `token_pairs_meet_wcag_aa` (`tests/contrast.rs`) checks every
declared foreground/background pair (sunken, accent-subtle and hover pairs
added in M4.5) in each theme: 4.5:1 for text and 3:1 for
UI and borders. Run it with
`cargo test -p console-ui --test contrast -- --nocapture` to print the table.
Combinations that are not declared are not checked. Test
`every_dark_color_has_a_light_counterpart` checks theme parity (the
`neutral-*` primitives are exempt).

## Components

Every component is a function component, exported from the crate root.

| Area | Components |
| --- | --- |
| Actions | `Button` (normal, primary, link; disabled; loading with `aria-busy`) |
| Layout | `Container`, `Header` (h1/h2, description, actions), `SpaceBetween`, `Grid` |
| Navigation | `Link`, `SideNavigation`, `Breadcrumbs`, `Tabs` (ARIA tabs keyboard pattern), `NavToggle` (icon button with `aria-expanded`/`aria-controls`; M4.5 `icon` prop, default `IconName::Menu`), `Rail` + `RailEntry` + `RailItem` (M4.5 icon rail; expanded or collapsed) |
| Chrome | `TopNavigation` (banner with start/brand/search/scope/utilities slots), `Dropdown` + `DropdownItem` (disclosure pattern, not `role=menu`; below 45rem the panel is anchored to the top bar so it stays in the viewport), `CommandPalette` + `PaletteItem` + `PaletteGroup` (M4.5), `SearchInput` (**deprecated**: `#[deprecated(note = "use CommandPalette; removal planned M6")]`; `role="search"` form, `maxlength`, submit prevented) |
| Media | `Icon` + `IconName` (M4.5; decorative inline SVG, `aria-hidden="true"`, `focusable="false"`, stroke `currentColor`) |
| Status | `Badge`, `StatusIndicator`, `Spinner`, `Alert`, `Flashbar` (with `flash_push` / `flash_dismiss`, max 5) |
| Forms | `FormField` (label, description, error, wired with `aria-describedby`), `Input`, `Select`, `Checkbox`, `Toggle` |
| Data | `Table` (columns, empty state, optional multi-select; needs `caption` and `item_key`) |
| Overlay | `Modal` (focus trap, Esc to dismiss, focus returned to opener, optional `inert_root`), `Popover` + `PopoverAlign` (M4.5 non-modal dialog) |

Notes:

- `Badge` / `BadgeColor` (M4.5): semantic variants `Neutral` (now the
  default; was `Grey`), `Info`, `Success`, `Warning`, `Error`. The legacy
  names `Grey`, `Blue`, `Green`, `Red`, `Yellow` stay as aliases with the
  same look. The enum is not `#[non_exhaustive]`, so exhaustive `match`es
  outside the crate break when variants are added; it becomes
  `#[non_exhaustive]` at the next breaking release (POAM-048).
- `TopNavigation` (M4.5): DOM order is `start` (nav toggle), brand,
  `search` (palette trigger), `scope`, `utilities`. Before M4.5 the brand
  came first.
- `Rail`: `<nav class="cui-rail" data-testid="side-navigation">`. Each link
  has an icon (or a two-letter monogram when `icon` is `None`) and a label.
  Collapsed, the label is visually hidden and remains the accessible name;
  an `aria-hidden` `.cui-rail__tip` shows on hover and focus. Escape hides
  the tips without moving focus or the pointer (`data-tip-dismissed` on the
  `<nav>`, cleared on link mouseleave, rail focusout or mode change), and
  the tip is hoverable. `RailItem::sub_page()` marks sub-pages, which get a
  distinct collapsed treatment. `RailMode` and `RAIL_STORAGE_KEY` live in
  `logic.rs`; `RailMode::parse` accepts only `expanded` and `collapsed`.
  The host stores the mode and binds the toggle.
- `CommandPalette`: modal dialog with an APG combobox (`maxlength` 256,
  `PALETTE_MAX_QUERY`) and listbox, `aria-activedescendant`, arrow and
  Home/End keys, a debounced (300 ms) polite count, a Tab trap and optional
  `inert_root`. Disabled items get `aria-disabled` and are skipped and not
  selectable. Labels and match highlights are text nodes. Focus returns to
  the opener; if that is `body` or gone, to the `fallback_focus` ids in
  order, and not at all if something else already took focus. It is mounted
  only while `open`. The host binds Ctrl/Cmd+K and closes it in
  `on_select`. Ranking is `logic::rank_palette` (or `fold_palette` plus
  `rank_folded`), capped at `PALETTE_MAX_RESULTS` (50).
- `Popover`: always rendered, `hidden` while closed, `role=dialog`. Opening
  focuses `initial_focus` or the first focusable element. Escape closes and
  focuses the anchor; an outside pointerdown or focus leaving for anything
  other than the anchor closes it. The host owns `open` and the trigger's
  `aria-expanded`. With `embedded`, the element is `display: contents` with
  no role and no handlers, so one tree can serve two layouts.
- Status, alert and flash types show a glyph and visually hidden text, so
  colour is never the only signal.
- `Breadcrumbs`: a non-last item with an empty `href` renders as plain text.
- `SideNavigation` dividers are decorative `<li aria-hidden="true">` so every
  list child is a list item.
- `Select` keeps the DOM `value` in sync with `selected` after every render,
  so Back/Forward updates show correctly.
- `Link`, `SideNavigation` and `Breadcrumbs` pass every `href` through
  `logic::safe_href`. Only `http:`, `https:`, `mailto:` and relative URLs are
  allowed. Anything else, including `//host` and backslash forms, becomes `#`.
- `Dropdown`: opening focuses the first item; ArrowUp/ArrowDown wrap, Home
  and End jump; Escape closes and returns focus to the button; focus moving
  outside or a pointerdown outside closes it. The document listener exists
  only while open. Items render only while open; the panel uses `hidden`.
  With `test_id = "x"` the button is `x-button`, panel `x-panel`, each item
  `x-item-{id}`.
- `Alert` `on_dismiss` does not move focus. The consumer must move it.
  `Flashbar` moves focus to a neighbouring flash, or to `fallback_focus_id`.
- Pure logic (keyboard navigation, focus trap, flash queue, selection,
  `safe_href`, theme and rail-mode parsing, palette ranking) lives in `src/logic.rs` and has unit tests.

To see every component in every state, run `make run` and open
<http://127.0.0.1:8080/gallery>.

## CSP rules

The gateway sends `style-src 'self'` and allows no inline script except
hashed bootstrap scripts. So console-ui and its users must follow these rules:

- **No `style=` attributes**, and no `.style()` or `set_property` calls. Use
  the enumerated `.cui-*` classes. Test `no_inline_styles` (`tests/guards.rs`)
  scans console-ui and console-shell source for these.
- No `<style>` elements and no raw HTML (`inner_html`,
  `Html::from_html_unchecked`). Test `no_raw_html` enforces this.
- Load stylesheets as files (`<link rel="stylesheet">`), never inline.

## Using the styles in a tool

A tool that uses console-ui must serve both stylesheets from its own origin,
in this order: tokens, then components.

- With Trunk, link the files directly:

  ```html
  <link data-trunk rel="css" href="path/to/console-ui/assets/tokens.css" />
  <link data-trunk rel="css" href="path/to/console-ui/assets/components.css" />
  ```

- Or write `console_ui::TOKENS_CSS` and `console_ui::COMPONENTS_CSS` to files
  in the tool's dist (for example from a build step) and link those files.

Do not put the constants into a `<style>` element at runtime, because
`style-src 'self'` blocks it. The theme follows `data-theme` on the tool
document's own `<html>` element. Since M4 the shell sends its theme to tools
with SDK v1 `Init` and `ThemeChanged` (see
[docs/INTEGRATING-A-TOOL.md](../../docs/INTEGRATING-A-TOOL.md)).

## Tests

```bash
cargo test -p console-ui
```

This runs the unit, contrast, guard and QA logic/CSS tests.

```bash
make wasm-test
```

This runs the browser tests in `tests/wasm.rs` with wasm-pack, in headless
Firefox by default (`make wasm-test WASM_BROWSER=chrome` to use Chrome). In
M2 they passed locally in headless Chrome and in the CI `wasm-test` job
(headless Firefox, 16 passed, CI run 41740455231), which closed POAM-003. See
`docs/compliance/POAM.md`.

M4.5 (local only, not yet run in CI): `cargo test -p console-ui` passes and
`make wasm-test WASM_BROWSER=chrome` runs 28 console-ui browser tests, all
passing (fixes round 3), including `icon_is_decorative_svg`,
`rail_collapsed_keeps_text_names`,
`palette_combobox_navigation_and_focus_return`,
`palette_empty_state_is_text_only`, `popover_open_escape_and_outside_click`,
`popover_closes_on_focus_out_without_moving_focus` and
`popover_embedded_keeps_children`.
