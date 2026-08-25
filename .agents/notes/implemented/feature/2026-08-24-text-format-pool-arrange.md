# Agent Note: text-format `pool`/`arrange` (the CLI smoke binary drives the clip editor)

Status: implemented

## Problem

The versioned text format (`host vN`, the wire schema for the CLI smoke binary and a future
Tauri shell) could not express the clip editor's `pool`/`arrange` commands — GLM-5.3's major
#7. So the CLI could drive the `Play`/`Splice` recorder path but not the arrangement: a script
could not build a clip arrangement and bounce it.

## Decision

`parse_script` gains:

- **`pool <dir>`** — `HostCommand::Pool` (sets the media pool; the host's `set_pool` validates
  the dir).
- **`arrange <op> <args> [loop_len] @frame`** — `parse_arrange` maps a snake_case op token +
  text operands to a `media::ArrangeOp`, adding all 12 variants (add_track, remove_track,
  add_clip, razor_split, trim, move_clip, move_clip_to_track, duplicate, delete, set_clip_gain,
  set_clip_fade, loop_region). The `@frame` (for the logged `at_frame`) and the optional
  `add_clip [loop_len]` are the documented grammar.

The parser is **strict**: a missing or wrong-typed operand, a non-finite gain (`NaN`/`inf`), a
`loop_region` count that overflows u32, or a wrong operand **count** (trailing junk) is a clean
`Err` — never a panic, never a silent coercion. `run_script` drives the arrangement
(pool + arrange + bounce) byte-identically.

## Alternatives considered

- **Keep the CLI on `Play`/`Splice` only** — rejected: the clip editor is the profile's heart,
  and the wire schema must express it for the CLI/Tauri shell to drive it end-to-end.
- **Reuse the engine op field encoding (`encode_op`) as the text** — rejected: the field-value
  map is an internal representation; a flat positional grammar is far more legible for a wire
  format.
- **Token namespace = the enum variant names (`AddTrack`)** — rejected: the text format is the
  public contract and its tokens are versioned; `snake_case` is the readable, contract-exposed
  form (kimi noted the variants are internal and free to be renamed without a version bump).

## Consequences

- A `pool` + `arrange` + `bounce` text script builds an arrangement, renders it (non-silent),
  and **replays byte-identically** through the CLI-smoke-binary path (the
  `text_format_pool_and_arrange_run_byte_identically` test). The current `Play`/`Splice` path
  remains.
- Tests: the 12 arrange-op grammar parses; malformed/long/NaN/overflow lines refuse; the
  end-to-end text script replays byte-identically. 18 workspace suites green, clippy clean.
- **Deferred**: an exhaustive-enumeration test that every `ArrangeOp` variant has a text arm
  (a compiler-enforced correspondence); quoted paths; a golden-baseline byte-identity check
  against the ClipEditor API; `@frame`-required strictly.
