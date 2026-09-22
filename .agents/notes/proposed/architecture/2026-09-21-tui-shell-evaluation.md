# Agent Note: evaluating a ratatui (TUI) shell

Status: proposed

## Problem

The shell question is open. [iced](2026-09-21-iced-shell-evaluation.md) is the promising
single-stack candidate — the human's own read of the spike is that its window "looks good and more
native" than the webview — but iced still has to grow a bespoke widget set (timeline canvas,
inspector, menus) and it is a heavy dependency for an app whose core interaction is *values over
time*. `egui` and Slint are out by inspection: neither looks like what this app wants (immediate
mode for the first, a second view DSL for the second).

A terminal shell is the cheapest possible shell and the only one that runs **anywhere**: no GPU, no
display server, over SSH, inside a container, on the Raspberry-Pi "box mode" that
[RESEARCH](../../../../RESEARCH.md) keeps deferring. The
[UI-as-plugin note](../../implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md)
listed ratatui as exactly this — "a good later second reference (the CLI with a face)" — and it is
the natural home for the keyboard-first workflow an instrument-like editor wants.

Two things are genuinely unknown, and they are why this is a spike rather than a decision:

1. **Can keys replace the mouse?** The concern with a TUI is that mouse interaction in a terminal
   is clumsy (capture conflicts with text selection and terminal multiplexers; hit targets are
   character cells). The working hypothesis is that a well-designed key scheme carries the
   workflow instead — but that has to be *tried*, and the scheme has to be discoverable.
2. **What is a timeline in a terminal?** A DAW's primary surface is a waveform at pixel
   resolution with drag editing. Braille/block codepoints can approximate a low-resolution
   rendering, and the Kitty graphics protocol (or sixel) can do real images in capable terminals —
   but none of that is proven here, and the honest fallback (a list of clips with a coarse tape
   map) is a much smaller product.

## Proposal

**A TUI spike, to the same scope as the iced one, so the two are comparable.** It lives at
[`spikes/tui-shell/`](../../../../spikes/tui-shell/) as its own workspace (the terminal backend
never enters the core build), and it drives the **same `host::live::HostHandle`** the Tauri bridge
and the iced spike use — in-process, no IPC, no webview, no TypeScript.

Deliberately identical to iced: play / stop / rewind / seek; channel and master meters; the
position readout following the audio; a `--probe` headless mode; a demo profile whose signal is the
same one the iced spike watches.

Deliberately different, because these are the TUI's own questions:

1. **An explicit key scheme with a discoverable overlay.** A `?` overlay is rendered *from the same
   table the key handler matches* (`KEYMAP`), so help and behaviour cannot drift — the first
   concrete answer to "can a key scheme be the interface".
2. **Mouse, so the hypothesis can be tested rather than assumed.** Clickable transport buttons,
   click-to-select meter rows, wheel = seek. The view publishes the rects it drew
   (`App::buttons` / `App::meter_rows`) and the mouse handler hit-tests them — the terminal has no
   widget tree, so hit regions are a small thing the shell owns.
3. **Command latency in the UI thread.** Every command is timed and shown
   (`last command: seek (13.4 ms …)`), because `TransportSeek` is O(target) and a shell that binds
   it to a key owns that stall.
4. **Verifiability without a display.** ratatui's `TestBackend` makes the *view* testable (a
   deterministic `--dump` frame plus five assertions on the rendered buffer), and a pty harness
   (`scripts/pty-check.sh`) covers the real path (raw mode, alternate screen, mouse capture, input
   decoding).

## Reference and prior art

