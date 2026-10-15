# spikes/iced-shell — the iced evaluation spike

**Status: an evaluation, not a shell.** This is the minimal proof behind the
[iced-shell evaluation note](../../.agents/notes/proposed/architecture/2026-09-21-iced-shell-evaluation.md):
can [iced](https://github.com/iced-rs/iced) be a sound-arranger shell — in-process
Rust over the same Host API the retired Tauri + Vue shell drove? The shell decision
itself (Tauri tried and decided against; iced and ratatui are the shells) is the
[shells note](../../.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md).

It is its **own workspace** on purpose: iced and wgpu must never enter the core's
`cargo build --workspace`. Delete this directory and nothing else changes.

## What it proves (and what it does not)

In scope:

- the window opens (iced 0.14, winit + wgpu, niri/Wayland + Mesa);
- the shell owns a `host::live::HostHandle` — the *same* actor the Tauri bridge
  used — and never touches the render path: no IPC, no webview, no serde wire;
- play / stop / rewind (space, `r`) drive the transport;
- channel + master meters and the position readout follow the audio at 60 Hz;
- **it speaks the shared workflow**: modes, keys, the `:` command line and the
  keymap overlay all come from the `workflow` crate — *the same table the ratatui
  shell reads* — so the two shells cannot grow two workflows. iced translates its
  key events into the workflow's neutral `Key`; what a key means is not this shell's
  business. Actions iced cannot express *yet* (split/trim/move a clip) **report
  themselves** in the status line instead of doing nothing: the parity debt is
  visible on purpose, because the rule is "no shell-only features";
- **the mixer is real stock-widget material**: `iced_audio 0.17` `VSlider`s on a
  dB range (`DBRange`, derived from the mixer's **declared** gain bounds → −60…+6.0 dB,
  unity at ~91 % of the travel), one fader per
  channel plus the master, each with a live meter and a dB readout. Dragging a
  fader sends the host a logged `set_param`, and the positions come back from the
  host's **parameter fold** — the log, not the widget's memory;
- **the timeline draws the material**: one lane per track, each clip a rectangle cut
  into its `at_frame`/`src_len` region, with the source's **real envelope** inside it —
  read from the pool's persisted `.peaks` pyramid through the pool directory the
  snapshot publishes, one level-aware query per point column, the level chosen from the
  zoom. A clip whose region starts mid-source draws that region (the case a waveform
  gets wrong), a reversed clip mirrors its range, a looped clip wraps it. `--wave`
  imports a piece so there is something real to draw;
- **the timeline has a viewport**: zoom around the playhead (`+`/`-`), scroll a screen
  eighth (`h`/`l`), fit (`0`), and click anywhere in the plot to seek (snapped to the
  workflow's grid when one is armed). `Tab` moves the focus between console and
  timeline — the workflow's own split, where `+ - 0` mean "zoom / fit" or "ride /
  unity" depending on the focused panel;
- **the frame budget is measured, not claimed**: `--perf` plays the loaded piece and
  reports the redraw cadence, the stalls by value, and the envelope columns per redraw. On
  this host, with a 30 s stereo piece (2 tracks, 6 clips, 1,628 envelope columns per redraw)
  and Mesa's **software** fallback renderer: **steady p50 16.7–17.0 ms (59–60 fps), p95
  18.7–30.4 ms** across three runs with the transport running. The demo profile (no clips —
  0 envelope columns) measures p50 13.3 ms (75 fps) in a clean run, so the envelope drawing
  costs a few milliseconds and still fits a display-rate frame. Some runs show redraw **gaps**
  of 0.4–1.0 s; they appear with 0 envelope columns too, which points at the *compositor*
  (a window it is not showing receives no frame callbacks) rather than the draw — `--perf`
  prints the gaps by value instead of averaging them away, and says so.

![the iced spike's mixer: four channel faders and a master, the master at -1.9 dB
because the demo script sets `master.gain 0.8`](mixer-fold.png)

The same shell with the shared keymap open (`cargo run -- --keys`) — the rows are
`workflow::help()`, the identical table the TUI renders:

![the iced spike's keymap overlay, rendered from the shared workflow
table](workflow-keys.png)

The screenshot is the boot state of the demo profile: `ch0…ch3` at +0.0 dB and
**master at −1.9 dB**, which is `20·log10(0.8)` — the value the *script* set, read
out of the host's log fold and shown on an `iced_audio` fader. (Under Wayland at
scale 2, so the shot is 2× the logical layout.)

Deliberately **not** in scope: clip editing, the pool panel, text-heavy layout,
menus/dialogs. Those are the surfaces that actually decide iced against ratatui, and
the note records them as the follow-up.

## Run it

```sh
cd spikes/iced-shell

cargo run                          # the window (needs a display and an audio device)
cargo run -- --keys                # …with the shared keymap overlay open
cargo run -- --wave <file.wav>     # import a piece into the session pool and draw it
cargo run -- --wave <file.wav> --perf   # …and measure the frame budget while it plays
cargo run -- --probe               # headless: host thread + transport + meters, no window
cargo test                         # key translation, the command line, the timeline maths
```

## Keys, and what is not wired yet

`space` `s` `r` `,` `.` drive the transport; `Tab` moves the focus between the console
and the timeline; `j`/`k` select a console strip (or the timeline's active track) and
`+`/`-`/`0` ride that fader / zoom that viewport (or return it to unity / fit the
piece); `h`/`l` scroll the timeline; a **click in the timeline seeks**; `u`/`Ctrl+r`
undo/redo through the log; `:` opens the command line (any `host v1` line, Enter runs,
Esc cancels, ↑/↓ history); `?` shows the keymap; `Esc` cancels. The rest of the
workflow's table — `x` `d` `<` `>` `t` `H` `L` `J` `K` `g` `G` `f` `F` `v` `n` `N` — is
the **editing** half, and each one says so in the status line when pressed. That is
deliberate: the workflow is shared, so the gap is a *rendering and editing* gap, and it
is visible.

`--probe` is the CI-able half: it boots a silent host (wall-clock pump, no audio
device), loads the demo profile, plays, polls the published snapshot for 600 ms,
and exits non-zero unless it observed meter signal.

## The demo profile, and `--wave`

The **demo profile** is the `docs/FIRST_SESSION.md` chain, inlined in `src/main.rs`:
euclidean → scale → tone → mixer channel 0, with the master fader at 0.8. It exists so
the meters have signal the moment the window opens and so the fader readout has a value
the shell did **not** choose — the log did. It has no clips, so the timeline is a lane
bed until something is placed.

`--wave <file.wav>` is that something: it imports a **copy** into the spike's own session
pool at the session rate (`target/iced-pool`, the same pool `--record-check` uses), one
source per channel, each cut into three **consecutive** clips — consecutive so the piece
plays like the file, and cut up so the canvas has clips whose `src_start` is not 0, which
is where a peaks mapping goes wrong. `--perf` loads a wave, plays it, measures the redraw
cadence for six seconds and prints the numbers (it needs a display; like `--sweep` and
`--record-check` it is a self-driving check, because a GUI cannot be scripted from
outside). Run it **without** `--wave` too: the demo profile has no clips, so the same
measurement with 0 envelope columns is the control that tells a slow draw apart from a
compositor that is not showing the window.

## The peaks path, and where it lives

A clip's `source` is a pool id, and the pool publishes its **directory** on the snapshot
(`Snapshot::pool_dir`) — so a shell resolves the id through the pool's own guarded lookup
(`Pool::path_for`, which refuses a crafted id) and reads the `.peaks` sidecar beside the
file (`PeakFile::sidecar_path`, the one home of that naming). The shell does **not**
re-read the WAV: `PeakFile::load` gives it the persisted pyramid, and
`Peaks::level_for`/`range_minmax_at` answer an envelope query in one or two bins per
column at any zoom. The pyramids are cached per source in the shell (a rebind drops the
cache), because the draw runs every frame and a file read per frame is I/O.

Failures are states: a source with no readable sidecar is drawn as a **muted** bar (never
as silence), counted in the timeline caption, and its reason is said once in the status
line.

## Notes from wiring `iced_audio` in

- It tracks iced exactly (`iced_core`/`iced_graphics` 0.14 for `iced_audio` 0.17,
  2026-09-09), so the widget crate cannot drift from the shell's iced.
- iced needs the `canvas` **and** `image` features enabled for it (both off by
  default); `canvas` is the widget, `image` is what makes the widget's `Element`
  bound hold on the wgpu + tiny-skia fallback renderer.
- The widget's contract is a **normalized parameter** (`NormalParam`) plus a
  *range* that owns the unit mapping (`DBRange::map_db` / `unmap_to_db`,
  `FreqRange`, `IntRange` with `snap`). The host's `set_param` takes the value in
  engineering units (a linear gain), so `unmap_to_db` → `10^(db/20)` is the bridge
  — one function, tested.
- `set_param`'s parameter *name* is `&'static str` on the command, so a shell
  needs a static name table (`GAIN_PARAMS`) and an explicit strip↔name mapping;
  the master's fader index is the channel count, which is the one off-by-one worth
  a test.
