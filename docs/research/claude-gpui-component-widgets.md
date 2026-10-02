# gpui-component API reference for tasklens (HEAD 3467e64)

Four things will catch implementers out:

- **CSD on Linux:** `TitleBar::window_options()` does not ask for client-side decorations (CSD). Add `window_decorations: Some(WindowDecorations::Client)` yourself.
- **Tabs:** there is no keyboard navigation.
- **Toasts:** the auto-hide delay is fixed at 5 s.
- **NativeMenu on Linux:** it is not native there. It falls back to an ordinary `PopupMenu`.

`examples/system_monitor/src/main.rs` (634 lines) is already a small task manager built on this kit: TitleBar with a segmented TabBar, DataTable, AreaChart, Progress and a status bar, fed by sysinfo. It is the closest template.

Paths are under `gpui-component/crates/`; `base/` means `crates/base/src`, and anything else is `crates/component/src`. "GPUI" means the gpui-pre-0.3.7 source in the scratchpad. Imports are `gpui_kit::component::...`. Bring these traits into scope as needed: `Sizable`, `Disableable`, `Selectable`, `ButtonVariants`, `ActiveTheme`, `WindowExt` (skills/gpui-kit/references/conventions.md).

## 0. Bootstrap

- `gpui_kit::init(cx)` must run before `gpui_kit::open_window(opts, cx, |w,cx| cx.new(..)) -> Result<(AnyWindowHandle, Entity<V>)>`. Without it you get the panic "component window state is missing" (root.rs:473).
- `open_window` wraps the view in `base::Root` (kit/src/lib.rs:149-163). Do not render your own Root.
- The Root plugin does three things:
  - mounts the sheet, dialog, notification, tooltip and native-menu overlay layers (root.rs:461-494);
  - wraps the surface in `window_border()` (root.rs:450-458);
  - calls `window.set_rem_size(theme.font_size)` (root.rs:436).

## 1. Tabs: `tab::{Tab, TabBar, TabVariant}`

Both are RenderOnce. The parent owns the selected index. Animation state is keyed on the TabBar id (`"{id}-tab-prev"`, `-anim`, `-tab-bounds`; tab_bar.rs:202-217), so the id must be stable.

**`TabVariant`** has `Tab` (default), `Outline`, `Pill`, `Segmented` and `Underline` (tab/tab.rs:14-21). Segmented, Pill and Underline get a sliding indicator (tab_bar.rs:186-195). Heights (tab.rs:23-44):

| Size | Underline | Other variants |
|---|---|---|
| xs | 26 px | 20 px |
| sm | 30 px | 24 px |
| md | 36 px | 32 px |
| lg | 44 px | 36 px |

**`TabBar::new(id: impl Into<ElementId>)`** (tab_bar.rs:59):
- Variant: `.with_variant(TabVariant)`, `.pill()`, `.outline()`, `.segmented()`, `.underline()` (:80-108). Children inherit it.
- `.selected_index(usize)` (:157).
- `.on_click<F: Fn(&usize, &mut Window, &mut App) + 'static>(F)` (:171). When set, it overrides each Tab's own `on_click`. Typical use is `cx.listener(|this, ix: &usize, w, cx| ..)`.
- Content: `.child(impl Into<Tab>)` (:151), `.children(..)` (:145), `.prefix(..)` (:133), `.suffix(..)` (:139), `.last_empty_space(..)` (:163).
- `.max_width(impl Into<Pixels>)` (:118): labels truncate with an ellipsis.
- `.track_scroll(&ScrollHandle)` (:127): it does not scroll the selected tab into view.
- `.menu(bool)` (:110): adds an overflow button.
- Implements `Sizable` and `Styled` (:347-357).

**`Tab::new()`** (tab.rs:478):
- Content: `.label(..)` (:483), `.aria_label(..)`, `.icon(impl Into<Icon>)` (:499), `.prefix(..)` (:535), `.suffix(..)` (:541).
- `.disabled(bool)` (:547).
- `.on_click(Fn(&ClickEvent,&mut Window,&mut App))` (:553).
- `From<&'static str | String | SharedString | Icon | IconName>` (:420-448), so `TabBar::child("Processes")` works.
- Implements `Selectable`, `Sizable`, `Styled`, `StatefulInteractiveElement`.

**Overflow.** The strip uses `overflow_x_scroll()` with `lock_scroll_axis()` (:540-549). `.menu(true)` adds a ghost xsmall Button with the fixed id `"more"`. It opens a dropdown listing every tab with a check on the selected one (:554-580).

**Limits:**
- No roving focus, arrow keys, Home/End or Enter (base/tabs.rs:15-21 is a TODO). Tabs are not focusable, so the app must bind its own actions, for example Ctrl+Tab.
- A Tab stops left mouse-down propagation (base/tabs.rs:173). That is why tabs inside the TitleBar do not start a window drag.

## 2. TitleBar and WindowBorder (CSD)

### `gpui_kit::component::TitleBar`

Re-exported at lib.rs:118. RenderOnce, with internal drag state from `window.use_state` (title_bar.rs:326), so use one per window.

- `TITLE_BAR_HEIGHT = px(34.)` (:15). Linux left padding is 12 px (:19).
- `TitleBar::new()` (:50).
- `TitleBar::title_bar_options() -> TitlebarOptions` (:59): `appears_transparent: true`, traffic lights at (9, 9).
- `TitleBar::window_options() -> WindowOptions` (:81): sets only `titlebar` and `app_owns_titlebar_drag: true`.
- `.on_close_window(impl Fn(&ClickEvent,&mut Window,&mut App) + 'static)` (:95):
  - Linux only (`cfg!` at :99).
  - The default action is `window.remove_window()` (:237).
  - The handler receives `ClickEvent::default()` (:235), not the real click.
