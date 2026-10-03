# tasklens UI spec: design foundation and shared components

ID prefix `UI-FND`. This file defines the token values and the shared component library that
every other UI spec file (`shell.md`, `processes.md`, `process-windows.md`, `performance.md`,
`system-tabs.md`) refers to by name. Precedence: `docs/DECISIONS.md`, then
`docs/RECONCILIATION.md`, then `docs/ARCHITECTURE.md`, then the merged specs, then this file.

How to read a value:
- **measured**: sampled from a TaskSlinger screenshot or video frame with ImageMagick
  (`docs/taskslinger-reference/*.png` at 2720 × 1680, and the contact sheets in the scratchpad).
  Colours are the raw sample; sizes are the ratio to the 28 px row, because the screenshots were
  taken at a Windows scale above 100%.
- **spec**: taken from an ADR, an R entry or a merged spec entry (the source is named).
- **decided**: chosen here. Every decided value is also listed in "Decided here" at the end.

All sizes are logical px at `ui_scale_pct = 100`. Every size is multiplied by
`ui_scale = ui_scale_pct / 100` at render time through `UiMetrics` (ADR-009), never by hand in a
view. GPUI applies the compositor scale on top; nothing in tl-ui reads `window.scale_factor()`
except the chart geometry key and the text cache invalidation (ADR-009).

Every component in section 2 is a Rust type in `crates/tl-ui/src/components/`. Component names
are stable: other files use them as written here. Each component file has a `#[cfg(test)]`
module and a story entry in `crates/tl-ui/src/components/gallery.rs` (UI-FND-030) that shows every
state, so a reviewer can check the look without the engine.

## 1. Tokens

### UI-FND-001 Colour slots: Default Dark and Default Light

- **Implements**: APP-035 (22 slots, accent, graph palette), R-068 (two themes in P1), ADR-009
  theme model, PLAN section 9 palette, D-004 none, R-059 (heat colour).
- **Anatomy**: the `TlTheme` struct fields from ADR-009, one row per slot. `#RRGGBB` strings are
  what `assets/themes/builtin.json` holds; `Hsla` is what `TlTheme` holds after parsing.

Default Dark. "Plan" is PLAN section 9. "Measured" is the sample from `processes.png`,
`performance.png` or `information.png`. Where plan and measurement differ by a few units the plan
value is used, because the PNGs went through WebP compression; the difference is noted.

| Slot (`TlTheme` field) | Value | Source |
|---|---|---|
| `window_bg` | `#191B1D` | plan; measured `#191B1D` (list background and Information page) |
| `titlebar` | `#27292B` | plan; measured `#27292B` |
| `toolbar` | `#202224` | plan ("tab bar and toolbar"); measured `#202224` |
| `input` | `#202224` | measured, filter box at rest (`processes.png` 2516,95) |
| `input_focus` | `#25282B` | decided: `input` lightened 2 steps, so the focused box reads lighter with the accent border |
| `header` | `#2E3235` | plan; measured `#2B3134` (compression) |
| `list_bg` | `#191B1D` | plan (same as `window_bg`) |
| `row_alt` | `#1D1F20` | plan; measured `#1D1F20` |
| `row_hover` | `#232629` | decided: `row_alt` plus 6 units, no hue; TaskSlinger's hover was not captured in any frame |
| `row_selected` | `#075B84` | plan; measured `#12425B` on a downscaled frame, consistent |
| `divider` | `#2B2F33` | plan; measured column divider `#36383A` (lighter where it crosses the header band) |
| `menu_bg` | `#2A2E31` | measured View menu body `#2D2F31` on a downscaled frame, rounded to the divider hue |
| `menu_hover` | `#104B68` | decided: midway between `row_selected` and `menu_bg`; the hovered item keeps `text_primary` |
| `tab_bg` | `#202224` | plan (tab bar) |
| `tab_hover` | `#27292B` | decided: equals `titlebar`, one step above `tab_bg` |
| `scroll_track` | `#1A1C1E` | measured (`processes.png` 2706,900) |
| `scroll_thumb` | `#494F54` | measured (`processes.png` 2706,340) |
| `text_primary` | `#D6D8D9` | plan; measured status bar text `#D6D8D9` |
| `text_secondary` | `#6C7072` | plan |
| `text_selected_primary` | `#FFFFFF` | decided: white on `row_selected` gives contrast 5.9:1 |
| `text_selected_secondary` | `#BFD9E6` | decided: a light tint of the accent, 4.4:1 on `row_selected` |
| `value_heat` | `#3E2A28` | plan ("heat tint at maximum"); measured `#3D2725` |
| `accent` | `#0277A6` | plan; measured accent line under the tabs `#0277A6`, selected sidebar card outline `#0078A6` |

Default Light. TaskSlinger's light theme appears only as a menu swatch in the customize video
(white body, dark text, light-blue selected row), so every value below is **decided**. The rule
used: keep the accent, invert lightness, keep the same step sizes between neighbours as Default
Dark, and keep every text pair at or above 4.5:1 contrast (`text_secondary` on `list_bg` is 5.0:1).

| Slot | Value |
|---|---|
| `window_bg` | `#F3F4F6` |
| `titlebar` | `#E4E6E9` |
| `toolbar` | `#EBEDEF` |
| `input` | `#FFFFFF` |
| `input_focus` | `#FFFFFF` |
| `header` | `#DDE1E5` |
| `list_bg` | `#FFFFFF` |
| `row_alt` | `#F6F7F8` |
| `row_hover` | `#E8EBEE` |
| `row_selected` | `#CFE6F2` |
| `divider` | `#D4D8DC` |
| `menu_bg` | `#FFFFFF` |
| `menu_hover` | `#E1EEF5` |
| `tab_bg` | `#EBEDEF` |
| `tab_hover` | `#E0E3E6` |
| `scroll_track` | `#EBEDEF` |
| `scroll_thumb` | `#B5BBC0` |
| `text_primary` | `#1B1F22` |
| `text_secondary` | `#5F666B` |
| `text_selected_primary` | `#0F1416` |
| `text_selected_secondary` | `#3E5460` |
| `value_heat` | `#F6D5CF` |
| `accent` | `#0277A6` |

