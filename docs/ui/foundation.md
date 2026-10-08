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
| `text_secondary` | `#959A9C` | decided: TaskSlinger's `#6C7072` is 3.45:1 on `list_bg`; `#959A9C` is the lightest grey on the same hue that reaches 4.5:1 on every Dark background, the lowest being 4.54:1 on `header` (R-269) |
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
| `text_secondary` | `#5E656A` (R-269: `#5F666B` was 4.44:1 on `header`) |
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
| `t_xs` | 11 | 400 | chart axis labels, menu shortcut column, badge counts, "+N" child counts |
| `t_sm` | 12 | 400 | status chip text (measured cap height 8.8 px), header column names, sidebar card secondary lines, tooltips, key/value labels, tab labels in secondary windows |
| `t_base` | 13 | 400 | table cells, menu items, inputs, buttons, body text, key/value values |
| `t_md` | 14 | 500 | dialog titles, Performance page header, sidebar card title, group rows ("Apps (11)") |
| `t_lg` | 16 | 600 | header total values are not this; `t_lg` is for the About window name, the App Properties header name and the Performance page header; Information section titles are `t_md` 500 (UI-FND-025) |
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
in-window tab strip is `row_height + sp2` (32). The main window's tab strip is the shell's
`TAB_STRIP_H = 30` (measured 29.5 from `processes.png`) and its title bar `TITLEBAR_H = 34`, both
exported from `window/main.rs`; this file's toast margin uses them.

Icons (spec ADR-009 for the 16 px process icon; the rest decided): `i_xs` 12 (validity lock, sort
arrow, menu check mark, submenu chevron, tree expander chevron), `i_sm` 14 (tab suffix icons:
history, copy; status bar health icon; filter clear button), `i_md` 16 (toolbar buttons, menu item
icons, process and app icons, caption pin, toast type icon, shield), `i_lg` 20 (dialog leading icon,
empty-state icon, About logo is 48). Icons are `gpui_kit::Icon::new(IconName::X).size(px)` tinted
with `text_primary`, or `text_secondary` for secondary rows. Lucide names used by this file:
`check`, `chevron-right`, `chevron-down`, `chevron-up`, `x`, `search`, `pin`, `pin-off`, `lock`,
`shield`, `shield-check`, `info`, `circle-check`, `triangle-alert`, `circle-x`, `loader-circle`,
`copy`, `arrow-up`, `arrow-down` (sort indicators, UI-PRC-005; R-185), `rotate-ccw-clock` (the Processes history toggle; `history` is not in the kit's 1830
icons), `minus`, `plus`, `eye`, `eye-off`. The bead that writes `components/icons.rs`
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
  (`TlTooltip::action(text, &action)` so the shortcut shows). Tab stop: yes by default for every
  variant; the owner may pass `.tab_stop(false)`. The main window toolbar does so (shell), because
  every toolbar command also has a menu item and a shortcut (table b), so Tab goes straight from
  the filter box to the table.
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
- **Anatomy**: `[ - ][  1000  ][ + ]`, height 28, minus and plus are `TlButton::Icon` 28 square (tooltips "Decrease" and "Increase", R-201)
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
  `TlCombo` adds a `TlTextInput` at the top of the popup for filtering when the list has 12 or
  more items (the language list has 12, so it gets one; R-208).
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

### UI-FND-018 TlTabs (in-window tab strip for Properties and Settings)

- **Implements**: PROC-077 Properties tabs ("General", "Threads [n]", …), SYS-011 to SYS-014
  Service Properties tabs, APP-023 (the Settings window uses a page sidebar, not tabs; TlTabs is
  used inside its "Performance" page for per-device groups), APP-042 Additional Setup sections.
  The main window tab strip is the shell's (`shell.md`), styled after APP-027; TlTabs is the
  plainer in-window style.
- **Anatomy** (measured from `adv_sheet_01` Properties window: a 32 px strip, labels centered in
  equal-width cells, the active cell slightly lighter with a 2 px accent underline):

```text
┌───────────┬───────────┬──────────────┬──────────┐ h 32, bg tab_bg
│  General  │Threads [17]│ Handles [1181]│  Token   │ t_sm, padding x sp5, min w 80
└═══════════┴───────────┴──────────────┴──────────┘ 1 px divider below; active: 2 px accent
```

  Active: text `text_primary` weight 500, bg `window_bg`, 2 px `accent` line at the bottom
  spanning the cell. Inactive: `text_secondary`, bg `tab_bg`. Counts "[n]" use `tnum` and update
  live. Overflow: when the cells do not fit at min width 80, the strip scrolls horizontally and a
  "»" `TlButton::Icon` (`chevron-right`) at the right opens a `TlMenu` listing every tab with a
  check on the active one (kit `TabBar::menu(true)`).
- **States**: hover (inactive): bg `tab_hover`, text `text_primary`. Pressed: bg `header`.
  Focused: the strip is one tab stop; the focused cell shows the focus ring inset 2 px (clipped
  strip). Disabled tab (`Strings` before its data loads): 0.5, not selectable, tooltip from the
  owner. Switching animates the underline sliding over `m_fast` (120 ms) and the content with no
  crossfade (content is a `match` on the index; the main window's 250 ms crossfade is APP-027's
  and belongs to the shell). Instant under reduced motion.
- **Interaction**: click selects. Keyboard, inside the strip: Left/Right move and select
  (wrapping), Home/End, Ctrl+Tab and Ctrl+Shift+Tab also work while focus is anywhere inside the
  window the strip belongs to (bound on the window root with `key_context("TlTabsWindow")`).
  `on_change(usize)`.
- **Data binding**: the owner holds `selected: usize` and the labels; counts come from the owner's
  snapshot slice.
- **Build**: `components/tabs.rs`, wraps kit `TabBar::new(stable_id).underline().small()` with
  `Tab` children; the keyboard handling is tasklens's because the kit has none. The id must be
  stable per window (animation state is keyed on it).
- **Performance**: value type; labels are `SharedString`s built once; the count suffix is
  re-formatted only when the count changes (owner caches `(count, SharedString)`).
- **Tests**: `tabs::tests::arrow_keys_wrap`, `tabs::tests::ctrl_tab_from_content`,
  `tabs::tests::overflow_menu_lists_all`.
- **Phase**: P2.
- **Decided here**: 32 px strip, underline style, min cell width 80, overflow rule.

### UI-FND-019 TlTooltip

- **Implements**: PROC-039 row hover tooltip (600 ms, up to 8 lines), ADR-003 validity tooltips,
  APP-003 shield tooltips, D-004 Memory header tooltip, PROC-061 "Ends the process immediately
  (SIGKILL)", every icon-only button (rule 8), PERF-013 reason tooltips.
- **Anatomy**:

```text
┌─────────────────────────────────┐ bg menu_bg, 1 px divider, r_sm, shadow (menu level)
│ sp4 │ text t_sm text_primary    │ padding y sp2, x sp4
│     │ second line …             │ max width 360 (text) or 560 (lines), wraps at word boundaries
│     │ Ctrl+E              t_xs  │ optional shortcut line, text_secondary, right-aligned
└─────────────────────────────────┘
```

  Two kinds: `TlTooltip::text(impl Into<SharedString>)` for one short line, and
  `TlTooltip::lines(Vec<SharedString>)` for the row tooltip, where each line is its own text run
  so long lines wrap independently. `.action(&dyn Action)` appends the shortcut line (kit
  `Tooltip::action`). Max width 360 for `text` and 560 for `lines` (the row tooltip's command
  line and path lines need the room, UI-PRC-012), max height 12 lines (about 190 px); more lines are cut and
  the last visible line ends with "…". The command-line line is already cut to 512 characters by
  PROC-039 before it reaches the tooltip; the tooltip itself applies no middle ellipsis.
- **States**: shown; hidden. No hover state of its own (the pointer never enters it; it closes on
  pointer move off the trigger, mouse down, key down, scroll, or window deactivate).
- **Interaction**: delay 500 ms for buttons and glyphs (kit default), 600 ms for table Name cells
  (PROC-039); both as `tooltip_show_delay`. Position: below the trigger, left-aligned, offset
  `sp2`; flips above when there is no room below; clamps to the window. Keyboard: a focused
  button shows its tooltip after the same delay while focus rests on it, and Esc hides it (focus
  stays). Moving between two triggers within 300 ms shows the second at once (kit grace period
  applies to kit widgets; for `.id().tooltip(..)` on plain elements there is no grace period, and
  tasklens does not add one).
- **Data binding**: text is built by the owner when the tooltip opens (the closure runs then),
  so a row tooltip reads the current `ProcessMeta` and formats once per open.
- **Build**: `components/tooltip.rs`, wraps kit `Tooltip::new(..)` / `Tooltip::element(..)` and
  returns the `AnyView` for GPUI's `.tooltip(|w, cx| ..)`. Kit widgets with a `.tooltip(text)`
  builder get `TlTooltip::text` content through the same style because `popover` and
  `popover_foreground` are mapped (UI-FND-002).
- **Performance**: a tooltip is one extra view only while visible. The table attaches the
  tooltip closure to the Name cell element only; the closure captures the `NodeId`, not the text.