- Implements `ParentElement` and `Styled`.

**How CSD is negotiated.**
- GPUI's default `window_decorations` is `None` (platform.rs:2362). It is requested as `unwrap_or(Server)` (window.rs:1597).
- Wayland: a compositor with xdg-decoration (KDE, wlroots) gives server-side decorations. Without it (Mutter/GNOME) GPUI falls back to Client (gpui-linux wayland/window.rs:2094-2111). The compositor can switch modes later through a configure event (:1150-1166).
- X11: a Client request without a compositor falls back to Server (x11/window.rs:1907-1914).
- The required pattern, copied from story/src/lib.rs:146-152, 185-190:

```rust
WindowOptions {
    #[cfg(target_os = "linux")]
    window_decorations: Some(WindowDecorations::Client),
    window_background: WindowBackgroundAppearance::Opaque,
    ..TitleBar::window_options()
}
```

- To change at runtime: `window.request_decorations(WindowDecorations)` (window.rs:2379).
- Related types: `Decorations::{Server, Client{tiling: Tiling}}` and `Tiling{top,left,right,bottom}.is_tiled()` (platform.rs:586, 763-789).

**Window controls.**
- Drawn only when `window.window_decorations()` is `Decorations::Client{..}` (title_bar.rs:267-270), so there is never a duplicate set under server-side decorations.
- Minimize and maximize appear only if `window.window_controls().minimize` / `.maximize` is true, as reported by the compositor (:275-291). Close is always shown.
- Each button is 34 px wide. Hover uses `secondary_hover`; close uses `danger` (:167-192).
- They call `minimize_window()`, `zoom_window()` and close (:223-241).
- Always on the right. The GTK/KDE button layout is ignored, even though GPUI has a `WindowButtonLayout` type [inferred].

**Drag, double-click and right-click.**
- Left mouse-down sets `should_move`. The next mouse move calls `window.start_window_move()` (:350-370).
- Double-click calls `window.zoom_window()` on Linux (:344-346).
- Right-click under CSD calls `window.show_window_menu(pos)` (:381-396).
- Any child that does not stop mouse-down propagation acts as a drag area [inferred].

### `WindowBorder` (`window_border`, `window_paddings`)

Re-exported at lib.rs:120. Root applies it automatically, so you never add it yourself.

- `window_border() -> WindowBorder` (window_border.rs:25).
- `.shadow_size(Pixels)` (:55): default 20 px on Linux, 0 elsewhere (:11-14).
- `.resize_hit_size(Pixels)` (:63): default 4 px (:17).
- `window_paddings(&Window) -> Edges<Pixels>` (:88): 0 under server-side decorations; under CSD, the shadow inset per side, with tiled sides set to 0.

**Under CSD it:**
- calls `window.set_client_inset(shadow)` on every render (:155);
- draws a 1 px border and a two-layer shadow that is dimmed when the window is inactive (:158-260);
- uses radius 0 on purpose, because GPUI has no rounded content mask (:18-22).

**Resize edges.** The backdrop's `on_mouse_down` runs `resize_edge()` and then `window.start_window_resize(edge)` (:190-205). That call does nothing on a non-resizable window (window.rs:2402). Edge and corner cursors are set at :303-360. Tiled sides get no resize band.

## 3. Sidebar: `sidebar::{Sidebar, SidebarCollapsible, SidebarMenu, SidebarMenuItem, SidebarGroup, SidebarHeader, SidebarFooter, SidebarToggleButton, SidebarItem}`

All are RenderOnce. The app owns `collapsed` and the active item. Internal state is keyed by id (sidebar/mod.rs:392-399, 507).

**`Sidebar<E: SidebarItem>::new(id)`** (mod.rs:238). All children share one type, for example `Sidebar<SidebarGroup<SidebarMenu>>` (examples/sidebar/src/main.rs:88-121).
- `.side(Side)` (:254).
- `.collapsible(impl Into<SidebarCollapsible>)` (:264): `Icon` (default, collapses to 48 px), `Offcanvas`, or `None`. A bool also works (true means Icon).
- `.collapsed(bool)` (:270), `.header(..)` (:276), `.footer(..)` (:282), `.child(E)`, `.children(..)`.
- Width: 255 px by default (:27). Override with `.w(px(..))`; it must be in pixels for the width to animate. Collapsed width is 48 px. The animation takes 200 ms (:27-29).
- Any padding you set is cleared (:390).
- Content renders in a virtualized `list()` with a scrollbar (:392-470).
- The user cannot resize it. Put it inside `resizable_panel` for that [inferred].

**`SidebarMenu::new()`** (menu.rs:20): `.child(impl Into<SidebarMenuItem>)`. Watch out: `.children(..)` replaces the list instead of adding to it (:45-51).

**`SidebarMenuItem::new(label)`** (menu.rs:114):
- `.icon(..)` (:135).
- `.active(bool)` (:155): marks the active item.
- `.on_click(Fn(&ClickEvent,&mut Window,&mut App))` (:161).
- Submenus: `.children(..)` (:203), `.default_open(bool)` (:178, first render only), `.click_to_open(bool)`, `.click_to_toggle(bool)`, `.collapsed(bool)`.
- `.suffix(Fn(&mut Window,&mut App)->E)` (:209).
- `.disable(bool)` (:221). The name is `disable`, not `disabled`.
- `.context_menu(Fn(PopupMenu,..)->PopupMenu)` (:235).
- Row is `h_7` (28 px) with `p_2` and `text_sm` (:300-325).
- In icon-collapsed mode the label becomes a tooltip (:393-395).

