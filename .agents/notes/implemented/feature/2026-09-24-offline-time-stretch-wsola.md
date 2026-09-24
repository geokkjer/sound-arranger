# Agent Note: offline time-stretch — the WSOLA core and tempo match (alpha slice E2)

Status: implemented

## Problem

"Tempo match" is one of the owner's explicit alpha asks: a take recorded at one tempo has to sit
in a session at another, and a clip has to be able to grow or shrink without changing its *pitch*.
The plan's item 11 fixes the shape — an **offline**, control-side transform that writes a **new pool
source**, with the logged op rewriting the clip's reference, so `Clip` gains no playback-rate
property and the one-frame-domain rule survives (`ArrangerNode` never knows a stretch happened) —
and the algorithm: **WSOLA-class**, overlap-add with a correlation search, because the material is
recorded performance and a phase vocoder smears exactly the transients this product is made of.

This note covers both halves of the slice: the **algorithm** (a streaming, deterministic
stretcher with its tests) and the **materialisation** — the host render command that writes a
stretched region into the pool as a new source, the logged `ArrangeOp::Stretch` and
`source_tempo` state that point a clip at it, the TUI gesture, and the tempo match that derives
the ratio from the session's tempo.

## Decision

`crates/media/src/stretch.rs` — `Stretch`:

- **WSOLA per block**: the synthesis hop is fixed (`hop_out`, 1024 frames ≈ 21 ms at 48 kHz), the
  analysis hop follows the ratio (`hop_in = hop_out / (num/den)`), and each block's read offset is
  chosen within ±`hop_out/4` to **maximize the cross-correlation with the output already written**
  (the overlap region), normalized by the candidate's own energy so a quiet passage is not chosen
  for being quiet. That alignment is what keeps a periodic waveform in phase instead of letting the
  overlap drift into cancellation.
- **The window is a periodic Hann at 50 % overlap** (`w[n] + w[n + hop] == 1` exactly), *except*
  that the **first and last blocks keep their outer half flat**: a pure Hann starts at `0 × input`,
  so output frame 0 would be silent (a 21 ms fade-in and a hole at the first sample) and the last
  hop would fade out. With the flat halves the material's own edges survive exactly — the test that
  a stretched **constant stays constant** (to 1e-3) is what pins it.
- **At the tail the read is held** at the last window fully inside the material, so the final blocks
  stretch the input's **own** last hop rather than reading zero padding into a fade.
- **The ratio is a rational** (`num: u32`, `den: u32`) and the arithmetic is a fixed, allocation-free
  sequence of `f32`/`f64` operations — no FFT, no tables, no randomness — so the same input and
  ratio produce the **same bytes** on every run (a test asserts it bit-for-bit). The log will carry
  the rational, never an `f32`, for exactly that reason.
- **The output length is window-aligned**: `blocks × hop_out + hop_out`, where
  `blocks = ceil(input / hop_in)`, capped at the material's own extent at this ratio plus the last
  window's flat half. The ratio is the musical intent; the length is a measurement the caller logs.
  (Nothing downstream assumes `input × num/den` to the frame.)
- **A region must be at least one window long to be *stretched*** (`min_region_frames`, 2048 frames
  ≈ 43 ms at the default hop): a shorter region has nothing to overlap, so WSOLA can only *place*
  it. `for_len` sizes the transform to the region (a quarter of it, so the window always fits),
  which is what the host's render command will use; a caller that hands a default `Stretch` a shorter
  region should refuse the stretch with a message rather than ship a placement that claims to be
  one. `is_identity` compares the **ratio** (`num == den` is exactly 1.0), not the rounded hops: with
  a default hop of 1024, `2049/2048` rounds `hop_in` to `hop_out` and would otherwise report itself
  as an identity and silently copy.
- `hop_in` is capped at one window: a compression beyond 2× would otherwise advance past the next
  block's window and **drop** the material between blocks rather than compress it. A ratio of
  `num == den` reports itself as an identity so the caller copies bit-exactly rather than
  "reconstructing" a 1:1 stretch.
- Streaming shape mirrors the resampler's: `process(&input, &mut out)` appends only **finished**
  frames (a block's first half; the second half belongs to the next block's overlap), `flush` ends
  the render. Memory is the accumulator (an offline render of a clip is megabytes and the host
  writes it out anyway) plus a bounded input ring.

**Materialisation — how a stretch becomes an arrangement change:**

