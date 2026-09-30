# Agent Note: A queued onset does not outlive its block

Status: implemented

## Problem

`ToneGen` (`crates/engine/src/graph.rs`) queues each incoming note as an onset and drains it
per sample, so a note is heard at the exact offset its producer gave it. The queue is a fixed ring of
`MAX_PENDING` (8) entries with a `pending_head` / `pending_count` pair, and the two were not
keeping each other honest:

```rust
fn schedule(&mut self, offset: u32, len: u32, freq: f32) {
    if self.pending_count < MAX_PENDING {
        let at = self.pending_head + self.pending_count;   // no wraparound
        self.pending[at] = (offset, len.max(1), freq);
        self.pending_count += 1;
    }
```

```rust
if self.pending_count == 0 {          // the head returns to 0 only on self-emptying
    self.pending_head = 0;
}
```

`head + count` is invariant under drains and grows by one per schedule, so the array was addressed
as if it were linear. It stayed in range only because the head came home at the end of every block,
and that happened only when the queue drained itself — which the per-sample loop does for every
onset whose offset it reaches, i.e. every offset inside the block. A note at `offset >= frames` is
**a legal payload**: `EventBuf` bounds capacity and never offsets, so nothing forbids one, and the
node's own `has_tail` comment already named the case ("a malformed `offset >= frames` could never be
drained by the per-sample loop").

So one such note is queued, is never drained, and parks the head. The sequence that reaches the
array's end: block A schedules a note at offset 0 (drains; head 0 → 1) plus seven notes at
`offset >= frames` (`pending_count = 7`); block A ends with the queue non-empty, so the head stays at
1; block B schedules any note at all, and `at = 1 + 7 = 8` indexes an 8-slot array —
**`index out of bounds`, a panic on the render thread**, mid-block, on a thread nothing catches
(`crates/host/src/live.rs` has no `catch_unwind` around its pump loop).

No shipped plugin can reach it: `EuclideanGen` pushes `frame - block.frame` for
`frame < block.frame + len`, `ScaleGen` passes a trigger's offset through, and a trigger only ever
comes from an in-block push. The invariant is enforced in one producer, relied on in a consumer, and
stated nowhere — which is why every test in the tree (`fan_in_merges_events_sorted` in
`crates/engine/tests/spike_a.rs`, `crates/engine/tests/drain.rs`) keeps its offsets inside the block
and no test could see the defect. It is the shape the
[euclidean step-walk note](./2026-09-29-the-euclidean-step-walk-is-bounded-and-counted.md) found one
node over: a render-thread index resting on an invariant the node does not itself hold.

## Decision

**The queue is a ring at both ends, and nothing in it outlives the block it was queued for.**

- **Both indices wrap.** `schedule` writes at
  `(self.pending_head + self.pending_count) % MAX_PENDING` and the drain advances
  `self.pending_head = (self.pending_head + 1) % MAX_PENDING`, so every access is in range from its
  own arithmetic, whatever the head and count are. This is the sibling of the euclidean node's
  "index by the pattern's own length" rule: a render-thread index must be right by construction, not
  by a discipline another function has to keep.
- **The queue is cleared at the end of every block** — `pending_count = 0; pending_head = 0` — which
  replaces the `count == 0` head reset. Every onset the sample loop drained had an offset inside
  `out`, so whatever is left over was offset at or past the block's last sample and this block could
  never reach it; the note is dropped with the block rather than carried into the next one. The
  clear is what makes the ring's discipline true rather than hoped for: the head is 0 at the start of
  every block, `count < MAX_PENDING` is the only thing the write needs, and no block can inherit a
  note that will not arrive.
- **A malformed offset is dropped, not clamped into the block.** The platform has no cross-block
  note queue — an onset is a position *inside the block it rides on* — so an offset at or past
  `frames` is a producer bug with no honest position to render it at, and clamping would put a blip
  at the block's last sample and call it the note that was asked for.
- **The drop stays silent**, like `EventBuf`'s capacity drop and `MAX_BLIPS`' pool overflow: this
  note fixes the panic, it does not add a counter. The silent-drop debt is already recorded, in the
  [patch-bay note](../../implemented/architecture/2026-08-17-patch-bay-typed-signal-streams.md)'s
  known limitations, and it is owed to every one of those three, not to this one alone.

