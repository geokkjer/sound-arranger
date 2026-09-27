# Agent Note: the snap grid — in the language and in the shell (alpha slice C)

Status: implemented

## Problem

Every edit in the tool landed on an arbitrary frame. `H`/`L` nudged a clip by one beat,
which was the whole of the "musical" vocabulary — and it was a **workflow violation**:
the step was computed in the TUI (`60/bpm × rate` in `spikes/tui-shell`), so the iced shell
could not express it, and a `host v1` script had no way to say "put this on the bar" at all.
There was also no musical ruler: the timeline showed seconds, so a user could not *see*
where a beat was, only read the transport's `beat` field.

The owner's core ask is "snap to grid"; the reviewed plan (item 5) fixes the shape: the log
keeps absolute frames, so a snapped edit is an edit whose frame was quantized **before** the
command was issued — the grid is UI state and is never logged.

## Decision

**Two domains, one seam.** Musical positions are *beats*; the language speaks *frames*.

- `crates/media/src/snap.rs` — the pure math, no engine dependency:
  [`Division`](../../../../crates/media/src/snap.rs) (`Bar`/`Beat`/`Half`/`Quarter`, with `beats(beats_per_bar)`,
  `label`, `parse`, and a cycle), [`Grid`](../../../../crates/media/src/snap.rs) (`nearest`/`floor`/`ceil` in
  the beat domain, `nearest_frame` at a constant tempo), the plan's
  `to_grid(frame, division, beats_per_bar, tempo, rate)` convenience, and
  `quantize_frames(frame, step)` — the frame-domain quantizer the parser uses. A degenerate
  meter (0 beats/bar) or tempo (0/NaN/∞) degrades to a usable value instead of dividing by
  zero.
- `crates/workflow/src/lib.rs` — [`workflow::Grid`](../../../../crates/workflow/src/lib.rs): which division is
  armed (`Option<Division>`, **off by default**), and `cycle()` in the workflow's own order
  (off → bar → beat → 1/2 → 1/4 → off), so two shells cannot disagree about what `b` means.
  New actions: `GridCycle`, and `SeekGrid(±1)`. New keys: `b`, `[`/`]`.
- `crates/host/src/lib.rs` — the additive `snap=<frames>` modifier on every command with a
  frame operand (`arrange add_clip|move_clip|move_clip_to_track|razor_split`, `transport
  seek`). It quantizes **at parse time**, so the logged command already carries the snapped
  frame and the log format does not change: `arrange move_clip t0 c0 48213 snap=480` parses
  to, and formats back as, `arrange move_clip t0 c0 48000`. `snap=0` is refused (omitting
  the modifier *is* "no grid"), and applying it to a command with no frame operand is a
  parse error rather than a silent no-op.
- `crates/host/src/live.rs` — `Snapshot` carries the session's `tempo_map` and `sample_rate`
  (and `host` re-exports `TempoMap`), because a shell must quantize against the *same*
  segments the engine plays — a second copy of the tempo would drift. The grid math stays in
  the shell: the host never learns what a grid is.
- `spikes/tui-shell` — the grid state, `snap_frame()` (beat-domain quantization through the
  tempo map), and snapping at every edit target: seeks (`nudge`, `,`/`.`), the split point,
  the fade playhead, the trim playhead, and the clip-move target. `H`/`L` move one **grid
  step** (a beat when off) and the target is quantized, so a clip that sat off the grid lands
  on it. `[`/`]` step the playhead to the previous/next grid line. The state line always shows
  the armed grid, an edit that the grid moved says so in its status (`snapped from 48213 on
  the beat grid`), and an armed grid replaces the seconds ruler with a **bar/beat ruler** —
  bar lines labelled with their number, `·` on each beat step.
- `spikes/iced-shell` — holds the same `workflow::Grid`, cycles it, and reports that nothing
  snaps yet (it has no timeline canvas); the model is shared even where the view is not.

## Alternatives considered