A dedicated study — [Helix's keyboard model, and what a terminal can actually draw](../../../../research/architecture/2026-09-21-tui-audio-prior-art.md)
— answers the two questions this spike deliberately left open, and both answers are load-bearing:

- **The interaction model has a proven ancestor, and it is not vim.** Helix's **selection-first**
  ("select, then act") model fits clips better than it fits text: a selection *is* `{track, clip,
  start, end}`, the acted-on set is rendered *before* anything is destroyed, and bulk edits need
  collapse/keep-primary keys to stay escapable. Alongside it, four mechanisms are directly
  portable: a **labelled keymap trie** with "user wins" TOML overrides and `no_op`, a **prefix
  infobox** that replaces this spike's full-screen `?` help, a **command palette** whose second
  column shows the keys bound to each command, and **documentation generated from the live keymap**
  (so the spike's hand-written keymap table stops being a second source of truth).
- **The `:` command line does not need to be invented.** Helix declares every action as typed data
  with arity validation; this project already has that — the versioned `host v1` text format, with
  registry validation and line-numbered errors, which is simultaneously the CLI surface, the LLM
  seam and the replay log. A TUI needs a **prompt widget**, not a command language.
- **The timeline is a density question, and the answer is "overview, not samples".** At 200×50 the
  terminal gives ~400 time columns and ~200 amplitude steps; 400 columns over a 3-minute clip is
  0.45 s/column, and the zoom floor is ~0.6 ms/column (~30 samples). So the timeline is an
  overview + coarse-trim surface, and the honest design is a **min/max envelope per column from the
  existing peak pyramid** (`PeakBuilder::range_minmax`) — the `audiowaveform` model, which needs no
  new audio work. Lanes want **braille** (2×4 sub-cells, one colour per cell); clip colour bars,
  meters and the playhead want **blocks/half-blocks** (colour per sub-pixel, lower resolution).
  **Terminal image protocols are a detail-pane luxury, never the timeline**: Kitty is stateful and
  can draw *under* text, sixel is immediate-mode and is *erased* by text drawn over it, tmux has no
  Kitty support at all, and Alacritty/Konsole are unusable.
- **The ground is unclaimed.** No terminal program draws a waveform in a **multi-track
  arrangement**: the field splits into single-file sample editors (`tui-wave` — Rust, alive, braille
  in every terminal plus Kitty graphics where available, cut/copy/undo on **one** buffer — and
  destructively `MrDopey/audio-tui-editor`), live meters with no file at all (cava, Prism at 60 FPS,
  scope-tui, sgram-tui), and **MIDI-only** DAWs (Phosphor, tek's arranger). No timeline, therefore no
  arrangement view anywhere. The nearest ancestors are `audiowaveform` (the data model), `cava`
  (sub-cell bars), `waveformchart` (a braille ratatui widget — a *pattern to fork*: it pins
  `ratatui 0.29` and we are on 0.30.2) and the tracker grid as an editing surface.
- **The rendering is not the moat.** The closest projects are agent-built or LLM-assisted
  (`tui-wave` states it outright), so drawing a waveform in a terminal is cheap for anyone now. What
  none of them has — and this repo's engine already does — is the **non-destructive clip model over a
  replayable log**.
- **And the model now has a direction.** The human's read is that the UI should be *for audio what
  vim/helix/emacs is for text*, with **visual mode** as the selection model. That is a shell-agnostic
  interaction contract, written down in the
  [modal editing model note](2026-09-21-modal-editing-model.md); this spike is where it gets tested
  (a TUI has no mouse to fall back on, so the keymap has to carry the whole workflow).

## Alternatives considered

- **iced only.** The current front-runner and the better *product* shell: real pixel graphics,
  text layout, fonts, HiDPI, a canvas widget for the timeline. Rejected as the *only* option
  because it is the heaviest dependency and it cannot run headless/over SSH — a TUI is a genuinely
  different deployment envelope, not a worse version of the same one. The two are not mutually
  exclusive; iced stays the evaluation in flight.
- **ratatui as a second *reference* host only (the UI-as-plugin note's original placement).** The
  cheap, safe framing — a test/dev shell, not a product. Not chosen as the *scope* of this spike:
  if a TUI could be the product for the editing workflow, that deserves a real evaluation, not a
  smoke test. The reference-host role remains the fallback if it loses.
- **egui / Slint / Vizia / GPUI.** Rejected by inspection for this app (immediate mode; a second
  view DSL; too small a track record; Zed's GPU-first framework not aimed at third-party apps).
  Recorded here so the same ground is not re-covered.
- **No TUI at all — keyboard-only shortcuts in the GUI shell.** Cheaper than a second shell, but it
  does not answer the deployment question (headless/SSH/Pi) and does not tell us whether the
  workflow is fundamentally keyboard-shaped. Rejected as an answer to *this* question.

## Acceptance criteria

The evaluation is answered — with an implemented or rejected note replacing this one — when:

1. The spike runs in a real terminal, restores the screen and mouse capture on exit, and its pty
   harness (entered alternate screen, no panic, exit 0) passes on a clean checkout under the rustup
   toolchain. **Met** (see Consequences), including one real crash found and fixed this way.
2. **The timeline question is answered with a built prototype**, not an opinion. **Met — and now an
   arrangement, not a viewer** (2026-09-21). `--script <host script>` draws the **clips and tracks the
   host holds** (the engine's own `Timeline`, resolved through the pool): one lane per track, a
   braille min/max envelope per clip from the peak pyramid, per-track colour, clip boundaries,
   fades and gain drawn, and a **visual-mode selection**. `--wave <file.wav>` is the same view with
   one lane. **Editing exists**: `x` splits and `d` deletes the clip under the playhead by handing
   the host's own parser an `arrange …` line, so a key runs exactly what a script writes and the
   engine logs it — demonstrated interactively (split at 2.000 s, delete the right half, the panel
   re-reads the host's value) and in tests. **Measured density:** 98.917 ms/cell fitted for a 9 s,
   3-track, 5-clip arrangement in a 100×26 terminal (≈10 cells/second), floor 5.333 ms/cell set by
   the pyramid's 256-sample bin, ceiling = the arrangement **plus a 4× margin** so a piece can be
   seen with room around it. **Still open:** move/trim/fades by key, undo gestures issued from the
   shell, the `:` prompt, and auditioning (the playhead is the *session* clock against the
   arrangement grid). Going below 5.333 ms/col needs a **raw-sample read path** — and the human's
   read is that the braille resolution is already enough, so it is not urgent.
3. **A task-level mouse-vs-keys verdict**: the same short task list (select a clip, seek, split,
   nudge, mute a channel) performed both ways, with the friction noted — including the terminal
   facts that matter (mouse capture vs copy/paste, tmux/niri passthrough, wheel vs key repeat).
   **Partly met:** the mouse now focuses panels, selects a track by clicking its lane, seeks from
   the ruler or a lane, picks a meter row and drives the transport — all dispatching the same
   commands the keys do. What has not been done is the *timed comparison*, and the mouse has no
   drag gestures yet.
4. A **widget inventory** for the TUI: what the shell needs (dialogs? text input? menus?) against
   what a terminal provides well. This is *expected to be smaller* than iced's answer, and that
   expectation has to be tested against the surfaces the app actually has. **Evidence so far:** the
   panel focus ring (`Tab`), the which-key overlay, the state line as a statusline, and click
   regions published by the view are all the shell needed to be usable — the missing pieces are a
   prompt (`:`), a picker (the media pool), and text entry (naming clips/markers).
5. The transport/meters seam is exercised **only** through `HostCommand` + `Snapshot` — the shell
   compiles without touching `HostSession`, exactly as required of the iced spike.

## Risks

- **The graphics ceiling is the real risk.** If the timeline degrades to a list, the product is a
  different (much smaller) instrument. This, not performance, is what decides it — and it is *not*
  about terminals: Prism runs a full meter rack at 60 FPS in a plain terminal, and this spike draws
  a real file's envelope in braille. The measured ceiling is in the **data**: the peak pyramid's
  256-sample bins cap the honest zoom at 5.333 ms/col, so a raw-sample path is what stands between
  this and sample-level work.
- **Mouse in a terminal is awkward, and capture is global.** Enabling mouse capture takes over
  selection (copy/paste) for the whole app; terminal multiplexers and some compositors interfere.
  The `m` toggle exists because a user may need to give the mouse back to the terminal.
- **Keyboard-only may be a fiction for editing.** Dragging a clip edge and marquee-selecting are
  spatial gestures; the key-scheme hypothesis may hold for transport and fail for arrangement.
  That is a finding, not a failure — but it caps what a TUI can be.
- **Fonts and dimensions vary wildly** (box-drawing, braille, Nerd Font glyphs, 80×24 to 400×120);
  a UI that only works at one size is not shippable. The zero-height crash found here is the first
  instance of this class.
- **A third shell is a third maintenance surface.** Both spikes are excluded from the default
  build, but whichever wins, the losing spike must be deleted (the notes say which).
- **The host has its own live-audio rough edges**, and a TUI is where they become visible — see the
  [underrun finding](../bug-fix/2026-09-21-live-host-underrun-burst-on-transport-change.md). This
  is a reason to fix the host, not a reason against a TUI.

## Consequences

The spike exists and is verified; the decision is **open**:

- **What it proved.** The TUI boots against the live host (real audio device negotiated at
  48 kHz / 2 ch), renders the transport, meters and readout, exits cleanly on `q`, restores the
  alternate screen and releases mouse capture; the keymap overlay renders from the same table the
  handler uses; clicks map to transport actions and meter-row selection; and `--probe` observes
  live meter signal (peak 0.88) exactly as the iced spike does.
- **The timeline exists and is measured** (`--wave <file.wav>`, `--script <script>`): a real
  arrangement drawn as braille min/max envelopes — one lane per track, per-clip boundaries and
  colour, fades and gain drawn from the clip's own fields — with a ruler, zoom/scroll, a playhead
  and a visual-mode selection. It answers the "audiofile view" question with a working artifact and
  measured numbers (98.917 ms/col fitted for a 9 s arrangement in a 100×26 terminal; 5.333 ms/col
  floor set by the 256-sample peak bin), and it produced the clearest next step in the evaluation:
  **a raw-sample read path** for the zoomed-in window, since no renderer improvement can go below
  the data's resolution.
- **Edits are commands, not UI actions.** `x` and `d` build an `arrange …` line and hand it to the
  host's **own parser** (`host::parse_arrange_line`), so a key runs exactly what a script writes,
  the engine logs it like any other command, and a refused op (a split at frame 0) is reported in
  the state line and never logged. This is the [modal editing model](2026-09-21-modal-editing-model.md)'s
  "keys and `:` share one vocabulary" claim holding for two operations already — the `:` prompt is
  now only a widget away.
- **Panel focus exists** (`Tab`/`Shift-Tab`, click-to-focus, a lit border), so `j`/`k` mean
  "channel" or "active track" depending on where the keys are pointed — the structure a multi-panel
  editor needs, and the reason the mixer and the timeline can both be live in one key space. It pays
  for itself immediately: `+`/`-`/`0` zoom and fit the timeline, and ride and reset a fader in the
  mixer, without a modal prefix.
- **The mixer is a console, not a bar chart** (the human's ask): with an arrangement loaded it takes
  the right-hand column as channel strips — a fader with a visible position, a meter showing the
  host's level beside it, a name with mute/solo flags, a value, and a master strip. Keys ride it
  (`+`/`-`/`0`, `M`/`S`), the mouse can click/drag a strip, and each change goes to the host as a
  logged `set_param` — so **a fader ride is automation in the session log**, which is what a moving
  fader on a real console records.
- **And the console is a *view of the log*, not a mirror of it.** The Host API gained a `params`
  value — the session's parameters **folded from the log** — and the shell adopts it on every load
  and refresh, so a script's `ch0.gain 0.3` shows as 0.30, and `u`/`Ctrl+r` (undo/redo, which the host
  implements by **replaying the log**) restore the arrangement *and* leave the faders exactly where
  the log says they are. The shell stores no engine state; the decision and its alternatives are in
  the [fold-of-the-log note](../../implemented/architecture/2026-09-21-shell-state-is-a-fold-of-the-log.md).
- **A loaded file is audible, not just drawn.** `--wave <file.wav>` no longer builds a shell-side
  arrangement: it synthesises the one-clip script a user would write, hands it to the host, and draws
  the host's value — so `space` plays it (verified headlessly: `--probe --wave` reports the file's own
  amplitude on ch0 and the equal-power centre-pan law on the master). The earlier caveat that the
  timeline was not wired to the transport is gone.
- **A rate-mismatched file plays instead of stopping the transport.** `--wave` on a 44.1 kHz file
  used to stop at play (`source is 44100 Hz but the session is 48000 Hz`) — the refusal is the
  clock-honesty guard, but it had no import door. The pool now owns the boundary
  (`Pool::import`/`Pool::conform`, a band-limited resampler; `set_pool` conforms as it adopts), and
  `--wave` imports into a **session-owned** pool at the session rate rather than pointing `pool` at
  the user's directory — see the
  [session-rate note](../../implemented/architecture/2026-09-22-session-rate-and-source-conversion.md).
- **Visual mode has a real object.** `v` anchors a selection at the playhead, `h`/`l` extend it, the
  state line shows the span and its duration, and `Esc` leaves — with `Esc` no longer quitting (the
  safety key must not be destructive; `q`/`Ctrl+c` are the way out).
- **Verification is cheap and CI-able**: twenty-two tests assert the rendered buffer (including the
  timeline panel, the zoom floor and the visual-mode transition) through `TestBackend`, `--dump`
  prints a deterministic frame (the README embeds one), and a pty harness covers the real terminal
  path.
- **Two findings already, both from running it rather than testing it**: a zero-height terminal
  panicked the meter view (fixed in the spike; the class of bug matters for a UI that must survive
  arbitrary terminal sizes), and the live host's underrun counter jumps once by ~2–3 s of device
  frames on the first transport command — recorded as a
  [proposed bug-fix note](../bug-fix/2026-09-21-live-host-underrun-burst-on-transport-change.md).
- **The shipped shell is unchanged.** Tauri + Vue remains the app; iced and ratatui are spikes
  until criteria 2–5 above are answered.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21.
