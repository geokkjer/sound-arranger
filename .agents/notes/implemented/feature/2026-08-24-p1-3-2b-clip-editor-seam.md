# Agent Note: P1.3.2b — the clip editor's engine seam (logged-command codec)

Status: implemented

## Problem

P1.3.2a gave the engine a closed-core message vessel (`Event::Arrangement` + dispatch). The
clip editor's ACID ops still had to become actual **logged commands**: an `ArrangeOp` must be
encoded to `engine.arrange`, applied by a handler, and reconstructed byte-identically from
the log on replay. There was no `ArrangeOp ↔ engine-command` codec and no way to prove the
value is a pure function of the op stream.

## Decision

`crates/media/src/clip_editor.rs`:

- **`Interner`** (encode-side): maps ids (`String`) to a stable`&'static str` for the log
  (`Box::leak` — spike scale). Decode reads `&'static str` back via `to_owned`, so it never
  needs the interner and is content-deterministic.
- **`encode_op(op) -> (op_name, fields)`** and **`decode_op(op_name, fields) -> ArrangeOp`**:
  lossless for every variant, strict on missing/wrong-typed fields, and `loop_len` encodes as
  `U64(0)`=None (collision-free given `loop_len > 0`; a direct `Some(0)` encode is guarded).
- **`register_handlers`** installs one handler per op name; each decodes the fields and
  applies to the shared `Timeline` (`*tl = tl.apply(&op)?` — validate-first, so an `Err`
  leaves the value untouched). The engine logs + dispatches on the control side.
- **`ClipEditor`** owns the interner + the shared `Timeline` (private `Arc<Mutex<>>`).
  `apply` **dry-runs** the op against the current value (fail-loud, a refused op is never
  logged), encodes, `engine.arrange`s, and **flushes immediately** so the value is current
  for the next dry-run. `snapshot()` gives a read-only window (no write handle around the
  log).

The reconstruction is a pure function of the op **order** (the handler ignores `at_frame`;
the scheduler preserves same-frame insertion order). This is the **log-visibility carve-out**:
the arrangement node's state *is* the logged value.

## Alternatives considered

- **Host flushes between ops (unenforced contract)** — the first draft; rejected after the
  kimi 3b review: a stale-timeline dry-run could wrongly accept a conflicting op that the
  handler then refused at flush (a "refused op in the log"). `apply` flushes instead.
- **Expose the `Arc<Mutex<Timeline>>` to callers** — rejected (kimi M1): a write handle
  around the log lets out-of-band mutation break the pure-reconstruction claim. `snapshot()`
  is read-only.
- **`v_u64` coercing `U32`/`I64`** — rejected (kimi S1): inconsistent with the strict
  decoder; now `U64` only.
- **A per-op `Event` variant in the engine** — rejected in P1.3.2a (core stays closed).

## Consequences

- The ACID ops are now logged commands: encode → `engine.arrange` (with `at_frame`) →
  handler applies to the value; replay reconstructs the identical `Timeline` (the
  `logged_ops_reconstruct_the_timeline_on_replay` test).
- A refused op (invalid semantics) is never logged: the `apply` dry-run catches it before
  `engine.arrange`.
- 3 unit tests (all-op round-trip incl. name ∈ `ALL_OPS`, loop_len 0=None, reject-wrong-type)
  + 2 integration tests (replay reconstruction, refused-never-logged). 15 workspace suites
  green, clippy clean.
- **Deferred**: the host wires `ArrangerNode`s into the graph from the reconstructed value
  (an op handler cannot reach `&mut Engine` — type-enforced, so the host owns it in
  P1.3.4), and reconciling readers as the value changes.
