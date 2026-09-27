# Agent Note: exporting our instruments as plugins — `nice-plug` as the framework

Status: proposed

## Problem

The owner asks whether [nice-plug](https://codeberg.org/RustAudio/nice-plug) can be used "in case we
want to make a CLAP plugin". Two recorded positions conflict with that question:

- The [no-sidecars note](2026-09-21-external-programs-not-sidecars.md) resolves the integration
  story as **process/device boundaries**, and its first item rejects plugin-format builds outright
  ("No VST3/CLAP builds, no CDP sidecar binary, no plugin host").
- The [native-soft-synth note](2026-08-30-native-soft-synth-building-blocks.md) rejected
  **`nih-plug`** as the export path: "in maintenance mode … its VST3 bindings (`vst3-sys`) are
  GPLv3 … use `clack`".

Both need re-reading against today's facts. `nih-plug` is no longer the framework in play: its
community fork on Codeberg, **nice-plug**, is now a separate, actively released project (0.4.2 on
2026-09-14, 19 releases since June, ISC-licensed, MSRV 1.88 — stable Rust). And this repository is
**GPL-3.0-or-later**, which already absorbs GPL-3.0 plugins (Cardinal, Dexed, Surge) per RESEARCH
§12 — so a GPLv3 VST3 binding is not the licensing problem the earlier note treated it as.

So the question is not "can we" but **"what would we export, and does that re-open the scope the
no-sidecars note closed?"**

## Proposal

1. **Export applies to *instruments and processors*, never to the arranger.** The arranger is a
   timeline host — CLAP has no notion of an arrangement, a clip or a session log. What maps
   cleanly onto a plugin is one voice or one effect: parameters, MIDI/note events, transport,
   state. So the export candidates are the **native soft-synth voices** (the `fundsp`-based
   building blocks), the **generators** (`euclidean`, `scale`, `tone`), and later an
   **effect/processing chain** — each a self-contained node our engine already renders in fixed
   blocks.
2. **`nice-plug` is the framework if and when we do it.** Verified facts (crates.io / README,
   2026-09-22):
   - `nice-plug 0.4.2`, ISC, edition 2024, **rust-version 1.88** — it builds on our stable
     toolchain. (Its README mentions optional nightly features, but only as a rust-analyzer
     convenience: "you can enable the nightly compiler for your local repository".)
   - **CLAP is the base** (`clap-sys`, Rust bindings, no C++ build). `vst3` is an opt-in feature
     that pulls the Steinberg SDK; `default-features = false` gives a CLAP-only build with no
     C++ toolchain.
   - `standalone` produces a **standalone binary with full JACK** (plus cpal, MIDI via `midir`,
     transport) — which is *the same artifact class* the no-sidecars note wants for partner
     programs: one instrument codebase can be a CLAP plugin, a VST3 plugin, and a JACK-native
     standalone app the arranger can sync and record.
   - Declarative parameters (`#[id = "…"]`, `#[derive(Params)]`, smoothing, `#[persist]` Serde
     state, migration, nesting), buffer adapters, optional SIMD, sample-accurate automation, and
     **`assert_process_allocs`** — a no-allocation assertion on the process callback, which is
     our own render-path invariant turned into a test.
   - GUIs: a baseview-based editor API with a first-party **`nice-plug-iced`** adapter (0.4.1,
     ISC), and `iced_audio`'s own `nice-plug` feature exists precisely to bind its widgets to
     nice-plug parameters — so the [shell decision](../../implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md)
     and the export path share one GUI stack.
3. **The bridge is a wrapper, not a port.** A `Plugin` implementation owns an `engine` instance
   (graph + clock), maps CLAP's parameters/note events/transport onto the same vocabulary
   `HostCommand` speaks, and returns the master output. The engine is `std`-only, allocates
   nothing on the render path, and already renders a fixed frame block — the shape a plugin
   callback wants. **What does not map is the point:** the session log (a plugin host owns
   history, not us), bouncing/offline render, and the multi-clip arrangement. Those stay in the
   application; that is why the arranger is not an export target.
4. **This narrows the no-sidecars decision rather than reversing it.** *Hosting* third-party
   plugins in-process (CLAP/VST3 hosting, a CDP sidecar binary, a plugin-host product) stays
   rejected — that was the scope complaint, and it is unchanged. What reopens is only: *our own
   instrument, shipped in someone else's DAW*, as a deferred per-instrument option.
5. **First move when we pick it up — a probe, not a port.** An isolated
   `spikes/clap-shell` workspace (the same shape as the iced/TUI spikes, kept out of the core
   workspace) that builds the existing `tone` generator, and then a `fundsp` voice, as a
   CLAP-only plugin (`default-features = false`) and loads it in a real host. Nothing in
   `crates/` changes until that probe says what the boundary actually costs.

## Alternatives considered

- **Keep the blanket "no plugin builds" rule.** Rejected as stated: it was written to stop a
  *product* scope (a plugin-format build chain and a host as roadmap items), not to forbid an
  export that reuses an existing engine node. Written as a rule, it would need an exception the
  moment the owner wants their own synth in a DAW; better to name the boundary now.
- **Export the arranger itself as a CLAP plugin.** Rejected: CLAP is per-track and
  event/parameter-oriented; it has no timeline, no clips, no log. A DAW hosting the arranger
  would either duplicate its host role or use one clip at a time — a worse version of both.
- **`nih-plug` (upstream).** Rejected: in maintenance mode by its own README, the framework is
  not on crates.io (only sub-crates), and the community has moved to nice-plug.
- **`clack-plugin` (build plugins) / `clack-host` (host them).** Still the right lower-level,
  permissive CLAP crate, and still the plan for *hosting* if we ever do that. Rejected as the
  *authoring* framework: params, state, migration, GUI and a standalone target are exactly the
  boilerplate we would otherwise write, and nice-plug has them. `clack` remains the fallback if
  nice-plug's experimental status bites.
- **`truce`, `vst3-host`, `mkapk`, or hand-writing the ABI.** Rejected: truce is custom-licensed,
  `vst3-host` is young and host-side only, `mkapk` is a one-processor packaging helper, and a
  hand-written ABI is unjustified while two permissive frameworks exist.
- **VST3 as the required target.** Rejected as the default: VST3 export pulls the Steinberg SDK
  (a C++ build) and, if shipped as GPLv3 bindings, makes the plugin GPL — fine for us, but it
  constrains future relicensing. CLAP first, VST3 behind a feature flag, standalone for free.
- **Ship plugins from the arranger repository's workspace.** Rejected for now: a plugin artifact
  has its own versioning, bundling (`cargo-nice-plug`) and release cadence. It belongs in its own
  crate/workspace that depends on `engine`, like the spikes — not in the core's build.

## Acceptance criteria

- A CLAP-only plugin built from `engine` nodes loads in a real host, exposes its parameters, holds
  its state across a save/load cycle, and renders audio **without allocating on the process
  callback** (`assert_process_allocs`).
- `cargo build --workspace` for the core is unaffected; the plugin lives in its own workspace and
  the core crates gain no new dependency.
- The standalone build of the same plugin starts on JACK here and the arranger can record it as a
  take — the export path and the integration path produce the *same* artifact class.
- The GUI (if any) reuses the iced shell's widgets through `nice-plug-iced` + `iced_audio` rather
  than a second widget stack.

## Risks

- **Experimental framework.** nice-plug says so itself ("recently undergone some large changes, so
  expect some bugs"), and it is a young fork (first release 2026-06-04). Mitigation: the probe is
  small and isolated; `clack-plugin` is a real fallback.
- **Two parameter models.** CLAP's automation model is host-driven per-block events; our engine's
  parameters are log entries (`SetParam`) folded and replayed. A wrapper must decide which is
  authoritative in which direction, and automation recorded in a DAW will not appear in our log.
  This is the real design work, not the build.
- **Two iced hosts.** A plugin editor runs iced under `baseview` (embedded in the host's window),
  while our shell runs iced on winit — two iced entry points, one widget vocabulary. Acceptable,
  but it means the editor is not simply "the shell's mixer panel" reused verbatim.
- **GPL reach.** Using the `vst3` feature makes the plugin GPL-bound; if a permissively licensed
  plugin ever matters, VST3 has to be dropped or the bindings replaced. Record it before shipping.
- **Upstream contribution policy.** The RustAudio community publishes an AI-usage policy that
  applies to contributions; using the crate is unaffected, but if we ever want to upstream a fix,
  read that policy first (and this repository's own attribution rules still apply to our code).
- **Scope gravity.** "We can build plugins now" is exactly the kind of permission that eats the
  arranger's remaining work. Mitigation: the export path stays *deferred* until an instrument the
  owner actually wants exists as a native voice — this note authorises a probe, not a program.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-22.
