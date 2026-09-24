# Agent Note: the alpha finish line — the arrange loop, end to end

Status: proposed

## Problem

The owner wants to push the project from pre-alpha to a state where arranging is **confident**:
cut and paste audio (append, snap), make new tracks, snap to grid, time-stretch/tempo-match, load
clips from the pool, and export a finished mix with simple mastering (a compressor on the mix out) —
plus "rudimentary audio manipulation tools like reverse".

The substrate is not the problem. `crates/engine` (clock, graph, log), `crates/media` (pool,
streaming, capture, arranger node, resampler) and `crates/host` (the `host v1` vocabulary + live
actor) already carry the hard parts, the ACID op set exists (`razor_split`, `trim`, `move_clip`,
`move_clip_to_track`, `duplicate`, `delete`, `set_clip_gain`, `set_clip_fade`, `loop_region`, `chop`),
and the TUI dispatches all of it by key or through `: `. **The owner under-credits what exists.**

What is missing is the **life around the log** — and the co-work passes found the gaps are bigger, and
one of them much earlier, than the owner's list implies. The most important finding: **recording is not
wired into the host at all.** `HostCommand::Record` returns
`Err("recording requires a device — … not wired into the host")` (`crates/host/src/lib.rs:537-539`)
and `media::Capture` is reached only by `media`'s own tests. The product's own description is
*record long generative runs, then arrange them*; the first half is unreachable from every shell.

This note is the scope and the order. It is deliberately a plan: the slices are the goal's rounds, each
a commit with tests + its own note, and the architectural ones pass the co-work reviewer gate.

## What exists, what is missing (verified)

| Capability | Today | The gap |
|---|---|---|
| Cut | `razor_split` + `delete` (`x`, `d`) | — |
| Copy / paste | **nothing** | No clipboard. `duplicate` stacks the copy at the *same* `at_frame` (`crates/media/src/timeline.rs`), so a paste is duplicate + move = three undo steps |
| Append | — | `add_clip` takes an explicit frame; no "after this clip" / "at the end" |
| Snap to grid | — | No grid, no snap. The tempo map exists; the ruler is **seconds**; `H`/`L`'s "one beat" lives in the TUI, so the *workflow* cannot express it (iced has no beat nudge at all) |
| Time-stretch / tempo match | — | Nothing. Resampling exists (import conformance) but duration-without-pitch is a different algorithm |
| New tracks | `add_track` in the log / `:` | No key/affordance in the TUI; no rename/delete/reorder; track → channel is fixed by index (`ch{ti}`) |
| Load clips from the pool | `pool`, `pool_sources`, `--wave`, `arrange add_clip` | No pool panel (browse + place + **audition**); the pool-before-arrange requirement is an error string, not a shell affordance |
| Export the mix | `bounce <frames> <path>` — 16-bit, deterministic, drain-checked | Needs a hand-computed frame count; 16-bit only; no normalize/loudness report; no stems; the documented `@frame`+bounce trap must not leak into the export UX |
| Mastering | — | No dynamics processing anywhere; the mixer is gain/pan/mute/solo/meters |
| Audio manipulation | — | Nothing: no reverse, invert, normalize, silence, trim-to-content |

**Gaps the owner did not list, which the co-work passes ranked as in-scope — the first four are
foundational, not polish:**

- **Recording into the host** (above). Verified: the wiring does not exist.
- **Session save/open.** The log lives in memory; there is no `save`. Arranging for a day and quitting
  loses the piece. The pool has crash recovery; the log has nothing.
- **Stereo material.** *Closed (2026-09-24).* A multi-channel file is now split at the import boundary into
  one mono source per channel, so a stereo jam imports as `{id}.ch0` + `{id}.ch1` and both are placed and
  panned (slice A4, [note](../../implemented/feature/2026-09-24-stereo-material-per-channel-pool.md)). The
  one-clip model stays mono per track; a genuine stereo *clip* is deferred.
