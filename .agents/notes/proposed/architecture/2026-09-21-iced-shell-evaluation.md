# Agent Note: evaluating iced as an in-process Rust shell

Status: proposed

> **Movement (2026-09-22):** the owner **retired the Tauri + Vue shell**, named ratatui primary and
> iced second, and fixed the rule that both implement the *same* modal, key-driven workflow — so this
> is no longer "an option against the status quo" but the shell that carries the GUI-only surfaces
> (text, dialogs, windows, plugin editors) over that workflow ([shells
> note](../../implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md)).
> `iced_audio` is adopted for the iced shell's mixer widgets (evidence below), and the no-webview
> comparison in the Problem section is now the shipped state, kept as the reasoning that got here.

## Problem

The shipped shell is **three runtimes for one program**: the Rust engine/media/host crates, a Rust
transport adapter (`crates/shell/src-tauri`), and a TypeScript + Vue UI — with an IPC + serde wire,
a second package manager (`pnpm`), a second test runner (vitest), and a webview between the last
two. The [UI plan](../../../../docs/design/ui-plan.md) documents what that costs in practice: the
governing constraint on the UI *design* is not the DAW but **WebKitGTK's compositing behaviour**
(no backdrop-filter, no large shadows, no deep re-layout during transport ticks, canvas-or-die for
anything at drag rate), and §4.4 is a list of data-flow rules invented to keep 60 Hz traffic off
the IPC boundary.

That complexity is *incidental*, and the project's own thesis says so: everything is a plugin, the
UI is a plugin, and the shell is interchangeable. The
[UI-as-plugin note](../../implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md)
anticipated this exact option — "egui / iced in-process shell — the boundary becomes voluntary. A
later second host, not the reference" — and deferred it. The question now is whether the deferred
option is better than the reference, given an app whose hot paths are a canvas timeline, live
meters, and a transport position: all of which are *values* the host already publishes.

