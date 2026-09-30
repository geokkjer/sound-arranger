# Agent Note: stereo material — one pool source per channel (alpha slice A4)

Status: implemented

## Problem

The pool's own convention was already per-channel — capture writes a take as
`{take_id}.ch0.wav`, `{take_id}.ch1.wav`, … because the device hands over interleaved
frames and each channel is its own source — but **import did not follow it**: a stereo
WAV was copied into the pool as one file, and every pool reader (`WavReader`) yielded
*channel 0 only*. So "load clips from the pool" silently threw away the right channel of
any stereo file (a mix, a two-mic take, an exported stem), and the arrangement had no way
to place it. The alpha's first user-visible promise — confidently cut and paste audio
from the pool — was half-true for exactly the material a musician is most likely to
bring in.

Two smaller walls sat in front of it: the reader refused more than two channels at all
(`unsupported channel count 6 (mono/stereo only)`), and its interleaved chunk buffer was
sized for two channels, so a wide file would have overrun the stack had it been allowed
through.

## Decision

**A pool source is mono, and a multi-channel file is split at the import boundary.**

- `WavReader::with_channel(k)` selects which channel `read_into` yields (checked against
  the file's channel count). The reader now accepts any channel count a recorder wrote, up
  to `MAX_CHANNELS` (256 — a frame must fit the reader's 1024-byte interleaved chunk, and
  the file's declared block-align is re-derived rather than trusted): the chunk is sized by
  `block_align`, so channel *k* of a 5.1 file decodes without overrunning, and a frame too
  wide for one chunk is **refused at open** rather than read without progress (zero channels
  too — a frame the reader cannot step over would spin).
- `WavReader::read_into` reads **whole frames** (`read_exact`). A short read used to be
  divided by `block_align` and the remainder dropped, which left the stream pointing
  mid-frame — every later sample would then come from the wrong channel.
- `Pool::import -> Import { id, channels, sources: Vec<Conform> }` writes one mono source
  per channel as `{id}.ch0`, `{id}.ch1`, … at the session rate (resampling each channel
  when the file's rate differs), exactly the naming capture already writes. A mono file
  keeps the old behaviour: `{id}`, copied byte for byte when the rate already matches.
  `sources` is always complete — one entry per pool source the material has, the same-file
  path included — because a shell places tracks by walking it.
- **Every write is staged, then renamed**: each channel (and the mono copy) is rendered to
  `{dest}.converting` first and only moved into place once all of them exist. A failure or a
  crash part-way through therefore never leaves a torn file where a source is expected, and
  never leaves a half-updated split — the leftover `.converting` file is not a `.wav`, so
  `list` does not index it. (The earlier draft wrote channels directly to their final names,
  which a crash could leave as an un-finalized take that `Pool::recover` would then
  "finalize" into a truncated channel.)
- `PoolSource.channels` reports the file's channel count, so a UI (or `conform`) can see
  a source that is not yet mono.
- **Importing over an id replaces it**, the multi-channel case included:
  `replace_sources(id, keep_channels)` drops the old whole-file source **and every
  `{id}.ch{k}` at or beyond `keep_channels`**, the number of channel names the incoming
  import writes — the channel count for a multi-channel import, which overwrites
  `ch0..channels-1`, and **0 for a mono import**, whose material lands as `{id}.wav` and
  keeps no channel name (so a split's `ch0` goes with the rest). Dropped after the new
  material is staged, so a failed import changes nothing. Without it, a stereo file imported
  under a stem that held a 5.1 take would leave `jam.ch2`…`jam.ch5` addressable, and a clip
  on them would play audio the user had just replaced; the [mono
  fix](../bug-fix/2026-09-29-a-mono-import-leaves-no-channel-of-the-take-it-replaced.md)
  covers the same failure in the other direction.
- `Pool::conform` **expands** a multi-channel source in place (hand-filled or legacy
  pools): the extra channels are written beside it as `{id}.ch{k}`, and `{id}` is
  rewritten as its channel 0 — preserving the original as `{id}.wav.pre{rate}` (or
  `.pre{channels}ch` when only the split rewrote it), numbered if that name is already
  taken by an earlier preservation. **`{id}` keeps meaning channel 0**, so a clip that
  referenced the file before the split still plays what it always played, and the other
  channel becomes addressable instead of silently dropped. The pass is idempotent because
  the split marks the source mono, which takes it out of the filter; a **sibling is always
  re-derived** (staged and renamed, never trusted because the name exists), so a torn
  channel from an interrupted run is healed rather than presented as audio.
- The TUI's `--wave` places each channel of an imported file on its own track and pans it
  (`ch0.pan -1`, `ch1.pan 1` through the host's own script), so opening a stereo file
  plays it as stereo. `--probe` now prints **every** mounted channel's meter; `ch1`
  moving is the non-TUI proof.

This is a *material* rule, not a *signal-path* rule: a clip is still one mono source
read straight into one mixer channel, and the graph's per-port stereo signal
(`Port::stereo`) is still unused by the timeline. Stereo arrives as two mono clips
panned apart, which is also what makes it editable: the channels can be cut, gain-staged
and panned independently, and no reader ever has to de-interleave on the audio path.

## Alternatives considered

- **A stereo *clip* (one source, two channels, a stereo port)** — the "genuine" route and
  the one the [stereo-pan
  note](../../../../.agents/notes/implemented/architecture/2026-09-05-stereo-pan-foundation.md)
  reserved a seam for.
  Rejected for the alpha: it makes the clip the unit of channel-ness, so every arranger
  node, ring, gain, fade and clip edit grows a channel dimension before any of them is
  finished, and the capture path (already per-channel) would need a second shape. The
  per-channel split reuses the convention the recorder already proves daily, and a
  stereo clip can still be added later as a *view* over two sources.
- **Keep reducing to channel 0 and document it** — rejected: it is silent data loss in
  the one workflow the alpha promises, and the fix is at the boundary, not in the
  arranger.
- **Downmix stereo to mono on import** — rejected: destroys the stereo image
  irreversibly, and a downmix is a mixing decision, not an import decision.
- **Split on read (de-interleave in the reader)** — rejected: the audio path must not
  allocate, and every reader (arranger, stream player, peaks) would pay for a capability
  only import needs. Split once, at the boundary, like the rate conversion.
- **Refuse multi-channel files above two** — rejected: the reader change is small
  (channel pick + chunk sizing) and refusing a 5.1 take means the user must convert
  before the tool will look at it — the same wall 24-bit PCM used to be.
- **Rewrite `{id}` out of existence on a split (leave only `{id}.ch{k}`)** — rejected:
  it breaks every clip that referenced `{id}` (a loud arrange failure, or worse a silent
  hole if the source were re-resolved). Keeping `{id}` as channel 0 makes the split
  additive.

## Consequences

- A stereo (or 5.1) file imported with `--wave` or `Pool::import` now plays **both**
  channels, panned to their sides; the right channel is no longer dropped.
- The pool is uniform: every source is mono and at the session rate, so a clip is still a
  straight read and the mixer is the only place a channel decision is made.
- Idempotence matters for `conform`, because it runs on **every** pool adoption: a split
  marks the source mono (`channels == 1`), which is what takes it out of the filter.
- Tests: a stereo import produces two sources whose samples are each channel's own; the
  reader picks channel *k* of a six-channel file and reads past one chunk boundary
  (the sizing fix), and refuses a frame wider than its chunk; `conform` splits a stereo
  source, preserves the original, is idempotent, and resamples-and-splits in one pass;
  a narrower import removes the older channels, and a **mono** import over a split removes its
  channels as well (`a_mono_import_removes_the_older_split_channels`); a **torn sibling is
  re-derived** (a never-finalized `jam.ch1.wav` holding the wrong audio comes back finalized and
  correct); an existing backup is not clobbered; at host level a stereo file adopted into the
  pool is arranged on two panned tracks and the **bounce** carries 440 Hz on the left and 880 Hz
  on the right, so a dropped *or swapped* channel fails the test.
- Three defects were found by **self-review before the gate**, all now fixed and tested:
  (a) a file whose declared channel count made one frame wider than the reader's chunk
  buffer (`want == 0`) made `read_into` **loop forever** — the ceiling is now enforced in
  `parse_header` and the loop returns rather than spinning; (b) importing a stereo file under
  a stem that already held a mono source left the **stale** `{id}.wav` in the pool; (c) the
  TUI's pool tests shared one session pool per process without serializing, so one test's
  `drop_pool()` deleted another's material mid-import.
- **The independent gate (GLM-5.3 stand-in, `merge with changes`) found five more, all
  fixed** ([review + disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-a4-glm-standin.md)):
  the stale-channel removal stopped at the whole-file source (`jam.ch2`… of a replaced 5.1
  take survived) → `replace_sources` by channel count; channels and siblings were written to
  their final names, so a crash left a torn take that `recover` would "finalize" into a
  truncated channel → **staging plus rename**, and siblings are re-derived instead of
  skipped; "a failed split leaves the pool as it was" was false past the first channel → the
  same staging makes it true for new files; a same-file multi-channel `Import` reported only
  `{id}` (the shell then mounted one track) → one entry per source; an existing
  `{id}.wav.pre{rate}` was silently truncated by the backup fallback → numbered backups. The
  gate also caught a documentation error (the channel ceiling is 256, not 512) — corrected
  here and in the plan.
- **Still deferred**: the graph's per-port stereo signal and stereo clips (above); a file
  with more channels than the mixer can place (the reader takes up to 256 and import splits
  them all, but `MIXER_CHANNELS_MAX` = 8 caps how many tracks a session can hold, so the
  shell's mount is refused with the host's own message — verified with a nine-channel file);
  a channel-map/name convention beyond `.ch{k}` (no "left/right" semantics — that is a pan
  decision in the arrangement); RF64 for >4 GiB takes; **the shells do not report
  `pool_conformed`** (a load that splits or resamples a pool rewrites files in place and says
  nothing — `--wave` reports its own import, a `pool`/session load does not; `HostOutcome`
  carries the list, so this is a status-line change, not plumbing).

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
