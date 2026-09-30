# Agent Note: A chop is bounded before it is tiled

Status: implemented

## Problem

`ArrangeOp::ChopClip` had two guards on `times` — `times >= 1` and `times <= src_len` — and
`src_len` reaches `i64::MAX`. `times` is a `u32` straight off the wire
(`arrange chop t0 c0 4294967295 pre @0`, parsed with only a `u32` fit check in
`crates/host/src/lib.rs`), so **one logged line could ask for up to 4.29 billion pieces**.

What that line then did, in `crates/media/src/timeline.rs`:

- The piece loop ran `times` times, and **each iteration called `clip_id_exists`**, which scans
  every clip on every track: `times` × clips string comparisons. It hung long before anything
  was allocated.
- If it ever finished, `pieces` held `times` `Clip`s each carrying **three heap `String`s** —
  `id`, plus `name: c.name.clone()` and `source: c.source.clone()` — roughly 250 bytes and
  three allocations per piece, inside a `Timeline` that `apply` had already **cloned**, followed
  by a `times`-element `sort_by_key`. Nothing in the value model reserves (`try_reserve` is
  absent throughout; the host is careful about it in `stretch` and `Pool::write_source`), so the
  end state was an allocation abort.

Nothing about it needed a large arrangement to reach: one clip of ordinary length with a long
`src_len`, and one mistyped number. The op's own grain is a slice per bar or finer, so a count
in the millions is a typo — but the model tried to honour it.

## Decision

**A chop refuses `times` above `MAX_CHOP_SLICES` (4096), naming the bound, and the derived-id
check is a hash lookup instead of a scan.**

- `MAX_CHOP_SLICES: Frame = 4096` in `crates/media/src/timeline.rs`, a module-private constant
  beside `Track`. It is a **refusal, not a cap**: the op does not silently slice fewer times
  than asked (that would produce a value the log cannot reconstruct — the op carries the count,
  not the pieces), it declines and says why (`"chop 4097 times exceeds the 4096-slice bound"`).
  The bound is **inclusive** — `times == 4096` applies — and it sits beside the `times <= src_len`
  check so the accepted range reads `1 <= times <= min(src_len, MAX_CHOP_SLICES)`, which is now
  written on the op's own `times` field.
- The piece loop gathers the arrangement's clip ids into one `HashSet<Id>` **before** it walks
  and tests membership per piece, so the duplicate check is O(1) per piece instead of O(clips).
  The set is built from **every track**, because clip ids are unique across the whole value
  (`Timeline::clip` resolves an id without naming a track, and `Duplicate` checks every track) —
  gathering only the chopped track's ids would have let two tracks hold the same id.

The same set absorbs the loop's own ids as it inserts them, so it replaces the old `seen` set
rather than adding to it. That `seen` set could never fire anyway: the derived id is
`format!("{prefix}.{i}")` over distinct `i`, so the ids are distinct by construction and the only
real test was the scan. Same allocation count as before, one pass instead of `times` passes.

The bound lives at the **op**, not in `parse_script`, for the reason the
[take-declaration note](2026-09-29-a-take-declaration-is-bounded-before-it-is-sized.md) gives: the
value arm is the one validation point every wire, session-document, journal-replay and
Rust-embedder path shares, and a parse-side bound would only turn the same refusal into an
earlier one for the wire while leaving an in-memory `execute` unbounded.

## Alternatives considered

- **Bound the count by a "reasonable" grain instead (one slice per bar, from the session's
  tempo).** Rejected: the bound would depend on state the op does not carry, so the same op text
  would be legal in one session and refused in another, and replay could diverge from the original
  apply if the tempo changed between them. A constant is the same rule everywhere.
- **Clamp `times` to the bound** and chop into 4096 anyway. Rejected for the reason above: it
  rewrites a value the user typed into a value the log does not describe, and the log is the
  reconstruction of the arrangement.
- **`try_reserve` the piece vector and let an absurd `times` fail as `Err`.** Rejected as the
  primary fix: it bounds the *allocation* and not the `times` × clips scan in front of it, and an
  error message about a failed reservation says less than one naming the slice bound. Worth doing
  on its own merits for the value model's other growable vectors, but it is a different claim.
- **A `HashSet` of the chopped track's ids** (what the review suggested). Rejected: it makes the
  duplicate check cheap by weakening it. Ids are unique per *arrangement*, not per track, so a
  chop whose `pre.3` already sat on another track would have been accepted. The set here is built
  from every track; `a_chop_is_refused_a_derived_id_taken_on_another_track` pins that.
- **Refuse any `times` that leaves a piece shorter than a frame, or a single-frame piece.**
  Rejected: one-frame slices are what a fine chop legitimately produces (`times == src_len`), and
  `chop_refuses_bad_inputs_and_a_looped_clip` already pins `times == src_len + 1` as the refusal
  for "more slices than frames".
- **A bound on the number of ops in a script or log.** Rejected as out of scope: it is a different
  claim against a different structure, and it would not have stopped the in-memory `apply`.

## Consequences

- `arrange chop t0 c0 4294967295 pre @0` is now a named `Err` before a single piece is built, and
  the process stays up. Every chop the platform or a hand legitimately asks for still applies: the
  bound is far past the op's grain, and nothing in the repo, the text format or the shells chops
  anywhere near it.
- The chop path is linear in `times` and in the arrangement's clip count, so a legal maximum chop
  (4096 pieces on a large arrangement) is bounded work rather than a scan per piece.
- Nothing outside `apply_mut` changed: `clip_id_exists` still serves `Duplicate`, and the wire
  format, the codec round-trip and the host text format are untouched.
- Two regression tests in `crates/media/src/timeline.rs`, beside the other chop tests:
  `a_chop_past_the_slice_bound_is_refused_not_sized` refuses `MAX_CHOP_SLICES + 1` (naming the
  bound), refuses the reported wire value `u32::MAX` on a clip long enough to satisfy
  `times <= src_len`, and asserts a chop *at* the bound still produces every slice and leaves every
  piece valid. It fails on the unfixed value, where the first assertion finds the op **applied**
  with 4097 pieces — a fast, clean failure rather than the hang the wire case would be.
  `a_chop_is_refused_a_derived_id_taken_on_another_track` pins the cross-track uniqueness the hash
  set preserves.
- The [chop-op note](../feature/2026-09-05-chop-clip-op.md) owns the refusal list and is updated
  with the bound beside it.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