[iced](https://github.com/iced-rs/iced) is the candidate because of what it refuses to be: one
language (no HTML/CSS/JS split, no view DSL — its
[philosophy](https://book.iced.rs/philosophy.html) argues the separation of structure/style/logic
is the web's core mistake), and a runtime built around the **Elm architecture** — state, message,
`update`, `view`, subscriptions — which is the same values-in/effects-out shape this codebase
already uses for the session log and the arrangement. If a shell can be one Rust binary that speaks
`HostCommand` directly, the dual stack, the wire, and the adapter all stop existing.

## Proposal

**An evaluation, not a migration.** Measure iced against the surfaces that actually decide the
question, and keep the Tauri + Vue shell shipped until those measurements exist.

1. **A spike lives at [`spikes/iced-shell/`](../../../../spikes/iced-shell/)** as its **own workspace**,
   so iced and wgpu never enter the core's `cargo build --workspace`; deleting the directory
   removes the option cleanly. It is built against **iced 0.14.0** (the released baseline; iced
   `master` is `0.15.0-dev` and needs rustc ≥ 1.93 — hence the rustup-managed toolchain, see the
   [rustup note](../../implemented/process/2026-09-21-rustup-managed-toolchain.md)).
2. **The spike proves the seam, minimally and honestly**: the window opens on this host; the shell
   owns the *same* `host::live::HostHandle` the Tauri bridge uses, in-process; transport
   (play/stop/rewind + hotkeys) drives it; channel + master meters and the position readout follow
   the audio; and a headless `--probe` mode (boot → load → play → poll the snapshot) is CI-able and
   fails loudly when no meter signal appears.
3. **The seam stays honest in-process.** The evaluation only counts if the shell mutates state
   *through the Host API* (`HostCommand` + the published `Snapshot`) rather than reaching into
   `HostSession`. In-process is a performance choice; it must not become a license to bypass the
   contract, or the "swapping shells swaps only the transport adapter" claim quietly dies.
4. **The measured follow-up is what decides.** Before any migration: a canvas timeline slice (the
   real pool peaks, zoom/pan, hit-testing, playhead) and a widget inventory of the surfaces the
   Vue shell already has.

## Evidence so far

- **The spike is built and verified** at [`spikes/iced-shell/`](../../../../spikes/iced-shell/)
  (iced 0.14, its own workspace): the window opens and renders on this host (niri/Wayland, Mesa,
  wgpu — the exact thing the parked Nix shell died on), it opens the real audio device at
  48 kHz / 2 ch, the transport drives it, the channel and master meters and the readout follow the
  audio, and `--probe` observes live meter signal (peak 0.88) headlessly.
- **The mixer is stock-widget material, and it reads the log** (2026-09-22). The spike's console
  uses `iced_audio 0.17` `VSlider`s on a dB range (`DBRange`, **derived from the mixer's declared
  gain bounds** → −60…+6.0 dB, unity at ~91 % of the travel) — one fader per channel plus the
  master, a live meter and a dB readout beside each. The
  positions come from the host's **parameter fold**: the demo script's `master.gain 0.8` boots as
  **master −1.9 dB** (20·log10(0.8)) while `ch0…ch3` sit at +0.0 dB, and a drag sends a logged
  `set_param` back (screenshot:
  [`spikes/iced-shell/mixer-fold.png`](../../../../spikes/iced-shell/mixer-fold.png)). Two tests
  pin the strip↔parameter mapping and the dB range round trip. The widget crate tracks iced
  exactly (it depends on `iced_core`/`iced_graphics` 0.14, the version this spike uses), so it
  cannot drift from the shell.
- **The canvas timeline draws real pool material, with a viewport, and the frame budget is
  measured** (2026-10-15) — acceptance criterion 2 below. `--wave <file>` imports a piece into the
  spike's own session pool and places it (one source per channel, cut into consecutive clips so the
  `src_start` offsets and the clip boundaries are real); the canvas draws each clip's **envelope**
  from the pool's persisted `.peaks` pyramid, resolved through the pool directory the snapshot now
  publishes (`Snapshot::pool_dir`) and queried level-aware (`Peaks::level_for`/`range_minmax_at`,
  new on the read side of `media::peaks`), so a column costs one or two bins at any zoom instead of
  walking the source. Zoom (`+`/`-`, anchored on the playhead, floored at one base bin per point),
  scroll (`h`/`l`), fit (`0`) and click-to-seek all work, with `Tab` choosing the focused panel —
  the workflow's own per-panel split, so no key was added to the shared table. Measured with
  `--perf` on a 30 s stereo piece (2 tracks, 6 clips, **1,628** envelope columns per redraw) under
  Mesa's **software** renderer: **steady p50 16.7–17.0 ms (59–60 fps), p95 18.7–30.4 ms** across
  three runs with the transport running (the demo profile, 0 columns, measures p50 13.3 ms / 75 fps
  in a clean run, so the drawing costs a few ms). Some runs show redraw gaps of 0.4–1.0 s that appear
  with 0 columns too — the compositor not asking for a frame reads as a stall, and `--perf` reports
  the gaps by value rather than hiding them. Numbers and the peaks path:
  [`spikes/iced-shell/README.md`](../../../../spikes/iced-shell/README.md).
- **The widget and plugin questions are answered, and the premise corrected** (2026-10-15).
  [The inventory](../../../../research/architecture/2026-10-15-iced-widget-inventory.md) measures the
  retired shell's real surface (plain markup; `reka-ui` declared and never imported; 2,251 lines of
  hand-written front end) against iced 0.14 core + `iced_aw 0.14.1` (version-aligned) and states the
  hand-rolled count; [the UI-plugin note](2026-10-15-ui-plugins-in-a-compiled-shell.md) answers
  criterion 4. Two findings are load-bearing: the Vue shell's "registry" was **static** (no dynamic
  import, no manifest fetch — the runtime loader was design, not shipped behaviour), and iced has **no
  accessibility tree** (zero `accesskit` references across `iced`, `iced_winit`, `iced_runtime`,
  `iced_core`), which is exactly what the plan bought with `reka-ui`.