- **A shell-only helper** (the TUI's beat-nudge, generalised). Rejected: it is exactly the
  drift this slice exists to fix — the iced shell and a `host v1` script could not use it.
- **Snap in the host, logged as a grid command.** Rejected: the log records *intent*, and the
  intent is "the clip is at frame N". A grid in the log would put UI state in the document,
  make replay depend on it, and change the log's meaning when a user toggles a key.
- **The grid on by default.** Rejected: it silently changes every existing gesture (a 64-frame
  trim becomes a beat) in a tool whose tests and habits are frame-exact. Off by default, one
  key to arm; the state line makes it visible.
- **Frames only** (quantize without the beat domain). Rejected: after a tempo change the grid
  would no longer sit on the music, and the ruler could not be drawn at all. The beat domain
  is the one that survives `set_tempo`.
- **Snapping the clip *length*** (so trims land on grid *durations*). Deferred: the trim ops
  are deltas (`by_frames`), so this is a second quantizer at the op boundary; the alpha needs
  the start frame on the grid, which it now gets.
- **Zero-crossing snap.** Deferred by the plan: a second quantizer behind the same seam, and
  click-freedom comes from default micro-fades on new boundaries (slice D).
- **A per-session `snap` in the host's parameter fold** (so a shell restores it). Deferred:
  it is a preference, not document state — persisting the grid belongs with the rest of the
  shell's settings, not in the log.
- **Making `snap=` its own command** (`set_snap 480` then edit). Rejected: a modifier keeps
  the "one edit = one line" property the script format and the gesture grouping rest on.

## Consequences

- Every positional edit can now land on a bar, a beat, or a subdivision, and the ruler shows
  the lines it will land on. `snap=<frames>` means a script reproduces a snapped edit exactly,
  with no tempo knowledge in the parser.
- Determinism is untouched: the log still holds absolute frames, so replay and the
  byte-identical bounce tests are unaffected. Nothing about the grid is stored —
  verified end-to-end through the CLI: a script with
  `arrange add_clip … 48213 … snap=24000` and `arrange move_clip … 150000 snap=96000`
  writes `arrange add_clip t0 c0 s1 0 48000 48000 0 0 1.0` and
  `arrange move_clip t0 c0 192000` into the session journal, with no modifier and no
  grid anywhere in the saved session.
- The status line carries the grid, and an edit that the grid moved from the playhead says so
  — a snapped edit that looks like a bug is worse than no grid.
- Tests: `media` covers the divisions, the beat math at 120 bpm/48 kHz (a beat is 24 000
  frames, a 4/4 bar 96 000), the guards, and `quantize_frames` (48 213 on a 480-frame grid is
  48 000, and a frame at the top of the range saturates); `host` covers the modifier on every snappable op, either order with `@frame`, the
  formatter writing the snapped frame, and the four refusals; `workflow` covers the cycle;
  the TUI covers the cycle in the state line, beat-domain quantization at three divisions,
  `[`/`]` on and off the grid, and the ruler switching between seconds and bar/beat; iced
  covers the shared cycle and its report.
- **The independent gate (GLM-5.3 stand-in, `merge with changes`) found two real defects and
  four notes, all dispositioned** ([review + disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-c-glm-standin.md)):
  `quantize_frames` could overflow `u64` on a nonsense-but-parsed frame (`transport seek
  u64::MAX snap=2`) — found by self-review first, fixed with a saturating add and a test; a
  **snapped fade did not say so** in its status (every other playhead edit did) — fixed with a
  test; the second-seek used the spike's 48 kHz constant instead of the session rate, and seeks
  were unclamped — fixed (one `clamp_frame` helper for all three seek paths); and the ruler's
  4096-step cap **blanked the far end of a zoomed-out fine grid** (14 000 steps for 100 columns
  in a 30-minute view) — fixed by drawing by cell (`ruler_marks`, O(width) at any zoom) with a
  two-zoom regression test. The gate's `set_tempo` bpm-0/NaN finding was **refuted** (the engine
  refuses it before anything changes) and the double tempo-map clone was already fixed. One
  finding stands as a documented limit: the ruler's bar *numbering* across a meter change inside
  the view keeps the view-start phase.
- **Still open**: a shell preference for the grid (it resets per run); snapping a clip's
  *length*; swing/humanize; a zero-crossing quantizer; a bar-phase notion in the tempo map, so a
  mid-view meter change continues the bar count instead of restarting the phase at the view.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
