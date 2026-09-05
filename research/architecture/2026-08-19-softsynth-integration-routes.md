# Research: softsynth integration routes — CLAP hosting, external seams, and per-app embedding

> **Date:** 2026-08-19. **Scope:** can existing C++ softsynths (Csound, SuperCollider, Cardinal/VCV Rack, Dexed) be used in sound-arranger "easily", or is integration per-app? The answer is a taxonomy of three routes, two of which are seams already designed. **Sources:** [FOSDEM 2026 — Modular in the DAW (Cardinal)](https://fosdem.org/2026/schedule/event/JYGFRE-modular-in-the-daw/), [Cardinal](https://cardinal.kx.studio/), [DISTRHO/Cardinal](https://github.com/DISTRHO/Cardinal), [Csound Debian COPYING (LGPL-2.1)](https://browse.dgit.debian.org/csound.git/commit/COPYING), our own [§16 plugin research](../../RESEARCH.md) and the [prior-art pass](2026-08-18-ardour-audacity-prior-art.md).

## The deciding question

For any synth: **does it speak a plugin ABI, an external control protocol, or neither?** That determines the route — and two of the three routes are seams we drew in Phase 0.

## Route A — plugin ABI via CLAP (`clack`): **zero per-app work**

The open plugin ABI ([clap](https://github.com/free-audio/clap), MIT; DISTRHO are the org behind it). Our Phase-4 plan already says "CLAP hosting (`clack`)": a CLAP plugin mounts as an **opaque node** (`Box<dyn AudioNode>` — the minimal-core note's designed slot for trusted in-process foreign code), becomes a patch-bay provider with declarative ports, and its **state is a blob in the composition** (the dawproject research: "plug-in states always embedded" — our session stores the state; `mount` with a state blob, patched like any generator).

- **Cardinal** — the poster child: the entire VCV-Rack module universe inside one plugin ([DISTRHO/Cardinal](https://github.com/DISTRHO/Cardinal), GPL-3.0; FOSDEM 2026 "Modular in the DAW"). Ships as a plugin; hosts in any CLAP host.
- The clap ecosystem (Surge, Vital, hundreds more) all land the same way. **No per-app work exists because the ABI is the integration.**

## Route B — external server via our declared seams: **no per-app plumbing**

- **SuperCollider** (GPL-3.0): `scsynth` is a separate process. Control = the **`OscSink` seam** (notes → `/s_new`, `/n_set`) — Phase 2's OSC interop, already declared in the patch-bay note; audio = JACK. The per-app part is only the control *vocabulary* (mapping our notes to SC instruments), confined to the provider adapter.
- Tidal via SuperDirt is the mirror: a SuperDirt-compatible **`OscSource`** endpoint (already declared).

## Route C — per-app embedding/sidecar: **adapter-only work, never the core**

- **Csound** (library LGPL-2.1, compatible): realtime = embed `libcsound` (a C API) behind an opaque node — per-app FFI, confined to the adapter (the opaque tier exists precisely for this: "its *existence and params* are values in the graph, its *code* is trusted in-process"); offline = the sound-sculptor profile's **`OfflineProcess` sidecar** (file-in/file-out — the same slot as CDP8/PaulStretch).
- **Dexed** (GPL-3.0, VST/AU): enters via CLAP/VST hosting or a fork port — same adapter pattern.
- Anything without an ABI export follows Route C: per-app, but the adapter is the only new code; the graph value, the log, the mixer, and the monitoring are untouched.

## Licensing: no blockers

clack is MIT; Cardinal/Dexed/Surge are GPL-3.0 (fine inside our GPL-3.0-or-later app); scsynth is GPL (separate process); Csound is LGPL-2.1 (dynamic link). Our GPL-3.0-or-later absorbs all of them — §12 already anticipated this class.

## The end goal

Every sound source — our generators (euclidean/scale/tone), fundsp composites, CLAP plugins (Cardinal, Dexed, Surge), SC/Csound via seams — is **a provider in the patch bay**. The original dropdown ("available inputs from MIDI to our own tone generators") extends to "and any hosted plugin". The mixer, channel strip, monitoring, and composition stay identical: a plugin instance is `mount` with a state blob, patched like the euclidean. The core never changes; only the provider registry grows. This is the payoff of the discipline: the seams drawn in Phase 0 (opaque tier, OSC/MIDI, `OfflineProcess`, CLAP-in-Phase-4) are precisely the integration points for every one of these synths. "Easily" = the ABI route; "per-app" = adapter-only work that never touches the core.

## Phase-plan touch (folds the routes in)

- **Phase 2** — SC becomes an explicit external-synth provider: the `OscSink` seam (notes → scsynth) + JACK (Route B).
- **Phase 4** — CLAP hosting explicitly covers external synth providers: Cardinal and the clap ecosystem via `clack` (Route A); plugin state stored in the composition.
- **Sound sculptor** — Csound offline via the `OfflineProcess` sidecar (Route C offline); realtime `libcsound` embedding is a later Route-C adapter behind an opaque node.