- **The human's read of the window is favourable** — "looks good and more native" than the webview
  shell — and **`egui` and Slint were dismissed by inspection** as not what this app wants. The
  appraisal is an input to the decision, not evidence for it: the acceptance criteria below are
  what closes it.
- **A second candidate is now in flight**: a terminal shell
  ([TUI evaluation](2026-09-21-tui-shell-evaluation.md)), deliberately scoped identically. Since
  2026-09-22 these two *are* the choice — three shapes became two.
- **And the interaction model is now a decided direction, separate from the shell**: the UI should be
  *for audio what vim/helix/emacs is for text* — modal, selection-first, with the existing `host v1`
  vocabulary as the `:` command line ([modal editing model](2026-09-21-modal-editing-model.md)).
  iced can host that model (keys, a visible mode, the same commands), but the two decisions belong
  together: a GUI-shaped shell hides the log that this engine is built around, and a modal model is
  cheapest to validate where there is no mouse to fall back on.
- **The spikes are already earning their keep on the host side**: running them against live audio
  surfaced a one-time ~2–3 s underrun-counter burst at the first transport command
  ([bug-fix note](../bug-fix/2026-09-21-live-host-underrun-burst-on-transport-change.md)).

## Alternatives considered

- **Keep Tauri v2 + Vue 3 (the status quo).** The safe answer, and what shipped first. It is *not*
  obviously worse: the webview gives text rendering, IME, accessibility, menus, and a component
  ecosystem for free, and the canvas timeline is already the right rendering strategy there.
  **Rejected by the owner on 2026-09-22** — not because it failed, but because it is three runtimes
  for one program and the direction is one stack
  ([shells note](../../implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md));
  the code stays frozen in the tree as the porting reference.
- **egui (immediate mode).** Smaller dependency tree and a very fast start, but immediate mode
  re-derives the whole UI every frame and offers no architecture (no message loop to reason about,
  no subscription model); it fits tools, not a long-lived instrument with a session value.
  Rejected: it contradicts the reason to look at iced at all.
- **Slint / Vizia / GPUI.** Slint's `.slint` DSL re-introduces a separate view language (that is the
  thing iced's philosophy refuses); Vizia is much smaller and less proven for high-frequency canvas
  work; GPUI is Zed's, GPU-first, moving fast, and not aimed at third-party apps. None beats iced
  for *this* reasoning. Rejected for now, not permanently.
- **Drop the webview but keep the web stack (Dioxus / Leptos / Yew in a webview, or a WASM UI).**
  Removes TypeScript and the serde wire but keeps HTML/CSS, keeps the webview's constraints, and
  adds WASM glue. Rejected: it pays most of the dual-stack cost for none of the single-language
  benefit.
- **Terminal shell (ratatui).** Initially rejected as *the* evaluation — it answers none of the
  questions a *pixel* DAW UI raises (waveform timeline, drag editing) — but it is now measured
  alongside iced: a TUI runs headless and over SSH, it is the cheapest shell to build and test, and
  it is the natural home for the keyboard-first workflow. It has its own spike and note
  ([TUI evaluation](2026-09-21-tui-shell-evaluation.md)), deliberately scoped identically so the
  two can be compared. **It won the primary role on 2026-09-22** — iced is now the second shell, and
  its measured job is the surfaces a terminal cannot carry, over the same workflow.

## Acceptance criteria

The evaluation is answered when all of these hold, and the answer is written as an implemented (or
rejected) note replacing this one:

1. The spike's `--probe` passes on a clean checkout under the rustup toolchain, and the window
   opens and renders on this host (niri/Wayland + Mesa). *Answered.*
2. A **canvas timeline slice** draws the real media-pool peaks for N clips with the same viewport
   maths as the Vue path (zoom/pan/seek), and holds frame rate while the transport runs.
   *Answered 2026-10-15 for peaks, viewport and frame rate (see the evidence above): real
   envelopes from the persisted pyramid, zoom/scroll/fit/click-to-seek, and a measured 60 fps
   steady p50 on the software renderer. The "same maths as the Vue path" is now the **TUI's**
   maths, since the Vue shell was retired: same laws, per-shell units.*