**Supporting parts:**
- `SidebarGroup::new(label)` (group.rs:9).
- `SidebarHeader::new()` and `SidebarFooter::new()` implement ParentElement and DropdownMenu.
- `SidebarToggleButton::new().side(..).collapsed(bool).on_click(..)` (mod.rs:302-351).

## 4. Menus: `menu::{PopupMenu, PopupMenuItem, ContextMenuExt, DropdownMenu}`

### `PopupMenu`

An entity that emits `DismissEvent` (popup_menu.rs:1411). Create it with `PopupMenu::build(window, cx, |menu, window, cx| menu...) -> Entity<Self>` (:358). Builder methods:

**Action items:**
- `.menu(label, Box<dyn Action>)` (:465)
- `.menu_with_disabled(..)` (:481), `.menu_with_icon(..)` (:543)

**Checkbox items:**
- `.menu_with_check(label, checked: bool, action)` (:565)
- `.menu_with_check_and_disabled(label, checked, action, disabled)` (:575)
- The check appears on the left, in place of the icon, by default. Change it with `.check_side(Side)` (:453).

**Custom rows:** `.menu_element(action, Fn(&mut Window,&mut App)->E)` (:587).

**Structure:**
- `.label(label)` (:492): a header row.
- `.separator()` (:680): drops a leading separator and collapses doubles.
- `.submenu(label, window, cx, Fn(PopupMenu,&mut Window,&mut Context<PopupMenu>)->PopupMenu)` (:694).
- `.submenu_with_icon(Option<Icon>, ..)` (:705).
- `.item(impl Into<PopupMenuItem>)` (:728).

**Size and dispatch:**
- `.min_w(..)` (default 120 px), `.max_w(..)`, `.max_h(..)`, `.scrollable(bool)` (:429-447).
- `.action_context(FocusHandle)` (:371): actions are dispatched to this handle.
- `.rebuild(window, cx, f)` (:753): replaces the items, for content that loads asynchronously.

**`PopupMenuItem`:**
- Constructors: `::new(label)`, `::separator()`, `::label(..)`, `::submenu(label, Entity<PopupMenu>)`.
- Modifiers: `.icon(..)`, `.action(Box<dyn Action>)`, `.disabled(bool)`, `.checked(bool)` (:71-196).
- `.on_click(Fn(&ClickEvent,&mut Window,&mut App))` (:196). When set, it runs instead of the action (:867-871).

**Keybinding hints are automatic.** For an item with an action, the menu looks up a binding through these focus handles in order: `action_context`, the trigger's focus, the previous focus, then the menu itself. If none match it falls back to `Kbd::global_binding_for_action` (:1119-1150). An item with only `on_click` shows no hint [inferred].

### Right-click menu

`ContextMenuExt::context_menu(self, Fn(PopupMenu,&mut Window,&mut Context<PopupMenu>)->PopupMenu + 'static) -> ContextMenu<Self>` works on any `InteractiveElement + ParentElement + Styled` (context_menu.rs:17-43).
- It builds a new menu on each right mouse-down, deferred one frame, opened at the cursor (:396-462).
- State is keyed by the element's `.id()`, or else by call site (:30-37).

**For table rows, use the table's hook instead of a context menu per row.** `TableDelegate::context_menu(&mut self, row_ix: usize, menu: PopupMenu, window, cx: &mut Context<TableState<Self>>) -> PopupMenu` (table/delegate.rs:101).
- One handler sits on the whole table (table/state.rs:2477-2486).
- `TableState::right_clicked_row() -> Option<usize>` (:527).
- The right-clicked row is highlighted (:2035).
- `TableEvent` has `SelectRow` and `DoubleClickedRow`, but no right-click event (:80-100).
- For a PID, use a data-carrying action such as `#[derive(Action)] struct Kill{pid:u32}`, which keeps the shortcut hints. `on_click` closures also work but lose the hints.

### DropdownMenu (left-click on a trigger)

- Trait methods: `.dropdown_menu(f) -> DropdownMenuPopover<Self>` and `.dropdown_menu_with_anchor(anchor, f)` (dropdown_menu.rs:14, 22).
- Implemented only for `Button`, `InputGroupButton`, `SidebarHeader` and `SidebarFooter`.
- The trigger needs an id. Without one the id defaults to 0 and collides (:27).
- `DropdownMenuPopover` has `.on_open_change(Fn(&bool,..))` (:64-79).
- `DropdownButton::new(id).button(Button).dropdown_menu(f)` (button/dropdown_button.rs:43-91).

### NativeMenu (`native_menu::NativeMenu`)

- Builder: `::new()`, `.menu(label, action)`, `.menu_with_check(..)`, `.separator()`, `.submenu(label, NativeMenu)`, `.show(Point<Pixels>, window, cx)` (:78-201).
- Real OS menus only on macOS and Windows (native_menu/mod.rs:39-42, 206-219).
- Linux uses `fallback::show`, a PopupMenu overlay in Root that is clipped to the window (fallback.rs:1-5).
- Actions only, no closures. On Linux it offers nothing over PopupMenu. It matters only because `Input::context_menu` takes it.

## 5. Popover: `popover::{Popover, PopoverState}`

RenderOnce. State is internal, created with `window.use_keyed_state(id)` (base/popover.rs:303).

