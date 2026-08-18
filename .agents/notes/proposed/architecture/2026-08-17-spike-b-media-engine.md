# Agent Note: Spike B — the media engine (disk streaming, recording writer, device-clock drift, splice-during-playback)

Status: proposed

## Problem

Spike A.5 proved the core (clock, graph interpreter, session log, context) and the patch bay. What it deliberately did *not* prove is the product's hard core — the part that must meet the audio deadline and that every profile depends on: playing a 20–30 minute file from disk without glitches, recording long takes to disk safely, keeping the input and output device clocks reconciled over long runs, splicing a clip *during playback* without a click, and bouncing byte-identically. The minimal-core note names this the **media engine — core-privileged, not a plugin** (minimal-core note §5): per-source reader threads, read-ahead rings, underrun policy, the recording writer with WAV header finalization and crash recovery of in-progress takes, and input↔output device-clock drift reconciliation. Spike B exists to prove these against the core shape *before* it hardens (minimal-core note, Risks: "if streaming and the writer force changes to the core shape, that is the cheapest possible moment to learn it").

## Proposal

Spike B ships **`crates/media`** — the media engine, a sibling crate to `crates/engine` (deps: `engine` path + `cpal` 0.18.x only; WAV I/O hand-rolled). The engine crate stays std-only and untouched: media nodes are **opaque-tier `AudioNode`s** mounted directly into `engine.graph` (`NodeKind::Opaque` + `graph.set_out` — the patch bay's own opaque tier, no core change). RESEARCH §11's "both spikes live in the engine crate alone" is honored in spirit — the Rust engine workspace alone, no Tauri, no frontend — and corrected in §11 to name the crate (see Alternatives).

### Modules

- **`ring.rs` — `Spsc<T>`**: lock-free single-producer/single-consumer ring (std atomics, acquire/release), fixed capacity at creation, `try_push`/`try_pop` only. The audio path touches nothing else. Used reader-thread→node and node→writer-thread.
- **`wav.rs`**: minimal RIFF reader/writer. `WavReader` (16-bit PCM + 32-bit float, mono or stereo→channel 0, chunked `read_into(&mut [f32])`); `WavWriter` (16-bit PCM, placeholder sizes in the header up front, `finalize()` patches sizes, `recover(path)` rescans and patches the header of a take that died mid-write — crash recovery of in-progress takes). Hand-rolled because the crash-recovery feature requires mid-write header control `hound` doesn't expose; `hound` stays the Phase-1 upgrade if format edge cases bite.
- **`stream.rs`** — the long-file player: `ClipRef { path, start, len }` (a named region — the clip seed), `FilePlayer` (reader thread + `Spsc<f32>` + eof/produced counters; the reader thread is started and its ring warmed on the *control side*, never on the render path), `PlaybackNode` (engine `AudioNode`, `out("audio")`): consumes the ring, zero-fills and counts underruns, and applies **splices sample-accurately inside the block** — an equal-power crossfade from the current source to the incoming clip over `crossfade` samples at the requested absolute frame.
- **`record.rs`**: the **`Recorder` seam trait** (composition-seams note) + the WAV provider: `WavRecorder` (writer thread + `Spsc<f32>` + `finalize()` on stop) and `RecordNode` (engine `AudioNode`, `in("audio")`): pushes its input into the recorder ring; `stop()` finalizes the take. A crashed take is recoverable via `wav::recover`.
- **`drift.rs`** — **`DriftCompensator`**: input↔output device-clock drift reconciliation. A pure fractional-accumulator linear-interpolation resampler: the input device delivers `in_rate` frames per second, the session runs at `out_rate`; the compensator turns a drifting input block into exactly `out.len()` output frames so the recorded timeline is in session frames and the buffer can't grow unbounded. The ratio is a parameter in the spike (real measurement from device frame counts is Phase 1, where cpal's `OutputCallbackInfo`/`InputCallbackInfo` timestamps become available).
- **`devices.rs`** — the cpal device path: device enumeration/selection and `open_output` / `open_input` stream builders whose callbacks only push/pop the ring (never allocate or block). Exerciseable for real on this machine (Scarlett 2i2 present).

### The splice command seam (control → render)

