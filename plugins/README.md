# plugins/ — external integration (placeholder)

**Direction changed 2026-09-21.** This directory is *not* where we build plugin or sidecar
artifacts. The decision is
[integration is external programs and devices, not in-process sidecars](../.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md):

- **No VST3/CLAP builds** of our capabilities and **no CDP sidecar binary**. Capabilities that
  already exist as better standalone programs (TidalCycles, VCV Rack, CDP8, sox, ffmpeg,
  `tui-wave`) are **driven as processes**: we provide the clicks and the timing, we record their
  audio into the media pool as takes, and offline processors run on rendered sources whose output
  comes back as a new source.
- **Hardware is treated the same way** (the M-AVE FM-1, the EP-133, …): capture its output, send it
  clock/MIDI. The device-side facts — which box takes MIDI clock vs an analog pulse vs DIN sync,
  which interface can emit a usable pulse, what cables — are **gear research** and live in the
  studio project
  (`~/Projects/music/music-composition-theory/studio/instruments/<name>/`), referenced from here,
  never copied.
- **"Everything is a plugin" still describes the engine** (the `Plugin` trait, the graph, the
  session log). This decision is about the *product* boundary, which is now process/device-level
  rather than binary-level.

Nothing is built here yet. When the first integration lands, it lands as a **contract** — how the
session starts/addressing a partner, what sync it takes, how its audio arrives, how a take is
logged with its provenance — with an `#[ignore]`d integration test and a documented manual
procedure, not as a plugin wrapper.