- **The render is an action; the reference is the state.** `HostSession::stretch(track, clip, num,
  den)` reads the clip's region (channel 0 — a pool source is mono, and a split stereo import keeps
  each channel its own source), runs it through `Stretch::for_len`, writes the result with
  `Pool::write_source` (a `.converting` temp file renamed into place, then peaks rebuilt), and then
  logs **one** `ArrangeOp::Stretch { track, clip, source, src_len, num, den }`. The op points the clip
  at the new material (`source`, `src_start = 0`, `src_len`, fades capped to fit) and touches nothing
  else — `at_frame`, `gain`, the id and `reversed` are untouched — so a replay re-derives the same
  arrangement value **without re-rendering**, `undo` restores the old reference in one step, and the
  rendered source stays in the pool as working material. This is the same split as `record`: an
  action that writes material, a logged op that points at it.
- **The pool source id is deterministic *and keyed on the region***:
  `{source}.stretch.{src_start}_{src_len}.{num}_{den}`, where `src_len` is the region **actually
  read** (the clip's declared `src_len` for a clip inside its source, clamped to the file's end
  otherwise — the id must name the content, and a declared window is not content). The same region at
  the same ratio is the same source, so warping a clip, undoing and warping again reuses the file
  rather than piling up copies. The region is part of the key because the render is a function of the
  region, not of the source: the first version keyed on the source alone and **two clips of one take
  at different offsets overwrote each other's material** — the second render left the first clip
  reading silence (found while preparing the gate brief, reproduced end to end: a 0.354-peak master
  dropped to zero). `a_stretch_id_keys_on_the_region_not_the_source` now pins it with a
  half-tone/half-silence fixture. (Stretching already-stretched material makes a new id, correctly —
  that is a different region at a different ratio.)
- **The op rewrites the reference and caps the fades** (`fade_in + fade_out <= src_len` is the
  model's invariant): a compression can make the old fades too long for the new region, so the op
  caps them the way `Trim` does, and the existing per-op validation against the running value does
  the rest.
- **Refusals are named, not silent**: a zero ratio, a 1:1 ratio (it would copy, not stretch), a
  ratio or output beyond the host's bounds (`MAX_STRETCH_RATIO`, `MAX_STRETCH_FRAMES` — the render
  allocates its output, so both ends are capped), a looped clip (loop phase is not representable in a
  straight region read), a region shorter than `min_region_frames` (the WSOLA core explains why:
  nothing to overlap), a missing pool, a missing source, and a region that reads no frames. Each says
  which fact is missing. The region read is clamped to what the source actually holds (a clip's
  `src_len` is its window, not a fact about the file) and both buffers are reserved with
  `try_reserve_exact`, so a bound that is ever wrong surfaces as an error rather than an abort.
- **`source_tempo <id> <bpm>` is logged state.** It records the tempo a take was performed at, is
  replayed with the rest of the log, and is exposed on the host outcome (`source_tempos`, sorted) so
  a shell can read it rather than keep its own copy of a document fact. The log stores the `bpm`
  (formatted via `fmt_f64`, the same printer as the rest of the format); the **ratio** is derived,
  never logged.
- **Tempo match is shell-side arithmetic on top of two logged facts**: `tempo_ratio(source_bpm,
  session_bpm)` scales both tempos by 1000, rounds, and reduces by `gcd` → an exact rational, with
  `None` for a non-finite or non-positive tempo. `session_bpm` is the tempo **at the clip's start**
  (`tempo_at(at_frame)`), so a tempo-mapped session matches each clip to the tempo it sits on. The
  shell refuses a 1:1 ratio with "already at the session tempo" instead of logging a no-op stretch.
- **`W` (warp) is the gesture**, and it is the only one that asks *why* before acting: without a
  recorded source tempo it says how to record one (`: source_tempo <id> <bpm>`) rather than guessing
  a ratio from `src_len`, because a guess would silently mis-time the material. It reports the render
  time, because the render is offline and a long clip's warp is a real pause.

## Alternatives considered

- **A phase vocoder** (STFT + phase advance). Rejected for this material: recorded performance has
  transients, and a phase vocoder smears them; it also needs FFT tables, which would end the
  "pure arithmetic, byte-reproducible" property the log leans on.
- **Resampling (varispeed)** — the plan's honest fallback. Rejected as the *only* mode: it changes
  pitch, which is not tempo matching. It remains a legitimate future "varispeed" clip property, and
  it is already implemented as `media::Resampler`.
