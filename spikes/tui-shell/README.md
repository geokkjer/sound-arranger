# spikes/tui-shell — the ratatui evaluation spike

**Status: an evaluation, not a shell.** This is the terminal counterpart of
[`spikes/iced-shell`](../iced-shell/), behind the same question: what should the
sound-arranger shell be? It drives the *same* `host::live::HostHandle` the Tauri
bridge uses — in-process, no IPC, no webview — and was written to the **same
scope** as the iced spike so the two can be compared directly.

The evaluation note is
[`.agents/notes/proposed/architecture/2026-09-21-tui-shell-evaluation.md`](../../.agents/notes/proposed/architecture/2026-09-21-tui-shell-evaluation.md).
It is its own workspace on purpose: the terminal backend must never enter the
core's `cargo build --workspace`. Delete the directory and nothing else changes.

## What it proves, and what it is testing

In scope (identical to the iced spike):

- it runs in a terminal (the TUI's version of "the window opens");
- transport — play / stop / rewind / seek — drives the host;
- channel + master meters and the position readout follow the audio.

What this spike adds, because it is what a TUI has to answer for:

- **a timeline** — `--wave <file.wav>` draws a real audio file as a **min/max
  envelope in braille**, with a ruler, a viewport you can zoom and scroll, a
  playhead, and a **visual-mode selection** over a frame span (see below);
- **an explicit key scheme**, with a `?` overlay rendered from the same table the
  key handler uses, so the help cannot drift from the behaviour;
- **mouse support** — clickable transport buttons, click-to-select meter rows,
  click the timeline to seek, wheel = seek — so *you* can judge whether a mouse in
  a terminal is workable or the keys carry it;
- **command latency in the UI thread** (`last command: seek (13.4 ms …)`), because
  the host's `TransportSeek` is O(target) and a shell that binds it to a key owns
  that stall.

Not in scope: clips (there is one file, not an arrangement), editing that changes
audio, dialogs.

## Run it

```sh
cd spikes/tui-shell

cargo run                          # the TUI (any terminal)
cargo run -- --wave /tmp/demo.wav   # …with the timeline for a file
cargo run -- --keys                # (same as pressing ? in the app)
cargo run -- --probe               # headless: host thread + transport + live meters, no TTY
cargo run -- --dump                # render one deterministic frame and print it as text
cargo run -- --dump --wave f.wav    # …including the timeline (deterministic)
cargo test                         # 13 tests; asserts the rendered frame, no TTY needed
./scripts/pty-check.sh             # the real terminal path, asserted
```

`--probe` and `--dump` are the two halves of a CI-able check: the probe exercises
the live host, the dump exercises the view (through ratatui's `TestBackend`, so
it needs no terminal), and the tests assert on the rendered buffer.

## The timeline (`--wave`)

The "audiofile view" a text editor has no analogue for, and the first object for
visual mode. What it does, and what it honestly cannot:

| | |
|---|---|
| **Envelope** | min/max per column from the media engine's **peak pyramid** (`PeakBuilder::range_minmax`) — never raw samples. Silence draws as the centre line. |
| **Resolution** | Braille is 2×4 sub-cells, so a 100-cell panel is 200 time positions × 4× vertical. |
| **Zoom floor** | **one base bin per cell — 256 frames, 5.333 ms at 48 kHz.** The pyramid walks whole 256-sample bins, so a narrower column would redraw the same bin and claim detail the data does not have. Going finer needs a raw-sample read path (that is how `tui-wave` reaches single samples); this is the next step, not a bug. |
| **Zoom ceiling** | the whole file, one column per ~1/width of it. The state line always shows the current `ms/col`, so the density claim is visible, not asserted. |
| **Selection** | `v` anchors at the playhead; `h`/`l` extend it; the statusline shows the span and its duration; `Esc` leaves. One mode, one visible indicator — a first slice of the [modal editing model](../../.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md). |
| **Not yet** | clips/tracks (one file, one lane), split/trim/move, and auditioning — the file is not wired to the transport, so the playhead shows the *session* position against the file's grid. |

A demo file with visible structure, made by the host itself:

```sh
printf 'host v1\nmount euclidean steps=8 pulses=5\nmount scale root=0 note_len=2400\nmount tone gain=0.9 blip_len=1800\nmount mixer channels=4\npatch euclidean.triggers scale.trigger\npatch scale.note tone.note\npatch tone.audio mixer.ch0\nset_param mixer master.gain 0.5 @0\nbounce 1440000 /tmp/demo.wav\n' | cargo run -q -p host
cargo run -- --wave /tmp/demo.wav
```

> **Watch the `@frame` trap when scripting one:** a command scheduled at frame *f*
> renders the session *up to* `f` first, and a `bounce` renders from wherever the
> clock now is — so several `set_param … @frame` lines followed by one `bounce`
> do not produce a file with those changes at those offsets (it starts at the last
> scheduled frame, and everything before it is never written). Bounce per section
> and join, or schedule nothing and vary the file another way.

## The keymap

| keys | meaning |
|---|---|
| `space` | play / stop |
| `s` | stop |
| `r` / `Home` | rewind to 0 |
| `,` / `.` | seek −1 s / +1 s (stops first: seek is rebuild-to-target) |
| `j` / `k` / `↑` / `↓` | select the next / previous channel |
| `v` | **visual mode**: anchor a selection at the playhead |
| `h` / `l` | scroll the timeline — in visual mode, extend the selection instead |
| `+` / `-` | zoom in / out (floor: one peak bin per column) |
| `0` | fit the whole file |
| `Esc` | leave visual mode (a second `Esc` quits) |
| `m` | toggle mouse capture |
| `?` | the keymap |
| `q` / `Ctrl+c` | quit |
| mouse | click Play/Stop/Rewind · click a meter row · click the timeline = seek · wheel = seek |

## What a deterministic frame looks like

`cargo run -- --dump` (100×26, with the synthetic demo snapshot — real audio is
not needed to review the view):

```text
┌ sound-arranger · tui spike ──────────────────────────────────────────────────────────────────────┐
│position 2.000 s   beat 4.00   tempo 120.0 bpm   frame 96000   playing                            │
│audio: 48000 Hz · 2 ch · underruns 0 · drops 0                                                    │
│selected ch1   undo yes   redo no                                                                 │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌ transport ───────────────────────────────────────────────────────────────────────────────────────┐
│Play    Stop    Rewind   space play/stop · ? keys · q quit                                        │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌ meters ──────────────────────────────────────────────────────────────────────────────────────────┐
│ch0     ██████████████████████████████████████████ ██████████████████████████████▉          0.88  │
│ch1     ██████████▏                                                                         0.12  │
│ch2                                                                                         0.00  │
│ch3     ██████████████████████████████████████████ ████████████████████████████████████     0.94  │
│master  ██████████████████████████████████████████                                          0.50  │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌ state ───────────────────────────────────────────────────────────────────────────────────────────┐
│mouse: on   last command: —                                                                       │
│demo loaded — mixer 4 ch, 8 log events, 0 underruns                                               │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
```

## What the spike already found

1. **A real crash:** a zero-height terminal underflowed the "more rows" notice
   (`attempt to subtract with overflow`). Found by running the spike under a pty,
   not by the tests — fixed, and the fix is the reason the notice is guarded.
2. **The live host's underrun counter jumps once (~95k–144k frames, ≈2–3 s) on the
   first transport command — even a pure seek while stopped — and then stays
   flat.** The TUI's audio line is what makes it visible; it is recorded as a
   proposed bug-fix note (`.agents/notes/proposed/bug-fix/`) with the reproduction.
   A shell that shows that counter as a dropout alarm will mislead its user.

## Verifying a TUI without a display

`TestBackend` covers the view; for the *real* path (raw mode, alternate screen,
mouse capture, input decoding) the spike is run under a pty. The reproducible
form is a script:

```sh
./scripts/pty-check.sh
```

It asserts: alternate screen entered *and* restored, mouse capture taken and
released, the UI actually drawn, and no panic — then exits non-zero if any of
those fail. `stty rows/cols` inside the pty matters (`script` allocates a
zero-sized terminal, which is a degenerate case to survive, not to test
through); the raw log can be replayed into a screen to read what was drawn.
