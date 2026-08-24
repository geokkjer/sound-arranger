# kimi review — P1.3.1 ArrangerNode (slice 2, 2026-08-24)

Session: `session_381d3e5b-b64d-46a6-a883-76dc84259a6d`

kimi reviewed the `ArrangerNode` (slice 2) before commit. The graph scan, fade math,
loop reader, and no-alloc path are sound; the soft spot is the **streaming/alignment
contract**. Its verdict, accurate:

> The graph-scan and DSP shape are good; the streaming/alignment contract is where the
> slice is soft.

## Must-fix (both integrated)

1. **An underrun permanently slips the stream — replay is not byte-identical once one
   occurs.** `pop_sample` returned 0.0 on a ring-empty-but-not-EOF without advancing
   `popped`; the next successful pop then delivered the frame of the *skipped* sample,
   so every later sample of that clip shifts by the cumulative underrun count while `off`
   (and the fade ramp) stay timeline-locked. Two runs with different scheduling → different
   audio. An SPSC ring has no random access, so you cannot skip a frame you never received.
   **Resolved by scoping the guarantee honestly:** byte-identical output requires a reader
   that never underruns; an underrun is counted and is a hard error the bounce must surface
   (assert `underruns == 0`). A `debug_assert_eq!(reader.popped, off)` (guarded on "reader
   still has data") catches a slip in tests instead of shipping shifted audio.
2. **`popped == off` holds only under contiguous-render-from-frame-0.** No seek exists; if
   the first rendered block begins above a clip's `at_frame` (offset bounce / punch-in /
   future seek), the reader serves source frame 0 while `off` claims otherwise — silent
   misalignment, no error. **Resolved by documenting the constraint** (render must be
   contiguous from frame 0) and letting the `popped == off` assert fire on a violation.

## Should-fix (integrated)

- **Loop test was blind to a broken wrap** (`loop_region_repeats_source` used a constant
  signal, so it passed even if `start_looped` never sought back). Now uses a ramp and
  asserts the wrap points (`out[100]==out[0]`).
- **Gap test didn't verify exact alignment** — now asserts `out[at_frame + k] == source[k]`
  against the file content.
- **`popped` is now used as the alignment guard** (was only the EOF check).
- **`out.len() > BLOCK` was a latent panic** — `debug_assert!` at render entry.
- **The underrun test exercised EOF, not starvation** — renamed to `eof_is_silence_not_panic`;
  a true starvation case is a scheduler/timing concern (covered by the alignment assert).

## Worth-considering (left as flags)

- Per-block `HashMap<Id, _>` string hashing on the render path is alloc-free but SipHash over
  a `String` per clip is needless; two aligned `Vec`s built once in `new` would be faster.
- One parked reader thread per clip lives until node drop; long arrangements with many short
  clips scale badly (a Phase-1 non-issue, flagged).
- Loop-clip fades apply over the whole baked `src_len`, not per pass — confirmed as the
  intended "per-clip authoritative" semantics.
- The loop reader math (`stream.rs` `start_looped`) was checked specifically and is correct
  (no over-read, no double-wrap, deterministic EOF break).

The full critique text is the reviewer's response in this session.