Status colours. Not slots; part of each built-in theme under a `status` key in `builtin.json`,
and fixed for user themes that omit the key (a user theme without `status` gets the values of the
built-in theme with the same `mode`). All decided; the dark danger is one step brighter than the
kernel line so the two never read as the same colour.

| Token | Default Dark | Default Light | Use |
|---|---|---|---|
| `status.success` | `#4CAF50` | `#2E7D32` | success toast icon, NEW row tint, "Enabled" badges |
| `status.warning` | `#E0A526` | `#B7791F` | warning toast icon, `Failed` glyph `!`, stale health text |
| `status.danger` | `#D9534F` | `#C62828` | error toast icon, EXITED row tint, destructive buttons, close button hover |
| `status.info` | `#2A9FD6` | `#0277A6` | info toast icon, links, "[PAUSED - Ctrl]" uses `accent`, not this |

Graph palette (`TlTheme.graph`, `GraphPalette` in ADR-009). Lines are drawn at full alpha, 1 px
(ADR-009 rule 6). Fills are drawn at full alpha with the colours below; the "fill" colours are
already darkened, so no alpha blending is needed per frame.

| Field | Default Dark | Source | Default Light |
|---|---|---|---|
| `cpu_fill` | `#0D546D` | plan; measured `#14232C` to `#2A3940` where the fill is thin | `#BFDCE8` |
| `cpu_line` | `#0A7096` | plan | `#0A7096` |
| `kernel_line` | `#C1514E` | plan; measured `#E6655E` on the net mini graph (same red family) | `#C1514E` |
| `mem_fill` | `#7E6A9A` | plan; measured `#DD89A5` at the top edge where the line sits | `#D8CCE6` |
| `mem_line` | `#A58CC4` | decided (plan gives only the fill) | `#7E6A9A` |
| `gpu_fill` | `#508A59` | plan; measured `#508A59` | `#CFE4D2` |
| `gpu_line` | `#71B768` | plan | `#3F8A4A` |
| `disk_fill` | `#8A7340` | decided: plan gives one disk colour `#D9B567`; the fill is that colour at 60% lightness so the line stays visible | `#F0E2BC` |
| `disk_line` | `#D9B567` | plan; measured `#D4B471` | `#B8923A` |
| `net_fill` | `#1E5C66` | decided from the measured mini graph fill `#144E58` | `#C4E3E7` |
| `net_line` | `#359CAB` | plan | `#2A8A98` |
| `grid` | `#2A3033` | decided: chart background measured `#1D2223`; grid one step lighter so hairlines show at 1 physical px | `#D9DEE2` |
| `chart_bg` | `#1D2223` | measured (`performance.png` 1000,300). Added to `GraphPalette` as a 13th field | `#FAFBFC` |

Derived tints (computed once per theme switch into `TlTheme.derived`, never per frame):

| Derived | Rule |
|---|---|
| `new_row_tint` | `status.success` at alpha 0.18 painted over the row background |
| `exited_row_tint` | `status.danger` at alpha 0.18 over the row background |
| `heat(value, reference)` | `row bg` blended toward `value_heat` by `clamp(value / reference, 0, 1) × heat_intensity_pct / 100` (PROC-040). Blending is in sRGB component space, which matches the measured TaskSlinger tints (`#3D2725` at 100%, `#766966` on a 60% row at 50% intensity reads as a lighter brown) |
| `focus_ring` | `accent` at alpha 1.0, 2 px outside the element (UI-FND-003) |
| `shadow` | `#000000` at alpha `0.35 × shadow_intensity_pct / 100` (dark) or `0.18 × …` (light) |
| `overlay` | `#000000` at alpha 0.4 (dark) or 0.25 (light), used only behind in-window dialogs |
| `stale_text` | `text_secondary` (ADR-003 Stale shows the last value in secondary colour) |

- **States**: not applicable (data).
- **Data binding**: `TlTheme` global; `theme.active`, `theme.follow_system`, `theme.light`,
  `theme.dark` from table (a); `appearance.heat_intensity_pct`, `appearance.shadow_intensity_pct`.
- **Build**: `crates/tl-ui/src/theme/{mod.rs, builtin.rs, derived.rs}`; `assets/themes/builtin.json`.
  Parse with `gpui::Rgba::try_from(&str)` then `.into()` to `Hsla`.
- **Performance**: all derived colours are computed in `TlTheme::rebuild_derived` on theme switch
  or intensity change. Views read fields; nothing calls a colour function per cell except
  `heat()`, which is two multiplies and three lerps.
- **Tests**: `theme::tests::builtin_json_parses` (every slot present in both themes, every value a
  6-digit hex); `theme::tests::contrast_floor` (each text/background pair in both themes is at or
  above 4.5:1, selected pairs above 4.4:1); `theme::tests::heat_endpoints` (`heat(0, r)` equals the
  row bg, `heat(r, r)` at 100% intensity equals `value_heat`).
- **Phase**: P1 (both themes), P2 (the other 10 themes, same file).
- **Decided here**: `input_focus`, `row_hover`, `menu_bg` rounding, `menu_hover`, `tab_hover`,
  `text_selected_*`, every Default Light value, status colours, `mem_line`, `disk_fill`, `net_fill`,
  `grid`, `chart_bg` as a 13th `GraphPalette` field, tint alphas, shadow alphas.

### UI-FND-002 gpui-kit `Theme` mapping (`theme/map.rs`)

- **Implements**: ADR-009 theme model ("Kit widgets read gpui-kit `Theme`"), APP-035 (U-8 token
  check), R-077 (two radii), R-020 (scale).
