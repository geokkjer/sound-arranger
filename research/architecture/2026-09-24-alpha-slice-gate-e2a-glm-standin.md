# Reviewer gate (stand-in) — alpha slice E2a (WSOLA core), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unusable: every API run today died with no output — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: the time-stretch slice's first half — `media::Stretch` (a streaming, deterministic WSOLA
> core), its tests, and its registration. This reviewer went further than reading: it compiled an
> **out-of-repo probe against the built rlib** to reproduce the edge cases, which is why both real
> findings are backed by executed evidence rather than inference.
>
> **Disposition: `merge with changes`; the must-fix and both should-fix findings were real and are
> fixed, with tests.**
>
> 1. **A region shorter than one hop came back as the material followed by silence** (must-fix) —
>    `variant` gave `blocks == 0` priority over the last-block test (so a single-block render never
>    got a flat-falling outer half), and `at_end = fed - window` saturates to 0 when the region is
>    shorter than the window, so the window's far half read zero padding. Reproduced by the probe:
>    twenty 0.5 frames through `for_len(2,1,20)` → `[0.5 × 20, 0.0 × 108]`. The shipped tests never
>    hit it because `for_len`'s hop kept `hop_in < len`. Fixed three ways: the window variants now
>    cover *both* edges (a single-block render is a rectangular window), every hand-out is clamped
>    to the target length (so padding cannot be handed out), and — because a region shorter than a
>    window cannot be *stretched* at all (there is nothing to overlap) — that limit is now an
>    explicit contract: `min_region_frames()` and `for_len` sizing the transform to the region.
>    Regression test `a_short_region_is_stretched_when_the_window_fits_it` at four ratios.
> 2. **`is_identity` compared the rounded hops, not the ratio** (should-fix) — `hop_in =
>    round(hop_out × den / num)`, so `2049/2048` (and `129/128` at a small hop) reported an identity
>    and a caller would **copy** material that a logged stretch claimed to transform (≈0.15 s of
>    grid drift over a 5-minute take). Fixed: the ratio is compared (`num == den` is exactly 1.0),
>    and `a_near_identity_ratio_is_not_an_identity` pins both the true and the near cases. The
>    rational is also exposed (`rational()`), since the log will carry it verbatim.
> 3. **The mono contract was stated nowhere** (should-fix) — the module docs now say it: a
>    `Stretch` transforms one channel, the caller feeds a mono region (every pool source is mono),
>    and the host's render command owns the multi-channel decision. (A channel parameter is a
>    second-half question, not a silent assumption.)
> 4. **The correlation tie-break was biased to `−search`** (note) — `score > best` keeps the first
>    candidate and `d` iterates from `−search`, so on a silent overlap (every score 0) the read was
>    systematically early; the probe reproduced a 536-frame shift where a quiet passage meets a
>    signal. Fixed: ties prefer the offset nearest the nominal position.
> 5. **Documentation overclaims** (note) — "byte-reproducible" is per build (libm's `cos` last-ulp
>    differences across platforms are the one thing that could shift a byte), and the window sum is
>    1 to within f32 rounding rather than exactly. Both corrected in the module docs and the note.
> 6. **The test evidence was weaker than the claims** (note) — the zero-crossing pitch test cannot
>    see beating from imperfect alignment, and the tail-window behaviour under expansion was
>    untested. Added: `streaming_in_chunks_matches_one_shot` (bit-for-bit across chunkings — the
>    property the gate had verified only by probe), plus the short-region and near-identity tests;
>    the note now says plainly what the pitch test is (a countable proxy) and what it is not.
>
> Reviewer-verified (recorded because they are the slice's load-bearing claims): the overlap-add
> bookkeeping (`base = blocks × hop_out`, a block's first half final, `produced = acc.len()` at
> flush, no double-writes, no holes); **streaming equals one shot bit-for-bit**; `trim` never drops
> input a later block reads; the search is bounded (`at < 0` skipped, candidate reads break at
> `fed`, no NaN on silence); the ratio arithmetic (`u32`→`f64` cannot overflow, `hop_in` capped at
> one window, `len = 0` → empty); bounded streaming memory; and that a constant reconstructs across
> all three window-variant boundaries (the probe measured 0 frames off by more than 1e-6).

---

## Verbatim review

Adversarial review of the WSOLA core (crates/media/src/stretch.rs + lib.rs registration + note). Workspace suite green, 6 stretch tests pass. I additionally compiled an out-of-repo probe against the built rlib to verify edge cases the tests don't reach — both suspected bugs are REAL and reproduced:

## Verdict
merge with changes

## Findings
1. **must-fix** — A render that fits in one block (input ≤ `hop_in`) emits the input followed by ZEROS (or, when `fed == window`, a Hann fade-out). Evidence: stretch.rs:219-225 — `variant` gives `blocks == 0` priority over the flushing/last test, so the single block never gets the flat-falling outer half; and stretch.rs:234 `at_end = fed.saturating_sub(window)` is 0 when `fed < window`, so `sample(start+j)` (stretch.rs:306-314) reads zero padding. Reproduced: `Stretch::for_len(2,1,20)` on twenty 0.5 frames → output = [0.5×20, 0.0×108]. Bites via `Stretch::new(1,2)` (hop_in = 2048 = window) on any region ≤ 2048 frames (42 ms), or `new(1,4)` etc. — contradicts the module claim (stretch.rs:15-19) that the material's edges survive. `for_len` masks it in the tests (hop ≈ len/8 ⇒ hop_in < len always), so the shipped suite never hits it. Fix: a combined flat-flat window when a block is both first and last, and clamp/hold the read sensibly when `fed < window`.

2. **should-fix** — `is_identity` is `hop_in == hop_out` (stretch.rs:153-155), not `num == den` as the note claims (note line 45). Because `hop_in = round(hop_out·den/num)` (stretch.rs:125), near-identity ratios round to `hop_out` and are silently reported as identity: verified `for_len(129,128,512)` → is_identity=true, and `new(2049,2048)` → is_identity=true. A caller tempo-matching by a fine rational copies instead of stretching: length error = input×(num/den−1) — ~0.15 s on a 5-minute take, i.e. grid drift; the logged op would claim a stretch that never happened. Fix: store `num`/`den` and compare them (or compare the f64 ratio == 1.0).

3. **should-fix** — The mono contract is stated nowhere. `Stretch` is mono (single `buf: Vec<f32>`, stretch.rs:70); the note and code never say the caller must feed mono regions or how stereo is handled (interleaved would be treated as one signal — phase-mangled; per-channel instantiation picks different offsets — image shift). The second half (host render command) will trip on this. Add the contract to the docs/note, and consider a debug assert or a channel-count parameter later.

4. **note** — Correlation tie-break is systematically biased to −search: `score > best.1` (stretch.rs:298-299) keeps the first candidate among equal scores, and d iterates from −search; when the written overlap is all zeros (silent passage) every candidate scores 0. Verified: silence-then-signal input, `last_offset = −256`, signal appears ~536 output frames early. Deterministic, bounded by `search` (hop/4), but placement near silence boundaries shifts by up to that much. Block 0 is safe only by accident (nominal = 0 ⇒ negative `at` skipped ⇒ d = 0 wins; stretch.rs:281-283).

5. **note** — The reproducibility claim is over-broad. Same-process/same-machine bit-identity is real (verified, including streaming in 37-frame chunks == one shot, bit-identical), and the arithmetic order is fixed (f64 accumulators, no HashMap/time/randomness). But `hann` uses `f64::cos` (stretch.rs:38): libm last-ulp differences across platforms/compilers can change the f32 window and hence the bytes. The doc's "same bytes on every run" (stretch.rs:8-9) is fine; do not let the note's "byte-reproducible" (line 37-38) be read as cross-platform. Likewise "w[n] + w[n + hop] == 1 exactly" (stretch.rs:33-34, note line 29) is not exact in floating point — the constant test passes at 1e-3, not exactly (verified all-0.5 render has 0 frames outside 1e-6, so it's very good, but "exactly" is a doc overclaim).

6. **note** — Test evidence is weaker than the claims. Zero-crossing count (stretch.rs:382-395) can't detect beating/amplitude modulation from imperfect overlap alignment (a detuned-but-crossing output passes); the transient test (415-434) checks peak position within a hop but not that the held tail window isn't *duplicated* under expansion (a click in the last window repeats once per extra flush block — untested). The single-block and near-identity cases above are exactly what these tests would not catch.

## Verified
- Overlap-add bookkeeping: `base = blocks × hop_out` (229), middle Hann windows overlap-add correctly; `produced = base + hop_out` (247) is right (a block's first half is final; the next block adds only to its second half); flush's `produced = acc.len()` (254) = `blocks × hop + hop` matches the documented length; no frame written twice or handed out early; output length formula verified empirically (23552 = blocks·hop+hop).
- Streaming equivalence: chunks-of-37 process calls produce bit-identical bytes to one shot (probe); the non-flush wait condition (214) guarantees all candidate reads are in real (untrimmed) data, so `fed` at run time cannot change a block's bytes; `trim` (317-328) never drops data a later block reads (next start ≥ nominal+hop_in−search ≥ keep_from since hop_in ≥ 1; forward data never trimmed).
- Search safety: `at < 0` skipped (282-283); candidate reads break at `fed` (289-290); flush's held read keeps `at+j < fed`; all-silent input yields no NaN (energy guard 1e-12, test covers).
- Ratio arithmetic: u32→f64 ratio can't overflow; hop_in capped at one window (125-127) so >2× compression degrades to approx-2× (documented in note as open); hop_out forced even (114); len=0 → empty output; huge/tiny num/den behave via the round+clamp chain.
- Cost: per-block work is the search only; `acc.resize` amortizes; `buf` bounded by window+search+chunk — bounded memory streaming (whole-output `acc` is documented as intended).
- First/middle/last window overlap sums: first↔middle and middle↔last boundaries reconstruct a constant (probe: 3000-frame constant at 1/2 → 0 frames off by >1e-6).

## Not verified
- Actual audio quality on real program material (drums/music) — no listening or SNDR measurement.
- Cross-platform byte-identity (only same-machine checked; libm cos caveat above).
- The second-half integration (host command, ArrangeOp::Stretch, log replay) — not in this diff.
- `for_len`'s hop choice (len/8 clamp) beyond the tests' ranges — no adversarial sweep over len/ratio combinations.
