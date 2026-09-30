# Space-bunny external review — index and disposition

> **Disposition.** External deep review of the whole workspace, run 2026-09-29 against `49e8774`
> (clean tree). Reviewer: **Space Bunny** (`opencode/space-bunny-free`) driven through OpenCode by
> DeepSeek Harness; the supervisor (**DeepSeek-V4.1-Flash**) wrote the seven scopes, ran the
> adversarial verification, triaged the claims, and reviewed or corrected every fix. 39
> CRITICAL/MAJOR claims were verified: **25 CONFIRMED, 14 PARTLY, 0 REFUTED**. The verbatim
> per-scope reviews and verdicts are the seven sibling files listed at the end; this file is the
> disposition — what was done about each finding, and what was deliberately not done.

## Method

1. **Seven scoped reviewers, one at a time**, each told to hunt what the compiler and the 326-test
   suite *cannot* catch, to quote real code with a constructible trigger, and to drop anything an
   existing test already covers. Scopes: `engine`; `media` storage/capture/realtime buffers;
   `media` DSP and arrangement; `host`; the MIDI clock-out slice; tests + CI + docs-vs-code; and a
   mechanical grep-driven sweep.
2. **Every CRITICAL/MAJOR claim then went to a fresh adversarial verifier** instructed to falsify
   it, with `REFUTED` as the default when uncertain. The verdicts are reproduced verbatim in each
   scope file. They corrected false sub-consequences in several claims — and, in doing so, found
   bugs the original reviewers had missed.
3. **Every landed fix was verified the same way**: a fresh reviewer reconstructed the defect, hunted
   the neighbouring case the fix missed, checked every caller for regression, and judged whether
   each new test would fail on the unfixed code. Sixteen such passes ran. That discipline paid for
   itself: it caught an overflow *introduced* by the first clock-out fix, an allocation
   *introduced* on the render path by the patch-order fix, a false arithmetic claim in a doc
   comment, and a `save` path that wrote a session directory it could not reopen.

## What the review found that the suite could not

- The clock-out tick walk could not terminate: `set_tempo(1e-15, 4)` was accepted and the render
  thread never returned. Reproduced at `49e8774` under `timeout` (exit 124, zero test results).
- `Engine::replay_from` refused a log the engine itself writes (`mount … unmount … mount`), so such
  a session could not be replayed — and the same defect shipped in the host's `rebuild`, in
  `load_session`, and in the journal.
- A fade pair summing past `u64` wrapped past the length check: a debug panic that poisoned the
  editor's timeline lock, and in release a clip rendered silent while the log claimed otherwise.
- A legal razor-split or chop produced a clip the renderer refuses — and the renderer refuses the
  whole *track*, so the session would not play.
- `WavWriter::recover` described a 24-bit take at two bytes a sample and truncated the file to two
  thirds of its audio.
- `save` could write a `session.txt` that `load_session` then refused, with the journal already
  truncated: a bricked session directory.
- Three unbounded walks reachable from a 30-byte script line: the clock-out tick walk, the
  euclidean grid walk, and a `take` declaration that asked for a multi-gigabyte allocation.

## What landed

