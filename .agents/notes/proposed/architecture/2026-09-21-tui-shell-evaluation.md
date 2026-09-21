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
   covers the real path (raw mode, alternate screen, mouse capture, input decoding).

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
2. **The timeline question is answered with a built prototype**, not an opinion: draw N clips with
   their waveform peaks in a terminal (braille/block codepoints, and — if the target terminal
   supports it — the Kitty graphics protocol), state the measured density (clips × seconds of
   material visible per screen), and demonstrate the editing gestures (select, move, split, trim)
   with the keyboard alone.
3. **A task-level mouse-vs-keys verdict**: the same short task list (select a clip, seek, split,
   nudge, mute a channel) performed both ways, with the friction noted — including the terminal
   facts that matter (mouse capture vs copy/paste, tmux/niri passthrough, wheel vs key repeat).
4. A **widget inventory** for the TUI: what the shell needs (dialogs? text input? menus?) against
   what a terminal provides well. This is *expected to be smaller* than iced's answer, and that
   expectation has to be tested against the surfaces the app actually has.
5. The transport/meters seam is exercised **only** through `HostCommand` + `Snapshot` — the shell
   compiles without touching `HostSession`, exactly as required of the iced spike.

## Risks

- **The graphics ceiling is the real risk.** A terminal cannot draw a waveform at DAW fidelity
  without an image protocol; if the timeline degrades to a list, the product is a different (much
  smaller) instrument. This, not performance, is what decides it.
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
- **Verification is cheap and CI-able**: five tests assert the rendered buffer through
  `TestBackend`, `--dump` prints a deterministic frame (the README embeds one), and a pty harness
  covers the real terminal path.
- **Two findings already, both from running it rather than testing it**: a zero-height terminal
  panicked the meter view (fixed in the spike; the class of bug matters for a UI that must survive
  arbitrary terminal sizes), and the live host's underrun counter jumps once by ~2–3 s of device
  frames on the first transport command — recorded as a
  [proposed bug-fix note](../bug-fix/2026-09-21-live-host-underrun-burst-on-transport-change.md).
- **The shipped shell is unchanged.** Tauri + Vue remains the app; iced and ratatui are spikes
  until criteria 2–5 above are answered.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21.