- **Accessibility**: tooltips repeat the accessible name; nothing is tooltip-only (the row
  tooltip's lines are all available in Properties or Copy details).
- **Tests**: `tooltip::tests::text_wraps_at_360`, `tooltip::tests::lines_wrap_at_560`,
  `tooltip::tests::twelve_line_cut`,
  `tooltip::tests::escape_hides_keeps_focus`.
- **Phase**: P1.
- **Decided here**: max width 360 (text) and 560 (lines), 12-line cut, keyboard show, placement.

### UI-FND-020 TlToast queue rules

- **Implements**: APP-044 (position, max 4, 5 s, 8 s errors, hover pause, replace in place,
  system delivery when hidden), PROC-058 toasts, R-035, ADR-011 settings toasts, ADR-015 (a
  collection error never becomes a toast unless the user started an action).
- **Anatomy**:

```text
          ┌─ i_md icon ─┬──────────────────────────────────────┬───┐  width 382 (kit default)
          │   ●         │ Message text t_base text_primary     │ x │  min h 40, padding sp4
          │             │ second line wraps (max 3 lines)      │   │  bg menu_bg, 1 px divider
          │             │ [ Retry as administrator ]  (button) │   │  r_md, shadow (toast level)
          └─────────────┴──────────────────────────────────────┴───┘
   position: horizontally centered in the window, top = TITLEBAR_H 34 + TAB_STRIP_H 30 + sp4 8 = 72
   stacked downward with sp3 gap, newest at the bottom of the stack
```

  Icon per kind: Info `info` in `status.info`; Success `circle-check` in `status.success`;
  Warning `triangle-alert` in `status.warning`; Error `circle-x` in `status.danger`. Progress
  (in-flight actions, service jobs): `TlSpinner` `i_md` in `text_secondary`. Close button
  `TlButton::Icon` sm with `x` `i_sm`, always present. An optional action button is a
  `TlButton::Secondary` sm under the text, left-aligned. Text is at most 3 lines; longer text is
  cut with "…" and the full text is in the toast's tooltip.
- **States**: entering (slides down 8 px and fades in over `m_base` 200 ms; the kit's 400 ms
  enter is overridden by the tasklens settings below), shown, paused (pointer over any toast in
  the stack, or keyboard focus on one), exiting (fades over 200 ms `ease_in_cubic`), replaced (same
  key: the content swaps in place with no animation and the timer restarts). Reduced motion or
  `motion.animations = false`: no enter or exit animation.
- **Queue rules** (`components/toast.rs::ToastQueue`, one per window, a GPUI `Entity`):
  1. At most 4 toasts are visible. A fifth is appended to a FIFO wait list and appears when a
     slot frees. The wait list holds at most 16; a 17th drops the oldest waiting Info or Success
     toast (never a Warning, Error or Progress one).
  2. Durations from the moment the toast becomes visible: Info and Success 5 s, Warning 5 s,
     Error 8 s, Progress none (it stays until replaced or 120 s, when it becomes the Warning
     "Outcome unknown" per PROC-058), toast with an action button 15 s. The kit's fixed 5 s
     autohide is used only for the 5 s kinds; the others set `autohide(false)` and the queue runs
     its own `cx.spawn` timer with `remove_notification1::<ToastKey>(key)`.
  3. Hover or focus on any toast pauses every visible toast's timer; it resumes when the pointer
     leaves the stack (kit behaviour for 5 s toasts; the queue mirrors it for its own timers).
  4. Replace in place: `ToastKey(u64)` chosen by the owner (`ActionId` for actions, a unit hash
     for service jobs, a fixed key per settings warning). Fixed keys are at or above
     `0x8000_0000_0000_0000` (`SERVICES_LOADING_TOAST = ToastKey(0x8000_0000_0000_0101)`, Startup
     `..0201`, Connections `..0301`); the tracker's generated keys (`id << 8 | index`) stay below
     because `ActionId` stays below 2^55 (R-232). Pushing with an existing key swaps the
     content and kind and restarts the timer; the position in the stack is kept.
  5. Coalescing: a toast with the same kind and text as a visible one pushed within 1 s is not
     added; the visible one's timer restarts and a count suffix " (×2)" is appended from the
     second repeat on (so "Copied to clipboard" pressed five times shows one toast "Copied to
     clipboard (×5)").
  6. Priority: an Error pushed while 4 toasts are visible evicts the oldest visible Info or
     Success toast instead of waiting. Warnings and Progress toasts wait.
  7. Esc never closes toasts (table b reserves Esc). Clicking the toast body does nothing unless
     `on_click` was given; the X closes it; the action button runs its action and closes it.
  8. Visibility: toasts are pushed only while the main window is visible (ADR-010 hidden state);
     while hidden to the tray, Error toasts go to a desktop notification through
     `NotificationDelivery::System` and the others are dropped. On restore nothing is replayed.
  9. Secondary windows have their own queue and stack (each has a kit Root, APP-045). In P1 every
     window uses the main window's top (72 × `ui_scale`, 66 with server decorations), because the
     kit `NotificationSettings` is app-global; the per-window top (title bar 34 + sp4 = 42) comes
     in P2 with a kit change or a `ToastQueue::new(window, cx, top)` that takes it (R-195).
- **Interaction**: Tab reaches the toast stack last in the window's focus order; inside it Tab
  moves between the action button and the X of each toast. Enter on the X closes, Enter on the
  action runs it.
- **Data binding**: owners call `ToastQueue::push(kind, text, key: Option<ToastKey>, action:
  Option<ToastAction>)`. `ToastAction { label, action: Box<dyn Action> }`. The exact texts live in
  the owning spec entries; this file fixes only the shapes.
- **Build**: `components/toast.rs`: `ToastQueue` (Entity, created in `MainWindow::new`) over kit
  `window.push_notification(Notification::..)`. In `theme::map::apply`: `notification.placement =
  Anchor::TopCenter`, `margins.top = (TITLEBAR_H + TAB_STRIP_H) × ui_scale + sp4` = 72 at 100%
  (constants from the shell's `window/main.rs`), `max_items = 4`, `width = px(382)`. Enter and
  exit durations: the kit's are fixed at 400 and 200 ms; the queue cannot change them, so the
  entry animation is accepted as 400 ms (noted in "Open").
- **Performance**: the kit runs a 50 ms tick only while a toast animates or counts down. The queue
  timers are `cx.background_executor().timer` futures, one per non-5 s toast. No toast is pushed
  from render.
- **Accessibility**: each toast has its icon's kind as an accessible prefix ("Error: …") in the
  tooltip text; the stack is keyboard reachable.
- **Tests**: `toast::tests::fifth_waits_then_shows`, `toast::tests::error_evicts_oldest_info`,
  `toast::tests::same_key_replaces_in_place`, `toast::tests::coalesce_count_suffix`,
  `toast::tests::error_lives_8s_info_5s` (with `TestAppContext` time advance),
  `toast::tests::hidden_window_drops_or_routes_to_system`.
- **Phase**: P1.
- **Decided here**: stack order, wait list size, eviction, coalescing text, action toast 15 s,
  Progress 120 s, 3-line cut, focus order, secondary window margin.

### UI-FND-021 TlDialog and TlSecondaryWindow (frames)

- **Implements**: APP-045 (every dialog is a window, titles, modality), APP-009 and R-073 and
  PROC-056 (pin button on every window), PROC-059 confirmations, PROC-077 window frame, APP-014,
  APP-015, APP-023, APP-030, APP-037, APP-042, APP-053 frames, ADR-010 decorations, D-006.
- **Anatomy** of a secondary window (`TlSecondaryWindow`):

```text
┌────────────────────────────────────────────────────────────────┐ CSD, kit window_border shadow
│ icon16  Title t_base 500 text_primary       [pin][min][max][x] │ title bar 34, bg titlebar
├────────────────────────────────────────────────────────────────┤ 1 px divider
│                                                                │
│   content (owner's), bg window_bg                              │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

  Title bar: kit `TitleBar` (34 px, Linux left padding 12). The caption buttons are the kit's
  (34 px wide each on secondary windows; the main window draws its own at 36 wide, measured, per
  the shell; close hover `status.danger` fill with white icon, others hover `tab_hover`);
  the pin is a tasklens `TlButton::Toggle` 34 × 34 placed just left of minimize, icon Lucide `pin`
  `i_md`; on: `accent` fill at alpha 0.25 and the icon tinted `accent`, tooltip "Always on top"
  (off) or "Always on top (on)". Where no keep-above mechanism exists it is disabled with tooltip
  "Not supported by this desktop" (R-073). Minimize and maximize appear only when
  `window.window_controls()` reports them. Double-click the title maximizes (tool windows) or does
  nothing (modal confirmations); right-click opens the compositor window menu. The title text is
  the exact string from the APP-045 table or the owning spec, truncated at the end with "…" when
  the bar is narrow (never the middle; the identifying part of these titles is at the start).
  Under `appearance.decorations = "server"` the title bar is not drawn and the pin moves to the
  top-right corner of the content as a 24 px `TlButton::Toggle` (decided).

  Kinds:

| Kind | `WindowKind` | Modal | Default size | Min size | Resizable | Esc |
|---|---|---|---|---|---|---|
| Confirmation (`TlDialog::confirm`) | `Dialog` | yes, to its parent | content-sized, width 420, height 140 to 320 | 360 × 120 | no | Cancel |
| Tool window (Settings, Theme Editor, Shortcuts, Handle Search, About, Additional Setup, Crash Report) | `Floating` | no | per owner | 480 × 320 | yes (About and Crash Report: no) | closes |
| Run window ("tasklens Run", APP-014) | `Floating` | no | 480 × 248 fixed | 480 × 248 | no | cancels |
| Properties window (process, service, startup, connection) | `Floating` | no | 900 × 640 (PROC-077); service and others per owner | 640 × 400 | yes | closes |

  Confirmation body (`TlDialog`):

```text
┌───────────────────────────────────────────────┐ width 420
│ [i_lg icon]  End kwin_wayland?                │ optional heading t_md 500 text_primary, one line
│              Ending kwin_wayland will end     │ padding sp6; icon triangle-alert status.warning
│              your desktop session. End it     │ (danger actions) or info status.info; text t_base
│              anyway?                          │ text_primary, wraps, max 6 lines
│                                               │
│                          [ Cancel ] [ End ]   │ footer: right-aligned, gap sp4, buttons md
└───────────────────────────────────────────────┘
```

  The heading is optional (`TlDialog::confirm(..).heading(text)`); owners whose tables give a
  title use it (UI-PRC-014, UI-SYS-047, UI-SYS-064). The action button is `Primary`, or `Danger`
  for destructive actions (End, Kill, Delete, Close connection, Reset settings); Cancel is
  `Secondary` and sits left of it (rule 11; measured TaskSlinger order is Cancel then the
  action). Buttons name the result ("End tree", "Delete", "Reset"), never "Yes". A confirmation
  can carry one `TlCheckbox` under the text ("Do not ask again" where a spec allows it; none in
  P1). Extra lines under the text (an entry name, a path, a list of targets) are the owner's
  `t_base` or `t_sm` elements inside the body.
- **States**: active window (full shadow), inactive (kit dims the shadow and border), pinned
  (pin on), modal parent blocked (the parent's content gets `derived.overlay` painted over it and
  ignores input while the confirmation is open; the compositor also marks the dialog modal
  through `xdg_dialog_v1`), too many windows (PROC-077: the owner shows a toast and opens
  nothing), content loading (owner's skeleton, UI-FND-025), error ("Could not open properties."
  toast, no window).
- **Interaction**: Enter activates the focused button in a confirmation (bound on the window
  root, not when a multi-line input has focus); Esc cancels or closes per the table; Tab order:
  content, then footer buttons left to right, then the pin; Ctrl+W closes a tool or properties
  window; focus on open goes to the first focusable content element, or to Cancel in a
  destructive confirmation, so a stray Enter cancels and the destructive button needs Tab then
  Enter, or a click (decided; the one rule for every file). In a non-destructive confirmation
  focus starts on the action button, so Enter confirms. Closing the main window closes every
  child window except detached graphs when `graphs.keep_detached_on_close` is on (APP-045).
  Opening Properties again for the same key activates the existing window (PROC-077).
- **Data binding**: `WindowRegistry` (Entity in `window/registry.rs`) holds every open secondary
  window handle keyed by `WindowKey` (`Properties(ProcessKey)`, `AppProperties(GroupKey)`,
  `Service(unit)`, `Run`, `HandleSearch`, `Settings`, `Columns`, `Smart(DeviceIdx)`,
  `DiskInUse(DeviceIdx)`, and the other tool windows; the full list is in `docs/ui/INDEX.md`),
  enforces the 8-window limit for Properties, and closes children with the main window. Pin state
  for the main window is `window.always_on_top`; secondary windows keep it in memory only.
- **Build**: `components/window_frame.rs` (`TlSecondaryWindow::open(cx, kind, title, size,
  build_content)` calls `gpui_kit::open_window` with `WindowOptions { window_decorations:
  Some(Client), window_background: Opaque, kind, window_min_size, app_id: the tasklens app id,
  ..TitleBar::window_options() }`, then the kit Root wraps the view; the pin dispatches
  `ToggleAlwaysOnTop { window: WindowKey, caption: SharedString }` (`#[action(no_json)]`; the
  caption is the exact window title at dispatch time, the main window passes "tasklens" and
  `WindowKey` has a `Main` variant that is never stored in the registry; R-176), which `tl-ui`
  forwards to the engine's KWin bridge (APP-009; there is no X11 path in v1, R-177; tl-ui never
  talks to KWin itself)). `components/dialog.rs` (`TlDialog` body
  and footer). In-window kit `Dialog` is used only for sub-prompts inside a secondary window
  (APP-045), styled with the same body and footer.
- **Performance**: a secondary window has its own frame loop; its content follows the same
  caching rules as pages (ADR-009). A confirmation window costs one small view.
- **Accessibility**: title is the window title (compositor reads it); default and cancel buttons
  are keyboard reachable; the warning icon is paired with text.
- **Tests**: `window_frame::tests::confirm_enter_runs_focused_button_escape_cancels`,
  `window_frame::tests::destructive_focuses_cancel`, `registry::tests::ninth_properties_refused`,
  `registry::tests::main_close_closes_children`, `window_frame::tests::pin_disabled_without_bridge`.
