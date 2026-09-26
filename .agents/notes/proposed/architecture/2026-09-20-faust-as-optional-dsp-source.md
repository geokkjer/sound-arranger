# Agent Note: Faust as an optional DSP source — AOT Rust behind our `AudioNode`

Status: proposed

## Problem

The platform wants *its own* instrument voices, and the [native-soft-synth note](2026-08-30-native-soft-synth-building-blocks.md) settled the layer: we keep our own `AudioNode`/patch-bay graph and use per-sample DSP blocks (fundsp/dasp) behind it, with the voice/polyphony layer above the blocks. That leaves the *content* question open. Hand-writing every oscillator, filter, envelope, reverb and physical model in Rust is a large, unglamorous surface, and it duplicates decades of published DSP. Two mature alternatives were evaluated (2026-09-20): **Faust** (a DSP language with an official Rust backend) and **HISE** (a C++/JUCE instrument-builder environment). The full evaluation, with measurements, is the [research doc](../../../../research/architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md).

The finding that matters: Faust's generated Rust can be dropped into this crates' engine with **no crate dependency, no FFI, no allocation on the render path**, and its parameter surface can be *generated* from the DSP — while HISE has no Rust, no embeddable library surface, no CLAP export, and a GPL-as-environment licence model.

## Proposal

**Adopt Faust as an optional, feature-gated DSP *source* for our own instruments: `.dsp` → generated Rust (AOT) → our own `AudioNode` wrapper. It is a peer of fundsp behind the existing voice layer, not a new graph model, not a runtime dependency, and not the patch value.**

- **Build-time only.** The generated Rust is produced by the `faust` CLI — no `libfaust`, no LLVM, no runtime dependency. Verified recipe:
  `faust -a crates/<…>/faust/arch_min.rs -lang rust -rnt -rnlm -it -cn <Name> <file>.dsp -o <file>.rs`
  with a ~20-line architecture file defining `F32`/`F64`/`FaustFloat`, `ParamIndex`, `Meta`, `UI<T>` and the `<<includeIntrinsic>>`/`<<includeclass>>` placeholders. Output has no `use`/`extern crate`, no `FaustDsp` trait (`-rnt`), no libm FFI (`-rnlm`), and no `RwLock` statics (`-it`). Generated code is byte-deterministic, so it can be checked in (auditable) or regenerated against a pinned compiler — the note does not pick one; the spike's `build.rs`/`xtask` decides.
- **Feature-gated.** A `faust` cargo feature (mirroring the `fundsp` feature) keeps the minimal core dependency-free by default. The generated modules and their tests compile only with the feature on.
- **The opaque tier is the home.** A Faust DSP mounts as `NodeKind::Opaque(Box<dyn AudioNode>)`, declaring ports (`note`/`audio`/`control`), reporting `latency()` from the DSP's declared `latency` metadata, and consuming our `Note`/`Trigger`/`Control` streams in-block. The voice layer above it — note→voice allocation, stealing, release tails, per-voice param offsets — stays ours; the Rust backend has no polyphony helper.
- **Params and identity.** Walk `build_user_interface` (or read `faust -json`) **once at mount** to build `path → ParamIndex`; the path (e.g. `/probe/cutoff`) is the logged parameter identity, so the session log remains the source of truth and the DSP is a pure replay of it. `set_param(name, value)` resolves through that map. **Measured 2026-09-26:** the UI walk and `-json` agree index-for-index — `ParamIndex(n)` is the *n*-th control in `-json` order, and `address` is just the label with `/`→`_` — so the walk is the cheaper source (already in the generated Rust, carries the index, no sidecar file) and there is no convention left to choose. Two refinements from the same measurement: the `[CV:N]` tag arrives as `declare(ParamIndex(n), "CV", k)` on the UI interface, so it is **data, not dead weight**, even in the no-CV engine tier; and `ParamIndex` renumbers whenever two widget declarations are reordered, so only a *name*-keyed log is replay-safe. [Research](../../../../research/architecture/2026-09-26-faust-param-identity-and-declarative-render-target.md) §1.
- **Adapter contract (the six real details).** Interleaved↔non-interleaved conversion through preallocated scratch; block **segmentation at note/param event offsets** (Faust has no timed API in the Rust backend); `class_init` once per class (`OnceLock`) + `instance_init` per instance; the DSP struct **boxed** (delay buffers are inline fields); `latency` read from declared metadata; a per-node `tail` flag for the drain contract.
- **Not now: libfaust.** JIT embedding (LLVM or interpreter) is reserved for a future *user-authored DSP* tier. When that arrives, prefer **WASM + a wasm runtime** (precompiled `faustwasm` output needs neither libfaust nor LLVM) over putting LLVM in the product — HISE's SNEX idea implemented with a sandbox.
- **Not HISE.** Inspiration only: its interpreted-graph → exported/compiled-node ladder, a user-DSP tier, and the one-project-per-instrument profile shape.

