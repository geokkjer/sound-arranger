# Agent Note: offline time-stretch — the WSOLA core (alpha slice E2, first half)

Status: implemented

## Problem

"Tempo match" is one of the owner's explicit alpha asks: a take recorded at one tempo has to sit
in a session at another, and a clip has to be able to grow or shrink without changing its *pitch*.
The plan's item 11 fixes the shape — an **offline**, control-side transform that writes a **new pool
source**, with the logged op rewriting the clip's reference, so `Clip` gains no playback-rate
property and the one-frame-domain rule survives (`ArrangerNode` never knows a stretch happened) —
and the algorithm: **WSOLA-class**, overlap-add with a correlation search, because the material is
recorded performance and a phase vocoder smears exactly the transients this product is made of.

This half lands the **algorithm**: a streaming, deterministic stretcher with its tests. The
materialisation (a host command that renders a clip's region into the pool, the logged
`ArrangeOp::Stretch`, the TUI gesture and `source_tempo`-driven tempo match) is the second half.

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
- **Still open (the slice's second half)**: the host command that renders a clip's region through
  `Stretch` into a new pool source with peaks; `ArrangeOp::Stretch { track, clip, source, src_len,
  num, den }` + codec + parser + formatter; the TUI gesture; and **tempo match** (a logged
  `source_tempo <id> <bpm>` plus the rational derived from it and the session tempo). Also open:
  ratios beyond 2× compression (the hop cap makes them approximate rather than dropping material);
  a transient-aware window length; and re-validating fades after a stretch grows a clip.

## The gate's findings

The slice's gate returned `merge with changes`; it compiled an out-of-repo probe against the built
rlib to reproduce both real findings ([review +
disposition](research/architecture/2026-09-24-alpha-slice-gate-e2a-glm-standin.md)):

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

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