- **Phase**: P1 (frame, confirmation, pin button drawn), P2 (pin working, properties windows).
- **Decided here**: sizes table, pin placement and look, server-decorations pin fallback, focus on
  Cancel for destructive confirmations with Enter on the focused button, optional heading line,
  title truncation rule.

### UI-FND-022 TlMenu and submenu (context, dropdown, menu bar menus)

- **Implements**: PROC-057 row menu, APP-012 to APP-026 menu bar menus, APP-036 theme submenu
  (painted items), SYS-008, SYS-025, SYS-032, PROC-024 header menu, column chooser, PERF graph
  menus, widgets rule 10 (one Action per command, shortcut hints), table (b) Shift+F10 and Menu.
- **Anatomy** (measured from `adv_sheet_01` context menu: rows about 0.8 of a table row, a 1 px
  lighter border, no icons, submenu arrows as ">" text; TaskSlinger draws no icon column in the
  row menu and a check column in View menus):

```text
┌───────────────────────────────────────────┐ bg menu_bg, 1 px divider, r_sm, menu shadow
│ sp2 padding                               │ padding y sp2
│ ┌───────────────────────────────────────┐ │
│ │ ✓ │ End Task                 Delete   │ │ item h 26; check/icon column 20; label t_base
│ │   │ End Process         Shift+Delete  │ │ shortcut column t_xs text_secondary, right, min gap sp6
│ ├───────────────────────────────────────┤ │ separator: hairline divider, margin y sp2, x sp4
│ │   │ Set Priority                    ▸ │ │ submenu chevron-right i_xs text_secondary
│ │   │ Set Affinity                    ▸ │ │
│ │   │ Copy all metrics (disabled)       │ │ disabled: opacity 0.5
│ └───────────────────────────────────────┘ │
└───────────────────────────────────────────┘ min w 180, max w 360; label truncated end "…"
```

  Item padding x `sp4`; the check or icon column is 20 wide and always reserved when any item in
  the menu has a check, icon or radio mark, otherwise the column collapses to 0 (the process row
  menu has none, matching the measured look; View menus have it). Check mark Lucide `check`
  `i_xs`; radio mark a 6 px filled circle; both `text_primary`. Header rows (`PopupMenu::label`)
  are `t_xs` `text_secondary` uppercase-free, 22 high, not selectable. Submenus open to the right
  with a 4 px overlap, flip left when there is no room, and align their first item with the parent
  item. Theme items (APP-036) are `menu_element` rows painted with their own theme's `menu_bg` and
  `text_primary`, 26 high, with the copy icon `copy` `i_sm` at the right (UI-FND-022 provides
  `TlMenu::painted_item(label, bg, fg, suffix_icon, action)`).
- **States**: hover or keyboard highlight: bg `menu_hover`, text `text_primary`, shortcut text
  `text_primary` too (so the row reads as one). Pressed: same as hover (the menu closes on
  release). Disabled: opacity 0.5, not highlightable by mouse, skipped by arrow keys. Checked and
  radio marks as above. Open submenu parent stays highlighted. Menu open animation: fade in over
  `m_fast` 120 ms with no slide (kit menus fade; the kit's duration is used as is); reduced motion
  instant. Empty menu (every item filtered out, column chooser): one disabled row "No items".
- **Interaction**: mouse: hover highlights, click runs the action and closes; a click on a submenu
  parent opens it without closing. Keyboard: Up/Down move (wrap, skipping separators and disabled
  items), Right opens a submenu and focuses its first item, Left closes a submenu back to its
  parent, Enter and Space run, Esc closes the topmost menu level and returns focus to what opened
  it (the row, the header cell, the button), typing a letter jumps to the next item starting with
  it, Home and End jump. Shift+F10 and the Menu key open the context menu for the current selection
  at the focused row's left edge plus `sp4` (table b). Right-click in a table selects the row first
  if it was not selected (ADR-009) and opens at the pointer. Menus clamp to the window; a menu
  taller than the window scrolls (`.scrollable(true)`, max height window minus `sp6`). Clicking
  outside closes and the click is consumed (it does not also select a row). Opening a menu closes
  any tooltip. Each item's shortcut hint is the kit's automatic lookup through the action context
  (`PopupMenu::action_context(focus_handle)`), so the row menu passes the table's focus handle and
  a menu bar menu passes the window's.
- **Data binding**: items are built when the menu opens from the owner's closure
  (`ContextMenuExt::context_menu` or `PopupMenu::build`); data-carrying actions capture their
  targets (`SmallVec<[ProcessKey; 1]>`, ADR-009). Dynamic availability (disabled items) is
  computed in the same closure from the current snapshot.
- **Build**: `components/menu.rs`: `TlMenu` is a thin builder over kit `PopupMenu` setting
  `.min_w(180)` (owners may raise it with `TlMenu::min_w(px)`: the process row and header menus
  use 240, the system table menus 220), `.max_w(360)`, `.scrollable(true)`,
  `.check_side(Side::Left)`, and providing
  `item`, `check_item`, `radio_item`, `submenu`, `separator`, `header`, `painted_item`. The look
  comes from the mapped tokens (`popover`, `accent`, `border`); the item height 26 is the kit's
  `text_sm` row at rem 13 plus padding, verified by the render test below. Row menu per table: one
  handler on the table element (not one per row).
- **Performance**: built on open only; nothing per frame while closed. Painted theme items are 12
  small divs.
- **Accessibility**: every command has a non-menu path (table b or the menu bar); shortcut hints
  are shown; keyboard navigation complete.
- **Tests**: `menu::tests::arrow_keys_skip_separators_and_disabled`,
  `menu::tests::right_opens_submenu_left_closes`, `menu::tests::shift_f10_opens_at_row`,
  `menu::tests::item_height_is_26_at_100pct` (render test), `menu::tests::outside_click_consumed`.
- **Phase**: P1.
- **Decided here**: item height 26, column widths, min width 180 with an owner override and
  max width 360, header row look, painted item API, empty text, submenu overlap and alignment.

### UI-FND-023 TlProgress and TlSpinner

- **Implements**: APP-044 progress toasts, SYS-005 job tracking rows, PROC-078 hash computation
  ("computed in the background"), APP-042 setup steps, PROC-083 Strings scan, kit rules in
  ADR-009 (no Spinner, Skeleton or Progress in table cells or charts).
- **Anatomy**: `TlProgress`: bar height 4 (sm) or 6 (md), track `scroll_track`, fill `accent`,
  radius full, width from the parent; determinate `.value(0..=100)`; indeterminate: a 30% wide
  segment moving left to right over 1.2 s, looping. Optional right-aligned `t_xs` percent label
  with `tnum`. `TlSpinner`: Lucide `loader-circle` at `i_sm` (14) or `i_md` (16), colour
  `text_secondary` by default, rotating 360° per 0.8 s (kit).
- **States**: determinate value changes jump (no transition; the kit's transition is turned off
  by giving a new `id` per value bucket of 1%, decided, because an animated bar in a 1 Hz-updated
  row requests frames). Indeterminate loops. Reduced motion: the indeterminate bar becomes a
  static 30% segment at the left plus the text "Working…" from the owner; the spinner becomes a
  static `loader-circle` icon at 50% opacity.
- **Interaction**: none. Tooltip from the owner where the meaning is unclear.
- **Where they may be used**: toasts, dialog footers, the Properties General tab hash row, the
  setup window, the Strings tab header, the Services status bar text. Never in a table cell, a
  chart, a sidebar card or the status bar of the main window (ADR-009).
- **Build**: `components/progress.rs`, wraps kit `Progress` (`.small()` or default, `.loading(bool)`)
  and `Spinner`. Both keep requesting frames while visible, so the owner removes them from the
  tree as soon as the work ends (not `visibility hidden`).
- **Performance**: at most 2 spinners or indeterminate bars visible per window at a time is the
  budget; a test counts them in the gallery.
- **Tests**: `progress::tests::reduced_motion_static`, `progress::tests::removed_when_done`.
- **Phase**: P1 (toasts), P2 (the rest).
- **Decided here**: sizes, 1% bucketing, reduced-motion stand-ins, usage list and budget.

### UI-FND-024 TlSkeleton rows and blocks

- **Implements**: APP-059, R-060, PROC-021 (no skeleton on Processes), SYS-016.
- **Anatomy**: `TlSkeletonRows { rows: usize, columns: &[Pixels] }` draws `rows` fake table rows
  of `row_height` with one rounded bar (height 10, radius `r_sm`, colour `skeleton` token from
  UI-FND-002) per column, width 40% to 80% of the column width from a fixed pattern
  (`[0.6, 0.45, 0.8, 0.5, 0.7]` repeating), inset `sp4`. `TlSkeletonBlock { lines }` draws text
  lines of height 10 with `sp3` gaps and widths `[0.9, 0.7, 0.8, 0.5]` repeating, for key/value
  grids and Information cards. The whole skeleton pulses opacity 0.5 to 1.0 over 2 s (kit
  `Skeleton` animation); under reduced motion it is static at 0.7.
- **States**: shown until the first data for that tab arrives in the session; then replaced in
  the same frame (no fade). Never shown again for that tab, even if the data later becomes empty
  (that is the empty state, UI-FND-029) or stale.
- **Row count**: `min(visible rows that fit, 12)` for tables; 8 lines per block.
- **Build**: `components/skeleton.rs`, kit `Skeleton` elements inside a `v_flex` of row divs.
- **Performance**: requests frames while visible (2 s loop). It is shown once per tab per session
  for under a few seconds; the budget is fine. It must not be inside `uniform_list`.
- **Tests**: `skeleton::tests::shown_once_per_tab` (fixture: empty snapshot, then data, then empty
  again; the skeleton appears only in the first state).
- **Phase**: P1 (component), P2 (used by the P2 tabs).
- **Decided here**: bar sizes and width pattern, 12-row cap, no fade on replace.

### UI-FND-025 TlKeyValueGrid (Properties, Information, Performance stats)

- **Implements**: PERF-012 ("Details" stats grid: label over value, three columns), PERF-013
  words, PROC-078 (three key/value columns, click copies), Information tab cards (PERF-100 to
  PERF-104 sections, `information.png`), SYS-011, SYS-026, SYS-034 property pages, D-007 none.
- **Anatomy**: two layouts in one component.

  Stacked (Performance stats, measured from `performance.png`: label in secondary colour over the
  value in primary colour, three equal columns, about 2.4 rows per pair):

```text
Utilization          Kernel time          Speed                 label t_sm text_secondary
18.6%                2.3%                 4631 MHz              value t_base text_primary tnum
                                                                pair height 2 × row_height (56)
Processes            Threads              Handles               columns: equal, min 160
295                  5596                 253248                column gap sp7, row gap 0
```

  Inline (Information and Properties General, measured from `information.png`: label with a
  colon in secondary colour, value in primary colour starting at a fixed x of about 360 px at that
  scale, one row per pair):

```text
OS Name:             Linux                                      row height row_height (28)
Version:             6.17.0-arch1                                label column 180, value flex
Build:               …                                          label ends with ":" (owner's text)
```

  Owner overrides for the inline layout: `.row_height(Pixels)`, `.label_width(Pixels)`,
  `.block_indent(Pixels)` and `.block_gap(Pixels)`. The defaults are 28, 180, `sp6` and `sp4`.
  The Information tab passes `.label_width(232)`, `.block_indent(sp5)` and `.block_gap(sp5)`
  (measured from `information.png`, UI-SYS-081). Process Properties General uses
  `.row_height(26 × ui_scale)` and a per-column label width of 96 to 150 px (UI-PWN-006; 126
  measured in the TaskSlinger Properties window, where three columns share a 900 px window). Every
  value is multiplied by `ui_scale` by the owner through `UiMetrics`; the grid never scales twice.

  Section title (Information cards, measured): a band of `header` colour, `row_height` high, with
  a 3 px `accent` bar at its left edge, title `t_md` 500 `text_primary` with `sp4` padding, and a
  copy `TlButton::Icon` (`copy` `i_sm`) at the right end that copies the section (PERF-011 format).
  The gap between cards is the page's (`sp6` by default; the Information page uses `sp8` 32,
  measured 37, UI-SYS-080) and `sp4` sits between the band and the first row. A sub-block
  (memory "Module 1", "Module 2") indents its rows by `block_indent` and is separated from the
  previous block by `block_gap`.

  Values are selectable text (PERF-012): the value element is a `Label` with text selection
  enabled; a click on a Properties value copies it with the toast "Process property value copied."
  (PROC-078) and the row shows a 120 ms `accent` at 0.25 flash. Links (PPID, Executable, Unit) are
  `status.info` with an underline on hover and the pointer cursor. Long values truncate at the end
  with "…" and get a tooltip with the full text; paths use the middle ellipsis (UI-FND-052).
