# Agent Note: the utility gestures — reverse and four gain/trim transforms (alpha slice E1)

Status: implemented

## Problem

The arrangement could place, cut, paste and move clips, but it could not **shape** one: no
reverse, no normalize, no polarity flip, no way to strip the silence a take starts and ends with.
Those five gestures are the plan's answer to the owner's "some rudimentary audio manipulation
tools like reverse etc" — and four of them need no new vocabulary at all, which is why the plan
put reverse as the only new clip property.

## Decision

**Reverse is a clip property with a mirrored reader; the other four are logged gestures over ops
that already exist.**

- `Clip.reversed: bool` (serde-defaulted) and `Clip::source_frame_at` mirrors on it: the clip's
  first timeline frame is the region's **top**, so the read walks down from `src_start + src_len − 1`
  to `src_start`. The pool source stays immutable — this is a reader mode, not a rewritten file
  (`Reverse` *does not* materialise a new pool source, which is the same call the stretch slice will
  have to make the other way).
- `ArrangeOp::Reverse { track, clip }` is a **toggle**, so one press is one log line and one undo;
  pressing it twice restores the clip exactly. `add_clip` carries the flag in its encoding (the
  codec's exactness contract covers it — without the field, an `AddClip` with `reversed: true`
  encoded fine and decoded forward), and the **clipboard's paste emits its own `reverse` in the same
  gesture** for a reversed entry: a paste is a value copy that goes back through `add_clip`, so
  without that line it would silently play forwards. A **looped** clip is refused (the loop phase of a
  mirrored read is not representable — the same reason razor-split, trim and chop already refuse a
  looped clip), and `LoopRegion` refuses a reversed clip for the mirror reason.
- **The ops that compute source offsets mirror their arithmetic with it** — the part that is easy to
  get silently wrong (right length, wrong samples):
  - *Razor split*: in time the left half is the region's **top** (`src_start = c.src_start + right_len`)
    and the right half the bottom.
  - *Trim the start*: the clip's first frames are the top, so only `src_len` shrinks — `src_start`
    does **not** move (the mirror of the forward start trim).
  - *Trim the end*: moving the end earlier cuts the region's **bottom**, so `src_start` **rises** —
    the sign flips against the forward case (and this is the one the test caught: the first draft
    wrote `+by` where the mirror needs `−by`).
  - *Chop*: the pieces walk **down** from the top, so piece 0 in time is the top of the region.
  - *Split/trim/chop* all carry `reversed` onto their results, and the outer fades stay on the
    first/last pieces as before.
- The **panel** carries the flag too (`Placed.reversed` + `source_frame` mirrored), so the braille
  envelope draws what the engine will read — a reversed clip's waveform is mirrored, which is the
  only way the picture stays honest.
- **Keys** (`V`, `U`, `i`, `E`, `T`), each one `host v1` line through the parser:
  - `V` — reverse (the status says "plays backwards" / "plays forwards").
  - `U` — normalize: read the clip's **source region** from the peak pyramid the panel already holds
    (no re-read of the audio), set `gain = 1/peak`, and cap at the console's +12 dB ceiling (saying
    so when the cap bites). A source that is silent is refused rather than amplified to noise.
    `peak` is `max(|min|, max)` — the pyramid returns the *signed* extremes, and a signal that swings
    mostly negative (or a polarity-inverted clip) has its loudest excursion in `min`; reading only
    `max` would normalize a −0.9 peak as if it were silent and clip the result. The region is what
    the reader **plays**: one loop for a looped clip (`src_len` is `loop_len × times`), not the
    repeats.
  - `i` — invert polarity: `gain = −gain` (a gain of −1), reversible with the same key.
  - `E` — silence: `gain = 0`, which keeps the clip's span, fades and place (delete is a different
    key and a different intent).
  - `T` — trim to content: scan in from both edges of the region one peak bin at a time
    (`Source::peak_of`, so a negative-only bin is content, not silence) until a bin is above
    −80 dBFS, then trim both edges to it in **one gesture** (one undo). The scan is in source
    frames; the timeline deltas are **mirrored for a reversed clip**, so "the start of the clip"
    always means what the listener hears first. If the new length would invalidate the clip's fades
    (`fade_in + fade_out <= src_len` — a clip whose fade-in fills it is reachable with `f` and with
    paste), the fades are **capped to the new length first in the same group** and the status says
    so: the host validates each op against the running value, so the cap has to precede the trims.

## Alternatives considered

- **Reverse as a rewritten pool source.** Rejected — the plan says so explicitly, and it is right:
  the pool is immutable session material, a second copy doubles disk for a *view* change, and the
  clip's identity (`source` + region) would stop meaning what it says.
