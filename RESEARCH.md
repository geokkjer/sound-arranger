# sound-arranger — Research & Architecture

> **Working title.** A tape-style **arrangement / composition** tool: record long live jams, then cut, rearrange and shape them into a finished piece, and master the result. No MIDI note sequencing.
>
> *Status: research draft, rev 4. Locked decisions are marked 🔒. Crate/license facts checked against crates.io / npm / GitHub on 2026-08-13 and listed in §14. Plugin-architecture research added 2026-08-15 (§15); minimal-core architecture reframed 2026-08-15 (§11 + note).*

---

## 0. TL;DR — recommendations on one screen

| Area | Recommendation | Why |
|---|---|---|
| **Target** | **x86 desktop first** (Linux primary; macOS/Windows via Tauri). ARM/RPi + hardware controls = a separate later phase | Best technical solution trumps; don't pre-optimize for a Pi |
| **App shell** | Tauri v2 + Vue 3 + TypeScript | Rust audio engine + web UI |
| **Architecture** | **Minimal core (clock · graph interpreter · session log · context plumbing) + everything-else-as-plugin; the product is an assembled profile** | §11, §15.7, [minimal-core note](.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md) |
| **UI library** | **Keep your `cdp-front` stack: `reka-ui` + shadcn-vue + Tailwind v4** | You already use it; headless primitives fit a bespoke DAW. Do **not** add Naive UI / PrimeVue |
| **Timeline rendering** | `<canvas>` 2D + precomputed waveform **peak pyramids** + viewport culling + offscreen clip caching | Fast and predictable; DOM-per-clip is a dead end |
| **Audio I/O** | `cpal` (in **and** out) — ALSA/JACK on Linux, CoreAudio on macOS, WASAPI on Windows | Lowest latency; owns the device directly |
| **Decode / encode** | `symphonia` (decode) · `hound` (WAV in/out) | Pure Rust |
| **Time-stretch** | `rubato` (real-time) · CDP8 pvoc / own `rustfft` stretch (offline) | Real-time vs "lush spectral" are different jobs |
| **Effects** | `fundsp` (real-time) · CDP8 as **offline sidecar** (later phase) | Dub sends in-engine; spectral rendered to disk |
| **License** | App **GPL-3.0-or-later**; CDP8 (LGPL-2.1) as a sidecar binary | Compliant + lets you embed/port anything GPL/LGPL (§12) |
| **Phase 1 (the goal)** | **Core + the sound-arranger profile: record → cut/splice → clip/loop arrange → soft mixer** | See §11 |

---

## 1. Concept & creative references

The through-line is **"composition happens in the edit, after the performance."**

- **Teo Macero** (producer/editor for Miles Davis — *Bitches Brew*, *On the Corner*): record long live sessions, then *compose* by splicing tape, looping fragments, dropping tracks in and out. The edit *is* the composition.
- **Morton Feldman**: long durations, quiet dynamics, sparse placement, non-goal-directed structure, texture/timbre over rhythm. Suggests a **seconds-based timebase** (not bars/beats), very long fades, fine gain resolution.
- **Tape artists** (musique concrète / early tape music): non-destructive splicing, varispeed, reversal, loops, layering takes.
- **Dub production** (King Tubby, Lee Perry): the mix console as instrument — aux sends into delay/reverb, "dropping" tracks to leave only the echo tail, EQ kills, fader rides recorded as automation.
- **Sony ACID (early 2000s)**: the "paint clips onto a timeline and it time-stretches them to fit" workflow — the closest mainstream ancestor for ease of use.

### Reading — the composition-theory corpus

