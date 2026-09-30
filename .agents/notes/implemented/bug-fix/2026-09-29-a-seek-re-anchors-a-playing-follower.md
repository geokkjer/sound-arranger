# Agent Note: A seek re-anchors a playing follower

Status: implemented

## Problem

The [host clock-out note](../../implemented/architecture/2026-09-29-midi-clock-out-in-the-host.md)
decided two things: **nothing is sent across a rebuild**, and **the next `play` re-syncs**. The second
was a claim about a message the host never sends on a seek's account, and it was load-bearing: it was
the only thing standing between a mid-playback seek and a permanently out-of-phase follower.

`Play` and `Stop` are **actions**, not state — `HostCommand::is_state` (`crates/host/src/lib.rs`)
deliberately omits them, because they are not a document a replay could reconstruct; so they never
enter `history`, and `rebuild` never re-applies them. Meanwhile every rebuilt session gets a
**brand-new** tap: `new_at_with` constructs a fresh `TransportLog` and provides it under
`"transport"`, and `carry_over` carries the slot, the port, the redo stack and `playing` — not the
log. So the sequence was: mount `clock_out`, play, render a second, seek to half a second in **while
playing**.

Nothing arrives. `rebuild` sets `rebuilt.playing = self.playing`, so the pump keeps rendering and the
node keeps emitting ticks — from the *new* position, computed statelessly from each block's own frame
(that part is right and is the engine half's whole design). But the follower is never told: it
received ticks 0…47 and now receives tick 24 onward, so it is counting 24 pulses where the session has
had 48 — half a second out of phase at 120 bpm, since 24 PPQN makes 24 pulses one beat. And nothing
later announces the jump: the transport is still "playing", so no `play` command ever follows, and the
note's "the next `play` re-syncs" had no next `play` to point at. A user stopping and re-starting
would re-anchor by hand; the host never did it on its own.

Whether the follower is *phase-corrected* by the message depends on how it reads it, and this note
does not claim more than the wire allows (see Consequences). What was unambiguously broken is that
the host never said anything at all.

The shipped test could not see it. `a_seek_sends_nothing_while_it_rebuilds_and_resumes_afterwards`
asserts `transport_events(&sends) == vec![(0, "Start")]` immediately after the seek — true, and true
for a second reason it did not know about — and then checks only the tick **frames** after the
post-seek render. It pinned the absence, so it could not fail.

The note's other rationale was false in the same way: "feeding it in apply is what makes a replay
re-feed the tap identically". No replay ever re-fed the tap. `grep` for `transport.push` finds two
sites, both inside the `TransportPlay`/`TransportStop` arms of `apply`, and neither is reachable from
a rebuild.

## Decision

**The rebuild's far side is the re-sync point.** `replay_to_kind` feeds the **rebuilt** session's own
log at the target frame when — and only when — the rebuild is a seek and the transport was playing:

```rust
if is_seek && self.playing {
    let transport = if frame == 0 {
        engine::Transport::Start
    } else {
        engine::Transport::Continue
    };
    rebuilt.transport.push(frame, transport);
}
```

The entry is queued, not sent: it goes out with the **first render after the adoption**, from inside
the session, at the target frame — by then the device is back in the shared slot, so
"nothing is sent across a rebuild" is untouched. The rebuilt node holds the same `Arc`, which is what
makes the feed reach the wire at all.

**The message is the one a `play` at that frame would send, and that is not incidental.** An earlier
draft of this note fed a `Start` at every seek target, on the argument that `Start` is the only
message on this wire that re-zeroes a pulse count, so re-anchoring "must" be a `Start`. That inverted
the meaning of the message. **MIDI `Start` means "return to song start"** — which is exactly why the
[host clock-out note](../../implemented/architecture/2026-09-29-midi-clock-out-in-the-host.md) and the
`TransportPlay` arm send it *only* at frame 0, and why `crates/engine/src/plugins/clock_out.rs`
defines it that way. Sent at 24 000, a `Start` would not re-zero a follower against the session; it
would tell gear to jump to **its own** song top while the session sits half a second in — a second,
larger error than the phase one this message exists to correct, and an error in the one message whose
whole content is a position claim.

So the re-anchor re-declares the only thing 24 PPQN can honestly state at a non-zero frame: *keep
running; the position moved*. That is `Continue`. A seek landing on frame 0 gets the `Start` that
frame deserves, because there the two messages finally say the same thing. The distinction is the
whole point of the pair and is pinned at both ends by
`a_seek_while_playing_re_anchors_the_follower_at_the_target`.

**What the re-anchor buys, stated exactly.** A follower that re-bases its beat count on a `Continue`
after a jump is re-anchored by this. A follower that reads `Continue` as pure "carry on counting" is
not — and 24 PPQN with no Song Position Pointer gives us no way to say more, because no message in
this scope states an absolute position. The honest claim is "the follower is *told* the position
moved", not "the follower's position is now correct".

**A seek, and only a seek.** Undo and redo call `replay_to_kind(self.engine.clock.frame(), false)`:
they rebuild *at the current frame* over an arrangement edit, and an arrangement edit cannot move a
tick (nothing in an `Arrange` op touches the tempo map). The follower is still in phase across one, so
a re-anchor would be a message about nothing having moved — and undo is common enough that noise there
would be its own defect. **A stopped transport** re-anchors nothing either: the follower was stopped
by the `Stop` the old session already sent, and a rebuild is not what stopped it.

## Alternatives considered

- **`Start` at every seek target** (the first draft of this note; the reason the supervisor sent it
  back). Rejected, and the rejection is the interesting part: it argued that a `Start` is the only
  message that re-zeroes a pulse count, so a re-anchor "must" be one. That treats `Start` as
  *re-anchor*, which is not what it means — it means "return to song start". The cost is not the
  declared one (a follower's own position display restarting at the target) but an undeclared one:
  gear obeying it jumps to its own top, away from the session, so the fix manufactures a worse error
  than the defect it addresses. It also contradicted three things the codebase had already decided —
  the `TransportPlay` arm, the host note, and the `Transport` enum's meaning. "Needs a human
  decision" was the wrong conclusion to reach: the decision was already in the code, and a third
  option is not what a contradicted decision calls for.
- **`Continue` at a non-zero target, `Start` at frame 0** (mirroring the `Play` arm; the shape the
  review proposed). **Adopted** — it is the decision above, for the reason above.
- **Send a `Start` on every rebuild, seek or edit.** Rejected: an undo moves no tick, so it re-anchors
  a follower that is already in phase. The `is_seek` gate is the difference between re-syncing and
  nagging.
- **Add `TransportPlay` to `is_state`, so the history carries the play and the replay re-feeds the
  tap.** Rejected: it makes the transport a document. A play is a *moment*, not a value — replaying
  `play` at a different frame is a different event, and the history would then claim the session's
  transport state, which is why the actions/actions split is drawn where it is.
- **Carry the tap's contents across the rebuild in `carry_over`,** so a seek to frame 0 after playing
  resumes mid-log and a seek home replays the whole history as a burst of transport. Rejected: it
  would send a `Stop`/`Continue`/`Start` history to the wire at a position that has nothing to do with
  it, and the burst would land in one block against a 64-entry send bound. A re-anchor is one message
  about the jump that actually happened.
- **Emit the re-anchor directly to the sink**, bypassing the log. Rejected: the log is the declared
  seam and it carries the frame; a direct send would have to invent an offset and would race the
  rebuilt node's own send in the same block. Queuing it means the node's existing
  take-due-then-flush path does the work, and the message lands with the ticks it re-anchors.
- **Say nothing, and file the whole thing as an SPP problem.** Rejected: it leaves the most common
  operator action in the app — a seek — silently unannounced, and the note would have no code behind
  it. SPP is the fix for *position*, not for *telling gear the position moved*.
- **Song Position Pointer.** Still the right fix for a *position* (it can state one; neither `Start`
  nor `Continue` can), and still deferred — it needs "what is a bar" in the gear's terms, which
  [the design note](../../proposed/architecture/2026-09-29-midi-clock-out.md) records. This fix is the
  partial mitigation, and it is honest about being one: it tells the follower the position moved, it
  does not tell it where in the piece it is.

## Consequences

- `crates/host/src/lib.rs`, `replay_to_kind`: the feed, after `restore_midi` and before `carry_over`
  (so the device is back before anything can send, and the rebuilt session is the one that ends up
  live). The `transport` field doc and the `TransportPlay` arm's comment lost the "a replayed command
  re-feeds the tap identically" claim, which was never true; the `clock_out` module header's "the next
  `play` re-syncs" now names the host as the owner of the far side of a rebuild.
- Tests, in `clock_out_wiring` beside `a_seek_sends_nothing_while_it_rebuilds_and_resumes_afterwards`:
  `a_seek_while_playing_re_anchors_the_follower_at_the_target` fails on the unfixed code with
  `left: [(0, "Start")]`, `right: [(0, "Start"), (24000, "Continue")]`, and pins **both** ends of the
  distinction in one test — `Continue` at 24 000 and `Start` at a seek home, so a later edit cannot
  quietly collapse the pair into one message;
  `a_seek_while_stopped_re_anchors_nothing` and `an_undo_while_playing_re_anchors_nothing` pin the two
  sides of the gate. The existing test's assertion is unchanged and still true — nothing crosses a
  *rebuild*; its message now says why the count is still one there (the re-anchor is queued, not sent).
- `crates/engine/src/plugins/clock_out.rs`: the `Transport` enum's doc now states what `Start` means
  ("return to song start") and that it is truthful only at frame 0. The enum was the definition site
  and said nothing about the distinction, which is how the host's first draft came to read `Start` as
  a generic re-anchor; a meaning that governs every caller belongs on the type.
- A seek while playing now puts one `Continue` on the wire (one `Start` when the target is frame 0).
  Gear that treats `Start` strictly as "return to the song top" is no longer told to jump away from
  the session, and gear that re-bases its beat count on a `Continue` is re-anchored. Gear that reads
  `Continue` as pure "carry on counting" is not re-anchored, and nothing in this wire's scope can fix
  that.
- **Not fixed here, and not claimed to be:**
  - **Position.** Only Song Position Pointer can state an absolute position, and it is deferred
    ([design note](../../proposed/architecture/2026-09-29-midi-clock-out.md)). The re-anchor announces
    that the position moved; it cannot announce where it moved to.
  - **A tempo change the host never announced.** A follower that counted pulses across a `set_tempo`
    that took effect mid-playback still drifts, and the re-anchor is not a fix for that — a tempo
    meta-event is ([design note](../../proposed/architecture/2026-09-29-midi-clock-out.md)).
- **Still true after the fix:** nothing is sent across a rebuild. The re-anchor is queued while the
  slot is empty and is sent by the first render after the adoption, which is the same rule the export
  clone relies on.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
