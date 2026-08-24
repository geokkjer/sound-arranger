# kimi review — review-driven minors batch (2026-08-24)

Session: `session_86a71a67-679d-406e-a7f9-9ca2bdbcac61`

kimi reviewed the batch of small hardening fixes. Verdict: mostly sound, but two needed
correction — the `peaks` `(0,0)` sentinel and the bounce guard constant.

## Must-fix (integrated)

- **`peaks` `(0,0)` sentinel corrupts a min/max fold.** A bad/empty bin returning `(0,0)`
  dragged a range's min/max toward 0 (the fold identity is `(+∞,-∞)`, not `(0,0)`), and
  conflated "bad bin" / "empty range" / "genuine digital silence". **Resolved:** `base_minmax`
  and `range_minmax` now return `Option<(f32,f32)>` — a genuine bin of silence is
  `Some((0,0))`, an out-of-range/empty query is `None`, and the range fold skips `None`.
  Contained to `peaks.rs` (no external callers yet).

## Should-fix (integrated)

- **ring false-sharing layout was wrong.** `Box<[Slot]>` is a *fat* pointer (16 bytes), so the
  `[u8;56]` pad after it put `head` at offset 72, not cache-line-aligned. **Resolved:** atomics
  first (`head`, pad, `tail`, pad, `count`, pad), `slots` last — the pads no longer depend on
  the buffer's size. Added compile-time `offset_of! % 64 == 0` assertions + the runtime test.
  (Apple Silicon's 128-byte lines are a noted portability caveat — `crossbeam::CachePadded`
  would be the portable answer.)
- **bounce guard counted frames, permitting a ~4 GiB allocation.** **Resolved:** byte-based
  (`frames × size_of::<f32>()` vs a 1 GiB budget); `Bounce` now routes through the guarded
  `render()`, and there's a test that a bounce over the budget is refused.
- **mixer channels float-cast shadow state** — `run_script` now validates the mixer
  `channels` param (positive whole number ≤ `MIXER_CHANNELS_MAX`), rejecting `4.5`/`0`/`9`;
  tested.
- **dead `pub fn engine(&mut self)` bypass** removed (no callers).

## Worth-considering / left as documented follow-ups

- **`MAX_PDC` silent clamp** — raised to 4096 (covers real limiters/reverb), but a >4096-sample
  latency node still clamps silently in release (debug_assert only). Reporting the clamp is a
  follow-up.
- **`arrangement()`'s `.ok()` poison swallow** — returns empty on poison rather than
  propagating; fail-safe for a read-only inspection accessor, noted (a UI could show an error
  state instead).
- **Range-checking mount-only params** (euclidean-style nodes with mount params absent from
  `params_table`) remains unimplemented — the finiteness check is the generic half.
- **Double-validation in `ArrangerNode::new`** (validate_clip + the ClipEditor apply path) is
  accepted — once per node construction, and the hand-built `Track` boundary justifies it.

The full critique is the reviewer's response in this session.