- **Anatomy**: `pub fn apply(tl: &TlTheme, m: &UiMetrics, kit: &mut gpui_kit::Theme)`. Every token
  name below was checked against `crates/component/src/theme/theme_color.rs` in the gpui-component
  checkout (`/home/oystein/work/tasklens-ref/gpui-component`, published as gpui-kit 0.7.0). All 30
  names in the ADR-009 list exist. The mapping below is the ADR-009 list plus the extra tokens the
  kit widgets used in this file actually read. A `ThemeColor` field not listed keeps the kit's own
  default for `tl.mode` (`ThemeColor::dark()` or `::light()`), re-applied before `apply` runs so a
  switch between modes resets them.

| Kit `ThemeColor` field | Value |
|---|---|
| `background` | `window_bg` |
| `foreground` | `text_primary` |
| `muted` | `row_alt` |
| `muted_foreground` | `text_secondary` |
| `border` | `divider` |
| `input` | `input` |
| `ring` | `accent` |
| `primary` | `accent` |
| `primary_hover` | `accent` lightened 8% (`Hsla.l + 0.08`) |
| `primary_active` | `accent` darkened 8% |
| `primary_foreground` | `#FFFFFF` |
| `secondary` | `toolbar` |
| `secondary_hover` | `tab_hover` |
| `secondary_active` | `header` |
| `secondary_foreground` | `text_primary` |
| `accent` | `menu_hover` (kit "accent" is the hover fill of menu and list items) |
| `accent_foreground` | `text_primary` |
| `popover` | `menu_bg` |
| `popover_foreground` | `text_primary` |
| `title_bar` | `titlebar` |
| `title_bar_border` | `divider` |
| `status_bar` | `toolbar` |
| `status_bar_border` | `divider` |
| `tab_bar` | `tab_bg` |
| `tab` | `tab_bg` |
| `tab_active` | `window_bg` |
| `tab_foreground` | `text_secondary` |
| `tab_active_foreground` | `text_primary` |
| `table` | `list_bg` |
| `table_even` | `row_alt` |
| `table_hover` | `row_hover` |
| `table_active` | `row_selected` |
| `table_active_border` | `accent` |
| `table_head` | `header` |
| `table_head_foreground` | `text_primary` |
| `table_row_border` | `divider` |
| `list` | `list_bg` |
| `list_even` | `row_alt` |
| `list_hover` | `row_hover` |
| `list_active` | `row_selected` |
| `list_active_border` | `accent` |
| `scrollbar` | `scroll_track` |
| `scrollbar_thumb` | `scroll_thumb` |
| `scrollbar_thumb_hover` | `scroll_thumb` lightened 10% |
| `selection` | `accent` at alpha 0.35 (text selection in inputs) |
| `caret` | `text_primary` |
| `skeleton` | `row_alt` lightened 6% (dark) or darkened 6% (light) |
| `success`, `success_foreground`, `success_hover`, `success_active` | `status.success`, `#FFFFFF`, ±8% lightness |
| `warning`, `warning_foreground`, `warning_hover`, `warning_active` | `status.warning`, `#1B1F22`, ±8% |
| `danger`, `danger_foreground`, `danger_hover`, `danger_active` | `status.danger`, `#FFFFFF`, ±8% |
| `info`, `info_foreground`, `info_hover`, `info_active` | `status.info`, `#FFFFFF`, ±8% |
| `link`, `link_hover`, `link_active` | `status.info`, lightened 10%, darkened 10% |
| `switch` | `scroll_thumb` (off track); the on track is `primary` inside the kit |
| `switch_thumb` | `#FFFFFF` |
| `slider_bar` | `accent` |
| `slider_thumb` | `#FFFFFF` |
| `progress_bar` | `accent` |
| `description_list_label` | `row_alt` |
| `description_list_label_foreground` | `text_secondary` |
| `group_box`, `group_box_foreground` | `row_alt`, `text_primary` |
| `chart_1`..`chart_5` | `graph.cpu_line`, `graph.mem_line`, `graph.gpu_line`, `graph.disk_line`, `graph.net_line` |
| `chart_grid` | `graph.grid` |
| `overlay` | `derived.overlay` |
| `window_border` | `divider` |
| `drag_border`, `drop_target` | `accent`, `accent` at alpha 0.15 |
| `button`, `button_hover`, `button_active`, `button_foreground` | `toolbar`, `tab_hover`, `header`, `text_primary` |
| `button_primary*`, `button_danger*`, `button_secondary*`, `button_success*`, `button_warning*`, `button_info*` | same values as `primary*`, `danger*`, `secondary*`, `success*`, `warning*`, `info*` |
| `sidebar`, `sidebar_border`, `sidebar_foreground`, `sidebar_accent`, `sidebar_accent_foreground`, `sidebar_primary`, `sidebar_primary_foreground` | `window_bg`, `divider`, `text_primary`, `row_hover`, `text_primary`, `accent`, `#FFFFFF` (the kit Sidebar is not used; mapped so a stray use is not unstyled) |
| `red`, `green`, `blue`, `yellow`, `magenta`, `cyan` and `*_light` | kit defaults (used only by kit syntax highlighting, which tasklens does not use) |
| `chart_bullish`, `chart_bearish`, `accordion`, `table_foot`, `table_foot_foreground`, `tab_bar_segmented` | kit defaults |

Non-colour `Theme` fields set by `apply`:

| Field | Value |
|---|---|
| `font_family` | `"Noto Sans"` |
| `font_size` | `m.base_text` (13 × ui_scale). Root calls `set_rem_size(font_size)`, so every kit rem unit is 13/16 of the kit documentation's px figures |
| `mono_font_family` | `"Noto Sans Mono"` if installed, else the kit's fallback chain. Used only by the Strings tab and the Environment value column |
| `mono_font_size` | `m.base_text` |
| `radius` | `m.radius_small` (4 × ui_scale, or 0 when `appearance.rounded_corners = false`) |
| `radius_lg` | `m.radius_card` (6 × ui_scale, or 0) |
| `shadow` | `appearance.shadow_intensity_pct > 0` |
| `focus_ring` | `true` |
| `scrollbar_mode` | `ScrollbarMode::Scrolling` (thumb fades out 2 s after the last scroll, kit `DEFAULT_IDLE`). The process table overrides to `Always` (UI-FND-003) |
| `mode` | `tl.mode` |
| `notification` | see UI-FND-021 |

- **Interaction**: none.
- **Data binding**: runs on `TlTheme` replacement and on every `UiMetrics` change, inside
  `Theme::update(cx, |t| apply(&tl, &m, t))`.
- **Build**: `crates/tl-ui/src/theme/map.rs`. One function, one match-free straight-line body.
  A comment at the top names the gpui-component commit the names were checked against. If a
  future kit version drops a name, the bead maps it to the nearest existing token and records that
  in the same comment (ADR-009).
- **Performance**: runs at most a few times per session. Not per frame.
- **Tests**: `map::tests::every_mapped_field_is_set` builds a `Theme` with sentinel colours, runs
  `apply`, and asserts no mapped field still holds the sentinel. `map::tests::light_dark_reset`
  switches dark to light and asserts `tab_active` equals the light `window_bg`.
- **Phase**: P1.
- **Decided here**: the extra token rows beyond the ADR-009 list, the ±8% hover and active rule,
  `selection` alpha 0.35, `scrollbar_mode` default.

### UI-FND-003 Metrics: spacing, text, radii, rows, icons, motion, focus, shadows, scrollbars

- **Implements**: ADR-009 "Metrics and scale" and "Fonts", R-020, R-077, R-017, APP-040, APP-058,
  brief shared vocabulary.
- **Anatomy**: `UiMetrics` (GPUI `Global`, `crates/tl-ui/src/theme/metrics.rs`). Every field
  below is a `Pixels` already multiplied by `ui_scale`, except the plain numbers marked "ratio".

```rust
pub struct UiMetrics {
    pub ui_scale: f32,            // ui_scale_pct / 100
    pub row_height: Pixels,       // 28 × row_height_pct/100 × ui_scale
    pub base_text: Pixels,        // 13 × ui_scale
    pub radius_small: Pixels,     // 4 × ui_scale, or 0
    pub radius_card: Pixels,      // 6 × ui_scale, or 0
    pub sp: [Pixels; 9],          // sp[1..=8] = 2 4 6 8 12 16 24 32 × ui_scale; sp[0] = 0
    pub text: TextSizes,          // xs sm base md lg xl = 11 12 13 14 16 20 × ui_scale
    pub icon: IconSizes,          // xs sm md lg = 12 14 16 20 × ui_scale
    pub style_version: u64,       // bumps on any change here or in TlTheme
}
```

Spacing (`sp1`..`sp8`, spec from the brief): 2, 4, 6, 8, 12, 16, 24, 32. Use: parts of one
control `sp2`; icon to label `sp3`; related controls `sp4`; cell horizontal padding `sp4`; one
group of controls `sp5`; panel padding and separate groups `sp6`; sections `sp7`; dialog outer
padding `sp6`; nothing uses `sp8` except the empty-state block (UI-FND-029).

Text (spec from the brief; family "Noto Sans"; line height 1.3 for every size, decided, which
gives 17 px for `t_base` and leaves 11 px of vertical room in a 28 px row):

| Token | Size | Weight | Used for |
|---|---|---|---|
| `t_xs` | 11 | 400 | status bar, chart axis labels, menu shortcut column, badge counts, "+N" child counts |
| `t_sm` | 12 | 400 | header column names, sidebar card secondary lines, tooltips, key/value labels, tab labels in secondary windows |
| `t_base` | 13 | 400 | table cells, menu items, inputs, buttons, body text, key/value values |
| `t_md` | 14 | 500 | dialog titles, Performance page header, sidebar card title, group rows ("Apps (11)") |
| `t_lg` | 16 | 600 | header total values are not this; `t_lg` is for the Information section titles and the About window name |
| `t_xl` | 20 | 600 | the big value on a Performance mini card is not this either (it is `t_md` 500); `t_xl` is reserved for the crash report heading and the rescue window |

Measured check from `processes.png`: cell text cap height is about 0.46 of the row, which at
28 px gives a 13 px font; header names and totals are the same size as cells (TaskSlinger draws
the two-line header in one size, so the header uses `t_sm` for the name and `t_base` for the total
value, decided). Numbers use `tnum` (UI-FND-051).

Radii (spec R-077): `r_sm` 4 for rows, menus, buttons, inputs, tooltips, badges, checkboxes,
tabs; `r_md` 6 for cards, dialogs, toasts, popovers, sidebar cards, mini graph cards.
`appearance.rounded_corners = false` sets both to 0 (APP-040). The kit `radius_full` is used only
for the switch track, the radio and the spinner.

Row height (spec R-020): `28 × row_height_pct/100 × ui_scale`. Also used for: menu item height is
not tied to it (fixed 26, below); the Performance sidebar card is `4 × row_height` (112 at 100%,
measured 116 from the ratio in `performance.png`, rounded to a row multiple, decided); the
in-window tab strip is `row_height + sp2` (32).