- `Popover::new(id)` (popover.rs:136).
- Position: `.anchor(impl Into<Anchor>)` (:162; default TopLeft, which clamps and does not flip), `.offset(..)` (:169), `.arrow(bool)`.
- `.mouse_button(MouseButton)` (:183): left by default.
- `.trigger<T: Selectable + IntoElement>(T)` (:189). The trigger must implement Selectable, so a plain div cannot be one.
- Open state: `.default_open(bool)`, or controlled with `.open(bool)` plus `.on_open_change(Fn(&bool,..))` (:205-225).
- `.trigger_style(StyleRefinement)` (:238).
- `.overlay_closable(bool)` (:244): true by default.
- `.content(Fn(&mut PopoverState,&mut Window,&mut Context<PopoverState>)->E)` (:249). It runs on every render, so do not create entities inside it.
- `.appearance(bool)` (:270).
- `.track_focus(&FocusHandle)` (:279).
- `PopoverState` methods: `is_open`, `dismiss(window, cx)`, `show`, `set_open`, `toggle_open` (base/popover.rs:57-85).
- Opens with a 150 ms fade and an 8 px slide. There is no exit animation.

## 6. Select and Combobox

### `select::{Select, SelectState, SelectEvent, SelectItem, SearchableVec, SelectGroup}`

Stateful.

- **State:** `SelectState::new(delegate: D, selected_index: Option<IndexPath>, window, cx)` (select.rs:152). Keep it as `Entity<SelectState<D>>` on the view.
- **Delegate:** `D` can be `Vec<T>` or `SearchableVec<T>`, where T is a string type or a custom `SelectItem`. `SelectItem` has `type Value: Clone+PartialEq`, `fn title(&self)->SharedString` and `fn value(&self)->&Value` (searchable_list/delegate.rs:8).
- **State methods:**
  - `.searchable(bool)` (:303)
  - `set_selected_index(Option<IndexPath>, w, cx)` (:309), `set_selected_value(&Value, w, cx)` (:337), `set_items(D, w, cx)` (:357)
  - `selected_index(&App)` (:367), `selected_value()` (:372), `focus(w, cx)` (:377)
  - Build an index with `IndexPath::new(row)` (base/index_path.rs:39).
- **Element:** `Select::new(&state)`:
  - `.placeholder(..)`, `.icon(..)`, `.title_prefix(..)`
  - `.cleanable(bool)`, `.menu_width(..)`, `.menu_max_h(..)`
  - `.search_placeholder(..)`, `.empty(..)`, `.disabled(..)`, `.appearance(bool)`
  - `Sizable` (:659-741).
- **Events:** `SelectEvent::Confirm(Option<Value>)` is the only variant (:71-76). Subscribe with `cx.subscribe_in(&state, window, |this, _, ev: &SelectEvent<D>, w, cx| ..)`.

### `combobox::{Combobox, ComboboxState, ComboboxEvent}`

- **State:** `ComboboxState::new(delegate, selected_indices: Vec<IndexPath>, w, cx)` (combobox.rs:145).
- **State methods:**
  - `.multiple(bool)`, `.searchable(bool)`
  - `selected_values()`, `set_selected_values(..)`, `clear_selection(..)`
  - `query(&App)`, `set_query(..)` (:287-402)
- **Events:** `ComboboxEvent::Change(Vec<Value>)` fires on every toggle. `ComboboxEvent::Confirm(Vec<Value>)` fires when the popup closes (:129-137).
- **Element:** `Combobox::new(&state)` takes the same builder methods as Select, plus `.check_icon(..)`, `.render_trigger(..)` and `.footer(..)` (:781-857).

## 7. Input: `input::{Input, InputState, InputEvent}`

The element is RenderOnce over a retained `Entity<InputState>`.

**State.** Create once in the view constructor: `cx.new(|cx| InputState::new(window, cx))` (base/input/base/state.rs:9951).

**State builder methods:**
- `.placeholder(..)` (:799), `.default_value(..)` (:1226)
- `.clean_on_escape()` (:1131), `.submit_on_enter(bool)` (:1145)
- `.masked(bool)`, `.pattern(Regex)`, `.validate(Fn(&str,&mut App)->bool)` (:9975-10003)
- `.context_menu(bool)` (:782)

**Setters:**
- `set_value(v, w, cx)` (:933): does **not** emit Change, and resets undo history.
- `replace_all(..)` (:965): keeps undo history.
- `set_placeholder(..)` (:884), `set_disabled(bool, cx)` (:1082), `set_loading(..)`.
- `clean(w, cx)` (:2058): clears the text and does emit Change.

**Reading the value:**
- `text() -> &Rope` (:1272) is a zero-cost borrow.
- `value() -> SharedString` (:1242) allocates on every call.

**Focus:** `focus(&self, w, cx)` (:1300) and `focus_handle()` (:501).

**Events.** `InputEvent { Change, PressEnter{secondary: bool, shift: bool}, Focus, Blur }` (:123-128). Subscribe with `cx.subscribe_in(&input, window, |view, state, ev: &InputEvent, w, cx| ..)` and store the returned `Subscription` on the view.

**Element: `Input::new(&Entity<InputState>)`** (component input/input.rs:203):
- `.prefix(impl IntoElement)` (:265) and `.suffix(..)` (:270). A search icon is `.prefix(Icon::new(IconName::Search).small())`.
- `.cleanable(bool)` (:306): the clear button shows only for an editable, single-line input with text. Clicking it runs `clean()` and then `focus()`, so Change fires (:707-819).
- `.appearance(bool)`, `.bordered(bool)`, `.disabled(bool)`, `.tab_index(isize)`.
- `.context_menu(Fn(NativeMenu,..)->NativeMenu)` (:360).
- Implements `Sizable`. Heights are xs 20, sm 24, md 32, lg 44 px (sizing.rs:281-289).

