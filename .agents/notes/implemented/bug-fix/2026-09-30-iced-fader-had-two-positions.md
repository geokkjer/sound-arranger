# Agent Note: the iced fader had two positions — a `step` default of 1.0

Status: implemented

## Problem

The console faders moved, in the right direction, between the right endpoints — and nothing else.
Every drag landed on **−60.0 dB or +6.0 dB**, never between, which reads as a control that jumps from
min to max.

iced's `VerticalSlider` carries a step size and defaults it to **one unit of the value type**:

```rust
// iced_widget-0.14.2/src/vertical_slider.rs
step: T::from(1),                       // T = f32  =>  step = 1.0
…
let percent = 1.0 - f64::from(cursor_position.y - bounds.y) / f64::from(bounds.height);
let steps = (percent * (end - start) / step).round();
let value = steps * step + start;
```

`percent` is 0..1, `end - start` is 1 for a `0.0..=1.0` range, and `step` is 1.0 — so `steps` is
`round(percent)`, which is **0 or 1**. The widget was behaving exactly as configured; the range the
fader was given made that configuration destructive. Nothing warned, because a `T::from(1)` default
over a unit range is legal and merely useless.

This is the third defect in the same control, and the one that produced the "min or max" symptom:

| # | defect | cause |
|---|---|---|
| 1 | handle moved *up* when dragged down, never reached the top | relative + vertically inverted virtual slider ([note](2026-09-30-iced-console-fader-is-not-a-drag-control.md)) |
| 2 | top of the fader refused, jumped back | ceiling invented in the shell (+12 dB) instead of derived ([note](../architecture/2026-09-30-a-shell-derives-values-from-the-engine.md)) |
| 3 | no position between the ends | `step` default of 1.0 over a 0.0..=1.0 range (this note) |

## Decision

**Set the step explicitly, and build every fader through one function so it cannot be forgotten.**

```rust
/// The fader's step over the unit range. iced's default is `T::from(1)` — for
/// `f32`, 1.0 — which over `0.0..=1.0` yields only the two ends.
const FADER_STEP: f32 = 0.001;

fn fader_widget(index: usize, value: f32) -> Element<'static, Message> {
    vertical_slider(0.0..=1.0, value, move |normal| Message::Fader(index, normal))
        .step(FADER_STEP)
        .width(18.0)
        .height(FADER_HEIGHT)
        .into()
}
```

0.001 gives ~1000 positions across a ~150 px throw — finer than a hand can resolve, and coarse enough
that the value is not noisy at pixel scale. The single builder matters: defects 1 and 3 were both a
default at a call site (`drag_scalar`, `step`) that no one set, so the call site is now the one place
that cannot omit them.

`fader_step::the_step_leaves_usable_positions_between_the_ends` recomputes the widget's own maths and
asserts both halves: that the crate default collapses to exactly **two** positions, and that
`FADER_STEP` yields at least 100 with a genuine mid-throw gain. The first assertion is what makes this
a regression test rather than a restatement.

## Alternatives considered

- **Scale the slider range to the value space instead of the unit interval** (e.g. `0.0..=2000.0`
  milligain, with the crate's step of 1.0). Rejected: it buys the same resolution by hiding the scale in
  the range, so the next reader has to work out why a fader runs to 2000 — and the dB mapping would be
  buried in that scale rather than stated once.
- **Keep the unit range and leave `step` at its default.** Rejected: that *is* the defect.
- **Set the step per call site.** Rejected: that is how both defaults got missed. One builder, one step.
- **Handle it in the dB mapping instead** (quantise `Normal` on the way through `gesture()`). Rejected:
  the widget would still offer only two positions to the pointer, so the drag would feel binary no
  matter what the handler did with the values.
- **Replace the widget with a custom canvas fader.** Still the recorded fallback if a styled console
  needs more than the stock three-layer slider, but three defects in, the stock widget has been
  *configured* rather than outgrown — each fix was a missing setting, not a missing capability.

## Consequences

- **The fader now has continuous travel**, verified by the round trip (every position from 0.0 to 1.0
  reaches the host as a distinct gain: `0.5 -> 0.1009`, `0.9 -> 0.9886`) and by rendering, which was
  never in doubt — a graded sweep of the four channels at 0.15/0.4/0.65/0.9 drew four handles at
  ascending heights. Worth stating plainly: **the *rendering* was always correct, and every previous
  screenshot looked right**, because the drawn handle follows `value` and the value only ever had two
  states. Three defects were visible in the hand and invisible in a screenshot.
- **This is the second `iced_audio`-family default that had to be set explicitly** (the first being
  `drag_scalar`), which upgrades the evaluation note's risk: the cost of these widgets is not their
  drawing, it is that their defaults assume a hardware-knob idiom the shell does not share.
- **`--sweep` is now the instrument for this class of question.** It drives the real `Message::Fader`
  path in-process and measures what the drag costs the device, which is how "the sliders cause
  underruns" was falsified: 300 messages at ~14,000/s cost **0** underruns. A GUI drag cannot be
  scripted from outside and cannot be piped in, so the shell does the dragging itself.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
