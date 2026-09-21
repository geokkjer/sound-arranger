# spikes/iced-shell — the iced evaluation spike

**Status: an evaluation, not a shell.** This is the minimal proof behind the
[iced-shell evaluation note](../../.agents/notes/proposed/architecture/2026-09-21-iced-shell-evaluation.md):
can [iced](https://github.com/iced-rs/iced) be a *second* sound-arranger shell —
in-process Rust over the same Host API the Tauri + Vue shell drives?

It is its **own workspace** on purpose: iced and wgpu must never enter the core's
`cargo build --workspace`. Delete this directory and nothing else changes.

## What it proves (and what it does not)

In scope:

- the window opens (iced 0.14, winit + wgpu, niri/Wayland + Mesa);
- the shell owns a `host::live::HostHandle` — the *same* actor the Tauri bridge
  uses — and never touches the render path: no IPC, no webview, no serde wire;
- play / stop / rewind (space, `r`) drive the transport;
- channel + master meters and the position readout follow the audio at 60 Hz.

Deliberately **not** in scope: the timeline canvas, waveform drawing, clip
editing, text-heavy layout, menus/dialogs. Those are the surfaces that actually
decide iced against Tauri + Vue, and the note records them as the follow-up.

## Run it

```sh
cd spikes/iced-shell

cargo run              # the window (needs a display and an audio device)
cargo run -- --probe   # headless: host thread + transport + meters, no window
```

`--probe` is the CI-able half: it boots a silent host (wall-clock pump, no audio
device), loads the demo profile, plays, polls the published snapshot for 600 ms,
and exits non-zero unless it observed meter signal.

## The demo profile

The `docs/FIRST_SESSION.md` chain, inlined in `src/main.rs`: euclidean → scale →
tone → mixer channel 0, with the master fader at 0.8. It exists so the meters
have signal the moment the window opens — the spike is about live values
reaching widgets, not about the final mixer look.
