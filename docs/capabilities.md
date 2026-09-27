# Capabilities and limitations — the alpha

> 🕒 Last verified against commit `52b7c09` (2026-09-27). If the code has moved on,
> trust the code and move this line forward.

What this alpha **is**: the **[arranger profile](../.agents/notes/proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md)**
— a terminal-first tool for recording long live jams and cutting them into a finished piece, with
the session log as its document. Recording appears here as the arranger's input; it becomes a
product in its own right in the **recorder profile**, which is the focus from here. What it is
**not** is part of the statement, not an omission — this page exists so nobody has to discover a
boundary by hitting it.

Scope is fixed by
[the reviewed plan](../.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md);
the per-slice decisions live in [the notes](../.agents/notes/README.md).

## What works, end to end

| Capability | Where it lives | Evidence |
|---|---|---|
| **Record** a take from the input device into the pool (stereo, 24-bit material too); the finished take is **declared state** (`take <id> <frames> <dropped> <channels> <at_frame>`), so the session replays without the device | `record <take_id>` / `:record` | `crates/host` tests (incl. `a_take_declaration_replays_without_a_device`), `crates/media/tests/hardware_input.rs` (`--ignored`) |
| **Import** a WAV: any rate, mono or multi-channel (split per channel) | `--wave <file>`, `pool` | `crates/media/tests/pool.rs` |
| **Arrange**: cut, copy, cut/copy-paste, append, move, trim, loop, chop, fade, gain | keys `x v y c p P d < > t H L J K g G f F` | the TUI's 55 tests, the host's arrangement tests |
| **Tracks**: add, rename, delete (with clips), reorder (the mixer channel follows) | `a R D { }` | `tracks_add_rename_delete_and_reorder` |
| **Snap**: off → bar → beat → 1/2 → 1/4, with a numbered bar/beat ruler | `b`, `[`/`]` | `crates/media/src/snap.rs`, TUI ruler tests |
| **Utility transforms**: reverse (a clip property), normalize, invert, silence, trim-to-content | `V U i E T` | `crates/media/src/timeline.rs` tests |
| **Tempo match**: `source_tempo <id> <bpm>` + `W` (offline WSOLA, materialised into the pool) | `W` | `crates/media/src/stretch.rs` (9 tests), host end-to-end |
| **Markers + names**: named points and clip labels, logged and silent | `' ; " C` | `markers_are_set_renamed_sorted_and_removed`, `a_clip_can_be_named_and_unnamed` |
| **Mastering**: a compressor + lookahead brickwall limiter on the mix bus | `mount master` + `patch mixer.audio master.audio` + `set_param master …` | `crates/engine/src/plugins/master.rs` (9 tests) |
| **Export**: the whole arrangement, f32 by default or dithered s16, peak/RMS reported, never a clipped file | `X` / `export <path> [f32\|s16]` | `export_writes_the_whole_arrangement_and_refuses_to_clip` |
| **Persistence**: sessions as directories (log + pool), a crash-safe per-gesture journal | `save` / `load`, autosave | session-directory and journal tests |
| **Undo/redo**: one gesture = one entry (a paste, a marker set, a track delete) | `u` / `Ctrl+r` | group tests + every gesture's test |
| **Two shells, one workflow**: the iced shell shares the keymap/actions and *reports* what it cannot render | `spikes/iced-shell` | `shared_actions_work_or_report_their_gap` |
| **Determinism**: the same log renders the same bytes; replay, save/load and export are byte-reproducible | everywhere | `full_script_bounces_byte_identically` and per-slice replay tests |
| **Seeking at scale**: a jump into a long piece is a one-second warm-up, proved equal to a replay | `transport seek` | `a_warm_seek_equals_a_replay` (30 min: 0.10 s vs 10.6 s) |

## Deliberate cuts (alpha)

Each of these is a *decision*, recorded with its reason; the first column is what
the user will not find.