## Alternatives considered

- **Hand-write everything on fundsp/dasp (no Faust)** — keeps the tree pure Rust and reviewer-simple, but pays for every reverb, physical model and effect a second time, and gives up a text-authorable, LLM-writable DSP corpus. Rejected as the *only* source; fundsp stays for hand-written parametric blocks, because a language compiler and a Rust library cover different needs.
- **Embed libfaust (LLVM JIT) in-process** — the powerful route (live compilation, the `-json`/box APIs), but it makes LLVM a product dependency (a `libfaust.so.2` of 13.6 MB plus `libLLVM`), moves compilation into the process we most want boring, and adds LGPL linking questions that AOT simply does not have. Rejected for our own instruments; kept for the future user-DSP tier, and even there WASM is preferred.
- **Use `faust-build`/`faust-types` from crates.io** — the obvious-looking path, and **measured broken against current output**: `faust-types` 0.2.1 (2024-11-20) has no `FaustFloat`, failing 18 errors against Faust 2.88.0 output. Rejected; a 20-line architecture file is smaller than the workaround.
- **Adopt Faust's graph/polyphony/architecture layer** (`mydsp_poly`, `-nvoices`, `faust2clap`) — those live in the C++ architecture world; the Rust backend emits neither, and adopting them would put a second graph model and a second voice allocator beside ours. Rejected: our patch stays the value, our voice layer stays ours.
- **Export instruments as plugins with Faust (`faust2clap`) instead of embedding them** — a possible later *distribution* story, not the in-engine source of our instruments; it does nothing for the arranger's own voices. Deferred.
- **Adopt HISE** — no Rust anywhere (verified by clone-and-grep), not a library, GPL-3.0-or-later that propagates to built plugins, and **no CLAP export**, so it cannot even enter through our CLAP route; only its ideas transfer. Rejected.

## Acceptance criteria

- A spike (its own change) mounts one real voice `.dsp` (oscillator + envelope + filter) as an opaque `AudioNode` behind a `faust` cargo feature, and passes the invariants the fundsp spike established: zero allocations in steady-state render (counting allocator), byte-identical replay, sample-accurate note onsets, logged `set_param` with undeclared params refused, and correct `latency()`/PDC.
- The spike settles and records: the parameter-identity convention (`-json` `address` vs UI-walk path — **already measured, see the note above; the spike asserts the agreement rather than choosing**), the tail/latency declaration in the mount value, and whether generated Rust is checked in or regenerated (with the compiler version pinned in the same change).
- The spike asserts the two properties §1 of [the research](../../../../research/architecture/2026-09-26-faust-param-identity-and-declarative-render-target.md) adds: a `ParamIndex` is stable only within one `.dsp` revision (reordering widgets renumbers), and a control's `[CV:N]` tag is readable as `declare(..., "CV", k)` at mount.
- `cargo test -p engine` still builds without the feature (minimal core stays dep-lean), and the feature build adds no runtime dependency beyond generated code.
- RESEARCH §6/§10 link to the [evaluation doc](../../../../research/architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md) instead of restating it.

## Risks

- **Backend performance.** Measured ≈1.3× slower than the C++ backend on a delay/table-heavy stereo reverb (parity on a small osc+filter): 167.6 ms vs 131.9 ms per 100 s of audio at default flags, 124.7 vs 90.7 with native codegen. Absolute cost is 0.12–0.17 % of a core, so the ratio only bites at high polyphony or heavy patches. Re-measure at every compiler bump.
- **Lock in the render path.** Table-using DSPs emit `static RwLock<…>` tables locked inside `compute` unless `-it` is used; `-it` is mandatory in the recipe and must be asserted in review, because a lock (and an `unwrap()` panic path) in the render path is a contract violation no other node has.
- **Unsupported primitives.** `soundfile` throws in the Rust backend; `-quad`, OpenMP and the scheduler throw. A patch that needs them cannot be adopted as-is — a real constraint on the offline/spectral end of the DSP corpus.
- **Reproducibility.** Replay determinism holds *within* a pinned compiler, not across backends or versions (C++ and Rust outputs differ in the last ulp(s)). Pin the compiler like a dependency; the session records what a render was made with.
- **Adapter maintenance.** The Faust Rust backend changes (2024–2026 alone: deterministic codegen, `-rnlm`, `-it`, struct move for derive macros); the adapter is ours and must track it. The spike's tests are the guard.
- **DSP dependency in the minimal core.** Same objection the fundsp spike raised: DSP content does not belong in the interpreter. Mitigation as there — feature-gate now, extract the voice into its own crate once the polyphonic voice pool exists.
- **Licence hygiene.** Generated code is outside the compiler's LGPL, but library functions carry per-file licences (STK-4.3 appears in `filters.lib/fir`); add a `faust -json` licence-metadata check so an import cannot silently bring in something unexpected.

*Authored with deepseek-v4-flash · DeepSeek Harness, 2026-09-20.*
