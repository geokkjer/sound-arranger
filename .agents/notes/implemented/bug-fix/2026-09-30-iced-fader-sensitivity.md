# Agent Note: the iced mixer faders were stuck near the top — a scroll rate used as a fader mapping

Status: implemented

## Problem

Hands-on use of the iced spike reported the mixer sliders as "almost broken — they could only move a
tiny bit up or down and was stuck at the top, very counter intuitive to how i expect a mixer to
function." Two independent causes compounded, and each alone would have been noticeable.

**Cause 1 — `iced_audio`'s default `drag_scalar` is a scroll-wheel rate, not a fader mapping.**
It is `0.00385` per pixel (`iced_audio-0.17.0/src/core/virtual_slider.rs:59`), and the crate's own doc
calls it "the amount the [`Normal`] value will change for each pixel the mouse moves" — a rate tuned
for fine adjustment, not for travelling a control. Over this shell's 150 px fader a full-height drag
therefore moved the parameter by only `150 × 0.00385 = 0.577`, **58 % of the range**:

```
--- a full 150px drag moves normal by 0.577 ---    (measured, before)
```

The fader could never reach its own top by dragging, which is exactly the reported "stuck".

**Cause 2 — the dB range kneed far above unity, compressing the useful top into a few pixels.**
`DRange`'s skew (1.38) is non-linear, so how much travel a dB is worth depends on where you are. With
the range as written (-60…+12, unity at 0.833 of the travel) the *bottom* was generous and the top was
not:

```
normal 0.833 -> +0.00 dB      normal 0.950 -> +7.34 dB
normal 0.900 -> +3.40 dB      normal 0.990 -> +11.02 dB   <- the last 10 % buys ~0.98 dB
normal 0.950 -> +7.34 dB      normal 1.000 -> +12.00 dB
```

That measurement is of the **fixed** +12 ceiling, and the +12 was already deliberate and already
pinned: `the_fader_range_round_trips_in_db` asserts `unmap_to_db(map_db(24.0)) == 12.0`, so the range
clamps a "+24 dB" request down to +12 on purpose. It matters here because +24 is the obvious "give it
headroom" move, and it is exactly what makes this acute rather than merely steep — a knee far above
unity squeezes the whole usable top into the last few millimetres. So cause 2 is not a defect that was
fixed; it is the constraint that turned cause 1 from ordinary sluggishness into a control that reads
as *stuck*, and the comment now added on `FADER` states it.

## Decision

The iced shell's console faders drag like faders: **one pixel of drag is one pixel of travel**, so a
full-height drag crosses the fader's whole range. In
[`spikes/iced-shell/src/main.rs`](../../../../spikes/iced-shell/src/main.rs):

1. **Sensitivity (the fix).** `fader_config()` gives the `VSlider` a config whose `drag_scalar` is
   `1 / FADER_HEIGHT`, derived from the widget's height rather than hard-coded. The fader height
   becomes the `FADER_HEIGHT` constant so the two cannot drift.
2. **The ceiling, documented not changed.** `FADER` keeps its existing +12 dB knee and the comment
   now says why: the shape near the top is what makes a sensitivity bug *read* as a stuck control, so
   a future "give the fader headroom" edit has to argue with a measurement.

Two regression tests pin it: `fader_travel::a_full_height_drag_uses_the_whole_range` (the arithmetic
of cause 1) and `fader_travel::the_top_of_the_throw_is_usable` (the curve shape of cause 2, including
that unity still sits at ~0.833 of the throw).

## Alternatives considered

- **Raise `drag_scalar` to some larger magic number.** Rejected: any constant is dimensionally wrong —
  the value the crate wants is *per pixel*, so it must be derived from the widget's height. Encoding
  `1 / FADER_HEIGHT` makes the "full drag = full range" intent explicit and survives a resize of the
  console.
- **Change the widget to absolute positioning (handle jumps to the cursor).** Rejected *for this fix*
  because `iced_audio`'s `VSlider` has no such mode — it is a virtual slider by construction. Worth
  revisiting only if relative dragging proves to feel wrong in use; the reported problem was
  sensitivity, not modality.
- **Raise the ceiling to +24 dB for headroom.** Rejected: measured, it costs far more at the top than
  it buys. +12 dB above unity is enough for a master fader, and every dB of knee above unity is
  travel taken from the range a user actually rides.
- **Leave it — it is a spike, not the shell.** Rejected: the spike's whole purpose is to answer "is
  iced a viable shell", and a control that cannot be moved is evidence against, not a cosmetic
  defect. It was also the *first* thing hands-on use found, which is the spike earning its keep.
- **Hand-roll a fader widget now.** Rejected as premature: the crate's widget is fine once
  configured, and the risk note already records that a replacement would be a canvas widget if
  `iced_audio` stalls.

## Consequences

- **This is a reusable trap for the iced shell, not a one-off.** Anything built on `iced_audio`'s
  sliders or knobs inherits the same default, so every future fader (per-clip gain, sends, plugin
  parameter editors) needs the same `drag_scalar` derivation. `fader_config()` is the single place
  that should hand it out.
- **The dB curve is now a recorded decision, not an accident.** The comment on `FADER` states the
  usable-top constraint, so a future "give the fader more headroom" edit has to argue with a
  measurement.
- **The measurement method is cheap and worth repeating.** The numbers above came from a throwaway
  `#[cfg(test)]` probe printing `FADER.unmap_to_db` across the travel. Any control whose *feel* is in
  question in a shell can be probed the same way instead of being judged by eye.
- **Not verified by hand.** The fix is verified by unit test (travel arithmetic and curve shape) and
  by build/clippy/fmt; whether it *feels* right needs the owner at the keyboard. If it still reads
  wrong, the remaining lever is `skew_factor` on `DBRange` (spreading the curve evenly above unity),
  which was deliberately left alone.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
