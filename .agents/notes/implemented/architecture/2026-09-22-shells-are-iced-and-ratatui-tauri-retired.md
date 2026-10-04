# Agent Note: the shells are iced and ratatui — Tauri is retired

Status: implemented

> **Superseded in framing (2026-09-30):** [Tauri was an experiment we decided
> against](2026-09-30-tauri-was-an-experiment-and-iced-is-next.md) sharpens this decision. Tauri is
> an approach the owner *tried and decided against*, not a "porting reference" with a live role,
> and that note records the forward plan — iced carries the recorder profile once the recorder is
> good enough. This note's reasoning and its decision stand; the role it assigns the frozen crate
> does not.

## Problem

The app shell is a **Tauri v2 + Vue 3 + TypeScript** desktop app. It works, and it was the
right thing to build first — it proved the UI-as-plugin contract end to end and is the only
shell that has reached a full editor. But it costs the project its shape: two languages, a
webview, an IPC wire and a serde surface to keep in step, a `!Send` host session behind a
mutex, an npm toolchain in the build, and webkit/GPU dependencies on the deploy path. The
[iced](../../proposed/architecture/2026-09-21-iced-shell-evaluation.md) and
[ratatui](../../proposed/architecture/2026-09-21-tui-shell-evaluation.md) spikes exist because the owner's direction is
"reduce complexity: one stack, in-process Rust, over the same Host API" — and having now run
both, the owner's decision is: **retire Tauri, continue with iced and ratatui.**

Which of those two is the *primary* shell is deliberately **not** decided yet; this note
retires a shell and records what decides the rest.

## Decision

**Tauri + Vue is retired. The shells are ratatui (primary, TUI) and iced (second, GUI), with one
modal, key-driven workflow across both.**

1. **Retired means out of the active path, not deleted.** `crates/shell/src-tauri` is removed
   from the root workspace `members` (and listed in `exclude`), so the default `cargo build
   --workspace` / `cargo test` / clippy no longer build Tauri, webkit or the Vue toolchain —
   but the directory stays as a reference, with `crates/shell/RETIRED.md` stating the status
   and the two commands that still build it in place. It is a working editor: the timeline
   canvas, the mixer layout, the transport store and the clip-edit gestures are the porting
   reference for the Rust shells. Deleting it is a one-command follow-up whenever the owner
   prefers the tree without it.
2. **One workflow, two shells is the rule** (2026-09-22). A capability lands in the *workflow* — an
   action in the `host v1` vocabulary, a key, a line in the keymap table — before or with the shell
   that shows it; a shell-only feature (or a key that exists in one shell and not the other) is a bug.
   **Built 2026-09-22:** the model is the **`workflow` crate** (`crates/workflow`, a core-workspace
   member with no UI toolkit) — modes, a neutral `Key`, the `Action` vocabulary, and the keymap table
   that generates the `?` help. The TUI translates crossterm into it and dispatches; **the iced shell
   does the same with iced's keys** — same table, same actions, same command line, same help. Where
   iced has no surface yet (the timeline), the action *reports the gap* in its status line rather
   than doing nothing, so the parity debt is visible instead of silent.
3. **The Host API is the shell seam, and two shells now prove it.** Both spikes drive the
   *same* `host::live::HostHandle` the bridge drove (`spikes/iced-shell`, `spikes/tui-shell`),
   each its own isolated workspace so neither framework leaks into the core's build. The
   [UI-as-plugin note](2026-08-18-ui-as-plugin-host-api-and-headless-reference.md) argued
   this seam; the retirement is the first real test of it, and the core crates did not change.
4. **`iced_audio` is the iced shell's widget library.** `iced_audio 0.17` (MIT, 2026-09-09)
   tracks `iced_core`/`iced_graphics` 0.14 exactly — the version the spike already used — and
   supplies what a mixer is made of: `VSlider`/`HSlider`/`Knob`/`Ramp`/`XYPad`/`ModRangeInput`,
   a normalized-parameter contract (`NormalParam`), unit ranges (`DBRange`, `FreqRange`,
   `IntRange` with `snap`), tick/text marks and modulation ranges. The spike uses it for the
   console: one `VSlider` per channel plus the master on `DBRange::new(-60.0, 12.0, …)`, meters
   beside them, values in dB. The widgets are **stock** — no fork, no vendored patch.
5. **The TUI is primary; iced is the second shell** (decided 2026-09-22, the day after the
   retirement). The modal, key-driven workflow is what the tool *is*, and the terminal is where it is
   cheapest to prove and cannot cheat — no mouse to fall back on, and frames that can be asserted.
   iced follows key for key, and its job is what a terminal cannot do: the same keys and the same
   `host v1` commands with real text (a `:` prompt with completion, dialogs, multi-window) — the
   emacs move. The owner's framing: "the same workflow on iced as on ratatui… and eventually lean
   into the benefits of not running inside a terminal", with iced's churn accepted as the price.
   See *one workflow, two shells* in the
   [modal editing note](../../proposed/architecture/2026-09-21-modal-editing-model.md).

## Evidence

- **The iced spike mixes with stock widgets and reads the log.** `iced_audio` faders sit on
  the host's **parameter fold**: the demo script's `set_param mixer master.gain 0.8` shows as
  **master −1.9 dB** (`20·log10(0.8)`) while `ch0…ch3` sit at +0.0 dB, and dragging a fader
  sends a logged `set_param` (the status line reports `ch0.gain = −54.0 dB (0.0020)` after a
  drag in the running app). Screenshot:
  [`spikes/iced-shell/mixer-fold.png`](../../../../spikes/iced-shell/mixer-fold.png).
  Two tests pin the parts that are easy to get wrong: the strip↔parameter-name mapping (the
  master's fader index is the channel *count*) and the dB range round trip (unity is 0 dB,
  silence floors at the range minimum instead of −∞/NaN).
