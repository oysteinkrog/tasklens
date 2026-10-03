# Shared brief: detailed UI spec (round 4)

tasklens is a native Linux task manager (Rust, GPUI via gpui-kit 0.7.0, asupersync engine) that
replicates every feature of TaskSlinger 0.9.2 and Mission Center 1.2.0. The owner said: "we need
very detailed UI plans, everything should be beaded and fully planned, I don't want implementation
agents to make any significant decisions". UI and rendering performance are the top priority. The
look, density and layout "more or less copy" TaskSlinger.

This round turns the merged feature specs into a UI spec precise enough that an implementer only
types code: every size, colour token, state, interaction, key, animation, empty and error state,
and the GPUI or gpui-kit building block for each piece.

## Read first

- `docs/DECISIONS.md`, `docs/RECONCILIATION.md` (canonical tables a to d), `docs/ARCHITECTURE.md`
  (ADR-009 UI architecture and ADR-010 window integration above all). Precedence: DECISIONS, then
  RECONCILIATION, then ARCHITECTURE, then the merged specs.
- The merged spec entries for your surface: `docs/spec/merged/proc.md`, `perf.md`, `sys-app.md`.
- `docs/research/claude-gpui-apis.md` and `docs/research/claude-gpui-component-widgets.md`: what
  GPUI and gpui-kit can do, which widgets are rejected and why.
- Visual references. Look at the images with the Read tool:
  - `docs/taskslinger-reference/processes.png`, `performance.png`, `information.png`
  - `/home/oystein/work/tasklens-ref/evidence-frames/*.png`, each with a `.txt` description
  - `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/ts/`
    (frames from the TaskSlinger advanced and customize videos)
  Measure from the images where you can (pixel sizes at the screenshot's scale), and say when a
  value is measured, when it is taken from a spec, and when you chose it.
- gpui-kit source, when you need an exact API: `/home/oystein/work/tasklens-ref/gpui-component/`
  (gpui-component, published as gpui-kit) and the extracted `gpui-pre-*-0.3.7` crates in
  `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/crates/`.
  Clone nothing.

## Shared vocabulary (use these names exactly; the foundation file defines their values)

- Colours: the `TlTheme` fields from ADR-009 (`window_bg`, `titlebar`, `toolbar`, `input`,
  `input_focus`, `header`, `list_bg`, `row_alt`, `row_hover`, `row_selected`, `divider`, `menu_bg`,
  `menu_hover`, `tab_bg`, `tab_hover`, `scroll_track`, `scroll_thumb`, `text_primary`,
  `text_secondary`, `text_selected_primary`, `text_selected_secondary`, `value_heat`, `accent`,
  and `graph.*`). Status colours: `status.success`, `status.warning`, `status.danger`,
  `status.info` (the foundation file defines them per theme).
- Spacing: `sp1` = 2, `sp2` = 4, `sp3` = 6, `sp4` = 8, `sp5` = 12, `sp6` = 16, `sp7` = 24,
  `sp8` = 32 logical px, all multiplied by `ui_scale_pct / 100`.
- Text: `t_xs` 11, `t_sm` 12, `t_base` 13, `t_md` 14, `t_lg` 16, `t_xl` 20 logical px, times the UI
  scale. Weights: regular 400, medium 500, semibold 600. Family "Noto Sans". Numbers use tabular
  figures (`tnum`).
- Radii: `r_sm` 4, `r_md` 6 logical px times the UI scale (R-077).
- Row height: 28 × row_height_pct/100 × ui_scale_pct/100 (R-020).
- Icons: Lucide names through `gpui_kit_assets::IconName`, or app SVG names under `assets/icons/`.
- Motion: durations `m_fast` 120 ms, `m_base` 200 ms, `m_slow` 300 ms; easing `ease_out_cubic`.
  Every animation lists what reduced motion does instead.

## What every UI entry contains

Use IDs with your prefix (UI-xxx-NNN). Each entry:
- **Implements**: merged spec IDs (and R or D entries) it realizes.
- **Anatomy**: an ASCII wireframe with sizes, and a part list: each part's size, padding, colour
  tokens, text style, icon, alignment.
- **States**: default, hover, pressed, focused (keyboard), selected, disabled, loading, empty,
  error, and each validity state where values show. Exact visuals for each.
- **Interaction**: mouse, keyboard (with the canonical shortcut table b), drag, context menu, focus
  order, what each action dispatches (the GPUI `Action` name), toasts and their exact text.
- **Data binding**: which snapshot component or setting key it reads, which `Command` or demand it
  sends, when it re-renders (which `ComponentMask` bits).
- **Build**: the GPUI element or gpui-kit widget to use, or "custom element" with its paint plan;
  the entity and file in `crates/tl-ui/src/`; what is cached.
- **Performance**: per-frame work, allocations to avoid, and any measure from ADR-014.
- **Accessibility**: focus ring, keyboard path, tooltips, text alternatives for charts.
- **Tests**: the `TestAppContext` or render test that proves it (name and what it asserts).
- **Phase**: P0 to P4 as in the merged specs.
- **Decided here**: any value you had to choose, so reviewers can check it.

If something is truly undecidable, give your recommended answer and list it at the end. Do not
leave it to the implementer.

## Output and commits (the terminal restarts often and kills long replies)

- Write your file directly with the Write tool, starting with the header and your first section,
  then add later sections with Edit (append) as you finish each one.
- After each section, commit it: `git -C /home/oystein/work/tasklens add <file>` then
  `git -C /home/oystein/work/tasklens commit -m "<message>" -- <file>`, ending the message with:
  `Co-Authored-By: Claude <your model name, for example Opus 5.5 or Fable 5.1> <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01Bmy1XwAdJ5bM8C4UG8Lb4g`
- Commit only your own file, always with the pathspec. Never switch branches, reset, rebase or
  amend. Other agents commit to the same branch at the same time.
- If the Write tool is blocked, send the content to "team-lead" with SendMessage in parts under
  10,000 characters labelled "PART n: <section>".
- End the file with: a summary table (ID, name, implements, phase), a coverage list mapping every
  merged spec ID in your scope to the UI entries that realize it (none left unmapped), and the
  "Decided here" and open lists.
- Final message, short: path, entry count, coverage gaps, open items.

Machine rules: do not run cass. Do not run builds.

Writing rules: plain, clear language; short sentences; no em dashes; no hype words; exact labels,
units and numbers.
