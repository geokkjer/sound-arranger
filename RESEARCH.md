# sound-arranger — Research & Architecture

> **Working title.** A clip-based **arrangement / composition** tool: record long **generative runs** — a eurorack patch or compositional algorithm producing evolving patterns over drones or stretched audio — then cut, paste, rearrange and shape them into a finished piece, and master the result. No MIDI note sequencing. The generative/edit-as-composition approach (musique concrète, dub, ACID) is *inspiration* — it tells us which gestures are worth having; the *model* is the modern visual computer: clips as first-class objects, cut/paste, paint-to-fit time-stretch (the ACID workflow). The "performer" is a running system, not a human musician.
>
> *Status: research draft, rev 6 (the **alpha is implemented** — see §11's status note and
> [`docs/capabilities.md`](docs/capabilities.md)). Locked decisions are marked 🔒. Crate/license facts checked against crates.io / npm / GitHub on 2026-08-13 and listed in §15. Plugin-architecture research added 2026-08-15 (§16); minimal-core architecture reframed 2026-08-15 (§11 + note); tape framing demoted to inspiration 2026-08-15 (§1). CDP WASM Suite (Oli Larkin's CDP8→WASM port, npm `cdp-wasm`) explored hands-on 2026-10-06 (§6). Rev 6 (2026-08-15): **umbrella-first locked** — the platform is the goal, sound-arranger is the first profile; offline processing split into the deferred sound-sculptor profile (§3, §11, [umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)); second review folded in (per-node latency/PDC, log rate-scoping, varispeed — minimal-core note).*

---

## 0. TL;DR — recommendations on one screen

| Area | Recommendation | Why |
|---|---|---|
| **Target** | **x86 desktop first** (Linux primary; macOS/Windows via iced (planned)). ARM/RPi + hardware controls = a separate later phase | Best technical solution trumps; don't pre-optimize for a Pi |
| **Direction** | **Umbrella-first** — the platform is the goal; profiles are the products; sound-arranger is profile #1 | [umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md) |
| **App shell** | **Tauri v2 + Vue 3 tried and decided against (2026-09-22)**; the shells are **iced** (§4.5, + `iced_audio` widgets) and **ratatui** (§4.6), both in-process Rust over the Host API. **The TUI is primary; iced is second, implementing the same modal, key-driven workflow** (and later leaning into the GUI-only extras) — [shells note](.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md), [one workflow, two shells](.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md#one-workflow-two-shells-2026-09-22) | One stack, one workflow; the frozen Tauri shell is history, not a reference ([`crates/shell/RETIRED.md`](crates/shell/RETIRED.md)) |
| **Architecture** | **Minimal core (clock · graph interpreter · session log · context plumbing) + everything-else-as-plugin; the product is an assembled profile** | §11, §16.7, [minimal-core note](.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md) |
| **UI library** | **`iced` 0.14 + `iced_audio` widgets** (GUI) and **`ratatui`** (TUI), one in-process Rust stack; the Vue stack (`reka-ui` + shadcn-vue + Tailwind v4) went with the shell we decided against | The `workflow` crate is the shared model, so no toolkit leaks into the core; do **not** add a second GUI toolkit |
| **Timeline rendering** | `<canvas>` 2D + precomputed waveform **peak pyramids** + viewport culling + offscreen clip caching | Fast and predictable; DOM-per-clip is a dead end |
| **Audio I/O** | `cpal` (in **and** out) — ALSA/JACK on Linux, CoreAudio on macOS, WASAPI on Windows | Lowest latency; owns the device directly |
| **Decode / encode** | `symphonia` (decode) · `hound` (WAV in/out) | Pure Rust |
| **Time-stretch** | `rubato` (real-time) · CDP8 pvoc / own `rustfft` stretch (offline). **Alpha decision:** an offline **WSOLA-class** render materialised into the pool (rational ratio in the log) — see the [alpha finish-line note](.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md); `rubato` stays the buy-it option if quality demands it | Real-time vs "lush spectral" are different jobs |
| **Effects** | `fundsp` (real-time) · CDP8 as **offline sidecar** (later phase) | Dub sends in-engine; spectral rendered to disk |
| **License** | App **GPL-3.0-or-later**; CDP8 (LGPL-2.1) as a sidecar binary | Compliant + lets you embed/port anything GPL/LGPL (§12) |
| **Phase 1 (first profile)** | **Core + the sound-arranger profile: record → cut/splice → clip/loop arrange → soft mixer** | See §11 |
| **Alpha finish line** | **Record in the tool → cut/copy/paste/append with grid snap → tracks → time-stretch → mastering chain → export → session save/open**, in the TUI, with the iced shell on the same workflow ([plan](.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md)) | The co-work passes (GLM-5.3-Flash, GLM-5.3, Kimi K3) found the gaps the owner's list assumed away — [reviews verbatim](research/architecture/2026-09-23-alpha-scope-co-work-reviews.md) |

**Where the project is going (2026-09-23; re-verified against the code 2026-09-30).** The three co-work
passes over the owner's alpha ask changed the order and the scope: the substrate is further along than the
owner's list implies (the whole ACID op set, the `host v1` language, undo-by-replay, the shared workflow).
Four of the five gaps that pass then reported are now **closed**: recording **is** wired into the host
(`HostCommand::Record`/`RecordStop` → `HostSession::record` opens the input device via
`media::devices::open_input`, captures into the pool, and finalizes the take on stop —
`crates/host/src/lib.rs`), the session **does** save and open as a directory (`session.txt` + `pool/` +
append-only journal, `HostSession::{save, load_session}`), stereo material is **split per channel at the
import boundary** instead of reduced to channel 0, and 24-bit WAV **is** accepted. What genuinely remains
is **`TransportSeek`, which is still O(target)** — `replay_to` rebuilds a session and renders from zero
(~28 s into a 30-minute jam, the one gap that gets worse with use) — plus the *shell* work that makes any
of the above reachable by a human. The plan orders the work foundations-first (compound gestures → session
save/open → recording → stereo/bit depths → grid snap → clipboard/append → tracks → pool panel → utilities
→ stretch → mastering → export → markers → seek → docs) and records what is deliberately cut from alpha;
items 1–7 of that order have now landed. The goal is live; the slices land as commits with tests and notes,
and the architectural ones pass the reviewer gate. Corrected here because the earlier text kept steering
sessions at work that was already done.

---

## 1. Concept & creative references

The through-line is **"composition happens in the edit, after the performance."** The "performance" here is not a human jam — it is a **running generative system**: a eurorack patch or compositional algorithm that runs for a long time and unfolds **evolving patterns over drones or stretched audio**. The composer authors the system and curates its output, aiming at something new and unheard rather than reproducing any existing artist or idiom. This is the Autechre move ([2026-09-01 study](research/architecture/2026-09-01-autechre-generative-composition-inspiration.md)): *the generator replaces the band* — the raw material is software run for a long time, and the take is a *run*, not a live performance.

**The model is the visual computer, not the tape machine.** The edit-as-composition techniques below (splicing, looping, dropping tracks, the studio cut) are *inspiration*: they tell us what gestures are worth having. This tool implements them in the modern paradigm — clips as first-class objects, cut/paste, paint-to-fit time-stretch, non-linear visual editing — where every tape technique becomes easier, reversible, and searchable. The design question throughout is: *how do the modern tools make these techniques better?*

- **Generative / modular — the "performer" is the system.** A eurorack patch (or a compositional algorithm) runs for a long time and unfolds **evolving patterns over drones or stretched audio**; the take is a *run*, not a band. The composer is simultaneously **instrument/algorithm author and editor** — the edit is the composition, and the material is non-goal-directed and texture/timbre-led. The aim is to author the system and curate its runs into something new and unheard. ([Autechre](research/architecture/2026-09-01-autechre-generative-composition-inspiration.md); the [Synclavier/modular line](research/architecture/2026-09-01-synclavier-design-inspiration-and-modular-hardware.md) — a software instrument on a general-purpose computer, the patch as a structured value, now *generating* rather than just playing.)
- **Morton Feldman**: long durations, quiet dynamics, sparse placement, non-goal-directed structure, texture/timbre over rhythm. Suggests a **seconds-based timebase** (not bars/beats), very long fades, fine gain resolution.
- **Tape artists** (musique concrète / early tape music): non-destructive splicing, varispeed, reversal, loops, layering takes.
- **Dub production** (King Tubby, Lee Perry): the mix console as instrument — aux sends into delay/reverb, "dropping" tracks to leave only the echo tail, EQ kills, fader rides recorded as automation.
- **Sony ACID (early 2000s)**: the "paint clips onto a timeline and it time-stretches them to fit" workflow — the **direct ancestor of the substrate**: clips as objects that stretch to fit, cut and paste as the primary verbs. The edit-as-composition gestures are re-expressed *in this model*, not in tape terms.

### Reading — the composition-theory corpus

The creative brief above is grounded in the **music-composition-theory** corpus (the Composer's Onion framework). Paths below resolve on this machine through the `~/Projects/music` symlink view — `../music/…` from this repo (see `~/Projects/music/README.md`):

- [Bitches Brew — the studio cut as composition](../music/music-composition-theory/analyses/miles-davis/bitches-brew.md) — Teo Macero's razor-blade editing; Onion Layer 6 (Time / Editing)
- [On the Corner](../music/music-composition-theory/analyses/miles-davis/on-the-corner.md)
- [Dub reggae (1968)](../music/music-composition-theory/theory/movements/1968-dub-reggae.md) — console-as-instrument; the §7 dub workflow
- [The Composer's Onion](../music/music-composition-theory/theory/framework/the-composers-onion.md) — Layer 6 = the studio cut; Layer 7 = system/process (generative)
- [The Two Tracks](../music/music-composition-theory/theory/framework/the-two-tracks.md) — compose → perform → select → *recompose*: live recordings return as raw material; the product thesis, stated as theory
- [Modular patching DSL sketch (Haskell)](../music/music-composition-theory/notes/modular-dsl-sketch.md) — idea source for a future scripting layer (§10 glicol)

The tool is a **clip-based visual arranger**, not a groovebox or MIDI sequencer. **MIDI note sequencing is *deferred*, not forbidden** — MIDI note events re-enter as a Phase-2+ `MidiSource`/`MidiSink` integration after the audio mixer/arranger is running; only the *note-sequencing UI* (piano roll) stays out of the timeline. Generators (euclidean, chords) produce *note events as values* throughout; see the [musical-event model note](.agents/notes/proposed/architecture/2026-08-15-musical-event-model.md) and the [decoupled pitch/rhythm note](.agents/notes/proposed/feature/2026-08-20-decoupled-pitch-rhythm-generators.md).

---

## 2. Relationship to your existing rigs

| Project | Role | How it relates |
|---|---|---|
| `pi5-daisy-synth-rig` | **The jam source** (Pi 5 + JUCE mixer + Daisy voices, USB interface, JACK) | In a **later phase** the arranger can record its output. Not part of the x86 prototype. |
| `plugins/` (CDP sidecar) | **CDP / offline-process sidecar** (CDP8, PaulStretch, Csound offline, phase-vocoder) as `OfflineProcess` plugins | The former `cdp-front` **frontend** scaffold is dropped — no standalone frontend until there was a shell (the Tauri one; now iced/ratatui). CDP integration is now a **sidecar plugin** in [`plugins/`](plugins/), not a separate repo. Its UI *stack* decision (reka-ui + shadcn-vue + Tailwind v4) is captured in §17 and the [app-shell note](.agents/notes/proposed/architecture/2026-08-13-tauri-vue-rust-app-shell.md). |

The gear/hardware **notes** (the USB characterizations and the Eurorack plan) live in the **studio project** — `music-composition-theory/studio/instruments/<name>/research-note.md` — not here. They are linked below for context; sound-arranger is software-only. (VCV Rack is its own project: `~/Projects/music/vcv-rack`.)

**Phase-1 capture hardware:** the [Soundcraft Notepad-12FX](../music/music-composition-theory/studio/instruments/soundcraft-notepad-12fx/research-note.md) — 4-channel multitrack USB (2 mono + 1 stereo pair), class-compliant, routing protocol already reverse-engineered for a Rust `nusb` provider. The pi5 rig's own WAV recording is *that* project's Phase 2 — the arranger records the rig only once that exists.

- **EP-133 K.O. II (attached, now `2367:8020` on OS 2.5.1; was `2367:0020` pre-2.5):** over USB it's a **class-compliant USB MIDI** device and, since OS 2.5.1, a **class-compliant stereo USB audio source/sink (16-bit / 48 kHz in + out)**; there is **no mass storage** — the structured filesystem is reachable only through the **undocumented SysEx protocol** carried over the MIDI interface. See the [EP-133 USB characterization](../music/music-composition-theory/studio/instruments/ep-133-k-o-ii/research-note.md); it pairs with the [EP-133 → DAW export tool](research/architecture/2026-08-18-dawproject-and-ep133.md) project-source path.
- **Korg NTS-3 kaoss pad kit (attached, `0944:0153`):** over USB it's **pure class-compliant USB MIDI** — no USB audio, no mass storage, no HID. The KAOSS **XY-pad → MIDI CC** makes it a **control surface** (CC automation for the engine), and the KORG KONTROL Editor channel manages effects/programs (`logue`SDK programmable). Audio is analog — route its jacks into the Notepad-12FX. Latest firmware is **v1.4** (Windows updater + KORG USB-MIDI Driver); this unit's `bcdDevice 1.00` suggests it's behind. See the [NTS-3 USB characterization](../music/music-composition-theory/studio/instruments/korg-nts-3/research-note.md).
- **Woovebox (attached, BLE-connected `WOOVEBOX-FA8E`):** over **USB it is charging-only** (no USB data/MIDI/audio). Its computer interface is **Bluetooth LE MIDI** (standard MIDI-over-BLE UUID available; paired/bonded/connected), and voice is **analog** (3.5 mm line out/in + TRS type-A MIDI). Audio goes line-out → Notepad-12FX. **Caveat:** the Wooveconnect web tool needs Web-MIDI, which on Linux is backed by ALSA — this host's BlueZ (5.87) is built **without `--enable-midi`**, so the BLE-MIDI device isn't published as an ALSA MIDI port (why the page shows "not connected"). Fix = build BlueZ with `--enable-midi` (or a BLE-MIDI→ALSA bridge, or a WIDI Bud/USB adapter). See the [Woovebox characterization](../music/music-composition-theory/studio/instruments/woovebox/research-note.md).
- **M-VAVE FM-1 (attached, `4c4a:c755` Jieli composite):** a pocket **6-op FM synth** that is a **native USB audio *and* MIDI** device — this is the first gear in the collection with USB audio. ALSA card 1 `[Device]` exposes **`pcm0c` (stereo 24-bit/44.1 kHz capture, EP 0x83)**, `pcm0p` (playback, EP 0x02), and `midi0` (notes/CC/**PC 0–127** patch select). Controls are **MIDI, not HID**. So the arranger's `Capture` can record it **directly over USB**, or route its line out into the Notepad-12FX. Firmware is updated via **M-Upgrade (Windows/Mac), USB-HID OTA** (the `FM-1.fwsc` image; changelog tops out at **V15**). See the [FM-1 USB characterization](../music/music-composition-theory/studio/instruments/m-vave-fm-1/research-note.md).

- **Behringer Eurorack rack (in development — living build plan):** explore *Eurorack* as the physical realization of generative / "studio as instrument" patching (see the [Autechre research](research/architecture/2026-09-01-autechre-generative-composition-inspiration.md)), as an analog voice + generative source feeding → Notepad-12FX. **Current direction: a one-voice, self-evolving drone** in a Tiptop Happy Ending Kit (84 HP) — a single complex oscillator (Victor) with a modulation/randomness backbone (Waves, 140/150, A-148 S&H, A-160-2 clock divider) and a reverb tail (2hp Verb), out via CU1A USB audio. **The rack drives itself** — no keyboard, no MIDI-CV, no dedicated VCA; evolution is in timbre, not amplitude. More voices / a VCA / a quantizer / a CV bridge are **deferred**, not part of this build. See the [Behringer Eurorack rack plan](../music/music-composition-theory/studio/instruments/behringer-eurorack/research-note.md).

Keep the arranger's **engine** as a standalone Rust crate (not Tauri-coupled), so a future headless/ARM "box mode" is possible — but don't design for it now.

---

## 3. Decisions 🔒 (rev 3)

- 🔒 **x86 desktop is the primary target** (Linux first; Tauri also gives macOS/Windows). Raspberry Pi 5 / ARM / hardware controls are a **separate, later phase**. Hobby/boutique, price not an issue → best technical solution wins.
- 🔒 **License: GPL-3.0-or-later** for the app (rationale + dependency matrix in §12).
- 🔒 **CDP8: embed the C as a sidecar CLI**, port to Rust only per-algorithm and only if a specific effect must run real-time (§6).
- 🔒 **Phase 1 scope:** audio input (record) → Acid-like cutting → clip/loop arrangement → soft mixer. No MIDI, no effects beyond the mixer.
- 🔒 **Horizontal timeline** — time flows left→right, tracks are stacked lanes.
- 🔒 **Umbrella-first** (rev 3, 2026-08-15) — the platform (minimal core + everything-as-plugin) is the goal; the product is assembled profiles. The clip-arranger profile ("sound-arranger") is first because it exercises recording, editing, mixing and the realtime path end-to-end ([umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)).
- 🔒 **Offline/non-realtime processing is its own deferred profile** ("sound sculptor", working name) — CDP8, PaulStretch, phase-vocoder live there, not in the arranger; unscheduled, no earlier than Phase 3.

---

## 4. Frontend

> **Movement (2026-09-22):** §4.1–4.4 describe the **retired** Tauri + Vue shell; the live question is
> §4.5 (iced) versus §4.6 (ratatui). The React/Vue reasoning below is kept as the record of what the
> shell was and why the web stack was chosen then — and as the porting reference for the Vue
> components the Rust shells replace.

### 4.1 Stack

- **Vue 3.5.x** (`vue@3.5.41`) + TypeScript, `<script setup>` SFCs.
- **Vite** — `rolldown-vite` (the build tool prototyped in the dropped `cdp-front` scaffold) is fine if you like it; Tauri v2 only calls your `dev`/`build` scripts.
- **Tauri v2** (`@tauri-apps/api/core`, not the v1 `tauri` module). Commands registered in `tauri::generate_handler![…]`; frontend uses `invoke()` / `listen()` / channels; permissions are **deny-by-default** via `src-tauri/capabilities/*.json`.
- On x86 the webview is a non-issue: WebKitGTK (Linux), WKWebView (macOS), WebView2 (Windows) all handle a canvas timeline comfortably.

### 4.2 UI library — reuse what you have

You asked for "a good UI library." The honest answer: **you already have one** — the `reka-ui` + shadcn-vue stack (prototyped in the now-dropped `cdp-front` scaffold, kept as a decision here):

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

### 4.5 iced — the in-process Rust shell (under evaluation, 2026-09-21)

**Why it is on the table.** The shipped shell is three runtimes for one program: a Rust core, a
Rust transport adapter, and a TypeScript/Vue UI, with an IPC + serde wire between the last two.
iced is the opposite bet — *all* of it in Rust, no HTML/CSS/JS split, and a framework built around
[the Elm architecture](https://book.iced.rs/philosophy.html) (state · message · update · view),
which is close to this project's own FP-shaped discipline (values, logged commands, replayable
state). The [UI-as-plugin note](.agents/notes/implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md)
already listed "egui / iced in-process shell — the boundary becomes voluntary" as the anticipated
second host; this section is that option being *measured* rather than assumed.

**Versions and features (verified 2026-09-21).** `iced 0.14.0` (released 2025-12-07, MSRV 1.88) is
the released baseline; `master` is `0.15.0-dev` (edition 2024, MSRV **1.93**) — so the pinned
`stable` toolchain is a prerequisite, not a luxury. Default features are
`["wgpu", "tiny-skia", "crisp", "web-colors", "thread-pool", "linux-theme-detection", "x11", "wayland"]`:
GPU (wgpu) *and* software (tiny-skia) renderers, both windowing backends. Two API facts that matter
to a live shell:

- **Frame-driven redraw has no timer backend.** `time::every()` exists only in iced's tokio/smol
  time backends, which the default feature set does *not* enable. `iced::window::frames()` →
  `Subscription<Instant>` is the idiom instead: each redraw is itself a message, so the loop is
  self-sustaining. That is exactly what a playhead wants.
- **`update` may return `()` or `Task<Message>`**; the builder form is
  `iced::application(boot, update, view).title(..).subscription(..).theme(..).window_size(..).run()`.

**What the spike measured** ([`spikes/iced-shell/`](spikes/iced-shell/), isolated workspace, iced
0.14): the window opens and renders on this host (niri/Wayland, Mesa, wgpu — the exact thing the
parked Nix/devenv shell died on); the shell owns the **same `host::live::HostHandle`** the Tauri
bridge uses, in-process (no IPC, no webview, no serde wire); transport (play/stop/rewind) drives it
and channel + master meters and the position readout follow the audio; the audio device opened at
its own rate with no negotiation work; and a headless `--probe` mode (boot → load → play → poll the
published `Snapshot`) is CI-able and fails loudly when no meter signal appears. The
evaluation note is [`.agents/notes/proposed/architecture/2026-09-21-iced-shell-evaluation.md`](.agents/notes/proposed/architecture/2026-09-21-iced-shell-evaluation.md).

**What is still open — the questions that actually decide it.**

1. **Canvas parity.** The Vue timeline's maths (`timelineView` / `timelineEdit` / `timelineTicks` +
   unit tests) and its peak-pyramid drawing have to exist in iced's `canvas` widget at 60 fps
   before this is a real option. Unmeasured.
2. **The widget gap.** reka-ui/shadcn give menus, dialogs, popovers, tooltips, context menus,
   comboboxes, virtualised lists, drag-and-drop, and text inputs with IME. iced ships a smaller
   core set (`button`, `text_input`, `pick_list`, `slider`, `pane_grid`, `table`, `scrollable`,
   canvas) plus third-party crates (`iced_aw`, …). The count of hand-rolled widgets
   *is* the cost of the single stack.
3. **The UI-as-plugin tension.** The composition thesis wants UI plugins loaded at runtime; the Vue
   shell does that with a declarative loader + registry. Compiled Rust has no runtime widget
   loader — an iced shell would need a data-driven view layer or a WASM/scripted seam to keep that
   promise. This is the sharpest architectural question, not a performance one.
4. **Framework churn.** iced is a single-maintainer project that explicitly reserves the right to
   break its API (0.14 is the released baseline; `master` already requires rustc 1.93). That is a
   real ongoing cost against a frozen Vue/webview baseline.

**Where it stands (updated 2026-09-22):** the shells are iced and ratatui — Tauri was retired by the
owner, so canvas parity and the widget inventory are now the work of *making* the shell, not of
qualifying an option ([shells note](.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md)).
**`iced_audio` (0.17, MIT, 2026-09-09)** answers a slice of the widget gap with stock widgets:
`VSlider`/`HSlider`/`Knob`/`Ramp`/`XYPad`/`ModRangeInput` on a normalized-parameter contract, with
unit ranges (`DBRange` logarithmic in dB, `FreqRange`, `IntRange` with `snap`), tick/text marks and
modulation ranges. It depends on `iced_core`/`iced_graphics` **0.14**, i.e. exactly the shell's iced,
so it cannot drift from it; iced must be built with `canvas` **and** `image` for it. The spike's
console is now real faders on the host's parameter fold — the demo's `master.gain 0.8` boots as
master **−1.9 dB** while `ch0…ch3` sit at +0.0 dB, and a drag sends a logged `set_param`
([screenshot](spikes/iced-shell/mixer-fold.png), [spike README](spikes/iced-shell/README.md)).

**The shell and the model are one decision (2026-09-21).** The direction is that the UI should be
*for audio what vim/helix/emacs is for text* — modal, selection-first, with the existing `host v1`
vocabulary as the `:` command line — written down in the
[modal editing model note](.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md).
iced can host that model, but a GUI-shaped shell hides the log this engine is built around, so the
two are evaluated together; the keyboard-native TUI is where the model gets tested first.

### 4.6 ratatui — the terminal shell (under evaluation, 2026-09-21; one of two candidates)

> **Movement (2026-09-22):** Tauri is retired, so this is no longer "a second reference alongside a
> shipped shell" — ratatui and iced are the two shells, and **the TUI is primary**: the modal
> workflow is where the product's identity is, so it is proved where it cannot cheat (no mouse, frames
> that can be asserted) and ported to iced afterwards
> ([shells note](.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md)).

**Why it is on the table.** `egui` and Slint are out by inspection (immediate mode; a second view
DSL) and the human's read of the iced spike is favourable ("looks good and more native"), but a
terminal shell answers a question iced cannot: it runs **anywhere** — no GPU, no display server,
over SSH, in a container, on the deferred Raspberry-Pi "box mode" — and it is the natural home for
the keyboard-first workflow an instrument-like editor wants. The
[UI-as-plugin note](.agents/notes/implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md)
already listed ratatui as "a good later second reference (the CLI with a face)"; this is that option
being measured as a *product* shell rather than a test shell.

**Versions (verified 2026-09-21).** `ratatui 0.30.2` (MSRV 1.88) with `crossterm 0.29` reached
through ratatui's own re-export (`ratatui::crossterm`), so the backend cannot drift from the widget
crate. The backend split matters: `ratatui-core`, `ratatui-widgets` and `ratatui-crossterm` are
separate crates now, and the `Backend` trait no longer exposes `buffer()` — `TestBackend::buffer()`
is the inherent accessor a view test uses. `ratatui::init()`/`restore()` own raw mode + the
alternate screen; mouse capture is still crossterm's to enable.

**What the spike measured** ([`spikes/tui-shell/`](spikes/tui-shell/), isolated workspace, scoped
identically to the iced spike): the TUI boots against the live host (real device at 48 kHz / 2 ch),
renders transport + channel/master meters + the position readout, exits cleanly on `q` and restores
the alternate screen and mouse capture; the `?` keymap overlay is rendered from the same table the
key handler matches; clicks map to transport actions and meter-row selection; and a headless
`--probe` observes live meter signal (peak 0.88), the same figure the iced spike reports. The
evaluation note is
[`.agents/notes/proposed/architecture/2026-09-21-tui-shell-evaluation.md`](.agents/notes/proposed/architecture/2026-09-21-tui-shell-evaluation.md).
Running the shells against live audio also surfaced a host-side rough edge: the underrun counter
jumps once by ~2–3 s of device frames on the first transport command — including a seek while
stopped — and then stays flat
([bug-fix note](.agents/notes/proposed/bug-fix/2026-09-21-live-host-underrun-burst-on-transport-change.md)).

**A rough edge the shell hit, and where it belongs (2026-09-22).** `--wave` on a 44.1 kHz file
stopped the transport (`clip 'c0' source is 44100 Hz but the session is 48000 Hz`). The refusal is
right — a rate-mismatched take would play ~8.8% flat with no counter — but there was no **import**
door at all, so the most common music rate was unusable. The fix is *not* shell-side: the pool owns
the rate boundary now (`Pool::import`/`Pool::conform` with a band-limited 128-tap Kaiser resampler;
converted once at the boundary, original preserved, and `set_pool` conforms as it adopts), so any
shell gets it. The spike's `--wave` therefore imports a copy into its own session pool
(`$TMPDIR/tui-shell-pool-<pid>`) instead of pointing `pool` at the user's directory — the pool is
session-owned working material. The 48 kHz-vs-44.1 kHz question and the conversion boundary are
recorded in the
[session-rate note](.agents/notes/implemented/architecture/2026-09-22-session-rate-and-source-conversion.md).

**Verifying a TUI is cheap** — which is itself part of the case for it: `TestBackend` makes the view
testable (a deterministic `--dump` frame, embedded in the spike's README, plus five assertions on
the rendered buffer), and a pty harness (`script -qec` with `stty rows/cols`) covers the real path
(raw mode, alternate screen, mouse capture, input decoding). The pty run is what found the spike's
zero-height crash.

**What is still open — the questions that decide it.**

1. **The timeline.** The density is now known analytically (see *Reference and prior art* below):
   a terminal gives an overview envelope, not samples. What is still missing is the **built
   prototype** — envelopes from the real peak pyramid, clip boundaries, playhead, and the keyboard
   editing gestures — and an honest statement of what it cannot do.
2. **Mouse vs keys.** The working hypothesis is that a well-designed key scheme carries the
   workflow and the mouse is a bonus; mouse capture in a terminal is global (it takes over
   copy/paste) and interacts with multiplexers. A task-level comparison is the test.
3. **Widget inventory.** Expected to be *smaller* than iced's (fewer dialogs/menus needed if keys
   carry it) but with a lower floor (no IME, no rich text, no colour fidelity guarantees).
4. **Sizes and fonts.** Anything that only works at one terminal size is not shippable; the
   zero-height crash is the first instance of a whole class.

**Reference and prior art (2026-09-21).** Two questions the spike left open now have answers in the
[Helix + terminal-drawing study](research/architecture/2026-09-21-tui-audio-prior-art.md):

- **The interaction model.** [Helix](https://github.com/helix-editor/helix) is the reference, and
  not for its text engine: **selection-first** ("select, then act") fits clips better than text —
  the selection *is* `{track, clip, start, end}` and it is rendered before anything is destroyed —
  plus a labelled keymap trie with "user wins" TOML overrides, a **prefix infobox** (better than the
  spike's full-screen `?` help), a command palette with a reverse-bound-keys column, the statusline
  as configured data, and **docs generated from the live keymap**. Crucially, the `:` prompt needs
  *no new vocabulary*: the versioned `host v1` text format already is that command line.
- **The timeline.** It is a density question, and the answer is **overview, not samples**: ~400 time
  columns and ~200 amplitude steps at 200×50, 0.45 s/column for a 3-minute clip, zoom floor ~0.6 ms
  (~30 samples). So the design is a **min/max envelope per column from the existing peak pyramid**
  (`PeakBuilder::range_minmax` — the `audiowaveform` model, no new audio work), braille (2×4
  sub-cells, one colour per cell) for lanes and blocks (colour per sub-pixel) for clip bars, meters
  and playhead. Terminal **image protocols are a detail-pane luxury, not the timeline**: Kitty is
  stateful and can draw under text, sixel is immediate-mode and erased by text over it, tmux has no
  Kitty support, and Alacritty/Konsole are unusable. No terminal app was found that draws an
  *editable* multi-track waveform timeline — the ground is unclaimed.

**Clips, tracks, and edits (2026-09-21).** The spike now draws an **arrangement**: `--script <host
script>` loads `pool`/`arrange` lines into the host and the panel renders the engine's own `Timeline`
— one lane per track, a braille min/max envelope per clip (gain and fades drawn from the clip's
fields, baked loops honoured), per-track colour, `▏`/`▕` boundaries — and **edits go through the
host's own text format** (`x` split, `d` delete build an `arrange …` line, hand it to
`host::parse_arrange_line`, and the engine logs it; a refused op is reported, never logged).
**Panel focus** (`Tab`, click-to-focus) makes `j`/`k` mean "mixer channel" or "active track"
depending on where the keys point. Measured: a 9 s / 3-track / 5-clip arrangement fits a 100×26
terminal at **98.917 ms/col**, the zoom-out ceiling is the arrangement **plus a 4× margin**, and the
zoom floor stays the pyramid's one bin (5.333 ms/col). A 6 s seek blocks the UI thread for
**~92 ms** — `TransportSeek` is O(target), now measured rather than theorised.

**The mixer is a console now (2026-09-21).** With an arrangement loaded the mixer takes the right-hand
column as **channel strips** — a fader with a visible position, a meter fed by the host's snapshot, a
name with mute/solo flags, a value, and a master strip. Keys ride it (`+`/`-`, `0`, `M`, `S`) and the
mouse can click or drag a strip; every change is dispatched as `set_param mixer …` **through the
host's text format**, so a fader ride lands in the session log as automation
(`last command: set_param (0.1 ms)` — unlike `TransportSeek`'s ~92 ms O(target) rebuild). Known gap:
the shell cannot *read* gains back from the host yet, so a reload resets faders to unity.

**State is a fold of the log (2026-09-21).** The Host API gained a **`params`** value — the session's
current parameters, **folded from the log** (mount params → `SetParam`, last write wins; unmount
removes) — so a shell derives its controls instead of mirroring them
([note](.agents/notes/implemented/architecture/2026-09-21-shell-state-is-a-fold-of-the-log.md)). The
TUI's console now shows what the log says (`ch0.gain 0.3` in a script → 0.30 on the strip), `u`/`Ctrl+r`
undo/redo by the host's **log replay** with the faders untouched, and `--wave <file>` is *audible*
because the shell hands the host a synthesised one-clip script rather than drawing a private picture
of the file (`--probe --wave` reports the file's amplitude on ch0 and the centre-pan law on the
master).

**Where it stands:** a spike, not a decision. The shipped shell stays Tauri + Vue.

**Prior art, precisely (2026-09-21).** The [study](research/architecture/2026-09-21-tui-audio-prior-art.md)
§3 surveyed the field and §5 draws the line. The gap is narrow but exact: **no terminal program
draws a waveform in a multi-track arrangement.** Everything waveform-capable is a single-file sample
editor (`tui-wave` — the closest prior art, alive, braille everywhere plus Kitty graphics,
cut/copy/undo on **one** buffer; `MrDopey/audio-tui-editor` — agent-built, destructive in-place
trimming), a live meter/scope with no file (cava, Open Cubic Player, Prism at 60 FPS, scope-tui,
sgram-tui), or a MIDI-only DAW (Phosphor, tek's arranger). None has a timeline; none has a
non-destructive clip model over a log. Since the three closest are agent-built or LLM-assisted, the
rendering is not the moat — **the clip/timeline model is**. And the interaction direction is now
written down: [modal editing: audio's vim/helix](.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md).

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
- **Non-destructive, clip-based data model** — the heart of the edit-as-composition workflow:

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

Three tiers, matching "extendable to include audio effects". Tier 2 below is **not arranger functionality** — it belongs to the deferred sound-sculptor profile (§11, [umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)); Tier 1 real-time effects are arranger Phase 2–3 territory:

### Tier 1 — real-time (in-engine)
`fundsp` nodes on tracks, aux buses (dub sends → delay/reverb) and master (EQ → compressor → limiter). **Phase 2.** Optionally, **Faust-authored DSP** as a second block source behind the same `AudioNode` (build-time AOT to Rust; §10, [evaluation](research/architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md)).

### Tier 2 — offline "processes" (CDP8) — *the extension point you asked for*

> **Direction changed (2026-09-21):** this tier is now **invoking external programs** (CDP8, sox,
> ffmpeg, `tui-wave`) on rendered sources and importing the result as a new pool source — not
> wrapping them as shipped sidecar artifacts. See the
> [no-sidecars note](.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md).
> The rest of this section stands as the research that shaped the seam.

CDP8 and PaulStretch are **file-based, offline** processors. Model them as one abstraction:

```
OfflineProcess { kind, params, input: Source }  →  job queue  →  new Source
```

- Run the external program as a **Tauri sidecar** (`bundle.externalBin`) on a rendered temp WAV; watch progress; import the output as a new `Source`. The job queue lives in Rust, runs in the background, emits progress events — shown in the UI like a render/bounce.

| Processor | Status / notes |
|---|---|
| **CDP8** (Composer's Desktop Project, Release 8) | `https://github.com/ComposersDesktop/CDP8` — **C** codebase (CMake), **LGPL-2.1-or-later**, actively maintained (last push 2026-06, ~726★). Large CLI suite (distortion, envelopes, filters, granular, morph, pitch, reverb, spectral, time — e.g. `blur`, `brassage`, `stretch`, `focus`). Release 8 adds ~80 Trevor Wishart programs (waveset distortion, speech/voice, multichannel ≤8) and **PVOCEX (`.pvx`) phase-vocoder** analysis across all pvoc programs. Existing GUIs: Soundloom 17.0.4, Tabula Vigilans. |
| **PaulStretch** (Paul Nasca) | Canonical "extreme stretch → lush pad" — **`GPL-2.0-only`, so it cannot be embedded or ported into our GPL-3.0 work** (verified 2026-09-30). Cited as the *sound* to reach, not a component to adopt; CDP8's `.pvx` pvoc tools are the compliant path. |
| **`ness_stretch`** (Rust, 0.5.1) | NessStretch spectral stretch — but **no license is declared** (crates.io `license=None`; repo has no LICENSE). **Do not ship it** without asking the author. |
| **`paulstretch` Rust crate** | **Does not exist** on crates.io. |

> **CDP distribution candidate (2026-10-06):** [`cdp-wasm`](https://github.com/cdp-wasm-suite/cdp-wasm) — Oli Larkin's WASM port of the *same* CDP8 C sources (215 programs, parity-tested bit-for-bit against native; MIT wrapper + LGPL-2.1-or-later modules; npm + a `cdp` CLI argument-identical to native; a curated typed catalog of 232 effects with param ranges). Prebuilt — no C/CMake toolchain — so it is the leading *distribution* for the external-programs seam, not a new architecture: the seam can invoke CDP through either distribution interchangeably. Explored hands-on (installed under Node 24, CLI + library API, timings): [the exploration](research/architecture/2026-10-06-cdp-wasm-suite-exploration.md).

### CDP8: embed vs port — verdict

**Embed the C (sidecar CLI); don't port the suite.**

1. CDP8 is ~800 programs / ~20 MB of C — porting is a multi-year effort with subtle DSP-regression risk.
2. The programs are offline/file-based anyway (render a WAV → run → import), so a subprocess is a *natural* fit — you'd gain almost nothing from in-process FFI for batch work.
3. Sidecar = **zero copyleft obligation** on your app (LGPL applies to CDP8 itself; you ship its `LICENSE` + point to the source).
4. **Port only a single algorithm** when you need it real-time (e.g. one pvoc effect on a live track) — then either (a) port that one function as an **LGPL-2.1-or-later** Rust translation (a derivative work — keep it LGPL, or fold it into a GPL-3.0 app), or (b) **clean-room reimplement** it with `rustfft` so it's yours under GPL-3.0.

For the "PaulStretch sound," the clean paths are CDP8's own `.pvx` pvoc tools (LGPL) or your own `rustfft` phase-vocoder stretch (GPL-3.0) — not `ness_stretch` (unlicensed).

> The CDP wrapping idea is the **offline-process tier** (§6) — now built as a `plugins/` **sidecar** (`OfflineProcess`), not as a separate frontend repo. The engine stays in Rust; no standalone frontend until the Tauri shell.

### Tier 3 — Rust audio plugin/DSP ecosystem (research, §10)

---

## 7. Workflow feature map

Concrete features by inspiration. **Bold = shipped or scheduled.** Each group is labelled with the
[profile](.agents/notes/proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md) that
owns it: **recorder** is the current focus, **arranger** follows it, **sculptor** is deferred.

**Capture — record (recorder)**
- **Record long takes** (mono/stereo; multitrack later) straight into the **media pool**; non-destructive; waveform preview via peak pyramids.
- **Markers while recording**; optional silence-based auto-split.

**Arrange — edit-as-composition (arranger)**
- **Razor/splice at playhead; ripple delete; copy/paste/duplicate regions; crossfades.**
- **Drag clips on the horizontal timeline; loop a clip region with adjustable loop crossfade.**
- **Trim/crop (src_in/src_out); move/layer takes on tracks.**
- Varispeed (linked pitch+speed) and independent time-stretch (`rubato`) — Acid-style "make it fit."
- Reverse; per-clip fade in/out; comping (later).

**Mix — soft mixer (recorder)**
- **Track strips: gain, pan, mute, solo; master fader; level meters.**
- Bounce/export the arrangement to WAV (`hound`).

**Sound — Feldman, time & texture (sculptor, deferred)**
- Seconds-based timeline, long durations, fine gain (0.1 dB), very long fades.
- Spectral/textural processing (CDP8 / own phase-vocoder) as "render to new source" actions.

**Dub — console-as-instrument (a usage pattern: two instances, arranger → recorder, Phase 2+)**
- Aux sends → delay bus (tape delay + feedback + filter) and reverb bus.
- **Live dub mixing** (mute/unmute sends, filter sweeps, fader rides) recorded as automation.
- "Drop the track, keep the echo tail" as a structural gesture.

**Master (recorder)**
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

"Rust audio plugins" spans three things — building plugins, hosting them, and the reusable DSP you build *from*. Current state (verified 2026-08-30; the Faust/HISE rows added 2026-09-20):

| Crate / framework | What it is | License | Verdict for us |
|---|---|---|---|
| **clack** (`clack-host` 0.2.0, `clack-plugin`, `clack-extensions`, `clap-sys`) | **Host CLAP plugins** in Rust; `clack-plugin` builds them | MIT OR Apache-2.0 | The active, permissive path to *load* third-party synths/FX (Cardinal, the clap ecosystem). 0.2.0, updated 2026-09-12. |
| **nice-plug** (`nice-plug` 0.4.2, `-core`, `-derive`, `-iced`, `-egui`) | Build **CLAP/VST3/standalone** plugins; declarative params + Serde state; baseview editor API; **standalone with JACK/MIDI/transport** | ISC (framework + examples) | **The live successor to `nih-plug`** (community fork on Codeberg, 19 releases since 2026-06-04, updated 2026-09-14, MSRV **1.88** — stable Rust). CLAP is the base; `vst3` is an opt-in feature that pulls the Steinberg SDK (a C++ build) and GPLv3-encumbers the artifact (fine for this GPL-3.0-or-later repo, worth knowing if ever relicensed). `assert_process_allocs` matches our no-alloc render invariant. **Proposed as the export framework for our own instruments** — see the [CLAP-export note](.agents/notes/proposed/architecture/2026-09-22-clap-export-via-nice-plug.md); `clack` remains the hosting path and the fallback. |
| **nih-plug** (`nih_plug_*`) | The upstream framework nice-plug forked | ISC framework; **VST3 bindings (`vst3-sys`) are GPLv3** | **Superseded** — in maintenance mode, not on crates.io (only sub-crates), community moved to nice-plug. Historical only. |
| **iced_audio** (0.17) | Audio widgets for iced (faders, knobs, XY pad, ramps) + unit ranges and modulation display | MIT | **Adopted for the iced shell's mixer** (§4.5): depends on `iced_core`/`iced_graphics` 0.14 — the shell's iced — and has a `nice-plug` feature to bind its widgets to plugin parameters, plus a `nice-plug-iced` editor adapter. |
| **vst** (0.4.0) | VST2 build/host | — | Legacy/frozen; VST2 SDK is closed — ignore |
| VST3 *hosting* | — | — | Immature in Rust; don't plan on it |
| **Faust** (`faust -lang rust`, 2.88.0) | **DSP language** with an official in-tree **Rust backend** (AOT; `-rnt`/`-rnlm`/`-it`/`-noreprc`), Rust architecture files + `faust2cpalrust`/`faust2jackrust`/`faust2portaudiorust` | Compiler **LGPL-2.1-or-later**; generated code explicitly **not** covered (FAQ) | **Adopt as an optional DSP *source*** — build-time only, no crate/FFI/alloc, wrapped behind our `AudioNode`; a peer of fundsp. Caveats: ≈1.3× slower than the C++ backend on delay-heavy DSP; `RwLock` tables unless `-it`; `soundfile`/`-quad`/`-omp` unsupported. [evaluation](research/architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md) · [note](.agents/notes/proposed/architecture/2026-09-20-faust-as-optional-dsp-source.md) |
| **HISE** (C++/JUCE) | Instrument-builder *environment* (scriptnode graph, SNEX JIT, a Faust node) | GPL-3.0-or-later + paid commercial licence | **Inspiration only** — no Rust anywhere, not a library, and **no CLAP export**. Mine its graph→compiled-node ladder and user-DSP tier. [evaluation](research/architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md) |
| **fundsp** (0.23.0) | Composable DSP + effects + synth; static `AudioNode` + dynamic `AudioUnit`/`Net` (runtime arity, one-shot `backend()`/`commit()`) | MIT OR Apache-2.0 | **Active (2026-01-07).** Use for per-sample *blocks* (osc/filter/env) behind our own `AudioNode` — *not* its graph, which is compile-time/acyclic/non-serializable with no note/event type. |
| **dasp / dasp_graph** (0.11.0) | PCM primitives + DSP graph | MIT/Apache-2.0 | **Dormant since 2020.** Reference for sample/frame types; not for new work. |
| **augmented-audio** (`augmented-dsp-filters` 2.5.0, `augmented-oscillator` 1.4.0, `augmented-midi` 1.8.0) | Grab-and-use DSP building blocks (filters/osc/midi) | MIT (lib crates); **full apps AGPLv3** | **Dormant (~2024), labeled "draft/experiments"; no published `adsr`/`music-theory`.** Algorithm reference only; use only the MIT lib crates. |
| **rustfft** (6.4.1) | FFT | MIT/Apache-2.0 | Spectral effects, phase-vocoder stretch |
| **rubato** (5.0.0) | Resampling / time-stretch | MIT | — |
| **glicol** (0.13.5) | Graph-oriented live-coding DSP engine (Rust, wasm-capable) | repo MIT; **crates.io "non-standard"** | **Experimental, dormant (~2024-04).** Don't adopt its DSL/engine; a reference. |
| **audio-graph** (0.1.0) | New DAG audio graph for DAWs (typed ports, topological scheduling) | MIT | Premature (1 release, ~650 lines). Watch, don't adopt. |
| **rsynth** | Polyphony / voice-allocation utilities (`EventDispatcher`, `SimpleVoiceState`) | MIT | Possible helper for voice stealing later |
| **rtrb** / **basedrop** / **audio_thread_priority** | RT-safe ring buffer / memory / thread priority | MIT/Apache-2.0 | Audio-thread safety |

**What this means for the effects/instrument plan:** built-in effects = our own `AudioNode` blocks using **fundsp** (or Faust-generated Rust) for the DSP; load third-party synths/FX = **`clack`** (CLAP, hosting); **export our own instruments** = **nice-plug** (CLAP base, VST3 and a JACK standalone behind features) — proposed, deferred, instruments only, never the arranger ([note](.agents/notes/proposed/architecture/2026-09-22-clap-export-via-nice-plug.md)). All permissively licensed → compose cleanly with GPL-3.0 (§12); the nice-plug + `nice-plug-iced` + `iced_audio` trio shares the shell's GUI stack.

**Building our own soft synth in Rust** — the landscape splits into three kinds of material: **(1) building-block DSP libraries** (fundsp, dasp, augmented-audio, glicol), **(2) graph/DSP engines** (fundsp `Net`, glicol's engine, dasp_graph), **(3) plugin build/host frameworks** (clack, nih-plug). For our own voices we use **fundsp/dasp as the block *algorithm* source and keep our own `AudioNode`/patch-bay graph as the abstraction** — our patch is a loggable, diffable value, whereas fundsp's graph is compile-time/acyclic/non-serializable and carries no note/trigger/event type; glicol's engine is an experimental DSL. **Added 2026-09-20: Faust is the second block source, not a second graph** — `.dsp` compiled AOT to Rust behind our `AudioNode`, an optional feature-gated peer of fundsp, with the voice/polyphony layer still ours (measured: zero-alloc render, byte-deterministic codegen, ≈1.3× the C++ backend on a delay-heavy reverb). Evaluation: [research](research/architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md); decision: [note](.agents/notes/proposed/architecture/2026-09-20-faust-as-optional-dsp-source.md). SuperCollider, Csound and every third-party CLAP instrument remain external hosts/embedded targets (the A/B/C routes in the [integration-routes](research/architecture/2026-08-19-softsynth-integration-routes.md) research), never the mechanism for our own voices. Decision: [note](.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md). **Validated 2026-08-30** by a feature-gated spike in `crates/engine` (`plugins::fundsp_synth`): a monophonic fundsp voice as an opaque `AudioNode`, passing the engine's zero-alloc-render, byte-identical-replay, sample-accurate-onset, `set_param` and PDC invariants; `fundsp` is an **optional** dependency (feature `fundsp`) so the minimal core stays dep-lean by default. Validating the spike in release surfaced a pre-existing, release-only engine bug (scheduled mounts never applied — `debug_assert!` swallowed `apply_mount`), now fixed: see the [bug-fix note](.agents/notes/implemented/bug-fix/2026-08-30-scheduled-mounts-apply-in-release.md) and the [soft-synth note](.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md).

---

## 11. Phased plan (revised — core + plugins)

> **Alpha status (2026-09-24).** The reviewed alpha finish line
> ([plan](.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md)) has been executed:
> foundations (one gesture = one undo, sessions with a crash-safe journal), recording (stereo and
> 24-bit material), the grid and the edit vocabulary, the shape-and-length gestures (utility
> transforms and offline WSOLA tempo match), the mastering chain and `export`, markers and clip
> names, and seeking at scale (a warm-up seek, proved equal to a replay). The plan's item 16 — this
> documentation pass — is what added [`docs/tui-first-session.md`](docs/tui-first-session.md),
> [`docs/capabilities.md`](docs/capabilities.md) and
> [`docs/beta-acceptance.md`](docs/beta-acceptance.md). Each slice's decision lives in its own note
> under [`.agents/notes/implemented/`](.agents/notes/implemented/), and each architectural one passed a
> cross-vendor reviewer gate (archived under [`research/architecture/`](research/architecture/)).
> The deliberate cuts and the measured limits are stated in
> [`docs/capabilities.md`](docs/capabilities.md), not here.

Architecture: a **minimal core** — clock, audio graph interpreter, session event log, context plumbing (the [minimal-core note](.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md)) — with every capability as a plugin; a product is an assembled profile. The platform is **`audio`**, assembling three profiles: **recorder** (the current focus — clock out, capture, align, mix, master, export), **arranger** (the ACID clip arranger; committed, sequenced after the recorder) and **sculptor** (audio transformation; deferred) — [profiles note](.agents/notes/proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md). Direction locked 2026-08-15: **umbrella-first** — the platform is the goal, the phases below build it ([umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)).

> **The sequence is a plan, not a gate.** These phases state an *intended order* and a shared
> scope vocabulary; they are not authorisation gates. In particular **Phase 3 is a suggestion**:
> the transition into it is an explicit decision, taken when Phase 2 is in a state the author is
> happy with — not an automatic consequence of Phase 2's feature list being finished. A deferral
> resting on *scope* (which profile owns a capability) is not a gate; one resting on *dependency*
> (X cannot work until Y exists) is. See
> [the phase note](.agents/notes/proposed/process/2026-09-10-phase-sequence-is-a-plan-not-a-gate.md).

- **Phase 0 — Core spikes (x86 Linux):** *Spike A* — **shipped 2026-08-15** (`crates/engine`): core (clock with tempo/meter map + sample-accurate scheduling queue, graph interpreter with two node tiers + PDC, session log with frame-carrying events, context with `inject` + disposers) + the euclidean plugin; 20 tests, incl. byte-identical replay, sample-accurate lifecycle, zero-allocation render; see the [implemented note](.agents/notes/implemented/architecture/2026-08-15-phase-0-spike-a-core.md). *Spike A.5 — the patch bay* — **shipped 2026-08-17**: named typed ports (audio/control/trigger/note) + patch cords + the euclidean→scale→tone chain as data, MIDI/OSC seam traits, 27 tests; see the [implemented note](.agents/notes/implemented/architecture/2026-08-17-patch-bay-typed-signal-streams.md). *Spike B — the media engine* — **shipped 2026-08-17** (`crates/media`): a 25-min file streams from disk while recording the master, a splice at minute 15 is sample-accurate with a 512-sample equal-power crossfade, zero underruns/overruns, crash-recoverable takes, device-clock drift kept the timeline in session frames, byte-identical bounce on replay, and the cpal device path ran on real hardware (Scarlett 2i2); 22 always-on tests + 2 hardware/soak; see the [implemented note](.agents/notes/implemented/architecture/2026-08-17-spike-b-media-engine.md) and the [kimi review](research/architecture/2026-08-17-kimi-review-spike-b.md). The core (`crates/engine`) stayed std-only and untouched through all three spikes — media nodes mount as opaque-tier `AudioNode`s. All spikes live in the engine workspace — no Tauri, no frontend; scaffold the app shell only after they pass.
- **Phase 1 — The sound-arranger profile (the old product goal):** recorder plugin (`cpal` → media pool, live peaks) + clip-editor plugin (cut/paste, razor-split, trim, copy/paste/duplicate, crossfade, loop regions, paint-to-fit time-stretch, drag on the horizontal canvas timeline) + soft mixer plugin (gain/pan/mute/solo, master fader, meters, bounce to WAV). Seconds-based timebase. **P1.0+P1.1 (foundation + soft mixer) — shipped 2026-08-18** (`crates/engine`): logged `SetParam` (declared `params()` surface, sample-accurate, replayable, refusals never logged), per-port audio fan-in (many audio Ins, one Out, `MAX_AUDIO_INS` enforced), the mixer plugin owning the master bus (4 channels = the EP-133's groups, gain/mute/solo, master fader, meters via the `mixer.meters` context service, mono — pan arrives with stereo), the tone no longer claims the output, and `media::bounce` (16-bit; the pool keeps float). 15 new tests incl. counting-allocator; see the [implemented note](.agents/notes/implemented/architecture/2026-08-18-p1-foundation-and-mixer.md) and the [kimi review](research/architecture/2026-08-18-kimi-review-phase1-foundation-mixer.md). **P1.2 (recorder + adaptable mixer) — shipped 2026-08-18** (`crates/media` + mixer): the mixer's `channels` is now a mount param (1..=8 at the time; any width since the [mixer-width note](.agents/notes/implemented/architecture/2026-09-27-mixer-width-is-a-mount-parameter.md), 2026-09-27) the profile sets from the device — the Soundcraft Notepad-12FX's 4 USB capture channels are the target (class-compliant USB, per-channel multitrack); `Capture` (interleaved source ring → demux → per-channel rings + **float-WAV pool sources + live 256-sample peak pyramid** sidecars), `CaptureNode` monitoring into the mixer (graph-level — the log-visibility carve-out is recorded), float WAV writer, source-ring overrun counting, hardware capture verified on the Scarlett 2i2. See the [implemented note](.agents/notes/implemented/architecture/2026-08-18-p1-2-recorder-and-adaptable-mixer.md) and the [kimi review](research/architecture/2026-08-18-kimi-review-p1-2-recorder.md). Next: P1.3 clip editor (per-clip params need a target-id / interned strings — recorded; the pool gains enumeration + crash-recovery). **UI as a plugin — shipped 2026-08-18** (`crates/host`): the Host API contract (commands with real `at_frame`s — the log is the command list — plus events `meters()`/`log()` and values `providers()`), versioned (`host vN`); the headless reference host + smoke binary (`run_script` assembles the profile and bounces byte-identically); Tauri v2 becomes the first rich shell next (thin transport adapter + Vue plugin runtime; engine/media stay shell-free). See the [implemented note](.agents/notes/implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md) and the [kimi review](research/architecture/2026-08-18-kimi-review-host-api.md).
- **Phase 2 — Generators & improv:** euclidean rhythm and chord-progression plugins (pure generators providing `ctx.rhythm` / `ctx.progression`); an improv plugin consuming the session log and responding — the paper's self-evolving component; made *auditable* by the log, not reversible (minimal-core note). **Decoupled pitch & rhythm (design, 2026-08-20):** rhythm objects (euclidean/interval pulses) emit identity-tagged triggers, note lists become a named shared service, and a selection-policy combinator (`forward/reverse/fwd_rev/random/markov/follow_pulse`) joins them as a patchable cross-product — Polypulse's mechanics, expressed as data (see the [decoupled note](.agents/notes/proposed/feature/2026-08-20-decoupled-pitch-rhythm-generators.md)). **External synth providers (Route B):** SuperCollider via the `OscSink` seam (notes → scsynth `/s_new`) + JACK audio; Tidal via a SuperDirt-compatible `OscSource` endpoint ([integration routes](research/architecture/2026-08-19-softsynth-integration-routes.md)).
- **Phase 3 — Real-time effects:** `fundsp` effect plugins + dub sends.
- **Phase 4 — Master & export:** master chain (EQ/comp/limiter), automation, FLAC/MP3 codecs (`Codec` plugins); CLAP hosting (`clack`) — **external synth providers (Route A): Cardinal and the clap ecosystem mount as opaque nodes, their state stored in the composition** — and plugin export (`nih-plug`). **Vertical scaling (2026-09-03):** Cardinal-as-CLAP-plugin is the multicore path — mount **multiple Cardinal instances as opaque nodes** and render them in parallel across cores (software=engine, hardware=interface; distributed network-audio only when one machine's CPU/RAM is exceeded, latency-tolerant because the arranger records). **Cardinal is Rack-1 based and self-contained** (bundles its open-source module set; Rack-2 modules won't load) — plan around its bundled modules, or pay VCV Pro / build a Rack-2 host for your own — [software engine + hardware interface; scaling](.agents/notes/proposed/architecture/2026-09-03-software-engine-hardware-interface-and-scaling.md).
- **Phase 5 (separate) — ARM/Pi 5 + hardware controls:** `midir` + GPIO/Daisy, headless box mode. Only then revisit §8.
- **Deferred, own profile — "sound sculptor":** offline/non-realtime processing — CDP8 sidecar, PaulStretch, phase-vocoder stretch as `OfflineProcess` plugins with progress events; **Csound offline runs here too** (the same sidecar slot; realtime `libcsound` embedding is a later Route-C adapter behind an opaque node). Unscheduled; no earlier than Phase 3, and only after the arranger profile and generators have exercised the seams ([umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)).

---

## 12. Licensing & legal

**Recommendation: license the app under GPL-3.0-or-later.**

- "Compliant open-source" + "best technical solution trumps" ⇒ a copyleft license means you are *almost never blocked* from using the best tool: LGPL/ISC/MIT/Apache/MPL and **GPL-2.0-or-later** all compose with GPL-3.0.
- Hobby/boutique (not mass prod, price not an issue) ⇒ copyleft's only real obligation ("publish source when you distribute binaries") is a non-issue.
- **The one thing GPL-3.0 cannot absorb is `GPL-2.0-only` code** — verified 2026-09-30. GPLv2-only is incompatible with GPLv3 (it offers no "or later" upgrade path), so a GPLv2-only component cannot be linked into a GPL-3.0 work. This is the one real boundary on "best tool wins". **Settled: it does not move us.** The GPLv2-only components we looked at (PaulStretch) were illustrative, not required — spectral stretching is the sculptor profile's job via a CDP8 sidecar (`LGPL-2.1-or-later`), not an in-engine port. `GPL-3.0-or-later` is the licence; do not reopen this for a convenience algorithm. (For the record, if a GPLv2-only component ever did become indispensable, `GPL-2.0-or-later` is the maximum-optionality licence, because `-or-later` upgrades forward and so absorbs both GPLv2-only and GPLv3-only code — at the cost of GPLv3's patent grant and anti-tivoisation terms.)
- **AGPL-3.0-or-later is deliberately not adopted** — verified 2026-09-30. AGPL differs from GPLv3 in exactly one substantive way (AGPL §13, superseding GPL §13): if users interact with a *modified* version **over a network**, the operator must offer them the source. It is dormant for a locally-installed desktop tool, so it would buy us nothing, and it would cost us: AGPL is widely avoided by third-party plugin and DSP developers, and the whole architecture is a plugin ecosystem. More decisively, it unlocks no code we want — Zrythm is the main AGPL peer and carries additional §7 terms besides. **Because we are `-or-later` we can still switch to AGPL-3.0 later unilaterally** (BSD-style: the FSF designed `-or-later` for exactly this upgrade); going the other way would not be possible. That optionality is worth more than the licence itself, which is the strongest argument for keeping `-or-later`.
- **The window on relicense freedom is open only while there is a sole copyright holder** (verified: every commit author/committer is the owner or his own agent identities). It closes on the first accepted outside contribution.
- **Alternative** if you prefer permissive: **MIT OR Apache-2.0** — but then CDP8 must remain a strictly separate sidecar and you may not fold any LGPL/GPL component into the engine (fine, but it constrains "best tool wins").

Dependency compatibility matrix (verified where marked):

| Dependency | License | Compose with GPL-3.0 app? | Notes |
|---|---|---|---|
| CDP8 | LGPL-2.1-or-later ✅ | ✅ | Sidecar = no obligation. Static link/port = derivative, keep LGPL or fold into GPL-3.0 |
| PaulStretch | `GPL-2.0-only` (verified 2026-09-30) ❌ | ❌ **cannot be linked into a GPL-3.0 work** | GPLv2-only has no "or later" upgrade path. Reach the *sound* via CDP8's LGPL `.pvx` pvoc tools instead |
| `ness_stretch` | **none declared** ⚠️ | ⚠️ | Avoid for release; ask author or use CDP8 pvoc / own `rustfft` stretch |
| nih-plug | ISC (framework); VST3 bindings GPLv3 ⚠️ | ⚠️ | Permissive framework, but VST3 output is GPL-encumbered; in maintenance mode — use clack instead |
| clack | MIT OR Apache-2.0 ✅ | ✅ | Permissive |
| fundsp | MIT OR Apache-2.0 ✅ | ✅ | Permissive |
| augmented-audio (lib crates) | MIT ✅ | ✅ | Permissive libs only — the full apps are AGPLv3; dormant (~2024) |
| glicol | repo MIT; **crates.io "non-standard"** ⚠️ | ⚠️ | crates.io license field is non-standard; experimental + dormant — avoid for release |
| symphonia | MPL-2.0 ✅ | ✅ | File-level copyleft; keep its sources available |
| cpal / rubato / hound / rustfft / dasp / midir / rppal | MIT / Apache-2.0 ✅ | ✅ | Permissive — still verify each repo's LICENSE before release |
| rayon / crossbeam / rtrb / basedrop | MIT / Apache-2.0 ✅ | ✅ | Permissive |

**Porting CDP8 algorithms to Rust:** a port is a **derivative work** of LGPL-2.1-or-later code, so the ported files stay LGPL-2.1-or-later (compatible inside a GPL-3.0 app). If you instead reimplement from scratch (no copying), it's yours under GPL-3.0. Either way you're compliant — this is *not* a blocker.

---

## 13. Dev environment & audio latency

- **Host-native dev (2026-09-10).** Dev uses the host toolchain: system `rust` (rustc/cargo/rustfmt/clippy), `node`/`pnpm`, and the host GTK/WebKit/Mesa (`base-devel`/`pkgconf`, `webkit2gtk-4.1`, `gtk3`, `libsoup3`, `librsvg`, `libayatana-appindicator`, `alsa-lib`, `openssl`, `appmenu-gtk-module`). The Nix + devenv config (previously `flake.nix` / `devenv.nix` / `.envrc`) is **parked under [`nix/`](nix/)** and reproducibility is deferred: on a non-NixOS host a Nix-built GUI binary cannot open a window (the Nix glvnd ships no EGL vendor, and host WebKit needs `GLIBC_2.44` vs the Nix toolchain's 2.42) — see the [native host dev toolchain note](.agents/notes/implemented/process/2026-09-10-native-host-dev-toolchain.md).
- **Kernel / latency on Linux & NixOS**: see [`docs/audio-latency.md`](docs/audio-latency.md). TL;DR — nixpkgs has **removed the `-rt` kernels** (PREEMPT_RT is mainlined since 6.12, so the separate package is gone); run `linuxPackages_latest`/`zen` + `rtkit` + `performance` governor, and talk to ALSA `hw:` from `cpal`. No RT kernel needed for an arranger.

---

## 14. Risks & open questions

1. **`ness_stretch` is unlicensed** — resolved by using CDP8's `.pvx` pvoc tools or a `rustfft` stretch instead.
2. ~~**PaulStretch's exact GPL version**~~ — **resolved 2026-09-30:** `GPL-2.0-only`, therefore not linkable into a GPL-3.0 work. PaulStretch is illustrative, not a requirement; the stretch path is the CDP8 sidecar. See §12.
3. **VST3 hosting is immature in Rust** — plan CLAP hosting (`clack`) instead.
4. **`rolldown-vite` + Tauri** — should be transparent (scripts are just called); confirm the dev-server port wiring.
5. **Tauri + Nix friction** — on x86 you can use plain system deps (webkit2gtk-4.1 etc.) and keep Nix optional; less of an issue than on a Pi.
6. **CDP8 sidecar adds a C/CMake toolchain step** to the build matrix (Phase 2+) — accepted trade-off for 800 mature DSP programs.
7. **Project name** — "sound-arranger" names the first profile, not the platform. Rename candidates: "audio", "sound". Cost of renaming: repo path, the `~/Projects/music` view, back-references from the theory corpus. Resolve before the first tag/release.
8. **Voice management** (allocation/stealing) is undesigned — invisible until euclid drives polyphony (Phase 2); decide then, not now.

---

## 15. Verified sources (2026-08-13)

**Versions (crates.io):** `cpal` 0.18.1, `symphonia` 0.6.1, `hound` 3.5.1, `rubato` 5.0.0, `fundsp` 0.23.0, `dasp`/`dasp_graph` 0.11.0, `rustfft` 6.4.1, `midir` 0.11.0, `rppal` 0.22.1, `crossbeam-channel` 0.5.16, `rayon` 1.12.0, `rodio` 0.22.2, `clack-host`/`clack-extensions`/`clap-sys` 0.1.1/0.1.1/0.5.0, `ness_stretch` 0.5.1, `augmented-dsp-filters` 2.5.0, `augmented-oscillator` 1.4.0, `vst` 0.4.0, `rtrb` 0.3.4, `basedrop` 0.1.3, `audio_thread_priority` 0.37.0.

**npm (latest):** `vue` 3.5.41, `reka-ui` 2.10.3, `naive-ui` 2.44.1, `primevue` 5.0.0.

**Licenses / repos:** CDP8 = LGPL-2.1-or-later (github.com/ComposersDesktop/CDP8, C + CMake, ~726★, push 2026-06-08); nih-plug = ISC (robbert-vdh/nih-plug, ~2938★; crate repo → codeberg.org/BillyDM/nih-plug); clack = MIT/Apache-2.0 (prokopyl/clack); augmented-audio = MIT (yamadapc); glicol = MIT (chaosprint/glicol, ~2996★); fundsp = MIT/Apache-2.0 (SamiPerttu); cpal = MIT/Apache-2.0 (RustAudio); `ness_stretch` = **no license declared** (crates.io `license=None`; spluta/TimeStretch has no LICENSE).

**Confirmed absent:** crate `paulstretch`.

**Faust / HISE (verified 2026-09-20, hands-on against Faust 2.88.0):** Faust 2.88.0 released 2026-09-09 (master-dev 2.89.0-dev; ~3.2k★); compiler **LGPL-2.1-or-later** (it shipped GPLv2 through 2.68.1 and was LGPL-2.1+ by 2.69.3 — relicensed late 2023); generated code explicitly outside the compiler licence ([FAQ](https://faustdoc.grame.fr/manual/faq/)); `-lang rust` emits a dependency-free struct with `FAUST_INPUTS/OUTPUTS` consts and a `usize`-count `compute`; the only published Rust crates (`faust-build`/`faust-types` 0.2.1, 2024-11-20, MIT/Apache, [Frando/rust-faust](https://github.com/Frando/rust-faust)) **no longer compile against 2.88 output** (missing `FaustFloat`); measured ≈1.3× C++ on `re.zita_rev1_stereo`, parity on a small osc+filter, zero render-path allocations. HISE (christophhart/HISE, develop HEAD `5d34685`, 2026-08-16, tags `v4.9.0`–`v4.9.3`, GPL-3.0-or-later + commercial licence): **zero Rust**, no CLAP export, not a library. Details: [evaluation](research/architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md).

**Local prior art:** the [Bol Processor (bp4) study — now folded into `research/prior-art/`](research/prior-art/) (sound-object placement semantics: pivot/cover/gap/relocation, pre/post-roll; rational integer-ratio time with LCM-aware quantization; strong prior art for clip placement and auto-arrange), `~/Projects/pi5-daisy-synth-rig` (Pi 5 + JUCE + Daisy + JACK + USB-MIDI), `~/faust-juce-writeup.md` §6.8–6.10. The audio-UI stack (reka-ui + shadcn-vue + Tailwind v4) is documented in §17 and the [app-shell note](.agents/notes/proposed/architecture/2026-08-13-tauri-vue-rust-app-shell.md) — the separate `cdp-front` scaffold was dropped (CDP is now a sidecar plugin).

---

## 16. Plugin architecture research — "everything as a plugin" (paper · cordis · deepseek-harness)

> Working research, 2026-08-15. The decision candidates live in the note [Composition seams — plugin architecture for the engine and host](.agents/notes/proposed/architecture/2026-08-15-composition-seams-plugin-architecture.md); this section holds the background. Primary sources read in full: the paper text at `.research/paper/paper.txt` (88-page PDF, draft 2026-08-13) and a local checkout of deepseek-harness; a reading companion sits at `.research/paper/SUMMARY.md`.

### 16.1 cordiverse/paper — the theory: dynamic composition made formal

Preprint (draft 2026-08-13) by Yifan Shi & Wei Zhang (Peking University) and Tianyi Cui (DeepSeek-AI). Thesis: static composition (functions, modules, inheritance) has formal foundations; **dynamic composition** — loading, unloading, and reconfiguring components at runtime, as plugin systems and self-evolving agent harnesses demand — has none. Two orthogonal dimensions:

- **Temporal composability** — on removal, a component's side effects on the shared environment must be fully reversed.
- **Spatial composability** — components declare dependencies; the runtime resolves and re-wires them reactively as providers appear, disappear, or change.

Both are lifted from compile-time type systems to **runtime mechanisms**:

- **Revertible effects**: an effect is a context transformation paired with an inverse, `Γ → Γ × (Γ→Γ)`. The runtime accumulates inverses (LIFO) in an effect context `∂Γ = Γ × (Γ→Γ)`; unloading a component applies the accumulator. The author writes an inverse only per *atomic* effect; the inverse of any composite is derived by composition — teardown is derived from loading, not written alongside it. Withdrawing one component among many requires effect **independence** (commutation of transformation monoids; recovery in any permutation order, Corollary 21). Where effects don't commute, order is imposed by the accumulator (within a component) or a declared dependency (across components).
- **Reactive coeffects**: the environment is a typed dependency table `Σ = (k:K) ⇀ V_k`; `set` is itself an effect, so provision is revertible. A component declares a coeffect specification (the keys it needs); every context change is classified **activating / deactivating / neutral** and drives the lifecycle. Two refinements: **isolation** (realms — the same key resolves differently per context; runtime ad-hoc polymorphism / scoped DI) and **interception** (mergeable metadata changing *how* a dependency is used without touching the provider — the basis for access control).
- **The context paradigm**: effects and coeffects unify into one recursive context type `Γ∞ = μΓ. Γ × (Γ→Γ) × Σ` — every interaction with the environment passes through one explicit context. The paper positions it between functional state-threading (traceable, verbose) and implicit mutation (ergonomic, untraceable). Recovery is up to an **observational equivalence** ≃: behavior, not representation.

Then: a **calculus of dynamic composition** (components `(d, p, e)` instantiated as fibers with an inertial lifecycle state machine; metatheory: preservation, temporal/spatial composability, progress, confluence) and the **Cordis** implementation with the **Koishi** case study (chatbot framework, 4 years, 4000+ community plugins; every feature is a plugin; server and web console are two independent Cordis applications).

### 16.2 cordiverse/cordis — the meta-framework

TypeScript, by shigma (Koishi author); v4 in active development (API unstable). "Meta-framework": it fixes how effects and coeffects compose and leaves domain vocabulary to the application. Five ideas (official primer):

1. **A plugin is an object implementing Service** — a function with optional `inject` + `apply(ctx)`, or a `Service` subclass mounted by Cordis.
2. **A context is a repository of services** — a service claims a stable `ctx.<key>`; consumers find it by key, never by importing an implementation.
3. **Declare dependencies via `inject`** — load order is expressed through service requirements, not manual boot sequencing.
4. **Typed events** — names via TS declaration merging; dispatched `emit` (observe) / `waterfall` (wrap; `next()` delegates) / `parallel` (fan out) / `serial` (ordered, returns value); the dispatch mode is part of the event's public contract.
5. **Registrations are reversible effects** — installed via `ctx.effect()` / `ctx.on()`, unwound on reload and teardown.

Plus a **declarative loader**: the system is a configuration tree of entries `{id, url, config, disabled, isolate, intercept}`; the loader reconciles incrementally (keyed diff, only changed rows), and HMR swaps modules transactionally with rollback. Headline property: **path independence** — the final system state depends only on the declared config, never on load order. `group` / `include` / `hmr` are themselves ordinary components.

### 16.3 deepseek-ai/deepseek-harness — "everything is a plugin" in production

MIT agent harness (v0.1 developer preview, 2026-08-13) whose README states the architecture verbatim: "everything is a plugin", powered by Cordis. TS monorepo (~7,400 files, 230+ workspace members), ~100k stars within two days of release. DeepSeek used this exact harness to produce its published agent-benchmark scores (Terminal Bench 2.1 87.9, DeepSWE 62.7, Toolathlon-Verified 74.1 for V4 Pro); anyone can reproduce them via the Python SDK (`BENCHMARK.md`).

- **Composition**: a running dsh is a plugin tree built from ordered layers — profiles list bundles; bundles ship config rows (`dsh.bundle` → `cordis.patch.yml`); patches replace any row by id (whole-row replacement, not deep merge; later layers win); `dsh --profile web --dump-config` prints the exact booted tree.
- **Core service keys**: `ctx.sessions` (append-only SessionEvent log — "model-visible means logged" is a runtime invariant), `ctx.systemPrompt`, `ctx.tools`, `ctx.agents`, `ctx.agentLoop` (the driver is itself swappable), `ctx.llm` (adapter seam).
- **Events are the extension points**: session events (durable facts), agent events (live interception), capability events (policy at seams); turn/step flow (`turn/start` → … → `llm/stream` → `tools/execute` → `step/end` → `turn/end`).
- **Capability seams**: every swappable capability is a triple of Service Definition / Provider / Consumer; one provider swap re-points everything downstream (fs/subprocess → remote sandbox moves Bash, PTY, and LSP with it).
- **Self-referential**: the `extensions` package lets an agent mount/unmount its own plugins at runtime — the paper's motivating endgame, shipped.

### 16.4 The paradigm against design principles

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

### 16.5 The FP (Haskell) reading

The formalism is category-theoretic and maps cleanly onto FP:

- **Context = Reader environment**: `App a = ReaderT Context IO a`; a plugin is a pure description `Config -> Context -> App ()` the kernel interprets. Coeffects = typed `ask` (requirements); `inject` is a constraint set. The paper's own §6.4: typeclasses (Haskell) / traits (Rust) are how a host extends the context type — a provider is an instance, a consumer's constraints are its inject.
- **Revertible effects = State with an undo stack**: `∂Γ = (γ, φ)` is `StateT Γ` with an inverse accumulator (`track (f,g)` composes `g` onto `φ`; `recover` applies `φ`). Twisted composition is just "inverses stack in reverse order". The witnessed type `𝔈Γ*` carries the proof obligation `g(f(γ)) = γ` — refinement-type territory.
- **Effects as values**: "every effect carries its inverse" is one step from a free-monad-style DSL the kernel interprets (`foldFree` accumulating inverses).
- **Interception ≈ effect handlers**: metadata that reshapes how a dependency is used, provider untouched — interpretation without modifying the operation (cf. Koka/Eff/Effekt; the paper positions itself against Effekt in §7.1).
- **Recovery up to ≃ = observational equivalence / logical relations**: states equal when no observer distinguishes them — the FP move that makes independence attainable (heap layout forgotten, behavior kept).
- **Adjacent literatures** (§7.3): STM (statically scoped reversal), linear types / RAII / Rust ownership (lexical reversal — complementary), reversible computing (global reversibility vs. Cordis's per-effect one-sided inverses).

### 16.6 The Nix reading

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

### 16.7 What it means for sound-arranger

The project is pre-code: the cheapest possible moment to fix the missing composition story. Plugin surfaces that already exist in the plan, unlabelled:

- **Recorders / input backends** — cpal devices; the Notepad-12FX routing (`nusb`) as a second provider behind one `Recorder` seam.
- **Offline processes** — CDP8 programs, PaulStretch, phase-vocoder stretch: each an `OfflineProcess` plugin; file-in/file-out, non-realtime, naturally isolated — their own deferred profile (sound sculptor) and the cheapest *second* domain to prove the seams.
- **Effects** — our `AudioNode` blocks built on fundsp (fundsp supplies the per-sample DSP, our patch stays the loggable value, per the [soft-synth note](.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md)); CLAP hosting via `clack` (Phase 4) is the industry plugin ABI.
- **Codecs** — WAV/FLAC/MP3 import/export behind one interface.
- **UI** — panels / toolbars / inspectors around the canvas timeline core.

The FP-shaped architecture: immutable Session + typed edit functions (pure, testable, undoable); the cpal callback as a pure interpreter of a small value-level instruction stream (never allocates, never registers plugin callbacks — the realtime path is the privileged kernel); plugin boundaries produce *values* (graphs, configs) the interpreter reads. Composition / orchestration (reversible effects, declarative rows, patches) lives on the Tauri/TypeScript side, where Cordis itself could eventually run. Decision candidates: see the [composition-seams](.agents/notes/proposed/architecture/2026-08-15-composition-seams-plugin-architecture.md), [minimal-core](.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md), and [musical-event model](.agents/notes/proposed/architecture/2026-08-15-musical-event-model.md) notes. Further reading: [Cordis overview & reuse analysis](research/architecture/2026-08-15-cordis-overview-and-reuse-analysis.md), [dsh agent presets (the four modes)](research/architecture/2026-08-15-dsh-agent-presets.md), [kimi review](research/architecture/2026-08-15-kimi-review-minimal-core.md). **DAW prior art (2026-08-18):** what 20 years of Audacity & Ardour tell the media engine — media pool format, waveform peaks, edit-during-playback, layered clips, click-free transport, undo gestures, PDC ([synthesis](research/architecture/2026-08-18-ardour-audacity-prior-art.md), with the full [Audacity](research/architecture/2026-08-18-audacity-design-knowledge.md) and [Ardour](research/architecture/2026-08-18-ardour-design-knowledge.md) reports). **Softsynth integration routes (2026-08-19):** CLAP hosting via `clack` (Cardinal, the clap ecosystem — zero per-app work, but Cardinal is Rack-1 based and **self-contained**, bundling its open-source module set — Rack-2-only modules won't load, so host its bundled set or pay VCV Pro/build a Rack-2 host), external servers via the declared seams (SuperCollider = `OscSink` + JACK), per-app embedding/sidecar for the rest (Csound = `libcsound` opaque node or the sculptor's `OfflineProcess`) — [integration routes](research/architecture/2026-08-19-softsynth-integration-routes.md). **Media-codec prior art (2026-08-20):** FFmpeg as the reference media codebase — send/receive decoupled I/O + flush/drain protocol, the 281-patch channel-layout retrofit ("never make one up"), libavfilter format negotiation (the hardest part of an audio graph), encoder-delay/gapless correctness, 300+ fuzzers on the import surface, and the 2026 Rust binding landscape (in-process libav embedding is a high-cost path: `ffmpeg-next` maintenance-only, `rsmpeg` slow, `ez-ffmpeg` single-maintainer) — so: keep the pool pure Rust (`symphonia`), export FLAC/MP3/AAC via an ffmpeg CLI sidecar as an `OfflineProcess` plugin (the CDP8 pattern), and steal the design lessons, not the weight — [ffmpeg design knowledge](research/architecture/2026-08-20-ffmpeg-design-knowledge.md). **Full-codebase review (2026-08-24):** all three crates reviewed (architecture + implementation, realtime safety, Rust quality) — architecture judged sound; 4 verified critical bugs at untested edges (stereo output callback, render-thread free on clip retire, >4 GiB WAV truncation, wire-parser panics) plus dead `DriftCompensator` and rate-mismatch blindness; findings and fix order — [full-codebase review](research/architecture/2026-08-24-full-codebase-review.md). **Tauri-shell review (2026-08-25):** the Host API contract vs. a *live* GUI — the shipped contract is batch (assemble + render), not live (drive in time); criticals: no transport / `at_frame` is catch-up-not-scheduling, `HostSession` is `!Send` (so `tauri::State<Mutex<_>>` won't compile — host-thread actor is the fix), the serde wire surface is unowned (two-grammar drift risk), `RemoveTrack` ghost-plays audio, and an unguarded session↔device sample-rate mismatch; plus the pool/peaks/position "values" are unreachable through the seam. Ordering: name/identifier → contract round-out (host-crate only) → live runtime (still no Tauri) → scaffold → first Vue slice → offline bounce — [Tauri-shell review](research/architecture/2026-08-25-glm53-review-tauri-shell.md). **Project interchange (2026-08-18):** DAWproject adoption (native in Bitwig, Studio One, Cubase/Cubasis/VST Live, Tracktion Waveform; MIT; the project-level import/export seam) and the EP-133 → DAW export tool (the owner's EP-133 becomes a project source via DAWproject import) — [dawproject & EP-133](research/architecture/2026-08-18-dawproject-and-ep133.md). **Musical/generative prior art (2026-09-01):** Autechre — generative composition, "the studio as an instrument", and the Macero-from-a-2000s-perspective argument (the raw material is a generative run, not a band; the author writes the instrument AND curates the output; the composition is the taste-driven edit over generated material; "it's not random, it's authored rule+process+steering"; two profiles — recursive *research* builds for the studio vs *responsible* human-gated builds live). Directly relevant to the edit-as-composition thesis, the Phase-2 generators/improv plugins, the patch-bay-as-value, and the "software as instrument" / input-jam-layer threads — [Autechre — generative composition & design inspiration](research/architecture/2026-09-01-autechre-generative-composition-inspiration.md).

**Modular hardware / control surface (2026-09-01):** design-history + architecture review — the Synclavier as a historical instance of the core+plugin thesis (a general-purpose computer whose *instrument* was software; voice boards = polyphony as module-count, RAM = the creative ceiling, and first-class input+display). Hardware review for the workstation/laptop target: **don't go Eurorack for compute** — the compute/RAM rationale that made the Synclavier modular is dead in 2026 (DRAM went from ~$4000/MB in 1987 to ~$10/MB; a laptop CPU + real-time-safe `cpal` callback does FM/additive/sampling/resynthesis with headroom). The modular hardware worth having is a **grid-pad control surface** and a **CV/sync I/O bridge**, both as plugin seams over class-compliant USB, plus a **touch/tablet** front-end. Keyboard+mouse is a strong *editor* but a weak *performer*; keep the mouse for precision editing, add a performative grid/mapping surface. See [Synclavier — design inspiration & modular hardware review](research/architecture/2026-09-01-synclavier-design-inspiration-and-modular-hardware.md). The refined scope — the **input/jam layer** as an *interoperability* (not compute) concern, with workload guardrails and the deferred **Daisy Seed** DIY avenue — is captured in a proposed note: [input/jam layer — device registry & I/O](.agents/notes/proposed/architecture/2026-09-01-input-jam-layer-device-registry-io.md).

**Prior-art ledger (2026-09-30):** the passes above are now indexed in one place — [research/prior-art/ledger.md](research/prior-art/ledger.md), one row per transferable idea with a pinned revision, a licence expression read from the source header, a confidence level and a landing site. It is the entry point for prior-art questions: read it before surveying a project again. Two findings from building it are worth stating here. First, **no major FOSS audio or creative project keeps a purpose-built prior-art ledger** — 36 candidate filenames probed across Ardour, LMMS, Tracktion, Carla, VCV Rack, Cardinal, BespokeSynth, SuperCollider, Csound, Pure Data, Audacity, MuseScore, OBS, Blender, Krita, Zrythm and Inkscape returned none; the closest are Cardinal's hand-written `docs/DIFFERENCES.md` beside its generated `docs/LICENSES.md`, and the Rust RFC template's required `## Prior art` section. Second, **three licence traps were found in projects this document already cites**: Zrythm is `AGPL-3.0-or-later` *plus* additional §7 terms under the non-SPDX `LicenseRef-ZrythmLicense` (copying from it would force the combined work to AGPL — it is named as a peer in the [TUI prior art](research/architecture/2026-09-21-tui-audio-prior-art.md) pass), VCV Rack is `GPL-3.0-or-later` *plus* a non-commercial plugin exception, and Ardour/LMMS/OBS — tagged `GPL-2.0` on their forges — are `GPL-2.0-or-later` in their source headers and therefore copyable into this GPL-3.0-or-later project. Ideas stay in the ledger; the machine-readable licence half is a separate `THIRD_PARTY_NOTICES.md` listing only code actually copied ([note](.agents/notes/proposed/process/2026-09-30-prior-art-ledger.md)).

**Audio-UI prior art (2026-09-09):** the Ableton Live Arrangement-view review — Session vs Arrangement framing (the clip-grid is the *opposite* of our non-live model; the Arrangement view is the parallel), the project/library split (self-contained copies vs a referenced XDG `~/Music/Samples` folder), the "everything is a loop" clip model (edge-drag `length` + loop-span handles; `stretch` deferred but locked), and the **macro-knob map** as the answer to exposing a curated set of CDP params rather than ~800 programs. Observations: [research/architecture/2026-09-09-ableton-prior-art-ui.md](research/architecture/2026-09-09-ableton-prior-art-ui.md). Decisions: [.agents/notes/proposed/architecture/2026-09-09-ui-revision-project-library-and-clip-model.md](.agents/notes/proposed/architecture/2026-09-09-ui-revision-project-library-and-clip-model.md).

**Decision models (2026-09-20):** Laya (Convai Innovations — *not* Convai the games/avatar company; Apache-2.0, 2026-09-18) and Jev (TypeSafe, commercial) — small **non-generative encoders that answer typed questions** (`choice`/`score`/`noul`) about a short state with **calibrated probabilities**, built for routing/triage/guardrails rather than generation. Independent implementations in the same class (not one another's releases), now with an independent yardstick ([JevBench](https://github.com/fstandhartinger/jevbench): Jev 75.4, Laya 70.1, a Qwen3.5-4B entrant 74.7). Evaluated against the [co-work routing](.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md) and the [Host-API LLM seam](.agents/notes/proposed/architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md): **not a review tier** (no findings, no cross-vendor independence), **not adopted for dev-loop triage** (512-token state vs. whole diffs, invariants that live in notes not diffs, ~2 commits/day, asymmetric *silent* failure modes, and a zero-shot base below the majority-class baseline by its own card), and shelved for the product seam behind a four-condition trigger (repeated · semantic · <20 options · labelable). Triage is better served by **rules encoded as a script**; if the class is ever adopted, adopt the *interface* (typed questions + calibration, and an LLM asked for distributions) before any encoder. — [evaluation](research/architecture/2026-09-20-decision-models-laya-jev-and-the-llm-seam.md) · [note](.agents/notes/proposed/process/2026-09-20-decision-models-as-a-triage-tier.md).

### 16.8 Sources (2026-08-15)

- **`cordiverse/paper`** — full text read locally at `.research/paper/paper.txt` (paper PDF 88 pages, draft 2026-08-13; repo holds exactly 3 files, no license, 1,399★/50 forks at fetch); reading companion `.research/paper/SUMMARY.md`.
- **`cordiverse/cordis`** — repo + core-package READMEs; official primer at <https://deepseek-harness.github.io/deepseek-harness/reference/cordis-primer>; <https://floatboat.ai/blog/cordis-plugin-framework>.
- **`deepseek-ai/deepseek-harness`** — README + `docs/{architecture,cordis-primer,capability-seams}.md` read from a local checkout; `BENCHMARK.md`; external coverage: 36kr, servola.de, forklog.com.
