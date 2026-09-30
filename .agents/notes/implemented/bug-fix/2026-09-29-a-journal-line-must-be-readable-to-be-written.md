# Agent Note: A journal line is read back before it is written, and an unreadable one is dropped

Status: implemented

## Problem

`save` verifies its own output: it parses the text it is about to write, compares the
result to the history, and **refuses** a lossy save (the
[session-directory note](../architecture/2026-09-23-session-directory-save-and-journal.md)).
`journal_append` had no equivalent check, so the autosave wrote the text form
**unchecked** — and the two holes in the form were reachable from the live path, which
accepts what the log cannot spell:

- `format_command` writes a `play`/`splice` **path raw**, so a path with a space or a `#`
  becomes `play /tmp/a b.wav ch0` — four words against `exact(&words, 3, …)`. The host
  plays it (`FilePlayer::start` opens any path), the entry lands in the history, and the
  journal line does not parse.
- `SetSourceTempo { source }` and `SourceAdd { matcher }` were never checked, although
  the log spells both as bare words (`source_tempo <id> <bpm>`, `match=…`) — the same
  discipline `SourceAdd`'s `name` already had through `media::valid_name`.

The consequence was on the **load** side, and it was total. `apply_journal` parsed the
whole tail as one text with `?`, so one unreadable line propagated out of
`load_session`: the session could not be opened at all, until the user hand-deleted
`journal.txt` — which is the autosave, so the edits on it went with it. Two lines below
the `?`, the same function's comment promises the opposite rule for a *refused* entry
("dropped and reported, never fatal"), and the
[architecture note](../architecture/2026-09-23-session-directory-save-and-journal.md)
lists a "hand-edited malformed journal line" as **owed debt**. This was that debt,
reached without a text editor: `load` a clean session (so `session_dir` is set), play a
file whose path has a space, and the session is bricked on the next open. The save path
already made such a session unsaveable, so the journal was the one remaining way the
material landed — and it destroyed loadability instead.

## Decision

**A journal line is read back by the parser that will read it, before it is written; and
an entry that cannot be read is dropped and reported, never fatal.**

- **`journal_append` verifies its entry, the way `save` verifies the script.**
  `entry_round_trips` renders the entry's expected command(s) — a bare command, or a
  `Group` when the entry is a gesture — parses the rendered text, and writes only if the
  parse succeeds *and* returns those same commands. A failure sets
  `HostSession::journal_error` and **skips the write**: the edit stands in the live
  session (it is already applied and logged), the loss is reported, and a save would
  refuse the same history, so nothing became saveable that was not before. This is
  format-agnostic: it covers the `play`/`splice` path hole without the guard needing to
  know about paths, and it covers whatever the next unspellable operand turns out to be.
- **`apply_journal` replays the tail entry by entry.** `journal_entries` splits the
  (already torn-trimmed) lines into entries — a bare command is one line, a gesture runs
  from its `group begin` to its `group end` — and each entry is parsed on its own. An
  entry that does not parse increments `JournalRecovery::refused`, records the reason in
  `refused_reason`, and the replay continues with the next entry. So a hand-edited
  journal, a journal from another host, or one written by a build with the `play` bug
  already in it costs **that entry**, not the session and not the entries after it.
  The walk is still entered and left around the whole replay, unconditionally.
- **A bare word the log must carry is one token.** `SetSourceTempo`'s `source` and
  `SourceAdd`'s `matcher` are now checked with `media::valid_name` — the same rule, and
  the same message shape, as `SourceAdd`'s `name` and a take id. The empty matcher is
  subsumed (`valid_name("")` is false) and its separate message is gone. Refusing at the
  door beats accepting an edit that quietly costs the user the ability to reopen their
  session; the shells speak this form through `parse_script`, which would have refused
  the same spelling anyway.

## Evidence

- `crates/host/src/lib.rs`:
  - `a_journal_line_the_text_form_cannot_spell_is_not_written` — a session saved, then a
    `play` of a file under a pool directory whose name has a space, then a trim. It
    asserts the unspellable line is absent from `journal.txt`, that `journal_error`
    names the format and the line, that the spellable trim is journalled and **replays
    on load** (`applied: 1`, the clip is 3 600 frames), and that a save still refuses the
    history. On the unfixed code it fails with the offending line in the journal
    (`play /tmp/host-journal-lossy-<pid>/my pool/s1.wav ch0`).
  - `an_unparseable_journal_line_is_dropped_not_fatal` — a hand-written journal with an
    unspellable `play` line and a well-formed `set_clip_gain` gesture beside it. The load
    succeeds, `refused: 1` with a reason naming the line, and the gesture applied
    (`gain == 0.25`). On the unfixed code the load fails outright with
    `line 2: play takes 2 operand(s), got 3`.
  - `a_source_and_a_matcher_the_text_form_cannot_spell_are_refused` — a matcher with a
    space, one with a `#`, and an empty one are all refused with nothing logged, a
    `SetSourceTempo` with a spaced id is refused with no tempo recorded, and a
    spellable id still records. On the unfixed code the first assertion fails.
