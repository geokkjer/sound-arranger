# Agent Note: A logged patch is a patch the graph will make

Status: implemented

## Problem

The graph is a typed patch bay with one ordering rule: a cord runs from an earlier
node to a later one (`Graph::connect`, `crates/engine/src/graph.rs`). A plugin's node
lands in the order its mount applied, so the rule is reachable from the *language* —
`mount mixer channels=2`, `mount tone`, `patch tone.audio mixer.ch0` is legal
`host v1` and is what the TUI console accepts, and it puts the mixer at node 0 and
the tone at node 1. A re-mount produces the same shape from a different direction:
`unmount tone` + `mount tone` + `patch tone.audio mixer.ch0` appends the tone
*behind* the sink.

`Engine::validate_patch` (`crates/engine/src/render.rs`) checked that both plugins
were known, that both were mounted or queued, that both ports existed, their
directions, their kinds and their channel counts. It never asked about **order**,
though for a mounted endpoint the answer is one `position` call on
`Graph::nodes()`. So the patch was validated, logged and scheduled, and at apply
`graph.connect` returned `Err("connect: patch cords must go forward …")` — whose
only report was a `debug_assert!`:

```rust
if let Err(e) = self.graph.connect(from_node, from.1, to_node, to.1) {
    debug_assert!(false, "scheduled patch refused at apply: {e}");
}
```

In release — the build a user runs — that compiles away: the destination channel is
never fed, the master is silent from that frame on, and nothing anywhere reports it.
In debug and in tests the assert **panics inside `render_block`**, on the pump
thread, where nothing catches it. Both are the same defect, and both are the inverse
of the engine's own "nothing logged is silently dropped": here a logged mutation
never happens at all.

The hazard was avoided by convention everywhere it was exercised, which is why no
test saw it: every engine test that wires `tone → mixer` mounts the mixer **last**
(`spike_a.rs`, `phase1_mixer.rs`, `drain.rs`, `fundsp_synth.rs`), and the one place
the graph's own rule is asserted — the forward-order line inside
`graph::tests::connect_rejects_type_mismatch` — calls `Graph::connect` directly,
never through `Engine::patch`. The host works the same constraint from the other
side — a player appended after a materialized mixer "would make its cord backward —
every later render fails", so `crates/host/src/lib.rs` `insert_before`s it — which
confirms the rule is real and unguarded from the user's side.

## Decision

**A patch the graph would refuse is refused at call time, and a patch that still
cannot be made is recorded rather than asserted.**

- **`Engine::validate_patch` asks the forward-order question before the log takes the
  patch**, from one rank scale (`Engine::graph_rank`): a **mounted** plugin's answer
  is its node's own index in `Graph::nodes()`; a **queued** mount has no node yet, so
  it ranks `nodes.len() + seq`, where `seq` is the mount-apply order recorded in
  `Engine::mount_seq` (one number per `mount` call, and a **fresh** one for a
  re-mount, which is appended again). Applied ranks `0..nodes.len()` and a queued
  mount ranks from `nodes.len()` up, so "the source precedes the destination" is a
  single `>=` with no case analysis. The refusal names the fix: *"'mixer' is mounted
  before 'tone' (a queued mount lands behind every node already in the graph), so
  mount 'mixer' first."*
- **The prediction rests on one stated fact: a mount's `apply` appends its node.**
  Every plugin in the tree does (`add_node`), and the one tool for a node that must
  *precede* an existing one — `Graph::insert_before` — belongs to the host, which
  places its own arranger nodes on the graph directly. It is not reachable from
  `Engine::patch`, whose endpoints are always plugins the engine itself mounted. A
  plugin that starts using `insert_before` in its `apply` would then be refused for a
  cord that would have worked: a loud false negative, in exchange for no longer
  accepting one that does not.
- **`replay_from` inherits both halves.** `replay_events` calls the same
  `validate_patch`, and its `Mount` arm writes `mount_seq` through the same
  `take_mount_seq` the live path writes, so a replayed cord is judged against the
  same order a live one would be. This is a **contract change for existing session
  files**: a recorded log carrying a backward cord now fails to *load*, with the
  same message, instead of loading and rendering a session the file does not
  describe. That is the outcome a frame-inverted log already gets
  (`Engine::frame_inverted`), and the right one — the file is wrong, and a loud
  refusal is how a document this engine wrote gets corrected.
- **A cord the engine still cannot make at apply is an `ApplyFault`** — recorded in
  every build, bounded by `MAX_APPLY_FAULTS`, counted past the bound, and *never
  drained*, exactly like a mount a plugin's `apply` refused. The one that remains
  reachable is a log-order error validation cannot see: a destination unmounted at
  the same frame the cord was scheduled for, which the frame-ordered queue applies
  first (FIFO within a frame). It was reachable before this change too, and the
  `debug_assert` covered it — as a panic. The fault names the cord as the log spells
  it (`patch tone.audio → mixer.ch0 refused: …`), so a host can point at the edit.
  Reusing the existing list is what makes `Engine::is_degraded()` true for a session
  with a channel nothing feeds, which is precisely what such a session is; the
  shells do not read it yet, and that is unchanged.
- **`apply_patch` no longer asserts.** The `debug_assert!`s are gone, following the
  mount path's own precedent: a refusal the engine cannot prevent is a fact about
  the session, and a fact is recorded, not asserted into a panic on the audio
  thread.

