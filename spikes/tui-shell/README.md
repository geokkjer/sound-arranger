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

- **an arrangement you can hear** — `--script <host script>` loads the session and
  draws the **clips and tracks the host holds** (the engine's own `Timeline`, not a
  copy): braille min/max envelopes, one lane per track, boundaries, per-track
  colour. `--wave <file.wav>` **imports the file into the spike's own session
  pool** at the session rate (a 44.1 kHz file is resampled once — see
  `media::Pool::import`), hands the host a one-clip script and draws the result —
  so a loaded file is **audible**, and the panel is never a picture of something
  the engine has not got;
- **edits that go through the host's own language** — `x` splits and `d` deletes
  by building an `arrange …` line and handing it to the host's parser, so a key
  runs exactly what a script writes, and the engine logs it like any command;
- **panel focus** — `Tab`/`Shift-Tab` move between the mixer and the timeline, the
  focused panel shows a lit border, and `j`/`k` mean "channel" or "track"
  depending on where you are;
- **a console, not a bar chart** — with a timeline loaded the mixer moves to the
  **right** and becomes channel strips: a fader with a visible position, a meter
  beside it, a name and a value, mute/solo flags, and a master strip. Ride it with
  `+`/`-`, reset with `0`, click or drag a strip with the mouse; every change goes
  to the host as a logged `set_param` (see below);
- **an explicit key scheme**, with a `?` overlay rendered from the same table the
  key handler uses, so the help cannot drift from the behaviour;
- **mouse support** — click a panel to focus it, a lane to make that track active
  and seek, the ruler to seek, a meter row to select, the transport to play;
- **command latency in the UI thread** (`last command: seek (92.1 ms …)`), because
  the host's `TransportSeek` is O(target) and a shell that binds it to a key owns
  that stall — measured, not feared.

Not in scope yet: mouse dragging (clips move by key: `H`/`L` for time, `J`/`K`
for tracks), fades by mouse, the keymap as loadable data, and auditioning (`play`
renders the session; the panel draws the arrangement). Move, trim, fade, gain and
the `:` command line are all in — and every one of them is a line in the host's
own text format.

## Run it

```sh
cd spikes/tui-shell

cargo run                            # the TUI (any terminal)
cargo run -- --wave /tmp/demo.wav     # …with a one-clip timeline for a file
                                      # (imported into $TMPDIR/tui-shell-pool-<pid> at 48 kHz,
                                      #  so a 44.1 kHz file plays at the right pitch)
cargo run -- --script /tmp/arr.script # …with the clips/tracks a host script builds
cargo run -- --keys                  # (same as pressing ? in the app)
cargo run -- --probe                 # headless: host thread + transport + live meters, no TTY
cargo run -- --probe --wave f.wav     # …and assert that the *loaded file* renders (meters move)
cargo run -- --dump                  # render one deterministic frame and print it as text
cargo run -- --dump --script f.script # …including the arrangement (deterministic)
cargo test                           # 22 tests; asserts the rendered frame, no TTY needed
./scripts/pty-check.sh               # the real terminal path, asserted
```

`--probe` and `--dump` are the two halves of a CI-able check: the probe exercises
the live host, the dump exercises the view (through ratatui's `TestBackend`, so
it needs no terminal), and the tests assert on the rendered buffer.

## The timeline (clips, tracks, and the audiofile view)

The "audiofile view" a text editor has no analogue for, the first object for visual
mode, and — with `--script` — a real arrangement: **one lane per track, one braille
envelope per clip, clip boundaries, per-track colour**.

`--script <file>` is the interesting mode: the file is a normal **host script**
(`pool` + `arrange` lines), the host loads it, and the panel draws the arrangement
value the host hands back. The clips you see are the engine's clips; an edit from
the shell is dispatched through the host's own parser (`host::parse_arrange_line`)
as a `HostCommand::Arrange`, so **a key runs exactly what a script writes** and the
engine logs it like any other command.

```sh
mkdir -p /tmp/pool && cd /tmp/pool      # put a few WAVs named after their ids
# … s1.wav s2.wav s3.wav …
cat > /tmp/arr.script <<'EOF'
host v1
mount mixer channels=4
pool /tmp/pool
arrange add_track t0
arrange add_clip t0 c0 s1 0 192000 0 0 0 1.0
arrange add_clip t0 c1 s2 0 96000 288000 0 0 0.8
arrange add_track t1
arrange add_clip t1 c2 s3 0 144000 48000 0 0 1.0
EOF
cargo run -- --script /tmp/arr.script
```

