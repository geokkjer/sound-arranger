# kimi review — P1.3.2b clip-editor engine seam (slice 3b, 2026-08-24)

Session: `session_7ab4be6a-c7f4-42b7-a0ca-6077b51e8233`

kimi reviewed the `ArrangeOp ↔ engine.arrange` codec + `ClipEditor`. Verdict: the codec is
solid; the seam's weak points are an unenforced flush discipline and a mutable escape hatch
on the shared value.

## Must-fix (both integrated)

1. **`ClipEditor::timeline()` handed out `Arc<Mutex<Timeline>>` — a write path around the
   log.** Any holder could out-of-band-mutate the value, breaking "the value is a pure
   reconstruction of the logged op stream" and silently invalidating the dry-run premise.
   **Resolved:** `timeline()` removed; a **snapshot()** (read-only) is exposed instead. The
   value is only ever mutated by the op handlers.
2. **The "host flushes between ops" contract was unenforced — a stale-timeline dry-run could
   wrongly accept a duplicate/conflicting op**, log it, then have the handler refuse it at
   flush (a debug panic, and in release a "refused op" left in the log). **Resolved:**
   `ClipEditor::apply` now validates, logs, and **flushes immediately** — so the value is
   current for the next dry-run; no stale-state window exists. A refused op is never logged.

## Should-fix (integrated)

- **`v_u64` leniency** — it coerced `U32`/`I64` while `Trim.by`/`LoopRegion.times` match
  their exact variants. Now strict (`U64` only), consistent with the rest of the decoder.
- **`encode_op` loop_len `Some(0)` collapse** — a `pub` encode of a hand-built clip with
  `loop_len = Some(0)` would silently decode as `None`. Guarded with a `debug_assert!`
  (the invariant is caller-enforced).

## Worth-considering (integrated as tests/notes)

- **`ALL_OPS` can drift from variants** — closed by the round-trip test asserting every
  encoded op name ∈ `ALL_OPS` (encode is exhaustive, but the *registration* list wasn't).
- **Unknown extra fields silently ignored** — documented as deliberate forward-compat on
  `decode_op`; a newer op version with an added field decodes on an old decoder, dropping it.
- **f32 "bit-exact in the log" is currently in-memory** (`Value::F32` passthrough); needs
  `to_bits` only when a serialized log lands. Noted.

## The answers we leaned on

- Codec is lossless: every field of every variant round-trips (tested for all 12); the
  `U64(0)`=None loop_len encoding is collision-free given the `Some(0)` invariant.
- Reconstruction is a pure function of the op **order** (the handler ignores `at_frame`;
  the scheduler preserves same-frame insertion order) — verified.
- The responsibility split is sound and type-enforced: `OpHandler` cannot reach
  `&mut Engine`, so graph-node mounting in P1.3.4 by the host is the only option.
- The value is clock-independent and the interner is content-deterministic (decode reads
  content, not pointer identity), so two sessions' logs compare equal.

The full critique text is the reviewer's response in this session.
