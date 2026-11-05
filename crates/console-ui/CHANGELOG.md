# Changelog: console-ui

## M4.5 (redesign)

### Breaking or behaviour changes

- `BadgeColor`: five semantic variants added (`Neutral`, `Info`, `Success`,
  `Warning`, `Error`). The legacy colour names (`Grey`, `Blue`, `Green`,
  `Red`, `Yellow`) stay as aliases with the same look. The default is now
  `Neutral` (was `Grey`). The enum is not `#[non_exhaustive]` yet, so
  exhaustive `match`es outside this crate need the new arms. It becomes
  `#[non_exhaustive]` at the next breaking release.
- `.cui-navtoggle__icon` no longer styles anything; the toggle renders an
  `Icon` (`.cui-navtoggle .cui-icon`). The selector is kept as a no-op for one
  release and is planned for removal in M6.
- `TopNavigation`: new `start` and `scope` slots (additive). Slot order is
  now `[start][brand][search][scope][utilities]`. Hosts that relied on the
  old visual order should check their layout.
- `RailItem` has a new public field, `sub: bool`, with a `sub_page()` builder.
  Code that builds `RailItem` with a struct literal has to set it; use
  `RailItem::new` instead.
- `SearchInput` is `#[deprecated]` (use `CommandPalette`); removal planned M6.

### Added

- Modules `icon` (`Icon`, `IconName`), `palette` (`CommandPalette`,
  `PaletteItem`, `PaletteGroup`), `popover` (`Popover`, `PopoverAlign`,
  `embedded` prop), `rail` (`Rail`, `RailEntry`, `RailItem`).
- `RailMode` and `RAIL_STORAGE_KEY`.
- `logic::fold_palette` / `logic::rank_folded` (rank without re-folding on
  each keystroke).
- New design tokens (additive only; none removed or renamed).

### Accessibility

- The collapsed-rail tooltip is a real `aria-hidden` element. Escape hides it
  and it can be hovered (WCAG 1.4.13).
- `Popover` closes when focus leaves the panel for anything but its anchor.
- Scope breadcrumb segments show a border and chevron at rest; the disabled
  state is dashed and sunken.
