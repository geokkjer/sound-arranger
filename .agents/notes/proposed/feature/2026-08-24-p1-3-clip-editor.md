# Agent Note: P1.3 — the clip editor (phase-plan note: "its shape is a dedicated note when it starts")

Status: proposed

Design reviewed by kimi before any code (2026-08-24) — the must-fix/should-fix
findings are integrated below and recorded in
[research/architecture/2026-08-24-kimi-review-p1-3-design.md](../../../../research/architecture/2026-08-24-kimi-review-p1-3-design.md).
Verdict on the central shape: sound.

## Problem

Phase 1 has a mixer and a recorder, but **no arrangement**. The host's media path is an
ad-hoc single-player: `HostCommand::Play { clip, channel }` starts one `FilePlayer`
routed into a mixer channel, and `HostCommand::Splice` crossfades the playing clip to
another (`PlaybackNode` + `SpliceCmd` in [`stream.rs`](../../../../crates/media/src/stream.rs)).
There is no *timeline value* — no notion of clips placed at frames on tracks, no ACID
editing, no pool a UI can enumerate. The clip editor must make "record → cut/paste →
rearrange into a piece" real, and it must do so as **logged commands** (the core rule:
*model-visible means logged*), because the profile's promise is byte-identical replay.

Two core-shape facts block a naive "add a clip plugin":

1. **The graph model cannot express an arrangement as nodes.** A clip is a *region* of a
   disk source placed at a frame on a track. Mounting one `AudioNode` per clip explodes
   the graph (small by design, topological, one audio Out per node, no-alloc) and turns
   every edit into node surgery.
2. **Media commands are not logged.** `Play`/`Splice` bypass the core log. The clip
   editor must land the arrangement in the log, or replay cannot reproduce the piece.

## Proposal

### The shape decision: the clip timeline is a **graph value**, not per-clip nodes

A **`Timeline` value** (immutable, `PartialEq`, FP-shaped) is the arrangement; a small
number of opaque **`ArrangerNode`s** interpret it — one per track. An ACID edit is a
**pure transform** `fn apply(timeline, op) -> Result<Timeline>`; the edit ops are logged
events, so the log *is* the op list and replay recomputes the identical value.

**Why the value form wins** (vs per-clip nodes): editing is value transformation not
graph surgery (testable, undoable via an immutable value, free of forward-order/no-alloc
worries); the graph stays constant-sized (one node per track); `plugin boundaries produce
values the interpreter reads`; and the existing `PlaybackNode`/`SpliceCmd` becomes a
special case (contiguous non-overlapping clips), so its equal-power boundary crossfade is
reused, not discarded. **Rejected: per-clip nodes** — graph grows with clip count, edits
are node surgery, and there is no single place to own the value or undo.

### The data model

```
Clip     { id, source: PoolId, src_start: u64, src_len: u64,
           at_frame: u64,          // position on the track (timeline)
           fade_in: u32, fade_out: u32, gain: f32 }
Track    { id, clips: Vec<Clip> }           // layered; overlaps sum (per-region fades apply)
Timeline { tracks: Vec<Track> }
```

- `PoolId` is a **content-hash id** into the media pool (immutable float-WAV source +
  `.peaks` sidecar). A clip references the *sound*, not a path.
- **Clip `id` is deterministic** — a pure function of the op stream (e.g. a sequential
  counter, or a hash of the producing op index), never random/UUID/time. Later ops
  reference `clip_id`, and replay recomputes the value, so ids must be reproducible.
  The two halves of `RazorSplit` and the clone from `Duplicate` derive ids the same way.
- Clips on a track **layer and overlap summing**; each clip carries its own
  `fade_in`/`fade_out` (per-region end fades). A gap is silence.
- **Clip gain** is a per-clip f32 with a logged `SetClipGain` op. Track gain is the
  *mixer channel* the track routes to (the arranger does not own a track fader).
- Mono for now (stereo arrives with the stereo step; the pool and graph are mono today).

### Logged commands (ops) — the ACID set

Each op is logged with `at_frame` (its effective frame) and applied by dispatcher to the
`Timeline` value through a pure function. A **refused op is never logged** — fail-loud
(patch-bay rule). Must-fix items from the review are written in.

- **`AddClip { track, clip }`** — place a clip at a frame.
- **`RazorSplit { track, clip_id, at_frame }`** — split one clip into two at a timeline
  frame (both halves share the source region); new half ids deterministic.
- **`Trim { track, clip_id, edge, by_frames }`** — `edge ∈ {start, end}`, `by_frames`
  signed; trimming `start` moves `at_frame` and `src_start` together; trimming `end`
  shortens `src_len` (and the timeline length). Exact semantics are the logged op's.
