# The clip arranger — an arrangement is data

> 🕒 Last verified against commit `2f6ad78` (2026-09-30). If the code has moved on,
> trust the code and move this line forward.

**What this is.** The engine ([`architecture-explainer.md`](architecture-explainer.md)) is a
minimal, model-free core: clock · graph · log · context. This doc is the *product* side of the
**[arranger profile](../.agents/notes/proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md)**
— what turns that core into an ACID-style clip arranger: the arrangement model, the ACID editing
ops, the node that renders it, the media pool it reads from, and the host wiring that makes edits
reach audio. (The platform's other profiles are the **recorder** — the current focus, owning clock
out, capture, alignment, mix, master and export — and the deferred **sculptor**.) If you've done
the FIRST_SESSION tour, you've seen the synth chain (euclidean→scale→tone→mixer). This is the half
that cuts, splices and rearranges *real* audio into a piece — the edit-as-composition half.

> Read the [overview note](../.agents/notes/implemented/feature/2026-08-24-p1-3-0-timeline-value.md)
> for why the model is *value-first*; this doc is the working explainer.

---

## 1. The headline: the timeline is pure data, rendered on demand

Nothing in the arrangement is "the audio." The arrangement is a **value** — a
`Timeline` (`crates/media/src/timeline.rs`) of **tracks**, each holding layered **clips**, where
a clip is a bounded region of a pool source placed on a track. That's it. It's completely
audible-preserving, diffable, loggable, and (because it's data) byte-identically replayable.

```
Timeline
 └─ Track { id, clips: Vec<Clip> }
     ├─ Clip { id, source, src_start, src_len, at_frame, fade_in, fade_out, gain, loop_len, name, reversed }
     ├─ Clip { ... }
     └─ ...
 └─ Track { ... }
```

A `Clip`'s fields all mean something precise (`timeline.rs`):
- **`source`** — a pool **id** (the file stem of an immutable float-WAV in the media pool, §4).
  It is *not* a path: a clip that could name a file outside the pool would make the document
  unportable and the pool unsealable, so the id is guarded (`valid_id` rejects separators and
  `..`) and the pool resolves it.
- **`src_start` / `src_len`** — which *region* of the source to read, and how long.
- **`at_frame`** — where on the track the clip starts (absolute sample frames).
- **`fade_in` / `fade_out`** — per-clip fades (frames), **authoritative** at boundaries.
- **`gain`** — per-clip gain (serialised bit-exactly in the log).
- **`loop_len`** — `Some(r)` means the source read wraps every `r` frames (a baked loop);
  `src_len` is then `r * times`.
- **`name`** — `Some(word)`, a human label with no semantics (ops and the render path key on
  `id`); it must be one word the `host v1` format can spell, or the session could not be
  reopened — which is why `RenameClip` refuses a space.
- **`reversed`** — play the region **backwards** (a clip property: the source stays immutable
  and the reader reads the other way). A reversed clip cannot be looped or re-looped.

Two helpers are the whole per-clip math: `end()` = `at_frame + src_len`, and
`source_frame_at(offset)` maps a position inside the clip back to a source frame, wrapping at
`loop_len` when set. Notice the timebase is **absolute frames**, not bars-and-beats — same
"seconds/samples timebase" decision as the engine's clock (§2.1 of the explainer), so tempo
changes never move stored positions.

---

## 2. The editing ops are a closed, logged vocabulary — ACID ops as data

Editing is a list of `ArrangeOp` variants (`crates/media/src/timeline.rs`), each a discrete
value-level mutation. This is the "everything the product does is data" discipline pushed into
the editor: there is no `timeline.mutate()` kitchen sink — there is a fixed set of ops.

| Op | What it does |
|---|---|
| `AddTrack` / `RemoveTrack` | add/remove a named track |
| `RenameTrack` / `MoveTrack` | rename a track / reorder it to an index (the mixer channel follows) |
| `AddClip` | place a clip on a track |
| `RazorSplit` | split a clip in two at `at_frame` (the "cut" gesture) |
| `Trim` | trim a clip's start or end edge by `by_frames` (negative = shrink) |
| `MoveClip` / `MoveClipToTrack` | move a clip in time / to another track |
| `Duplicate` | copy a clip under a new id |
| `Delete` | remove a clip |
| `SetClipGain` / `SetClipFade` | set the clip's gain / fade |
| `LoopRegion` | bake a loop (repeat the region `times`) |
| `ChopClip` | slice a clip into `times` contiguous pieces (ids derived from a `prefix`) — the auto-slice / chop verb; bounded (`MAX_CHOP_SLICES`), and the first/last piece keep capped outer fades so the result always renders |
| `Reverse` | play a clip's region backwards (a property flip, not a pool rewrite; not loopable) |
| `RenameClip` | name or unname a clip (one word — the format has no quoting) |
| `SetMarker` / `RemoveMarker` | named points on the ruler — logged, silent (they never change audio or export bytes) |
| `Stretch` | offline WSOLA time-stretch of a clip's material `num/den`, materialised into the pool as a **new** source (the op carries the new id and the rendered length) |

Crucially, **every entity-creating op carries the id it creates** (`new_left`, `new_right`,
`new_id`, `track`, `clip`, ...), so *all* ids are logged and deterministic. The whole model
reconstructs itself from the op stream — which is the engine's core invariant ("render is a
pure function of the log") applied to editing.

### 2.1 `ClipEditor.apply` — validate live, then log

The `ClipEditor` (`crates/media/src/clip_editor.rs`) is the control-side editor. `apply` does
two things, in order:

1. **Validate + mutate the live value fail-loud.** It locks the shared `Timeline`, and calls
   `Timeline::apply(&op)`, which *returns an error and changes nothing* on a refused op. A bad
   op is never applied and never logged.
2. **Log it.** It encodes the op into a `(&'static str, Vec<(&'static str, Value)>)` and hands
   it to `engine.arrange_logged(...)` — the engine's **closed-core message dispatch**. The op is
   logged but **never scheduled**; arrangement ops are value-level, so they don't touch the
   scheduler.

So an edit is a logged command that reconstructs the identical value on replay — the same
discipline as the engine's `Event` log, one level up.

> The "interner" (`Interner`) converts runtime id strings to `&'static str` for the log — the
> closed-vocabulary trick from the explainer, extended to user-generated ids.

---

## 3. `ArrangerNode` — one per track, a render-side interpreter

How does data become sound? `crates/media/src/arranger.rs`: each track gets one **opaque**
`ArrangerNode` (an `AudioNode`) that interprets that track's `Track` value on the render path.
It doesn't talk to the mixer — it just produces the track's audio at its `out("audio")` port,
and the **host** wires that into a mixer channel (§5).

Per block, the node:

1. **Selects active clips** — clips whose span `[at_frame, end)` overlaps `[frame, frame+BLOCK)`
   (a sorted-by-`at_frame` scan, no allocation).
2. **Streams one reader per active clip *instance*** — a source read at two different
   `src_start`s is two readers. Readers are opened and **warmed on the control side**
   (`ArrangerNode::new`), and only `try_pop`-ed on the render path.
3. **Sums overlaps** — layered clips sum; a gap is silence.
4. **Applies per-clip fades** — `fade_in`/`fade_out` ramps, authoritative (the auto-splice
   equal-power crossfade is **not** applied here, so there's no double-fade).

### The render contract, hard

- **No allocation, no blocking.** The node only pops rings and indexes preallocated buffers —
  the steady-state rule from the explainer (§3.4).
- **Rate-match is enforced, not assumed.** `ArrangerNode::new` opens each source and refuses a
  clip whose sample rate differs from the session. A rate-mismatched take would otherwise play
  pitch-shifted with no diagnostic — a bug that's invisible until you hear it. The refusal is the
  invariant guard, not the user's door: a **pool** is material at the session rate, and adopting it
  converts a foreign-rate source once (`Pool::conform`, original preserved) — see the
  [session-rate note](../.agents/notes/implemented/architecture/2026-09-22-session-rate-and-source-conversion.md).
- **Mid-transport rebuilds are positioned correctly.** `from_frame` computes `off0`
  (`from_frame - at_frame`, clamped) so a reader rebuilt while the transport is mid-clip
  continues the clip instead of restarting it.
- **Determinism is scoped to "no underrun."** Byte-identical output holds for a reader that never
  underruns. An underrun is counted and must be surfaced (`assert underruns == 0`), because an
  SPSC ring has no random access — you can't skip a frame you never received. The node
  `debug_assert!`s `popped + off0 == off` (except at a source's end-of-file, where `off`
  runs ahead into silence) so a slipped test fails loudly.

---

## 4. The media pool — immutable float-WAVs + peaks, crash-recoverable

Clips reference pool **source ids**, resolved to WAV paths by a `PoolResolver`. The
`Pool` (`crates/media/src/pool.rs`) is a directory of **float-WAV** sources + `.peaks` sidecars
(the live peak pyramids the UI will draw, §6.6 of the explainer).

- `Pool::list` enumerates `.wav` files (sorted, deterministic), with frame counts, sample rate,
  and whether the `.peaks` sidecar exists. A source that can't be read is reported in `errors`
  and skipped — never fatal to the whole index.
- `Pool::recover` finalizes **un-finalized** takes and rebuilds missing/corrupt `.peaks`
  sidecars. This pairs with the crash-recoverable WAV writer (the header carries placeholder
  sizes; a crashed take is formally well-formed after recovery). Recovery salvages only what
  is there — it never lengthens a file — and describes a take at the sample width its header
  declares, so a 24-bit take is recovered at 3 bytes a sample, not truncated; foreign WAVs
  with trailing chunks are left alone.
- **Ids are plain file stems.** `valid_id` rejects path separators and `..` so a *user-craftable*
  clip id can never escape the pool directory (a `..` traversal guard).

The pool is what makes `take-03.wav` → a clip id `take-03` → `clip_editor` → `ArrangerNode`
work, and why the whole recorder→pool→arrange pipeline is crash-safe.

---

## 5. The host — reconcile-on-dirty, not add-only

The host (`crates/host/src/lib.rs`) is where arrangement meets the engine's graph, and the
`wire_arranger` function is the clever bit. The reference host is
**reconcile-on-dirty**, *not* add-only:

- An `Arrange` op **mutates the value eagerly** and sets `arrange_dirty`.
- The **next render** re-derives the graph from that value (`wire_arranger`), instead of the
  UI/script having to explicitly tear down and rebuild nodes.

This is what makes an edit **reach audio** — and it fixes the `RemoveTrack` ghost (previously a
wired track was never retired, so removing it left its `ArrangerNode` mounted and audibly
playing while the UI showed no track).

`wire_arranger` does a strict **validate-before-commit**, mirroring the engine's two-phase rule:

1. **Read-only validation first** (nothing torn down on failure): materialize scheduled mounts
   (`flush_scheduled`), resolve the mixer **by name** (not `graph.out_node`, which points at
   whichever node last claimed the bus — misleading if the mixer was unmounted), and build every
   new reader (missing source, rate mismatch, too many tracks — the fallible part). Only if all
   succeed do we proceed.
2. **Commit (cannot fail):** retire the old wiring, then `insert_before(mixer)` each new
   `ArrangerNode` and connect its `audio` out to `mixer.ch{ti}`.

**Order matters** (the graph's topological rule from explainer §3.6): arranger nodes must precede
the mixer, so each is placed `insert_before(mixer)`, keeping `arranger → mixer` a forward cord.
Channels map to **track index** (`ch{ti}`), the same stable mapping as a fresh arrangement.

`flush_scheduled` materializes the mixer before the inserts (no discarded block). The whole
thing stays **control-side**: readers are warmed here; the render path only pops.

Unmounting the mixer is the mirror case: `unmount mixer` **retires** the arranger wiring —
the per-track nodes are removed from the graph, their underrun counters cleared, and the
arranger marked as awaiting a bus — so a later `mount mixer` rebuilds the wiring instead of
finding cords aimed at a node that no longer exists (and reader nodes holding files open for
nothing). Before the 2026-09 hardening a remounted mixer had *no inputs at all*.

---

## 6. Driving it — the text format

The versioned host script spells the ops directly:

```text
host v1
mount mixer channels=2 @0
pool /data/takes
arrange add_track t0 @0
arrange add_clip t0 c0 s1 0 48000 0 0 0 1.0 @0
bounce 48000 /tmp/out.wav
```

`pool <dir>` sets the resolver; `arrange <op> … @<frame>` is one `ArrangeOp`, logged at that
frame. Because the whole arrangement is a value reconstructed from the logged op stream, the
same script bounces byte-identically — the CLI smoke binary (`cargo run -p host`) exercises the
clip editor end-to-end with no GUI.

---

## 7. Honest corners

- **One log, two livenesses.** Arrangement ops and media commands (`pool`/`play`/`splice`/
  `bounce`/`export`) all log events in the **one** engine log — an `Arrange` event carries its
  op and fields; a media command logs its record — so determinism is one system, not a
  discipline bridging two. The corner that remains: arrangement ops are logged but *never
  scheduled* (value-level, applied on rebuild), while media commands run eagerly — the log is
  the document, and the split is liveness, not truth.
- **A reader thread and a 512 KiB ring per clip, per edit.** Each rebuild re-warms one reader
  (and its thread) per active clip; sharing or lazily mounting readers is deferred because the
  fix is architectural, not local — recorded with a reason in
  [the review disposition](../research/architecture/2026-09-29-space-bunny-review.md).
- **Reader reuse is deferred.** After an edit the arrangement is rebuilt and readers re-warmed
  from the transport frame; the shared-state `ArrangerNode` reuse (reusing a reader across
  rebuilds without re-warming) is still deferred — see the README's honest gaps.
- **Mono inputs, stereo master, no effects** — the mixer pans mono channels into a stereo
  master. Stereo *material* now arrives as one mono pool source per channel (import splits a
  multi-channel file as `{id}.ch0`/`{id}.ch1`, placed on two panned tracks), but a genuine
  stereo *clip* (one source, two channels) is still forthcoming, and there are no effects
  (the "sound sculptor" offline profile is separate and deferred).
- **The snap grid is shell state, not document state.** `b` arms off → bar → beat → 1/2 → 1/4,
  every positional edit quantizes through the session's tempo map before the command is built,
  and a script asks for the same with `snap=<frames>` on the line. The log therefore still holds
  absolute frames and nothing about the grid is stored — which is what keeps replay a pure
  function of the log. The grid resets per run (a shell preference, not yet persisted).
- **Underrun surfacing is a hard requirement.** A bounce with `underruns != 0` must be treated
  as an error, not a quiet glitch.

---

*Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-08-27; re-verified against
`7c5a2e7` with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-05; re-verified against
`2f6ad78` with GLM-5.3 · OpenCode, 2026-09-30.*