## Alternatives considered

- **Bound the queue by evicting the *oldest* entry when it is full** (`pending_head += 1;
  pending_count -= 1` before the write), which is the obvious companion to the modulo. Rejected on
  correctness: the drain loop stops at the first head entry whose offset is past the sample it is
  looking at, so an undrained stale entry at the head holds back every onset behind it. Evicting the
  oldest **in the presence of a malformed offset** moves the stale entry's neighbours rather than
  clearing it, and a fresh in-block note can be stranded behind a stale head entry for the rest of
  the block — turning a panic into silence, with the pool full. The documented behaviour stays:
  beyond the pool, the incoming onset is dropped.
- **Clamp `note.offset` to `io.frames`** in `ToneGen::render`. Rejected as a fix on its own: `frames`
  is the first sample *past* the block, so a clamped offset is exactly as undrainable as the one it
  replaced, and the queue still parks the head. The clamp that would work is `frames - 1`, and it
  plays the note at the wrong time on purpose — see the Decision.
- **Validate offsets in `EventBuf::push` / `insert_sorted`**, which is where the platform-wide
  contract would belong. Rejected as unreachable from there: `EventBuf<T, CAP>` is generic over the
  event (`Trigger = u32`, `NoteEvent`) and knows nothing about a block, so "this offset is out of
  range" is not expressible in it. The contract belongs to the **consumer that has the block's
  width**, which is what the clear in `ToneGen::render` is: it needs no knowledge of offsets at all,
  and it is the property a plugin cannot violate — the queue cannot survive the block.
- **Drain the leftovers at the end of the block instead of clearing them** (run the sample loop's
  drain over the remaining entries at the last sample). Rejected: it would play every malformed
  note, all of them at the same sample, at the cost of new logic in the render loop. Dropping is the
  answer the note above already gives to an onset with no position in this block.
- **Grow the pool** so the head cannot reach the array's end. Rejected: the pool is the voice bound
  and voice management stays deferred (RESEARCH §14 risk 8); more slots would change nothing about a
  queue whose entries outlive their block.
- **Make `pending_head` / `pending_count` private accessors or an assert on them.** Rejected as
  ceremony: they already are private to the node, and an assert on a ring's own indices is a panic
  on the render thread for a condition two lines of wrapping already make impossible.

## Consequences

- **`pending[8]` is unreachable.** `schedule` indexes modulo the pool, and the head is 0 with
  `pending_count < MAX_PENDING` at the start of every block, so the write is in range twice over.
- **A malformed note no longer poisons the node.** It occupies the rest of its own block and is
  dropped at the block's edge; the next block starts with an empty queue and an ordinary note sounds
  normally. Before the fix the same payload left the queue full of entries the drain loop could
  never reach.
- **Audible output is unchanged for every offset a well-formed producer emits** — the ring holds
  `MAX_PENDING` entries and the count a block can reach is far below it — and the whole engine suite
  passes: `cargo test -p engine`, 119 tests (46 lib, 71 integration, 2 doctests), 0 failures.
- **Regression test:** `graph::tests::an_onset_past_the_block_does_not_outlive_its_block`, node
  level, beside the euclidean node's render-path tests. It renders one block carrying a note at
  offset 0 plus `MAX_PENDING` notes at `offset == frames`, asserts that block's in-block note still
  sounds, that the queue is empty afterwards, and that the **next** block's ordinary note sounds too.
  On the unfixed code it fails twice over: the queue reads `(1, 7)` instead of `(0, 0)`, and with
  that assertion skipped the next block panics at
  `index out of bounds: the len is 8 but the index is 8` — the render-thread panic, reproduced.
- **No new surface, and the render path still allocates nothing**: two integers and two field
  writes per block, no counters and no locks. `MAX_PENDING` is still private and still 8.
- **Left, deliberately:** the dropped-onset count. This fix makes the drop *bounded and per-block*
  rather than reported; `euclidean.drops`, `clock_out.overflows` and `DrainOutcome::capped` all
  report their bound because a session has to be able to interpret it, and the same argument applies
  here — one counter, in the same shape, when the silent-drop debt is paid across all three at once
  rather than for this node alone.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
