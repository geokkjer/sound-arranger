# Agent Note: the shells use iced's components until we want our own visual style

Status: implemented

## Problem

The two shells need widgets — faders, sliders, meters, and (next) a timeline. Building them by hand is
the obvious way to get exactly the interaction a DAW wants, and the obvious way to spend the project's
time on drawing instead of audio. The question arrived twice in one session: the console fader was
built on `iced_audio`'s `VSlider`, then on iced's own `vertical_slider`, and each defect prompted "do we
just write our own?".

## Decision

**Use the framework's components. Write our own only when we want our own visual style — not to fix
behaviour.** Concretely:

- The faders are iced's `vertical_slider`, configured through one `fader_widget()` builder.
- `iced_audio` is kept for its **value maths only** (`DBRange`, `Normal`); its `VSlider` was dropped,
  because its interaction model — a relative, vertically inverted virtual slider — is not a fader
  ([note](../bug-fix/2026-09-30-iced-console-fader-is-not-a-drag-control.md)).
- Every engine-declared value is derived, never restated ([note](../architecture/2026-09-30-a-shell-derives-values-from-the-engine.md)).
- When a component needs configuring to be correct, the configuration lives in **one builder function**
  so a call site cannot omit it.

The trigger for writing our own is aesthetic and specific: a console with tall thin caps, a dB scale,
send indicators, or a timeline that does not look like a *progress bar*. Not "the stock one misbehaved".

## Alternatives considered

- **Write a custom canvas fader now.** Rejected, and this is the decision's centre: three defects hit this
  one control, and **all three were missing configuration, not missing capability** — a relative drag
  model, a `step` default of 1.0 over a unit range, and a ceiling invented in the shell. A hand-written
  fader would have hidden the first two by accident and then owned a third set of bugs. The fallback
  stays recorded: if the stock widget cannot be *styled* the way the console needs, the draw code is a
  canvas widget, not a rewrite of the shell.
- **Write our own because the defaults are traps.** Rejected as a reason: the traps are real
  (`drag_scalar` 0.00385/px, `step` 1.0) and they are now recorded, tested, and set in one place. Two
  settings are cheaper than a widget.
- **Fork `iced_audio` to fix its sign.** Rejected: a permanent fork of a young single-author crate to
  correct one interaction, for an evaluation spike.
- **Adopt styled third-party widgets instead of stock.** Rejected for now: the shells' rule is that both
  speak one workflow, and a widget library with its own visual language would quietly become a third
  design. Style is a decision, not a dependency.

## Consequences

- **The cost of the stock route is now visible and bounded:** every stock widget needs its defaults read
  before it is trusted. Both fader defects were a default at a call site. The mitigation is the single
  builder, and the `--sweep` mode that drives the real message path so behaviour is measured rather than
  eyeballed.
- **Style is deferred, not precluded.** No shell code draws a fader, so replacing the drawing later is a
  contained change to `fader_widget()` plus a canvas widget — which is exactly why the builder exists.
- **Screenshots are insufficient evidence for a control.** All three defects rendered correctly in every
  screenshot, because a drawn handle follows its value and the value was wrong in ways a still image
  cannot show. Behaviour needs the hand or an in-process driver (`cargo run -- --sweep`).
- **This is a rule about components generally, not just iced's.** The shells are the interchangeable
  part of the platform; the audio is the product. Frameworks' widgets are the right default for the
  interchangeable half.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