## Alternatives considered

- **Record apply-time refusals in a new `Vec<String>` the host drains** (the review's
  second suggestion). Rejected: two lists for one class of fault, and a *drainable*
  list inverts the stated discipline — `apply_faults` is never drained precisely
  because a poll that consumed the evidence would let the evidence disappear, and
  because a fault is a standing fact, not work to be done later. The existing list
  already has the bound, the drop counter and the degraded flag; a second list would
  have none of them, and `is_degraded` would not see a never-fed channel.
- **Compare mount-apply order assigned in `apply_mount`** (the review's first
  suggestion). Rejected as stated, because a queued mount has no sequence number yet:
  the case the bug is actually about is `patch` before any render, which is the shape
  every `host v1` script and every console session produces, and there both endpoints
  are queued. The number has to be taken when the mount is **scheduled**, which is
  also when it is knowable and when the order is settled — the queue is frame-ordered
  and FIFO within a frame, so schedule order is apply order.
- **Let the engine fix the order** — `insert_before` a mount that must precede an
  already-mounted node, so the cord is legal as written. Rejected: the graph's node
  order is the mix's decision, not the engine's, and reordering rewires the cords
  already attached to every node after the insertion point. That is a bigger and
  quieter change than a refusal that names the fix, and it would make the log
  describe a graph the user never built.
- **Record the fault at apply and leave validation alone** — the engine "survives" a
  backward cord. Rejected: that keeps the defect, which is a *logged* mutation that
  cannot happen, and it pays for it with a degraded session instead of an `Err` at
  the keyboard. The fault is the backstop for what validation cannot foresee, not the
  answer to what it can.
- **Wire the cord to something else at apply** (the first audio provider, the master
  bus) so the channel is fed anyway. Rejected: it renders audio nobody asked for, out
  of a graph the user did not build, and reports no fault about it.
- **Ask the graph to reorder on `connect` refusal.** Rejected: a cord is a connection
  between two declared ports; silently moving a node to satisfy it would make the
  graph's own forward-order rule unenforced everywhere else that depends on it (the
  interpreter walks nodes in order).

## Consequences

- **Regression tests, all three beside their module's neighbours:**
  `spike_a::a_backward_patch_is_refused_before_it_is_logged` (the refusal, the log
  holding only the two mounts, no `Event::Patch` written, the session not degraded,
  the re-mount shape refused the same way, and forward order still accepted),
  `spike_a::a_log_carrying_a_backward_cord_is_refused_at_load` (a hand-built log
  refused by `replay_from`, which is what pins the replay half and the
  `take_mount_seq` write), and
  `apply_refusals::a_patch_the_engine_cannot_make_at_apply_is_reported_and_does_not_panic`
  (a destination unmounted at the cord's frame: one fault naming `tone.audio →
  mixer.ch0` at the log's frame, `is_degraded`, the same audio and the same fault
  from a loaded log). On the unfixed code the first two fail at their `expect_err`
  (the patch is accepted), and the third fails where the render did not come back:
  the `debug_assert` unwinds out of `render_block` — the render-thread panic,
  reproduced — so the caught render yields no audio and the fault list is empty.
- **`cargo test -p engine`: 125 tests (49 lib, 74 integration, 2 doctests), 0
  failures**, and `cargo build --workspace` is clean. `cargo test -p host`, `-p media`
  and `-p workflow` also pass, which is the real check on the contract change: every
  script the host runs, and every mastering-chain test (`mount master` after the
  mixer, then `patch mixer.audio master.audio`) is forward.
- **A user now hears about it at the keyboard.** `HostCommand::Patch` is
  `self.engine.patch(…)`, so the console's `patch` returns the refusal as a command
  error instead of accepting a mute session; a script that mounts its sink before its
  source now fails at that line.
- **The render path is untouched.** `graph_rank` and `validate_patch` are control
  side, and `apply_patch` allocates only on the misuse path it already took (a fault
  string), exactly as `apply_mount` does. No lock, no growth on an abiding path; the
  counting-allocator tests hold.
- **The two ends of the prediction are both still open, honestly:** a plugin whose
  `apply` inserts before an existing node would be refused for a cord that would have
  worked (loud, and the fix is the plugin's), and a mount that the render loop *parks*
  (`changes_master_width`) applies ahead of an earlier-scheduled queued mount, so the
  rank scale can predict forward where the graph lands backward. That second case is
  unreachable from the control side today — parking happens inside a render, and a
  mount cannot be scheduled during one — and the fault list is the backstop for it if
  that ever changes.
- **No shell reads `apply_faults` yet.** A session with a refused cord is degraded in
  the engine's own words, and it is the host's job to show that; the surface is
  public, and the plumbing is not built. The call-time half needs none of it: the
  TUI console prints `: {line} — {err}` and a script exits 1 with the message.
- **Left, deliberately: the `SetParam` and `Arrangement` arms of `apply_event`**
  still `debug_assert!` an endpoint that is not mounted or a refused op, for the same
  reason this note moved `apply_patch` and did not move them — it is the same defect
  in two more places, and a fix that rewrites every arm of the apply path is a
  different change than this one. The argument for the record-not-assert answer is
  the euclidean note's, and it applies to them unchanged.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
