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

- **an explicit key scheme**, with a `?` overlay rendered from the same table the
  key handler uses, so the help cannot drift from the behaviour;
- **mouse support** — clickable transport buttons, click-to-select meter rows,
  wheel = seek — so *you* can judge whether a mouse in a terminal is workable or
  the keys carry it (the open question the iced comparison raises);
- **command latency in the UI thread** (`last command: seek (13.4 ms …)`), because
  the host's `TransportSeek` is O(target) and a shell that binds it to a key owns
  that stall.

Not in scope: a timeline, waveforms, clip editing, dialogs. Same as iced.

## Run it

```sh
cd spikes/tui-shell

cargo run                 # the TUI (any terminal)
cargo run -- --keys       # (same as pressing ? in the app)
cargo run -- --probe      # headless: host thread + transport + live meters, no TTY
cargo run -- --dump       # render one deterministic frame and print it as text
cargo run -- --dump --keys# …the keymap overlay instead
cargo test                # 5 tests; asserts the rendered frame, no TTY needed
```

`--probe` and `--dump` are the two halves of a CI-able check: the probe exercises
the live host, the dump exercises the view (through ratatui's `TestBackend`, so
it needs no terminal), and the tests assert on the rendered buffer.

## The keymap

| keys | meaning |
|---|---|
| `space` | play / stop |
| `s` | stop |
| `r` / `Home` | rewind to 0 |
| `,` / `.` | seek −1 s / +1 s (stops first: seek is rebuild-to-target) |
| `j` / `k` / `↑` / `↓` | select the next / previous channel |
| `m` | toggle mouse capture |
| `?` | the keymap |
| `q` / `Esc` / `Ctrl+c` | quit |
| mouse | click Play/Stop/Rewind · click a meter row to select · wheel = seek |

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
