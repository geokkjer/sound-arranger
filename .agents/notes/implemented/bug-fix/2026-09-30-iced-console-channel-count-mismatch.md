# Agent Note: the iced mixer crashed at startup on two channel counts that disagreed

Status: implemented

## Problem

`cargo run` in [`spikes/iced-shell`](../../../../spikes/iced-shell/) panicked on the first frame:

```
thread 'main' panicked at src/main.rs:739:58:
index out of bounds: the len is 0 but the index is 0
```

The console guarded on one channel count and indexed with another:

```rust
if self.channels == 0 { return text("no mixer mounted").into(); }   // the FOLD's width
for index in 0..=self.channels {
    let (label, meter) = if index == self.channels {
        ("master".to_string(), self.snap.master)
    } else {
        (format!("ch{index}"), self.snap.channels[index])           // the SNAPSHOT's levels
    };
```

`self.channels` is adopted from the host's **parameter fold** (`outcome.params`,
`mixer channels`) — the mixer's configured width. `self.snap.channels` is the published
**meter levels** vector, which is empty while no audio device is open. The two are independent by
construction, and the guard only tested the first, so a wide-but-unmetered mixer indexed an empty
vector.

This was a live startup crash, not a corner case: the window never came up.

## Decision

**A strip that cannot be metered reads 0 rather than being skipped, and the meter vector is indexed
safely.** One line, in [`spikes/iced-shell/src/main.rs`](../../../../spikes/iced-shell/src/main.rs):

```rust
(format!("ch{index}"), self.snap.channels.get(index).copied().unwrap_or(0.0))
```

Pinned by `an_unmetered_strip_reads_zero_instead_of_panicking`, which builds the exact state that
crashed — a headless spike with a mounted mixer whose `snap.channels` has been emptied — and asserts
the console builds.

## Alternatives considered

- **Guard on both counts (`self.channels > 0 && !self.snap.channels.is_empty()`).** Rejected: it
  fixes the crash by hiding the whole console whenever a device is closed, so the mixer vanishes
  instead of showing unmoving faders. The faders are still correct and still usable — only the
  *meters* have no data — so dropping the level to 0 is the honest degradation.
- **Loop over `0..self.snap.channels.len()`.** Rejected: that makes the *view* the authority on the
  mixer's width, which is the opposite of this shell's rule that the log (and its fold) is the
  source of truth. The fold decides how many strips there are; the snapshot only supplies levels.
- **Make the host publish a channel-level vector sized to the mixer width.** Attractive and probably
  right long-term, but it is a host API change to paper over a view bug, and the recorder work is the
  live priority. Noted here as the option if unmetered strips ever become common rather than
  transient.
- **Leave it: the spike is not the shell.** Rejected — this is the second defect hands-on use found
  in ten minutes (the first being the [stuck faders](2026-09-30-iced-fader-sensitivity.md)), and a
  shell that does not start cannot answer the question the spike exists to answer.

## Consequences

- **The mismatch is now documented where it bit.** The comment on the indexing says which count comes
  from where, because "channels" in this shell means two different numbers and the next reader will
  otherwise assume they are the same one.
- **The invariant is stated: the fold sizes the console, the snapshot only fills it.** Any future
  meter, readout or strip added to this console inherits that rule.
- **A class of bug, not one line.** Both counts are `usize` and both are called `channels`, so nothing
  in the type system stopped this and nothing will stop the next one. Where a fold count and a
  snapshot vector cross, the vector access wants `.get()`.
- **The spike keeps earning its keep.** Two hands-on defects in the first minutes of use is the
  strongest argument yet for the evaluation note's premise — the parts most likely to be worse are
  the ones only hands reveal. Neither would have been found by building or testing.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
