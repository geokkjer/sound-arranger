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
  business. Actions iced cannot express *yet* (split/trim/move a clip — it has no
  timeline) **report themselves** in the status line instead of doing nothing: the
  parity debt is visible on purpose, because the rule is "no shell-only features";
- **the mixer is real stock-widget material**: `iced_audio 0.17` `VSlider`s on a
  dB range (`DBRange`, −60…+12 dB, unity at ~83 % of the travel), one fader per
  channel plus the master, each with a live meter and a dB readout. Dragging a
  fader sends the host a logged `set_param`, and the positions come back from the
  host's **parameter fold** — the log, not the widget's memory.

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

Deliberately **not** in scope: the timeline canvas, waveform drawing, clip
editing, text-heavy layout, menus/dialogs. Those are the surfaces that actually
decide iced against ratatui, and the note records them as the follow-up.

## Run it

```sh
cd spikes/iced-shell

cargo run              # the window (needs a display and an audio device)
cargo run -- --keys    # …with the shared keymap overlay open
cargo run -- --probe   # headless: host thread + transport + meters, no window
cargo test             # key translation, the command line, the console keys
```

## Keys, and what is not wired yet

`space` `s` `r` `,` `.` drive the transport; `j`/`k` select a console strip and
`+`/`-`/`0` ride it (or return it to unity); `u`/`Ctrl+r` undo/redo through the
log; `:` opens the command line (any `host v1` line, Enter runs, Esc cancels, ↑/↓
history); `?` shows the keymap; `Esc` cancels. The rest of the workflow's table —
`x` `d` `<` `>` `t` `H` `L` `J` `K` `g` `G` `f` `F` `v` `n` `N` — needs the timeline
canvas, and each one says so in the status line when pressed. That is deliberate:
the workflow is shared, so the gap is a *rendering* gap and it is visible.

`--probe` is the CI-able half: it boots a silent host (wall-clock pump, no audio
device), loads the demo profile, plays, polls the published snapshot for 600 ms,
and exits non-zero unless it observed meter signal.

## The demo profile

The `docs/FIRST_SESSION.md` chain, inlined in `src/main.rs`: euclidean → scale →
tone → mixer channel 0, with the master fader at 0.8. It exists so the meters have
signal the moment the window opens and so the fader readout has a value the shell
did **not** choose — the log did.

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
