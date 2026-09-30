# Lesson 12 — Bounded walks and loud caps

**Material:** the euclidean walk in
[`crates/engine/src/graph.rs`](../../crates/engine/src/graph.rs)
(`EuclideanGen`, ~lines 617–683), the tick walk in
[`crates/engine/src/plugins/clock_out.rs`](../../crates/engine/src/plugins/clock_out.rs)
(`TICK_WALK_CAP`, `due_ticks_from`), and the drain in
[`crates/engine/src/render.rs`](../../crates/engine/src/render.rs)
(`MAX_DRAIN_FRAMES`, `DrainOutcome`).

Prerequisites: Lessons 6 (the no-allocation rule) and 11 (total functions,
saturating casts). This lesson is the third rule of the render path, and the
one that saved the project from a hang.

## 12.1 The bug that wrote this lesson

In September 2026 an external review found that `set_tempo(1e-15, 4)` — a legal
script line, accepted by validation — made the MIDI clock-out plugin's tick
walk loop essentially forever on the render thread. Not a wrong answer: *no
answer*, ever, while holding the audio path. Reproduced under `timeout`:
exit 124, zero test results. The euclidean grid walk had the same property at
absurd tempos, and a `take` declaration could ask for a multi-gigabyte buffer
from a 30-byte line.

The rule that came out of it:

> **No unbounded loop on the render thread.** Whatever a bound clips is
> **counted, never hidden.**

The pattern appears four times in the engine, and each appearance pairs a cap
with a counter. Learn the pattern once here, then recognize it on sight.

## 12.2 The euclidean walk: a closed-form count makes the cap honest

Read `EuclideanGen`'s render in `graph.rs` slowly. The block must decide which
pattern steps fall inside it. At a sane tempo that's a handful of steps; at a
tempo like 1e9 bpm it's millions — and nothing in the *music* says that can't
happen, because the tempo came from a logged `set_tempo` that validated its
*range* but not its *walk length*. The walk is bounded two ways:

```rust
let grid = s1.saturating_sub(s0).max(0) as u64;   // the whole grid, closed form
let stride = self.pattern.len() as u64;
let mut step = s0;
let mut walked = 0u64;
while step < s1 {
    // … emit the trigger if the pattern position is true and in-block …
    walked += 1;
    if walked >= EUCLIDEAN_STEP_CAP || out_triggers.is_full() {
        break;
    }
    step = step.saturating_add(1);
}
let skipped = grid.saturating_sub(walked);
if skipped > 0 {
    self.drops.fetch_add(skipped, Ordering::Relaxed);
}
```

Four things to notice:

1. **`EUCLIDEAN_STEP_CAP` is `BLOCK * 8`** — the finest grid a block can
   *audibly* distinguish (a sixteenth note every 0.125 ms at the fastest
   walking tempo, per the constant's doc). The cap isn't arbitrary; it's the
   point past which walking more changes nothing you can hear.
2. **`grid` is computed in closed form** — how many steps the walk *would*
   visit, known without visiting them (Lesson 11's saturating casts make that
   count honest at absurd tempos). This is what makes the counter *exact*: the
   block can say "I skipped 4,294,967,295 steps" truthfully because it never
   had to take them to know.
3. **`out_triggers.is_full()` is the cheaper stop** — a push into the full
   buffer would be refused anyway (Lesson 6), so stopping there changes
   nothing, and "this node is never the source of a silently refused push" is
   written as a decision.
4. **`drops` is the loud part** — an `Arc<AtomicU64>` published as the
   `euclidean.drops` service (Lesson 4's second contribution), read by tests
   and any host that asks. The doc says it exactly: *"a loud bound, never a
   silent truncation."*

## 12.3 The tick walk: when the count itself needs an algorithm

The clock-out walk had a harder problem. To know which MIDI ticks are due in a
block it must count the due ticks from a start index — and counting them by
*visiting* them is exactly the unbounded walk again, wearing a different hat.
Read `due_ticks_from` in `clock_out.rs`:

```rust
// Bracket by doubling from `from` (already known due) up to a tick at or
// past `block_end`. The doubling cannot run away: the last step pins `hi`
// at `u64::MAX`, which the check above has just established is *not*
// due, so the bracket is real and the search converges on the first
// index past the block.
let mut lo = from;
let mut hi = from.saturating_add(1);
while Self::tick_frame(map, hi) < block_end {
    lo = hi;
    if hi >= u64::MAX / 2 {
        hi = u64::MAX;
        break;
    }
    hi *= 2;
}
while hi - lo > 1 {
    let mid = lo + (hi - lo) / 2;
    if Self::tick_frame(map, mid) < block_end {
        lo = mid;
    } else {
        hi = mid;
    }
}
```

`tick_frame` (a tempo-map lookup) is **monotone in the tick index** — ticks
never come before earlier ticks. Monotone + "find the first index past the
block" = **binary search**, and binary search over `u64` is ~64 lookups *no
matter what the tempo is*. The doubling bracket finds the search window without
assuming anything about its size.

And read the comment *above* the function — it confesses that an earlier
version of the comment itself was wrong (a saturated corner case understated
the count by one until someone proved it). Even the prose around the code is
held to the standard the code is.

## 12.4 The drain: a bound with a *flag*, not a counter

`Engine::drain` renders the tail after the arrangement ends — but a ringing
feedback structure (if one ever ships) never ends, so:

```rust
let max_frames = max_frames.min(MAX_DRAIN_FRAMES);   // 48_000 * 60 — one minute
…
if frames_done >= max_frames {
    capped = true;
    break;
}
```

`MAX_DRAIN_FRAMES` is one minute of audio — generous for any tail a mix has,
fatal for an infinite one. The outcome isn't a counter this time but a
**flag**: `DrainOutcome { tail_frames, capped }`, and the host's own summary
prints `drain tail frames: N (CAPPED)` — you saw the uncapped shape in
[FIRST_SESSION](../FIRST_SESSION.md). Same rule, different reporting shape:
when the honest answer is one bit ("the drain stopped early"), one bit is
enough.

## 12.5 The fourth cap you already know

`MAX_APPLY_FAULTS` (Lesson 10) is the same pattern applied to *storage*: faults
past the bound are counted in `apply_faults_dropped`, never silently
disappeared. Four sites, one shape:

| Site | The bound | The loud part |
|---|---|---|
| euclidean walk | `EUCLIDEAN_STEP_CAP` = `BLOCK * 8` | `euclidean.drops` counter (exact, closed-form) |
| tick walk | `TICK_WALK_CAP` (the correction walk, both directions) | due-tick count by binary search; the overflow counter stays *exact* |
| drain | `MAX_DRAIN_FRAMES` = 1 minute | `DrainOutcome::capped` flag |
| fault list | `MAX_APPLY_FAULTS` = 64 | `apply_faults_dropped` counter |

**A cap without its counter is a silent truncation.** If you take one rule
from this lesson, take that sentence — it's the difference between "bounded"
and "quietly wrong within budget."

## Your turn

⭐ **1.** Read the two tempo-bound tests and run them:

```sh
cargo test -p engine euclidean     # the bounded-walk and drops-counter tests
cargo test -p engine clock_out     # the tick-walk tests, incl. the absurd-tempo ones
```

Find the test that feeds an absurd tempo and asserts on the *counter*, not the
audio. That asymmetry — audio unchanged, counter nonzero — is the whole design.

⭐ **2.** `TICK_WALK_CAP` is `1 << 16`, and it bounds the **noise-correction**
walk in `first_tick_at_or_after` — in *both* directions — not the due-count.
Read its doc comment: the correction undoes f64 rounding and is one or two
ticks at any tempo where a tick is worth a frame, so why does the bound exist
at all, and what property of a "tempo whose ticks are closer together than
frames" makes a generous cap the only exit?

🔧 **3.** `due_ticks_from` special-cases "every index the map can name is due"
*before* building the bracket (returning
`u64::MAX.saturating_sub(from).saturating_add(1)`). Write the test that forces
that case — the doc says a tempo fast enough that even `u64::MAX` rounds inside
the block — and check it in.

🔧 **4.** Pick any loop on a render path in the repo (`rg "while|for" crates/engine/src -n`)
and audit it against this lesson: what bounds it? What is counted or flagged
when the bound bites? If the answer is "nothing bounds it" or "nothing is
reported", you've found either a bug or a lesson-12 exercise worth filing.

## Checkpoints

1. Why is `EUCLIDEAN_STEP_CAP` exactly `BLOCK * 8`, and where is that justified?
2. Why does the euclidean counter need `grid` in closed form — what would the
   counter mean without it?
3. What property of `tick_frame` licenses the binary search in
   `due_ticks_from`?
4. Why does the drain use a flag where the walks use counters?

*(Answers: 1 — it is the finest grid a block can audibly distinguish (a
sixteenth every 0.125 ms); walking finer changes nothing you can hear, so the
cap costs no music — the constant's doc does the arithmetic. 2 — the count of
skipped steps must be *exact* to be honest ("4,294,967,295 skipped" is a fact;
"many skipped" is a shrug), and only the closed form knows the total without
walking it. 3 — monotonicity: ticks never land before earlier ticks, so "first
index past the block" is a sorted boundary, which is exactly what binary search
finds in ~64 fixed lookups. 4 — the drain's honest answer is one bit — "the
tail stopped early" — and `capped` carries it with the length it did reach;
there is no meaningful "how much more" for a structure that rings forever.)*

Next: [back to the course README](README.md) — then re-read
[the architecture explainer](../architecture-explainer.md) §9–10 and watch the
caps and counters appear in the decisions' cons columns.