**Cost per keystroke.** Each edit does a rope edit, an undo record, a search update, `emit(Change)` and `cx.notify()` on the InputState entity (state.rs:3805-3874). Visible lines are re-shaped in prepaint (base input element.rs:1921-1990). The parent re-renders only if your handler calls `notify` [inferred]. For tasklens, debounce the table filter on Change and read the text with `text()`, not `value()` [inferred].

## 8. Buttons, controls and sizing

### Size and `Sizable` (sizing.rs)

- `Size { Size(Pixels), XSmall, Small, Medium (default), Large }` (:6-13).
- `Sizable` provides `with_size(impl Into<Size>)`, `xsmall()`, `small()` and `large()` (:190-222). There is no `medium()`.

### `button::{Button, ButtonVariant, ButtonVariants, ButtonGroup, DropdownButton, Toggle, ToggleGroup}`

**`Button::new(id)`** (button.rs:241). RenderOnce.
- Content: `.label(..)` (:371) and `.icon(impl Into<ButtonIcon>)` (:397). ButtonIcon accepts an Icon, IconName, Spinner or ProgressCircle.
- Tooltip: `.tooltip(..)` (:403), `.tooltip_placement(Placement)` (:412), `.tooltip_with_action(text, &dyn Action, Option<&str>)` (:418).
- State: `.loading(bool)` (:435; a loading button is inert), `.selected(bool)`, `.disabled(bool)`, `.toggled(bool)` (:497; accessibility only).
- Look: `.compact()`, `.outline()`, `.rounded(..)`, `.dropdown_caret(bool)`.
- Focus: `.tab_stop(bool)`, `.tab_index(isize)`.
- `.on_click(Fn(&ClickEvent,&mut Window,&mut App))` (:447) and `.on_hover(Fn(&bool,..))` (:456).

**Variants.** `ButtonVariant` has Default, Primary, Secondary, Danger, Info, Success, Warning, Ghost, Link, Text and Custom (:146-158). Call them through the `ButtonVariants` trait: `.primary()`, `.ghost()`, `.danger()` and so on (:48-100).

**Sizes:**
- Icon-only (no label and no children): a square of xs 20, sm 24, md 32, lg 32 px (:659-665).
- With a label: xs 20, sm 24, md 32 and lg also 32 px high. Large only adds padding (:668-683).
- The cursor is the default arrow except for the link and text variants (:644-653).

**`ButtonGroup::new(id)`** (button_group.rs:44):
- `.child(Button)`, `.children(..)`, `.multiple(bool)`, `.layout(Axis)`, `.compact()`, `.outline()`.
- `.on_click(Fn(&Vec<usize>,&mut Window,&mut App))` (:123).
- Controlled: the caller sets `.selected()` on each child.

**`Toggle::new(id)`**: `.label(..)`, `.icon(..)`, `.checked(bool)`, `.on_click(Fn(&bool,..))` (toggle.rs:50-100). Variants are `.ghost()` and `.outline()`.

**`ToggleGroup::new(id)`**: `.child(..)`, `.segmented()`, `.on_click(Fn(&Vec<bool>,..))` (:235-275). It is multi-select only. For a single-select segmented control, use ButtonGroup without `multiple`, or RadioGroup [inferred].

### Switch, Checkbox, Radio, RadioGroup

All four are RenderOnce and controlled. `on_change` gives the requested value; the owner stores it and calls `notify`. `on_click` is an alias, and whichever is set last wins.

- **`switch::Switch::new(id)`**: `.checked(bool)`, `.label(..)`, `.tooltip(..)`, `.on_change(Fn(&bool,&mut Window,&mut App))` (switch.rs:35-92). Track size is sm 28×16, md 36×20, lg 44×24 px.
- **`checkbox::Checkbox::new(id)`**: `.label(..)`, `.checked(bool)`, `.tooltip(..)`, `.on_change(Fn(&bool,..))` (checkbox.rs:39-102). There is no indeterminate state.
- **`radio::Radio::new(id)`**: `.label(..)`, `.checked(bool)`, `.on_change(Fn(&bool,..))` (radio.rs:41-120).
- **`RadioGroup::new(id)`**, or `::horizontal(id)` / `::vertical(id)`:
  - `.selected_index(Option<usize>)` (:334), `.child(impl Into<Radio>)` (strings convert), `.layout(Axis)`.
  - `.on_change(Fn(&usize,..))` (:328).

### `slider::{Slider, SliderState, SliderEvent, SliderValue}`

Stateful.

- State: `cx.new(|_| SliderState::new().min(0.).max(100.).step(1.).default_value(50.))` (base/slider.rs:198-266).
- Logarithmic scale: `.scale(SliderScale::Logarithmic)`.
- `set_value(v, w, cx)` (:273) does not emit an event. Read with `value() -> SliderValue::{Single(f32), Range(f32,f32)}`.
- Events: `SliderEvent::Change(SliderValue)` fires on every drag move (:391). `SliderEvent::Release(SliderValue)` fires once at the end (:416).
- Element: `Slider::new(&state).horizontal()` or `.vertical()`, plus `.disabled(..)` and `.reverse()` (component slider.rs:96-137). It does not implement Sizable.

## 9. Tooltip

**On kit components.** Button, Checkbox, Radio, Switch and Toggle have `.tooltip(text)`. These use Root's managed overlay:
- 500 ms show delay (base/tooltip.rs:14).
- 300 ms grace period: moving to another trigger switches at once with a 200 ms slide (:13, :150-160).
- Hidden on mouse-down.
- The managed path is `pub(crate)`, so app code cannot use it on other elements.

