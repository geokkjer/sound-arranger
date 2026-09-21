# Agent Note: Composition seams — plugin architecture for the engine and host

Status: proposed

## Problem

sound-arranger has no composition story. Every planned extension — CDP8 offline processes, PaulStretch, CLAP hosting, import/export codecs, recorders (including the Notepad-12FX), UI panels, scripting — is additive ("add a sidecar", "host CLAP in Phase 4"), but nothing defines *how* a capability plugs in, what it may touch, or how it is removed cleanly. RESEARCH.md §16 documents the "everything as a plugin" paradigm (cordiverse/paper, cordiverse/cordis, deepseek-harness); this note owns the decision: adopt that discipline for sound-arranger, in the shape the paradigm itself suggests — seams as typed interfaces, declarative composition, reversible effects for non-realtime contributions, and a pure interpreter core for the realtime path.

## Proposal

- **The engine is a pure core plus an interpreter.** The Session model (Sources/Tracks/Clips) is immutable data; edits are typed pure functions `EditOp -> Session -> Session`. The cpal callback is an interpreter of a small value-level instruction stream fed through the existing lock-free ring buffers (`rtrb`/`basedrop`); it never allocates, blocks, or runs plugin-registered callbacks. The realtime path is explicitly the privileged kernel.
- **Engine service seams are plain Rust traits**, defined now: `Recorder` (cpal devices today; Notepad-12FX routing via `nusb` as a second provider), `OfflineProcess` (CDP8 sidecar, PaulStretch, phase-vocoder stretch), `EffectHost` (fundsp graph as data; CLAP via `clack` later), `Codec` (WAV/FLAC/MP3), later `TransportSync`, `ScriptHost`. Consumers depend on trait definitions, never concrete providers — the capability-seam discipline (definition / provider / consumer).
- **The composition layer lives on the Tauri (TypeScript) side.** A minimal declarative loader over rows `{id, name, config, disabled}` with layered patches (later layers win; whole-row replacement, not deep merge) and registrations as reversible effects — every registration returns a disposer; teardown is derived LIFO, not hand-written. Start with the discipline; adopt Cordis itself only when the frontend genuinely needs a plugin runtime (its API is still unstable).
- **First plugin domain: the clip-arranger profile's plugins** (recorder, clip editor, soft mixer) — the profile stresses the substrate end-to-end; direction locked in the [umbrella-first note](2026-08-15-umbrella-first-product-direction.md). The OfflineProcess tier is no longer arranger functionality: it splits into its own deferred profile ("sound sculptor") and becomes the cheapest *second* domain to prove the seams when that work starts. In the same change, write down the IPC command list: it is the engine/host contract and the first surface the seams must hold.
- **Kernel honesty:** "no privileged core" holds everywhere except the audio callback; the callback is privileged by physical necessity, and stating that boundary is part of the design.

The core this runs on — clock, graph interpreter, session log, context plumbing — is defined in the [minimal-core note](2026-08-15-minimal-core-clock-graph-session-log.md).

**Superseded in part (2026-09-21):** the *product boundary* is now external programs and devices —
no VST3/CLAP builds, no CDP sidecar binary, no plugin host ([note](2026-09-21-external-programs-not-sidecars.md)).
The internal seam discipline this note defines (traits, declarative composition, reversible
effects, a pure interpreter core) is unchanged and is what makes the engine safe to integrate
against.

## Alternatives considered

- **Adopt Cordis wholesale into the Rust engine** — Cordis is a TypeScript/npm runtime; it belongs on the host side. The paper's own language guidance (§6.4) is that typeclasses/traits are how a host language extends the context type, i.e. Rust traits *are* the recommended mechanism here.
- **Dynamic loading of Rust plugins (`libloading`)** — an ABI/safety minefield for no current need; when third-party DSP arrives, CLAP via `clack` is the industry ABI (Phase 4) and offline processors are sidecars.
- **No plugin architecture (keep the additive plan)** — acceptable for phase 1, but the project is pre-code: seams drawn now cost nothing, retrofitted later they cost a rewrite.
- **Full Cordis-style runtime with HMR from day one** — YAGNI; hot reload is machinery, not discipline; add it on demonstrated pain (e.g. iterating on CDP8 wrappers).

## Acceptance criteria

- Phase 0 spike shows the value boundary: the engine crate compiles and its tests run with plain `cargo test`; a headless smoke binary records and bounces without the frontend. (The OfflineProcess sidecar job moves to the sound-sculptor profile — see the [umbrella-first note](2026-08-15-umbrella-first-product-direction.md).)
- The IPC command list is written down (documented or generated) and reviewed against the seams.
- A recorder backend other than plain cpal devices (the Notepad-12FX routing) is added without touching the engine core — proving the `Recorder` seam.
- Layered config precedence (rows, patches, whole-row replacement) is documented before the first user-facing setting exists.

## Risks

- **Over-engineering:** the discipline can decay into ceremony (more config, more naming, more cognitive overhead — the paper's own §6.5 warning). Mitigate: start with one plugin domain, keep the realtime kernel honest, and measure against the acceptance criteria before widening.
- **Config semantics surprise:** whole-row replacement differs from NixOS-style merging (`mkDefault`/`mkForce`); document precedence early so later layers behave predictably.
- **Cordis API instability** if adopted: v4 is under active development; pin the vendored version and keep the seam contract (the IPC list) independent of it.