What it does, and what it honestly cannot:

| | |
|---|---|
| **Envelope** | min/max per column from the media engine's **peak pyramid** (`PeakBuilder::range_minmax`) — never raw samples. Silence draws as the centre line. |
| **Clips** | the engine's clip fields (`at_frame`, `src_start`, `src_len`, `gain`, fades, `loop_len`): the envelope is read from the clip's source window, `gain` and the **fades are drawn** (a column inside a fade is scaled by its ramp), and a baked loop wraps. Boundaries are the `▏`/`▕` edges in white; colour is per track. |
| **Edit ops** | `x` razor-splits the clip under the playhead on the active track; `d` deletes it; `n`/`N` jump to the next/previous clip. A refused op (e.g. a split at frame 0) is reported in the state line and never logged. |
| **Resolution** | Braille is 2×4 sub-cells, so a 100-cell panel is 200 time positions × 4× vertical. |
| **Zoom floor** | **one base bin per cell — 256 frames, 5.333 ms at 48 kHz.** The pyramid walks whole 256-sample bins, so a narrower column would redraw the same bin and claim detail the data does not have. The human's read is that this is already enough resolution; going finer needs a raw-sample read path (how `tui-wave` reaches single samples). |
| **Zoom ceiling** | the whole arrangement **plus margin** (4× the fitted density), so a piece can be seen with room around it. The state line always shows the current `ms/col`, so the density claim is visible, not asserted. |
| **Lanes** | rows are shared out: `min(4, rows ÷ tracks)` per track, so 3 tracks in a 26-row terminal get 2 rows each and a tall terminal gives envelopes more amplitude resolution. |
| **Selection** | `v` anchors a frame span at the playhead, `h`/`l` extend it, the state line shows the span and its duration, `Esc` leaves. Mode and focus are always on screen. |
| **Not yet** | registers/repeat/macros, the keymap as loadable data, the prefix infobox, a command palette, auditioning, and drag-to-move — `play` renders the session while the panel draws the arrangement, so the playhead is the *session* clock against the arrangement grid (they line up when the arrangement is what the session renders). |
| **Empty tracks are lanes** | every host track draws as a lane, including one with no clips: a track is a place a clip can be moved *to*, and a lane that disappeared when its last clip left made `J`/`K` and the active-track cursor disagree with the host's own track list (found by wiring `J`). |

### The `:` command line

Every key in this shell is an `arrange` / `set_param` / `transport` line dispatched through the
host's own parser — so the command line is not a separate feature, it is the **same vocabulary**
typed instead of pressed:

```
: arrange set_clip_gain t0 c0 0.25      # a line with no key of its own
: transport seek 96000                  # the transport
: set_param mixer ch0.gain 0.5          # the console reads it back out of the log
```

One line is one command; the prompt wraps it in the `host v1` header the format requires and hands
it to `host::parse_script`, so a key, a script line and a typed line are the same command with the
same logging and the same errors. A **gesture** — several arrangement ops that are one user action,
like `t`'s two trims — is one command too (`group begin` … `group end`), so it is one undo step:
the host validates the whole group before applying any of it. A refused line is shown and **not logged**. `↑`/`↓` walk the
history (including the line that failed, which is the one worth fixing). The status line becomes
the command line while it is open, and the mode badge reads `COMMAND` — a modal UI that hides its
mode is a trap.

This is what makes the vocabulary *complete* before the keys are: the modal editing note's point
that the `:` prompt is "a widget away, not a project" — and it is now built.

### Measured, from the demo above

`--probe --wave sections.wav` (a file whose first second is a 0.8-amplitude tone) reports
**ch0 = 0.8000** and **master = 0.5657** — the file's own amplitude, and the equal-power centre-pan
law (0.8 × 1/√2) on the way to the stereo master. That is the headless proof that a loaded file is
audible: a silent host's meters only move if the arrangement rendered through the mixer.

`--probe --wave /tmp/rate44.wav` (a 1-second 440 Hz file at **44.1 kHz**) reports
`imported rate44 — resampled 44100 → 48000 Hz, 48000 frames`, then **ch0 = 0.5000** and
**master = 0.3535** — the same file, at the session rate, played instead of refused (before the
pool boundary existed this stopped the transport: `pump error: … source is 44100 Hz but the session
is 48000 Hz`).