- **States per value** (`TlValue { text, validity, tooltip }`, the PERF-013 word table): Valid
  shows the text in `text_primary`; WarmingUp "Collecting…"; Stale: last value in `text_secondary`
  with tooltip "Last updated {age} ago"; Restricted "Restricted"; Unsupported "Not supported",
  "Not reported" or "Needs setup" per the owner's reason, with "Needs setup" as a link to the
  Additional Setup window; Missing: the row or card is removed; Failed "Unavailable" with tooltip
  "Could not read: {last_error}". Every non-valid word is `text_secondary`. Rows that PERF-013
  lists as hidden-when-missing are omitted by the owner. Empty grid (filtered to nothing in
  Properties): "No properties match the current filter." centered, `t_base` `text_secondary`.
  Loading: `TlSkeletonBlock`.
- **Interaction**: click copies (Properties); text drag selects; Ctrl+C with a selection copies the
  selection, without one it runs the page's Copy action (PERF-011 on Performance). Context menu on
  a value: "Copy value", "Copy row" (`Label: value`), "Copy section".
- **Data binding**: the owner passes `&[TlKvRow]` built from its snapshot slice; the component
  re-renders when the owner notifies. The grid caches nothing itself; the owner keeps the
  formatted `SharedString`s and re-formats only changed values (the same rule as `CellTextCache`).
- **Build**: `components/kv_grid.rs`; custom `div` grid (kit `DescriptionList` does not do the
  stacked layout or the validity words). Columns are fixed widths computed once from the available
  width (`floor((w - 2 × sp7) / 3)`).
- **Performance**: a Performance stats grid has 12 to 30 values updating at 1 Hz; each is one
  text element; the page's `.cached()` wrapper skips repaint when nothing changed.
- **Accessibility**: label and value are adjacent text nodes; the copy button has a tooltip
  "Copy section".
- **Tests**: `kv_grid::tests::validity_words` (one row per validity, asserts text and colour),
  `kv_grid::tests::stacked_three_columns`, `kv_grid::tests::click_copies_and_toasts`.
- **Phase**: P1 (stacked and inline), P2 (links, section copy).
- **Decided here**: label column 180, pair height, section band look, flash on copy, context menu
  items.

### UI-FND-026 TlValidityGlyph (table cells, header totals, sidebar readouts)

- **Implements**: ADR-003 validity table, R-050, PERF-013 (the word table is UI-FND-025; this is
  the glyph table), PROC-021 (`…` in rate cells on the first sweep).
- **Anatomy**: `TlValidityGlyph::render(validity, value_text: &SharedString, m, theme) ->
  impl IntoElement`, used by every numeric cell and readout:

| Validity | Element | Colour | Tooltip (UI-FND-019, 500 ms) |
|---|---|---|---|
| Valid | the formatted text | `text_primary` (or `text_selected_primary` on a selected row) | none |
| WarmingUp | `…` (U+2026) | `text_secondary` | "Collecting first sample" |
| Stale | the last value | `text_secondary` | "Last updated {age} ago" (short age format, table d) |
| Restricted | Lucide `lock` `i_xs` 12, right-aligned like a number, vertical center | `text_secondary` | "Needs administrator access. Click the shield to grant it." (before P3: "Needs administrator access.") |
| Unsupported | `n/a` | `text_secondary` | "Not available on this device" |
| Missing | `-` (U+002D) | `text_secondary` | "No longer present" |
| Failed | `!` | `status.warning` | "Could not read: {last_error}" |

  Glyph cells keep the column's alignment (right for numeric, left for text) and width, so a
  change from `…` to a number does not move anything. On a selected row the secondary colour becomes
  `text_selected_secondary`. Heat colouring applies only to Valid cells (a glyph cell never gets a
  tint). Charts: Valid drawn; Stale drawn at 50% alpha after the last fresh sample then a gap; all
  others a gap (ADR-003), handled by the chart element, not this component.
- **States**: as the table above. Hover shows the tooltip; nothing else changes.
- **Interaction**: Restricted glyph click: no action of its own (the tooltip tells the user to use
  the shield); the row click still selects. Copy actions on a glyph cell copy the glyph's text
  (`…`, `n/a`, `-`, `!`) and for Restricted the word "Restricted" (ADR-015 copy formats).
- **Data binding**: `Validity` from `FieldValidity` per `ProcessField`, or the `ComponentHeader`
  validity for totals.
- **Build**: `components/validity.rs`; the text variants are `SharedString` statics (no
  allocation); the lock is an `Icon` element. `CellTextCache` keys include the validity (ADR-009)
  so the glyph text is cached like a value.
- **Performance**: a glyph is one text run; the lock icon is one SVG quad. No per-frame work.
- **Accessibility**: the tooltip carries the meaning; copy formats spell it out.
- **Tests**: `validity::tests::glyph_per_state` (render test asserting text, colour and that the
  width of `…`, `n/a`, `-`, `!` never exceeds the "100.0%" template width);
  `validity::tests::selected_row_colours`.
- **Phase**: P1.
- **Decided here**: lock icon right-aligned like a number; copy text for glyph cells.

### UI-FND-027 TlBadge (status badges, counts)

- **Implements**: SYS-003 service state column ("running", "failed", …), SYS-020 startup Status
  (Enabled, Disabled), SYS-029 connection State, PROC-057 "+N" child counts next to a PID are not
  badges (plain `t_xs` text), APP-007 is dropped (no update badge), PROC-077 tab counts are plain
  text.
- **Anatomy**: a pill 18 high, padding x `sp3`, radius full, `t_xs` 500 text, min width 18.
  Kinds: `Neutral` (bg `header`, text `text_primary`), `Success` (bg `status.success` at 0.2,
  text `status.success`), `Warning`, `Danger`, `Info` (same pattern with their colour), `Count`
  (bg `accent` at 0.25, text `text_primary`, `tnum`, shows `99+` above 99). An optional leading
  6 px dot in the status colour (`.dot(true)`) is used for service states so the state is not
  colour-only when the text is short. In a table cell the badge is vertically centered and
  left-aligned with the cell padding; the cell's selected and hover colours show around it.
- **States**: static. On a selected row the tinted backgrounds stay (they are translucent) and the
  text keeps its status colour, which stays above 4.5:1 on `row_selected` in both themes
  (checked for the four status colours; the light `status.warning` on light `row_selected` is
  4.6:1).
- **Interaction**: none; tooltip from the owner (for example the full systemd sub-state).
- **Build**: `components/badge.rs`, custom `div` (kit `Badge` is a count overlay and kit `Tag` has a
  different height; neither used).
- **Performance**: one quad plus one text run; cached through `CellTextCache` like other cells.
- **Tests**: `badge::tests::kinds_render`, `badge::tests::count_caps_at_99_plus`.
- **Phase**: P2 (first user is the Services tab).
- **Decided here**: all geometry, tint alpha 0.2, the dot rule.

### UI-FND-028 TlShield (privilege state button and badge)

- **Implements**: APP-003 (shield locked and unlocked, tooltips, hidden before P3 and without
  tl-helper), APP-002 toolbar slot, PROC-058 "Retry as administrator" (the toast button dispatches
  the same `StartHelperSession` action the shield uses), ADR-003 Restricted tooltip "Click the
  shield".
- **Anatomy**: a `TlButton::Toggle` 24 × 24 in the toolbar (position per `shell.md`), icon
  `shield` `i_md` in `text_primary` when locked; unlocked: app SVG `shield-unlocked.svg` (a shield
  with an open padlock cut-out, 16 px grid, 1.5 px strokes, drawn by the icon bead) tinted
  `status.success`, with the Toggle "on" fill `accent` at 0.25 and 1 px `accent` border. Measured:
  TaskSlinger's shield is a two-tone blue and yellow 16 px glyph at the left of the toolbar;
  tasklens uses the single-colour Lucide glyph (decided; no TaskSlinger artwork ships, D-005).
  While a polkit prompt is open or the helper is starting: the icon is replaced by `TlSpinner`
  `i_md` and the button is inert. Badge: when the helper session is alive, the status bar's right
  side shows nothing extra (the toolbar state is enough); when a privileged action was refused
  (EPERM) in the last 10 s and the helper is not installed, the status bar shows "Restricted: N
  processes" (APP-029), which is the shell's.