Icons (spec ADR-009 for the 16 px process icon; the rest decided): `i_xs` 12 (validity lock, sort
arrow, menu check mark, submenu chevron, tree expander chevron), `i_sm` 14 (tab suffix icons:
history, copy; status bar health icon; filter clear button), `i_md` 16 (toolbar buttons, menu item
icons, process and app icons, caption pin, toast type icon, shield), `i_lg` 20 (dialog leading icon,
empty-state icon, About logo is 48). Icons are `gpui_kit::Icon::new(IconName::X).size(px)` tinted
with `text_primary`, or `text_secondary` for secondary rows. Lucide names used by this file:
`check`, `chevron-right`, `chevron-down`, `chevron-up`, `x`, `search`, `pin`, `pin-off`, `lock`,
`shield`, `shield-check`, `info`, `circle-check`, `triangle-alert`, `circle-x`, `loader-circle`,
`copy`, `history`, `minus`, `plus`, `eye`, `eye-off`. The bead that writes `components/icons.rs`
checks each name against `gpui_kit_assets::IconName` and records any rename there. App SVGs under
`assets/icons/`: `shield-unlocked.svg` (the shield-hi variant APP-003 asks for) and `tasklens.svg`.

Motion (spec from the brief and R-017): `m_fast` 120 ms (hover fills, check marks, toggle thumb),
`m_base` 200 ms (theme crossfade, menu open, toast exit, filter box grow is 150 per APP-028 and
stays 150), `m_slow` 300 ms (NEW and EXITED fade-out, dialog open). Easing `ease_out_cubic` for
every entrance and property change; `ease_in_cubic` for the toast exit only. Reduced motion
(`App::reduce_motion()` or `motion.animations = false`) makes every one of these instant, except
chart sliding which `motion.smooth_graphs` alone controls (ADR-009). Nothing loops. Hover fills on
rows, buttons and menu items are not animated at all (TaskSlinger changes them instantly; and an
animation on a 2,000-row table would request frames). So `m_fast` applies only to checkbox, radio,
switch and the tab indicator.

Focus ring (decided; the kit draws a ring outside the border when `Theme::focus_ring` is true):
2 px solid `accent`, 2 px outside the element's border box, with the element's radius plus 2. It is
shown only for keyboard focus: GPUI has no `focus-visible`, so `TlFocusRing` (a helper in
`components/focus.rs`) tracks `window.last_input_was_keyboard`, set in a window-level
`on_key_down` and cleared in `on_mouse_down`, and views pass `show_ring = focused && keyboard`.
Rows in the tree-table show focus as a 1 px inset `accent` border instead of the outside ring,
because the list clips its children. Inputs show the `input_focus` background plus a 1 px `accent`
border, no ring.

Shadows and borders (decided, scaled by `appearance.shadow_intensity_pct`): menus and popovers
`BoxShadow { offset (0, 2), blur 8, spread 0, color derived.shadow }` plus a 1 px `divider` border;
toasts and in-window dialogs offset (0, 4), blur 16; secondary windows get the kit `window_border`
shadow (20 px, kit default) and nothing else; mini graph cards offset (0, 1), blur 4. At intensity
0 no shadow is drawn and the 1 px border remains. Every hairline is 1 physical px: draw with
`px(1.0 / window.scale_factor())` through `UiMetrics::hairline(window)`.