- **`MoveClip { track, clip_id, at_frame }`** — drag on the canvas within a track.
- **`MoveClipToTrack { from_track, clip_id, to_track, at_frame }`** — drag across tracks.
- **`Duplicate { track, clip_id }` / `Copy{..}` + `Paste{..}`** — clone a clip (new id).
- **`Delete { track, clip_id }`** — remove.
- **`SetClipGain { track, clip_id, gain }`** — per-clip gain (logged; f32 canonical).
- **`SetClipFade { track, clip_id, fade_in, fade_out }`** — set per-clip fades.
- **`LoopRegion { track, clip_id, times }`** — **bake**: one clip with deterministic
  `src_len = times × region`. Fades apply to the *outer* clip; inner seams are hard
  (a click at the loop seam is documented; a loop-crossfade is deferred). A live loop
  *marker* is a performance feature, out of scope here.
- **`AddTrack { id }` / `RemoveTrack { id }`** — track lifecycle, now part of the op set.

**Fail-loud placement rule (the warm-race fix):** an op whose `at_frame` is before the
current render position is **refused** (never logged). A clip that reaches the playhead
before its reader is warmed yields **silence** (underrun counted, not audible), so the
*audio* remains a pure function of `(log, pool, frame, block)` — replay byte-identical.

### Replayability (the byte-identity argument, made explicit)

- The log is the op sequence. Replay applies each op at its recorded frame to a fresh
  empty `Timeline` → deterministic value → audio.
- **Equivalence class:** byte-identical replay holds over a **fixed (log, exact pool
  content-set)**. The pool is immutable *post-finalize*; `Pool::recover()` is a
  pool-mutating event (see below) that must be logged/acknowledged, not a silent repair.
- **Canonical payload encoding:** `f32` values in logged ops are serialized bit-exactly
  (`to_bits`); no float-formatting ambiguity in the log. Op *payloads*, not the value, are
  serialized — the value is never snapshotted per edit.
- **Undo is itself logged** — as inverse ops, or an explicit `CheckpointTimeline`/
  `RestoreTimeline` logged op. A UI-side snapshot swap without a logged op is *not*
  permitted: it would desync the visible model from the log and break replay.

### Where the interpretation lives: `ArrangerNode` (opaque, media)

One `ArrangerNode` per track, mounted as an opaque `AudioNode` (`out("audio")` per track),
routed into the mixer channel. For each block it:
- finds the clips active in `[frame, frame+BLOCK)` on its track,
- pulls each active clip's samples **per active clip instance** (a reader per *read
  position* — a source read at two `src_start` offsets is two readers), warmed off the
  render path and detached (never joined) on retire,
- applies `fade_in`/`fade_out` per clip, sums overlapping clips, and splices between
  consecutive clips at their boundaries.

**Boundary-fade precedence (the double-fade fix):** per-clip `fade_in`/`fade_out` are
**authoritative**; the auto equal-power crossfade applies only when both abutting
boundary fades are zero. A non-zero fade and an auto-crossfade never double-attenuate.

**No-alloc / determinism:** the render path allocates nothing. Active clips per track are
bounded by a declared budget; exceeding it **refuses the op**, fail-loud (the counting-
allocator test covers an arranger graph). A reader not yet warmed yields silence; the
**audio** result is deterministic, and only the underrun *counter* varies (instrumentation,
not output). This generalizes `PlaybackNode` (contiguous single-track) without replacing
its equal-power crossfade; the two share the ring-pop + equal-power-fade core.

### Log-visibility carve-out for graph-level media nodes (decided)

`ArrangerNode` is a **graph-level media node**, but its *state* is the `Timeline` value,
which **is** logged (as the ACID ops). So the node needs no additional logged state:
replay reconstructs it from the op log. The carve-out is that a media node is log-visible
**through the value it interprets**, not by logging its internals. The host's old
`Play`/`Splice` commands are superseded; the Host API migrates to the arrangement
commands (P1.3.4).

### Media pool enumeration + crash recovery

The pool is a directory of content-hash float-WAV sources + `.peaks` sidecars
(immutable; content-hash referenced). P1.3 adds:

- **`Pool::list() -> PoolIndex`** (a value the host UI's timeline reads): enumerate
  `.wav`/`.peaks` pairs with frame counts, sample rate, and peak pyramid. Peaks rebuilt
  when missing.
- **`Pool::recover()`**: rescan for un-finalized sources (a `.wav` whose header is still
  placeholder — a crashed take) and call [`WavWriter::recover`](../../../../crates/media/src/wav.rs)
  (patches the frame-aligned size, truncates a torn tail), then rebuild missing `.peaks`.
- **Content-set semantics:** a recovered take's *content* changes (torn tail truncated),
  so it is **not** a silent repair under the same content-hash id. Recovery either (a)
  re-hashes the source (new `PoolId`, clips referencing the old id are left unchanged and
  point at a now-absent source — fail-loud on use), or (b) is an explicit, logged
  session-fork event. The design picks (a) re-hash + fail-loud for P1.3, surfaced as a
  warning; the log records the recovery so the equivalence class is explicit.

### The event opening: change-patch for other events

The one remaining encoding choice is **decided** (kimi must-fix 9): the log grows a single
**`Event::Arrangement { op: ArrangeOp, at_frame }`**, dispatched through a handler the
clip-editor plugin registers. `RazorSplit` is *plugin* semantics, not a core concept — the
std-only core must not grow one variant per plugin op. An unknown op is **fail-loud** (never
logged), which replaces the exhaustiveness an explicit `Event`-per-op enum would buy. `op`
is a value enum, and the payload encodes `f32` bit-exactly.

## Alternatives considered

- **Per-clip nodes (mount one audio node per clip)** — rejected (above): graph-grow,
  node-surgery edits, fights the topological/no-alloc/small-graph model, no value for undo.
- **Serialise the whole `Timeline` into the log on every edit** — re-logs the arrangement
  per op and turns partial edits into snapshots. Rejected: the log is a command log, not a
  snapshot store. (A *periodic* logged `Timeline` checkpoint is a possible complement for
  long-session open cost, considered not required for correctness.)
- **Keep the host's `Play`/`Splice` and bolt a UI on top** — the ACID heart is absent;
  the clip editor *is* the arrangement. Rejected.
- **A separate `media` timeline the engine never sees** — split-brain; replay drifts.
  Rejected.
- **One `ArrangerNode` for the whole canvas (N tracks in one node)** — fewer nodes but a
  bigger opaque blob and a wider PDC/out surface (one audio Out per node in Phase 1).
  Rejected for now; a single-canvas node is a later consolidation.
- **Explicit `Event`-per-op encoding** — rejected above (see "event encoding"): makes the
  core depend on plugin vocabulary. The single-envelope + dispatch keeps the core closed.

## Acceptance criteria

- A `Timeline` value exists; clip `id`s are deterministic (pure function of the op
  stream); clips carry `{ id, source, src_start, src_len, at_frame, fade_in, fade_out, gain }`.
- Every op in the ACID set (incl. `AddTrack`/`RemoveTrack`, `MoveClipToTrack`,
  `SetClipGain`) is a pure `Timeline -> Result<Timeline>` and is logged with `at_frame`;
  a refused op (bad id/frame, active-clip budget exceeded, `at_frame` in the past) is never
  logged.
- Replay: the same (log, pool) on fresh engines produces byte-identical audio; a mid-session
  edit at frame F applies sample-accurately.
- `ArrangerNode` renders a track: contiguous clips splice sample-accurately; per-clip fades
  are authoritative (no double-fade); overlapping clips sum; gaps/silence; no allocation
  (counting-allocator test covers an arranger graph).
- Edit-during-playback never stops the stream (an op at the current frame takes effect with
  already-buffered audio continuing); a not-yet-warmed clip is deterministic silence with a
  counted underrun.
- Undo is logged (inverse op or logged restore) — the visible model and the log cannot diverge.
- `Pool::list()` returns a timeline-readable value; `Pool::recover()` finalizes crashed
  takes and rebuilds missing `.peaks`, re-hashes recovered sources (fail-loud on stale ids),
  and is logged.
- Bounce does not truncate a tailed arranger (drain/EOF — see the
  [drain-eof note](2026-08-20-drain-eof-phase.md)).
- Workspace tests green; clippy clean.

## Risks

- **Disk-streaming many active readers** — one reader per active clip instance; warmed
  before reach and detached on retire (as `FilePlayer::Drop`). Contention → counted
  underrun, deterministic silence.
- **The warm-race at the playhead** — mitigated by the fail-loud placement rule + silence-
  on-unwarmed; the residual risk is that a user pasting a clip "now" hears silence for the
  warm window (surfaced as an underrun/warning), which is the deterministic price.
- **Log growth on dense editing** — ops are discrete and gestures coalesce into one op
  (a drag is one `MoveClip`). Linear in gestures, not samples.
- **LoopRegion seams click** — baked loops hard-cut at inner seams (documented); loop
  crossfades are deferred.
- **Content-hash stability across recovery** — recovery re-hashes; stale references are
  fail-loud, never assumed to be the same audio.

## Slice plan (each: implement → kimi review → integrate → commit with note)

1. **P1.3.0** — `Timeline`/`Clip`/`Track` value + the pure ACID transforms + unit tests.
2. **P1.3.1** — `ArrangerNode` (media): render a track from the value; equal-power splice
   with the fade-precedence rule; overlap-sum; per-clip fades; gaps=silence; no-alloc +
   determinism + edit-during-playback + warm-race tests.
3. **P1.3.2** — engine: `Event::Arrangement` + dispatch + `at_frame`; replay applies the
   ops; the log-visibility carve-out lands as node-state = logged value.
4. **P1.3.3** — `Pool::list()` + `Pool::recover()` (+ `.peaks` rebuild + re-hash) + tests.
5. **P1.3.4** — migrate the Host API: swap `Play`/`Splice` for the arrangement commands;
   the reference host runs a clip-editor script byte-identically.