| Not in the alpha | Why |
|---|---|
| **Auditioning a pool source** (preview before placing) | The mix bus is single-connection; a preview needs its own monitor path and an unlogged action. "Load clips from the pool" is the ask; audition is the next step. |
| **Comping / take lanes** | Recording multiple takes over the same span is an arrangement-model change, not a UI one. |
| **Zero-crossing snap** | Grid snap is what the owner asked for; sample-exactness is a separate, later tool. |
| **Stereo clips** | A pool source is mono per channel (a stereo import is split across two panned tracks). Keeping one frame domain and one channel per source is what makes the reader and the peaks simple; stereo *material* is supported by the split. |
| **Formats beyond WAV**, compressed export (FLAC/MP3) | The pool is float-WAV on disk; a decoder stack is its own project. |
| **LUFS / true-peak metering** | Peak and RMS are reported; loudness standards need a window and a standard to conform to. |
| **Dither beyond fixed-seed TPDF s16** | TPDF removes the error's mean and signal correlation; noise shaping is the next step and the module is where it would go. |
| **Varispeed (resample) as a clip property** | The resampler exists (import conformance); exposing it per clip is a second rate domain the reader does not have. |
| **Stretching a looped clip** | The loop seam's interaction with a stretched region is its own design; refused by name rather than guessed. |
| **CLAP/VST hosting and export** | Evaluated and deferred ([note](../.agents/notes/proposed/architecture/2026-09-22-clap-export-via-nice-plug.md)); the plugin seam exists, the hosting does not. |
| **A sound-sculptor profile** (offline processing) | The second profile; the engine's plugin discipline is its substrate. |
| **Network audio, TUI macros/registers, the iced timeline canvas** | Named in the plan as out of alpha; the iced shell keeps the *workflow* and reports the missing canvas. |
| **The retired Tauri/Vue shell** | Retired in favour of the two Rust shells; the design docs are kept as history. |

## Known limits and rough edges

- **A session has one sample rate**, fixed when it is created. A `session_rate`
  line naming a different rate is refused rather than pretending to convert.
- **The master plugin's ceiling is −12…0 dBFS** and its `threshold` −60…0 dB: the
  declared ranges the log validates against.
- **The in-memory render bound is ~1 GiB**, about **46 minutes** of stereo at
  48 kHz (an export holds the whole mix in one buffer, not a stream). Longer
  pieces need stems or a streaming export.
- **Pieces longer than a few minutes want a checkpoint**: a *cold* backward seek
  renders the timeline from 0, which is 10.6 s for a 30-minute arrangement. The
  warm-up path avoids that unless state is placed inside the one-second run-in
  (a mid-piece `mount`/`set_param`), in which case the jump falls back to the
  full render — correct, and slow.
- **Pool source ids must be log-spellable**: no whitespace, no leading `@`/`snap=`,
  no `#`. An imported file whose name has a space gets `_` (the sanitised id is
  reported), and other unusable stems are refused rather than mangled.
- **Clip and marker names are one word** (dashes are fine): the text format is
  space-separated with no quoting, so a name that cannot be spelled is a session
  that cannot be reopened.
- **A marker past the last clip does not extend an export** (the length is
  clip-based): a marker is navigation, not content.
- **Meters read "the last rendered block"**. An offline render that ends in a
  drained silence reports 0 dB of gain reduction — honest, and the reason the
  mastering tests assert the audio rather than the meter.
- **A backdated `set_tempo`** (a command stamped earlier than the current clock)
  is applied at the current position live but at its stamped frame on replay.
  Pre-existing, identical on both seek paths, not yet fixed.
- **A clip whose declared region is longer than its source** plays silence after
  the source ends (counted as underruns). It is a broken session, not an error.
- **Recording while arranging** is not supported: a seek or a rebuild finalizes
  the take in progress (and says so) because a rebuild replaces the session.

## Flaky and unfinished (the honest list)

- `a_take_records_into_the_pool_and_plays` has failed intermittently
  ("capture stopped with a partial frame (1/2 samples) dropped") in full-suite
  runs and passes in isolation. Seen by three reviewer gates; it needs its own
  look — a flaky test erodes trust in the rest.
- The iced shell's `shared_actions_work_or_report_their_gap` failed once in ~40
  full-suite runs: it read the live snapshot immediately after pressing space and
  raced the actor's publish. The test now waits for the state (fixed); the
  *product* is fine — a shell re-reads the snapshot every frame.
- The TUI shows the *mixer's* meters, not the mastering chain's gain reduction
  (the values are published on the outcome; nothing draws them yet).
- `docs/design/` describes the retired Tauri/Vue UI. It is history, not a plan.

## Performance, measured

On one machine (release build, 48 kHz), from the slice notes:

| Operation | Cost |
|---|---|
| 30-minute, 4-track backward seek, warm-up path | **0.10 s** |
| the same seek, full replay (state inside the run-in) | 10.6 s |
| 10-minute seek before the warm-up path existed | 0.7 s |
| WSOLA stretch of a clip | sub-second per minute of material |
| export of a 30-minute arrangement | dominated by the render; ~1 GiB bound |

---

*Authored with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-24.*
