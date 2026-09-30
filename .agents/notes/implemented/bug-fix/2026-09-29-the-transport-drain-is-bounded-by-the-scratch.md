# Agent Note: The transport drain is bounded by the scratch

Status: implemented

## Problem

The [clock-out note](../../implemented/architecture/2026-09-29-midi-clock-out-in-the-engine.md) gave the
node two per-block buffers, both `Vec::with_capacity(CLOCK_OUT_CAP)`, and bounded **one** of them. The
event scratch had a guard — `ClockOutNode::push` refuses past `scratch.capacity()` and counts the
refusal in `overflows` — and the transport scratch had this:

```rust
let at = queue.partition_point(|&(f, _)| f < block_end);
out.extend(queue.drain(..at));
```

`out` is `transport_scratch`, so a block owing more than 64 transport commands reallocated **inside
`ClockOutNode::render`** — `Vec::extend` over a `Drain` (a `TrustedLen` iterator) reserves, and 65
items into a 64-capacity buffer is a reallocation. The function's own doc claimed the opposite ("The
node reuses a preallocated buffer, so draining allocates nothing"), and the tick path two dozen lines
below had already been through this argument once, in the
[tick-walk note](./2026-09-29-the-tick-walk-is-bounded-not-just-the-scratch.md): the bound has to be
on the thing that moves, not only on the buffer it moves into.

Nothing bounds the other end either. The host feeds the log from its **apply** path
(`TransportPlay` / `TransportStop` → `TransportLog::push`, `crates/host/src/lib.rs`) and nothing
renders between the commands: a script of seventy `transport play` / `transport stop` lines and a
`bounce` (each `at_frame() == None`, so `process` pre-renders nothing), or any number of commands sent
to a live host while it is stopped — the actor renders only `if session.is_playing()`
(`crates/host/src/live.rs`), so the log simply accumulates.

The counting-allocator proof could not see it: `render_path_does_not_allocate`
(`crates/engine/tests/spike_a.rs`) mounts `clock_out` with **neither** a sink nor a transport log, so
`take_due` is never entered, and the module's transport test queued four entries.

**And it lost commands.** The unfixed drain removed *every* due entry from the log, then handed them
to `push`, which refuses past the event scratch's capacity. A block owing 129 commands drained all
129, sent 64, and dropped the other 65 — counted as `overflows`, but counted as *transport*, with no
way to tell a dropped tick from a dropped `Stop`. This is the log's own contract broken from the
inside: "a late feed flushes into the next block rather than being lost".

## Decision

**The drain takes what the caller's buffer has room for, and the rest stays queued** — the
transport scratch is bounded the way the event scratch is, and bounded *without* inventing a second
drop.

- **`TransportLog::take_due` bounds the drain by the room in `out`**, so the node's preallocated buffer
  is never grown and the render path allocates nothing however many commands are pending. The bound is
  read from the buffer's own capacity rather than passed as a new parameter: it is the same rule
  `push` already follows (`len < capacity`), it needs no call-site change, and it cannot be set
  inconsistently with the buffer it protects. A buffer with **no** capacity takes the whole due
  prefix — an uncapped caller (nothing on the render path today) keeps the old behaviour rather than
  silently never draining.
- **What does not fit stays in the log**, and the next block's drain carries it at that block's first
  frame. This is the point of the fix: a transport command that arrives late is a command that
  arrives, and one that is dropped is a follower left running after the operator pressed stop. A
  `Start` flushed two blocks late is a start; a dropped `Start` is a session that never begins.
  Nothing is lost, so nothing new is counted, so `overflows` keeps its one meaning — *events that did
  not fit the event scratch* — and a transport flood does not inflate it.
- **The event scratch is still the send bound, and it still speaks for itself.** A block whose scratch
  is entirely transport drops its own tick and counts it, which the regression test asserts rather than
  hides. Two bounds, two different jobs: the transport scratch bounds *how much is drained* (a
  staging bound, nothing is lost), the event scratch bounds *what is sent* (a send bound, counted and
  loud).
- **The counting-allocator proof carries a transport log now.** `render_path_does_not_allocate`
  provides one under `TRANSPORT_KEY` and feeds 200 commands after the prime render, so the measured
  region actually enters `take_due`. On the unfixed code it reports `1` allocation; on the fix, `0`.

## Alternatives considered

