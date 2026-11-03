# console-ui

Yew 0.23 component library (Cloudscape-like) shared by the console shell and
by tools. It has design tokens for light and dark themes and targets WCAG 2.1
AA.

## Stylesheets

| File | Rust constant | Contents |
| --- | --- | --- |
| `assets/tokens.css` | `console_ui::TOKENS_CSS` | `--cui-*` custom properties only |
| `assets/components.css` | `console_ui::COMPONENTS_CSS` | `.cui-*` rules that use tokens only (no raw colours) |

`components.css` needs `tokens.css` and must be loaded after it. Test
`exported_css_consts_match_files` checks that the constants equal the files.

## Tokens

All tokens use the `--cui-` prefix. Groups:

- Colour (`--cui-color-*`): bg, surface, surface-raised, surface-hover, text,
  text-muted, accent, accent-hover, accent-text, accent-subtle, on-accent,
  border, border-disabled, topbar, and status colours info, success, warning,
  error, pending and stopped, each with a matching `-bg` where used.
- Typography (`--cui-font-*`): family, size scale, weight, line height.
- Spacing (`--cui-space-*`), radius (`--cui-radius-*`), shadow
  (`--cui-shadow-*`), focus ring (`--cui-focus-*`), motion
  (`--cui-motion-duration-*`), minimum target size
  (`--cui-size-target-min`), z-index (`--cui-z-*`).
- Border widths (`--cui-border-width-s|m|indicator|l`) and component sizes
  (`--cui-size-control-check`, `--cui-size-toggle-*`, `--cui-size-modal-w`,
  `--cui-size-dropdown-w`, `--cui-size-arrow`).

`components.css` contains no raw `px` lengths (test
`components_css_has_no_raw_px`). The only exceptions are lines marked
`/* px-ok: reason */` (visually-hidden clip, forced-colors outline,
shrink-to-fit table column).

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

Contrast: test `token_pairs_meet_wcag_aa` (`tests/contrast.rs`) checks 40
declared foreground/background pairs in each theme: 4.5:1 for text and 3:1 for
UI and borders. Run it with
`cargo test -p console-ui --test contrast -- --nocapture` to print the table.
Combinations that are not declared are not checked.

## Components

Every component is a function component, exported from the crate root.

| Area | Components |
| --- | --- |
| Actions | `Button` (normal, primary, link; disabled; loading with `aria-busy`) |
| Layout | `Container`, `Header` (h1/h2, description, actions), `SpaceBetween`, `Grid` |
| Navigation | `Link`, `SideNavigation`, `Breadcrumbs`, `Tabs` (ARIA tabs keyboard pattern), `NavToggle` (icon button with `aria-expanded`/`aria-controls`) |
| Chrome | `TopNavigation` (banner with brand/start/search/utilities slots), `Dropdown` + `DropdownItem` (disclosure pattern, not `role=menu`; below 45rem the panel is anchored to the top bar so it stays in the viewport), `SearchInput` (`role="search"` form, `maxlength`, submit prevented) |
| Status | `Badge`, `StatusIndicator`, `Spinner`, `Alert`, `Flashbar` (with `flash_push` / `flash_dismiss`, max 5) |
| Forms | `FormField` (label, description, error, wired with `aria-describedby`), `Input`, `Select`, `Checkbox`, `Toggle` |
| Data | `Table` (columns, empty state, optional multi-select; needs `caption` and `item_key`) |
| Overlay | `Modal` (focus trap, Esc to dismiss, focus returned to opener, optional `inert_root`) |

Notes:

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
  `safe_href`, theme parsing) lives in `src/logic.rs` and has unit tests.

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
document's own `<html>` element. Passing the shell's theme to tools is not
built yet (protocol work, M4).

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
