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

<!-- APPEND -->
