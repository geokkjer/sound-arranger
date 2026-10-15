# The iced widget inventory — the retired shell's real surface against iced 0.14

**Research, 2026-10-15.** The evidence behind acceptance criterion 3 of the
[iced evaluation](../../.agents/notes/proposed/architecture/2026-09-21-iced-shell-evaluation.md): *"a widget
inventory lists every surface the Vue shell has today (menus, dialogs, context menus, tooltips, pickers,
text inputs with IME, virtualised lists, drag-and-drop) against iced's core + third-party crates, with the
hand-rolled count stated explicitly."* The answer to the UI-plugin half of the evaluation is the note
[UI plugins in a compiled shell](../../.agents/notes/proposed/architecture/2026-10-15-ui-plugins-in-a-compiled-shell.md).

Everything below is read from source in this checkout or from the crates' published metadata, on
2026-10-15; where a claim is an inference rather than a reading, it says so.

## 1. What the retired Vue shell actually shipped

The premise of the criterion is a shell with a rich component surface. The shipped shell had a **small,
hand-written** surface, and its declared component library was never imported.

**Its whole front-end is 2,251 lines** (`crates/shell/src/*.vue` + `*.ts`, including the timeline maths
and its vitest suites) — four views, a bridge, a transport, the timeline maths, and a plugin registry.

**Imports across every `.vue` and `.ts` file** (verified by grepping the import specifiers):

| Import | Where |
|---|---|
| `vue` | the 4 views + App |
| `@tauri-apps/api/core` | `bridge.ts`, `transport.ts` (the IPC seam) |
| local modules (`./bridge`, `./editor`, `./transport`, `./timeline*`, `./plugins`) | views and App |
| **`reka-ui`** | **nowhere** |

`package.json` declares `reka-ui ^2.0.0` and `vue ^3.5.0`, with `tailwindcss ^4.0.0` in devDependencies.
`reka-ui` is **imported by no file**; Tailwind is used, but only as `@import "tailwindcss"` in
`style.css`. There is no shadcn-vue in the tree at all.

**The primitives it really used** (counted by tag/attribute across the `.vue` files):

| Primitive | Uses |
|---|---|
| `<button>` | 15 |
| `<input>` | 3 |
| `<textarea>` | 3 |
| `<canvas>` | 1 (the timeline) |
| `title=` (the native tooltip) | 18 |
| `aria-*` | 3 |
| `role=` | 2 |
| `v-for` (a list, unwindowed) | 3 |
| `<select>` · `<dialog>` · `draggable` · `@contextmenu` · `contenteditable` | **0 each** |