`PlaybackNode` drains a shared **mailbox** (`Arc<Mutex<VecDeque<SpliceCmd>>>`, `SpliceCmd { at_frame, incoming: FilePlayer, crossfade }`) with `try_lock` — the render path never blocks on it. The control side constructs the incoming `FilePlayer` (thread spawn + warm ring) *at command-issue time*; the render path only mixes two rings. This is the spike's miniature control→render handoff, in the same documented Phase-0 category as mounts/patches applying on the render call stack (patch-bay note, Consequences): command *application* is render-side, but all allocation and thread spawning happen control-side at issue time. Phase 1 replaces the mailbox with the real lock-free control→render channel (already tracked).

### Core-shape findings Spike B records (not fixes)

1. **The session log's flat-f32 plugin params cannot express file-backed plugins** — a recorder/player needs a path, not `Vec<(&'static str, f32)>`. The media events (play/splice/record) therefore live as media-side command types in `crates/media` in the spike; Phase 1 decides whether `Event` grows value-typed variants or the media log merges at the profile layer.
2. **Media mounts bypass the core log** (nodes go straight into `graph`); media determinism is therefore proven at the media-command level (same command sequence ⇒ byte-identical bounce) in Spike B's tests, not via `replay_from`.
3. **Master-out ownership**: media playback sets `out_node` directly — the same mount-order bus limitation already tracked (Phase 1 mixer owns the bus).
4. Media nodes report `latency() == 0` (ring read-ahead is a stream buffer, not processing latency); the Phase 1 mixer owns per-source PDC.

## Alternatives considered

- **Feature-gate cpal inside `crates/engine`** — keeps one crate but pulls ALSA bindings and OS-specific code into the deterministic std-only core; blurs the boundary the minimal-core note draws between core and media engine. Rejected.
- **Use `hound` for WAV I/O** — battle-tested, but its writer finalizes only on close and cannot patch a placeholder header mid-write, which is exactly what crash recovery needs; the 16-bit/float subset the spike needs is ~150 lines. Rejected for the spike; `hound` remains the Phase-1 upgrade path.
- **Playback/record as plugins through `Engine::register_factory`** — impossible without a core change: the factory signature `fn(&[(&'static str, f32)])` cannot carry a file handle, and `Event::Mount`'s params could not be replayed to rebuild a file-backed node. The impossibility is itself a finding (above).
- **Load the whole file into memory** — trivial for short clips, fails the 20–30 min case and the "long file from disk" requirement. Rejected by definition of the spike.
- **Skip real drift handling in the spike (assert only)** — drift over 25 min is thousands of samples; ignoring it invalidates the recorded take. The compensator is small and pure; included.
- **Real cpal input↔output loopback as the drift test** — hardware-dependent and flaky in CI; the compensator is tested numerically against simulated device blocks; the cpal path is exercised by a device smoke test that skips politely when no device exists.

## Acceptance criteria

- A 20–30 minute file plays from disk through the engine graph glitch-free (soak test, `#[ignore]` for routine runs): every sample of the file reaches the output exactly once, zero underruns, zero overruns; a splice at minute 15 lands sample-accurately with a crossfade of exactly the requested length and no click beyond the fade.
- Recording writes a take to disk whose playback is byte-identical to the rendered master; a take that dies mid-write (no finalize) is recovered by `wav::recover` to its full length.
- A virtual input device at ratio 48001/48000 records a 440 Hz tone at ~440 Hz (zero-crossing check) over many seconds, with the recorded timeline in session frames and bounded buffering.
- Same media command sequence on fresh engines ⇒ byte-identical bounce WAVs (media-level determinism).
- The media render path (steady state, no commands firing) allocates nothing — counting-allocator test in `crates/media`.
- The cpal device path opens the default input and output and runs a short pass on real hardware (this machine's Scarlett 2i2); skips with a message when no device is present.
- `crates/engine` diff is zero; `cargo test` (workspace) and clippy stay green.

## Risks

- **Flaky timing in splice/soak tests** — the reader thread warms on the control side before the splice fires, so the fade window is always fed; underrun counters make any starvation visible rather than silent.
- **cpal API drift** (0.18.x is pre-1.0) — the device path is thin and isolated in `devices.rs`; the numeric tests don't depend on it.
- **The mailbox is a temporary seam** — a Mutex touched by `try_lock` on the render path is acceptable in the spike (same documented category as mounts applying on the render stack) but must become the lock-free control→render channel in Phase 1; recorded in Consequences, not papered over.