- **States**: hidden (before P3, or `/usr/lib/tasklens/tl-helper` missing); locked (default);
  starting (spinner, 120 s deadline per ADR-006, then back to locked with the toast
  "Authentication cancelled" if the prompt was dismissed or "Could not start the privileged
  helper" on any other failure); unlocked (on look, tooltip "Privileged actions unlocked (click
  to lock)"); hover and pressed and focus as TlButton::Toggle. The helper's 300 s idle exit moves
  the button back to locked without a toast (APP-003).
- **Interaction**: click or Space or Enter: locked dispatches `StartHelperSession`; unlocked
  dispatches `EndHelperSession`. Tooltip when locked: "Unlock privileged actions"; while starting:
  "Waiting for authentication…" (R-201). A failed start is reported once through
  `HealthSnapshot.helper_start` (R-148). Shift held at
  launch and `privilege.unlock_at_launch` start the session at launch, which shows the starting
  state in the toolbar.
- **Data binding**: `HealthSnapshot.helper: HelperState { Missing, Locked, Starting, Unlocked }`
  (the engine publishes it; ADR-006). Re-renders on the `health` `ComponentMask` bit.
- **Build**: `components/shield.rs`; `assets/icons/shield-unlocked.svg`.
- **Performance**: one button.
- **Accessibility**: tooltip text is the accessible name; the state is also in text in the
  tooltip, not colour alone.
- **Tests**: `shield::tests::hidden_without_helper`, `shield::tests::states_and_tooltips`.
- **Phase**: P3 (hidden until then; the component is written in P3).
- **Decided here**: single-colour glyph, starting state look, failure toast text "Could not start
  the privileged helper".

### UI-FND-029 TlEmptyState

- **Implements**: PROC-077 "No properties match the current filter.", PROC-079 "No threads
  found.", SYS-036 and other "no rows" states, APP-059 (empty is not loading), filter with no
  matches in every table.
- **Anatomy**: centered block in the content area: optional icon `i_lg` `text_secondary`, text
  `t_base` `text_secondary` (the exact string from the owning spec), optional second line `t_sm`,
  optional `TlButton::Secondary` sm ("Clear filter"). Vertical gap `sp4`, outer padding `sp8`. The
  table header stays visible above it so the user keeps the columns.
- **States**: static. When the empty state is because of a filter, the "Clear filter" button is
  shown and dispatches `ClearFilter`.
- **Build**: `components/empty.rs`.
- **Tests**: `empty::tests::clear_filter_button_only_when_filtering`.
- **Phase**: P1.
- **Decided here**: layout; the button rule.

### UI-FND-030 Component gallery (`--gallery`)

- **Implements**: ADR-013 render tests need a host; the brief's "every state" requirement.
- **Anatomy**: `tasklens --gallery` (built with `--features bench`, next to `--bench`) opens one
  window with a `TlTabs` strip, one page per component above, each page listing every variant and
  state in both themes side by side (left half Default Dark, right half Default Light, by
  rendering two `TlTheme`-scoped subtrees), with reduced motion toggled by a `TlSwitch` at the top.
- **Build**: `crates/tl-ui/src/components/gallery.rs`. The render tests in ADR-013 open the same
  pages headless and compare against golden PNGs (`TL_UPDATE_GOLDEN=1` rewrites them).
- **Tests**: `gallery::tests::every_component_has_a_page` (a const list checked against the
  module list).
- **Phase**: P1.
- **Decided here**: the gallery exists and is the golden-image host.

### UI-FND-031 TlBanner (information strip inside a pane)

- **Implements**: SYS-015 journal access notice (UI-SYS-038), PERF-105 "Needs setup" context,
  UI-PRC-036 recording-off banner, APP-058 (no colour-only meaning).
- **Anatomy**:

```text
┌──────────────────────────────────────────────────────────────────────────┐ height 32
│ sp5 │ i info 14 │ sp3 │ text t_sm text_primary, one line, end ellipsis │ [Button] sp5 │
└──────────────────────────────────────────────────────────────────────────┘ 1 px bottom border
```

  Fill `status.info` at alpha 0.12; bottom border 1 px `status.info`; icon Lucide `info` 14 px in
  `status.info`; text `t_sm` `text_primary`, one line, ellipsis at the end with the full text in a
  tooltip; zero or more trailing `TlButton::Secondary` sm buttons, gap `sp4`, right-aligned. A
  Warning banner swaps `status.info` for `status.warning` and the icon for `triangle-alert`.
  Kinds: `TlBannerKind::{Info, Warning}`.
- **States**: shown; hidden (the owner removes it; a banner has no close button of its own
  because it states a fact about the data below it; an owner that wants dismissal adds a "Hide"
  button).
- **Interaction**: none on the strip itself; the buttons are ordinary `TlButton`s and Tab stops
  in reading order.
- **Data binding**: owner passes `text`, `kind` and `buttons: Vec<(SharedString, Box<dyn
  Action>)>`.
- **Build**: `components/banner.rs`, a `RenderOnce` `h_flex`.
- **Performance**: static; part of the owner's cached subtree.
- **Accessibility**: the text is plain; the icon is decorative; buttons carry their labels.
- **Tests**: `banner::tests::info_and_warning_colours`, `banner::tests::long_text_ellipsis_tooltip`.
- **Phase**: P2 (first use: Logs tab, UI-SYS-038).
- **Decided here**: height 32, fill alpha 0.12, no close button.

### UI-FND-032 TlLink (inline link text)

- **Implements**: PROC-078 PPID, Executable and Unit links, SYS-011 documentation and Main PID
  links, PERF-061 "Network settings", PERF-013 "Needs setup", UI-SYS-034 "Additional setup…".
- **Anatomy**: inline text in the surrounding text style (`t_base` unless the owner says
  otherwise), colour `status.info`; hover: underline and the pointer cursor; focused (keyboard):
  focus ring; pressed: `status.info` darkened 8%; disabled: `text_secondary`, no underline, no
  pointer. No icon. Long link text truncates like its container (UI-FND-052).
- **Interaction**: click, Enter or Space dispatches the owner's `Box<dyn Action>`; a link is a
  Tab stop. Right-click: the owner's menu, if any (the `TlKeyValueGrid` adds "Copy value").
- **Data binding**: `TlLink::new(text, action)`; `.disabled(reason_tooltip)`.
- **Build**: `components/link.rs`, a `div` with `cursor_pointer`, `on_click` and a focus handle.
  The `TlKeyValueGrid` link value (UI-FND-025) is a `TlLink`.
- **Performance**: static.
- **Accessibility**: the text is the accessible name; links are reachable by keyboard.
- **Tests**: `link::tests::hover_underlines`, `link::tests::enter_dispatches_action`.
- **Phase**: P2.
- **Decided here**: colour `status.info`, underline on hover only, Tab stop.

## 3. Theme pipeline

### UI-FND-040 TlTheme global and the switch path

- **Implements**: ADR-009 theme model and "Theme switch", APP-035, APP-036 (follow system,
  200 ms crossfade), R-017, R-069, R-068.
- **Anatomy**:

```rust
// crates/tl-ui/src/theme/mod.rs
pub struct TlTheme {
    pub name: Arc<str>, pub mode: ThemeMode, pub version: u64, pub builtin: bool,
    // 22 slots + accent + graph (ADR-009), plus:
    pub status: StatusColors,     // success warning danger info
    pub derived: Derived,         // UI-FND-001 derived tints, rebuilt by rebuild_derived()
}
pub struct ThemeStore {              // GPUI Global, one per app
    pub builtin: Arc<[ThemeDef]>,    // 2 in P1, 12 in P2, in TaskSlinger order
    pub user: Vec<ThemeDef>,         // from themes.json, in file order
    pub active: Arc<TlTheme>,        // the current theme; views read cx.global::<ThemeStore>().active
    pub preview: Option<Arc<TlTheme>>, // live editor preview (UI-FND-043); when Some, it is what renders
    pub system_scheme: ColorScheme,  // portal value 0, 1 or 2
}
```

  `ThemeDef` is the parsed JSON record (name, 22 slots, `accent`, optional `status`, optional
  `graph`, `mode` derived from `window_bg` lightness when the file has no `mode` key: lightness
  below 0.5 is dark).

  Switch path (`ThemeStore::activate(name, cx)`):
  1. Resolve `name` against `user` first, then `builtin` (a user theme may shadow a built-in name;
     the menu shows both, the user one marked "(custom)").
  2. Build `TlTheme` from the def, run `rebuild_derived(appearance)`.
  3. If `motion.animations` is on and reduce-motion is off: start a 200 ms crossfade (`m_base`).
     The crossfade is a `TlTheme` interpolation: each frame, `active` is replaced by
     `lerp(old, new, t)` in Hsla space for every slot, status and graph colour (about 40 lerps),
     and `style_version` bumps once at the start and once at the end, not per frame, so chart
     geometry rebuilds twice, not 12 times. The frames are driven by the window's
     `AnimationDriver` (a one-shot request per frame, same mechanism as charts). Instant when
     animations are off.
  4. On completion: `theme.active = name` is written to settings (unless the switch came from
     `follow_system`, which writes nothing), `theme::map::apply` runs once with the final theme,
     `CellTextCache` is not cleared (text colour is not part of the cache key; only
     `style_version` consumers rebuild).
  5. Every view that reads colours re-renders because `MainWindow` observes `ThemeStore` and
     notifies its children through the usual subscription; `.cached()` wrappers see the new
     `style_version` in their keys and repaint.

  Follow system (APP-036): `ThemeStore` holds the portal `color-scheme` value, fed by the engine's
  D-Bus worker through `SnapshotHub` (`system.color_scheme`, updated on `SettingChanged`). When
  `theme.follow_system` is on, a change selects `theme.dark` for 0 and 1 and `theme.light` for 2,
  through the same `activate` path with the crossfade. Turning follow-system off keeps the current
  theme and writes it to `theme.active`.

  Scale and radius changes do not go through `activate`; they bump `style_version` through
  `UiMetrics::recompute` and clear `CellTextCache` (ADR-009).

- **States**: idle; crossfading (input still works; a second switch during a crossfade restarts
  from the current interpolated colours); previewing (UI-FND-043).
- **Interaction**: View > Theme items dispatch `ActivateTheme { name }`; the Settings window's
  theme fields dispatch the same action. Toasts per APP-036 belong to the shell's menu entry; the
  store emits `ThemeEvent::{Activated, Deleted, Copied, ImportFailed, Imported { count: u32 }}` and
  the shell turns them into toasts; `ThemeError` has `BuiltinCannotBeDeleted` and
  `BuiltinCannotBeEdited` beside its other variants (R-209).
- **Data binding**: `[theme]` section of table (a); `system.color_scheme` from the snapshot.
- **Build**: `theme/mod.rs` (`TlTheme`, `ThemeStore`, `activate`, crossfade), `theme/derived.rs`,
  `theme/map.rs` (UI-FND-002). The crossfade uses the window's `AnimationDriver::request` with a
  reserved `EntityId` for the theme, so there is still exactly one `on_next_frame` per frame.
- **Performance**: 40 Hsla lerps per crossfade frame, 12 frames at 60 Hz; two geometry rebuilds
  per switch (ADR-009 asks for "once"; the crossfade needs the start and end, and a switch with
  animations off rebuilds exactly once).
- **Tests**: `theme::tests::activate_user_shadows_builtin`,
  `theme::tests::crossfade_bumps_style_version_twice`,
  `theme::tests::follow_system_picks_light_for_2_dark_otherwise`,
  `theme::tests::animations_off_is_instant_and_single_rebuild`.
- **Phase**: P1 (activate, instant switch), P2 (crossfade, follow system, 10 more themes).
- **Decided here**: user theme shadows built-in with "(custom)" marker, two rebuilds per animated
  switch, restart-from-current on double switch, mode inferred from `window_bg` lightness.

### UI-FND-041 Built-in theme loading (`assets/themes/builtin.json`)

- **Implements**: ADR-009 ("Built-in themes ship as `assets/themes/builtin.json`"), APP-035, R-068,
  ADR-011 (built-ins are not on disk).
- **Anatomy**: the file is embedded with `include_str!` (it is not read from disk at run time) in
  TaskSlinger's format with the tasklens additions:

```json
{"themes":[{"name":"Default Dark","mode":"dark",
  "WINDOW_BG":"#191B1D","TITLEBAR":"#27292B","TOOLBAR":"#202224","INPUT":"#202224",
  "INPUT_FOCUS":"#25282B","HEADER":"#2E3235","LIST_BG":"#191B1D","ROW_ALT":"#1D1F20",
  "ROW_HOVER":"#232629","ROW_SELECTED":"#075B84","DIVIDER":"#2B2F33","MENU_BG":"#2A2E31",
  "MENU_HOVER":"#104B68","TAB_BG":"#202224","TAB_HOVER":"#27292B","SCROLL_TRACK":"#1A1C1E",
  "SCROLL_THUMB":"#494F54","TEXT_PRIMARY":"#D6D8D9","TEXT_SECONDARY":"#959A9C",
  "TEXT_SELECTED_PRIMARY":"#FFFFFF","TEXT_SELECTED_SECONDARY":"#BFD9E6","VALUE_HEAT":"#3E2A28",
  "accent":"#0277A6",
  "status":{"success":"#4CAF50","warning":"#E0A526","danger":"#D9534F","info":"#2A9FD6"},
  "graph":{"cpu_fill":"#0D546D","cpu_line":"#0A7096","kernel_line":"#C1514E","mem_fill":"#7E6A9A",
    "mem_line":"#A58CC4","gpu_fill":"#508A59","gpu_line":"#71B768","disk_fill":"#8A7340",
    "disk_line":"#D9B567","net_fill":"#1E5C66","net_line":"#359CAB","grid":"#2A3033","chart_bg":"#1D2223"}},
 {"name":"Default Light","mode":"light", "...": "UI-FND-001 light values"}]}
```

  Slot keys are the upper-case TaskSlinger names from APP-035; `accent`, `mode`, `status` and
  `graph` are lower-case tasklens keys so a TaskSlinger import never collides with them.
  Parsing rules: every one of the 22 slots is required for a built-in (a missing slot is a build
  failure through the parse test); `mode` optional (derived from `WINDOW_BG` lightness); `status`
  and `graph` optional (defaults: the Default Dark or Default Light values for the theme's mode).
  Colour strings accept `#RRGGBB` and `#RRGGBBAA`; anything else fails the parse.
  The 10 P2 themes are appended in TaskSlinger order (APP-035) with values extracted per APP-035's
  mechanism; their `status` and `graph` keys are omitted unless the extraction yields them, so they
  fall back to the mode defaults.
- **Build**: `theme/builtin.rs`: `pub fn load() -> Arc<[ThemeDef]>` using `serde_json` into a
  `BTreeMap<String, serde_json::Value>` per theme, then a typed conversion that reports the theme
  name and key on error. Runs once at `TasklensUi::init`.
- **Performance**: once at start; under 1 ms for 12 themes.
- **Tests**: `builtin::tests::all_slots_present_in_every_theme`, `builtin::tests::names_in_ts_order`
  (the const list from APP-035), `builtin::tests::light_theme_mode_inferred_if_missing`.
- **Phase**: P1 (2 themes), P2 (12).
- **Decided here**: `include_str!`, key casing rule, `#RRGGBBAA` accepted, strict parse for
  built-ins.

### UI-FND-042 User themes file (`themes.json`)

- **Implements**: ADR-011 (path, atomic write), APP-036 (Create copy, Delete, Import), APP-038
  (`TL1.` codes and raw JSON import), R-018, R-079.
- **Anatomy**: `$XDG_CONFIG_HOME/tasklens/themes.json`, same format as UI-FND-041. Loading, at
  start and after every write:
  1. Missing file: `user` is empty; nothing is written until the first copy or import.
  2. Parse error: rename to `themes.json.corrupt-<unix seconds>`, `user` empty, toast (Warning)
     "Themes file was unreadable and has been reset. The old file was kept as
     themes.json.corrupt-<unix seconds>" (the ADR-011 settings pattern applied to this file).
  3. Per theme, lenient: a missing slot takes the value from the built-in theme of the same
     `mode` (Default Dark or Default Light) and a `warn!` names the theme and slot; a bad colour
     string does the same; a theme without a name gets "Custom theme N"; duplicate names get " (2)",
     " (3)" appended. A file with more than 64 themes keeps the first 64 and logs the count.
  4. Unknown keys are preserved on save (the file is round-tripped through the `BTreeMap`), so a
     file shared with TaskSlinger keeps TaskSlinger-only keys.
  Saving: `write_atomic` (ADR-011) of the whole list, pretty-printed with 2-space indent, after
  Create copy, Delete, Import, and when the Theme Editor finishes (Done). Never on activate.
  Create copy: the new theme is named "<name> copy", or "<name> copy 2" and so on, placed at the end
  of `user`, and activated. Delete: asks per APP-036 and, if the deleted theme was active, activates
  `theme.dark` or `theme.light` by the deleted theme's mode. Import accepts raw JSON of a
  `{"themes":[…]}` object or a single theme object, and `TL1.` codes (APP-038); `TS1.` codes per
  R-079 after U-17 is verified.
- **Build**: `theme/user_store.rs`; file I/O through the `tl-app` persist callback (tl-ui does no
  file I/O itself, ADR-011): `PersistTheme::{Save(Vec<ThemeDef>), RenameCorrupt}` requests.
- **Tests** (with `tempfile`): `user_store::tests::missing_file_empty`,
  `user_store::tests::corrupt_renamed_and_toast`, `user_store::tests::missing_slot_falls_back`,
  `user_store::tests::unknown_keys_round_trip`, `user_store::tests::copy_names_increment`.
- **Phase**: P2 (load, copy, delete), P3 (import codes, editor save).
- **Decided here**: lenient per-theme repair, 64-theme cap, round-trip of unknown keys, copy
  naming, fallback on delete of the active theme.

### UI-FND-043 Theme editor live-preview contract

- **Implements**: APP-037 ("Edits apply live to the app; Cancel reverts"), R-083 title. The
  editor UI itself (slot list, hue strip, colour plane, readouts, buttons) is in `shell.md`; this
  entry fixes how the editor and the rest of the app exchange colours.
- **Anatomy** (`theme/preview.rs`):

```rust
pub struct ThemePreview;            // zero-size handle type; methods on ThemeStore
impl ThemeStore {
    pub fn begin_preview(&mut self, base: &ThemeDef, cx) -> PreviewToken;  // snapshots `active`
    pub fn set_preview_slot(&mut self, tok: &PreviewToken, slot: ThemeSlot, c: Hsla, cx);
    pub fn set_preview_name(&mut self, tok: &PreviewToken, name: Arc<str>);
    pub fn commit_preview(&mut self, tok: PreviewToken, cx) -> ThemeDef;   // Done
    pub fn cancel_preview(&mut self, tok: PreviewToken, cx);               // Cancel, or editor window closed
}
pub enum ThemeSlot { WindowBg, Titlebar, …, ValueHeat, Accent }  // 22 + accent = 23
```

  Rules:
  1. Only one preview exists at a time; `begin_preview` while one is live cancels the first
     (and the first editor window is closed by the shell).
  2. While `preview` is `Some`, every view reads `preview` instead of `active` (the accessor
     `ThemeStore::current()` returns whichever applies; views never read `active` directly).
  3. `set_preview_slot` replaces `preview` with a new `Arc<TlTheme>` (clone, one field changed,
     `rebuild_derived`), bumps `style_version` and notifies. No crossfade during preview. Calls
     are coalesced to one per frame: the editor's colour plane drags at pointer rate, and the store
     applies the last value in `on_next_frame`.
  4. Editing `Accent` also refreshes `derived.focus_ring` and the kit `primary` and `ring` tokens
     through `map::apply`, so buttons and focus rings preview too. The graph palette and status
     colours are not editable in the editor (APP-037 lists the 22 slots plus Accent) and keep the
     base theme's values; a "Random" theme (toast "Random theme generated.") randomizes the 23
     editable values only, with the constraint that `text_primary` on `list_bg` stays at or above
     4.5:1 (the generator re-rolls `text_primary` lightness until it does).
  5. `commit_preview` writes the def into `user` (replacing the theme with the same name, which
     must be a user theme; built-ins are refused upstream with "Copy a theme before editing it"),
     saves through UI-FND-042, activates it without crossfade, and clears `preview`.
  6. `cancel_preview` clears `preview` and bumps `style_version`; `active` was never changed, so
     the app returns to it in one frame. Closing the editor window by its X is a cancel.
  7. The editor's own mini preview pane (mini table, tab bar, graph) renders with
     `ThemeStore::current()` like everything else; it has no private theme copy.
  8. Theme Editor "Share" encodes the current preview def (not the saved one) as a `TL1.` code
     (APP-038) so an unsaved edit can be shared.
- **States**: no preview; previewing (the Theme submenu shows the previewed theme name with
  " (editing)" and the check on it; `ActivateTheme` while previewing is refused with the toast
  "Finish or cancel the theme editor first").
- **Data binding**: none persisted until commit.
- **Build**: `theme/preview.rs`; the editor view in `window/theme_editor.rs` (shell file) calls
  these methods only.
- **Performance**: one `TlTheme` clone (about 1 KB) per applied frame; `style_version` bumps make
  charts rebuild geometry each applied frame during a drag, which is the same cost as a theme
  switch frame and happens only while dragging in the editor.
- **Tests**: `preview::tests::views_read_preview_when_live`, `preview::tests::cancel_restores_active`,
  `preview::tests::commit_saves_and_activates`, `preview::tests::second_begin_cancels_first`,
  `preview::tests::random_keeps_contrast`.
- **Phase**: P3 (with the editor).
- **Decided here**: the whole contract, the "(editing)" marker, the refusal toast text, the random
  contrast rule, Share encodes the preview.

## 4. Text rendering rules

### UI-FND-050 Fonts: bundling, loading and fallback

- **Implements**: ADR-009 "Fonts", APP-043 (languages: the bundled face must cover every shipped
  language's Latin and Cyrillic; CJK comes from fallback), ADR-015 (sanitized text only).
- **Anatomy**: `assets/fonts/NotoSans-Regular.ttf`, `NotoSans-Medium.ttf`, `NotoSans-SemiBold.ttf`
  (static instances; the variable font is not used because of zed#64783). Loaded once in
  `TasklensUi::init` before the first window with `cx.text_system().add_fonts(vec![Cow::Borrowed(
  include_bytes!(..)), ..])`. Then `theme::map::apply` sets `font_family = "Noto Sans"`.
  Weights used: 400 (`regular`), 500 (`medium`), 600 (`semibold`); no bold 700 anywhere (a 700
  request would synthesize or fall back; the gallery test asserts the three weights resolve to the
  bundled faces and `FontWeight::BOLD` is not used in tl-ui: `grep` in CI).
  Monospace: `NotoSansMono-Regular.ttf` (static, SIL OFL) is bundled too, family
  `"Noto Sans Mono"`, for the Service Properties Logs tab (UI-SYS-036), the Strings tab and
  Environment values (R-118). The start-up check covers it (U-40); if it fails to load the kit's
  mono fallback chain applies (UI-FND-002). No layout depends on the mono font's metrics.
  Fallback: non-Latin process names, window titles and command lines fall back through
  cosmic-text's fontconfig lookup to the system's Noto CJK or whatever is installed. A missing
  glyph shows as the font's `.notdef` box; tasklens does not substitute `U+FFFD` for unknown glyphs
  (only for control characters, UI-FND-053).
  Verification at start (ADR-009, zed#61660): after `add_fonts`, `text_system().font_id(&font("Noto
  Sans"))` must succeed for each weight; on failure tasklens logs `error!` and continues with the
  system UI font (`.SystemUIFont`), and the About window shows "Bundled font failed to load" under
  the version so the user can report it.
- **Build**: `crates/tl-ui/src/text/fonts.rs`.
- **Tests**: `fonts::tests::three_weights_load` (headless `TestAppContext` with the asset source),
  `fonts::tests::no_bold_in_tl_ui` (source scan).
- **Phase**: P1.
- **Decided here**: no 700 weight, Noto Sans Mono bundled (R-118), start-up check behaviour and
  About note.

### UI-FND-051 Tabular figures and numeric text

- **Implements**: ADR-009 "Numbers use the tabular-figures font feature and right alignment",
  table (d), PROC-023 template widths, R-081 (no locale forms).
- **Rules**:
  1. Every numeric text run (table cells, header totals, sidebar readouts, stats values, badge
     counts, slider values, stepper inputs, chart axis labels, tab counts "[17]", PID "+N" text,
     status bar counts, toast counts "(×2)") uses `TextStyle { font_features:
     FontFeatures(Arc::new(vec![("tnum".into(), 1)])), .. }`. The `Arc` is a `static` built once in
     `text/style.rs::TNUM`, never allocated per cell.
  2. Numeric columns are right-aligned; the sort arrow and header name share the right edge. The
     thin space U+2009 used for digit grouping is in Noto Sans and is itself tabular-width in
     `tnum` mode (checked in the gallery golden).
  3. Minimum numeric column widths come from the PROC-023 template strings measured with
     `text_system().layout_line` in the table font at the current scale with `tnum` on; the
     results are cached in `TextMetricsCache` keyed by `(template, font size bits, scale bits)` and
     recomputed when `style_version` changes. Unit suffixes ("MiB/s") are part of the same run as
     the number; there is no separate unit column.
  4. Percent, size and rate texts never change length within their template (table d fixed
     forms), so a value change never shifts neighbours. Where a value exceeds its template (a
     1.5 TiB RSS, a 10 GiB/s rate) the text is still drawn and clipped at the cell's right edge
     with no ellipsis (numbers are never ellipsized; the tooltip shows the full text).
  5. Labels and names use proportional figures (no `tnum`): "Apps (11)", "Threads [17]" are labels
     and use `tnum` only on the number run when the count updates live (TlTabs and group rows
     split the run so the label does not jiggle).
- **Build**: `text/style.rs` (`TNUM`, `numeric_style(m)`, `label_style(m)`), `text/metrics.rs`
  (`TextMetricsCache`).
- **Tests**: `style::tests::tnum_static_is_shared` (pointer equality), `metrics::tests::template_
  widths_monotonic_in_scale`, gallery golden for a column of 0 to 9 digits aligning.
- **Phase**: P1.
- **Decided here**: clip-not-ellipsize for overflowing numbers, split runs for live counts.

### UI-FND-052 Truncation and ellipsis rules per element type

- **Implements**: ADR-009 (names use `SharedString::from(Arc<str>)` without copying), PROC-039
  (512-char tooltip cut), R-008 (command line read and display limits), PROC-023 (60 px text
  column minimum), APP-045 titles, kit `TabBar::max_width` behaviour.
- **Rules** (the ellipsis is always the single character U+2026; truncation is done by GPUI's
  `text_overflow(TextOverflow::Ellipsis)` on a `whitespace_nowrap` element unless a rule says
  "manual"):

| Element | Rule |
|---|---|
| Table text cell (Name, User, Description, Image path, Unit, Window title) | end ellipsis; the full text in the row tooltip (Name) or the cell tooltip (others, 500 ms, shown only when truncated: the cell compares its measured width to the available width in prepaint and sets a flag) |
| Table text cell holding a path (Image path, Command line, Working dir, Executable) | manual middle ellipsis (UI-FND-052a) when the column is narrower than the text; `CellTextCache` caches the truncated `SharedString` per `(NodeId, ColumnId, width bucket of 8 px)` |
| Group row "Apps (11)" | end ellipsis on the label run; the count run is never cut |
| Header column name | end ellipsis; full name in the header tooltip (which also carries D-004's "Shows RssAnon…" for Memory) |
| Header total value | never cut (numeric rule UI-FND-051) |
| Tab label (main strip, TlTabs) | end ellipsis at `max_width` 160; count suffix kept |
| Menu item label | end ellipsis at `max_w` 360 minus the shortcut column; full text in a tooltip only for painted theme items (user theme names can be long) |
| Tooltip lines | wrap at 360 px; the 12-line cut per UI-FND-019; no per-line ellipsis except the last visible line |
| Toast text | wrap, 3 lines, then end ellipsis; full text in its tooltip |
| Window title | end ellipsis |
| Key/value grid value | end ellipsis plus tooltip; paths get the middle ellipsis; the Properties "Cmd line" row wraps instead (PROC-078) with a 32 KiB cap already applied by `sanitize` |
| Sidebar card lines (device name, model) | end ellipsis; full text in the card tooltip (measured: `performance.png` cuts "AMD Ryzen 7 7800X3…" at the end) |
| Status bar text | end ellipsis on the left segment; the right (health) segment never truncates, it wins the space |
| Chart labels | never cut; the chart element only draws labels that fit (`ChartView` measures the `ShapedLine`) |
| Button labels | never cut; buttons grow, and a dialog is sized to its buttons |
| Badge text | never cut; badges grow to 120 px then end-ellipsize |
| Input text | scrolls horizontally inside the input (kit behaviour); no ellipsis |

- **UI-FND-052a Middle ellipsis for paths** (`text/ellipsis.rs::middle_ellipsis(text, max_px,
  measure) -> SharedString`): keep the last path component whole when it fits in at least 40% of
  the width, else keep its last 12 characters; fill the start from the beginning of the path up to
  the width minus the ellipsis width; cut at a character boundary (never inside a grapheme; use
  `unicode_segmentation` graphemes); the ellipsis replaces at least 2 characters (if only one
  would be removed, keep the text whole and let it clip). Measurement uses the same
  `TextMetricsCache` with a per-character advance cache for the table font, so one call is O(n)
  additions, no shaping per candidate. The result is cached per cell as above.
  Examples at a width that fits 30 characters: `/usr/lib/firefox/firefox-bin` stays whole;
  `/home/oystein/.local/share/Steam/steamapps/common/Game/bin/game.x86_64` becomes
  `/home/oystein/.local/s…/game.x86_64`.
- **Build**: `text/ellipsis.rs`, `text/metrics.rs`; the "truncated" flag for conditional
  tooltips is a `Cell<bool>` set in `prepaint` of `TlTextCell` (table file owns `TlTextCell`; this
  file owns the rule).
- **Tests**: `ellipsis::tests::keeps_last_component`, `ellipsis::tests::grapheme_boundary`
  (combining marks and an emoji flag not split), `ellipsis::tests::two_char_minimum`,
  render test: a 60 px Name column shows "…" and its tooltip holds the full name.
- **Phase**: P1.
- **Decided here**: the per-element table, the 40% and 12-character rules, the 8 px width bucket,
  conditional tooltips only when truncated.

### UI-FND-053 Bidi and control-character display (ADR-015)

- **Implements**: ADR-015 "Untrusted process text", ADR-003 (names from `/proc`), APP-043.
- **Rules**:
  1. All process-derived text (`comm`, name, exe, cmdline, user, cgroup, unit description,
     window caption, environment values, strings) is sanitized by `tl-collect::sanitize` before it
     reaches `ProcessMeta` (ADR-015): C0 and C1 controls become U+FFFD; bidi controls U+202A to
     U+202E and U+2066 to U+2069 and U+200B to U+200F are removed; invalid UTF-8 is replaced
     lossily. tl-ui trusts that and does not re-scan per frame. A debug assertion in
     `TlTextCell` (debug builds only) checks that no control character survives.
  2. U+FFFD is drawn as the font's replacement glyph in `status.warning` colour when it appears
     in a Name cell (the run is split around it), so a name that contained controls is visibly
     marked. In other cells and in tooltips it is drawn in the normal colour. A name consisting
     only of U+FFFD or empty after sanitizing shows `comm` instead, or `[pid N]` if `comm` is empty
     too (decided).
  3. Bidi: the bidi *control* characters are removed, but right-to-left letters (Hebrew, Arabic
     process names and window titles) are kept and shaped by cosmic-text's bidi algorithm as
     paragraphs in GPUI. Each table cell is its own paragraph with base direction LTR (the table is
     LTR), so an RTL name renders right-to-left inside a left-aligned cell and never reorders
     neighbouring cells, the count suffix or the ellipsis. The ellipsis is appended to the logical
     end of the string, so it appears at the left of an RTL run; this is accepted.
  4. Line separators (U+2028, U+2029) and the form feed are covered by rule 1's control class and
     by `sanitize`'s "lines are joined with spaces"; a cell is always a single line.
  5. Zero-width joiners inside emoji sequences (U+200D) are kept (not in the removed set) so
     emoji names render as one glyph; U+200B to U+200F are removed, which includes the zero-width
     space and the LRM/RLM marks.
  6. Tabs (U+0009) are in C0 and become U+FFFD; a command line's argument separators are already
     spaces (`sanitize` replaces NUL with a space), so TSV copy (ADR-015 "Copy rows as table") is
     unambiguous: a cell value can never contain a tab or a newline.
  7. Copy and export use the sanitized text as displayed (not the raw bytes), with one exception:
     "Copy all metrics" and Properties "Cmd line" copy the full 32 KiB sanitized command line, not
     the 512-char tooltip cut.
  8. Text direction of the UI itself is LTR in every shipped language (APP-043 lists no RTL
     language); no mirroring is implemented.
- **Build**: `text/untrusted.rs` (`split_fffd_runs`, debug check), used by the table's
  `TlTextCell` and `TlKeyValueGrid`.
- **Tests**: `untrusted::tests::fffd_run_split_and_coloured`, `untrusted::tests::empty_name_falls_
  back_to_comm_then_pid`, `untrusted::tests::rtl_name_in_ltr_cell` (render test: the ellipsis and
  the "+N" run stay at the right), plus the ADR-015 `sanitize` tests in tl-collect.
- **Phase**: P1.
- **Decided here**: U+FFFD highlighted in Name cells only, the name fallback chain, LTR paragraph
  per cell, the ellipsis at the logical end.

## 5. Summary table

| ID | Name | Implements | Phase |
|---|---|---|---|
| UI-FND-001 | Colour slots: Default Dark and Default Light | APP-035, R-068, R-059, ADR-009 theme model, PLAN 9 | P1 (P2 more themes) |
| UI-FND-002 | gpui-kit Theme mapping (map.rs) | ADR-009 map list, APP-035 U-8, R-077, R-020 | P1 |
| UI-FND-003 | Metrics: spacing, text, radii, rows, icons, motion, focus, shadows, scrollbars | ADR-009 metrics and fonts, R-020, R-077, R-017, APP-040, APP-058 | P1 |
| UI-FND-010 | TlButton | APP-002, APP-003, APP-004, APP-005, APP-045, PERF-014 | P1 |
| UI-FND-011 | TlCheckbox | APP-028, APP-040, PROC-024, APP-023, SYS-004 | P1 |
| UI-FND-012 | TlRadio, TlRadioGroup | APP-023, APP-037 | P2 |
| UI-FND-013 | TlSwitch | APP-023, D-003, APP-036 | P2 |
| UI-FND-014 | TlSlider | APP-040, APP-023 | P2 |
| UI-FND-015 | TlStepper | APP-023, PROC-066, PROC-067 | P2 |
| UI-FND-016 | TlTextInput, TlFilterBox | APP-028, PROC-077, APP-014, APP-037, PROC-024, R-043 | P1 |
| UI-FND-017 | TlSelect, TlCombo | APP-023, SYS-012, SYS-013 | P2 |
| UI-FND-018 | TlTabs (in-window) | PROC-077, SYS-011 to SYS-014, APP-042 | P2 |
| UI-FND-019 | TlTooltip | PROC-039, ADR-003, APP-003, D-004, PROC-061, PERF-013, R-009 | P1 |
| UI-FND-020 | TlToast queue rules | APP-044, PROC-058, R-035, ADR-011, ADR-015 | P1 |
| UI-FND-021 | TlDialog, TlSecondaryWindow | APP-045, APP-009, R-073, PROC-056, PROC-059, PROC-077, ADR-010, D-006 | P1 (P2 pin, properties) |
| UI-FND-022 | TlMenu and submenu | PROC-057, APP-012 to APP-026, APP-036, SYS-008, SYS-025, SYS-032, PROC-024, table (b) | P1 |
| UI-FND-023 | TlProgress, TlSpinner | APP-044, SYS-005, PROC-078, APP-042, PROC-083 | P1 (P2 rest) |
| UI-FND-024 | TlSkeleton | APP-059, R-060, PROC-021, SYS-016 | P1 (P2 use) |
| UI-FND-025 | TlKeyValueGrid | PERF-012, PERF-013, PROC-078, PERF-100 to PERF-104, SYS-011, SYS-026, SYS-034, PERF-011 | P1 (P2 links) |
| UI-FND-026 | TlValidityGlyph | ADR-003, R-050, PERF-013, PROC-021 | P1 |
| UI-FND-027 | TlBadge | SYS-003, SYS-020, SYS-029 | P2 |
| UI-FND-028 | TlShield | APP-003, APP-002, PROC-058, ADR-003 | P3 |
| UI-FND-029 | TlEmptyState | PROC-077, PROC-079, SYS-036, APP-059 | P1 |
| UI-FND-030 | Component gallery | ADR-013 | P1 |
| UI-FND-031 | TlBanner | SYS-015, PERF-105, APP-058 | P2 |
| UI-FND-032 | TlLink | PROC-078, SYS-011, PERF-061, PERF-013 | P2 |
| UI-FND-040 | TlTheme global and switch path | ADR-009, APP-035, APP-036, R-017, R-069 | P1 (P2 crossfade, follow system) |
| UI-FND-041 | Built-in theme loading | ADR-009, APP-035, R-068, ADR-011 | P1 (P2 12 themes) |
| UI-FND-042 | User themes file | ADR-011, APP-036, APP-038, R-018, R-079 | P2 (P3 import) |
| UI-FND-043 | Theme editor live-preview contract | APP-037, R-083 | P3 |
| UI-FND-050 | Fonts | ADR-009 fonts, APP-043 | P1 |
| UI-FND-051 | Tabular figures and numeric text | ADR-009, table (d), PROC-023, R-081 | P1 |
| UI-FND-052 | Truncation and ellipsis | ADR-009, PROC-039, R-008, PROC-023, APP-045 | P1 |
| UI-FND-053 | Bidi and control characters | ADR-015, ADR-003, APP-043 | P1 |

## 6. Coverage

Merged spec IDs, R and D entries and ADR sections in this file's scope, and the entries that
realize them. Entries owned by other files are named "(shell)", "(processes)", "(process
windows)", "(performance)", "(system tabs)" where this file supplies only a component.

| Scope item | Realized by |
|---|---|
| APP-035 Built-in themes and colour slots | UI-FND-001, UI-FND-002, UI-FND-040, UI-FND-041 |
| APP-036 Theme menu and follow system | UI-FND-040 (switch, follow system), UI-FND-042 (copy, delete), UI-FND-022 (painted items); menu itself (shell) |
| APP-037 Theme editor | UI-FND-043 (contract); editor UI (shell) |
| APP-038 Theme share and import codes | UI-FND-042 (import and file format); Share window (shell) |
| APP-039 Graph accent colours | not a foundation item: the 17 named colours are constants in the performance file; UI-FND-001 supplies `accent` as the default |
| APP-040 Appearance sliders | UI-FND-003 (metrics effects), UI-FND-014 (slider), UI-FND-011 (checks); menu (shell) |
| APP-044 Toasts | UI-FND-020 |
| APP-045 Dialogs as separate pinnable windows | UI-FND-021 |
| APP-009 Always on top and the pin, R-073, PROC-056 | UI-FND-021 (pin button); the KWin path (engine, ADR-004) |
| APP-058 Accessibility and reduced motion | UI-FND-003 (focus ring, motion rules), every component's keyboard path, UI-FND-023 and UI-FND-024 reduced-motion stand-ins; scale-from-font default (shell, P4) |
| APP-059 Loading placeholders, R-060, PROC-021 | UI-FND-024, UI-FND-026 (`…` cells) |
| APP-003 Shield | UI-FND-028 |
| APP-028 Filter box, R-043 | UI-FND-016 (component); placement and per-tab text (shell) |
| APP-023 Settings window | UI-FND-011 to UI-FND-017 (fields), UI-FND-018 (in-page tabs), UI-FND-021 (frame); pages (shell) |
| APP-002 Title bar toolbar buttons | UI-FND-010 (variants); layout (shell) |
| APP-008 Caption buttons | UI-FND-021 (pin look, kit controls) |
| PERF-012 Page layout stats grid, PERF-013 Fallback words, R-050 | UI-FND-025, UI-FND-026 |
| PERF-011 Copy device info | UI-FND-025 (section copy button, Ctrl+C routing); format (performance) |
| PROC-039 Row hover tooltip, R-009 | UI-FND-019 (component and 600 ms delay); line content (processes) |
| PROC-040 Heat colouring, R-059 | UI-FND-001 (`heat()` derived rule); cell use (processes) |
| PROC-057 Row context menu, R-033 | UI-FND-022 (component); items (processes) |
| PROC-058 Action toasts, R-035 | UI-FND-020 (shapes, replace, retry button); texts (processes) |
| PROC-059 Confirmations, R-036, D-009 | UI-FND-021 (TlDialog); texts and rules (processes) |
| PROC-077 Properties window frame | UI-FND-021 (frame, 8-window registry), UI-FND-018 (tabs), UI-FND-016 (filters), UI-FND-029 (empty); content (process windows) |
| PROC-078 General tab key/value | UI-FND-025 (grid, click-to-copy); fields (process windows) |
| PROC-023 Column widths from templates | UI-FND-051 (measurement cache); columns (processes) |
| PROC-024 Column picker | UI-FND-011 (indeterminate), UI-FND-016 (filter), UI-FND-022 (menu entry); picker (processes) |
| SYS-003, SYS-020, SYS-029 state columns | UI-FND-027 |
| SYS-011 to SYS-014, SYS-026, SYS-034 property windows | UI-FND-021, UI-FND-018, UI-FND-025 (frames and grids); content (system tabs) |
| SYS-016 loading state | UI-FND-024, UI-FND-020 |
| ADR-003 validity table | UI-FND-026 (glyphs), UI-FND-025 (words), UI-FND-019 (tooltips) |
| ADR-009 theme model, map.rs, metrics, fonts, icons, text caching rules for text | UI-FND-001, UI-FND-002, UI-FND-003, UI-FND-040, UI-FND-050, UI-FND-051, UI-FND-052 |
| ADR-010 decorations, window options | UI-FND-021 |
| ADR-011 themes.json path and atomic write, settings toasts | UI-FND-042, UI-FND-020 |
| ADR-013 render tests host | UI-FND-030 |
| ADR-014 per-frame rules for components | each entry's Performance line; UI-FND-023 budget |
| ADR-015 untrusted text, copy formats | UI-FND-053, UI-FND-026 (glyph copy text) |
| R-017 motion settings | UI-FND-003, UI-FND-040 |
| R-020 scale and row height | UI-FND-003 |
| R-068, R-069 theme phase and keys | UI-FND-040, UI-FND-041 |
| R-077 corner radius | UI-FND-003, UI-FND-002 |
| R-079 TS1 codes | UI-FND-042 (after U-17) |
| R-081 number forms | UI-FND-051 |
| R-083 window titles | UI-FND-021 |
| D-004 Memory header tooltip | UI-FND-019 (component), UI-FND-052 (header tooltip rule) |
| D-005 no TaskSlinger artwork | UI-FND-028 (own shield glyph), UI-FND-003 (Lucide only) |
| D-006 client-drawn title bar | UI-FND-021 |
| D-007 no serial numbers | UI-FND-025 (owner omits rows; the grid has no special case) |
| Brief shared vocabulary (colours, spacing, text, radii, row height, icons, motion) | UI-FND-001, UI-FND-003 |

No scope item is unmapped.

## 7. Decided here (all in one list)

1. Default Dark slots not in PLAN 9: `input` #202224, `input_focus` #25282B, `row_hover` #232629,
   `menu_bg` #2A2E31, `menu_hover` #104B68, `tab_hover` #27292B, `scroll_track` #1A1C1E,
   `scroll_thumb` #494F54, `text_selected_primary` #FFFFFF, `text_selected_secondary` #BFD9E6.
2. Every Default Light value (UI-FND-001 table).
3. Status colours for both themes; `status` as an optional theme key with mode defaults.
4. Graph palette additions: `mem_line`, `disk_fill`, `net_fill`, `grid`, and a 13th field
   `chart_bg`; light graph palette.
5. Derived tints: NEW and EXITED alpha 0.18; heat blend in sRGB; shadow alpha 0.35 dark and 0.18
   light; overlay 0.4 and 0.25; selection 0.35.
6. Kit mapping rows beyond ADR-009's list; ±8% lightness for hover and active; `scrollbar_mode`
   Scrolling by default and Always on tables.
7. Line height 1.3; icon sizes 12, 14, 16, 20; header name `t_sm` over total `t_base`.
8. Hover fills are not animated; `m_fast` only for check, radio, switch, tab underline.
9. Focus ring 2 px accent, 2 px outside, keyboard-only through `TlFocusRing`; rows use a 1 px
   inset border; inputs use the accent border.
10. Shadow geometry per surface; hairline = 1 physical px.
11. Sidebar card height `4 × row_height`; in-window tab strip 32.
12. TlButton heights 24 and 28, min width 72, toggle fill alpha 0.25, Segment look.
13. TlCheckbox 16 px custom box with an indeterminate state; TlRadio roving tab index; TlSwitch
    small only.
14. TlSlider geometry and keyboard steps; per-frame coalescing of scale drags.
15. TlStepper design (kit NumberInput not used).
16. TlTextInput states; filter box stays 280 wide while non-empty; 20 px clear hit area.
17. TlSelect: popup max 10 items, filter field at 12 or more items, type-ahead.
18. TlTabs: underline style, min cell 80, overflow menu.
19. TlTooltip: max width 360 for text and 560 for lines, 12-line cut, keyboard show,
    below-then-above placement.
20. Toast queue: stack grows downward, newest at the bottom; wait list 16; Error evicts oldest
    Info or Success; coalesce within 1 s with " (×N)"; action toasts 15 s; Progress 120 s then
    "Outcome unknown"; 3-line cut; secondary windows' margin 34 + 8.
21. Window kinds table (sizes, modality, Esc; Run fixed 480 × 248); pin placement left of minimize; server-decoration
    pin fallback at the content's top-right; destructive confirmations focus Cancel; titles
    truncate at the end.
22. TlMenu: item height 26, check column 20 (collapsing), min 180 and max 360 wide, header rows
    22 high, submenu overlap 4 px, "No items" row, `painted_item` API.
23. TlProgress 4 and 6 px, 1% buckets, reduced-motion stand-ins, 2-per-window budget.
24. TlSkeleton bar pattern, 12-row cap, no fade on replace.
25. TlKeyValueGrid: label column 180 and row 28 by default with `.row_height`, `.block_indent`,
    `.block_gap` and
    `.label_width` overrides (Properties General 26 and 126), pair height 2 rows, section band
    with 3 px accent bar, copy flash, value context menu.
26. TlValidityGlyph: lock right-aligned like a number; copy text per glyph.
27. TlBadge geometry, tint 0.2, status dot rule.
28. TlShield single-colour glyph, starting state, failure toast "Could not start the privileged
    helper".
29. TlEmptyState layout and "Clear filter" rule.
30. Component gallery as the golden-image host, `--gallery` flag.
30a. TlBanner: height 32, `status.info` at alpha 0.12, no close button. TlLink: `status.info`,
    underline on hover only, Tab stop.
31. Theme store: user theme shadows built-in with "(custom)"; animated switch rebuilds geometry
    twice; a second switch restarts from the current colours; mode from `window_bg` lightness.
32. Built-in JSON embedded with `include_str!`; upper-case slot keys, lower-case tasklens keys;
    `#RRGGBBAA` accepted; strict parse for built-ins, lenient repair for user themes; 64-theme cap;
    unknown keys round-trip; copy naming "<name> copy N"; delete of the active theme falls back by
    mode.
33. Preview contract: single preview, `current()` accessor, per-frame coalescing, "(editing)"
    marker, refusal toast "Finish or cancel the theme editor first", Random keeps 4.5:1, Share
    encodes the preview.
34. Fonts: no 700 weight; Noto Sans Mono bundled (R-118); load failure falls back to the system
    font with a note
    in About.
35. Numbers clip rather than ellipsize; live counts are separate `tnum` runs.
36. Truncation table per element; middle ellipsis keeps the last component (40% rule, 12-char
    floor, 2-char minimum removal); width bucket 8 px; tooltips only when truncated.
37. U+FFFD highlighted in Name cells only; name fallback `comm` then `[pid N]`; one LTR paragraph
    per cell.

## 8. Open

None. The five items this file carried are settled in `docs/RECONCILIATION.md`: toast enter
animation stays the kit's 400 ms (R-109); Default Light values ship as given and are checked
against the TaskSlinger export under U-18 (R-122); kit rem scaling and the `TlFocusRing`
heuristic are verified by the first component bead (R-123, U-41, U-42); `chart_bg` is the 13th
`GraphPalette` field in ARCHITECTURE ADR-009 (R-119).
