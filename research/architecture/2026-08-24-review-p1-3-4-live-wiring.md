# review — live arranger wiring (P1.3.4 fix), 2026-08-24, via dsh headless opencode-go

kimi-cli remained rate-limited; this ran through `dsh --profile headless` opencode-go
(`kimi-k2.6`). Reviews the fix for the silent-bounce bug (arranger→mixer backward cord).

## The fix (verified working)

- **`Engine::arrange_logged`** — validate + log (`Event::Arrangement`) WITHOUT scheduling;
  used for value-level arrangement ops so they never flush the mixer early. `arrange` keeps
  log + schedule.
- **`ClipEditor::apply`** now applies the op **eagerly** to the shared `Timeline`
  (validate-first `*tl = tl.apply(&op)?`; a refused op returns before logging) and logs via
  `arrange_logged`. The live value is always current; the handler runs only on replay.
- **`wire_arranger`** builds one `ArrangerNode` per track from the final value, adds them to
  the graph **FIRST**, then flushes (the scheduled mixer applies **last**), then connects
  `arranger → mixer ch{i}` — forward order restored. The mixer's `set_out` overrides the
  transient out_node claim of the first source.

Verified by a **byte-identical, non-silent** arrangement bounce (previously the test passed a
*vacuous* silent bounce).

## Why the remaining W8 (edit-after-wiring) is genuinely deferred, not a bug here

The mixing-bus ordering is the crux: once the mixer is applied, any *new* arranger source is
added **after** it in the graph (backward again). So re-wiring after an edit cannot reuse the
one-shot build-then-bounce path — it needs the **shared-state `ArrangerNode`** (which reads a
live value + reconciles readers without re-laying the graph) plus the mixer re-applied last.
That is the dynamic-edit architecture, a separate step. The reference host is **build-then-
bounce** (all ops, one bounce), for which the current wiring is correct; the byte-identical
test covers it.

## M/S/W (the reviewer's)

- **W8 (must-fix, deferred)** `wired_tracks` skip makes an edit after the first bounce silent.
  Root cause is the mixer-last ordering (above), not the skip per se; the shared-state
  reconcile is the fix. **Deferred** (documented here + the P1.3.4 note).
- **W10 (must-fix, minor)** `render().expect()` panics on a data-dependent wiring error. The
  reference host's valid scripts don't hit it; making `render` return `Result` is a follow-up.
- **W7 (must-fix, deferred)** removed-then-re-added track id collides with `wired_tracks`;
  same shared-state/rebuild fix.
- **W4/W5 (should-fix)** equal-frame ordering determinism — the scheduler is FIFO for equal
  frames (verified in the engine); noted.
- **W1 (should-fix)** the interner leaks strings (acknowledged spike debt — a serialized log
  needs a string table).
- **W6 (worth)** `add_node`'s auto out_node claim is fragile; the mixer's explicit `set_out`
  overrides it correctly.

The full review is the reviewer's response in the headless run.
