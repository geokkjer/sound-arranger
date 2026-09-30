# Agent Note: the iced console fader is not a drag control — replaced with iced's own slider

Status: implemented

## Problem

The console faders were unusable under the mouse, and the reported behaviour was precise enough to
identify the cause: *"minimal movement, stuck on top, grab and pull with the mouse makes the fader
move upward."*

[The previous fix](2026-09-30-iced-fader-sensitivity.md) had set `drag_scalar` to `1 / FADER_HEIGHT`,
reasoning that the crate's `0.00385`-per-pixel default was a scroll rate rather than a fader mapping.
That reasoning was sound about the *magnitude* and wrong about the *cause*. Reading
`iced_audio-0.17.0/src/core/virtual_slider.rs`:

```rust
// CursorMoved, for a vertical slider:
(position.y, position.y - state.prev_drag_pos)      // drag DOWN -> positive delta
…
self.set_param_value(state.continuous_normal - normal_delta)   // …and it SUBTRACTS
```

`y` grows downwards, so dragging down yields a positive delta which is then subtracted — the value
*increases* as the pointer moves down, and the handle travels up. That sign is correct for a
horizontal slider (drag right = turn down, the knob metaphor); for a vertical fader it is inverted.
`VSlider` is a **virtual** slider: a relative-drag control whose movement is a delta from the previous
pointer position, never the pointer's position itself. No `drag_scalar` configures that away, and no
magnitude fix could.

## Decision

**Use iced's own `vertical_slider`, which is absolute.** One widget swap in
[`spikes/iced-shell/src/main.rs`](../../../../spikes/iced-shell/src/main.rs):

```rust
vertical_slider(0.0..=1.0, fader.normal.as_f32(), move |normal| Message::Fader(index, normal))
    .width(18.0)
    .height(FADER_HEIGHT)
```

`iced_widget-0.14.2/src/vertical_slider.rs` locates the value from the pointer's position in the
widget bounds — `-(cursor.y - bounds.y) / bounds.height` — so it has both properties a fader needs:
the handle goes where the pointer is, and the vertical polarity is already correct. The slider's range
is `0.0..=1.0`, which is exactly the space `Normal` lives in, so the two need no conversion.

`Message::Fader` now carries the new position (`f32`) instead of a `Gesture`, and `gesture()` takes it
directly. `FADER`, `DBRange` and `Normal` stay: the mixer's values are gains in dB, and the mapping
between a position on a fader and a dB value is a real design decision worth keeping in one place.
What is removed is `fader_config()` and `FADER_DRAG_SCALAR`, and with them the previous note's
`fader_travel::a_full_height_drag_uses_the_whole_range`, which pinned an arithmetic that no longer
exists; it is replaced by `the_fader_range_is_the_sliders_whole_range`.

## Alternatives considered

- **Negative `drag_scalar` to cancel the inversion.** Rejected, and it would not have worked: the
  crate's `move_virtual_slider` does `continuous_normal - normal_delta`, so a negative scalar would
  indeed flip the sign — but the control would remain *relative*, so the handle would still lag the
  pointer and drift off it on every drag. A fader whose handle is not under the pointer is not a
  fader, and on a 150 px throw a relative control also cannot express "take this to −6 dB" in one
  gesture.
- **Fork or patch `iced_audio`.** Rejected: the crate is a young single-author dependency already
  named in the evaluation note's risks, and carrying a fork to fix one widget's sign is a permanent
  maintenance cost for an evaluation spike.
- **Hand-roll the fader as a `canvas` widget.** Rejected as premature, though it stays the recorded
  fallback if iced's built-in slider proves insufficient for the styled console the real shell wants
  (tall thin caps, a dB scale, sends). iced's widget draws the standard three-layer slider.
- **Keep `iced_audio` only for the dB maths and drop it from the widget layer.** Partially what
  happened: `DBRange`/`Normal` are retained, `VSlider`/`virtual_slider` are not. Dropping the crate
  entirely would mean reimplementing the skew curve and its tests for no gain.
- **Keep the previous note's fix and add the polarity fix on top.** Rejected — that is the trap this
  note exists to record. Both symptoms had one cause (a relative control with inverted vertical
  polarity), so patching the magnitude left the control just as unusable, and it cost a commit and a
  hands-on test cycle to find out.

## Consequences

- **The mixer console now behaves like a mixer**, verified by screenshot as well as by test: faders
  render at the dB their values imply (`ch0…ch3` at +0.0 dB = unity, `master` at −1.9 dB = the 0.8
  gain in the demo log), and dragging maps the pointer to the value absolutely.
- **`iced_audio`'s risk profile drops to its widget layer.** The crate's dB maths are proven and
  tested; its slider is the part that does not fit this shell. The evaluation note's mitigation — "the
  fader/knob draw code is a canvas widget, not a rewrite of the shell" — is now sharper: the *draw*
  code is fine, the *interaction model* is what a real shell may have to replace.
- **The staged/grab interaction is a thing to check in use.** An absolute slider jumps the value to
  the pointer on press; many consoles prefer a grab-offset drag instead (press and move relative to
  where you grabbed). This was not in the reported defect and is not fixed here — recorded so the next
  hands-on pass reports it rather than rediscovering it.
- **The lesson generalises past this widget:** "the parameter barely moves" and "it moves the wrong
  way" are different diagnoses, and a magnitude fix applied to a polarity/relativity bug looks like a
  fix in the code while behaving like none in the hand. Both defects in this shell so far were found
  by hand, not by build or test.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