- **Materialising the stretch into the pool *inside* the reader** (a rate property on the clip).
  Rejected by the plan and by the one-frame-domain rule: the arranger would have to resample on the
  audio path, and every reader (stream player, peaks) would grow a rate. An offline render into a
  new source keeps clips as straight reads, and the pool's immutability premise is intact — the new
  source is *new* material.
- **An `f32` ratio in the log.** Rejected: two platforms could print it differently, and the whole
  point of logging the operation is that replay is byte-identical. A rational is exact.
- **A single-shot (non-streaming) API** (`stretch(input) -> Vec`). Rejected: a 30-minute take is
  gigabytes — the streaming shape is what lets the host render clip-region by clip-region and write
  as it goes, and it matches the resampler's existing seam.
- **Normalizing the output by the window sum** (dividing by the accumulated window) to fix the edges.
  Rejected once the flat-edge windows made it unnecessary: the division adds a per-sample cost and a
  0/0 at the first sample, and the exactness it would buy is already there.
- **Exact-length output** (padding or truncating to `input × num/den`). Rejected: the honest grain of
  an overlap-add transform is a hop, and an exact cut would clip a window mid-fade. The log records
  the frames actually written.
- **A `Clip` playback-rate property** (`rate_num`/`rate_den` on the clip, resampling on the audio
  path). Rejected: it puts a resampler in the reader, makes peaks and the stream player rate-aware,
  and it is a *different* feature (varispeed — pitch changes with tempo). Tempo match is
  pitch-preserving by definition, so the transform has to be offline material.
- **Re-rendering on replay** (logging the stretch as an action so a replay re-runs it). Rejected: it
  would make replay time proportional to the number of stretches and would make the arrangement
  value depend on the stretcher's byte output *now* rather than on what was logged. The material is
  the action; the reference is the state.
- **Guessing the source tempo** from the region length or the grid. Rejected: a guess is a wrong
  ratio applied silently, and the whole point of the gesture is that the fact is recorded once and
  reused. The shell names the missing command instead.
- **An `f64` bpm in a deterministic source id** (`s1.stretch.0.75`). Rejected for the same reason as
  the `f32` ratio in the log: two platforms could print it differently. The id carries the integer
  rational.
- **Stretching a looped clip** by stretching one loop and rewriting `loop_len`. Rejected for alpha:
  the interaction between a stretched region and the loop seam (phase, crossfade, whether the tail
  should wrap) is its own design, and a wrong answer is worse than a refusal.

## Consequences

- The hardest part of tempo match — an algorithm that stretches *pitch-preserving* and reproducibly —
  exists and is tested: length within two hops of the ratio, pitch preserved at four ratios (a
  zero-crossing count), a transient within a hop of where it was, a constant exactly constant,
  bit-identical output across runs, and the guards (a zero ratio refused, a one-frame input, silence
  without NaN).
- The cost is the search: ~256 candidates × 128 decimated samples per 21 ms block, all on the control
  side. A minute of audio is a sub-second pause in release; a 30-minute take would be a wait, which
  is why the stretch is an explicit offline action and not something a fader does.
- Nothing on the audio path changed: no allocation, no blocking, and the render still reads a clip
  as a straight region of an immutable source.
- **Tempo match is now reachable end to end**: `source_tempo s1 90` then `W` on a clip of `s1` in a
  120 bpm session renders `s1.stretch.0_4800.3_4`, points the clip at it, shortens the region by 25 % and
  says so — one key, one history entry, one undo. The host test
  (`stretching_a_clip_materialises_a_pool_source`) drives the same path through the CLI format and
  asserts the pitch is preserved, the source has peaks, undo restores the reference, and the
  deterministic id is reused on a repeat; the TUI test
  (`warp_stretches_a_clip_to_the_session_tempo`) covers the gesture, the undo, and the
  missing-tempo message, and `the_tempo_ratio_is_a_reduced_rational` pins the arithmetic including
  the non-finite/zero refusals.