3. A **widget inventory** lists every surface the Vue shell has today (menus, dialogs, context
   menus, tooltips, pickers, text inputs with IME, virtualised lists, drag-and-drop) against iced's
   core + third-party crates, with the hand-rolled count stated explicitly. *Answered 2026-10-15:*
   [the inventory](../../../../research/architecture/2026-10-15-iced-widget-inventory.md) — and it
   corrects this criterion's premise: the shipped Vue shell used **plain markup** (15 `<button>`, 3
   `<input>`, 3 `<textarea>`, 1 `<canvas>`, 0 selects/dialogs/context menus/drags), declared `reka-ui`
   and imported it **nowhere**; iced's core has 37 widget modules (including `tooltip`, `table`,
   `markdown`, rich text, and IME in both text widgets) and `iced_aw 0.14.1` is version-aligned and
   adds the menu bar, context menu, drop-down, tabs and pickers. The real gaps are pointer
   interaction, list windowing and the **accessibility tree** (zero accesskit references anywhere in
   the iced stack).
4. The **UI-as-plugin question** is answered in writing: how (or whether) runtime-contributed UI
   survives in a compiled Rust shell, and what that costs the composition thesis. *Answered 2026-10-15:*
   [UI plugins in a compiled shell](2026-10-15-ui-plugins-in-a-compiled-shell.md) — views are compiled
   crates behind a toolkit-free contract, composition is a data table over a compiled registry (which
   is what the Vue shell's static `plugins.ts` actually was), the `Command` half is already the
   shared `workflow` table, runtime *code* contribution is deferred behind a named WASM/external-process
   door, and accessibility is a stated cost rather than an assumed freebie.
5. The transport/meters seam is exercised **only** through `HostCommand` + `Snapshot`, verified by
   the shell compiling without touching `HostSession`. *Holds by construction; still not asserted as
   a gate.*

## Risks

- **The widget gap is the real cost — but it is narrower, and differently shaped, than this note first
  assumed.** iced's core has 37 widget modules and `iced_aw` covers menus, context menus, drop-downs,
  tabs and pickers; the shipped Vue shell used plain markup and never imported the component library
  this note credited it with. What is genuinely ours to build is **pointer interaction** (clip drag,
  resize, marquee, cross-widget drag), **list windowing** (no virtualised list; `lazy` only caches) and
  an **accessibility tree**, which iced does not have at all
  ([inventory](../../../../research/architecture/2026-10-15-iced-widget-inventory.md)).
- **Single-maintainer churn.** iced explicitly reserves breaking changes; 0.14 → 0.15-dev already
  moves the MSRV to 1.93. A shell written against it is a commitment to periodic porting.
- **Losing the webview's free surface**: text layout, IME, accessibility, HiDPI, clipboard,
  file dialogs — all hand-rolled or crate-supplied in iced.
- **The in-process boundary rots.** With no IPC in the way, it becomes easy to reach into the host
  directly; the seam then survives only by discipline (criterion 5).
- **A spike is not a shell.** The proof now covers windowing, threading, transport, live meters,
  the timeline canvas with real peaks and a viewport, and a measured frame budget. It still does not
  cover clip **editing** (split/trim/move/fade), the pool panel, or the detail view — the parts most
  likely to be worse.
- **Two shells to maintain during the evaluation.** The spikes are excluded from the default build,
  but their dependencies and their drift are still a (small) ongoing cost. Since the retirement
  there is no third shell to keep in step — the frozen Tauri crate is out of the workspace.
- **`iced_audio` is a young crate by one author** (0.17, 2026-09-09, MIT, in the iced org but
  released by BillyDM). Mitigation: it is widgets on top of `iced_core` 0.14, version-locked to the
  shell's iced; if it stalls, the fader/knob draw code is a canvas widget, not a rewrite of the
  shell.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21.
