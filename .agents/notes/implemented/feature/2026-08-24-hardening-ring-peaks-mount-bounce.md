# Agent Note: review-driven hardening — cache cache lines, peaks Option, mount param validation, bounce guard

Status: implemented

## Problem

The full-codebase review's minors + kimi's following review surfaced several small but real
issues: the SPSC ring's `head`/`tail` shared a cache line (false sharing), the `peaks` query
API panicked on a bad bin/max, mount params skipped the finiteness check, the ring's cache-line
isolation was actually broken (a fat-box pointer offset it), and a malformed `bounce` could
OOM.

## Decision

- **`ring.rs`** — the `Spsc` layout puts the three atomics first (`head`, pad, `tail`, pad,
  `count`, pad) and the buffer last, so `head` (producer), `tail` (consumer), and `count`
  (both) each sit on a distinct 64-byte cache line. The pads no longer depend on the buffer's
  (fat) pointer size — a compile-time `offset_of! % 64 == 0` assertion pins it.
- **`peaks.rs`** — `base_minmax`/`range_minmax` return `Option<(f32,f32)>`: a genuine silence
  bin is `Some((0,0))`, an out-of-range/empty query is `None`; the range fold skips `None`.
  A UI zoom query can no longer panic, and an error is never representable as data.
- **`render.rs`** — `validate_mount` rejects non-finite mount params (`mount tone gain=NaN`),
  so a `NaN`/`inf` never reaches a plugin's apply silently.
- **`graph.rs`** — `MAX_PDC` raised 64 → 4096 (≈85 ms, covering real limiter/reverb lookahead;
  64 was well below a ~5 ms limiter's ≈240 samples).
- **`host/lib.rs`** — the dead `pub fn engine(&mut self)` bypass removed; `run_script` validates
  the mixer `channels` mount param (positive whole number ≤ `MIXER_CHANNELS_MAX`); `render()`
  bounds the bounce by **bytes** (1 GiB budget) so a malformed `bounce 999999999999` can't OOM;
  `Bounce` routes through the guarded `render()`.

## Alternatives considered

- **`(0,0)` sentinel** for a bad bin — rejected (kimi): it corrupts a min/max fold and
  conflates silence with an error. `Option` is the honest encode.
- **`crossbeam::CachePadded`** for the ring — the portable answer (128-byte lines on Apple
  Silicon), but a dependency; the explicit `align(64)` + compile-time assert suffices for the
  x86 target.
- **Keep the dead `engine()` bypass** — rejected (review): it exposes `&mut Engine` around the
  Host contract; no callers.

## Consequences

- The ring is cache-line isolated (no head/tail bouncing per sample); the isolation is pinned
  compile-time.
- A waveform/zoom query is bounds-safe (no panic); silent bins and errors are distinct.
- A `NaN`/`inf` mount param is refused loud; a fractional/zero/oversized mixer channel count is
  refused; a >1 GiB bounce is refused instead of OOM.
- 5 new tests (ring cache-line isolation + the compile-time assert, peaks bounds-safe,
  bounce-over-budget, fractional/zero/oversized channels). 18 workspace suites green, clippy
  clean.
- **Deferred**: `MAX_PDC` silent-clamp reporting for a >4096-sample-latency node; the
  `arrangement()` poison swallow (fail-safe empty vs a UI error state); range-checking
  mount-only params (euclidean's mount params are absent from `params_table`).