**On any element.** Use GPUI's `.id(..).tooltip(|w, cx| Tooltip::new("End process").action(&Kill, Some("ProcTable")).build(w, cx))`.
- `tooltip::Tooltip` has `::new(impl Into<Text>)`, `::element(Fn(&mut Window,&mut App)->E)`, `.action(&dyn Action, Option<&str>)`, `.key_binding(Option<Kbd>)` and `.build(&mut Window,&mut App)->AnyView` (tooltip.rs:43-81).
- Evidence: story tooltip_story.rs:158-166.
- Delay: GPUI's default is 500 ms (div.rs:53). Override with `.tooltip_show_delay(Duration)` (div.rs:1702).
- This path has no grace-period switching [inferred].

## 10. Notifications (`notification::{Notification, NotificationType}`; `WindowExt`)

**Window API** (window_ext.rs:66-79):
- `window.push_notification(impl Into<Notification>, cx)`
- `remove_notification::<T>(cx)`, `remove_notification1::<T>(key, cx)`
- `clear_notifications(cx)`

**Building a notification:**
- Constructors: `Notification::info(msg)`, `::success(msg)`, `::warning(msg)`, `::error(msg)` (notification.rs:203-224). A plain `&str` also converts.
- Content: `.title(..)`, `.message(..)`, `.icon(..)`.
- Lifetime: `.autohide(bool)` (default true).
- `.on_click(..)`.
- `.on_close(Fn(&mut Window,&mut App))`: fires on every way of closing.
- `.action(Fn(&mut Self,&mut Window,&mut Context<Self>)->Button)`: forces autohide off (:347-353).
- `.content(..)`.
- `.id1::<T>(key)`: pushing again with the same id replaces the toast in place (:236-242). Use this for one status toast that updates.
- `.delivery(NotificationDelivery::{InApp, System, InAppAndSystem})`: system delivery is available.

**Types:** `NotificationType::{Info, Success, Warning, Error}` (:38).

**Auto-dismiss:**
- Fixed at 5 s (:872); only on or off is configurable.
- Pauses while the stack is hovered or focused.
- Enter takes 400 ms and exit 200 ms. A 50 ms tick runs only while a toast is animating or counting down (:724-764).

**Global settings.** `cx.theme().notification` (code-only, it is serde-skipped): placement TopRight, top margin 34+16 px, at most 10 items, width 382 px (:537-571).

**Limit.** The kit owns `cx.on_system_notification_response` (:574-581). The app must not register its own handler.

## 11. Dialog, AlertDialog and Sheet (`WindowExt`)

### Dialog

**Window API:**
- `window.open_dialog(cx, |dialog, w, cx| dialog...)`
- `window.open_alert_dialog(cx, |alert, w, cx| ...)`
- `has_active_dialog(cx)`, `close_dialog(cx)` (closes the topmost), `close_all_dialogs(cx)` (window_ext.rs:31-63).

**Behaviour.** Dialogs stack and focus returns to the previous handle on close (root.rs:235-273). The builder closure is called again on every render [inferred], so keep InputState and similar outside it.

**`dialog::Dialog::new(cx)`** (RenderOnce):
- Content: `.title(..)`, `.content(Fn(DialogContent,..)->DialogContent)`, `.footer(..)`.
- `.on_ok(Fn(&ClickEvent,&mut Window,&mut App)->bool)` and `.on_cancel(..)->bool`: return true to close (:416-433).
- `.on_close(..)`.
- Size: `.width(..)` (default 448 px), `.max_w(..)`.
- Close behaviour: `.close_button(bool)`, `.overlay(bool)`, `.overlay_closable(bool)`, `.keyboard(bool)` (Escape).
- A plain Dialog has no default buttons; you supply a footer.

**Custom footer:**
- `DialogFooter::new().child(DialogClose::new().child(btn)).child(DialogAction::new().child(btn))`.
- `DialogAction` dispatches Confirm, which runs `on_ok` (dialog/footer.rs:122-139).

### `AlertDialog::new(cx)`

- Defaults: not overlay-closable, no X button, shows an OK button. `.confirm()` adds Cancel (alert_dialog.rs:95).
- Text: `.title(..)`, `.description(..)`, `.ok_text(..)`, `.cancel_text(..)`.
- Look: `.ok_variant(ButtonVariant)`, `.icon(..)`.
- `.on_ok(..)`, `.on_cancel(..)`.
- Its doc comment mentions `.warning()` and `AlertVariant`, but neither exists. The doc is stale.

Confirm flow (story alert_dialog_story.rs:103-117):

```rust
window.open_alert_dialog(cx, |a, _, _| a
    .title("End process?")
    .description("...")
    .confirm()
    .ok_text("End")
    .ok_variant(ButtonVariant::Danger)
    .on_ok(|_, w, cx| { /* act */ true }));
```

### Sheet

- Open: `window.open_sheet(cx, |sheet, w, cx| ..)` (right side) or `open_sheet_at(Placement, cx, f)`. Only one sheet is open at a time.
- `sheet::Sheet`: `.title(..)`, `.footer(..)`, `.size(DefiniteLength)` (default 350 px), `.resizable(bool)` (default true), `.overlay(..)`, `.overlay_closable(..)`, `.on_close(..)` (sheet.rs:42-134).

## 12. Resizable: `{h_resizable, v_resizable, resizable_panel, ResizableState, ResizablePanelEvent}`

These re-export base types with kit styling (resizable.rs:18-36).

**Group: `h_resizable(id)` or `v_resizable(id)` returns a `ResizablePanelGroup`** (RenderOnce, base/resizable/panel.rs:31-137):
- `.child(impl Into<ResizablePanel>)`: any element or a nested group converts.
- `.with_state(&Entity<ResizableState>)`. Without it, state comes from `use_keyed_state(id)`.
- `.on_resize(Fn(&Entity<ResizableState>,&mut Window,&mut App))`: fires on mouse-up after a drag (:474-484), not continuously.

