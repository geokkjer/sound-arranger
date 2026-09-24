# Agent Note: Spike B — the media engine (disk streaming, recording writer, device-clock drift, splice-during-playback)

Status: implemented

## Problem

Spike A.5 proved the core (clock, graph interpreter, session log, context) and the patch bay. What it deliberately did *not* prove is the product's hard core — the part that must meet the audio deadline and that every profile depends on: playing a 20–30 minute file from disk without glitches, recording long takes to disk safely, keeping the input and output device clocks reconciled over long runs, splicing a clip *during playback* without a click, and bouncing byte-identically. The minimal-core note names this the **media engine — core-privileged, not a plugin** (minimal-core note §5). Spike B exists to prove these against the core shape *before* it hardens.

## Decision

Spike B ships **`crates/media`** — the media engine, a sibling of the std-only core in `crates/engine` (deps: `engine` path + `cpal` 0.18.x only; WAV I/O hand-rolled). The engine crate is untouched: media nodes are **opaque-tier `AudioNode`s** mounted directly into `engine.graph` (`NodeKind::Opaque` + `graph.set_out` — the patch bay's own opaque tier). RESEARCH §11's "both spikes live in the engine crate alone" now reads "in the engine workspace" — still no Tauri, no frontend.

### Modules

- **`ring.rs` — `Spsc<T>`**: lock-free SPSC ring (std atomics, acquire/release; `UnsafeCell` slots with a documented `unsafe impl Sync`). The audio path touches only `try_push`/`try_pop`.
- **`wav.rs`**: minimal RIFF reader/writer. `WavReader` (**16/24-bit PCM + 32-bit float** — 24-bit is what other tools write, decoded to f32 on the control side; a 32-bit *PCM* tag is refused rather than guessed at, since only float is a WAV this pool writes. Mono, or stereo→channel 0; chunked `read_into`); `WavWriter` (16-bit PCM, placeholder sizes up front, `finalize()` patches sizes, `recover()` rescans and patches a crashed take). The writer thread **flushes after every chunk** — a crashed take loses nothing the writer already drained (found by the crash-recovery test: the `BufWriter` tail would otherwise be lost).
- **`stream.rs`** — the long-file player: `ClipRef { path, start, len }` (the clip seed), `FilePlayer` (reader thread + `Spsc<f32>`; started and warmed on the control side), `PlaybackNode` (`out("audio")`): pops the ring, zero-fills and counts underruns on a **shared `AtomicU64`** (so the rig can assert glitch-freedom), and applies **splices sample-accurately inside the block** — an equal-power crossfade of exactly `crossfade` samples at the requested absolute frame.
- **`record.rs`**: the **`Recorder` seam trait** (`id`/`push`/`stop`/`path`; `push` is audio-path, never allocates/blocks) + the WAV provider: `WavRecorder` (writer thread + `Spsc<f32>` + `finalize()` on stop) and `RecordNode` (`in("audio")`). A crashed take is recoverable via `wav::recover`.
- **`drift.rs`** — **`DriftCompensator`**: input↔output device-clock drift reconciliation — a pure fractional-accumulator linear-interpolation resampler (`push_input`/`pull_output`), with a *hold at the tail* so every delivered sample is consumed. The recorded timeline is in session frames and buffering stays bounded (tested at 48001/48000 and 47999/48000). The ratio is a parameter in the spike; real measurement from cpal device timestamps is Phase 1.
- **`devices.rs`** — the cpal device path: enumeration and `open_output`/`open_input` stream builders whose callbacks only push/pop a ring. Exercised on real hardware (Scarlett 2i2): a 1 s tone plays and is drained; the input captures ~47k samples in 0.5 s.

### The splice command seam (control → render)

`PlaybackNode` drains a shared **mailbox** (`Arc<Mutex<VecDeque<SpliceCmd>>>`, `SpliceCmd { at_frame, incoming: FilePlayer, crossfade }`) with `try_lock` — the render path never blocks on it. The control side constructs the incoming `FilePlayer` (thread spawn + warm ring) at command-issue time; the render path only mixes two rings. All allocation and thread spawning happen control-side. Phase 1 replaces the mailbox with the lock-free control→render channel (already tracked).

## Alternatives considered

- **Feature-gate cpal inside `crates/engine`** — pulls ALSA bindings into the deterministic std-only core; blurs the core/media boundary. Rejected.
- **Use `hound` for WAV I/O** — its writer finalizes only on close and cannot patch a placeholder header mid-write, which is exactly what crash recovery needs. Rejected; `hound` remains the Phase-1 upgrade for format edge cases.
- **Playback/record as plugins through `Engine::register_factory`** — impossible without a core change: the factory signature `fn(&[(&'static str, f32)])` cannot carry a file handle, and `Event::Mount`'s flat-f32 params could not be replayed to rebuild a file-backed node. The impossibility is itself a finding (below).
- **Load the whole file into memory** — fails the 20–30 min case. Rejected by definition.
- **Real cpal input↔output loopback as the drift test** — hardware-dependent and flaky in CI; the compensator is tested numerically against simulated device blocks, and the cpal path by a device smoke test that skips politely when no device exists.

## Consequences

- **Verified by tests (media: 15 unit + 7 integration + 2 ignored):** a 60 s file streams sample-identical with zero underruns and silence after EOF; a splice at frame 60 000 with a 512-sample crossfade is sample-accurate (pure A before, both sources in the window, pure B after); recording round-trips the master to WAV with zero overruns; a crashed take recovers to its full length; drift keeps the recorded timeline in session frames at ~440 Hz (zero-crossing check) with bounded buffering; the same media command sequence bounces **byte-identically**; the media render path (steady state) allocates nothing (counting allocator); the 25-minute soak (play + record + splice at minute 15 + bounce) is glitch-free with zero underruns/overruns; the cpal device path runs on real hardware.
- **The offline render loop is a fast-forward consumer** (hundreds of × real time); the reader/writer threads sustain a fixed rate, so an unpaced fast-forward bounce starves them — real playback never does (the device callback consumes at exactly real-time speed). Tests pace the consumer below the sustained rate (0.5–1 ms per 512-frame block ≈ 10–20× real time); the soak runs at 0.2 ms/block in release. **Phase 1 must decide how offline bounce treats this** (pace the bounce, or guarantee reader throughput) — a core-shape finding the spike surfaced.
- **Core-shape findings recorded (not fixed):** (1) the session log's flat-f32 plugin params cannot express file-backed plugins — media commands live as media-side types in `crates/media`; Phase 1 decides whether `Event` grows value-typed variants or the media log merges at the profile layer. (2) Media mounts bypass the core log (nodes go straight into `graph`); media determinism is proven at the media-command level. (3) Media playback sets `out_node` directly — the same mount-order bus limitation already tracked (Phase 1 mixer owns the bus). (4) Media nodes report `latency() == 0` (ring read-ahead is a stream buffer, not processing latency); the Phase 1 mixer owns per-source PDC.
- **Documented Phase-0 edges:** a splice arriving while a crossfade runs waits for it to complete (the profile's edit model schedules splices on clean boundaries); a command whose frame already passed applies at block start; stopping a recorder mid-render may drop the last in-flight block (the node checks the stop flag before pushing — race-free in the spike's single-threaded control, worth a logged `RecordStop` frame in Phase 1); the drift compensator holds at the tail (take-boundary exactness is Phase 1); device error reporting is open-time only (later stream errors are captured but not surfaced through the handle).
- **Next:** Phase 1 — the recorder plugin (`cpal` → media pool), the clip-editor plugin (cut/paste, splice is proven), the soft mixer owning the master bus, and the control→render handoff (lock-free channel) that absorbs the mailbox.

## Risks

- **Flaky timing in splice/soak tests** — the reader warms on the control side before the splice fires and the consumer is paced, so the fade window is always fed; underrun/overrun counters make any starvation visible rather than silent. The soak is `#[ignore]`d and run in release.
- **cpal API drift** (0.18.x pre-1.0) — the device path is thin and isolated in `devices.rs`; the numeric tests don't depend on it.
- **The mailbox is a temporary seam** — `try_lock` on the render path is acceptable in the spike (same documented category as mounts applying on the render stack) but must become the lock-free control→render channel in Phase 1.
