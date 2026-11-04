# Accessibility conformance notes

Status as of M4, 2026-11-04. Target: WCAG 2.1 AA (Section 508). This file
holds conformance remarks for a future VPAT. It is not a full VPAT. M4
evidence is from local runs and CI run 41892456284 (Firefox included).

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
- Service-menu "Loading tools..." and "No tools available" are
  non-interactive notes.

Tests: E2E `tool-a11y.spec.ts` (Tab, Shift+Tab, click, F6/Shift+F6, skip
link, not-found heading, reload focus), axe on the loaded, loading, 403
and 404 container states (`tools.spec.ts`, Chromium); shell
`frame_ring_only_on_the_iframe_stop`, `reload_requests_refocus_until_settled`,
`title_announced_only_when_focus_in_frame_and_changed`,
`not_found_meta_has_error_heading`; SDK wasm
`focus_edge_picks_visible_target_only_from_body`; console-ui wasm
`dropdown_keyboard_and_escape`.

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
- WebKit skips the native Tab/Shift+Tab tool tests (WebKit without Full
  Keyboard Access does not tab to links and buttons); F6 and the skip link
  cover WebKit.
- Not live-tested: 320 px / 200% zoom with many tools and long titles,
  "not connected" warning contrast in both themes, reduced motion on the
  spinner, forced-colors rendering of the frame ring.
