# Agent Note: a shell derives its values from what the engine declares

Status: implemented

## Problem

A shell that hard-codes a number the engine already owns will drift from it, and the drift shows up as
a broken control rather than as a compile error.

The iced console demonstrated it. Its fader mapped its top to **+12 dB** (gain 3.98), chosen in the
shell as a plausible "console headroom" ceiling. The mixer plugin declares

```rust
// crates/engine/src/plugins/mixer.rs
ParamDef { name: "ch0.gain", min: 0.0, max: 2.0 }
```

so **+6.02 dB**. The engine enforces its own declaration — `RenderEngine::set_param` refuses anything
outside `[min, max]` with `"parameter 'ch0.gain' out of range [0, 2]: 3.98"` — so the top of every
fader asked for a value the host must reject. The visible symptom was a fader that **jumped from max
back to its previous value** and a status line reading *command refused*, which reads like a UI bug and
is actually the shell having invented a bound.

## Decision

**A shell derives every value that the engine declares. It does not restate one.**

Where the value is needed, the shell reads the declaration:

```rust
fn fader_max_db() -> f32 {
    let gains: Vec<f32> = engine::plugins::MIXER_PARAMS
        .iter()
        .filter(|def| def.name.ends_with(".gain"))
        .map(|def| def.max)
        .collect();
    let first = *gains.first().expect("the mixer declares gain parameters");
    assert!(
        gains.iter().all(|max| (*max - first).abs() < f32::EPSILON),
        "the mixer's gain bounds disagree: {gains:?}"
    );
    20.0 * first.log10()
}
```

The fader's whole range follows from that plus a display floor: `DBRange::new(FADER_FLOOR_DB,
fader_max_db(), …)`, and unity is computed from both (`(0 - floor) / (ceiling - floor)`) rather than
asserted as the 83 % console convention. With `[0, 2]` the ceiling is +6.02 dB and unity lands at
~91 % of the throw — a different feel from the 83 % the shell previously hard-coded, and *correct*
because it is what the declared bounds imply.

Two tests hold the coupling: `fader_matches_the_declared_bounds::the_top_of_the_fader_is_a_value_the_host_accepts`
walks every declared gain def and asserts the fader's top is within it, and `unity_follows_from_the_bounds`
asserts unity is exactly where the two bounds put it. Changing a gain bound in the engine now fails
the shell's tests instead of producing a refused command in the hand.

## Alternatives considered

- **Keep the ceiling in the shell and widen the engine's declared max to match.** Rejected: it makes
  the engine's declaration follow the UI, which inverts the dependency this note exists to fix. The
  declaration is the contract; the UI is one of its consumers.
- **Read the bound from the host at runtime instead of the engine crate.** The better long-term shape —
  `Snapshot` publishes parameter *values* (`params: Vec<(&'static str, &'static str, f32)>`) but not
  their bounds, so a shell currently cannot learn a limit except from the crate that declares it. The
  spike shares the workspace and already depends on `engine`, so reading `MIXER_PARAMS` is honest and
  zero-cost; publishing bounds on the snapshot is the right move if a shell ever runs out-of-process.
- **Clamp silently in the shell before sending.** Rejected: it hides the disagreement. A value the host
  would refuse is a fact about the shell being wrong, and clamping turns a loud failure into a quiet
  loss of range (the top of the fader would simply not reach the declared max).
- **Assert the 83 % console convention and pick bounds to suit it.** Rejected: it would reverse-engineer
  engine bounds from a UI preference. Unity's position is an *output* of the declared range.

## Consequences

- **The rule generalises beyond faders.** Any shell surface that shows or edits an engine-owned value —
  the mixer's mute/solo/pan, the tempo range, master plugin parameters, the clip-op vocabulary — takes
  its limits from the declaration. This is the same discipline the workflow already enforces by making
  both shells read one keymap table: one declaration, many consumers.
- **A missing declaration is a boot failure, not a fallback.** `fader_max_db()` expects gain params and
  asserts the bounds agree, so a mixer that stops declaring them fails loudly at startup rather than
  silently restoring the old invented ceiling.
- **The spike's dependency list grew by one crate** (`engine`), which is honest: the shell already
  links `engine` transitively through `host`, and the dependency now says what the shell actually needs.
- **This is a class of defect worth watching for, not a one-off.** The same shape — the shell owning a
  number the engine owns — is how the mixer console drift started, and the fix is cheap only when the
  declaration is reachable from the shell.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