- **Import breadth.** *Closed (2026-09-23/24).* The reader accepts 16-bit, **24-bit** and 32-bit float
  PCM/WAV, and any channel count up to the reader's chunk ceiling (256).
- **Seek cost on long material.** `TransportSeek` is `replay_to`: rebuild a fresh session, replay the
  history, render from zero to the target. Measured ~92 ms for a 6 s seek → **~28 s into a 30-minute
  jam**, and 30-minute jams are the workflow. The one gap that gets worse with use.
- **Markers / sections, and clip naming.** Navigating a 20-minute arrangement needs points to jump to and
  names to recognise.
- **Click-free edits.** Overlapping clips sum; a cut or paste through a waveform clicks without fades.
- **Auditioning.** The host's one-shot `play` command exists and nothing uses it for "hear this before I
  place it".
- **A view of the log.** The log is the document and the undo history; nothing shows it — and it is the
  cheapest window onto the LLM seam.
- **Grid-aware recording** (a click/count-in), take management/comping, fader-ride undo — beta.
- **Versioned-format discipline.** Every new verb is `host v1` growth: additive ops are compatible (old
  scripts load; a new script in an old host fails at the exact line), but a *changed meaning* needs `v2`.
  Recorded here so vocabulary growth is deliberate.

## Proposal

**The finish line:** the owner records a long jam *in the tool*, arranges it into a finished piece by
keyboard (cut/copy/paste/append with snapping, new tracks, tempo-matching a clip, loading pool material),
and exports a mastered mix — **without losing work and without leaving the tool**. Beta: a second person
can do the same from the docs.

Ordered so that nothing is built on sand. Each slice: tests + note + docs in the same commit, reachable
from the shared workflow (a key or a `: ` line — never a shell-only feature).

**A — the document has a life (foundations)**
1. ~~**Compound gestures — one gesture, one undo.**~~ **DONE** (2026-09-23): implemented as
   [`HostCommand::Group`](../2026-09-23-compound-gestures-one-undo-step.md) with the host history as
   entries; `t` in the TUI is the first gesture. The rest of the item is preserved below as the design
   of record. At the **host** level: `history: Vec<HostCommand>`
   becomes `Vec<Vec<HostCommand>>` (a bare command is a one-element group); a gesture opens a group,
   streams its `Arrange` commands, closes it, and the group applies all-or-nothing (folded over a clone
   first, so a refused member never enters history). The **engine log is unchanged**, so replay and the
   byte-identical bounce tests are untouched by construction; undo/redo move whole entries. Persisted text
   carries the structure as `group begin` / `group end` lines the parser accepts. *First because
   persistence journals per group and stretch logs as a compound op.* Fixes the standing
   one-gesture-two-undos bug (`t`) and the paste/append/utility gestures below.
2. ~~**Session save/open — a session is a directory.**~~ **DONE** (2026-09-23): `session.txt` (the log,
   loaded by the existing parser) + `pool/` + a journal for autosave, all three `host v1` lines
   (`session_rate`/`save`/`load`) — [note](../2026-09-23-session-directory-save-and-journal.md). The
   design below is preserved as the record. `mysong.d/session.txt` (the state-command history
   verbatim, with `set_tempo` and a **session-relative** `pool`) + `mysong.d/pool/` (content-addressed
   WAVs, including conformed and stretched material). Load is the *existing* `parse_script` — no second
   format, no second versioning. `Save`/`Load` are actions (like `Bounce`), written atomically (temp +
   rename). **Crash safety is an append-only journal**: every committed group is appended and flushed;
   autosave *is* the journal; a torn trailing line is dropped on load and reported. Not saved (rebuilt by
   replay): wiring, counters, the timeline snapshot, the clipboard, grid/UI state, bounce outputs.