- The neighbouring tests still hold and still say what they said:
  `a_save_that_cannot_round_trip_is_refused` (the save path),
  `a_stale_journal_is_dropped_not_fatal` (a *refused* entry),
  `the_journal_autosaves_and_a_torn_line_is_dropped` (a *torn* entry — `torn_lines` and
  the `group begin`-without-`group end` truncation are untouched), and
  `a_journal_that_re_mounts_a_plugin_is_applied_not_dropped` (a document walk, whose
  `applied` count is unchanged by per-entry parsing: a gesture is one `Group` command
  either way). 67 host tests green.

## Alternatives considered

- **Leave the write side and make the load side tolerant only** (the report's option (a)
  alone). Rejected as the whole fix: it keeps accepting edits whose only durable record
  is the line we then drop on load, so the user's edit is silently lost rather than
  refused. The read-back is what makes "the journal can be read back" a property of the
  writer, and the tolerant load is the belt to that braces — neither alone is right.
- **Refuse the `Play`/`Splice` command itself** when the path cannot be spelled. Rejected:
  the host is right to play a file the user named, and persistence is not a reason to
  refuse an edit. `save` sets the precedent — refuse to *write*, report, keep the edit.
- **Quote the path** (extend the tokenizer to `"…"`), so a spaced path round-trips. Not
  rejected on merit — it is the real fix for the form — but it is a **format change**:
  `host v1` gains a quoting rule that an older host misreads, which is a versioning
  decision, not a bug fix. The `source add` parser already says quoted matchers "await a
  tokenizer extension, not one grown here". Until that ships, the read-back is what
  stands between an unspellable operand and a bricked session.
- **Refuse the whole journal** (drop every entry, report once) instead of parsing per
  entry. Rejected: it turns one bad line into a silent loss of every later edit, which is
  the failure the previous journal fix in this note's sibling
  ([replay validates the log's lifecycle](2026-09-29-replay-validates-the-logs-lifecycle.md))
  called out by name. Per entry is also the rule the recovery already used for a torn
  tail, so this makes the two halves of "replay the complete entries" one rule.
- **A new `JournalRecovery` field** (`unparsed`) beside `refused`. Rejected: it is the
  same event from the shell's point of view — journal content that did not apply and is
  reported — and a new public field is API surface for no new information. The
  `refused` doc says so.
- **Validate the name-like operands in `format_command`/the parser instead of `apply`.**
  Rejected: the same reason the
  [fade-sum note](2026-09-29-fade-sums-are-checked-not-wrapped.md) gives — a check at
  parse time cannot repair what the user typed, and the state command is the thing that
  has to be refused. `SourceAdd`'s `name` already set the precedent.
- **Guard only `play` and `splice`.** Rejected: the guard is the round trip, so a
  hand-written list of unspellable commands is a list that rots. The two name checks are
  kept anyway, because refusing at the door is what tells the user *why* their edit is
  not saveable.

## Consequences

- A session directory is never made unopenable by its own autosave. An unspellable edit
  is reported in `journal_error` and dropped from the journal; an unreadable entry in an
  existing journal costs that entry and is reported in `last_recovery()`.
- A path with whitespace, or a `#` anywhere in it, is now a session that **applies, plays
  and exports, but will not save and will not autosave** — the same verdict `save` has
  always given it, now consistent across both write paths, and visible in one place
  (`journal_error`) rather than as a bricked directory.
- A declared source's matcher and a recorded source tempo are refused when they are not
  one token, with a message that names the rule. The shells go through `parse_script` for
  both, so no caller that worked before stops working: a spaced spelling was already a
  parse error there.
- `JournalRecovery::refused` now also counts an entry the `host v1` form cannot parse.
  `torn_lines` still means *incomplete* — the two were deliberately not merged.
- The autosave costs one extra `parse_script` of the entry being committed, on the
  control path (`commit_state`), never in the render path.
- Still owed from the
  [session-directory note](../architecture/2026-09-23-session-directory-save-and-journal.md):
  journal `fsync` (the journal is flushed, not synced, so a power cut can still lose the
  tail) and non-strict arity for `pool`/`save`/`load`. And a quoting rule for the text
  form, which is what would make a spaced path saveable at all.
- The [session-directory note](../architecture/2026-09-23-session-directory-save-and-journal.md)
  is updated in place: its debt list no longer carries the malformed-journal-line item, and
  its journal bullet states the read-back and the per-entry drop.
- **Refined by the verification of this change.** The round-trip check first compared the
  re-parsed commands with the derived `PartialEq`, and `HostCommand` carries `f32`/`f64`
  operands — so one `NaN` (`SetParam`, the one numeric operand the host applied with no
  finiteness check) made the comparison permanently false and the autosave refused *every*
  later append, with a message blaming the text form; the next successful write then erased
  the refusal. The check now compares the serialised commands through `same_commands`,
  `SetParam` refuses a non-finite value at the door the way the arrange path already did, and
  `journal_error` carries a `JournalFault` that distinguishes a `Refused` edit from a `Write`
  failure — only the latter is cleared by a later success, so a refusal cannot be erased.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