**Panel: `resizable_panel()`:**
- `.size(Pixels)`: the initial size only.
- `.size_range(Range<Pixels>)`: default minimum is 100 px (`PANEL_MIN_SIZE`, mod.rs:14).
- `.visible(bool)`.
- Give fixed side panels `.flex_none()`.

**`ResizableState`** emits `ResizablePanelEvent::Resized`:
- `sizes() -> &Vec<Pixels>`
- `resize_panel(ix, size, w, cx)`: clamps, then emits.
- `insert_panel(..)`, `remove_panel(..)`, `reset_panel(..)`, `container_size()`.

**Persistence is not built in.** Save `sizes()` from `on_resize`. Restore through `.size()`, clamped against the current window [inferred].

**Handle.** A 1 px line with 4 px of hit padding on each side and a resize cursor (resize_handle.rs:12-13).

## 13. Scroll: `scroll::{ScrollableElement, Scrollbar, ScrollbarMode, ScrollbarHandle}`

**Simple form.** On a `Div` or `Stateful`: `.overflow_y_scrollbar()`, `.overflow_x_scrollbar()` or `.overflow_scrollbar() -> Scrollable<Self>` (scrollable.rs:16-63).
- The scroll handle is internal and keyed by call site. Add `.id(..)` when one call site creates several.
- There is no way to scroll programmatically.
- It already releases the flex minimum size (:220-250).

**Programmatic form.** Use `.id(..).track_scroll(&handle).overflow_y_scroll().vertical_scrollbar(&handle)`. The overlay is absolutely positioned, so the container must be `relative` [inferred].

**Handles.** `ScrollbarHandle` is implemented for `ScrollHandle`, `UniformListScrollHandle`, `ListState` and `VirtualListScrollHandle` (base/scrollbar.rs:84-121).

**`Scrollbar`:**
- `Scrollbar::vertical(&h)`, `.mode(ScrollbarMode::{Scrolling, Hover, Always})`, `.max_fps(..)` (base/scrollbar.rs:815-935).
- Width 16 px.
- The global mode is `theme.scrollbar_mode`.

## 14. Small widgets (all RenderOnce)

| Widget | Constructor | Builders and notes |
|---|---|---|
| StatusBar | `status_bar::StatusBar::new()` | `.left(..)` and `.right(..)` can be called repeatedly; children go in the middle. Style is `py_1 px_2 border_t_1 text_xs muted_foreground` (status_bar.rs:32-110). |
| Progress | `progress::Progress::new(id)` | `.value(f32 0..100)`, `.loading(bool)` for indeterminate, `.color(..)`. Height xs 4, sm 6, md 8, lg 10 px. Each value change starts a transition [inferred cost] (progress.rs:14-124). |
| ProgressCircle | `progress::ProgressCircle::new(id)` | Same builders as Progress. Sizes run from 8 to 20 px (progress_circle.rs:17-175). |
| Badge | `badge::Badge::new()` | `.count(usize)` (hidden at 0), `.max(99)`, `.dot()`, `.icon(..)`. Drawn over its children (badge.rs:31-101). |
| Tag | `tag::Tag::new()`, `::primary()`, `::danger()`, `::success()`, `::warning()`, `::info()`, `::custom(..)`, `::color(ColorName)` | `.outline()`, `.rounded_full()`, Sizable (tag.rs:10-234). |
| Kbd | `kbd::Kbd::new(Keystroke)` | `Kbd::binding_for_action(&dyn Action, Option<&str>, &Window) -> Option<Kbd>`, `global_binding_for_action(..)`, `Kbd::format(&Keystroke)` (kbd.rs:31-105). |
| Label | `label::Label::new(..)` | `.secondary(..)`, `.masked(bool)`, `.highlights(..)`. It lowercases both strings on every render, which costs allocations in big lists [inferred] (label.rs:14-110). |
| Separator | `separator::Separator::horizontal()`, `::vertical()` and the `_dashed` versions | `.label(..)`, `.color(..)` (separator.rs:17-126). |
| Skeleton | `skeleton::Skeleton::new()` | `.secondary()`. A 2 s repeating animation keeps requesting frames while visible (skeleton.rs:37-55). |
| Spinner | `spinner::Spinner::new()` | `.icon(..)`, `.color(..)`. A 0.8 s repeating rotation keeps requesting frames (spinner.rs:10-78). |
| GroupBox | `group_box::GroupBox::new()` | `.title(..)`, `.footer(..)`, `.content_style(..)`. Variants `.normal()`, `.fill()`, `.outline()` (group_box.rs:11-141). |
| DescriptionList | `description_list::DescriptionList::new()`, `::horizontal()`, `::vertical()` | `.item(label, value, span)`, `.columns(1..=10)` (default 3), `.label_width(..)` (default 120 px), `.bordered(bool)` (description_list.rs:10-250). |
| Form | `form::{v_form(), h_form(), field()}` or `Form::new()` | `.columns(n)`, `.label_layout(Axis)`, `.footer(..)`. `Field` has `.label(..)`, `.description(..)`, `.required(bool)`, `.col_span(u16)`. Layout only, no validation (form/form.rs:14-119, form/field.rs:81-241). |

### Setting (`setting::{Settings, SettingPage, SettingGroup, SettingItem, SettingField}`)

A complete settings page. It draws a resizable sidebar of pages, a search box and the active page (settings.rs:34-398).