3. ~~**Recording into the host.**~~ **DONE** (2026-09-23): `record <take_id>` captures the default input
   device into the pool at the session rate, `record stop` finalizes it, and a test drives the same seam
   with a ring instead of a device
   ([note](../2026-09-23-recording-into-the-host.md)). The gate's stand-in review returned `do not
   merge` on a real blocker (a mono input ring demuxed as multi-channel: every take silently corrupt);
   fixed in the slice, with a hardware-gated test. The design below is preserved as the record. Wire `media::Capture` (which already has drift compensation, crash
   recovery and per-channel writers) behind the existing `record <take_id>` line: the host opens the input
   device, mounts `CaptureNode`s into the pool, and the take lands as pool sources (`{take_id}.ch{N}`)
   that the pool panel can then place. This is the loop's missing first half.
4. **Material that is actually usable: stereo and bit depths.** *Done (2026-09-24):* the reader accepts
   **16/24-bit PCM and 32-bit float** (24-bit decoded to f32 on the control side; a 32-bit PCM tag is
   refused — only float is a format the pool writes, and guessing int-vs-float would be silent
   corruption). A **multi-channel file is split at the import boundary** into one mono pool source per
   channel (`{id}.ch0`, `{id}.ch1`, … — the capture convention, now the import convention too), so both
   channels of a stereo file survive and are placed independently; `conform` expands a legacy
   multi-channel source in place, keeping `{id}` as channel 0 so no existing clip breaks
   ([note](../../implemented/feature/2026-09-24-stereo-material-per-channel-pool.md)). A stereo *clip*
   (one source, two channels) stays deferred — the split makes each channel editable, which is what the
   alpha needs.

**B — the grid and the edit vocabulary (the owner's core ask)**
5. **Grid snap, in the language and in the shell.** *Done (2026-09-24):* `media::snap` holds the math
   (`Division`/`Grid` in the **beat** domain, `quantize_frames` for the parser), `workflow::Grid` holds
   the armed division (off → bar → beat → 1/2 → 1/4, `b`), and the shell quantizes every edit target
   through the session's own tempo map (`Snapshot::tempo_map`) — seeks, split, fade, trim and the
   clip-move target, with `[`/`]` stepping the playhead a grid line and `H`/`L` moving a grid step. The
   parser's additive `snap=<frames>` modifier snaps a *script* identically
   (`arrange move_clip t0 c0 48213 snap=480` → 48000) by quantizing at parse time, so the log still
   holds absolute frames and nothing about the grid is stored. An armed grid replaces the seconds ruler
   with a **bar/beat ruler** (numbered bars, beat ticks), and the state line always names the grid
   ([note](../../implemented/feature/2026-09-24-grid-snap-and-the-bar-beat-ruler.md)). Zero-crossing
   snap stays deferred (a second quantizer behind the same seam); click-freedom comes from default
   micro-fades instead.
6. **Clipboard: copy / cut / paste.** The clipboard is a shell-owned **value** (`Vec<(track_delta, Clip)>`
   normalised to the selection's first frame) and is **never logged** — copying changes nothing. Paste
   *is* logged, as one compound gesture with minted ids (`paste.{k}`, the `chop` precedent), onto a
   target track, failing atomically on an id collision. New boundaries get default micro-fades.
7. **Append** needs no new op: `MoveClip`/`AddClip` with `at_frame` = the preceding clip's end or the
   arrangement's end, computed shell-side. Value ops stay absolute-frame-only.
8. **Tracks**: add / rename / delete / reorder by key; the strip already exists as channel `ch{ti}`.
9. **Pool panel**: browse sources (name, length, rate, peaks), place at the playhead or on a track, and
   **audition** first. Also where the pool-before-arrange requirement becomes an affordance instead of an
   error string.

**C — shape and length**
10. **Utility gestures, mostly over ops that already exist**: **reverse** is the one new clip property
    (`ArrangeOp::Reverse` + a reverse-read mode in the reader — *not* a rewritten pool copy, which would
    break the pool's immutability premise); **normalize** measures the peak pyramid and issues
    `set_clip_gain`; **invert** is `set_clip_gain -1`; **silence** is gain 0; **trim-to-content** is a peak
    scan plus the compound `trim`. Four of the five are logged gestures over existing vocabulary, which is
    why they are cheap.
11. **Time-stretch / tempo match — offline, materialised into the pool.** A control-side render writes a
    **new pool source**, and the logged compound op rewrites the clip's reference
    (`arrange stretch t0 c0 <new-source> <new-src-len> <num> <den>`), so `Clip` gains no playback-rate
    property and the one-frame-domain rule survives: `ArrangerNode` never knows a stretch happened.
    **WSOLA-class** (overlap-add + correlation search): pure arithmetic, no allocation, transient
    behaviour that suits recorded material better than a phase vocoder. The **ratio is logged as
    `u32/u32`**, never an f32, so output length and phase are byte-reproducible; tempo match derives it
    from the tempo map plus a logged `source_tempo <id> <bpm>`. Refuses a looped clip (loop phase is not
    representable) and re-validates fades against the new length.

**D — finish the piece**
12. **Mastering chain, in the graph.** A `master` engine plugin (`HOST_PLUGINS`) mounted after the mixer
    claims the graph's output, so the live pump and the bounce both flow through it with no new code path
    (determinism inherited). Chain: compressor (`threshold`, `ratio`, `attack_ms`, `release_ms`, `makeup`)
    + a lookahead **brickwall limiter** (`ceiling`, default ≈ −0.4 dBFS) whose latency is declared through
    the existing PDC value, with gain-reduction and peak metering. One documented truth: `master.gain` is
    the fader, `makeup` belongs to the compressor.
13. **Export — a sibling of `bounce`, never an overload of it.** `bounce` keeps its 16-bit WAV, drain
    semantics and byte-identical test role. `export <path> [f32|s16]` renders the **whole arrangement** (no
    hand-computed frame count, and it renders from frame 0 so the compressor's ballistics are
    reproducible), writes **f32 WAV by default** (bit-exact, golden-file testable) or s16 with
    **fixed-seed TPDF dither** (deterministic, never thread-local randomness), reports peak/RMS, and
    **fails rather than writes a clipped file**. Stems and compressed formats follow.
14. **Markers/sections and clip naming** (logged lines + shell list + keys) so a 30-minute piece is
    navigable and recognisable.

**E — alpha → beta**
15. **Seek at scale**: checkpoints (a rebuilt session held at intervals, provably equal to a replay) so a
    jump into a 30-minute arrangement is interactive.
16. **Onboarding + honesty**: a first-session doc for the Rust shell, a capability/limitations statement,
    the beta acceptance checklist, and the README/RESEARCH refresh.

**Explicitly out of alpha:** CLAP/VST hosting and export (see the
[nice-plug note](2026-09-22-clap-export-via-nice-plug.md)), a sound-sculptor profile, network audio,
comping/takes, FLAC/MP3 import/export, TUI macros/registers, and the iced timeline. The iced shell keeps
the *same* workflow and reports the actions it cannot render yet: the parity rule is "no shell gets a
private feature", not "both shells ship together".

## Alternatives considered

- **Build the owner's list in the order given, without the foundations.** Rejected: save/open and
  compound-gesture undo are what make the rest safe; retrofitting one-gesture-one-undo after
  clipboard/append/stretch ship means reshaping the log with users' sessions in it.
- **Treat recording as out of scope because "the pool already has material".** Rejected: the product is
  *record → arrange*, the owner's rig is the source, and the wiring is a host-side job over machinery that
  is already built and tested. This is the one gap that makes the rest unusable in practice.
- **Realtime time-stretch in the reader.** Rejected: the clip model's `src_len` is both the source window
  and the timeline span; a per-clip ratio splits that one frame domain across the arranger, the peaks, the
  drawing and the snapshot, and a realtime stretcher would need lookahead buffering on the render path. A
  rendered derivative keeps one domain and is offline, so quality can be traded for time.
- **Reverse as a pool copy.** Rejected: a rewritten source is a *destructive* file workflow (the pool's
  premise is immutable, content-addressed sources) and it costs a file per reversal. A clip property plus
  a reader mode is non-destructive, logged and undoable.
- **An effect-plugin framework before any effect.** Rejected: a compressor and a limiter on the master are
  two `AudioNode`s; generalise when there is a third.
- **Persist as a bespoke serde snapshot rather than the log.** Rejected: the log already *is* a text format
  with a version header, a parser and replay tests. A second representation would need its own versioning
  and could disagree with the log.
- **Snap as a shell-only helper.** Rejected: the TUI's beat-nudge is already a workflow violation (iced
  cannot express it). Snap belongs in the language as well as the shell.
- **Buy time-stretch from a crate (rubato).** Kept open: the same "own DSP vs dependency" call as the
  resampler; decide after measuring the offline version's quality.
- **Chase feature parity in the iced shell during alpha.** Rejected: the TUI is primary; iced follows.

## Acceptance criteria

**Alpha (the owner, alone, in the TUI):**
1. A jam recorded **in the tool** (or imported: stereo and 24-bit included) can be cut into clips,
   copied, pasted, appended, snapped to the bar/beat grid, moved between tracks, reversed and
   tempo-matched — and **one gesture is one undo**.
2. Nothing is lost: quitting and relaunching restores the session (pool, log, tempo, rate), including
   after a crash.
3. The mix plays with the mastering chain on the bus; export renders the whole arrangement to a file that
   does not clip, with a reported peak/RMS.
4. Every step is a `host v1` line: the saved `session.txt` reproduces the session, and a paste, a stretch
   and an export are each **byte-reproducible** across runs.
5. Seeking anywhere in the 30-minute arrangement is interactive (< ~1 s), not a wait.
6. Both shells still speak one workflow; the iced shell reports, rather than silently drops, what it cannot
   render yet.

**Beta (a second person):** the same, from `docs/` alone, with no author help — their own files, a
different session rate, and an export they can hand to someone else.

## Risks

- **The log's shape is the expensive thing to change.** Compound gestures and any new clip property
  (reverse, stretch reference) are schema changes; doing them after users have sessions means migration.
  Hence phase A first.
- **Time-stretch quality is a research risk, not an implementation risk**: a bad stretcher is worse than no
  stretcher. Measure (null test, transient/tone tests, CPU per minute) before shipping; varispeed via the
  existing resampler is the honest fallback.
- **Seek checkpoints touch the `!Send` session internals** and the "replay is deterministic" claim: the
  checkpoint must be provably equivalent to a replay, or it is a correctness bug.
- **Recording's thread and device life** is where the host's `execute`-on-the-call-stack shape gets stressed
  first (readers are rebuilt per edit); recording *while* arranging is the harder half and can be deferred
  within slice 3.
- **Scope creep is this project's standing risk** (the owner's own words). The phase order is the defence:
  no phase-C capability starts before phase B is usable.
- **Deliberate alpha cuts**, recorded so they are choices and not oversights: zero-crossing snap,
  clip-level varispeed, LUFS/true-peak, formats beyond WAV, dither beyond fixed-seed TPDF s16, stretching
  looped clips, auto-crossfade on paste (micro-fades + an explicit `set_clip_fade` cover it), persisting
  the redo stack, fader-ride undo.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-23. Co-work: **GLM-5.3-Flash** (value pass,
gap inventory) and **GLM-5.3** (depth pass, the four designs) both returned and are archived verbatim
with dispositions in
[`research/architecture/2026-09-23-alpha-scope-co-work-reviews.md`](../../../../research/architecture/2026-09-23-alpha-scope-co-work-reviews.md).
**Kimi K3's reviewer gate did not run**: the `opencode-go` route failed twice and the `kimi -p` CLI route
hung on a tool approval (no TTY; `--auto` cannot be combined with `--prompt`). Its documented slot is the
*merge* gate on a substantive slice, so it is scheduled on **slice A1** (compound gestures) before that
commit merges — and the two passes that did run are cross-vendor to the author, so this plan is not a
same-vendor review.
