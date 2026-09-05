# Agent Note: ChopClip — auto-slice a clip into N contiguous pieces

Status: implemented

## Problem

The ACID editing vocabulary had razor-split (cut one clip at a point) and
duplicate, but no **chop** — splitting a whole clip into a run of sub-clips in one
gesture (the MPC/ACID "slice a sample into pads" workflow, and one of the verbs the
resumed scope named). A value-level op is the right shape: nothing in the
arrangement is "the audio", so chopping is a pure transform of the `Timeline`
value, logged and byte-identically replayable like the other ops.

## Decision

Add `ArrangeOp::ChopClip { track, clip, times, prefix }` to the arrangement value
(`crates/media/src/timeline.rs`):

- It splits the clip's source region into `times` **contiguous** pieces (equal
  within one frame — `base = src_len / times`, remaining frames spread over the
  first pieces), replacing the original. Each piece reuses the parent's `source`,
  `gain`; resets `fade_in`/`fade_out` to 0 and `loop_len` to `None` (a slice is a
  plain region).
- The sub-clip ids are **derived deterministically** as `format!("{prefix}.{i}")`,
  so replay reproduces the same value with no randomness. `prefix` is carried in
  the op (and interned into the engine log), matching the value model's
  "ids are logged and deterministic" rule.
- **Fail-loud refusals** (never logged): `times == 0`, `times > src_len`, a
  derived id that already exists or repeats, and chopping a **looped** clip (the
  loop phase at a cut is not representable — the same rule as razor-split).
- The op is wired end-to-end through the closed-codec path (the clip-arranger's
  "everything is a logged command" discipline): `encode_op`/`decode_op`
  (`crates/media/src/clip_editor.rs`, added to `ALL_OPS`) and the versioned text
  format (`arrange chop t0 c0 4 pre @0`, in `crates/host/src/lib.rs`).

## Consequences

- `chop` is a genuine, complete ACID editing verb at the value layer: a UI (or the
  CLI) can slice one clip into N pieces that tile the original span and render.
- The value stays a pure reconstruction of the logged op stream — chopping and
  replaying reproduces the identical `Timeline` (and thus identical audio).
- Tests: two `timeline` value tests (contiguous coverage + refusals) and a host
  text-format test that chops via `arrange chop` and bounces audio. `cargo test
  --workspace` is green.
- Follow-ups (deferred): slice alignment to the **tempo grid / beats** and
  **transient** chop are the next ACID-substep; this op is the fixed-count,
  frame-aligned primitive they build on. It does not yet time-stretch (a later
  StretchClip/beat-match step).

## Alternatives considered

- **Carry `new_ids: Vec<Id>` in the op** (matching RazorSplit's explicit
  `new_left`/`new_right`): would need a new engine `Value` variant (Value has only
  Str/U64/I64/U32/F32 — no list), adding a wire-schema change. Rejected; a carried
  `prefix` + derived ids needs no new Value type and stays deterministic.
- **Chop as a sequence of RazorSplits:** the caller would issue N-1 splits and
  compute all the ids/frames by hand, pushing id dedup and tiling into the UI.
  Rejected — one valueop with validation is simpler and encodes intent.
- **Keep the original clip alongside the pieces:** complicates the value (a
  "chop" that leaves the parent is ambiguous) and the UI semantics. Rejected; a
  chop replaces the clip, matching how a slice-take is then arranged.

*Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-05.*
