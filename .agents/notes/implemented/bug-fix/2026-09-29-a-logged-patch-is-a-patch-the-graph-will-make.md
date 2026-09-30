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

### Amendment (same day): a fault is recorded as data, never as a sentence

The verifier of this note's change (`e330241`) found the new record allocating on
the audio path, and the module doc claiming the render path does not allocate with
a claim the code did not honour. Its finding, verbatim:

> **`crates/engine/src/render.rs:869-878` — a new allocation on the audio path, and
> the module doc now lies about it.**
>
> ```rust
> reason: format!(
>     "patch {}.{} → {}.{} refused: {reason}",
>     from.0, from.1, to.0, to.1
> ),
> ```

The decision above stands; the *shape* of the record is corrected here.

- **The record holds data and the sentence is built on read.** `ApplyFault::reason`
  is `ApplyFaultReason`, not `String`: `Cord { class: &'static str, from, to }` for a
  cord the engine or the graph refused (the class is a `&'static str`; the four
  identities are the log's own `&'static str`s, so nothing is copied), and
  `Given(String)` for a plugin's fallible `apply` refusal, **moved** verbatim. The
  line a host prints is `ApplyFault::describe()`, which formats on the caller's
  thread — the read is allowed to allocate, the render is not. This is the same
  control→render discipline the `parked` list already follows (a refusal the engine
  records is control-side material, read later), applied to the fault list.
- **The graph answers with a classification, not a message.** `Graph::try_connect` is
  `Graph::connect` without the sentence: it returns `ConnectRefusal` — a `Copy`
  `ConnectClass` plus the borrowed port names, node indices and the two ports —
  built without allocating. `connect` keeps its `Result<(), String>` and is now that
  check plus `to_string()`, so the **one** rule (forward order, port lookup,
  direction, kind, single-driver control, channel count) has two doors and the
  control-side messages are byte-for-byte what they were. `apply_patch` calls
  `try_connect` and keeps only `class.as_str()`, so a graph refusal is recorded
  without a `String` too.
- **The list's storage is the bound, reserved at `Engine::new`.**
  `Vec::with_capacity(MAX_APPLY_FAULTS)` replaces `Vec::new()`, so `record_apply_fault`
  writes into memory that already exists: a refusal costs no allocation even when the
  list is empty. The bound already said the list holds *at most* 64; reserving exactly
  that makes the bound the **capacity**, which is what turns "bounded" into
  "allocation-free to record". ~7 KB once per engine, paid on the control side.
- **One `ApplyFault` shape, two paths.** The mount path and the cord path share the
  type and the `record_apply_fault` bound; they differ only in *who* worded the
  refusal, which is exactly the fact the enum's two variants record. The mount path
  is unchanged in what it costs the engine: `Plugin::apply` is `Result<_, String>`,
  so the plugin built that message (and that allocation) before the engine was in
  the picture, and the engine's own contribution is still zero.
- **The module doc is now true rather than aspirational**, and narrower where it was
  wrong: "nothing in steady state" (which a counting-allocator test does enforce),
  plus the one misuse cost that genuinely remains — the `parked` push — and an
  explicit statement that a recorded fault costs no allocation at all.

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
- **Keep the sentence and drop it from the audio thread by formatting lazily in a
  `Display` impl on the same `String`-carrying struct** — i.e. the `Given`-for-
  everything shape, with the cord's identities recovered from a `format!` at read
  time. Rejected: the message has to be *stored* to be displayed later, and storing
  a `String` on the render path is the allocation; a `Display` on the record only
  postpones a format that has nowhere to live.
- **A fixed-capacity inline buffer for the fault's message** (a `[u8; N]` + length in
  the record, `String::from_utf8_lossy` on read). Rejected: it trades one heap
  allocation for a silent truncation — a long class or plugin message would be cut
  without a counter, which is the *opposite* of this repo's "a bound is reported,
  not hidden" discipline (`apply_faults_dropped`, `euclidean.drops`,
  `DrainOutcome::capped`). The class-plus-identities shape needs no bound at all.
- **Make `Plugin::apply` return a non-allocating error too** (a class enum, as the
  graph now does), so even the mount path builds nothing on the render thread.
  Rejected *for this change*: it is a breaking change to every plugin's `apply`
  signature in the tree plus every call site, for an allocation the plugin makes
  deliberately and the engine cannot unmake. It is the right next step if the
  invariant is ever stated as "no allocation whatsoever, including a refuser's", and
  it is recorded below as left.
- **Keep the graph's `String` and record it as `Given`** (the smaller diff: the
  engine moves the message it is handed). Rejected: it would have made the fault
  record honest about the engine's own behaviour while leaving an allocation on the
  render path *inside the graph* for the class this note names as the backstop — and
  the fault would have lost the cord's name, since a graph message speaks of node
  indices a host cannot resolve. `try_connect` is the same amount of code with the
  allocation gone.

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
- **`cargo test -p engine`: 127 tests (50 lib, 75 integration, 2 doctests), 0
  failures**, and `cargo build --workspace` is clean. `cargo test -p host`, `-p media`
  and `-p workflow` also pass, which is the real check on the contract change: every
  script the host runs, and every mastering-chain test (`mount master` after the
  mixer, then `patch mixer.audio master.audio`) is forward. *(That sentence is the
  original change's evidence; the amendment's own count is the 127 above, and
  `Graph::connect`'s `String` messages are unchanged, which is what the host's
  `reference_host` assertions on `signal kind mismatch` depend on.)*
- **A user now hears about it at the keyboard.** `HostCommand::Patch` is
  `self.engine.patch(…)`, so the console's `patch` returns the refusal as a command
  error instead of accepting a mute session; a script that mounts its sink before its
  source now fails at that line.
- **The render path is now allocation-free on the misuse path too, which is what the
  first version of this note wrongly claimed.** `graph_rank` and `validate_patch` are
  control side. `apply_patch` records data and no longer formats: the two engine
  classes are `&'static str`, the graph's is `ConnectClass::as_str()`, the cord's four
  identities are the log's own `&'static str`s, and the list's storage is the bound,
  reserved at `Engine::new`. No lock, no growth, no panic — asserted, not asserted
  *about*, by the new counting-allocator regression test below.
- **New regression test for the amendment:**
  `spike_a::a_refused_cord_on_the_render_path_records_without_allocating` drives the
  reachable refusal (a destination unmounted at the cord's own frame, two cords
  scheduled behind that unmount) under the counting allocator and asserts **zero**
  allocations, then asserts the two faults are recorded, name the cord as the log
  spells it, and degrade the session. It lives in `spike_a.rs` because that is where
  the counting allocator lives: the allocator is a `#[global_allocator]` per test
  binary, so a test in `apply_refusals.rs` could not see the render path's
  allocations at all. Verified to fail on the unfixed code in both halves — 4
  allocations with the `format!` restored, 1 with `Vec::with_capacity` reverted to
  `Vec::new()` — so neither the message nor the storage is load-bearing by accident.
- **The read side moved, deliberately.** `ApplyFault::reason` is an enum, so a host
  reads `ApplyFault::describe()` (which allocates, on its thread) rather than
  pattern-matching a `String`. No shell reads the list yet, so this is free today; it
  is the shape the shell plumbing will want.
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
- **Left, deliberately: one allocation on the render path remains, and it is not the
  engine's.** A refused *mount* carries `ApplyFaultReason::Given(String)`, because
  `Plugin::apply` returns `Result<_, String>`: the plugin built that message, on the
  render thread, before the engine saw it. The engine adds nothing, and the fault
  list's storage is reserved, so recording it costs the engine zero — but a session
  whose plugin refuses *does* allocate once per refusal. Closing that is the
  `Plugin::apply` signature change listed under Alternatives, not a change this note
  should smuggle in beside a patch-bay fix.
- **Left, deliberately: a first block is not steady state.** Applying a scheduled
  mount adds a node and applying a scheduled cord pushes one, and both grow the
  graph's own vectors — one structural change per mutation, on the control side's
  schedule, not per block. The counting-allocator test primes before it measures,
  which is why the module doc says "nothing in steady state" rather than "nothing,
  ever". Pre-reserving `Graph::cords` would close it; it is not this defect.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