- **Count the remainder in `overflows`, as the tick path counts its skipped ticks.** Rejected: it drops
  transport commands to keep a counter that a `Stop` is not a tick for. It is also mechanically
  awkward — `TransportLog` holds no counter, so the count would have to be returned and `fetch_add`ed
  by the node, or the log given an `Arc<AtomicU64>` of its own. The log's contract is that a command
  flushes late, not that it is discarded, and the fix costs one `min`.
- **Take a `cap: usize` parameter** (the review's other suggestion). Rejected on cost, not on
  correctness: it is a source-breaking change to a public method for one caller, and it introduces a
  second number that can disagree with the buffer it is supposed to protect — a caller passing
  `cap = 1000` into a 64-capacity buffer reintroduces exactly the allocation being fixed. Reading the
  capacity cannot drift from the buffer.
- **Bound the log at `push` instead** (refuse the 65th command at the door, where the host feeds it).
  Rejected as the fix: the door is the host's apply path, `push` is a public method, and a bound there
  would still leave the drain unbounded for a caller that got in earlier. It is a reasonable
  *complement* — an unbounded log is the host's own memory growth — but it is a different decision
  about the host, not this one.
- **Grow the transport scratch on purpose, at construction, to some larger multiple.** Rejected: it
  does not remove the bound, it moves the number past which the render path allocates, and the number
  is a guess about a host that is not written yet.
- **Emit transport outside the event scratch** (send `Start`/`Stop` in their own `send`, so a flood of
  transport never displaces a tick). Tempting — the two do not compete for meaning — and rejected as
  out of scope: it changes the wire shape (two sends per block, and their order), touches the sink
  contract, and belongs with slice B's real device sink rather than with a review finding.
- **Update the existing `transport_is_emitted_at_logged_frames` to cover the flood.** Not needed, and
  the new test is beside it for the same reason the tick note's fix needed no rewrite of
  `overflow_is_counted_never_grown`: the late-flush behaviour that test pins is unchanged — a command
  that does not fit now flushes in the *next* block rather than the same one, which is the same
  guarantee the test already asserts for a command logged after its frame.

## Consequences

- `crates/engine/src/plugins/clock_out.rs`: `TransportLog::take_due` reads the due count, clamps it
  to the caller's room, and drains that much. `ClockOutNode::transport_scratch` is still
  `with_capacity(CLOCK_OUT_CAP)`, is still `clear`ed each block, and is now documented as a buffer
  the render path never grows.
- Tests: `plugins::clock_out::tests::a_transport_flood_is_bounded_and_nothing_is_lost` (module, beside
  `transport_is_emitted_at_logged_frames`) queues `2 × CLOCK_OUT_CAP + 1` commands at one frame and
  asserts all 129 reach the wire — the first cap at the logged frame, the rest at the first frame of
  the block that drained them — that `transport_scratch.capacity()` is still `CLOCK_OUT_CAP`, and that
  emitted ticks plus `overflows` still account for every tick due.
  `spike_a::render_path_does_not_allocate` is extended, not rewritten: it now provides a transport log
  and feeds 200 commands. Both fail on the unfixed code — the module test on the count (64 of 129
  sent) and on the capacity (129, not 64), the counting test with `1` allocation.
- A host that floods the tap while stopped now gets its commands over the next few blocks rather than
  losing 65 of them silently, and gets them at the frame they were logged for wherever that has
  passed. At any sane rate — a person pressing play and stop — nothing about the emitted stream
  changes; `transport_is_emitted_at_logged_frames` is unchanged and still passes.
- **Left, deliberately:** `TransportLog`'s queue is still unbounded, and it is the host that fills it.
  Bounding the feed belongs to the host's command path (a `Stop` a user pressed twice is two entries
  today, and a thousand pressed while stopped is a thousand), and that is a separate decision about
  the host rather than a render-path bound.
- **Left, deliberately:** the transport scratch and the event scratch are both `CLOCK_OUT_CAP`, and
  nothing states that a block's transport can therefore displace its ticks. That is the
  `[clock-out note](../../implemented/architecture/2026-09-29-midi-clock-out-in-the-engine.md)`'s
  "one send per block" shape, and it is now pinned by a test rather than changed here: separating the
  two budgets is a wire-shape decision (see the rejected alternative above).

*Authored with Space Bunny · OpenCode, 2026-09-29.*