Twenty-six commits on `fix/space-bunny-review` (worktree `../sa-spacebunny-fixes`), one per defect,
`main` untouched. Each carries a regression test and passed the full local gate set
(`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the workspace
suite, the agent-notes verifier, and both shell-spike builds).

| Claim | Sev | Verdict | Disposition |
|---|---|---|---|
| `1-engine#1` replay cannot read a re-mounting log | CRIT | CONFIRMED | `8bd5a82`, `fe9ab1e`, `18c6f9e` |
| `1-engine#2` a patch is logged that the graph will refuse | MAJ | CONFIRMED | `e330241`, alloc `6c2c1f6` |
| `1-engine#4` a queued onset indexes past its ring | MAJ | CONFIRMED | `fc032b8` |
| `3-media-dsp#1` a fade sum wraps `u64` | CRIT | PARTLY (core confirmed) | `307aaad` |
| `3-media-dsp#2` split/chop leave an unplayable clip | CRIT | CONFIRMED | `c5b44b7`, `676bea7` |
| `3-media-dsp#3` a clip source escapes the pool | MAJ | CONFIRMED | `aacc472` |
| `3-media-dsp#6` chop has no bound on its piece count | MAJ | CONFIRMED | `3eab8f7` |
| `4-host#2` one bad journal line makes a session unopenable | MAJ | CONFIRMED | `0241677`, `8dd72b0`, `dc86b55`, `d21b3c2` |
| `4-host#3` a failed undo/redo desynchronises the history | MAJ | CONFIRMED | `0f4bc64` |
| `4-host#5` a take's channel count aborts the process | MAJ | CONFIRMED | `0f290a2` |
| `4-host#6` a refused live load wipes the session | MAJ | CONFIRMED | `34abe57` |
| `5-midi#1` the tick walk hangs the render thread | CRIT | CONFIRMED | `5acac32`, `639c2de` |
| `5-midi#2` a seek never re-syncs the follower | MAJ | CONFIRMED | `62eb94c` |
| `5-midi#3` the transport drain allocates on the render path | MAJ | CONFIRMED | `a917a1c` |
| `5-midi#5` an offline bounce drives the gear | MAJ | CONFIRMED | `c44f263` |
| `2-media-io#1` (also `6-tests#1`, `7-mech#4`) 24-bit recovery truncates | MAJ | CONFIRMED | `74186ec`, `117f2f1` |
| `2-media-io#2` a crossfade of 0 or 1 samples drops the incoming's first sample | MAJ | CONFIRMED | `6e834f8` |
| `2-media-io#3` `Capture::start` never validates `input_rate` | MAJ | CONFIRMED | `aefe129` |
| `6-tests#3` the WAV reader rejects a valid odd-sized metadata chunk | MIN | CONFIRMED | `e981f81` |

Defects the **verification** found, not the original reviewers — all fixed: the euclidean grid walk
was unbounded (`6814cfc`) and its pattern-size invariant was breakable from safe code (`069d27a`);
a refused `apply` left state behind and was silent in release (`218eee6`); the host's rebuild and
`load_session` carried the replay defect (`fe9ab1e`, `18c6f9e`); `frame_at`'s segment sum could
overflow (`dd5b6f8`); one `NaN` operand wedged the autosave forever (`8dd72b0`) — and the fix for
that then wrote a `save` directory it could not reopen (`dc86b55`) and, once corrected, briefly made
`save` run the session it was validating (`d21b3c2`); `recover` *inflated* a foreign WAV with a
trailing chunk (`117f2f1`); an apply fault allocated on the audio path (`6c2c1f6`).


## Deferred, with reason

Each is recorded in its scope file with path, trigger and a suggested fix. Nothing here was
rejected as false; these are real findings left for a next pass, with the reason.

- **`1-engine#3` a finite mount param can put a permanently-NaN oscillator on the bus.** Needs a
  per-plugin parameter-range rule (the ranges exist for `set_param` but not for mount params);
  wiring that is a design change, not a bug fix, and `7-mechanical#6` is the same gap.
- **`3-media-dsp#4` `replace_sources` deletes the previous take before the new material is
  committed** (PARTLY: the stated trigger could not be constructed, the ordering contradiction is
  real). A commit-ordering change in the pool; worth doing with the pool's own tests as the gate.
- **`3-media-dsp#5` a mono import over a split id leaves `{id}.ch0` addressable**, so a clip keeps
  playing material the user replaced. Data-correctness, needs a pool-level rule for stale
  per-channel siblings.
- **`3-media-dsp#7` / `7-mechanical#6` a reader thread and a 512 KiB ring per clip per edit.**
  A resource-cost issue; the fix is architectural (share or lazily mount readers).
- **`4-host#1` `wire_pending` resolves the mixer as `graph.out_node`** (PARTLY: the failure is real,
  the stated route is not). Needs the host to track the mixer node independently of the bus owner.
- **`4-host#4` `unmount mixer` leaves the arranger wiring stale** (a remounted mixer gets no inputs,
  orphaned nodes keep reading files). Needs the unmount path to release the cords it created.
- **`5-midi#4` the sink's `dropped_events`/`dropped_bytes` are unreachable in the product.** The
  host boxes the sink into a trait object, so the counters cannot be read; needs an accessor on the
  seam rather than a concrete-type downcast.
- **`5-midi#6` / `7-mechanical#7` the clock-out node takes two mutexes and two `expect`s on the
  render path.** PARTLY: reachable only through the slot the host fills on the control side. Worth
  revisiting when the sink seam loses its `Mutex`.
- **`6-tests#2` the media no-allocation test does not cover the retire free it claims to.** A test
  defect rather than a code one: the fix re-times the existing fixture (splice inside the measured
  window) and adds an EOF-before-splice variant, which is its own pass over that test.
- **`6-tests#4` `docs/FIRST_SESSION.md`'s expected output and silence-detection snippet are both
  false against the current code** (PARTLY). A docs fix that should also advance the freshness
  banner.
- **`7-mechanical#1` a mount/unmount scheduled at a frame inside a render block is applied on the
  render stack** (PARTLY: library-only — the host always schedules at the current frame, so no
  shipped script reaches it). A 32 KiB allocation against invariant 1, reachable from the engine
  API.
- **`7-mechanical#5` `Pool::import` deletes the previous material before the rename commits.**
  Same family as `3-media-dsp#4`.
- **Found while verifying the fixes, not yet fixed:** `Engine::node_of`/`disposers` are keyed by
  `plugin.id()` while patches and unmounts key by the mount name, and nothing enforces that they
  agree (only a `debug_assert`); a spike carries the same unguarded pool join `aacc472` removed
  from the host (`spikes/tui-shell/src/timeline.rs`); and the apply-fault allocation test measures
  the wrong thread, so it does not yet prove what it claims.
- **Documented rather than fixed:** a euclidean block pays up to `EUCLIDEAN_STEP_CAP` full
  `TempoMap` segment scans (removing that needs prefix sums, which also move `beat_at` and the
  clock-out walk); a refused *mount* still carries the plugin's own `String`, because unmaking it
  would change every `Plugin::apply` signature; and `first_tick_at_or_after`'s capped correction
  walk is bounded but not exact in the degenerate regime where it cannot converge.

## Verifications that did not complete

Two fix-verification runs (`74186ec`, `dc86b55`) ended after narration without emitting a verdict —
an artefact of the reviewer process, not of the fixes. `74186ec` was re-verified in a later batch as
`LANDED`; `dc86b55` remains verified only by its own tests and the supervisor's review. Recorded
here so the gap is visible rather than assumed closed.

One correction to the record: the commit message for `74186ec` says `recover` returned a wrong frame
count. It did not — the returned count was right and the *file* was truncated. The verifier caught
the overclaim; the code and the note are correct and the message stands uncorrected because
rewriting the history would invalidate every verification reference above.

## How to read the verbatim files

Each scope file has the same shape: a short disposition header, section 1 = the reviewer's report
verbatim, section 2 = the adversarial verifier's verdicts verbatim. The findings are the
reviewer's, not the supervisor's — **read the verdicts before acting on any single claim**, because
the verifiers corrected several of them.

- [engine crate](2026-09-29-space-bunny-review-1-engine.md)
- [media — storage, capture, realtime buffers](2026-09-29-space-bunny-review-2-media-io.md)
- [media — DSP and arrangement](2026-09-29-space-bunny-review-3-media-dsp.md)
- [host crate](2026-09-29-space-bunny-review-4-host.md)
- [the MIDI clock-out slice](2026-09-29-space-bunny-review-5-midi-clockout.md)
- [test quality, CI gates, docs-vs-code](2026-09-29-space-bunny-review-6-tests-ci-docs.md)
- [mechanical sweep](2026-09-29-space-bunny-review-7-mechanical.md)

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness (the review's conduct, verification and this
index) and Space Bunny · OpenCode (the fixes), 2026-09-29.