- **Still open**: ratios beyond 2× compression (the hop cap makes them approximate rather than
  dropping material); a transient-aware window length; a stretcher that re-validates `loop_len`;
  time-stretch on the *pool* rather than per clip (stretching a source once and reusing it, which the
  deterministic id makes natural but no command exposes yet); and the tail: the render pads the last
  window's flat half, so a warped clip is up to ~21 ms longer than the ratio implies — audible as
  nothing, but a `trim`-to-tempo-exactness gesture would be the follow-up. One documented
  approximation: a **reversed** clip renders its region *forward* and stays mirrored, so the result
  is `mirror(WSOLA(x))` rather than `WSOLA(mirror(x))` — the same length and pitch, with the
  transient alignment possibly a few milliseconds different (WSOLA's search is not mirror-symmetric).
  Folding the reversal into the render and clearing the flag would be exact, at the cost of making
  two identical regions render different material and so different ids.

## The gate's findings

Each half of the slice was gated separately by a stand-in reviewer (the designated gate, Kimi K3, is
unusable — see [`research/architecture/2026-09-23-kimi-gate-alpha-slices.md`](../../../../research/architecture/2026-09-23-kimi-gate-alpha-slices.md)).
Both returned `merge with changes`, and in both cases the findings were real and are fixed with tests.

**E2a — the WSOLA core** ([review +
disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-e2a-glm-standin.md)); it
compiled an out-of-repo probe against the built rlib to reproduce both real findings:

1. **A region shorter than one hop came back as material followed by silence** (must-fix) — the
   single-block case never got a flat-falling window, and the held tail read sat at 0, so the
   window's far half was zero padding. The shipped tests never hit it because `for_len`'s hop kept
   `hop_in < len`. Fixed: the window variants cover *both* edges being flat (a single-block render
   is rectangular), every hand-out is clamped to the target length, and the case is now an
   **explicit contract** — `min_region_frames` — with `for_len` sizing the transform to the region;
   `a_short_region_is_stretched_when_the_window_fits_it` covers it at four ratios.
2. **`is_identity` was `hop_in == hop_out`, not `num == den`** (should-fix) — `2049/2048` and
   `129/128` round to an "identity" and would have **copied** material a logged stretch claimed to
   transform (~0.15 s of drift on a 5-minute take). Fixed: the ratio is compared, and
   `a_near_identity_ratio_is_not_an_identity` pins it.
3. **The mono contract was unstated** (should-fix) — now in the module docs.
4. **The correlation tie-break was biased to `−search`** (note) — on a silent overlap every
   candidate scores 0 and "first wins" read the material early (the gate reproduced a 536-frame
   shift near a quiet passage). Fixed: ties prefer the offset nearest the nominal position.
5. **Documentation overclaims** (note) — "byte-reproducible" is per build (libm's `cos` last-ulp
   differences across platforms are the one thing that could shift a byte) and the window sum is 1
   to within f32 rounding, not exactly. Both docs corrected.
6. **Test evidence was weaker than the claims** (note) — added a **streaming-equals-one-shot**
   test (bit-for-bit across chunkings, which the gate had verified only by probe) and the short
   region/near-identity cases above; the zero-crossing pitch test remains a countable proxy, and the
   note says so.

**E2b — the materialisation** ([review +
disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-e2b-glm-standin.md)); it ran
probes from `/tmp` against the real binary:

1. **A large `num` aborted the host process** (must-fix) — the output was sized with
   `region.len() * num / den` and `Vec::with_capacity`, so one logged line
   (`stretch t0 c0 4294967295 1`) died with an 824 TB allocation failure (SIGABRT). Fixed:
   `MAX_STRETCH_RATIO` (1000:1) and `MAX_STRETCH_FRAMES` (two hours at 48 kHz) are refused by name,
   the output is reserved with `try_reserve_exact`, and the *input* side is bounded the same way (a
   clip's declared `src_len` is clamped to what the file holds). Regression tests:
   `stretching_a_clip_materialises_a_pool_source` and
   `a_declared_region_longer_than_the_source_is_read_to_the_end`.
2. **The rename-overwrite in `write_source` was POSIX-only** (should-fix) — the deliberate
   "re-render the same deterministic id" path would fail on Windows. Fixed: remove-then-rename
   fallback, keeping the atomic rename where the platform has it.
3. **`valid_id` accepted whitespace** (should-fix) while `write_source`'s doc promised a
   whitespace-free id — an id no `host v1` line could ever name back. Fixed: whitespace is refused,
   with tests for a space and a tab.
4. **A peaks failure left a live source with a misleading error** (should-fix) — the message now
   says the audio was written and only the sidecar failed (an orphaned render after a failed `apply`
   is documented as intended: a render is working material).

**Found while preparing the E2b brief, before the gate reported**: the first id keyed on the source
alone, so two clips of one take at different offsets overwrote each other's material (reproduced end
to end: a 0.354-peak master went silent). The gate's own diff-timing note confirms it caught the tree
mid-fix. Fixed by keying the id on the *rendered* region, with
`a_stretch_id_keys_on_the_region_not_the_source` as the regression test.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