- **Wiring cost, measured.** iced must be built with the `canvas` **and** `image` features for
  `iced_audio` (both off by default) — the first build fails without `image` on the
  wgpu + tiny-skia fallback renderer. `HostCommand::SetParam` takes `&'static str` names, so a
  shell needs a static name table and an explicit index mapping rather than `format!`.
- **The core is untouched by the retirement.** After removing the shell from the workspace:
  `cargo build/test/clippy --workspace` cover `engine`, `media`, `host` — 22 suites green,
  clippy clean — and the retired crate still `cargo check`s standalone from its own workspace.
- **Both spikes run headless**, which is what made them usable evidence without a human
  watching: `spikes/iced-shell --probe` (silent host, meters) and `spikes/tui-shell --probe`
  / `--dump` (a deterministic rendered frame) plus its pty harness.

## Alternatives considered

- **Keep Tauri as the shipped shell and grow iced/ratatui as spikes.** Rejected by the owner:
  it keeps exactly the dual-stack cost the direction exists to remove, and it splits attention
  three ways. The spikes already answer "can it drive the Host API"; the remaining question is
  which Rust shell is primary.
- **Delete `crates/shell` now.** Rejected for this commit: it is the only complete editor and
  the porting reference for the Rust shells' most expensive surfaces (timeline canvas, clip
  gestures, zoom/pan). Freezing it costs nothing (it is out of the build); deleting it is
  trivial later, and `git log` keeps it either way.
- **Keep the shell a workspace member, just "frozen".** Rejected: a `members` entry is a
  standing tax — every `crates/host` API change must keep a retired crate compiling, and the
  default test run builds webkit and the npm toolchain for a shell nobody ships.
- **Pick a primary shell now (iced, or ratatui).** Rejected as premature: the iced spike has
  transport and a real mixer but no timeline; the TUI has the timeline, the console, modal
  editing and a pty harness, but its interaction model (mouse in a terminal, key-only editing)
  is exactly what is unproven. Choosing now would decide on vibes.
- **Adopt a different iced audio-widget library (hand-rolled faders, `egui`-style immediate
  widgets, or vendoring `waveformchart`).** Rejected: `iced_audio` is the maintained,
  permissively licensed, version-locked option, in the iced org, and it is by the same author
  as the plugin framework below — a hand-rolled fader is a week of styling for a worse result.
- **Take the Tauri shell's Vue components as the design language for the Rust shells.**
  Partially rejected: the *layout* is worth porting (desk layout, timeline ruler, transport),
  but the visual language should come from iced/TUI idioms, not from replicating shadcn-vue in
  a canvas.

## Consequences

- **The repo is a single stack again**: Rust engine + media + host, with Rust shells. The
  removed surface is the whole TypeScript/Vue app, the IPC wire, the serde bridge grammar
  duplication, and the webkit/GPU deploy dependency.
- **A working editor sits frozen in the tree.** Until one Rust shell reaches parity, features
  the owner wants *today* have nowhere to live; that is the accepted cost of the retirement,
  and `crates/shell/RETIRED.md` says how to resurrect it if a porting question needs it.
- **The primary is the TUI; iced is second** — and the criteria below are why, in the order they
  counted (not as an open question any more, but as the standard the second shell is still held to):
  - **Surfaces**: timeline (waveform, lanes, playhead), mixer, pool, detail; how much is stock
    widget vs hand-drawn canvas.
  - **Interaction**: modal editing, visual-mode selection, the `:` prompt; whether a terminal's
    keys and mouse can carry it or mouse-first wins.
  - **Text**: the project is text-heavy (scripts, the log, file paths, numbers) — a GUI with
    real layout vs a terminal's cell grid.
  - **Reach**: the TUI runs over SSH, on a container, on a Raspberry-Pi "box mode", with no GPU;
    iced needs a display and a working wgpu stack (it does work here: niri/Wayland + Mesa).
  - **Testability**: ratatui's `TestBackend` + a pty harness (deterministic frames) vs iced's
    windowed app (no headless render in the spike yet). This decided it: the workflow can be
    *asserted* in the TUI, and a workflow is what is being built.
  - **The plugin story**: if we later export instruments as CLAP plugins (see
    [the export note](../../proposed/architecture/2026-09-22-clap-export-via-nice-plug.md)),
    the iced shell's widgets and layouts are reusable as plugin editors via `nice-plug-iced`;
    a TUI cannot be a plugin editor at all.
- **`RESEARCH.md` §4.5/§4.6 and the §0 app-shell row** now read as "retired / under evaluation"
  rather than "shipped", and the notes that assumed the Tauri shell carry a superseded marker.
- **iced's churn is an accepted, budgeted cost** (the author explicitly reserves breaking changes;
  0.14 → 0.15-dev already moves the MSRV). Two things keep it affordable: the shells are thin over
  the Host API (the hard work lives in `engine`/`media`/`host`, which don't move with iced), and the
  *workflow* is the durable artifact — it is a note, a keymap table and a text vocabulary, none of
  which is iced-shaped. A future iced can be re-adopted from the note; a workflow cannot.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-22.
