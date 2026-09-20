# Research: Faust / libfaust and HISE — adoption, inspiration, and the Rust story

> **Date:** 2026-09-20. **Scope:** evaluate [Faust](https://faust.grame.fr) (the language + compiler), **libfaust** (the embeddable compiler) and [HISE](https://github.com/christophhart/HISE) for *inclusion* in sound-arranger or as *inspiration* — with the Rust story for each as the spine. **Method:** desk research plus a hands-on spike against the locally installed **Faust 2.88.0** (`/usr/bin/faust`, LLVM 22.1.8): real `.dsp` files were compiled to Rust, compiled and executed under cargo with a counting allocator, benchmarked against the same DSPs generated as C++, and checked for cross-run determinism. Numbers below are from that spike, not estimates. **Status:** research, not a decision — the proposal it supports is the [Faust-as-DSP-source note](../../.agents/notes/proposed/architecture/2026-09-20-faust-as-optional-dsp-source.md).

## 0. Verdict on one screen

| Question | Answer |
|---|---|
| Does Faust have a real Rust story? | **Yes, and it is official** — an in-tree `-lang rust` backend (started 2017, present in 2.88.0), GRAME-authored Rust architecture files, and three `faust2*rust` tools including `faust2cpalrust`. The weak part is the *crate* ecosystem, which is one stalled project whose published types crate no longer compiles against current output. |
| Adopt Faust? | **Yes — as an optional, build-time DSP source**: `.dsp` → generated Rust → our own `AudioNode` adapter. Verified to need **no crate, no FFI, no allocation** in the render path. |
| Adopt libfaust? | Only for **user-authored / live-compiled** DSP (LLVM JIT or WASM) — a heavier, different route (13.6 MB `libfaust.so.2` + LLVM here). Not the way to build our own instruments. |
| Adopt HISE? | **Inspiration only.** C++/JUCE instrument-builder *environment*, GPL-3.0, **zero Rust anywhere**, not a library you embed — and **no CLAP export**, so even the "use one HISE instrument" path would need the VST3 hosting we deferred. |
| Licensing blockers? | **None.** Compiler is LGPL-2.1-or-later; the FAQ states the compiler's licence *does not* apply to generated code; the Rust architecture files are GPL-3.0-or-later **with a linking exception** (and we write our own ~20-line one anyway); build-time use is a tool invocation, so nothing is linked at all. Fine under GPL-3.0-or-later — and even an MIT app could ship AOT-generated Faust. |

## 1. What Faust is

A functional, block-diagram **DSP language** (`.dsp`) with a single-binary compiler and many backends. A program is a composition of signal expressions; `process = ...` is the entry point. The composition operators (`:`, `,`, `<:`, `:>`, `~`) make it a point-free dataflow algebra, and the compiler lowers it to an imperative form (FIR) and then to C, C++, LLVM IR, WebAssembly, Cmajor, Rust, and others.

Three properties are the reason it maps onto our architecture so cleanly:

- **The source is data.** A `.dsp` is a small text value; the code it produces is generated. That is the same split we already insist on ("graph-as-value, code trusted in-process" — [patch-bay note](../../.agents/notes/implemented/architecture/2026-08-17-patch-bay-typed-signal-streams.md), [native-soft-synth note](../../.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md)).
- **Bounded, known resources.** All state is explicit and fixed at compile time; loops are bounded by construction, so there is no unbounded work per sample and memory is known at init (the compiler's own FAQ: "the generated code computes the sample in a *finite number* of operations", "the DSP memory footprint is perfectly known at compile time"). The documented deployment model is exactly ours: allocate and `init` with a sample rate at load time, then call `compute` on buffers.
- **The control surface is declared.** Widgets (`hslider`, `button`, …) with hierarchical pathnames (`vgroup("Osc", hslider("freq", …))` → `/Osc/freq`) *are* the parameter model. `faust -json` emits the whole tree with `address`, `init`, `min`, `max`, `step` and per-widget metadata (`[unit:Hz]`, `[scale:log]`, `[style:knob]`, `[tooltip:…]`) — i.e. a node's parameter ports can be *generated* from the DSP rather than hand-written.

Recent activity: **2.88.0 released 2026-09-09**; `master-dev` is on 2.89.0-dev; the GitHub repo is at ~3.2k stars with pushes on the day of writing. Maintained by GRAME (Lyon) with INRIA. Release cadence is roughly monthly-ish, and the Rust backend is maintained alongside the rest.

## 2. What libfaust is

`libfaust` is the compiler as a library: `createDSPFactoryFromString/File` → an in-memory factory → `createDSPInstance()` → the standard `dsp` interface (`init`, `compute`, `buildUserInterface`, `clone`). Three dynamic backends matter ([embedding docs](https://faustdoc.grame.fr/manual/embedding/)):

- **LLVM JIT** — full compile in-process, fastest generated code, needs LLVM (`libfaust.so.2` is 13.6 MB here and links `libLLVM-22`).
- **Interpreter** — no LLVM, slower; for environments where JIT is unavailable.
- **WebAssembly** — compile to wasm and run in a wasm runtime (also the basis of `faustwasm` in the browser).

The compiler is also the *only* way to get the `-json` description and the box/signal APIs programmatically. C headers ship at `/usr/include/faust/dsp/libfaust.h` and `libfaust-c.h`.

**When this is the right tool:** a feature where the *user* writes Faust in the running app (live-coding, an in-app DSP editor, an LLM-authored patch — see §7). It is the wrong tool for our own instruments: it makes LLVM a runtime dependency of the product and moves DSP compilation into the process we care most about keeping boring.

## 3. The Rust story — measured, not assumed

### 3.1 The backend is official and current

`faust --help` lists `rust` among the output languages, alongside Rust-only flags:

```
-lang <lang>   … 'lang' should be asc, c, cpp (default), cmajor, codebox, csharp, dlang,
                   fir, interp, java, jsfx, julia, linen, llvm, nnx, ocpp, rust, sdf3, vhdl or wast/wasm.
-noreprc       (Rust only) Don't force dsp struct layout to follow C ABI.
-rnt           (Rust only) Don't generate FaustDsp trait implementation.
-rnlm          (Rust only) Don't generate FFI calls to libm.
```

The generator lives in-tree at [`compiler/generator/rust/`](https://github.com/grame-cncm/faust/tree/master-dev/compiler/generator/rust) (`rust_code_container.cpp`, `rust_instructions.hh`, `dag_instructions_compiler_rust.*`). GRAME also ships Rust architecture files (`rust/minimal.rs`, `rust/cpal.rs`, `rust/jack-float.rs`, `rust/jack-double.rs`, `rust/portaudio-*.rs`), the documented tools `faust2jackrust` and `faust2portaudiorust` ([faust2 tools docs](https://faustdoc.grame.fr/manual/tools/)), and — in this distribution — `faust2cpalrust`, which is notable because **cpal is our audio I/O crate**: GRAME's own path for "Faust as a Rust binary" already targets it. The backend is not a side project: it was started in 2017, its trait API was reworked in 2020 (Fabian Keller + Letz), it has been maintained since (deterministic codegen 2024-12; `-rnlm` 2025-02; `-it` for Rust 2026-03; `-ftz 2` fix in 2.88.0), and contributors outside GRAME now send Rust-backend patches.

The one honest caveat, straight from GRAME's Stéphane Letz (2021): the generated code "is far from optimal… it has to be improved to reach the C++ level". That statement is now **partly stale** — see §3.4 for 2026 measurements.

Hard limits in the Rust backend (from the generator source): `-quad`, OpenMP and the work-stealing scheduler all throw "not supported for Rust", and the **`soundfile` primitive throws** ("'soundfile' primitive not yet supported for Rust"). Vectorisation (`-vec`) *is* supported; `soundfile` matters only if a patch plays back files, which our offline tier does not need.

### 3.2 What the generated code actually is

`faust -lang rust -cn ProbeDsp probe.dsp -o probe.rs` (Faust 2.88.0) emits a plain Rust struct — no C ABI, no trait objects, no heap:

- `#[repr(C)] pub struct ProbeDsp { … }` with all state in fixed arrays (`fRec0: [F32;3]`, `fVec0: [F32;2]`, …); disable the C layout with `-noreprc`.
- Compile-time arity constants: `pub const FAUST_INPUTS: usize`, `FAUST_OUTPUTS`, `FAUST_ACTIVES`, `FAUST_PASSIVES`. (This directly answers the 2021 request on the mailing list that the counts be constants — they now are.)
- Inherent methods: `new()`, `metadata()`, `class_init`, `instance_*`, `init(sample_rate)`, `build_user_interface(&self, &mut dyn UI<FaustFloat>)`, `get_param(ParamIndex) -> Option<T>`, `set_param(ParamIndex, T)`, and
  `pub fn compute(&mut self, count: usize, inputs: &[impl AsRef<[T]>], outputs: &mut [impl AsMut<[T]>])`.
- A `FaustDsp` trait impl (`compute(count: i32, &[&[T]], &mut [&mut [T]])`) that `-rnt` removes.
- Metadata accessors: `declare latency "N";` in the `.dsp` shows up in `metadata()` and in the JSON as `{"latency": "N"}` — **latency is programme-declared, not inferred**, so PDC for a Faust node is the DSP author's responsibility.

Two RT-safety details the generator's own README/source flags, both confirmed here:

- **Static tables become `RwLock`s locked inside `compute`.** A `rdtable`-using DSP emits `static ftbl0…: std::sync::RwLock<[F32; 1024]>` and, at the top of `compute`, `// Obtaining locks on 1 static var(s)` followed by `.read().unwrap()`. It is an uncontended read lock (no syscall, no allocation), but it *is* a lock and a panic path in the render path, and the table is process-global — shared by every instance of that DSP class, which is why `class_init` exists. **`-it` (inline tables, implemented for Rust on 2026-03-18) removes the statics entirely** ("Obtaining locks on 0 static var(s)") and must be in our recipe. This directly touches our render-path contract, which no other node violates.
- **The DSP struct can be large** — delay buffers live inline (`[F32; N]`), so a multi-second delay is hundreds of KB *by value*. GRAME's Rust architecture README warns that large structs can overflow the stack and that `Box::new(Dsp::new())` only avoids it in release builds (they point at `default-boxed`). Our adapter constructs the DSP on the heap.

Also worth setting deliberately: **denormals are not flushed by default** (`-ftz 0`); `-ftz 2` is the fast masked variant (`-ftz 2` only became usable in Rust with the 2026-07 bitcast fix shipped in 2.88.0).

The generated file is **byte-identical across runs** (`faust -lang rust … -o a.rs` twice → same md5; codegen determinism on the Rust path was an explicit fix in 2024-12), which makes it safe either to check the generated Rust in or to regenerate it in `build.rs` with a pinned compiler version.

### 3.3 The crate ecosystem is the weak link

crates.io is thin, and the one published integration is stale relative to the compiler:

| Crate | Version / date | License | State |
|---|---|---|---|
| `faust-build` | 0.2.1, 2024-11-20 | MIT OR Apache-2.0 | `build.rs` helper that shells out to `faust -lang rust` with a template; ~2.1k downloads |
| `faust-types` | 0.2.1, 2024-11-20 | MIT OR Apache-2.0 | The traits the generated code used to need; ~1.5k downloads |
| `faust-state`, `faust-macro` | — | — | Promised in the repo README, **never published** |
| `faust` | 1.0.2, 2022 | MIT | Unrelated crate — a "Fast Async Url STatus checker" |

Both come from [Frando/rust-faust](https://github.com/Frando/rust-faust) (85★, last push 2025-05, Apache-2.0), as do the two unpublished crates; only the unrelated `faust` crate is someone else's. **Measured:** generating with Faust 2.88.0 and compiling against `faust-types` 0.2.1 fails with 18 errors — `cannot find type FaustFloat in this scope` (the alias is new; GRAME's own architecture files define it themselves). One added line, `pub type FaustFloat = F32;`, fixes the whole build.

The lesson is not "Faust+Rust is broken"; it is **don't route through `faust-types`/`faust-build` at all**. The supported shape is an architecture file that defines the handful of types the generator references, and GRAME's `rust/minimal.rs` does exactly that. Verified end-to-end here:

```
# ~20-line architecture: F32/F64/FaustFloat aliases, ParamIndex, Meta, UI<T> … then
#   <<includeIntrinsic>> / <<includeclass>>
faust -a arch_selfcontained.rs -lang rust -rnt -rnlm -cn ProbeDsp probe.dsp -o out.rs
```

The result has **no `use`/`extern crate` of any kind**, no `FaustDsp` trait, and no libm FFI (`-rnlm`), so it drops into a crate with zero new dependencies. Dropping `-rnlm` leaves only `unsafe extern "C" { remainderf / rintf }` and needs libm at link time. Wrong output arity does not corrupt memory: the generated `compute` is guarded (`let [outputs0, ..] = outputs.as_mut() else { panic!("wrong number of output buffers") }`), which the spike tripped deliberately.

### 3.4 Does it run clean and fast?

Harness: generated DSP behind a counting global allocator, 512-frame blocks, 44.1 kHz, release builds; C++ generated from the identical `.dsp` and driven through the same block loop. Each cell is 100 s of audio (8 600 blocks), best of three runs on one machine (Ryzen 7 7735HS) — read the ratios, not the milliseconds.

| DSP | Rust (default target) | Rust (`target-cpu=native`) | C++ `-O3` | C++ `-O3 -march=native` |
|---|---|---|---|---|
| `os.sawtooth(freq) * en.adsr(…) : fi.resonlp(cutoff,q,1)` — mono, 100 s | 12.8 ms | 17.4 ms | 13.0 ms | 14.8 ms |
| `re.zita_rev1_stereo(…)` — stereo, delay/table heavy, 100 s | 167.6 ms | 124.7 ms | 131.9 ms | 90.7 ms |

What survives the noise:

- **Render-path allocations: zero** (counting allocator, 8 600 blocks, both DSPs) — the engine's zero-alloc invariant is satisfied by construction.
- **Small DSP: rough parity** (12.8 vs 13.0 ms at default flags). On this workload the 2021 "far from C++" caveat no longer holds. (`native` being *slower* for both compilers here is a real effect on a ~13 ms measurement — vectorisation helping nothing and costing setup — not a transcription slip.)
- **Table/delay-heavy DSP: ≈1.3× slower** (167.6 vs 131.9 default; 124.7 vs 90.7 native). The gap narrowed with `target-cpu=native` on the Rust side but did not close. In absolute terms a full Zita reverb is 0.12–0.17 % of one core, so the ratio matters only for very heavy patches or very high polyphony.
- **Cross-backend numerics differ slightly** — same DSP, same input, last samples `0.104009084` (Rust) vs `0.104009174` (C++), `0.340832978` vs `0.340832800` on the reverb. Within one backend, reruns are bit-stable. Consequence for us: **replay determinism holds, but a Faust→Rust port of an existing C++ Faust DSP is not sample-identical**, and changing compiler versions can shift output. If a render must be reproducible forever, pin the compiler version with the session (the same discipline as pinning dependencies).

### 3.5 Bridging into our `AudioNode`

The adapter is small and mechanical, but six details are real work:

1. **Interleaving.** Our nodes receive/write interleaved audio (`out_audio.len() == channels * frames`); Faust wants non-interleaved per-channel slices (`&mut [&mut [T]]`). The adapter deinterleaves into preallocated scratch, calls `compute`, re-interleaves. Bounded, allocation-free, one extra copy per block — a per-node cost worth noting but not a design problem.
2. **Parameters.** `set_param(name, value)` is string-keyed in the engine; Faust addresses params by `ParamIndex` and the Rust backend has **no path-based setter**. Walk `build_user_interface` (or parse `faust -json`) once at mount to build `path → ParamIndex` (measured: `[("cutoff",0),("freq",1),("gain",2),("gate",3),("q",4)]`). The path (`/probe/cutoff`) becomes our logged parameter identity, so the log stays the source of truth and the DSP is a pure replay of it.
3. **Sample-accurate events.** Our notes/triggers carry sample offsets *inside* the block; Faust's `compute` is per-block, and its timestamped/`timed_dsp` API is a C++-architecture thing the Rust backend does not emit. The adapter must therefore **segment the block at event boundaries** (render `n` frames, apply the param/gate change, render the rest) — the same discipline the fundsp voice spike already uses.
4. **Construction and class state.** `class_init` must run once per DSP *class* (it initialises the shared static tables), while `instance_init` runs per node instance — a `OnceLock`-guarded per-class init in the adapter. The DSP struct itself is heap-constructed (see §3.2 on inline buffers).
5. **PDC.** Read declared `latency` metadata at mount and return it from `latency()`. Faust will not infer it; a patch that delays without declaring it reports 0 and breaks alignment. Our mount path should refuse (or warn on) a Faust node whose declared latency disagrees with a value we know.
6. **Drain/tail.** A Faust node has no tail concept (`has_tail()` is ours). A reverb/delay patch must be configured tailful; a stateless one must not lie. Simplest is a per-node flag in the mount value, with the same monotone-`has_tail` contract the engine already requires.

**Not solved by Faust, and correctly left to us:** note→voice allocation, voice stealing, release tails, per-voice parameter offsets, and the note/trigger event streams. Faust's polyphony (`-nvoices`, `mydsp_poly`) lives in the *C++ architecture* layer; the Rust backend does not emit it. That is exactly the boundary the [native-soft-synth note](../../.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md) drew: **Faust is another optional block source behind our voice layer — a peer of fundsp, not a replacement for the patch bay.**

### 3.6 What a Faust integration would and would not buy

**Buys:** a large, battle-tested DSP library corpus (`stdfaust.lib`: oscillators, filters, envelopes, delays, reverbs, physical models, effects) with a declarative parameter surface; a text source that is diffable, reviewable and LLM-authorable; bounded resources and a proven block API; and a second, independent DSP implementation path alongside hand-written fundsp blocks.

**Does not buy:** a graph model (Faust is one node per `.dsp`), voice management, event/trigger handling, or a live-editing experience (that is the libfaust route).

## 4. HISE — and its Rust story (there isn't one)

HISE is a mature, single-author C++/JUCE **instrument-building environment** (Christoph Hart, "Hart Instruments Sampler Engine", 2016→, ~1.4k★): sample-map based sampler/synth/effect builder, a JavaScript-derived scripting layer (HiseScript) for UI and logic, and an export pipeline for VST/VST3, AU, AAX and standalone (Win/macOS/iOS; Linux experimental). It is not a toy — shipped commercial instruments include Wave Alchemy's Triaz, Lunacy Audio's CUBE, Auddict's PercX, MNTRA's ATMA — and it is very actively developed: repo created 2016-08, v1.0.0 in 2017, tags `v4.9.0` (2026-05-05) through `v4.9.3` (2026-07-15), develop self-versioned 4.9.4, commits through 2026-08; the last *GitHub release* is older (4.1.0, 2024-10-29). At the top level it is `hi_core` (core/DSP/components/modules/sampler), `hi_dsp_library` (the scriptnode node library), `hi_scripting`, `hi_snex` (the JIT), `hi_faust*`, `hi_lac`/`hi_streaming` (its own lossless codec and disk streaming), `hi_backend` (the IDE) and `hi_frontend` (the exported plugin) — on a **forked JUCE** (`christophhart/JUCE_customized`).

**The Rust story is exactly nothing.** A clone-and-grep pass over the HISE source and documentation found zero `.rs` files, zero `Cargo.toml`, and no Rust (the language) mention anywhere — not in the code, the docs, the forum or the issue tracker; crates.io has no `hise`/`hise-sys`/`scriptnode` crate. HISE is C++/JUCE end to end. There are no bindings to write against, because HISE is not a library: it is a program you build plugins *with*. (Amusingly, the only Faust→Rust connection is a forum mention that Faust *can* emit Rust — HISE itself never does; it uses libfaust's LLVM JIT and exports static C++.)

The architecture is nonetheless the interesting part, because it is the closest thing in the audio world to "everything is a plugin, with a tier for user code":

- **scriptnode** — a visual DSP graph that is deliberately *not* a free pin graph: a tree of containers (`chain` / `split` / `multi` / `frame2_block` / `oversample`) wrapping ~197 node types. At runtime it is **interpreted** (each node's `process(ProcessDataDyn&)` dispatched through a switch — virtual calls, which the docs say must be avoided "at all costs" in the final product). The escape hatch is the **C++ generator**: any container can be exported to a class deriving from `scriptnode::hardcoded<T>`, expressed as variadic templates (`container::chain<core::gain, core::peak>`, borrowed from `juce::dsp`), with the original graph embedded as Base64 so it can be *unfrozen* and edited again. During development the same generated C++ can be built as a DLL and hot-loaded. Prototype interpreted → ship compiled, and keep the round-trip.
- **SNEX** — a typed C subset for per-sample code, JIT-compiled: no classes, no strings, block iteration as the only loop, bounds-checked array index types, fixed callback names (`prepare`/`process`/`handleHiseEvent`/…). Docs say asmjit; current source defaults to a vendored **MIR** JIT (asmjit is the fallback), and iOS cannot JIT at all (no executable memory). Same source works in the JIT and in the AOT export — that invariant is the clever part.
- **Faust as its user-DSP source** — opt-in at build time (`HISE_INCLUDE_FAUST`/`HISE_INCLUDE_FAUST_JIT`, both off by default), in-process **libfaust with the LLVM JIT** (interpreter fallback, "significantly slower"), LLVM ≥ 15, compilation/processing guarded by a read-write lock — and exported plugins get the Faust DSP as **static C++**. Forum threads show the usual JIT friction (version-mismatch segfaults). So HISE independently arrived at "graph for structure, script for logic, Faust for DSP", and treats all three as *user-facing authoring*, not as the host's own instrument model.
- **Also**: a fixed-size event type (`HiseEvent`, timestamp baked in, an EventID linking a note-on to its note-off and voices), its own lossless codec/monolith packing for sample libraries, `Engine.renderAudio` offline rendering, three native extension paths (HISE modules, "Third Party" C++ nodes, and a `hise::raw` C++ API that bypasses the app entirely), and — of note for this project's LLM seam — an **MCP server, a CLI and an LSP server shipped as part of the repo** so agents can drive the editor, rebuild and profile.
- **Real-time safety is instrumented, not architectural**: lock-free queues (`moodycamel`/`LockFreeDispatcher`) exist beside `SpinLock`/`CriticalSection` around engine state, and the debug logger ships a `PriorityInversion` failure type with explicit `checkPriorityInversion(...)` calls to catch the audio lock being held. That is a posture we deliberately do not share.

**What is worth stealing** (the part that outlives "inspiration only"):

1. **Interpreted prototyping, AOT freeze, with the graph embedded for round-trip** — the single best idea here, and one we could implement in Rust with a code generator emitting our own composition (and it composes with our patch-as-value: the graph stays a value, the generated code is an artifact).
2. **A restricted container tree instead of a free pin graph** — serial/parallel/per-channel/per-sample/oversampled covers most DSP and keeps both the UI and the compiler simple; polyphony then falls out as per-voice state.
3. **A tiny typed JIT language for user DSP where the *same source* also compiles into the AOT build** — Rust's equivalent would be Cranelift/wasmtime, but the invariant is the point. (The one Rust project that tried this shape, HexoSynth/`hexodsp`, was withdrawn — see below.)
4. **Parameters and modulation as one abstraction** (double-valued function objects with range conversion; the container decides the update rate).
5. **Instrument packaging as a first-class, engine-independent layer** (sample maps, monoliths, resource archives, presets) — which is exactly the profile shape our umbrella thesis predicts, seen from the instrument side.
6. **Build the priority-inversion detector and task-after-suspension plumbing in from day one**, rather than discovering it in the field.

**The Rust analogue gap.** Nothing in Rust combines HISE's parts, and the pieces that exist do not join up:

- *Graph engines*: fundsp 0.23.0, firewheel 0.14.0 (active, ~50k downloads), knyst 0.5.1, rill-lang 0.6.0-M2; oddio and `dasp_graph` are dormant.
- *Plugin export and hosting*: clack 0.2.0 (2026-09-12, MIT/Apache — the healthy CLAP host+builder), nice-plug 0.4.2, truce 6.3.0 (custom licence), `mkapk` for one-processor→Standalone/VST3/AUv2/AAX packaging (a 1★ project, not on crates.io). **nih-plug is in maintenance mode** by its own README (community fork on Codeberg), the framework itself is not on crates.io (only sub-crates), and its VST3 export rides the GPLv3 `vst3-sys` bindings — RESEARCH §10 already records this.
- *Plugin hosting*: `vst3-host` 0.9.0 (MIT, 2026-07-28) is the first Rust VST3 host worth watching; `clack-host` covers CLAP. Both young.
- *Samplers/instruments*: **no mature Rust one.** The nearest are SoundFont players (`rustysynth` 1.3.6 MIT, `oxisynth` 0.1.0 LGPL-2.1); `sofiza` is the SFZ parser (dormant 2022); RustAudio's `sampler` is archived. And the names are traps: crates.io `sfz` is a static-file HTTP server, `same` is object-identity traits, `patchbay` is a network-namespace lab, `carla` is a self-driving simulator client.
- *Visual patch → native*: HexoSynth/`hexodsp` upstreams now 404 (the `synfx-dsp-jit` crate still builds from crates.io — a live supply-chain wrinkle); `quartz` is explicitly unmaintained; the two nearest 2026 attempts are `helgev-in-arcana/audio-graph` (a node-graph plugin that *hosts* VST3/CLAP nodes) and `ampactor-labs/sonido` (one DSP kernel → CLAP + browser node-graph editor + Daisy firmware), both 3★ and both **one fixed binary with a patch inside** — neither emits a new plugin per patch.

**No maintained project joins "visual patch graph" to "native code" to "plugin" in Rust.** If that is interesting later, the non-Rust reference points are HISE's scriptnode, Cycling '74's **RNBO**, and **Cmajor** (proprietary, LLVM-JITs patches live and exports via a generated JUCE project) — three systems with the same shape, all C++/commercial-side.

One licensing contrast worth keeping between the two environments: **VCV Rack is GPL-3.0-or-later *with* a §7 plugin exception** (free plugins may be released under any terms; selling non-GPL ones needs a commercial royalty licence), while **HISE has no plugin exception** — hence "everything you build with it is GPLv3 unless you buy the licence". If a modular-style plugin ecosystem were ever the goal, that difference matters more than any technical one.

**Why it is inspiration and not inclusion:**

1. No Rust, and no embeddable library surface — the only ways in are its own scripting/scriptnode layers, i.e. living inside its editor.
2. **License**: GPL-3.0-or-later, and it propagates — HISE's own docs say every plugin built with it must be released under GPLv3, with a commercial licence (Indie/Pro, per-year) as the escape hatch; that commercial licence explicitly *excludes* `hi_backend` (the IDE), so embedding the editor in a product is not covered even then.
3. **No CLAP export** (verified: zero whole-word `CLAP` hits across `hi_core`/`hi_backend`/`hi_frontend`/`hi_tools`/`projects`). Our external-instrument route is CLAP via `clack` (Route A in the [integration-routes research](2026-08-19-softsynth-integration-routes.md)); a HISE instrument would arrive as VST3/AU/AAX, and VST3 hosting in Rust is the thing we explicitly deferred as immature (RESEARCH §10) — though a new `vst3-host` crate (MIT, 0.9.0, 2026-07-28) is the first sign of that gap closing. So today the "use one HISE instrument" path needs a host we chose not to build. (The current official loader for HISE libraries, RHAPSODY by Libre Wave, is itself VST3/AU/AAX, GPL-3.0.)

## 5. Fit against our existing seams

Faust lands on seams we already drew; HISE lands on none.

| Faust artifact | Our seam | Notes |
|---|---|---|
| A `.dsp` + its generated Rust | **Opaque tier** (`NodeKind::Opaque(Box<dyn AudioNode>)`) | Peer of the fundsp voice in the [native-soft-synth note](../../.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md) — *another block source behind our voice layer*, not a new graph model. |
| The `.dsp` text + its `-json` description | **Patch-as-value** ("existence and params are values, code is trusted in-process") | The DSP source is a diffable value; the parameter surface (path, range, unit, default) is machine-generated instead of hand-written. |
| `compute()` block API + declared `latency` | `AudioNode::render` / `latency()` / `set_param` | Adapter only: interleave/deinterleave, name→index map, declared-latency read, tail flag. |
| libfaust JIT / faustwasm | **Future user-DSP tier** (nothing today) | The HISE-style "user writes DSP" feature. If we ever want it, prefer the wasm route over in-process LLVM. |
| Faust "process a buffer" | *(not CDP's tier)* | Faust is a sample-block processor; it does not replace the `OfflineProcess` sidecar slot (CDP8, PaulStretch) for file-based offline work. |
| HISE | — | No ABI we host, no Rust, not a library. Design reference only. |

Two boundaries worth stating plainly:

- **Faust does not replace fundsp.** fundsp is a pure-Rust library with no external tool; Faust is a language whose code is generated by a binary at build time. They cover different needs — hand-written parametric Rust blocks versus a large authored DSP corpus — and the build-time-tool shape is arguably *cleaner* for the minimal core than another crate (it adds no runtime dependency and no crate graph).
- **Faust does not bring polyphony, events, or the patch.** Those stay ours; that is the whole point of the voice layer in the native-soft-synth note.

## 6. Licensing

Everything Faust-related composes with our GPL-3.0-or-later app, and the AOT route is unusually clean:

| Component | Licence | Consequence |
|---|---|---|
| Faust compiler | **LGPL-2.1-or-later** (COPYING.txt; it shipped GPLv2 through 2.68.1 and was already LGPL-2.1+ at 2.69.3 — relicensed in late 2023) | Build-time invocation → **no linking at all**, so no copyleft obligation of any kind. |
| Code generated by the compiler | **Not covered** by the compiler licence — "the LGPL license of the compiler *doesn't* apply to the code generated by the compiler. The license of the code generated … depends only on the licenses of the input files" ([FAQ](https://faustdoc.grame.fr/manual/faq/)) | AOT output is ours to license; safe even in a future permissive build. |
| Faust standard libraries | Mixed, but with an explicit exception: "you may create a larger FAUST program which directly or indirectly imports this library file and still distribute the compiled code … under your own copyright and license" (`basics.lib` header) | `faust -json` **names the licences per library** — our probe reported `envelopes.lib = LicenseRef-LGPL-2.1-or-later-with-Faust-exception` and `filters.lib/fir = LicenseRef-STK-4.3`. CI can audit this instead of trusting a doc. |
| Architecture files | Base licence varies (`cpal.rs` is GPL-3.0+); each carries the exception "you may create a larger work that contains this FAUST architecture section and distribute that work under terms of your choice, so long as this FAUST architecture section is not modified" | Non-issue for us; and writing our own ~20-line arch file (as the spike did) avoids the question entirely. |
| libfaust (if ever embedded) | LGPL-2.1-or-later; §6b covers dynamic linking ("work that uses the Library"), §6a covers static (relinkability) | Moot under GPL-3.0-or-later; and only relevant for the deferred user-DSP tier. |
| HISE | GPL-3.0-or-later + a paid commercial licence that excludes the IDE backend | Plugins built with HISE are GPLv3 unless commercially licensed. Irrelevant to us unless we adopt it, which we do not. |

## 7. Recommendation

1. **Adopt Faust as an optional, feature-gated *DSP source* for our own instruments** — `-lang rust`, AOT, wrapped in our `AudioNode`, behind the existing voice layer. Recipe, verified end to end here:
   `faust -a crates/<…>/faust/arch_min.rs -lang rust -rnt -rnlm -it -cn <Name> <file>.dsp -o <file>.rs`
   with a ~20-line arch file defining `F32/F64/FaustFloat`, `ParamIndex`, `Meta`, `UI<T>`. The result needs **no crate, no FFI, no allocation**, and the generator is byte-deterministic, so generated Rust can be checked in (auditable) or regenerated against a pinned compiler.
2. **Do not embed libfaust now.** Reserve it for a future *user-authored DSP* tier — and when that arrives, prefer **WASM + a wasm runtime** (precompiled `faustwasm` output runs without libfaust or LLVM) over putting LLVM in the product. That is HISE's SNEX idea implemented with a sandbox instead of a JIT.
3. **Do not adopt HISE.** Mine three ideas: the interpreted-graph → exported/compiled-node ladder, a user-DSP tier, and the "one project per instrument, with an export pipeline" profile shape — the last is our umbrella thesis already, seen from the instrument side.
4. **Next steps (a spike, not this change):** one real voice `.dsp` (osc + envelope + filter) wrapped as an `AudioNode`, passing the fundsp spike's invariants (zero-alloc counting allocator, byte-identical replay, sample-accurate onsets, logged `set_param`, PDC from declared `latency`); settle the parameter-identity convention (`-json` `address` vs UI-walk path) and the tail/latency declaration in the mount value; pin the compiler version and add a `faust -json` licence-metadata check to CI.
5. **What would falsify this:** `-it`/`-rnlm` proving insufficient on real patches (`soundfile` is unsupported in the Rust backend; `-quad`/`-omp`/scheduler throw); the ≈1.3× DSP gap becoming material at our polyphony targets; or the `RwLock`-guarded table pattern (§3.2) surviving `-it` in common patches.

Known sharp edges to carry into the spike, all verified in this pass:

- Table-using DSPs emit **`static RwLock<[F32; N]>` tables locked inside `compute`** (`// Obtaining locks on 1 static var(s)`, then `.read().unwrap()`) — a lock and a panic path in the render path, and table state shared between instances of the same node. **`-it` (inline tables) removes the statics entirely** ("Obtaining locks on 0 static var(s)") and should be mandatory in our recipe.
- `-rnlm` drops the `unsafe extern "C" { remainderf/rintf }` (libm) block; without it, libm must link.
- `-rnt` drops the `FaustDsp` trait impl, leaving a plain struct — which is what we want.
- The DSP struct holds delay buffers inline; **construct it boxed** (the architecture README's stack-overflow warning, `default-boxed`).
- `class_init` (shared static tables) is once per class, `instance_init` once per instance.
- `latency` must be declared by the DSP author (`declare latency "N";`); Faust does not infer it, so PDC is only as good as the patch metadata.
- Denormals are **not** flushed by default (`-ftz 0`); decide `-ftz 2` per project, knowing it changes the last bits of the output.
- Cross-backend float results differ in the last ulp(s) (§3.4): replay determinism holds within a pinned compiler, not across backends or compiler versions.
- `faustwasm` (the browser path, if we ever want an in-UI preview) is npm **LGPL-3.0**, not the compiler's LGPL-2.1+.

## 8. Sources

**Faust — primary (local, 2.88.0):** `faust --help` (language list; `-noreprc`/`-rnt`/`-rnlm`), `/usr/share/faust/rust/` (`minimal.rs`, `cpal.rs`, `README.md`), `/usr/include/faust/dsp/libfaust*.h`, `/usr/lib/libfaust.so.2.88.0`, and the compiled probes described above. Generated-file quotes and all measurements come from this run.

**Faust — web:** [faustdoc FAQ — compiler licence and deployed code](https://faustdoc.grame.fr/manual/faq/) · [compiler options](https://faustdoc.grame.fr/manual/options/) · [faust2 tools](https://faustdoc.grame.fr/manual/tools/) · [architecture files](https://faustdoc.grame.fr/manual/architectures/) · [embedding the compiler](https://faustdoc.grame.fr/manual/embedding/) · [deploying on the web](https://faustdoc.grame.fr/manual/deploying/) · [syntax](https://faustdoc.grame.fr/manual/syntax/) · [COPYING.txt](https://github.com/grame-cncm/faust/blob/master-dev/COPYING.txt) · [compiler/generator/rust](https://github.com/grame-cncm/faust/tree/master-dev/compiler/generator/rust) · [rust architecture files](https://github.com/grame-cncm/faust/tree/master-dev/architecture/rust) · [faustlibraries `basics.lib` exception](https://raw.githubusercontent.com/grame-cncm/faustlibraries/master/basics.lib) · [Stéphane Letz on the Rust backend's origins (2021)](https://sourceforge.net/p/faudiostream/mailman/message/37195218/) · [faustwasm](https://github.com/grame-cncm/faustwasm) · IFC 2024 talk [*Experiences with Rust + Faust*](https://faust.grame.fr/community/ifc/2024/Experiences_with_Rust_Faust.pdf).

**Rust ecosystem:** crates.io metadata for `faust-build` 0.2.1 and `faust-types` 0.2.1 (both 2024-11-20, MIT OR Apache-2.0); [Frando/rust-faust](https://github.com/Frando/rust-faust) (Apache-2.0, last push 2025-05-10); `faust-state`/`faust-macro` unpublished; no `libfaust`/`faust-llvm` crate.

**HISE:** [christophhart/HISE](https://github.com/christophhart/HISE) (develop HEAD `5d34685`, 2026-08-16; tags `v4.9.0`–`v4.9.3`, last release 4.1.0) and [hise_documentation](https://github.com/christophhart/hise_documentation) — cloned and grepped for Rust/CLAP as described; [docs.hise.audio](https://docs.hise.audio/) (scriptnode, SNEX, glossary, C++ generator, export) and the [licence text](https://github.com/christophhart/HISE/blob/develop/license.txt); [hise.dev licence preview](https://hise.dev/hise_license_preview.pdf) (Indie/Pro terms, `hi_backend` carve-out); [forum.hise.audio](https://forum.hise.audio/) (Faust/LLVM version threads, licence threads); [RHAPSODY player](https://librewave.com/rhapsody/) and the [hise.dev showcase](https://hise.dev/).

**Adjacent systems (for the comparison):** [VCV Rack plugin licensing](https://vcvrack.com/manual/PluginLicensing) · [Cmajor](https://cmajor.dev) · [RNBO plugin export](https://rnbo.cycling74.com/learn/audio-plugin-target-export-overview) · crates.io/GitHub metadata verified for `clack-host` 0.2.0 (MIT/Apache, 2026-09-12), `nih_plug_core` 0.1.2 (ISC, framework itself unpublished, [README maintenance-mode notice](https://github.com/robbert-vdh/nih-plug)), `nice-plug` 0.4.2, `truce` 6.3.0, `vst3-host` 0.9.0 (MIT, 2026-07-28), `firewheel` 0.14.0 (MIT/Apache, 2026-09-05), `knyst` 0.5.1, `rustysynth` 1.3.6 (MIT), `oxisynth` 0.1.0 (LGPL-2.1), `sofiza` 0.3.1, `quartz` 0.0.4 (unmaintained), `synfx-dsp-jit` 0.6.2 (repo deleted), [mkapk](https://github.com/mkaudio-company/mkapk), [audio-graph](https://github.com/helgev-in-arcana/audio-graph), [sonido](https://github.com/ampactor-labs/sonido).

**Cross-checks:** [softsynth integration routes](2026-08-19-softsynth-integration-routes.md), [native soft-synth building blocks](../../.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md), RESEARCH §10 (Rust DSP ecosystem) and §12 (licensing).

*Authored with deepseek-v4-flash · DeepSeek Harness, 2026-09-20.*
