# Agent Note: evaluating iced as an in-process Rust shell

Status: proposed

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
- **The human's read of the window is favourable** — "looks good and more native" than the webview
  shell — and **`egui` and Slint were dismissed by inspection** as not what this app wants. The
  appraisal is an input to the decision, not evidence for it: the acceptance criteria below are
  what closes it.
- **A second candidate is now in flight**: a terminal shell
  ([TUI evaluation](2026-09-21-tui-shell-evaluation.md)), deliberately scoped identically, so the
  choice is between three shapes rather than two.
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

- **Keep Tauri v2 + Vue 3 (the status quo).** The safe answer, and the shipped one. It is *not*
  obviously worse: the webview gives text rendering, IME, accessibility, menus, and a component
  ecosystem for free, and the canvas timeline is already the right rendering strategy there.
  Rejected as an *unexamined* default, not as an option — it stays the shell until the evaluation
  finishes.
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
  two can be compared; the reference-host role is its fallback if it loses.

## Acceptance criteria

The evaluation is answered when all of these hold, and the answer is written as an implemented (or
rejected) note replacing this one:

1. The spike's `--probe` passes on a clean checkout under the rustup toolchain, and the window
   opens and renders on this host (niri/Wayland + Mesa).
2. A **canvas timeline slice** draws the real media-pool peaks for N clips with the same viewport
   maths as the Vue path (zoom/pan/seek), and holds frame rate while the transport runs.
3. A **widget inventory** lists every surface the Vue shell has today (menus, dialogs, context
   menus, tooltips, pickers, text inputs with IME, virtualised lists, drag-and-drop) against iced's
   core + third-party crates, with the hand-rolled count stated explicitly.
4. The **UI-as-plugin question** is answered in writing: how (or whether) runtime-contributed UI
   survives in a compiled Rust shell, and what that costs the composition thesis.
5. The transport/meters seam is exercised **only** through `HostCommand` + `Snapshot`, verified by
   the shell compiling without touching `HostSession`.

## Risks

- **The widget gap is the real cost.** iced's core widgets are a fraction of reka-ui/shadcn; the
  re-implementation of timeline maths, inspector inputs, menus, and drag gestures may exceed the
  complexity it removes.
- **Single-maintainer churn.** iced explicitly reserves breaking changes; 0.14 → 0.15-dev already
  moves the MSRV to 1.93. A shell written against it is a commitment to periodic porting.
- **Losing the webview's free surface**: text layout, IME, accessibility, HiDPI, clipboard,
  file dialogs — all hand-rolled or crate-supplied in iced.
- **The in-process boundary rots.** With no IPC in the way, it becomes easy to reach into the host
  directly; the seam then survives only by discipline (criterion 5).
- **A spike is not a shell.** The proof covers windowing, threading, transport, and live meters. It
  deliberately does not cover the timeline, the detail view, or editing — the parts most likely to
  be worse.
- **Two shells to maintain during the evaluation.** The spike is excluded from the default build,
  but its dependencies and its drift are still a (small) ongoing cost.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21.
