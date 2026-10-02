# GPUI and gpui-component API map for tasklens

**Bottom line:** depend on the one umbrella crate `gpui-kit = "=0.7.0"`. It pins GPUI as the crates.io snapshot `gpui-pre =0.3.7`, built from zed@1a28cff (published 2026-09-28). I downloaded the full GPUI source for that version and read it, so the signatures below are verified.

Four things decide the design. Each is covered in its section below.
- **Charts:** the built-in ones work for 40 live charts only with most features off. A custom sparkline element is the safer choice.
- **Table:** `DataTable` has single-row selection only, stores selection by row index, has no tree rows, and has keyboard quirks.
- **Wayland risk:** zed#64499 (open). The window can stop redrawing until the mouse moves, which would freeze timer-driven graphs.
- **X11 plus NVIDIA risk:** a startup panic on the X11 fallback with the proprietary driver (zed#52944, probably still present).

Paths use these short names:
- `GC/` is `/home/oystein/work/tasklens-ref/gpui-component/`.
- `G/` is the gpui-pre-0.3.7 `src`, unpacked at `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/crates/`. Sibling folders hold `gpui-pre-linux`, `gpui-pre-wgpu` and the other snapshot crates.

Anything marked [inferred] was not checked line by line.

## 1. Dependencies and toolchain

**Which crates**
- The GPUI pin is at `GC/Cargo.toml:62-77`: `gpui = { package = "gpui-pre", version = "=0.3.7" }`, plus `gpui-pre-platform`, `-macros`, `-sum-tree` and `-web`, all `=0.3.7`.
- The crates.io description reads "gpui-pre snapshot of zed@1a28cff". gpui-kit publishes these snapshots itself with `GC/script/bump-gpui.ts`, because Zed rarely publishes `gpui`.
- The GPUI source is on crates.io as `.crate` files. I fetched all 25 `gpui-pre-*` crates (`gpui-pre-reqwest` is a different version).
- On crates.io (all 2026-09-28): `gpui-kit`, `gpui-component`, `gpui-base` and `gpui-kit-assets`, each at 0.7.0.

**What the app's Cargo.toml needs**
- Only `gpui-kit = "=0.7.0"`. Its default features are `component` and `assets` (`GC/crates/kit/Cargo.toml`).
- gpui-kit carries `gpui_platform` with `["font-kit","x11","wayland","runtime_shaders"]`.
- On its own, `gpui-pre-platform` has **empty default features**. Without `wayland`/`x11` it panics with `unreachable!` (gpui-kit #2315). Going through gpui-kit avoids this.
- Imports: `use gpui_kit::*;` is GPUI. Components are `gpui_kit::component::...` and icons `gpui_kit::assets`.
- Pin every crate exactly. Mixing gpui-pre versions breaks type compatibility (gpui-kit #2532).

**Optional kit features**
- `profiler`: frame histograms and the `gpui-fps` HUD.
- `test-support`: test helpers.
- `inspector`: element inspector.
- `decimal`: decimal values in charts.

**Toolchain**
- Edition 2024. No `rust-version` is declared anywhere. Stable is fine (local rustc is 1.98.1). There is no `rust-toolchain` file, and CI uses the latest stable.
- Renderer on Linux: `gpui-pre-wgpu` on wgpu 29.0.4, Vulkan plus GL. Text is cosmic-text 0.19 with swash. font-kit only matters on macOS.

**Arch packages** (from `GC/script/install-linux.sh` and the flake's runtime list)

| Purpose | Packages |
|---|---|
| Build | `base-devel`, `clang`, `pkgconf` |
| Runtime | `wayland`, `libxkbcommon`, `libxkbcommon-x11`, `libxcb`, `fontconfig`, `vulkan-icd-loader`, `nvidia-utils` (Vulkan ICD) |
| Debugging (optional) | `vulkan-validation-layers` |

All runtime packages are already installed locally. The webview and speech features would add webkit2gtk and alsa; we do not need them. [inferred] GTK, atk and xdg-desktop-portal matter only for file dialogs and portal-based appearance.

## 2. Bootstrap and windows

**Start-up** (`GC/crates/kit/src/lib.rs:149-181`, `GC/examples/system_monitor/src/main.rs:601-634`)

```rust
gpui_kit::application()                    // -> gpui::Application
    .with_assets(gpui_kit::assets::Assets) // G/app.rs:202; also .with_quit_mode(QuitMode::{Default,LastWindowClosed,Explicit})
    .run(|cx: &mut App| {
        gpui_kit::init(cx);                // must come before any window
        cx.text_system().add_fonts(vec![Cow::Borrowed(INTER)]).unwrap(); // G/text_system.rs:295
        let (handle, view) = gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| Main::new(window, cx)))?;
    });
```

- `gpui_kit::open_window(options, &mut App, FnOnce(&mut Window,&mut App)->Entity<V>) -> Result<(AnyWindowHandle, Entity<V>)>` wraps the view in `base::Root`. Root hosts dialogs, toasts, tooltips and menus, adds the window border and sets rem size.
- Never construct Root yourself. Inside async code, call `open_window` within `cx.update`.
- The raw GPUI form is `App::open_window(options, build) -> Result<WindowHandle<V>>` (`G/app.rs:1350`).
- **Multiple windows:** call `open_window` again.

**`WindowOptions`** (`G/platform.rs:2174-2243`, defaults at :2345)

| Field | Notes |
|---|---|
| `window_bounds: Option<WindowBounds>` | `Windowed`, `Maximized` or `Fullscreen(Bounds)`. `WindowBounds::centered(size, cx)`. |
| `titlebar: Option<TitlebarOptions>` | `appears_transparent` has no effect on Linux. |
| `window_decorations: Option<WindowDecorations::{Server,Client}>` | Linux only. |
| `window_min_size: Option<Size<Pixels>>` | |
| `window_background: WindowBackgroundAppearance` | `Opaque`, `Transparent` or `Blurred`. |
| `app_id: Option<String>` | |
| `display_id` | |
| `kind: WindowKind` | `Normal`, `PopUp`, `AnchoredPopup`, `Floating`, `LayerShell`, `Dialog`. |
| `inactive_frame_interval` | Default about 30 fps while unfocused. |
| `focus`, `show`, `is_resizable`, `is_minimizable`, `is_movable`, `app_owns_titlebar_drag`, `icon` | `icon` is X11 only. |

**Custom title bar (CSD)**
- `TitleBar::window_options()` (`GC/crates/component/src/title_bar.rs:81`) does not ask for client-side decorations. You must write:
  `WindowOptions { window_decorations: Some(WindowDecorations::Client), ..TitleBar::window_options() }`
  This is the pattern in `GC/crates/story/src/lib.rs:146-152`.
- KDE supports server-side decorations. Using `Server` there is the lower-risk choice.
- Window methods:
  - `start_window_move()`, `start_window_resize(ResizeEdge)` and `show_window_menu(pt)` (`G/window.rs:2840`, :2402).
  - `window_decorations() -> Decorations::{Server, Client{tiling}}` (:2856) and `request_decorations(..)` (:2379).
  - `set_client_inset(Pixels)` (:2845).
  - `zoom_window()`, `minimize_window()`, `toggle_fullscreen()`, `set_window_title()`, `set_app_id()`.

**Saving and restoring window bounds**
- Read with `window.window_bounds() -> WindowBounds` (:2420) and watch with `cx.observe_window_bounds(window, |this, w, cx| ..)` (`G/app/context.rs:402`).
- Close hook: `on_window_should_close(cx, Fn(&mut Window,&mut App)->bool)` (:6658).
- Wayland never lets a client read or set its screen position. On X11 the origin is wrong (zed#64445) and the window manager ignores the position (#64447). **Save size and maximized state only.**

**Wayland specifics** (`gpui-pre-linux/src/linux/wayland/window.rs`)
- Fractional scale comes from `wp_fractional_scale_v1` (`preferred_scale/120`) plus `wp_viewporter`, per surface (:840, :1184).
- Frames are paced by `wl_surface.frame` callbacks, so the window draws at the compositor's rate: 144 Hz on a 144 Hz output (:924-1035).
- Present mode is `Mailbox` (:579). X11 uses `Fifo` and a timer at the first CRTC's refresh rate (`x11/client.rs:1961-1985`, :2166).
- Backend choice: Wayland when `WAYLAND_DISPLAY` is non-empty, otherwise X11 (`G/platform.rs:158-183`). Force XWayland with `env -u WAYLAND_DISPLAY`.

**Scale factor**
- `window.scale_factor() -> f32` (:2925). Paint calls take logical `Pixels`, and GPUI scales and pixel-snaps them (`pixel_snap*`, :3007).
- On X11 there is one scale per process, overridable with `GPUI_X11_SCALE_FACTOR` (`x11/client.rs:80`).
- Choose the GPU with `ZED_DEVICE_ID=0x<pci id>` (`gpui-pre-wgpu/src/wgpu_context.rs:292`).

## 3. Drawing primitives and custom Elements

**The `Element` trait** (`G/element.rs:53-123`)

```rust
type RequestLayoutState: 'static; type PrepaintState: 'static;
fn id(&self) -> Option<ElementId>;
fn source_location(&self) -> Option<&'static panic::Location<'static>>;
fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Self::RequestLayoutState);
fn prepaint(&mut self, id, inspector_id, bounds: Bounds<Pixels>, request_layout: &mut Self::RequestLayoutState, window, cx) -> Self::PrepaintState;
fn paint(&mut self, id, inspector_id, bounds, request_layout, prepaint: &mut Self::PrepaintState, window, cx);
```

The element also implements `IntoElement`.

**Layout helpers**
- `window.request_layout(Style, children, cx)` (`G/window.rs:5023`).
- `request_measured_layout(Style, Fn(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App)->Size<Pixels>)` (:5052).
- `insert_hitbox(bounds, HitboxBehavior) -> Hitbox` (:5109).
- State that lives across frames:
  - `with_element_state(&GlobalElementId, FnOnce(Option<S>,&mut Window)->(R,S))` (:4199)
  - `use_keyed_state` (:4156) and `use_state` (:4182)

**Quick custom drawing:** `canvas(prepaint: FnOnce(Bounds<Pixels>,&mut Window,&mut App)->T, paint: FnOnce(Bounds<Pixels>,T,&mut Window,&mut App))` (`G/elements/canvas.rs:10`).

**Paint calls** (paint phase only)

| Call | Signature and notes |
|---|---|
| `paint_quad` | `paint_quad(PaintQuad)` (:4502). Build with `fill(bounds, bg)`, `outline(bounds, color, BorderStyle)` or `quad(bounds, radii, bg, border_widths, border_color, style)` (:7601-7640). A border-only quad is split into 4 strips. |
| `paint_path` | `paint_path(Path<Pixels>, impl Into<Background>)` (:4573). |
| `PathBuilder` | `G/path_builder.rs:88-244`: `stroke(Pixels)`, `fill()`, `with_style`, `dash_array`, `move_to`, `line_to`, `curve_to`, `cubic_bezier_to`, `arc_to`, `add_polygon(&[Point], closed)`, `close`, `translate`, `scale`, `rotate`, `transform`, `build()->Result<Path<Pixels>>` (CPU tessellation with lyon). `Path::new`, `line_to` and `push_triangle` build raw triangles (`G/scene.rs:801-876`). |
| Shadows | `paint_drop_shadows(bounds, Corners, &[BoxShadow])` and `paint_inset_shadows(..)` (:4376, :4412). `BoxShadow{color, offset, blur_radius, spread_radius, inset}`. |
| Sprites and text | `paint_glyph(origin, FontId, GlyphId, font_size, Hsla)` (:4639), `paint_emoji`, `paint_svg(bounds, path, Option<&[u8]>, TransformationMatrix, Hsla, cx)` (:4801), `paint_image(bounds, image_bounds, Corners, Arc<RenderImage>, frame_index, grayscale)` (:4871), `paint_underline` and `paint_strikethrough`. |
| Text | `window.text_system().shape_line(SharedString, font_size, &[TextRun], Option<Pixels>) -> ShapedLine` (`G/text_system.rs:638`), then `ShapedLine::paint(origin, line_height, TextAlign, Option<Pixels>, window, cx)` (`G/text_system/line.rs:108`). Repeated shaping is cached per frame. |
| Clipping and layers | `with_content_mask(Option<ContentMask<Pixels>>, f)` (:3981), `paint_layer(bounds, f)` (:4351), `defer_draw(..)` (:4322), or `overflow_hidden()` on a div. |
| Images | `img(impl Into<ImageSource>)` (`G/elements/img.rs:201`). |

**Animation**
- `window.request_animation_frame()` (:2622) is `on_next_frame(|_, cx| cx.notify(current_view))`. It re-renders the current view on the next frame.
- `on_next_frame(FnOnce(&mut Window,&mut App))` (:2602).
- `Animation::new(Duration)` with `AnimationExt::with_animation` (`G/elements/animation.rs`). It respects `App::reduce_motion()`.

**Primitives the renderer has** (`G/scene.rs:222`): Shadow, Quad, Path, Underline, MonochromeSprite, SubpixelSprite, PolychromeSprite, Surface. Surface is macOS only (`paint_surface(CVPixelBuffer)`).

**No raw wgpu access and no custom shaders.** The app-facing API has no hook for them. `WgpuRenderer` and `WgpuContext` are internal to the platform crate. The only route would be forking `gpui-pre-wgpu` [inferred].

**Path cost on the GPU:** each paths batch is drawn into a full-window 4x MSAA intermediate texture, then composited back (`gpui-pre-wgpu/src/wgpu_renderer.rs:1538-1572`). Quads are cheap instanced draws. Prefer quads where you can.

**View caching**
- `Entity<T>::cached(StyleRefinement)` (`G/view.rs:242`) replays the previous frame's prepaint and paint until the entity is notified. It needs a definite size.
- Use it for every chart and panel that is not changing.

**Frame stats** (needs the `profiler` feature)
- `window.frame_duration_snapshot()` and `input_latency_snapshot()` (:3463-3469).
- `GC/crates/fps` provides `fps_monitor(window, cx)`.

## 4. gpui-component widgets

### DataTable (`GC/crates/component/src/table/`)

**Wiring**
- `cx.new(|cx| TableState::new(delegate, window, cx).col_selectable(false)...)`, then render `DataTable::new(&state).stripe(true).bordered(false).small()`.

**`TableDelegate`** (`delegate.rs`)
- Required: `columns_count`, `rows_count`, `column(ix,&App)->Column`, and `render_td(row, col, window, &mut Context<TableState<Self>>) -> impl IntoElement`.
- Optional: `perform_sort`, `render_th`, `render_tr`, `context_menu(row_ix, PopupMenu, ..) -> PopupMenu` (:101), `move_column`, `render_empty`, `loading`, `has_more`, `load_more`, `visible_rows_changed`, `cell_text`, `group_headers`.

**`Column`** (`column.rs`)
- `Column::new(key, name)` with `.width()`, `.min_width()`, `.max_width()`, `.sortable()`, `.sort(ColumnSort)`, `.text_right()`, `.fixed_left()`, `.resizable(bool)`, `.movable(bool)`.
- `ColumnFixed` has only `Left`.

**`TableState` methods**
- `delegate_mut()`, `refresh(cx)` (does not notify, and resets widths and sort from `column()`), `scroll_to_row`, `set_selected_row`, `selected_row`, `right_clicked_row`, `visible_range`.

**`TableEvent`** (`state.rs:80-127`)
- `SelectRow`, `DoubleClickedRow`, `SelectColumn`, `SelectCell`, `DoubleClickedCell`, `ColumnWidthsChanged(Vec<Pixels>)`, `MoveColumn`, `RightClickedRow(Option<usize>)`, `RightClickedCell`, `ClearSelection`.

**Virtualization**
- Rows use GPUI `uniform_list`, with a fixed height per size: xs 26, sm 30, md 32, lg 40 px.
- Each row holds its own horizontal `virtual_list` over the columns.
- Only visible cells render, so 2000 rows cost about the same as 50.
- But **every visible cell is rebuilt every frame**. Row 0 is built 2 extra times, there is a `format!` id per cell, and `DataTable` adds its state uncached (`data_table.rs:173`).
- The story measures about 414 ns per `render_td` (`data_table_story.rs:629-635`).
- Dragging the scrollbar is capped at 60 fps (`state.rs:2385`).

**Gaps that need a design decision**
1. **Selection:** single row only, and stored by index. The delegate must map PID to index after each sort or refresh. `set_selected_row` emits events and scrolls.
2. **Keys:** left and right switch the table into column mode even with `col_selectable(false)`. Home and End select columns, not rows.
3. **Sort:** starts only from a click on the sort icon. The cycle is Default, Descending, Ascending.
4. **Right-click:** does not select the row.
5. **Tree rows:** not supported. Flatten the tree in the delegate and draw the indent and chevron in `render_td`.
6. **Column visibility:** no toggle. Change `columns_count` and `column`, then `refresh` and `notify`. Keep widths in the delegate.

**Reference example:** system_monitor (`GC/examples/system_monitor/src/main.rs`)
- A 500 ms `cx.spawn` loop with `smol::Timer`.
- `this.update(cx, |this, cx| { collect; cx.notify() })` (:300-314).
- Data reaches the table through `process_table.update(cx, |t, cx| { t.delegate_mut().update_processes(&sys); cx.notify() })` (:351).
- Two `AreaChart`s are fed a **cloned** `Vec<MetricPoint>` on every render (:451-459), with `String` x labels.
- A TitleBar with a segmented TabBar, `Progress` bars in the status bar, and `Theme::change(ThemeMode::Dark, ..)`.

### Lists

| Option | Signature | Use it for |
|---|---|---|
| GPUI `uniform_list` | `uniform_list(id, count, Fn(Range<usize>,&mut Window,&mut App)->Vec<R>)` (`G/elements/uniform_list.rs:22`). `UniformListScrollHandle::scroll_to_item(ix, ScrollStrategy::{Top,Center,Bottom,Nearest})`. `with_decoration` can draw indent guides. | Fixed-height rows at the lowest cost. Best basis for a custom tree-table. |
| GPUI `list(ListState, FnMut(usize,..)->AnyElement)` | `ListState::new(count, ListAlignment, overdraw)` (`G/elements/list.rs`) | Rows of different heights. |
| `v_virtual_list` / `h_virtual_list` | `(view, id, Rc<Vec<Size<Pixels>>>, f)` (`GC/crates/base/src/virtual_list.rs:139`) | Varied but known sizes. |
| Component `List` / `ListDelegate` | | A searchable picker. Not for tables. |
| `Tree` / `TreeState` | `uniform_list` underneath | No columns, single selection, and it deep-clones items on every rebuild. Not for 2000 processes refreshed each second. |

### Other widgets (all imported from `gpui_kit::component::...`)

**TabBar and Tab** (`tab/`)
- `TabBar::new(id).segmented()` (also `pill`, `underline`, `outline`), `.selected_index(usize)`, `.on_click(cx.listener(|this, ix: &usize, w, cx| ..))`, `.child(Tab::new().label(..).icon(..))`, `.menu(true)` for overflow, `.prefix()`, `.suffix()`, Sizable.
- **No keyboard navigation**, so bind your own actions.

**TitleBar**
- `TitleBar::new().child(..).on_close_window(..)`, 34 px tall.
- Draws minimize, maximize and close only under CSD. Double-click maximizes and right-click opens the window menu.
- `WindowBorder` is applied by Root automatically: a 20 px shadow inset and 4 px resize edges on Linux.

**Sidebar**
- `Sidebar::new(id).collapsible(SidebarCollapsible::Icon).collapsed(bool).header(..).footer(..).child(SidebarGroup::new(label).child(SidebarMenu::new().child(SidebarMenuItem::new(label).icon(..).active(bool).on_click(..))))`.
- 255 px wide, 48 px collapsed, 200 ms animation.
- Note: `SidebarMenu::children()` replaces the list instead of adding to it.

**Menus**
- `PopupMenu` builder: `.menu(label, Box<dyn Action>)`, `.menu_with_check(label, checked, action)`, `.separator()`, `.submenu(label, w, cx, f)`, `.label()`, `.item(PopupMenuItem::new(..).on_click(..))`.
- Key binding hints come from the action automatically.
- Right-click on any element: `.context_menu(|menu, w, cx| menu...)` (`menu/context_menu.rs:23`).
- On a table, use `TableDelegate::context_menu` instead.
- Left-click dropdown on a Button: `.dropdown_menu(f)`.
- `NativeMenu` on Linux falls back to `PopupMenu`.
- For process actions, use data-carrying actions such as `#[derive(Action)] struct Kill { pid: u32 }`. These keep the shortcut hints.

**Popover**
- `Popover::new(id).trigger(T: Selectable).content(|state, w, cx| ..).anchor(..)`.
- Open state is either uncontrolled or controlled with `.open(bool).on_open_change(..)`.

**Select and Combobox**
- `SelectState::new(Vec<T> | SearchableVec<T>, Option<IndexPath>, w, cx)` with `Select::new(&state).placeholder(..)`. Emits `SelectEvent::Confirm(Option<Value>)`.
- `ComboboxState` adds multi-select and the `Change` and `Confirm` events.

**Input (the filter box)**
- `InputState::new(w, cx).placeholder(..).clean_on_escape()`, and `Input::new(&state).prefix(Icon::new(IconName::Search).small()).cleanable(true).small()`.
- `InputEvent::{Change, PressEnter{..}, Focus, Blur}`. Subscribe with `cx.subscribe_in`.
- Read the text with `text() -> &Rope`, which costs nothing. `value()` allocates.
- `set_value` does not emit `Change`.
- Debounce filtering [inferred].

**Slider:** `SliderState::new().min().max().step().default_value()`, emitting `SliderEvent::{Change, Release}`.

**Switch, Checkbox, Radio and RadioGroup:** RenderOnce with `.checked(bool).on_change(Fn(&bool,..))`. RadioGroup takes `.selected_index` and `.on_change(Fn(&usize,..))`.

**Button**
- `Button::new(id).label(..).icon(IconName::X).ghost()` (also `primary` and `danger`), `.small()`, `.tooltip(..)`, `.tooltip_with_action(..)`, `.on_click(..)`.
- Icon-only buttons are squares: 20, 24 or 32 px.
- Also `ButtonGroup`, `Toggle` and `ToggleGroup`.

**Tooltip**
- Kit widgets have `.tooltip(text)` with a 500 ms delay.
- On any element: `.id(..).tooltip(|w, cx| Tooltip::new("..").action(&A, ctx).build(w, cx))` plus `.tooltip_show_delay(Duration)` (`G/elements/div.rs:1676`, :1702).

**Notifications**
- `window.push_notification(Notification::success(msg).title(..).id1::<T>(key), cx)`.
- Auto-hide is fixed at 5 s. Reusing the same id replaces the toast in place.

**Dialogs**
- `window.open_dialog(cx, |d, w, cx| ..)` and `window.open_alert_dialog(cx, |a, _, _| a.title(..).description(..).confirm().ok_text("End process").ok_variant(ButtonVariant::Danger).on_ok(|_, w, cx| true))`.
- Also `window.open_sheet(..)`.

**Resizable panels**
- `h_resizable(id).with_state(&state).child(resizable_panel().size(px(..)).size_range(a..b)).on_resize(..)`.
- Panel minimum is 100 px. The resize event fires on mouse-up. Saving sizes is up to us.

**Small widgets:** `StatusBar` (`.left()`, `.right()`), `Progress` and `ProgressCircle` (`.value(0..100)`), `Badge`, `Tag`, `Kbd`, `Label`, `Separator`, `GroupBox`, `DescriptionList`, `Form`.
- `Spinner` and `Skeleton` request frames for as long as they are visible.

**Settings page:** `Settings` with `SettingPage`, `SettingGroup`, `SettingItem` and fields such as `SettingField::switch(get, set)`. It includes a sidebar of pages and search.

**Dock:** an IDE-style docking system. It is more than tasklens needs. Use TabBar or Sidebar and a `match` on the active view instead.

### Charts (`GC/crates/component/src/chart/`, `GC/crates/base/src/plot/`)

**What exists**
- AreaChart, LineChart, BarChart, CandlestickChart, PieChart, RadarChart, SankeyChart. Each is a `#[derive(IntoPlot)]` struct rendered by `PlotElement`.
- AreaChart builder: `AreaChart::new(data).x(..).y(..).stroke(..).fill(Background).linear()` (also `.natural()` and `.step_after()`), `.appear(bool)`, `.interactive(bool)`, `.x_axis(bool)`, `.grid(bool)`, `.y_domain(..)`, `.tick_margin(..)`.

**Per-frame cost**
- `new()` collects the data into a fresh `Vec` on every render.
- Scales are rebuilt 2 to 3 times per frame.
- Cached tessellated paths have their **vertex Vec cloned** on every paint (`path_cache.rs:542`).
- The axis labels are shaped again.
- Each moving mouse re-renders the owning view.
- Appear animations are on by default and last 1 s.

**Verdict for 40 charts at 144 Hz**
- Possible only with `.appear(false).interactive(false).x_axis(false).grid(false).linear()`, each chart in its own `Entity` embedded with `.cached(style)`, and an index-based data source in place of a cloned Vec.
- **Recommended instead:** a custom sparkline `Element` that holds a `Rc` ring buffer.
  - Build the path once per data tick with `PathBuilder::stroke(px(1.5))` and `line_to`.
  - Draw the area fill as per-sample `paint_quad` or one fill path.
  - No hitbox and no id unless a tooltip is needed.
  - Notify only on new data, at 1 to 4 Hz.
- The FPS HUD already draws this way (`GC/crates/fps/src/monitor.rs:549-593`).

## 5. Theming, fonts and icons

**The `Theme` global** (`GC/crates/component/src/theme/mod.rs:147-212`)
- Fields: `colors: ThemeColor` (139 `Hsla` tokens), `font_family` (default `.SystemUIFont`, which maps to IBM Plex Sans on Linux), `font_size` (16 px, which also sets rem size), `mono_font_family`, `radius` (6), `radius_lg` (8), `shadow`, `scrollbar_mode`, `motion`.
- Read it with `cx.theme()` (the `ActiveTheme` trait). It derefs to `ThemeColor`.
- Write it with `Theme::update(cx, |t| ..)`. Switch modes with `Theme::change(ThemeMode::Dark, Some(window), cx)`.

**Colour tokens**
- General: `background`, `foreground`, `muted_foreground`, `border`.
- Tables: `table`, `table_even`, `table_hover`, `table_active`, `table_head`, `table_row_border`.
- Charts: `chart_1` to `chart_5`, `chart_grid`.
- Bars: `title_bar`, `status_bar`.
- Base palette: `red`, `green`, `blue`, `yellow`, `magenta`, `cyan` and their `_light` forms.
- Plus button, sidebar, tab and status families.

**JSON themes** (`schema.rs`)
- Format: `ThemeSet { name, themes: [ThemeConfig { name, mode, "font.family", "font.size", radius, colors: { "background": "#RRGGBB[AA]", "chart.1": .., "table.row.border": .. } }] }`.
- 21 ready-made sets are in `GC/themes/`. The schema is `GC/.theme-schema.json`.
- Load from a string with `ThemeRegistry::global_mut(cx).load_themes_from_str(..)`, or hot-reload a folder with `ThemeRegistry::watch_dir(path, cx, f)`.
- Apply with `Theme::update(cx, |t| t.apply_config(&cfg))`.

**Fonts**
- Load with `cx.text_system().add_fonts(Vec<Cow<'static,[u8]>>)`. Use the family name stored inside the file; the bundled Inter is named "Inter Variable".
- Register every weight file you need. Variable-font axes cannot be set, and variable fonts render at regular weight (zed#64783).
- Tabular numbers: `.font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))` (`G/styled.rs:714`).
- **Faces without an `'m'` glyph are silently dropped** (`gpui-pre-wgpu/src/cosmic_text_system.rs:420`).

**Icons**
- `Icon::new(IconName::Cpu).small().text_color(..)`.
- `gpui_kit_assets::IconName` has 1,830 icons: Lucide plus 12 of the kit's own. The component crate's `IconName` is a legacy set of 101.
- SVGs are rasterized once per size into the atlas and then drawn as a sprite.
- Custom SVGs: write an `AssetSource` that tries your own icons first and then falls back to `Assets`.
- **The Material Symbols icon font works** [inferred]: `div().font_family("Material Symbols Rounded").child("\u{e322}")`. Ligature names should also work. Steady-state cost is about the same as SVG.

## 6. Async and executor

| API | Signature |
|---|---|
| App spawn | `cx.spawn(async move \|cx: &mut AsyncApp\| ..) -> Task<R>` (`G/app.rs:2039`) |
| Entity spawn | `Context<T>::spawn(async move \|this: WeakEntity<T>, cx: &mut AsyncApp\| ..)` (`G/app/context.rs:230`) |
| Window spawn | `spawn_in(window, ..)` (:670) |
| Background work | `cx.background_executor().spawn(fut: Send + 'static) -> Task<R>` (`G/executor.rs:103`) |
| Timer | `background_executor().timer(Duration) -> Task<()>` (:183). `smol::Timer::after` also works, as in system_monitor. |
| Main-thread executor | `ForegroundExecutor::spawn` and `spawn_when_idle` |

- A `Task` must be stored or `.detach()`ed. Dropping it cancels the work.
- Updating state from async code:
  - `WeakEntity::update(cx, |s, cx| ..) -> Result<R>` (`G/app/entity_map.rs:799`). Stop the loop when it returns `Err`.
  - `AsyncApp::update(|cx| ..) -> R`.
- Repaint with `cx.notify()` (context.rs:221) or `cx.notify(entity_id)`.

**Waking the UI from an outside thread.** `AsyncApp` and `Entity` are not `Send`. Use a channel from the sampler thread to a foreground task:

```rust
let (tx, rx) = smol::channel::bounded(4);
std::thread::spawn(move || loop { tx.send_blocking(sample()).ok(); });
cx.spawn(async move |this, cx| {
    while let Ok(s) = rx.recv().await {
        if this.update(cx, |v, cx| { v.apply(s); cx.notify() }).is_err() { break }
    }
}).detach();
```

The waker posts to the main thread through the calloop channel (`gpui-pre-linux/src/linux/dispatcher.rs:113`) [inferred that this is thread-safe; that is how the executor works].

## 7. Keyboard and focus

**Actions**
- Unit actions: `actions!(tasklens, [Quit, KillProcess])`. Use the kit's macro (`GC/crates/kit/src/lib.rs`), because GPUI's own macro does not resolve `gpui::Action` through the facade.
- Actions with data: `#[derive(Clone, PartialEq, serde::Deserialize, schemars::JsonSchema, Action)] #[action(namespace = tasklens)] struct Kill { pid: u32 }` (`G/action.rs:62`).

**Bindings**
- `cx.bind_keys([KeyBinding::new("ctrl-f", Focus, Some("ProcessTable"))])` (`G/keymap/binding.rs:33`).
- Global handler: `cx.on_action(|_: &Quit, cx| cx.quit())` (`G/app.rs:2367`).

**Focus**
- On elements: `.key_context("ProcessTable")`, `.track_focus(&focus_handle)`, `.on_action(cx.listener(|this, a: &Kill, w, cx| ..))`, `.on_key_down(..)`, `.tab_index(..)`, `.focus(|s| ..)` (`G/elements/div.rs:788-1267`).
- `cx.focus_handle()` (`G/app.rs:2789`). Call `handle.focus(window, cx)` or `window.focus(&handle, cx)`.
- Events: `cx.on_focus`, `on_blur`, `on_focus_in`, `on_focus_out` (context.rs:540-635).
- `window.focus_next()` and `focus_prev()`. Root handles Tab and Shift-Tab.

**Shortcut labels:** `window.bindings_for_action(..)`, and `Kbd::binding_for_action(..)` for display.

## 8. Testing

**Test macro and harness**
- Use `#[gpui_kit::test] fn t(cx: &mut TestAppContext)` with the kit's `test-support` feature.
- `TestAppContext` (`G/app/test_context.rs`) provides `update`, `add_window`, `open_window`, `simulate_window_resize`, `simulate_window_scale_factor_change`, `executor().advance_clock(..)` and `run_until_parked()`.

**Kit UI helpers** (`gpui_kit::test::TestWindowExt`, `GC/crates/kit/src/test.rs:25-50`)
- `window.render_frame(cx)`, `find(id).value()`, `.label()`, `.bounds()`, `.focused()`.
- Actions: `click`, `right_click`, `double_click`, `hover`, `scroll`, `drag`, `press("backspace")`, `input("text")`.
- Worked example: `GC/crates/kit/tests/ui.rs`.

**Rendering checks**
- Without a GPU: `window.painted_quads()` and `painted_underlines()` under `test-support` (`G/window.rs:2710-2722`).
- Screenshots on Linux, headless:
  - `window.render_to_image()` (:2704).
  - `HeadlessAppContext::with_platform(Arc::new(CosmicTextSystem::new(..)), assets, || gpui_platform::current_headless_renderer())` followed by `.capture_screenshot(window)` (`G/app/headless_app_context.rs:65,168`).
  - `current_headless_renderer` returns a `WgpuHeadlessRenderer` on Linux (`gpui-pre-platform/src/gpui_platform.rs:85-97`).
  - The kit's own `tests/rendering.rs` claims there is no headless renderer outside macOS. That comment is out of date. The Linux path exists but gpui-kit does not exercise it [inferred].

## 9. Known Linux, Wayland and NVIDIA issues for our setup

| Risk | Issue | Status | What to do |
|---|---|---|---|
| Window stops drawing until the mouse moves (KWin Wayland, demand-driven frame loop, which is in our snapshot) | zed#64499 | **open** | Soak-test early: 3 windows, about 1 Hz updates, 10 minutes with no input. A local patch to `schedule_frame` may be needed. |
| Startup panic on X11 with NVIDIA proprietary (wgpu GL/EGL `unwrap`) | zed#52944 | closed by stale bot, **not fixed** | `__NV_FORCE_ENABLE_X11_EGL_PLATFORM=1`, or patch the backends to Vulkan only. Prefer native Wayland. |
| X11 uses one scale and one refresh rate per process | #21851, #38791 | open | Keep all 3 monitors at 110% and 144 Hz. Set `GPUI_X11_SCALE_FACTOR=1.1` on X11. |
| Blurry fractional scale | #25195 fixed for KDE/GNOME (June 2025); #65032 open, only when `wl_compositor` is below v6 | | 110% on KWin should be sharp. |
| Window position cannot be restored | Wayland protocol; #64445 and #64447 on X11 | open | Save size and maximized state only. |
| Displays list is empty before the first window on Wayland | #46378 | open | Do not pick an output at startup. |
| Garbled atlas after suspend and resume | #58382 | open | Test suspend and resume on the NVIDIA box. |
| Device-loss recovery | #23288 | open | Best-effort only; offer a restart path. |
| Driver versions to avoid | 580.65 and 580.76 hang (#35948); 535 crashes in egl-wayland (#56383) | | Use driver 580.82.07 or newer. The machine has 615.71. |
| CPU use grows with refresh rate on heavy views | #63184 | open | The budget is 6.9 ms per frame. Cache views and do not request frames when nothing changed. |
| GNOME shows no window controls under CSD | gpui-kit #3035 | open | Use server-side decorations on KDE. |
| IME panic (modifiers before keymap) | #64660 | fixed 2026-09-29, **after** the snapshot | |
| Variable font always at regular weight; fonts without `'m'` dropped | #64783, #61660 | open | Bundle static weights. |

**Fixed before the snapshot:**
- Wayland FIFO stalls, fixed by Mailbox present mode (PR #57077).
- Redundant-commit flicker (#54214).
- 1 px edge gaps (#44528).

**Background:**
- Unfocused windows are throttled to about 30 fps, and to 60 fps under thermal pressure.
- No report shows GPUI stuck at 60 Hz on a 144 Hz Linux monitor.

The four subagent reports (table, widgets, charts and theming, issues) are folded in above. Raw issue dumps are in the scratchpad (`iss/`, `zed.txt`, `zed2.txt`, `gc.txt`). No repo files were changed.