So the surfaces the criterion lists — menus, dialogs, context menus, pickers, virtualised lists,
drag-and-drop, IME inputs — were **not** surfaces the shipped shell had. They are surfaces the
[UI plan](../../docs/design/ui-plan.md) *named for the stack it chose* (§1 "shell/chrome primitives
… button, tooltip, menu, popover, combobox, scroll-area, tabs, divider"; §3 "headless base primitives →
`reka-ui` … Behavior + a11y only"), and the plan's stated reason was behavioural, not visual: *"You'd
otherwise re-implement focus traps and ARIA for nothing."*

That distinction is the inventory's most important finding: **the widget gap is a gap against a plan, and
the one thing the plan bought with reka-ui that a compiled shell cannot get back for free is the
accessibility tree and focus management**, not a pile of visual components.

## 2. iced 0.14's core inventory

`iced_widget 0.14.2` ships **41 top-level modules, of which 37 are widgets** (the others are the crate
root, `helpers`, the `Action` type and `themer`). The ones that matter here, all in core:

- **chrome**: `button`, `tooltip`, `rule`, `container`, `scrollable`, `stack`, `space`, `float`, `pin`,
  `mouse_area`, `sensor`, `responsive`, `themer`
- **text**: `text` (single run), `text/rich` (`Rich` over `Span`/`Fragment`/`Highlighter`), `markdown`,
  `text_input`, `text_editor` — **both text widgets implement `input_method()`**, so IME is wired in core
- **selection**: `checkbox`, `radio`, `toggler`, `pick_list`, `combo_box`, `slider`, `vertical_slider`
- **collections / layout**: `column`, `row`, `grid`, `keyed`, `lazy`, `table`, `pane_grid`, `overlay`
- **media & drawing**: `canvas`, `image`, `svg`, `shader`, `qr_code`, `progress_bar`
- **platform**: `iced::clipboard` (a real module), window settings and multiple windows via `iced_winit`

Two of those matter more than the rest for this product: `canvas` (the timeline, the meters, the knobs —
all the audio-native drawing) and `pane_grid` (the Surface slot model is a pane grid with fixed splits).

**Not in core**: a modal/dialog, a menu bar or context menu, tabs, a virtualised list, cross-widget
drag-and-drop, a toast, a file dialog, and an accessibility tree.

On the last: **no crate in the stack mentions accesskit** — a grep for `accesskit` across
`iced`, `iced_winit`, `iced_runtime` and `iced_core` returns **zero files**. The webview shell inherited
the DOM's accessibility; the iced shell has none, and nothing in core provides one.

`lazy` is worth naming precisely, because it is not virtualisation: its doc says *"a widget that only
rebuilds its contents when necessary"* — a **caching** mechanism keyed on a `Dependency`, not a windowed
list. A pool of thousands of sources needs hand-rolled windowing (or a `table` + `lazy` combination) on
top.

## 3. Third-party crates

**[`iced_aw` 0.14.1](https://docs.rs/iced_aw/0.14.1/iced_aw/)** (MIT; crates.io 0.14.1 published
2026-04-27) is **version-aligned with this shell's iced**: it depends on `iced_core ^0.14.0` and
`iced_widget ^0.14.2`, and its own dev-dependency is `iced ^0.14.0` with the `advanced` feature. Its
widgets, read from the crate's `widget` module page: `badge`, `card`, `color_picker`, `context_menu`,
`custom_layout`, `date_picker`, `drop_down`, `labeled_frame`, `menu` (`MenuBar`, the `menu!`/`menu_bar!`
macros), `number_input`, `overlay` (a layer that puts interactive elements on top of others), `quad`,
`selection_list` (`List`/`SelectionList`), `sidebar`, `slide_bar`, `spinner`, `tab_bar`, `tabs`,
`time_picker`, `typed_input`, `wrap`.

That closes most of what core lacks: the **menu bar, context menus, drop-downs/popovers, tabs, cards,
number inputs and selection lists**, plus an `overlay` layer to build a modal on. It is not an iced-org
crate (three maintainers, one of them the `icebreaker` author), and 0.14.1 has **no modal widget** (the
older `modal` is gone), **no toast** and **no virtualised list** — the focus trap, the toast queue and
the windowed pool list are ours.

**[`iced_audio` 0.17](https://github.com/iced-rs/iced_audio)** is already adopted for the console
(`VSlider`/`HSlider`/`Knob`/`Ramp`/`XYPad`/`ModRangeInput` on normalized parameters, with unit ranges
and tick marks), version-locked to the same `iced_core` 0.14. It has **no meter widget** — the iced
spike draws meters with core `progress_bar` and says in-code that a peak meter with hold would be styled
by hand.

**File dialogs** have no iced widget and are not part of the core; the standard answer is the `rfd`
crate (native dialogs). *Not verified in this inventory* — it was not added to the spike, and its iced
compatibility is untested here.

## 4. The surface-by-surface inventory

The left column is the product's actual surface (the plan's chrome primitives, the four shipped views,
and the timeline interactions the plan specifies). "Hand-rolled" means the shell writes it against
`canvas`/`mouse_area`/`overlay` rather than a widget crate.

| Surface | iced 0.14 core | third-party | hand-rolled |
|---|---|---|---|
| button, divider | `button`, `rule` | — | — |
| tooltip | `tooltip` | — | — |
| scroll area | `scrollable` | — | — |
| tabs | — | iced_aw `tabs`/`tab_bar` | — |
| menu bar / context menu | — | iced_aw `menu`/`context_menu` | — |
| popover / drop-down | `overlay`, `stack` | iced_aw `drop_down` | (a bespoke popover: small) |
| combobox / picker | `combo_box`, `pick_list` | iced_aw `selection_list`, `number_input` | — |
| dialog / modal | — (core `overlay` + `stack`) | iced_aw `overlay` (the layer, not the dialog) | **yes** — focus trap, escape, backdrop |
| toast / notification | — | — | **yes** |
| file dialog | — | `rfd` (untested here) | — |
| text input, IME | `text_input`, `text_editor` (`input_method()`) | iced_aw `typed_input` | — |
| rich text, markdown | `Rich`, `markdown` | — | — |
| lists / pool panel | `scrollable` + `keyed`/`grid` | — | **yes** for windowing (no virtualised list) |
| table / detail view | `table`, `grid` | iced_aw `labeled_frame`, `card` | — |
| Surface slots (fill/left/right/bottom) | `pane_grid` | — | — |
| meters (peak, hold) | `progress_bar` as the bar | `iced_audio` has none | **yes** for hold/ballistics |
| faders, knobs, XY | `slider` | **`iced_audio`** (adopted) | — |
| timeline: clips, lanes, playhead | `canvas` | — | **yes** (shipped) |
| timeline: **waveform envelope from the peak pyramid** | `canvas` + `media::peaks` | — | **yes** (shipped) |
| timeline: zoom/scroll/fit | `canvas` | — | **yes** (shipped) |
| timeline: snap grid + guide | `canvas` | — | **yes** (grid drawing; snapping is `media::snap`) |
| timeline: click-to-seek | `canvas` `Program::update` | — | **yes** (shipped) |
| timeline: clip drag / edge resize / marquee / loop bracket | — | — | **yes, unbuilt** — pointer model on `canvas` |
| cross-widget drag-and-drop | — | — | **yes** — the payload + cancel protocol is ours to design |
| multiple windows (plugin editors) | `iced::window` | — | — |
| HiDPI | `iced_winit` (the spike's screenshots are scale 2) | — | — |
| clipboard | `iced::clipboard` | — | — |
| **accessibility tree** | **none** (verified: zero `accesskit` references) | none | **yes, or declared absent** |

## 5. The hand-rolled count, stated

Criterion 3 asks for the count explicitly. It splits into what is shipped and what is still owed.

**Shipped in the iced spike** (`spikes/iced-shell/src/main.rs`, 3,938 lines of which ~1,170 are tests):

| Piece | Lines |
|---|---|
| timeline viewport (zoom/scroll/fit/clamp) + envelope maths + `canvas` program | **~550** |
| `--wave` import and placement | ~85 |
| console, peaks cache, focus/keys, command line, keymap overlay, perf instrument, status | the remaining ~2,000, of which the fader/repeater machinery leans on `iced_audio` |
| tests | ~1,170 (29 tests) |

**Still owed for the plan's surface** — *estimates*, scaled from the shipped timeline work (~550 lines
for one canvas view with its maths and tests), not measurements:

| Piece | Estimate |
|---|---|
| clip drag / edge resize / marquee / loop bracket (one pointer model, with snap + guides) | 400–700 |
| snap grid + guide drawing | 100–150 |
| a peak meter with hold/ballistics | 100–150 |
| pool panel with windowing (rename, import, drag to lane) | 250–400 |
| detail view fields (typed inputs on `text_input`/iced_aw) | 150–300 |
| menu bar / context menu wiring (iced_aw widgets, our command dispatch) | 100–200 |
| a dialog/modal + toasts | 150–250 |
| cross-widget drag-and-drop protocol | 150–300 |
| accessibility tree (or the decision to declare its absence) | unbudgeted |

So the honest shape is **roughly 1,400–2,600 further hand-rolled lines** for the surface the plan
describes — one canvas-heavy view already accounts for ~550 — against a *webview* whose equivalent
chrome came from the DOM and whose component library was, in the shipped shell, never used. The gap is
real but it is concentrated in **pointer interaction, list windowing and accessibility**, not spread
across visual components.

## 6. Sources and how they were read

- `crates/shell/` (the retired shell, frozen in-tree): `package.json`, `src/*.vue`, `src/*.ts`,
  `src/style.css` — imports, tags/attributes and line counts above are greps of this directory.
- `docs/design/ui-plan.md` §1–§4 — the plan's chrome primitives, the plugin model, and the hybrid
  rendering strategy (reka-ui for behaviour+a11y, Canvas 2D for audio-native widgets, thin CSS).
- `~/.cargo/registry/src/…/iced_widget-0.14.2/src/` — the widget module list, `lazy.rs`'s own doc,
  `text_input.rs`/`text_editor.rs` `input_method()`, `text/rich.rs`, `table.rs`, `markdown.rs`.
- The accesskit grep covered `iced-0.14.0`, `iced_winit-0.14*`, `iced_runtime-0.14*`, `iced_core-0.14.0`.
- crates.io API: `iced_aw` versions and `iced_aw 0.14.1`'s dependency requirements; docs.rs for its
  widget list.
- The iced spike's own instrumentation (`--perf`): 1,628 envelope columns per redraw, steady p50
  16.7–17.0 ms on the software renderer — see the
  [spike README](../../spikes/iced-shell/README.md).

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-15.*