- **Reverse as a `set_clip_reverse <0|1>` op** (absolute rather than a toggle). Rejected: a toggle is
  what the key means, it needs no reading of the current state to build the line, and replay is
  still one fact per press (`undo` is exact). An absolute op would make a key press depend on the
  shell's picture being current.
- **Allowing reverse on a looped clip** (and defining the phase). Rejected for the same reason the
  existing ops refuse loops: a loop phase that is not representable must fail loud rather than be
  approximated.
- **Normalize from the rendered audio** (read the file and measure exactly). Rejected: the peak
  pyramid is already loaded, is what the panel draws, and its bins bound the error at one bin —
  which is what a normalize decision tolerates. A sample-exact normalize would re-read every clip
  on a keypress.
- **Normalize to a target LUFS / headroom** rather than full scale. Deferred with the rest of the
  metering work: full scale is the honest alpha default, and the plan puts loudness in the
  mastering slice.
- **Normalize/invert/silence as new ops.** Rejected: they are all one `set_clip_gain`, which the log
  already carries; inventing `normalize` as an op would put a *measurement* in the document.
- **Trim-to-content in timeline frames only** (no mirror). Rejected: on a reversed clip that trims
  the wrong edge, which is exactly the bug class this slice's mirror handling exists to avoid.
- **A per-clip "content" cached on the clip.** Rejected: it is a measurement of the source, and the
  log's clip is intent (the same reason the pool is not rewritten).

## Consequences

- A clip can be reversed and reshaped, each gesture one line in the log, one `u` from undone, and
  the panel's picture follows the reader.
- The mirror arithmetic is pinned by a test that walks every op: the mapping itself, the toggle and
  its refusals, split (left is the top), both trims (including the sign flip), chop (pieces walk
  down), and the reversed `trim_to_content` (the two edge removals **swap exactly**).
- `TrimToContent` is a compound gesture, so it exercises the A1 group machinery from a new angle:
  one undo restores both trims.
- **Recorded limitations**: the panel's `Placed::source_frame` clamps to the source's last frame
  where the engine does not (a hand-written region past the file draws the last frame repeated
  instead of silence — a pre-existing divergence the reversed case inherits, and the *scans* clamp
  sanely); the peak bins are 256 frames wide and cover whole bins *overlapping* the range, so loud
  source material **adjacent to** a clip's edge can stop the scan at that edge (within the one-bin
  error the scan already tolerates, but it is worth naming); `i` on a silent clip says so instead of
  reporting a no-op inversion.
- **Still open**: time-stretch/tempo match (the plan's item 11, the other half of phase E);
  a *sample-exact* normalize and any loudness target (mastering, phase F); reversing a *looped*
  clip; a per-clip "reverse" affordance beyond the key (the pool panel has none for clips); fading
  across a reversed split seam is still a manual `set_clip_fade` on each half.

## The gate's findings

The slice's gate returned `merge with changes`
([review + disposition](research/architecture/2026-09-24-alpha-slice-gate-e1-glm-standin.md)):

1. **Normalize and trim-to-content read only the positive half of the peak** (must-fix) — the true
   peak is `max(|min|, max)`, so a −0.9 excursion read as `max = 0.25` gave a gain of 4 and clipped,
   and a negative-only bin was trimmed away as silence (audible content deleted). Fixed with
   `Source::peak_of` in both scans, tested on a constant −0.9 fixture.
2. **A pasted reversed clip lost its direction** (should-fix) — `add_clip` has no direction operand
   and the parser/decoder pin `reversed: false`, so the paste played forwards in-session and after a
   reload. Fixed: the paste emits a `reverse` line for each reversed entry in the same group; tested.
3. **`AddClip`'s encoding was silently lossy for `reversed: true`** (should-fix) — it encoded fine
   and decoded forward, narrowing the codec's exactness contract. Fixed: the field is carried (and
   the round-trip test now covers a reversed clip).
4. **`U` measured a looped clip's repeats** (should-fix) — `source_region` now returns one loop.
5. The panel/engine clamp divergence and the bin-granularity edge are recorded above as limitations.
6. **`T` on a heavily-faded clip failed with an opaque validation error** (note) — the fades are now
   capped first in the gesture, and the status says so.
7. **`i` on a silent clip** reported "already inverted" (note) — it now says silence has no polarity.
8. The reviewer's addendum claimed iced's new keys are "silently dead" (note) — **refuted**: the iced
   shell's key translation maps characters into the shared `WorkflowKey` and its `on_key` calls
   `workflow::action`, so `V`/`U`/`i`/`E`/`T` reach `dispatch` and its catch-all reports that the
   iced timeline canvas is not built (spikes/iced-shell/src/main.rs:333-371, 446-452). The `_ =>
   return None` the reviewer read is the translation's fallthrough for *unmapped iced keys*
   (function keys), not for the workflow's characters.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
