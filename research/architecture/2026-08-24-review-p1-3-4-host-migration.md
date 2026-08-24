# review — P1.3.4 host migration (2026-08-24), via dsh headless opencode-go

kimi-cli reached its usage limit (403) this session; this review was run through
`dsh --profile headless` with the `opencode-go` route (`kimi-k2.6`) — the fallback the
owner set up. Same co-worker-loop role.

The review found the host migration structurally sound for the "build-then-bounce"
reference host, but flagged five must-fix + five should-fix. The two that most mattered
were surfaced *empirically* by the reviewer and by my own test:

## What it caught

- **M1/M2 (the real one): the arranger node is connected to the mixer with a backward
  cord.** The mixer (the bus) is applied at graph index 0 during the first
  `ClipEditor::apply` flush, *before* the arranger nodes exist. The graph's forward-order
  rule (`connect` requires the source index < the sink index) then rejects
  `arranger → mixer`. A naive test therefore passed **vacuously** (silent bounce). The
  mixer-as-bus must be the *last* node, which needs the arranger source nodes added before
  the mixer is applied — and that requires the shared-state `ArrangerNode` reconcile (the
  dynamic-edit architecture), not a one-shot snapshot.
- **M3** `.expect()` on wiring could panic on a bad script.
- **M4** `ArrangerNode::new` spawns threads + warms on the render-call stack (fine for the
  single-thread reference host; a hazard for a future threaded host).
- **M5** `Bounce` counts as a media command (pre-existing semantic nit).
- **S1** `parse_script` doesn't parse `arrange`/`pool` (text-format gap); **S2** `wired_tracks`
  never shrinks on track removal; **S3** `ArrangerNode` lacks explicit reader cleanup;
  **S4** `warm()` wall-clock; **S5** `set_pool` delayed stem validation.

## Resolution this slice

Shipped the parts that are genuinely correct and non-vacuous: the host gains
`HostCommand::Arrange` (a logged ACID op) + `HostCommand::Pool` (self-describing pool
pointer), which build the arrangement value and replay it byte-identically (proven by a
value-equality test). The live audio wiring of arranger nodes into the mixer is **deferred
and documented** as the next step: it needs the shared-state `ArrangerNode` reconcile plus
the mixer applied last (both the forward-order and the dynamic-edit architecture). The
broken one-shot wiring was removed rather than left as a silent/vacuous path.

The full review text is the reviewer's response in the headless run. Session not persisted
(kimi-cli 403'd); the fallback is `dsh --profile headless` with `opencode-go/kimi-k2.6`.