The creative brief above is grounded in the **music-composition-theory** corpus (the Composer's Onion framework). Paths below resolve on this machine through the `~/Projects/music` symlink view — `../music/…` from this repo (see `~/Projects/music/README.md`):

- [Bitches Brew — the studio cut as composition](../music/music-composition-theory/analyses/miles-davis/bitches-brew.md) — Teo Macero's razor-blade editing; Onion Layer 6 (Time / Editing)
- [On the Corner](../music/music-composition-theory/analyses/miles-davis/on-the-corner.md)
- [Dub reggae (1968)](../music/music-composition-theory/theory/movements/1968-dub-reggae.md) — console-as-instrument; the §7 dub workflow
- [The Composer's Onion](../music/music-composition-theory/theory/framework/the-composers-onion.md) — Layer 6 = the studio cut; Layer 7 = system/process (generative)
- [Modular patching DSL sketch (Haskell)](../music/music-composition-theory/notes/modular-dsl-sketch.md) — idea source for a future scripting layer (§10 glicol)

The tool is an **arranger / tape editor**, not a groovebox or MIDI sequencer. MIDI enters only later as *control* (knobs/faders), never as *notes*.

---

## 2. Relationship to your existing rigs

| Project | Role | How it relates |
|---|---|---|
| `pi5-daisy-synth-rig` | **The jam source** (Pi 5 + JUCE mixer + Daisy voices, USB interface, JACK) | In a **later phase** the arranger can record its output. Not part of the x86 prototype. |
| `cdp-front` | Vue 3 + **reka-ui + shadcn-vue + Tailwind v4** frontend (a CDP wrapper) | **Reuse this exact frontend stack.** Its CDP-wrapping idea becomes the arranger's "offline process" tier (§6). |

Keep the arranger's **engine** as a standalone Rust crate (not Tauri-coupled), so a future headless/ARM "box mode" is possible — but don't design for it now.

---

## 3. Decisions 🔒 (rev 2)

- 🔒 **x86 desktop is the primary target** (Linux first; Tauri also gives macOS/Windows). Raspberry Pi 5 / ARM / hardware controls are a **separate, later phase**. Hobby/boutique, price not an issue → best technical solution wins.
- 🔒 **License: GPL-3.0-or-later** for the app (rationale + dependency matrix in §12).
- 🔒 **CDP8: embed the C as a sidecar CLI**, port to Rust only per-algorithm and only if a specific effect must run real-time (§6).
- 🔒 **Phase 1 scope:** audio input (record) → Acid-like cutting → clip/loop arrangement → soft mixer. No MIDI, no effects beyond the mixer.
- 🔒 **Horizontal timeline** — time flows left→right, tracks are stacked lanes.

---

## 4. Frontend

### 4.1 Stack

- **Vue 3.5.x** (`vue@3.5.41`) + TypeScript, `<script setup>` SFCs.
- **Vite** — reuse your `cdp-front` choice of `rolldown-vite` if you like it; Tauri v2 only calls your `dev`/`build` scripts.
- **Tauri v2** (`@tauri-apps/api/core`, not the v1 `tauri` module). Commands registered in `tauri::generate_handler![…]`; frontend uses `invoke()` / `listen()` / channels; permissions are **deny-by-default** via `src-tauri/capabilities/*.json`.
- On x86 the webview is a non-issue: WebKitGTK (Linux), WKWebView (macOS), WebView2 (Windows) all handle a canvas timeline comfortably.

### 4.2 UI library — reuse what you have

You asked for "a good UI library." The honest answer: **you already have it** in `cdp-front`:

- **`reka-ui`** (`2.10.3`) — headless, accessible, Radix-UI primitives ported to Vue; unstyled, so the look is yours.
- **shadcn-vue** on top — copy-in styled components (slider, menubar, sheet/sidebar, tooltip, dialog, context menu, select).
- **Tailwind CSS v4**, `@vueuse/core`, `lucide-vue-next`.

Why **not** the alternatives: **Naive UI** (2.44.1) is a fine batteries-included fallback but redundant here; **PrimeVue** (5.0.0) added a license-manager/premium tier and is more opinionated than a DAW wants; **Vuetify** is Material-locked; **Element Plus** is dated/heavy.

> **Key insight:** the UI library only styles *panels, toolbars and dialogs*. The timeline is **canvas** (§4.3), so the library has almost no bearing on timeline performance. "Ease of use + performance" is solved by *conventions + canvas*, not a cleverer component library.

### 4.3 Timeline / waveform rendering

**Rule 0: never render clips as DOM nodes.** A few thousand clips as Vue components will jank. Draw on **canvas** — layered:

1. **Static layer** — track lanes, grid, ruler. Redrawn only on zoom/scroll/layout change.
2. **Clip layer** — clips + waveforms. Each clip's waveform is rendered once into an **offscreen canvas** (per clip, per zoom bucket) and **blitted**; only viewport-intersecting clips are drawn.
3. **Overlay layer** — playhead, selection, ruler cursor. Redrawn each frame.

Supporting techniques:

- **Peak pyramids (LOD).** In Rust, precompute min/max peaks per Source at several bucket sizes (128 / 512 / 2048 / 8192 samples). Send the right level for the current zoom as a compact typed array, once, cached in JS.
- **Viewport culling.** Skip clips/peak buckets outside the view; skip peaks thinner than ~1px.
- **Dirty-flag `requestAnimationFrame`.** Redraw only what changed; never repaint static content per frame.
- **Offload peak/thumbnail generation** to a **Web Worker / `OffscreenCanvas`** (or, more robustly, generate peaks in Rust and ship them over IPC).
- **Virtualize DOM lists** (track headers, clip inspector, media pool) with `@tanstack/vue-virtual` (already a `reka-ui` dependency).

**Interaction** is pointer-events on the canvas (drag, razor-split, select, stretch edges) with `reka-ui` menus/dialogs/tooltips on top. **WebGL/PixiJS** is unnecessary now; reach for it only for per-clip spectrogram overlays.

### 4.4 IPC / data-flow rules

- **Control only over IPC** — `invoke()` commands carry JSON (`load_audio`, `record_start/stop`, `add_clip`, `set_clip`, `splice`, `set_param`, `render_offline`, `compute_peaks`, …).
- **Never stream audio samples over IPC.** Peaks arrive downsampled; audio stays in Rust.
- **Transport position** — local clock on the frontend, re-synced by sparse events (~10 Hz), not 60 Hz polling.
- **Meters / job progress** via throttled `emit` events (~30 Hz).

---

## 5. Audio engine (Rust)

### 5.1 Crates (verified 2026-08-13)

| Crate | Version | Role |
|---|---|---|
| `cpal` | 0.18.1 | Audio I/O — **input (recording) and output** |
| `symphonia` | 0.6.1 | Decode WAV/FLAC/MP3/AAC/Ogg (enable the codec features you need) |
| `hound` | 3.5.1 | WAV read/write (recording capture + bounce) |
| `rubato` | 5.0.0 | Real-time async resampling / time-stretch (SMBS) |
| `fundsp` | 0.23.0 | Audio graph + built-in effects (delay, reverb, filters, EQ) |
| `dasp` / `dasp_graph` | 0.11.0 | PCM primitives + DSP graph |
| `rustfft` | 6.4.1 | FFT (spectral effects, phase-vocoder) |
| `rtrb` | 0.3.4 | Realtime-safe SPSC ring buffer (UI↔audio) |
| `basedrop` | 0.1.3 | Realtime-safe memory management |
| `audio_thread_priority` | 0.37.0 | Bump the audio thread to RT priority (Linux) |
| `crossbeam-channel` | 0.5.16 | Lock-free queues between UI and engine threads |
| `rayon` | 1.12.0 | Parallelize offline render / peak generation |
| `midir` | 0.11.0 | MIDI **input** (later: knobs/faders) |
| `rppal` | 0.22.1 | Pi GPIO/I2C/SPI/PWM (later phase) |
| `rodio` | 0.22.2 | *Optional* quick playback bootstrap; drop once you use `cpal` directly |

### 5.2 Architecture

```
┌─────────────── Tauri (Vue 3) ────────────────┐
│  canvas timeline · panels (reka-ui/shadcn)   │
│        invoke() / listen() — control only     │
└────────────────────┬─────────────────────────┘
                     │  commands + events (JSON)
┌────────────────────▼─────────────────────────┐
│ Rust engine (owns its own thread)            │
│   Session model: Sources / Tracks / Clips    │
│   Transport + real-time mix (cpal callback)  │
│   Recording (cpal input → hound WAV)         │
└──────────────────────────────────────────────┘
```

- The **audio callback** (`cpal`) pulls from a lock-free mix buffer; it must never allocate or block. Use `rtrb`/`basedrop` for realtime-safe handoff and `audio_thread_priority` for RT scheduling on Linux.
- **Non-destructive, tape-style data model** — the heart of the Macero workflow:

```rust
Source { id, path, sample_rate, channels, duration_samples, peaks /* pyramids */ }
Track  { id, name, color, gain, pan, mute, solo, effects: Vec<Effect>, sends: Vec<Send> }
Clip   { id, source_id, src_in, src_out,     // which region of the take
         timeline_start,                       // where it sits in the arrangement
         gain, pan, pitch_shift, stretch_factor,
         reverse, fades { in, out }, warp_markers }
```

  Edits move/trim clips; the source audio is never mutated.

- **Timebase = seconds/samples**, not bars/beats (Feldman). A beat grid can come later as an optional snap.
- **Recording** (Phase 1) is `cpal` input → `hound` WAV into the media pool, with live waveform peaks. On Linux, use ALSA `hw:` or JACK for low-latency capture; avoid PulseAudio for the critical path.

---

## 6. Effects & offline processing (CDP8 + spectral stretch)

Three tiers, matching "extendable to include audio effects":

### Tier 1 — real-time (in-engine)
`fundsp` nodes on tracks, aux buses (dub sends → delay/reverb) and master (EQ → compressor → limiter). **Phase 2.**

### Tier 2 — offline "processes" (CDP8) — *the extension point you asked for*

CDP8 and PaulStretch are **file-based, offline** processors. Model them as one abstraction:

```
OfflineProcess { kind, params, input: Source }  →  job queue  →  new Source
```

- Run the external program as a **Tauri sidecar** (`bundle.externalBin`) on a rendered temp WAV; watch progress; import the output as a new `Source`. The job queue lives in Rust, runs in the background, emits progress events — shown in the UI like a render/bounce.

| Processor | Status / notes |
|---|---|
| **CDP8** (Composer's Desktop Project, Release 8) | `https://github.com/ComposersDesktop/CDP8` — **C** codebase (CMake), **LGPL-2.1-or-later**, actively maintained (last push 2026-06, ~726★). Large CLI suite (distortion, envelopes, filters, granular, morph, pitch, reverb, spectral, time — e.g. `blur`, `brassage`, `stretch`, `focus`). Release 8 adds ~80 Trevor Wishart programs (waveset distortion, speech/voice, multichannel ≤8) and **PVOCEX (`.pvx`) phase-vocoder** analysis across all pvoc programs. Existing GUIs: Soundloom 17.0.4, Tabula Vigilans. |
| **PaulStretch** (Paul Nasca) | Canonical "extreme stretch → lush pad." GPL → embedding/porting forces a GPL app (fine under §12). |
| **`ness_stretch`** (Rust, 0.5.1) | NessStretch spectral stretch — but **no license is declared** (crates.io `license=None`; repo has no LICENSE). **Do not ship it** without asking the author. |
| **`paulstretch` Rust crate** | **Does not exist** on crates.io. |

### CDP8: embed vs port — verdict

**Embed the C (sidecar CLI); don't port the suite.**

1. CDP8 is ~800 programs / ~20 MB of C — porting is a multi-year effort with subtle DSP-regression risk.
2. The programs are offline/file-based anyway (render a WAV → run → import), so a subprocess is a *natural* fit — you'd gain almost nothing from in-process FFI for batch work.
3. Sidecar = **zero copyleft obligation** on your app (LGPL applies to CDP8 itself; you ship its `LICENSE` + point to the source).
4. **Port only a single algorithm** when you need it real-time (e.g. one pvoc effect on a live track) — then either (a) port that one function as an **LGPL-2.1-or-later** Rust translation (a derivative work — keep it LGPL, or fold it into a GPL-3.0 app), or (b) **clean-room reimplement** it with `rustfft` so it's yours under GPL-3.0.

For the "PaulStretch sound," the clean paths are CDP8's own `.pvx` pvoc tools (LGPL) or your own `rustfft` phase-vocoder stretch (GPL-3.0) — not `ness_stretch` (unlicensed).

> Your `cdp-front` project is a head start: its CDP-wrapping logic becomes the offline-process tier's UI, with the engine moved into Rust.

### Tier 3 — Rust audio plugin/DSP ecosystem (research, §10)

---

## 7. Workflow feature map

Concrete features by inspiration. **Bold = Phase 1 (the goal).**

**Capture — record (Phase 1)**
- **Record long takes** (mono/stereo; multitrack later) straight into the **media pool**; non-destructive; waveform preview via peak pyramids.
- **Markers while recording**; optional silence-based auto-split.

**Arrange — Macero, edit-as-composition (Phase 1)**
- **Razor/splice at playhead; ripple delete; copy/paste/duplicate regions; crossfades.**
- **Drag clips on the horizontal timeline; loop a clip region with adjustable loop crossfade.**
- **Trim/crop (src_in/src_out); move/layer takes on tracks.**
- Varispeed (linked pitch+speed) and independent time-stretch (`rubato`) — Acid-style "make it fit."
- Reverse; per-clip fade in/out; comping (later).

**Mix — soft mixer (Phase 1)**
- **Track strips: gain, pan, mute, solo; master fader; level meters.**
- Bounce/export the arrangement to WAV (`hound`).

**Sound — Feldman, time & texture (Phase 2+)**
- Seconds-based timeline, long durations, fine gain (0.1 dB), very long fades.
- Spectral/textural processing (CDP8 / own phase-vocoder) as "render to new source" actions.

**Dub — console-as-instrument (Phase 2+)**
- Aux sends → delay bus (tape delay + feedback + filter) and reverb bus.
- **Live dub mixing** (mute/unmute sends, filter sweeps, fader rides) recorded as automation.
- "Drop the track, keep the echo tail" as a structural gesture.

**Master (Phase 3)**
- Master chain (EQ → compressor → limiter) → offline bounce; FLAC/MP3 via a sidecar (ffmpeg/sox).

---

## 8. Raspberry Pi 5 embedded design *(deferred — later phase)*

Not part of the x86 prototype. Kept here as the target for the eventual ARM phase; the engine being a standalone Rust crate is all that's needed now to enable it later.

- **Audio I/O**: Pi 5 has no analog jack → USB interface or I2S DAC+ADC HAT; `cpal` over ALSA `hw:` or JACK.
- **Controls**: USB-MIDI via `midir` (MIDI-learn, persisted via `tauri-plugin-store`); later GPIO via `rppal` or a Daisy-as-MCU. Match USB MIDI by serial; watch ground loops.
- **UI**: desktop mode (WebKitGTK under X11) or headless "box mode" (engine + physical panel, same Vue UI served over LAN).
- Reuse the `pi5-daisy-synth-rig` lessons: RT config, JACK device ownership, latency budget (8–15 ms round trip).

---

## 9. Performance strategy (summary)

- **Rendering:** canvas + peak pyramids + culling + offscreen clip caches + dirty-flag rAF (§4.3).
- **Audio:** realtime thread lock-free (`rtrb`/`basedrop`); mutations on the engine thread; never block the callback; `audio_thread_priority` for RT scheduling.
- **IPC:** control-only JSON; peaks downsampled; no sample streaming (§4.4).
- **Offline work** (peak gen, CDP8, bounce): background threads (`rayon`), progress via events.
- On x86 there is ample headroom; the strategy above still keeps the door open for ARM later.

---

## 10. Rust audio plugin/DSP ecosystem (requested research)

"Rust audio plugins" spans three things — building plugins, hosting them, and the reusable DSP you build *from*. Current state (2026):

| Crate / framework | What it is | License | Verdict for us |
|---|---|---|---|
| **nih-plug** (`nih_plug_*`) | Build **VST3/CLAP/standalone** plugins in Rust; GUIs via `nih_plug_egui` / `_iced` / `_vizia` | ISC | De-facto standard if we later *export* our effects as plugins |
| **vst** (0.4.0) | VST2 build/host | — | Legacy/frozen; VST2 SDK is closed — ignore |
| **clack** (`clack-host`, `clack-extensions`, `clap-sys`) | **Host CLAP plugins** in Rust | MIT OR Apache-2.0 | Best path to *load* third-party FX (CLAP is GPL-friendly) |
| VST3 *hosting* | — | — | Immature in Rust; don't plan on it |
| **fundsp** (0.23.0) | Real-time DSP graph + effects + synth | MIT OR Apache-2.0 | Our built-in effect engine |
| **dasp / dasp_graph** (0.11.0) | PCM primitives + DSP graph | MIT/Apache-2.0 | Lower-level alternative to fundsp |
| **augmented-audio** (`augmented-dsp-filters` 2.5.0, `augmented-oscillator` 1.4.0, `augmented-adsr`, `augmented-midi`, …) | Grab-and-use DSP building blocks (filters/osc/adsr/midi/music-theory) | MIT | Ready-made effect/EQ blocks |
| **rustfft** (6.4.1) | FFT | MIT/Apache-2.0 | Spectral effects, phase-vocoder stretch |
| **rubato** (5.0.0) | Resampling / time-stretch | MIT | — |
| **glicol** | Graph-oriented live-coding DSP engine (Rust, wasm-capable) | MIT | Inspiration / scripting layer later |
| **rtrb** / **basedrop** / **audio_thread_priority** | RT-safe ring buffer / memory / thread priority | MIT/Apache-2.0 | Audio-thread safety |

**What this means for the effects plan:** built-in effects = `fundsp` (+ `augmented-audio` blocks); load third-party FX = `clack` (CLAP); export our FX as plugins = `nih-plug`. All permissively licensed → compose cleanly with GPL-3.0 (§12).

---

## 11. Phased plan (revised — core + plugins)

Architecture: a **minimal core** — clock, audio graph interpreter, session event log, context plumbing (the [minimal-core note](.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md)) — with every capability as a plugin; the product is an assembled profile. "sound-arranger" is the tape-editor profile.

- **Phase 0 — Core spike (x86 Linux):** the four core pieces + one euclidean rhythm plugin driving a sine blip. Validates the sample-accurate clock, realtime-safe graph rendering, live plugin mount/unmount, and the ctx/IPC contract end-to-end.
- **Phase 1 — The sound-arranger profile (the old product goal):** recorder plugin (`cpal` → media pool, live peaks) + tape-editor plugin (razor/splice, trim, copy/paste/duplicate, crossfade, loop regions, drag on the horizontal canvas timeline) + soft mixer plugin (gain/pan/mute/solo, master fader, meters, bounce to WAV). Seconds-based timebase.
- **Phase 2 — Generators & improv:** euclidean rhythm and chord-progression plugins (pure generators providing `ctx.rhythm` / `ctx.progression`); an improv plugin consuming the session log and responding — the paper's self-evolving component, made safe by core reversibility.
- **Phase 3 — Effects & offline processes:** `fundsp` effect plugins + dub sends; the offline-process tier (CDP8 sidecar, PaulStretch, phase-vocoder stretch) as `OfflineProcess` plugins with progress events.
- **Phase 4 — Master & export:** master chain (EQ/comp/limiter), automation, FLAC/MP3 codecs (`Codec` plugins); CLAP hosting (`clack`) and plugin export (`nih-plug`).
- **Phase 5 (separate) — ARM/Pi 5 + hardware controls:** `midir` + GPIO/Daisy, headless box mode. Only then revisit §8.

---

## 12. Licensing & legal

**Recommendation: license the app under GPL-3.0-or-later.**

- "Compliant open-source" + "best technical solution trumps" ⇒ a copyleft license means you are *never blocked* from using the best tool: GPL/LGPL/ISC/MIT/Apache/MPL all compose with GPL-3.0.
- Hobby/boutique (not mass prod, price not an issue) ⇒ copyleft's only real obligation ("publish source when you distribute binaries") is a non-issue.
- **Alternative** if you prefer permissive: **MIT OR Apache-2.0** — but then you may *not* embed/port GPL PaulStretch, and CDP8 must remain a separate sidecar (fine, but it constrains "best tool wins").

Dependency compatibility matrix (verified where marked):

| Dependency | License | Compose with GPL-3.0 app? | Notes |
|---|---|---|---|
| CDP8 | LGPL-2.1-or-later ✅ | ✅ | Sidecar = no obligation. Static link/port = derivative, keep LGPL or fold into GPL-3.0 |
| PaulStretch | GPL (verify v2-only vs v2+) ✅ | ✅ only if app is GPL | If it's GPL-2.0-*only*, use GPL-2.0-or-later for the app instead |
| `ness_stretch` | **none declared** ⚠️ | ⚠️ | Avoid for release; ask author or use CDP8 pvoc / own `rustfft` stretch |
| nih-plug | ISC ✅ | ✅ | Permissive |
| clack | MIT OR Apache-2.0 ✅ | ✅ | Permissive |
| fundsp | MIT OR Apache-2.0 ✅ | ✅ | Permissive |
| augmented-audio | MIT ✅ | ✅ | Permissive |
| glicol | MIT ✅ | ✅ | Permissive |
| symphonia | MPL-2.0 ✅ | ✅ | File-level copyleft; keep its sources available |
| cpal / rubato / hound / rustfft / dasp / midir / rppal | MIT / Apache-2.0 ✅ | ✅ | Permissive — still verify each repo's LICENSE before release |
| rayon / crossbeam / rtrb / basedrop | MIT / Apache-2.0 ✅ | ✅ | Permissive |

**Porting CDP8 algorithms to Rust:** a port is a **derivative work** of LGPL-2.1-or-later code, so the ported files stay LGPL-2.1-or-later (compatible inside a GPL-3.0 app). If you instead reimplement from scratch (no copying), it's yours under GPL-3.0. Either way you're compliant — this is *not* a blocker.

---

## Dev environment & audio latency

- **Nix + devenv** (same pattern as `tidal-lsp`): `flake.nix`, `devenv.nix`, `devenv.yaml`, `.envrc` are committed. Run `direnv allow` (or `devenv shell`) for Rust + Node/pnpm + the Tauri (webkit2gtk) and ALSA build deps.
- **Kernel / latency on Linux & NixOS**: see [`docs/audio-latency.md`](docs/audio-latency.md). TL;DR — nixpkgs has **removed the `-rt` kernels** (PREEMPT_RT is mainlined since 6.12, so the separate package is gone); run `linuxPackages_latest`/`zen` + `rtkit` + `performance` governor, and talk to ALSA `hw:` from `cpal`. No RT kernel needed for an arranger.

---

## 13. Risks & open questions

1. **`ness_stretch` is unlicensed** — resolved by using CDP8's `.pvx` pvoc tools or a `rustfft` stretch instead.
2. **PaulStretch's exact GPL version** — verify; if GPL-2.0-only, set the app to GPL-2.0-or-later.
3. **VST3 hosting is immature in Rust** — plan CLAP hosting (`clack`) instead.
4. **`rolldown-vite` + Tauri** — should be transparent (scripts are just called); confirm the dev-server port wiring.
5. **Tauri + Nix friction** — on x86 you can use plain system deps (webkit2gtk-4.1 etc.) and keep Nix optional; less of an issue than on a Pi.
6. **CDP8 sidecar adds a C/CMake toolchain step** to the build matrix (Phase 2+) — accepted trade-off for 800 mature DSP programs.

---

## 14. Verified sources (2026-08-13)

**Versions (crates.io):** `cpal` 0.18.1, `symphonia` 0.6.1, `hound` 3.5.1, `rubato` 5.0.0, `fundsp` 0.23.0, `dasp`/`dasp_graph` 0.11.0, `rustfft` 6.4.1, `midir` 0.11.0, `rppal` 0.22.1, `crossbeam-channel` 0.5.16, `rayon` 1.12.0, `rodio` 0.22.2, `clack-host`/`clack-extensions`/`clap-sys` 0.1.1/0.1.1/0.5.0, `ness_stretch` 0.5.1, `augmented-dsp-filters` 2.5.0, `augmented-oscillator` 1.4.0, `vst` 0.4.0, `rtrb` 0.3.4, `basedrop` 0.1.3, `audio_thread_priority` 0.37.0.

**npm (latest):** `vue` 3.5.41, `reka-ui` 2.10.3, `naive-ui` 2.44.1, `primevue` 5.0.0.

**Licenses / repos:** CDP8 = LGPL-2.1-or-later (github.com/ComposersDesktop/CDP8, C + CMake, ~726★, push 2026-06-08); nih-plug = ISC (robbert-vdh/nih-plug, ~2938★; crate repo → codeberg.org/BillyDM/nih-plug); clack = MIT/Apache-2.0 (prokopyl/clack); augmented-audio = MIT (yamadapc); glicol = MIT (chaosprint/glicol, ~2996★); fundsp = MIT/Apache-2.0 (SamiPerttu); cpal = MIT/Apache-2.0 (RustAudio); `ness_stretch` = **no license declared** (crates.io `license=None`; spluta/TimeStretch has no LICENSE).

**Confirmed absent:** crate `paulstretch`.

**Local prior art:** `~/Projects/cdp-front` (reka-ui + shadcn-vue + Tailwind v4 + rolldown-vite), `~/Projects/pi5-daisy-synth-rig` (Pi 5 + JUCE + Daisy + JACK + USB-MIDI), `~/faust-juce-writeup.md` §6.8–6.10.

---

## 15. Plugin architecture research — "everything as a plugin" (paper · cordis · deepseek-harness)

> Working research, 2026-08-15. The decision candidates live in the note [Composition seams — plugin architecture for the engine and host](.agents/notes/proposed/architecture/2026-08-15-composition-seams-plugin-architecture.md); this section holds the background. Primary sources read in full: the paper text at `.research/paper/paper.txt` (88-page PDF, draft 2026-08-13) and a local checkout of deepseek-harness; a reading companion sits at `.research/paper/SUMMARY.md`.

### 15.1 cordiverse/paper — the theory: dynamic composition made formal

Preprint (draft 2026-08-13) by Yifan Shi & Wei Zhang (Peking University) and Tianyi Cui (DeepSeek-AI). Thesis: static composition (functions, modules, inheritance) has formal foundations; **dynamic composition** — loading, unloading, and reconfiguring components at runtime, as plugin systems and self-evolving agent harnesses demand — has none. Two orthogonal dimensions:

- **Temporal composability** — on removal, a component's side effects on the shared environment must be fully reversed.
- **Spatial composability** — components declare dependencies; the runtime resolves and re-wires them reactively as providers appear, disappear, or change.

Both are lifted from compile-time type systems to **runtime mechanisms**:

- **Revertible effects**: an effect is a context transformation paired with an inverse, `Γ → Γ × (Γ→Γ)`. The runtime accumulates inverses (LIFO) in an effect context `∂Γ = Γ × (Γ→Γ)`; unloading a component applies the accumulator. The author writes an inverse only per *atomic* effect; the inverse of any composite is derived by composition — teardown is derived from loading, not written alongside it. Withdrawing one component among many requires effect **independence** (commutation of transformation monoids; recovery in any permutation order, Corollary 21). Where effects don't commute, order is imposed by the accumulator (within a component) or a declared dependency (across components).
- **Reactive coeffects**: the environment is a typed dependency table `Σ = (k:K) ⇀ V_k`; `set` is itself an effect, so provision is revertible. A component declares a coeffect specification (the keys it needs); every context change is classified **activating / deactivating / neutral** and drives the lifecycle. Two refinements: **isolation** (realms — the same key resolves differently per context; runtime ad-hoc polymorphism / scoped DI) and **interception** (mergeable metadata changing *how* a dependency is used without touching the provider — the basis for access control).
- **The context paradigm**: effects and coeffects unify into one recursive context type `Γ∞ = μΓ. Γ × (Γ→Γ) × Σ` — every interaction with the environment passes through one explicit context. The paper positions it between functional state-threading (traceable, verbose) and implicit mutation (ergonomic, untraceable). Recovery is up to an **observational equivalence** ≃: behavior, not representation.

Then: a **calculus of dynamic composition** (components `(d, p, e)` instantiated as fibers with an inertial lifecycle state machine; metatheory: preservation, temporal/spatial composability, progress, confluence) and the **Cordis** implementation with the **Koishi** case study (chatbot framework, 4 years, 4000+ community plugins; every feature is a plugin; server and web console are two independent Cordis applications).

### 15.2 cordiverse/cordis — the meta-framework

TypeScript, by shigma (Koishi author); v4 in active development (API unstable). "Meta-framework": it fixes how effects and coeffects compose and leaves domain vocabulary to the application. Five ideas (official primer):

1. **A plugin is an object implementing Service** — a function with optional `inject` + `apply(ctx)`, or a `Service` subclass mounted by Cordis.
2. **A context is a repository of services** — a service claims a stable `ctx.<key>`; consumers find it by key, never by importing an implementation.
3. **Declare dependencies via `inject`** — load order is expressed through service requirements, not manual boot sequencing.
4. **Typed events** — names via TS declaration merging; dispatched `emit` (observe) / `waterfall` (wrap; `next()` delegates) / `parallel` (fan out) / `serial` (ordered, returns value); the dispatch mode is part of the event's public contract.
5. **Registrations are reversible effects** — installed via `ctx.effect()` / `ctx.on()`, unwound on reload and teardown.

Plus a **declarative loader**: the system is a configuration tree of entries `{id, url, config, disabled, isolate, intercept}`; the loader reconciles incrementally (keyed diff, only changed rows), and HMR swaps modules transactionally with rollback. Headline property: **path independence** — the final system state depends only on the declared config, never on load order. `group` / `include` / `hmr` are themselves ordinary components.

### 15.3 deepseek-ai/deepseek-harness — "everything is a plugin" in production

MIT agent harness (v0.1 developer preview, 2026-08-13) whose README states the architecture verbatim: "everything is a plugin", powered by Cordis. TS monorepo (~7,400 files, 230+ workspace members), ~100k stars within two days of release. DeepSeek used this exact harness to produce its published agent-benchmark scores (Terminal Bench 2.1 87.9, DeepSWE 62.7, Toolathlon-Verified 74.1 for V4 Pro); anyone can reproduce them via the Python SDK (`BENCHMARK.md`).

- **Composition**: a running dsh is a plugin tree built from ordered layers — profiles list bundles; bundles ship config rows (`dsh.bundle` → `cordis.patch.yml`); patches replace any row by id (whole-row replacement, not deep merge; later layers win); `dsh --profile web --dump-config` prints the exact booted tree.
- **Core service keys**: `ctx.sessions` (append-only SessionEvent log — "model-visible means logged" is a runtime invariant), `ctx.systemPrompt`, `ctx.tools`, `ctx.agents`, `ctx.agentLoop` (the driver is itself swappable), `ctx.llm` (adapter seam).
- **Events are the extension points**: session events (durable facts), agent events (live interception), capability events (policy at seams); turn/step flow (`turn/start` → … → `llm/stream` → `tools/execute` → `step/end` → `turn/end`).
- **Capability seams**: every swappable capability is a triple of Service Definition / Provider / Consumer; one provider swap re-points everything downstream (fs/subprocess → remote sandbox moves Bash, PTY, and LSP with it).
- **Self-referential**: the `extensions` package lets an agent mount/unmount its own plugins at runtime — the paper's motivating endgame, shipped.

### 15.4 The paradigm against design principles

| Principle | Where it shows up |
|---|---|
| Parnas modularity | The paper's opening citation; plugins as modules with declared interfaces |
| Open/Closed | Extend via seams / extension points; never modify the core |
| Inversion of Control / DI | The Context is a service repository; `inject` replaces service locators; the paper formalizes IoC containers as a coeffect context |
| Dependency inversion | Consumers depend on service keys / trait definitions, never concrete providers |
| Hexagonal (ports & adapters) | Harness capability seams are exactly ports + adapters |
| Command/undo, saga compensations | Temporal composability's lineage: developer-authored inverses; Cordis makes the inverse structural (derived by composition) |
| Capability-based security | Access = declared inject + mediating proxy; undeclared access fails |
| Microkernel | Cordis is the microkernel; Koishi / dsh / our app are the personalities |
| Desired-state reconciliation | Loader diffing + layered patches — same family as Kubernetes desired state, NixOS config |

### 15.5 The FP (Haskell) reading

The formalism is category-theoretic and maps cleanly onto FP:

- **Context = Reader environment**: `App a = ReaderT Context IO a`; a plugin is a pure description `Config -> Context -> App ()` the kernel interprets. Coeffects = typed `ask` (requirements); `inject` is a constraint set. The paper's own §6.4: typeclasses (Haskell) / traits (Rust) are how a host extends the context type — a provider is an instance, a consumer's constraints are its inject.
- **Revertible effects = State with an undo stack**: `∂Γ = (γ, φ)` is `StateT Γ` with an inverse accumulator (`track (f,g)` composes `g` onto `φ`; `recover` applies `φ`). Twisted composition is just "inverses stack in reverse order". The witnessed type `𝔈Γ*` carries the proof obligation `g(f(γ)) = γ` — refinement-type territory.
- **Effects as values**: "every effect carries its inverse" is one step from a free-monad-style DSL the kernel interprets (`foldFree` accumulating inverses).
- **Interception ≈ effect handlers**: metadata that reshapes how a dependency is used, provider untouched — interpretation without modifying the operation (cf. Koka/Eff/Effekt; the paper positions itself against Effekt in §7.1).
- **Recovery up to ≃ = observational equivalence / logical relations**: states equal when no observer distinguishes them — the FP move that makes independence attainable (heap layout forgotten, behavior kept).
- **Adjacent literatures** (§7.3): STM (statically scoped reversal), linear types / RAII / Rust ownership (lexical reversal — complementary), reversible computing (global reversibility vs. Cordis's per-effect one-sided inverses).

### 15.6 The Nix reading

The NixOS module system is the canonical declarative composition; the correspondence is tight (a Chinese analysis literally titled harness+Cordis "活着的Nix" — a living Nix):

| Nix | Cordis / plugin systems |
|---|---|
| Modules declare options + set config | Entries declare inject/provide; loader reconciles |
| System = fixpoint of module functions | System = fixpoint of the plugin tree; path independence |
| `mkIf` / `mkDefault` / `mkForce` | `disabled` fields, defaults, layered patches |
| Overlays (redefine a package) | Patches (`cordis.patch.yml`) — replace any row without forking |
| Generations / rollback | Revertible effects — unload = rollback of a contribution |
| Purity & reproducibility | Path independence (same config → same state, order-independent) |
| NixOS = "everything is a module" | dsh = "everything is a plugin" |
| Composition offline, once, at activation | Composition online, continuously, reversibly |

One deliberate difference: NixOS modules **merge** option sets with priority-based conflict resolution; Cordis patches use **whole-row replacement**. Less merge cleverness, more predictability — a trade worth keeping in mind if we ever build layered config.

### 15.7 What it means for sound-arranger

The project is pre-code: the cheapest possible moment to fix the missing composition story. Plugin surfaces that already exist in the plan, unlabelled:

- **Recorders / input backends** — cpal devices; the Notepad-12FX routing (`nusb`) as a second provider behind one `Recorder` seam.
- **Offline processes** — CDP8 programs, PaulStretch, phase-vocoder stretch: each an `OfflineProcess` plugin; file-in/file-out, non-realtime, naturally isolated — the first plugin domain.
- **Effects** — fundsp graphs as *data*; CLAP hosting via `clack` (Phase 4) is the industry plugin ABI.
- **Codecs** — WAV/FLAC/MP3 import/export behind one interface.
- **UI** — panels / toolbars / inspectors around the canvas timeline core.

The FP-shaped architecture: immutable Session + typed edit functions (pure, testable, undoable); the cpal callback as a pure interpreter of a small value-level instruction stream (never allocates, never registers plugin callbacks — the realtime path is the privileged kernel); plugin boundaries produce *values* (graphs, configs) the interpreter reads. Composition / orchestration (reversible effects, declarative rows, patches) lives on the Tauri/TypeScript side, where Cordis itself could eventually run. Decision candidates: see the [composition-seams](.agents/notes/proposed/architecture/2026-08-15-composition-seams-plugin-architecture.md) and [minimal-core](.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md) notes.

### 15.8 Sources (2026-08-15)

- **`cordiverse/paper`** — full text read locally at `.research/paper/paper.txt` (paper PDF 88 pages, draft 2026-08-13; repo holds exactly 3 files, no license, 1,399★/50 forks at fetch); reading companion `.research/paper/SUMMARY.md`.
- **`cordiverse/cordis`** — repo + core-package READMEs; official primer at <https://deepseek-harness.github.io/deepseek-harness/reference/cordis-primer>; <https://floatboat.ai/blog/cordis-plugin-framework>.
- **`deepseek-ai/deepseek-harness`** — README + `docs/{architecture,cordis-primer,capability-seams}.md` read from a local checkout; `BENCHMARK.md`; external coverage: 36kr, servola.de, forklog.com.
