# Agent Note: A take declaration is bounded before it is sized

Status: implemented

## Problem

The `take` line is the wire form of a take declaration — the state a finished take commits, the
line a session document carries so a reopened session knows which pool sources it has — and its
**channel count sizes the pool-source vector** the apply arm builds:

```rust
HostCommand::Take { take_id, frames, dropped, channels, at_frame } => {
    self.status_take(&TakeReport {
        sources: (0..*channels).map(|k| format!("{take_id}.ch{k}")).collect(),
        ...
```

`parse_script` parsed that operand with no bound, so `take jam 0 0 100000000000 0` — thirty bytes
— reached the `collect`. A `Range`'s size hint is exact, so `Vec::with_capacity(n)` runs first: past
`isize::MAX / size_of::<String>()` it is a `capacity overflow` panic, and below that it is a
multi-terabyte allocation and an allocation-failure abort. Either way the host dies on malformed
wire input, which is the contract `crates/host/tests/parse_script_robustness.rs` states outright
(*"must be a clean `Err` … it cannot panic"*). It is reachable three ways: the wire, a hand-edited
`session.txt`, and any replay of a saved document carrying the line (`undo`, `seek`, `export`).

Every sibling arm that sizes work from a declared width is already bounded — `source add` checks
`1..=media::capture::CAPTURE_CHANNELS_SANITY`, `media::Capture::start` refuses the same range, and
`bounce` runs `check_bounce_budget`. This arm was the one that sized from a number nothing checked,
which makes it an inconsistency rather than a decision.

## Decision

**`HostCommand::Take`'s apply arm refuses `channels` outside `1..=media::capture::CAPTURE_CHANNELS_SANITY`
before it builds `sources`**, with the bound and the offending count named in the message
(`take 'jam' channels must be 1..=64, got 100000000000`).

The bound sits at the **command**, not in `parse_script`. `apply` is the one validation point every
script, session-file and journal path shares — the same reasoning that made the
[tick-walk note](./2026-09-29-the-tick-walk-is-bounded-not-just-the-scratch.md) *defer* a second
parse-side tempo check ("duplicating the check in the parser would give two places to drift") — and
it is the only placement that also bounds an in-memory `HostCommand::Take`, which is where a replayed
declaration and any Rust embedder's `execute` come from. A parse-side bound would only turn the same
refusal into an earlier one for the wire.

The bound is the **capture sanity bound**, the constant `media::Capture::start` already enforces, so
the rule is one rule: no declaration can name a width no capture could have produced, and no new
ceiling is introduced. It is inclusive at 64.

## Alternatives considered

- **Bound the operand in `parse_script` as well** (the review's "better still"). Rejected: it
  duplicates the rule and its message in two places that can drift, and it bounds only the wire — an
  in-memory `execute` or a replayed log would still reach the `collect` unbounded.
- **Clamp to the sanity bound** (`channels.min(sanity)`). Rejected: a declared width is a claim about
  material that either exists in the pool or does not, and silently narrowing it would name a take
  the pool does not hold. (`record` *does* clamp the device's channel count — that is a device fact
  being made safe, not a claim about named files.)
- **Cap the source vector's length by a second, unrelated number.** Rejected: two bounds for one
  rule is how the inconsistency started.
- **Validate in `format_command`**, the writer that emits `take …`. Rejected: the writer is not the
  reader — a hand-edited or generated document never passes through it, and the defect is on the
  reading side.

## Consequences

- A `take` line — or a saved document, or any replay of one — carrying a width outside `1..=64` is a
  named `Err`, and the process stays up.
- Nothing the platform can legitimately produce is refused: a take can only be recorded at a width
  `media::Capture::start` accepts, so every declaration it commits round-trips through `script_text`
  and `parse_script`. `a_finished_take_is_committed_to_the_session_state` and
  `a_take_declaration_replays_without_a_device` are unchanged and still pass.
- The `take` document format is unchanged; only the accepted range is narrower, and the range is
  stated in the refusal.
- `a_take_declaration_refuses_a_channel_count_it_could_not_have_recorded` (host lib, beside
  `a_take_declaration_replays_without_a_device`) refuses `SANITY + 1` through the wire via
  `run_script`, refuses `usize::MAX` in memory — the width that is a `capacity overflow` panic
  rather than merely an enormous allocation, so the regression fails loudly instead of OOM-aborting
  the test binary — asserts nothing landed in the session, and still binds a take at exactly
  `SANITY`. It fails on the unfixed code.
- If a future interface ever declares wider takes (a multi-mic array past 64 channels), the constant
  to raise is `media::capture::CAPTURE_CHANNELS_SANITY` — the one `Capture::start` enforces, so the
  capture and the declaration stay the same rule.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
