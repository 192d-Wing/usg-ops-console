# Accessibility conformance notes

Status as of M4.5 (redesign), 2026-11-04. Target: WCAG 2.1 AA (Section
508). This file holds conformance remarks for a future VPAT. It is not a
full VPAT. M4 evidence is from local runs and CI run 41892456284 (Firefox
included). M4.5 evidence is from local runs and CI run [42014779167](https://github.com/192d-Wing/usg-ops-console/actions/runs/42014779167) (commit `b229338`, all jobs green, Firefox wasm and E2E included).

Related evidence: axe scans (`e2e/tests/a11y.spec.ts`, Chromium only,
POAM-012), keyboard and reflow specs, the token contrast test
(`crates/console-ui/tests/contrast.rs`). See
[NIST_800-53_MAPPING.md](NIST_800-53_MAPPING.md#related-evidence-not-a-nist-control)
and [POAM.md](POAM.md).

## Tool container (M4)

What the shell does (`crates/console-shell/src/tool_host.rs`,
`crates/console-sdk/src/transport.rs`):

- The tool iframe has a `title` (the tool title) and a visible focus ring
  from the `--cui-focus-ring` token (`shell.css` `.tool-frame:focus`,
  `:focus-visible`, `[data-frame-focused]`).
- No focus is moved into the tool on launch. "Skip to tool content" (tool
  routes only), F6/Ctrl+F6 and Shift+F6 move focus into and out of the
  tool; the SDK applies `Focus{first|last}` only when the tool has no
  focused element.
- Launch errors use `role=alert`; the "not connected" warning sits in a
  `role=status` region; `aria-busy` clears on timeout. Not-found and
  not-permitted tools show an H1 and title "Tool not found" with a link to
  Console home. Reload moves focus to the loading region, then the frame
  wrapper or error panel, never into the tool. Tool title changes are
  announced politely while focus is in the frame.
- (M4.5) The service menu is removed. In the command palette, "Loading"
  is an `aria-disabled` row that cannot be selected, and a failed
  `/api/tools` gives an enabled Retry row.

Tests: E2E `tool-a11y.spec.ts` (Tab, Shift+Tab, click, F6/Shift+F6, skip
link, not-found heading, reload focus), axe on the loaded, loading, 403
and 404 container states (`tools.spec.ts`, Chromium); shell
`frame_ring_only_on_the_iframe_stop`, `reload_requests_refocus_until_settled`,
`title_announced_only_when_focus_in_frame_and_changed`,
`not_found_meta_has_error_heading`; SDK wasm
`focus_edge_picks_visible_target_only_from_body`; console-ui wasm
`dropdown_keyboard_and_escape`.

## Shell redesign (M4.5)

Conformance remarks for the rail, command palette and scope popovers. The
ux-a11y review approved these after round 2; open items are listed at the
end of this section.

### Collapsed rail tooltip (SC 1.4.13 Content on Hover or Focus)

- Collapsed rail links keep their text label as the accessible name
  (visually hidden). The tooltip is a separate `aria-hidden` element
  (`.cui-rail__tip`, `crates/console-ui/src/components/rail.rs`), so it is
  not part of the name (SC 4.1.2).
- **Dismissible:** Escape hides every tip without moving focus or the
  pointer (`data-tip-dismissed` on the rail `<nav>`; the show rule in
  `components.css` excludes it). It re-arms on link mouseleave, rail
  focusout or a mode change.
- **Hoverable:** the tip is inside the link, and a transparent bridge
  covers the gap, so the pointer can move onto it and it stays shown.
- **Persistent:** it stays until hover or focus ends or it is dismissed.
- Evidence: wasm `rail_collapsed_keeps_text_names`
  (`crates/console-ui/tests/wasm.rs`, checks computed visibility at rest,
  on hover and focus, and after Escape; passed locally, fixes round 3);
  E2E `rail.spec.ts` "accessible names are exactly the label while tips
  render; tips are aria-hidden" and "hoverable: moving the pointer from the
  link onto the tip keeps it shown" (passed on 5 engines locally, QA round
  2). The E2E Escape cases ("Escape dismisses the hover tip…", "Escape
  dismisses the focus tip…", `rail.spec.ts:176` and `:196`) failed in QA
  round 2 (R2-D1) and pass after fixes round 3 on macOS Chromium and WebKit
  and Linux Chromium, Firefox and WebKit (QA "Round 3 rerun"; also green in CI run
  42014779167). The wasm test also asserts the tip's computed visibility.

### Command palette (combobox pattern)

- `CommandPalette` (`crates/console-ui/src/components/palette.rs`) is a
  modal dialog with the APG combobox-with-listbox pattern: `role=combobox`
  input with `aria-controls`, `aria-expanded`, `aria-autocomplete`,
  `aria-activedescendant`; options with `aria-selected`; arrows wrap,
  Home/End; Tab and Shift+Tab are trapped; Escape closes and returns focus
  to the opener (or the palette trigger, then main content). The chrome
  behind it is `inert`. The result count is announced in a polite status
  region, debounced 300 ms (SC 4.1.3). Disabled rows are `aria-disabled`
  and skipped. Options are at least 24x24 px (SC 2.5.8).
- Ctrl+K / Cmd+K opens it from the shell; the visible "Search…" trigger is
  the pointer and discoverable path (SC 2.1.1). The trigger has
  `aria-haspopup` and `aria-keyshortcuts`.
- Evidence: E2E `palette.spec.ts` (ARIA wiring, keys, Tab trap, focus
  return, count, targets, reduced motion), wasm
  `palette_combobox_navigation_and_focus_return`,
  `palette_empty_state_is_text_only`; axe on palette results, empty state
  and 320 px in light and dark (`a11y.spec.ts`, Chromium). Passed locally
  in QA round 2.

### Scope popovers

- Each breadcrumb segment is a button with `aria-controls` and
  `aria-expanded` that opens a non-modal `role=dialog` popover with a
  native `<select>`. Opening moves focus to the select (or to its hint
  when the select is disabled). Escape closes and returns focus to the
  segment; an outside pointerdown or Tab out closes it without pulling
  focus back. The cascade live region (`scope-announcer`) sits outside the
  popovers. Below 45rem one "Scope" button opens a single popover with all
  three selects, inside the 320 px viewport (SC 1.4.10).
- Segments show a 3:1 border and a chevron at rest (SC 1.4.11).
- Evidence: wasm `popover_open_escape_and_outside_click`,
  `popover_closes_on_focus_out_without_moving_focus`,
  `popover_embedded_keeps_children`; E2E `scope.spec.ts` (A11Y-002 Tab-out
  and A11Y-004 border cases), `reflow.spec.ts` compact scope at 320 px; axe
  on site, tenant and compact popovers (Chromium). Passed locally in QA
  round 2. Residual: POAM-049 (disabled segment style unreachable;
  `border-disabled` equals `border`).

### Contrast, forced colors, motion

- Contrast: `token_pairs_meet_wcag_aa` (`crates/console-ui/tests/contrast.rs`)
  checks every declared token pair in light and dark (4.5:1 text, 3:1 UI
  and borders), including the M4.5 sunken, accent-subtle and hover pairs;
  `every_dark_color_has_a_light_counterpart` checks parity. Pairs that are
  not declared are not checked. Passed locally (`make check`).
- Forced colors: `components.css` uses Highlight for focus outlines, the
  active rail item and the active palette option, and CanvasText borders on
  the palette, popovers and tooltip. Only a Chromium emulation spot check
  exists (`rail.spec.ts` "forced-colors spot check").
- Reduced motion: palette and rail transitions are 0 (E2E `palette.spec.ts`
  "palette and rail have no transition under prefers-reduced-motion").

### Open manual checks (POAM-046)

- Windows High Contrast on a real Windows host (rail active item,
  tooltip, palette, popovers).
- VoiceOver pass over the rail, palette and scope popovers. No screen
  reader has been used on M4.5; names were checked by accname computation.

## Known platform behaviour: Firefox reverse-Tab stop on the tool iframe

VPAT remark for **SC 2.4.3 Focus Order** (also relevant to 2.1.1, 2.1.2,
2.4.7, 4.1.2):

> In Firefox, pressing Shift+Tab from the first control inside a tool
> stops once on the tool's frame element in the console before reaching
> the preceding console control. The stop is named (frame title) and has a
> visible focus indicator; the next Shift+Tab continues in the expected
> order. There is no keyboard trap. Chromium and WebKit do not add this
> stop.

- Cause: Firefox inserts the `<iframe>` element as an extra sequential
  focus stop in the embedding document on reverse traversal. `tabindex=-1`
  on the iframe does not remove it, and the shell cannot tell this stop
  from focus inside the frame (probe evidence:
  `fixes-round-1-shell.md`, round 2, probe item 5, in run directory
  `.agent-runs/20261104-064220-m4-tools/`).
- Ruling (ux-a11y review, M4): acceptable documented platform behaviour;
  2.1.1, 2.1.2 and 2.4.3 pass; 2.4.7 and 4.1.2 are met at the stop by the
  iframe title and the state-driven focus ring (`data-frame-focused`,
  because Firefox does not match `:focus` on the iframe at that stop).
  Not a POA&M item. The title and ring must not regress.
- Evidence: `e2e/tests/tool-a11y.spec.ts:80` (Firefox only) asserts at the
  stop that the shell `activeElement` is the tool iframe, the tool
  document `hasFocus()` is false, the iframe title is non-empty, the
  computed `box-shadow` is not `none`, and `data-frame-focused` is present;
  then one more Shift+Tab leaves the frame (at most 2 presses). Passed on
  Linux Firefox in the QA final round (local Playwright container).
- Browser version observed: Firefox 157.0 as bundled with Playwright
  1.64.1 (`playwright-core/browsers.json`, revision 1555). This is
  Playwright's Firefox build, not a Firefox ESR release; behaviour on the
  deployed ESR has not been checked.
- **Recheck** on every Firefox ESR bump in the supported-browser baseline
  and on every Playwright upgrade; update this remark if the extra stop
  disappears or the ring stops showing.

## Known gaps

- axe runs on Chromium only (POAM-012).
- M4.5: manual Windows High Contrast and VoiceOver checks not done
  (POAM-046);
  sub-page styling in the collapsed rail (A11Y-003) checked visually only.
- WebKit skips the native Tab/Shift+Tab tool tests (WebKit without Full
  Keyboard Access does not tab to links and buttons); F6 and the skip link
  cover WebKit.
- Not live-tested: 320 px / 200% zoom with many tools and long titles,
  "not connected" warning contrast in both themes, reduced motion on the
  spinner, forced-colors rendering of the frame ring.