Scrollbars (measured: `processes.png` shows a 6 px wide thumb inset in a 14 px track at the table's
right edge; decided to follow the kit's constants, which match): kit `Scrollbar`, width 16 (4 inset
+ 8 active thumb + 4), idle thumb 6 px wide, active 8 px, minimum thumb length 48, radius 0 (kit
constant, matches TaskSlinger's square thumb). Modes: the process, services, startup, connections
and properties tables use `ScrollbarMode::Always`; every other scroll region uses `Scrolling`. The
track colour is `scroll_track` only in `Always` mode; in `Scrolling` mode the track is transparent.
Horizontal scrollbars are `Always` on tables and appear only when the columns overflow.

Cursor (spec: widgets rule 7): arrow everywhere, `text` only over an input's text, `col-resize`
only while over a header edge or a splitter, `pointer` only on real links (Executable path,
Documentation, PPID link).

- **Data binding**: `appearance.ui_scale_pct`, `appearance.row_height_pct`,
  `appearance.rounded_corners`, `appearance.shadow_intensity_pct`, `motion.*`,
  `App::reduce_motion()`.
- **Build**: `crates/tl-ui/src/theme/metrics.rs` (`UiMetrics`, `TextSizes`, `IconSizes`,
  `hairline`), `components/focus.rs`, `components/icons.rs`. `UiMetrics::recompute(settings,
  window)` runs on a settings change or `window.scale_factor()` change, bumps `style_version`,
  clears `CellTextCache` and calls `theme::map::apply` (ADR-009).
- **Performance**: views read `cx.global::<UiMetrics>()` once per render into locals. No `px()`
  arithmetic in element builders beyond `m.sp[n]`.
- **Accessibility**: focus ring contrast is the accent on every background (3.2:1 on `window_bg`
  dark, 4.6:1 light; both above the 3:1 non-text floor).
- **Tests**: `metrics::tests::scale_and_row` (scale 150 and row 120 give `row_height` 50.4,
  `base_text` 19.5, `sp[4]` 12); `metrics::tests::rounded_off_zeroes_radii`;
  `focus::tests::ring_only_after_keyboard` (mouse focus: no ring; Tab focus: ring).
- **Phase**: P1.
- **Decided here**: line height 1.3, icon size table, hover fills not animated, focus ring
  geometry and keyboard-only rule, shadow numbers, scrollbar modes per region, sidebar card height
  `4 × row_height`, in-window tab strip height.

## 2. Shared components

Conventions for every entry in this section:
- The component is a `RenderOnce` value type unless it says `Entity`. Entities are created in the
  owner's constructor, never in `render` (ADR-009).
- Sizes: `sm` is the table and toolbar density, `md` the dialog and settings density. There is no
  `lg`. Every height below is at 100% scale.
- "Kit" means the gpui-kit widget the component wraps; the component sets the kit variant, size
  and tokens once so no view repeats them.
- Hover and pressed states change at once (no animation), per UI-FND-003.
- Disabled: `opacity 0.5` on the whole element, no hover or pressed change, tooltip still shown,
  not a tab stop.

### UI-FND-010 TlButton (primary, secondary, ghost, icon, toolbar toggle)

- **Implements**: APP-002 toolbar buttons, APP-003 shield (icon variant), APP-004 and APP-005
  (toolbar toggle), APP-045 dialog buttons, widgets rules 8 and 10, PERF-014 page toggles
  ("Overall", "Kernel on").
- **Anatomy**:

```text
primary / secondary (md, label):            icon (sm):     toolbar toggle (sm, on):
┌──────────────────────┐  h 28              ┌──────┐ 24×24  ┌──────┐ 24×24
│  sp5 │ Label │ sp5   │  radius r_sm       │ icon │        │ icon │ bg accent@0.25
└──────────────────────┘  min w 72          └──────┘ r_sm   └──────┘ 1px accent border
```

| Variant | `TlButtonKind` | Height sm / md | Padding x | Fill | Border | Text |
|---|---|---|---|---|---|---|
| Primary | `Primary` | 24 / 28 | `sp5` | `accent` | none | `#FFFFFF`, `t_base` 500 |
| Secondary | `Secondary` | 24 / 28 | `sp5` | `toolbar` | 1 px `divider` | `text_primary`, `t_base` 400 |
| Danger | `Danger` | 24 / 28 | `sp5` | `status.danger` | none | `#FFFFFF`, 500 |
| Ghost | `Ghost` | 24 / 28 | `sp4` | transparent | none | `text_primary` |
| Icon | `Icon` | 24 / 28, square | 0 | transparent | none | icon `i_md` `text_primary` |
| Toolbar toggle | `Toggle { on }` | 24 square | 0 | off: transparent; on: `accent` at alpha 0.25 | on: 1 px `accent` | icon `i_md`; on: `accent` tint |
| Page toggle | `Segment { on }` | 24 | `sp5` | off: `toolbar`; on: `accent` at 0.25 | 1 px `divider`; on: `accent` | label `t_sm`; measured from "Overall" and "Kernel on" in `performance.png`: 24 px high, 1 px light border, no fill when off |

Icon with label: icon `i_md` left, gap `sp3`. Minimum label-button width 72 (dialog buttons line
up). A button that opens a dialog ends its label with "…" (rule 11).

- **States**: hover: Primary and Danger lighten 8%; Secondary, Ghost, Icon, Toggle fill
  `tab_hover`; Segment fills `tab_hover` when off. Pressed: Primary and Danger darken 8%; the rest
  fill `header`. Focused (keyboard): focus ring. Disabled: opacity 0.5. Loading: label replaced
  by `TlSpinner` `i_sm` and the button is inert, width kept (measure label first). Selected
  (`Toggle` on, `Segment` on): as in the table; a dropdown trigger stays in the pressed look while
  its menu is open (rule 9).
- **Interaction**: click and Space and Enter activate. Every button carries a `Box<dyn Action>`
  (`TlButton::action(..)`) and dispatches it on the window's focused handle; `on_click` closures
  are allowed only inside dialogs for OK and Cancel. Tooltip: required for `Icon` and `Toggle`
  (`TlTooltip::action(text, &action)` so the shortcut shows). Tab stop: yes for every variant.
- **Data binding**: none of its own; the owner passes `on` and `disabled`.
- **Build**: `components/button.rs`, wrapping kit `Button` with `.small()` or default size,
  `.ghost()`, `.primary()`, `.danger()`, `.compact()` for `Icon`; `Toggle` and `Segment` are custom
  `div`s (the kit `Toggle` has no accent-outline look), `.id(..)`, `.tab_stop(true)`,
  `.key_context("TlButton")`. Icon-only buttons set `.aria_label` through the kit's `tooltip`
  text.
- **Performance**: value type; nothing cached. Toolbar renders at most 8 of them.
- **Accessibility**: accessible name = label or tooltip text; focus ring; Space and Enter.
- **Tests**: `button::tests::dispatches_action_on_enter`; `button::tests::toggle_look` (render test
  asserts `on` draws the accent border); `button::tests::loading_keeps_width`.
- **Phase**: P1.
- **Decided here**: heights 24 / 28, min width 72, toggle fill alpha 0.25, Segment look.

### UI-FND-011 TlCheckbox

- **Implements**: APP-028 "Include children", APP-040 check items, column picker rows (PROC-024),
  settings fields (APP-023), SYS-004 state filters.
- **Anatomy**: box 16 × 16 (kit `Checkbox` md is `size_3` = 12 px at rem 16 and 9.75 at rem 13, too
  small; tasklens draws its own), radius `r_sm` min 3, 1 px border `text_secondary`, fill `input`.
  Checked: fill `accent`, border `accent`, Lucide `check` `i_xs` white, stroke 2. Label to the
  right, gap `sp3`, `t_base` `text_primary`. Row height: 24 in menus and settings, 20 inline (filter
  strip). Click target is the whole label row.
- **States**: hover: border `text_primary`, checked fill lightens 8%. Pressed: fill `header`
  (unchecked) or accent darkened 8%. Focused: focus ring around the box only. Disabled: opacity 0.5.
  Indeterminate (`Some(None)`): fill `accent`, Lucide `minus` white; used by the column picker group
  heading when some of its columns are visible. Check mark appears with a 120 ms scale 0.6 to 1.0
  (`m_fast`), instant under reduced motion.
- **Interaction**: click, Space toggles. `on_change(bool)`; the owner stores and notifies. In a menu
  the item is a `TlMenu` check item (UI-FND-023), not this component.
- **Build**: `components/checkbox.rs`, custom `div` with `.id`, `.tab_stop(true)`,
  `.key_context("TlCheckbox")`, `on_action(ToggleCheckbox)` bound to Space.
- **Tests**: `checkbox::tests::space_toggles`, `checkbox::tests::indeterminate_glyph`.
- **Phase**: P1.
- **Decided here**: 16 px box, custom rather than kit, indeterminate state.

### UI-FND-012 TlRadio and TlRadioGroup

- **Implements**: APP-023 settings choices (view mode, units, start tab), Set Priority dialog
  options where a dialog is used, the Theme editor "mode".
- **Anatomy**: circle 16 × 16, 1 px border `text_secondary`, fill `input`; checked: border
  `accent`, inner dot 8 px `accent`. Label right, gap `sp3`, `t_base`. Group layout vertical with
  `sp2` between rows (24 px rows) or horizontal with `sp5` gaps.
- **States**: as TlCheckbox; the dot grows 120 ms (`m_fast`). Focus ring on the focused circle;
  arrow keys move the selection and the focus inside the group (roving tab index: the group is one
  tab stop).
- **Interaction**: click selects; Up/Left and Down/Right move; Home/End first and last.
  `on_change(usize)`.
- **Build**: `components/radio.rs`, custom; kit `RadioGroup` is not used because it has no arrow
  keys.
- **Tests**: `radio::tests::arrow_keys_cycle`, `radio::tests::group_is_one_tab_stop`.
- **Phase**: P2 (first use is the Settings window).
- **Decided here**: all sizes, roving tab index.

### UI-FND-013 TlSwitch

- **Implements**: APP-023 boolean settings, D-003 history switch, APP-036 "Sync theme with system"
  in the Settings window (the menu uses a check item).
- **Anatomy**: kit `Switch` small: track 28 × 16, thumb 12, inset 2, radius full. Off: track
  `scroll_thumb`, thumb white. On: track `accent`, thumb white. Label left of the track in settings
  rows (the kit `label_side` left), gap `sp4`, `t_base`. Thumb slides 120 ms (`m_fast`).
- **States**: hover: track lightens 8%. Focused: focus ring around the track. Disabled: 0.5.
- **Interaction**: click, Space, Enter toggle (kit). `on_change(bool)`.
- **Build**: `components/switch.rs`, wraps kit `Switch::new(id).small()`.
- **Tests**: `switch::tests::space_toggles` (kit has one; tasklens wraps it in its own test with
  the mapped tokens).
- **Phase**: P2.
- **Decided here**: small size only.

### UI-FND-014 TlSlider

- **Implements**: APP-040 (UI Scale, Row Height, Value Color Coding Intensity, Drop Shadow
  Intensity), APP-023 numeric ranges with a slider, `motion.smooth_scroll_amount`.
- **Anatomy** (measured from `cus_sheet_01` Appearance submenu: a 4 px track about 120 px long with
  a 12 px round thumb and the value text "100%" right of it in secondary colour):

```text
Label               ────●──────────── 100%
t_base              track 4 h, 120 w    t_sm text_secondary, min w 40, tnum
```

  Track `scroll_thumb`, filled part `accent`, thumb 12 round white with 1 px `divider` border;
  in a menu row the whole row is 28 high with the label left (min 140), slider, then the value.
  In a settings page the slider is 200 wide.
- **States**: hover: thumb grows to 14. Dragging: thumb 14, value text `text_primary`. Focused:
  focus ring on the thumb; Left/Right step by `step`, Shift+arrow by 5 × step, Home/End min/max,
  PageUp/PageDown by 10 × step. Disabled 0.5.
- **Interaction**: drag previews live (`SliderEvent::Change` applies the preview); the setting is
  saved on `SliderEvent::Release` (APP-040). Keyboard steps save at once. Value text format is the
  owner's (`"{}%"`).
- **Data binding**: `SliderState` entity owned by the view; `set_value` when the setting changes
  from elsewhere (Ctrl+Plus), guarded against re-entry.
- **Build**: `components/slider.rs`, wraps kit `Slider::new(&state).horizontal()` inside a row
  `div`; the keyboard handling is tasklens's (`key_context("TlSlider")`) because the kit slider has
  none.
- **Performance**: `Change` events during a UI-scale drag rebuild metrics each move; the view
  debounces metric application to one per frame (coalesce in `on_next_frame`).
- **Tests**: `slider::tests::release_saves_once`, `slider::tests::arrow_steps_and_clamps`.
- **Phase**: P2.
- **Decided here**: all geometry, keyboard steps, per-frame coalescing.

### UI-FND-015 TlStepper (numeric input with - and +)

- **Implements**: APP-023 ranged integers (`processes.interval_ms` custom value,
  `history.retention_h`, `history.max_disk_mb`, `motion.tab_switch_ms`), Set Priority custom nice,
  Set Affinity count display.
- **Anatomy**: `[ - ][  1000  ][ + ]`, height 28, minus and plus are `TlButton::Icon` 28 square
  with 1 px `divider` border joined to a `TlTextInput` of width 72 (numbers right-aligned, `tnum`);
  outer radius `r_sm` on the group, inner corners square. Unit suffix (`ms`, `h`, `MB`) inside the
  input in `text_secondary`.
- **States**: input states from UI-FND-016. At min the minus button is disabled; at max the plus.
  Invalid typed text (non-digit, out of range): input border `status.danger`, tooltip "Enter a
  number between {min} and {max}", value not applied until valid; blur with invalid text reverts
  to the last valid value.
- **Interaction**: click buttons step by `step`; Up/Down in the input step; Shift+Up/Down 10 ×
  step; Enter applies; mouse wheel over the input steps only while it is focused. `on_change(i64)`
  fires on apply, not per keystroke.
- **Build**: `components/stepper.rs`, composed of TlButton and TlTextInput; kit
  `NumberInput` is not used (its look does not match and its wheel behaviour is unconditional).
- **Tests**: `stepper::tests::clamps_and_disables_ends`, `stepper::tests::invalid_reverts_on_blur`.
- **Phase**: P2.
- **Decided here**: everything.

### UI-FND-016 TlTextInput and TlFilterBox

- **Implements**: APP-028 (filter box), PROC-077 per-tab filters, APP-014 Run dialog input, theme
  editor hex input (APP-037), settings text fields, column picker filter (PROC-024).
- **Anatomy**:

```text
TlTextInput sm (24 h) / md (28 h):          TlFilterBox (in the tab strip):
┌──────────────────────────┐ r_sm            Filter: ┌────────────────────┬──┐ 24 h
│ sp4 │ text        │ sp4  │ bg input        t_sm    │ sp4 text         │ x│ 160 w, 280 focused
└──────────────────────────┘ 1px divider     text_secondary                 └──┘ i_sm clear button
```

  Text `t_base` `text_primary`, placeholder `text_secondary`, caret `text_primary` 1 px, selection
  `accent` at 0.35. Optional prefix icon (`search` `i_sm`, `text_secondary`) with `sp3` gap.
  Optional suffix: clear button (Lucide `x` `i_sm`, 20 × 20 hit area) shown only when the text is
  non-empty. Measured: the TaskSlinger filter box sits flush with the tab strip's bottom line, its
  label "Filter:" is in secondary colour and the box has a 1 px accent border when focused.
- **States**: default as above. Hover: border `text_secondary`. Focused: bg `input_focus`, 1 px
  `accent` border, no ring. Disabled: 0.5, text `text_secondary`. Invalid (`.invalid(true)`):
  border `status.danger`, and the owner's tooltip gives the reason. Loading (`.loading(true)`,
  used by the Run dialog while a command starts): suffix `TlSpinner` `i_sm` replaces the clear
  button.