- `Settings::new(id)`: `.pages(..)`, `.sidebar_width(..)` (default 250 px, range 160 to 360 px).
- `SettingPage::new(title)`: `.icon(..)`, `.group(..)`.
- `SettingGroup::new()`: `.title(..)`, `.item(..)`.
- `SettingItem::new(title, field)`: `.description(..)`, `.keywords(..)` (feeds search).
- Field constructors take get and set closures over `App`, so the source of truth is a Global you own:
  - `SettingField::switch(Fn(&App)->bool, Fn(bool,&mut App))` and `::checkbox(..)`
  - `::input(..)`
  - `::dropdown(Vec<(value, label)>, get, set)`
  - `::number_input(NumberFieldOptions{min, max, step}, get, set)`
  - `::render(..)` for anything custom
  - (fields/mod.rs:138-309)

## 15. Dock (`dock::*`)

An IDE-style system: `DockArea::new(id, Some(version), w, cx)`, with splits, draggable tab groups, zoom, and `dump` / `load` of the layout (examples/dock). Every panel must be an Entity implementing the base `Panel` trait (an `EventEmitter<PanelEvent> + Render + Focusable`) plus the kit's presentation trait, and must be registered in a registry.

For tasklens's fixed views (Processes, Performance, Services) it is too much [inferred]. Use a TabBar or Sidebar plus a `match` on the active view, as system_monitor does at :588-596.

## 16. Rules from design-guides.md and coding-guides.md that bind tasklens

1. **No raw colors.** No hex, rgb or hsla in app UI. Read every color from `cx.theme()` by role: `background`, `muted_foreground`, `primary`, `danger`, `border`, and so on. Status colors only for their meaning, and never color alone.
2. **No direct `px(..)` in layout.** Use rem helpers (`p_2`, `gap_3`, `h_8`, `text_sm`) or component sizes. Pixels are allowed only for documented physical cases: a hairline, a window inset, measured geometry.
3. **Zoom is the theme font size.** `Theme::update(cx, |t| t.font_size = px(18.))` changes it. Anything cached from layout, such as row heights or virtual-list measurement, must be invalidated when `window.rem_size()` changes.
4. **Spacing scale:** 2, 4, 8, 12, 16, 24, 32 px (xxs to xxl), from `cx.theme().spacing_tokens()`. Parts of one control are 4 apart, related controls 8, one group 12, separate groups and panel padding 16, sections 24. Padding belongs to the component; the gap between components belongs to the parent. Do not double pad.
5. **Density.** Medium is the default. Use Small for toolbars, menus and tables, and apply density to a whole region, never to one control. Table rows are about 26, 30, 32 or 40 px. Use the `Size` API, never a custom height.
6. **Radius.** Use the theme's tiers only, with inner ≈ outer minus the gap. `radius_full` only for real circles and pills.
7. **Cursor.** The default arrow for buttons, tabs, checkboxes and menu items. The pointing hand only for real links. Resize and text cursors only during that manipulation.
8. **Button variants.**
   - `primary` is only for the default commit, the one Enter triggers. An "Add" button in a toolbar is not primary.
   - `ghost` is for quiet toolbar actions.
   - `danger` is for destructive commits.
   - `Link` is only for external URLs.
   - Every icon-only button needs an accessible name and, where its meaning is unclear, a tooltip.
9. **Selection** is shown with the item's own fill or weight. Never use a leading bar or a one-sided border. A dropdown trigger stays visibly pressed while its popup is open.
10. **One command, one Action.** The toolbar button, context menu, dropdown and key binding all dispatch the same Action. Call `cx.bind_keys` before building menus. Any essential command needs a non-context-menu path too. Do not use hover-only toolbars.
11. **Copy.** Sentence case. No final period on labels. Use "…" (the single character) on commands that open a dialog or need more input. Confirmation buttons name the result ("End process", not "Yes"), with Cancel beside them. Use an AlertDialog only for serious actions that cannot be undone; prefer undo or a toast for reversible ones.
12. **Layout.**
    - Use `items_stretch()` on an `h_flex` whose children are full-height columns.
    - Use `min_w_0()` / `min_h_0()` on flex children that are allowed to shrink.
    - Each scroll region has one owner, and its scrollbar sits on the region's trailing edge.
    - Give each major pane a minimum, a default and a rule for taking extra space. Save user splits and clamp them when restoring.
13. **State.**
    - RenderOnce for value-like elements. `Entity<T>` for retained behavior.
    - Create entities, subscriptions and focus handles once, never in render.
    - Keep Subscriptions on the owner.
    - Never mutate or call `notify` unconditionally in render.
    - Notify the narrowest entity.
14. **Data-heavy UI.**
    - Virtualize long collections (Table, VirtualList).
    - Use stable domain IDs, not indexes, for element ids.
    - Keep selection by domain identity (PID) through sort and filter.
    - Right-align numbers in tabular figures.
    - Do not clone the whole data set per render.
15. **Async.** Start work from events, not from render. Keep the Task handle for recurring work so dropping it cancels the work. Attach a revision and reject stale results. Model idle, loading, loaded and failed states explicitly.
16. **Focus and overlays.** Escape closes the topmost layer, and focus returns to the trigger. Attach `key_context` and `on_action` to the same focused region. A focus handle is a Tab stop only when built with `.tab_stop(true)`.
17. **Motion.** Motion is only for explaining change, and must honor reduced motion. Skeletons are only for real loading, never for switching between data that is already loaded.

## Performance notes for a perf-first tasklens [inferred]

- Spinner and Skeleton request frames continuously while visible, and every Progress value change starts a transition. Use them sparingly in per-row cells that update every second.
- Label's highlighting allocates on every render.
- Context menus and Popover content are rebuilt each time they open or render, which is fine. Dialog builders are re-run on every render, so keep them light.
- Tab and Sidebar keyed state needs stable ids.