A 9-second arrangement, 3 tracks, 5 clips, rendered in a 100×26 terminal:
**98.917 ms/cell** fitted (≈10 cells per second), playhead and ruler legible, all
five clips distinguishable by colour and boundary. A seek from 0 to 6 s (the `n`
motion) blocked the UI thread for **~92 ms** — that is `TransportSeek`'s
`O(target)` rebuild, visible in the shell rather than theorised about.


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

## The mixer

With an arrangement loaded, the console takes the right-hand column and the
timeline takes the rest — the desk layout, not a stack of panels. Each strip is a
**fader** (`░` above the handle, `█` the handle, `│` below), a **meter** showing
what the engine is publishing next to it, the channel name with a mute/solo flag
(`ch0M`, `ch1S`), and the value. The last strip is the master.

| | |
|---|---|
| **The host owns the audio *and* the values** | Every change is sent as `set_param mixer ch<n>.gain …` through the host's own text format, and the strips are **read back from the log** (the Host API's `params` value is a fold of the session log). Load a script that sets `ch0.gain 0.3` and the strip shows 0.30; `u` (undo) replays the log and returns the clip while the faders stay exactly where the log says they are. |
| **A fader ride is automation** | Every step is a logged `set_param` at its frame, which is exactly what a moving fader on a real console records. `last command: set_param (0.1 ms in the UI thread)` — unlike a seek, a parameter change is cheap. |
| **Keys** | `+`/`-` ride the selected fader by 0.05, `0` returns it to unity, `M` mutes, `S` solos, `j`/`k` (or `Tab` then the arrows) pick the strip. |
| **Mouse** | Click or drag anywhere on a strip: the fader follows the row under the pointer, and the strip becomes the selected one. |
| **Undo is a log replay** | `u` / `Ctrl+r` undo and redo the last arrangement edit; the host rebuilds by replaying the log and the shell re-reads the arrangement *and* the parameters. A fader ride is not an arrangement edit, so undoing a split leaves the faders alone — which is the correct answer, and now a visible one. |

Without an arrangement there is no console column (only one panel would be
pointless), so the shell falls back to the wide meter bars under the transport.

## The keymap

| keys | meaning |
|---|---|
| `space` | play / stop (global, in any panel/mode) |
| `s` | stop |
| `r` / `Home` | rewind to 0 |
| `,` / `.` | seek −1 s / +1 s (stops first: seek is rebuild-to-target) |
| `Tab` / `Shift-Tab` | move the focus ring: mixer ⇄ timeline |
| `j` / `k` / `↑` / `↓` | the **focused** panel: mixer channel, or active track |
| `n` / `N` | timeline: jump the playhead to the next / previous clip |
| `u` / `Ctrl+r` | undo / redo the last arrangement edit (a log replay) |
| `v` | **visual mode**: anchor a selection at the playhead |
| `h` / `l` | timeline: scroll — in visual mode, extend the selection instead |
| `+` / `-` | timeline: zoom in / out · **mixer: ride the selected fader** |
| `0` | timeline: fit · **mixer: fader to unity** |
| `M` / `S` | **mixer: mute / solo** the selected channel |
| `x` | timeline: razor-split the clip under the playhead |
| `d` | timeline: delete the clip under the playhead |
| `<` / `>` | timeline: trim the clip's **start / end to the playhead** |
| `t` | **visual**: trim the clip to the selection (consumes the selection; **one undo**) |
| `H` / `L` | timeline: move the clip **one beat** earlier / later (clamps at 0) |
| `J` / `K` | timeline: move the clip to the track **below / above**, keeping its time |
| `g` / `G` | timeline: clip gain **−1 dB / +1 dB** (the range is the console fader's) |
| `f` / `F` | timeline: fade in / fade out **to the playhead** (absolute, not a nudge) |
| `Esc` | leave visual mode / close this overlay — **never quits** |
| `m` | toggle mouse capture |
| `:` | **the command line**: type any `host v1` line (Enter runs, Esc cancels, `↑`/`↓` history) |
| `?` | the keymap (modal, scrollable with `j`/`k`) |
| `q` / `Ctrl+c` | quit |
| mouse | click a panel to focus · a lane = that track + seek · the ruler = seek · **drag a mixer strip = set that fader** · Play/Stop/Rewind · wheel = seek |

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