- **Interaction**: standard editing from the kit `InputState` (select all, undo, word moves,
  clipboard). Esc: if `clean_on_escape` is set (filter boxes), one Esc clears the text and emits
  `Change`, then the owner moves focus (APP-028 sends it back to the table); a second Esc follows
  the global rule (table b). Enter emits `PressEnter`. Right-click opens the kit edit menu (Cut,
  Copy, Paste, Select all) through `Input::context_menu`.
  TlFilterBox extras: width 160, grows to 280 over 150 ms `ease_out_cubic` on focus and shrinks on
  blur unless the text is non-empty (then it stays 280); instant under reduced motion. Ctrl+F
  focuses it from anywhere in the window (Global action `FocusFilter`). A printable key while a
  table has focus (and no input or dialog has focus) focuses the box and inserts the key (ADR-009).
  Debounce: 100 ms after the last `Change`, the owner sends the query (ADR-009); the box itself
  does not debounce.
- **Data binding**: `Entity<InputState>` owned by the view, created once. Read text with
  `state.read(cx).text()` (a `Rope` borrow), never `value()` (allocates).
- **Build**: `components/input.rs`: `TlTextInput` wraps kit `Input::new(&state)` with
  `.small()` or default, `.cleanable(true)` where asked, `.prefix(..)`, `.appearance(true)`;
  `TlFilterBox` wraps `TlTextInput` with the "Filter:" label and an `Animation` on width keyed by
  focus. Both apply `.key_context("TlInput")`.
- **Performance**: each keystroke re-shapes the visible line in the kit (prepaint) and emits
  `Change`; the parent re-renders only when its debounce fires.
- **Accessibility**: focus visible through the accent border; label "Filter:" is a real text node
  next to the box; clear button has tooltip "Clear filter".
- **Tests**: `input::tests::escape_clears_then_emits_change`,
  `input::tests::filter_box_width_follows_focus` (render test, reduced motion on so widths are
  final), `input::tests::printable_key_routes_to_filter` (in the table test fixture).
- **Phase**: P1.
- **Decided here**: heights, clear button hit area, "stay wide when non-empty" rule, invalid and
  loading states.

### UI-FND-017 TlSelect (dropdown) and TlCombo

- **Implements**: APP-023 enumerated settings (`general.language`, `general.start_tab`,
  `units.network`, `performance.cpu_graph_mode`), PERF time range chooser where not a menu, Set
  Priority dialog policy, service Log On and Recovery fields (SYS-012, SYS-013).
- **Anatomy**: trigger looks like `TlTextInput` md (28 h, min w 160) with the current label left
  and Lucide `chevron-down` `i_xs` right in `text_secondary`. Popup: a `TlMenu` surface
  (UI-FND-023 look: `menu_bg`, 1 px `divider`, `r_sm`, shadow) at least as wide as the trigger,
  max height 10 items (260) then scrolls, items 26 high with `check` `i_xs` on the selected one.
  `TlCombo` adds a `TlTextInput` at the top of the popup for filtering when the list has more than
  12 items (the language list has 12, so it gets one).
- **States**: trigger hover: border `text_secondary`; open: border `accent`, chevron rotated 180°
  (instant); focused: `accent` border; disabled 0.5. Item states as TlMenu. Empty filter result:
  "No matches" in `text_secondary`, `t_base`, centered, 40 px row.
- **Interaction**: click, Space, Enter, Alt+Down open; Up/Down move the highlight (wrapping),
  typing a letter jumps to the next item starting with it, Enter confirms, Esc closes without
  change. Focus returns to the trigger on close. `on_change(value)` fires on confirm only.
- **Data binding**: `Entity<SelectState<Vec<TlSelectItem>>>` owned by the view;
  `set_selected_value` when the setting changes elsewhere.
- **Build**: `components/select.rs`, wraps kit `Select` and `Combobox` with `.small()`, `.menu_width`
  equal to the trigger width, `.menu_max_h(px(260))`, `.appearance(true)`.
- **Tests**: `select::tests::keyboard_open_move_confirm`, `select::tests::escape_restores_focus`.
- **Phase**: P2.
- **Decided here**: sizes, 12-item threshold for the filter field, type-ahead.

<!-- APPEND -->
