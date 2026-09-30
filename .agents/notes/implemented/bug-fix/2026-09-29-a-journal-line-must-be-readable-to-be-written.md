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

**Refined by the verification of this change.** The review confirmed the wedge was closed
and found two more on the same path:

- **`save` wrote a session directory that would not reopen.** Its self-check answered the
  *form*'s question — "does this text parse to the same commands?" — which is not the
  reader's question. An entry can be spelled faithfully and still be **refused by the
  applier**: `set_param … NaN` has exactly one spelling and exactly one verdict
  (`parameter '…' must be finite`), so the comparison passed and the save wrote a
  `session.txt` that `load_session` returns `Err` on — the brick this note exists to
  prevent, with the journal already truncated by the same call. Today's trigger is a
  `NaN`/`inf` operand; tomorrow it is any op whose form the writer spells and the
  applier refuses, because a writer-side *list* of commands that will not apply is a list
  that rots, and the form is not what decides it.
- **A refusal outlived the edit it named.** Only a `save` cleared the `Refused` variant,
  `undo` took the offending entry out of the history, and `carry_over` carried the report
  across the rebuild — so `journal_error()` asserted "this edit is in the live session and
  nowhere else" about an edit that was no longer in the session, and the TUI's status line
  read "autosave failed" after every subsequent edit. A false alarm is the mirror image of
  the lost alarm the previous change fixed, and it is the one users actually hit.

**Refined again, by the verification of the "save *applies* the script" half.** That
correction was right about the hole and wrong about how it closed it. It made `save`
build a whole `HostSession` from the script it was about to write, to prove the file
would open. **Building a session is running it**, and a saved script is full of state
commands that are *actions against material*: a `play` opens the file it names, starts a
`FilePlayer` on a decoder thread, and blocks in `warm_player` until the ring fills or a
ten-second deadline expires. So the check asked questions the document cannot answer:

- **A `play` whose file had moved failed the save.** The play line names an absolute path
  (`rebase_pool` rewrites `pool` commands, not play paths), so a renamed file, an ejected
  drive or a pool deleted under the session made `save` refuse a history that is a
  perfectly good document. The same shape as a save that fails because the disk is full:
  the document is fine, and the session must still save.
- **`save` could block for ten seconds and spawn a decoder thread** to validate a file
  that was never going to be played — on the control path, in a UI's save handler.
- **A *successful* save manufactured a false alarm whose text was false.** After writing,
  `save` re-derives the outstanding refusal, because its pool re-point can have changed
  the answer. But the re-derivation asked only the **journal's** spelling, and the two
  spellings differ for exactly one line: `Pool`. The journal names the pool by the
  absolute path the session used; a save names the copy inside the session directory,
  relative to it. So `save "/…/My Songs/song.d"` re-pointed the history at
  `"/…/My Songs/song.d/pool"`, the journal could not spell that, and the save reported a
  refusal — "…and a save refuses the history as well" — over a save that had plainly
  succeeded, for a path the form spells perfectly well as `pool pool`. A user cannot act
  on that message, which makes it worse than no message.

The root of all three is one confusion: **"can this document be read back and applied" is
a question about the *document*, and the check was asking the *machine*.** Whether a
particular WAV is on this disk right now is not a property of the session text; the text
says what the session is, and the file is this machine's copy of it.

## Decision

**A journal line is read back by the parser that will read it, before it is written; an
entry that cannot be read is dropped and reported, never fatal; a saved session is
*checked against the applier's own doors* before it is written, by a **dry run** that
runs the document and none of the material; and a report of what could not be saved
cannot outlive the edit it names, or claim a loss that a `save` has already repaired.**

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
- **`save` validates by *walking the script on a dry run*, not by parsing the text alone —
  and the check sits before the temp file, not before the rename.** The write side's rule is
  **"whatever is written reads back, and every value in it passes the applier's own
  doors"**: the script is issued to a fresh session by the very walk `load_session` runs
  (`HostSession::from_script`, reached through the private `from_script_with`, whose
  differences from a load are a silent MIDI sink — a save must not open a device, let
  alone take the live session's — and the `validating` mode), and the first refusal is
  named by **the edit** (its own lines in the file) plus the applier's reason.
  `HostSession::from_script` now returns the index of the command it refused behind its
  usual reason, because an index is what lets a *writer* name an edit and a *reader*
  needs nothing more.
  **The order is the decision.** Every refusal happens before a single byte is written, and
  in particular before the journal is truncated: the journal is the autosave and the only
  durable record of the edits since the last save, so truncating it and then failing would
  destroy the tail to buy nothing. A refused save therefore leaves the directory exactly as
  it was — the previous `session.txt` still opens, the journal still holds the unsaved
  edits, and not even a `session.txt.tmp` appears.
- **The dry run is a *mode* on the session, not a second list of checks** — and that is the
  whole correction. `HostSession::validating` is a flag, and the check is the ordinary
  apply path with three places standing down, so **the value doors a reader would meet are
  the reader's own code** (`Engine::set_param`'s finiteness and range, the timeline's op
  rules, the mixer's channel count, the editor's arrangement) rather than a writer-side
  list of "commands that will not apply", which is the same list that rots one layer up and
  would be worse for *looking* complete. A writer-side list of unspellable commands was
  already rejected below; a writer-side list of unappliable ones is the identical mistake in
  a new place, and a dry run is what makes it unnecessary. The three places that stand down
  are exactly the three whose work is **not about the document**:
  1. `Play`/`Splice` do not resolve a whole-file region through `WavReader::open`, do not
     start a `FilePlayer`, and do not `warm_player`. They keep the document's doors (the
     mixer must be mounted, the channel must be inside it, one clip at a time) and record
     the state a later `splice` in the same script reads, so a `splice` after a `play` is
     still checked against a session that has a player.
  2. `process` does not render up to a command's `at_frame`. Rendering is what wires the
     arranger, opens every pool source through a reader thread and warms it; a walk that
     renders is a session *running*, and a save must not run the session it is saving. No
     verdict is lost: every door this walk reaches asks a question of the *sequence* of
     commands (`mount`/`patch`/`set_param` all accept a scheduled-or-mounted plugin, and the
     document walk is what makes `mount … unmount … mount` legal), and the sequence is
     walked whole either way.
  3. `Pool` adopts the directory without `conform`ing it (`note_pool`). The door half of
     `set_pool` still runs — the path must be a directory, which is what the reader asks —
     and the guarded resolver is still installed, so every arrangement op behind it sees a
     pool. What stands down is the *write*: `conform` reads every source's header and
     rewrites the ones at a foreign rate, so a save that ran it would write into the very
     directory it is publishing.
- **A file that has moved is a runtime condition, and the reader still reports it.** A
  missing WAV is not a defect in the session text; it is this machine's copy of it, and the
  same argument that a full disk must not fail a save applies to a file on another volume.
  So the save lands, and the missing file is reported where it always was — by the **reader**
  (`a_session_whose_play_file_moved_still_saves` pins both halves: the baseline walk's `Err`
  naming the path, and the journal path's existing drop-and-report). The tolerance is
  deliberately on the *write* side only, and the test asserts the report is not lost, so
  "the save tolerates it" can never quietly become "nobody mentions it".
- **The two write paths hold deliberately different strengths, and the split is by
  durability.** The journal checks *reads back as itself* (`entry_fault`), per entry,
  because it is best-effort and the load side already drops and reports an entry it cannot
  apply — the one write path that cannot be made to refuse opens. `save` additionally walks
  the applier's doors, over the whole script, because it writes the baseline the session is
  rebuilt from and that file has to open. So an entry the session itself would refuse (a
  `NaN` param) is journalled and reported per entry on replay, and the save that would
  have bricked the directory is refused by name instead.
- **A refusal is re-derived from the history; it is not latched and not merely cleared.**
  `rederive_journal_fault` re-reads the outstanding refusal whenever the history changes
  under it — `carry_over` (undo, redo, a seek) and `save`'s pool re-point — from
  `outstanding_refusal`, which uses the *same predicate the write uses* (`entry_fault`) and
  the same report text (`refusal_report`), so the write and the report cannot disagree about
  what is spellable. One rule, both directions: an entry still in the history and still
  unspellable keeps its report (which is what a successful write must not erase), an entry
  that `undo` removed takes it with it, and one a `redo` put back brings it back. Two
  guards: **no session directory, no report** (there is no autosave to have refused
  anything — an unspellable edit in a directory-less session is a `save` refusal, named by
  `save`), and **a write fault stands** (it is the file's, and a history edit is no
  evidence about a file; a `save` rewrote the file, so it clears that one itself).
- **The re-derivation asks *both* spellings, because the report is a claim about *both*
  records.** `entry_fault` now takes the spelling to ask in: the journal's (`None`, the
  absolute paths the session used) or the save's (`Some(dir)`, the copy inside the session
  directory). `Pool` is the only line the directory changes, and a pool path with a space is
  unspellable in one and spellable in the other. An entry the journal can write is a record
  full stop; an entry the journal cannot but a save can is a `pool` line, and the pool is
  **context, not an edit** (`rebase_pool` says so) — it is in `session.txt` the moment the
  save lands, so there is no standing loss to report. `entry_round_trips` takes the resolve
  directory for the same reason: a save's `pool pool` must read back as the absolute path it
  resolves to, which is `resolve_session_paths` — the same step `save` and `load_session`
  run — or the two spellings would not compare equal to the entry that made them.
  This is what removes the false alarm: the report's sentence ("it is in the live session
  and nowhere else, and a save refuses the history as well") is a claim about both records,
  so it is now only made where it is true of both. **A report that is not actionable is
  worse than no report**, which is the same reason the load side drops rather than aborts.
- **The word-based form still says so when it cannot express a command**, and says it as
  a fact about the *form*: `write_entry`'s refusal is now the bare
  "the host v1 text form cannot express …", and the caller adds what it does about it (a
  save refuses the write, the journal drops the entry and reports it). The journal also no
  longer skips such an entry **in silence** — the report says "it has no line in the host
  v1 text form" — because the journal is the one write path that can meet an entry a save
  never saw (a session opened from a hand-edited script).

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
  either way). They pass with the two below.
- **From the verification that found the two wedges above** (`cargo test -p host`: 74 lib
  tests green, plus the session/journal integration binaries):
  - `a_session_the_applier_refuses_is_not_saved` — a saved session, a spellable edit (so
    the journal holds a tail), then a `NaN` param committed to the document. `save` is
    refused, and the message names **both** the edit (`set_param mixer ch0.gain NaN`) and
    the applier's reason (`must be finite`). `session.txt` is byte-identical to before,
    `journal.txt` still holds the tail, no temp file exists, and the directory still
    **loads** with the unsaved edit in it. On the unfixed code the save *succeeds*
    (`expect_err` panics on `Ok(())`) and the directory it wrote will not open.
  - `an_undo_that_removes_a_refused_edit_clears_the_report` — a clip id with a space (an
    `Arrange` op, so it is undoable: a false alarm needs a *removable* subject) is refused
    by the autosave and named; a later **successful** write (`set_param … 0.5`, not the
    `Arrange` entry) is journalled and leaves the refusal standing; the **undo** then
    clears it, a spellable edit after it is autosaved silently, the **redo** brings the
    report back (so the state is re-derived, not cleared), and the directory loads with
    nothing in the journal refused. On the unfixed code the undo leaves the report
    standing for an edit that is no longer in the session.
  - `a_value_that_cannot_compare_equal_to_itself_does_not_wedge_the_autosave` keeps its
    subject — the *form* carries a `NaN`, the journal writes it, the replay costs one
    dropped entry — and its final assertion is now that the save **refuses** it, which is
    the honest verdict rather than a brick. The test's doc states the split: that is the
    autosave's half of the rule; `save` holds the stronger one.
- **From the verification of that correction** (`cargo test -p host`: 80 lib tests green,
  1 ignored, plus the four session/journal integration binaries at 16/4/4/8). Each of the
  three new tests was run against the *unfixed* `save` — the same file with the previous
  check restored and only the tests added — and **each fails there, with the finding's own
  words**:
  - `a_session_whose_play_file_moved_still_saves` — a saved session, a `play` of a file in
    the pool, then the file is deleted. The save lands, the play line is in the baseline,
    the journal restarts from it, and **the reader still reports the missing file in the
    shape it always used**: the baseline walk's `Err` naming `s1.wav`, and — for a `play`
    that arrives through the autosave instead — the journal path's existing
    drop-and-report (`refused: 1`, reason naming the file, the rest of the session intact).
    On the unfixed code the *save* fails: `save: the session text does not apply — refusing
    to write a session that would not open: play /…/pool/s1.wav ch0 is refused when the
    file is read back: open /…/pool/s1.wav: No such file or directory`.
  - `the_save_check_never_opens_the_material_it_validates` — a `play` naming a file that
    exists and is deliberately **not a WAV**. The moved-file case above is refused *fast*
    by the old code, so on its own it says nothing about the ten-second `warm_player`
    deadline or its decoder thread, which are paid for a file that **is** there; a
    non-WAV is the case where the old code read the file before it ever reached the warm.
    The save lands in well under two seconds (a **bound**, not a measurement — it is there
    so a future blocking read fails here rather than costing a user ten seconds per save)
    and the play line is written. On the unfixed code the save fails with `not a RIFF
    file: expected 'RIFF', found [116, 104, 105, 115]` — the check read the file.
  - `a_save_into_a_directory_with_a_space_reports_nothing` — a save into
    `…/My Songs/song.d`. It lands, the baseline spells the pool **relative**
    (`pool pool`, the movable-session rule), `journal_error` is `None`, and it is *still*
    `None` after a later edit, a second save and a reopen — the re-derivation runs on every
    history change, so a false claim would come back on the next `undo` too. On the
    unfixed code the save **succeeds** and then reports:
    `the host v1 text form cannot read it back (line 2: pool takes 1 operand(s), got 2), so
    this edit is not autosaved — it is in the live session and nowhere else, and a save
    refuses the history as well: pool /…/My Songs/song.d/pool`.
  - `a_session_the_applier_refuses_is_not_saved` (the `NaN` case the dry run exists for) is
    **unchanged and still load-bearing**: replacing the dry run with `Ok(())` — i.e.
    dropping the applier walk entirely — is the only one of the four that then fails, so
    the tolerance above was not bought by weakening the check. A scratch probe (removed
    before the commit) confirmed the dry run still catches doors that are not `NaN` at all:
    a `Take` with `channels: 9999` is refused by name with `take 'take-1' channels must be
    1..=64, got 9999`, which is the evidence that the walk still runs the *reader's* rules
    rather than a list.

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
- **A writer-side list of commands the applier will refuse** (a `match` over
  `HostCommand` next to `apply`, "these are the ones a script cannot carry"). Rejected:
  the same rot argument as the hand-written unspellable list, one layer up — and it is
  worse, because it *looks* complete. The writer already has the reader: `from_script` is
  the definition of "applies", so a save that does not use it is the one place on the
  platform where a second, stale answer to that question could be maintained. **This is
  also the argument against answering the *dry run* with a list**, which is why the mode
  is a flag on the ordinary apply path and not a `match` of "safe to check" commands: such
  a list is the same rot, and the new commands that need to be in it are exactly the ones
  nobody remembers.
- **A dry run that skips a `match` of "these commands are safe to validate"** (the mirror
  of the list above — enumerate the arms with no material in them, and refuse the rest).
  Rejected for the same reason, and it is the *more* dangerous of the two because the
  enumeration grows silently: a new media command is refusing-by-default until someone
  adds it, and the failure is a save that writes a file nobody can open — the exact
  regression this note's first correction was made to stop. The three stand-downs here are
  at the *leaf* that does the work (`FilePlayer::start`, `warm_player`, `conform`, the
  pre-`at_frame` render), so a new command is in the check by default and only has to be
  argued out of it.
- **Give the applier a real "dry" enum per command** (`Apply::Live` / `Apply::Check`), so
  each arm declares its own behaviour. Rejected as more machinery than the problem needs
  for the same answer: it spreads one fact ("this session is being checked") across every
  arm of a 900-line `match`, and the arms that must change are the three that do I/O. A
  session-level flag is one fact in one place, and the arms read it where the work is.
- **Keep building the session, but validate the script's *values* separately** (walk the
  parsed commands and check only the numeric operands against the engine's finiteness and
  range rules, without building anything). Rejected: it is the writer-side list again, and
  it is the version that *looks* most complete, because "check every `f32` is finite" does
  catch today's `NaN`. It still knows nothing about the mixer's channel count, the
  arrangement's op rules or anything else a new door adds — and those are exactly the
  refusals a reader will meet. The dry run asks the reader.
- **Tolerate the failure in `save` instead of in the check** (build, and ignore a
  `Play`/`Splice` failure that names a file). Rejected: it cannot tell the two apart. The
  same `Err` type carries "this WAV is not on this disk" and "this arrangement op is
  illegal in this arrangement", and string-matching an `Err` to decide which is a writer
  guessing at the reader's reasons — and it would silently widen every time a door learns
  to mention a path. The mode is declared where the work is, so the split is a fact about
  the code rather than a reading of a message.
- **Make the baseline walk tolerant of a missing file too** (drop the `play` on load, as
  the journal path already does). Rejected as out of scope and *not* obviously right: the
  journal's tolerance is justified by the journal being best-effort, and a baseline is the
  one file the session is rebuilt from, so a `play` it cannot resolve is a load the user
  needs to be told about loudly. The requirement here is the narrower and more defensible
  one — the save must not refuse, and the reader keeps reporting the file in the shape it
  already used — and the test pins that the report is not lost. **Owed:** whether a
  baseline whose material has moved should open degraded (the arrangement, the pool, the
  mixer, minus the `play`) is a real product question, and it is a decision about the
  reader, not about this bug fix.
- **Refuse the operand at the door instead** — `format_command`/`parse_script` rejecting
  a `NaN`, as the *clip id* is a plain stem and the *source* a pool id (the
  [pool-id note](2026-09-29-a-clip-source-is-a-pool-id-not-a-path.md)). Not rejected on
  merit; that is the right shape for an operand that has an illegal-for-the-log spelling.
  Rejected as the *whole* fix for two reasons. A `NaN` has **no** legal spelling, so the
  door cannot be the only line — and the door does not need to be, because the engine's
  `set_param` already refuses it before the value is logged, which is where a value rule
  belongs. And the door is unreachable for the general case anyway: this is about any op
  whose *form* the writer spells and whose *apply* refuses, which is a set nobody can
  write down. The engine's refusal stays the door; this is the writer's half.
- **Re-derive the refusal by comparing against the `redo` stack** (`journal_error` holds
  the entry's text; a history edit checks whether that text is still there) instead of
  re-running the predicate over the history. Rejected: it needs the fault to carry the
  entry's identity, which is a second place to keep a report in step with the one the
  write builds — and it cannot answer the question the re-derivation answers for free: a
  history that holds two unspellable entries reports the most recent, and undoing *that*
  one must fall back to the other rather than going silent.
- **Check the journal too, by applying each entry** (the strongest rule on both write
  paths). Rejected as a cost, honestly stated: the journal is the control path, once per
  committed gesture, and applying an entry means building a session — a full walk per
  gesture, where the round trip is one `parse_script`. The asymmetry is justified instead,
  by durability: the journal cannot make the directory unopenable (`apply_journal` drops
  and reports per entry), and the save can, so the save is where the stronger rule earns
  its cost. If the journal ever learns to apply its entries cheaply, this is the note to
  supersede.
- **Clear the refusal on any history edit** instead of re-deriving. Rejected: it is the
  fix for the false alarm and a *new* lost alarm in the same breath — a `redo` puts the
  unspellable edit back into the session and the report would not come back with it, so
  the loss is silent from then on. One rule (the history says what is outstanding) has no
  gap to reason about; "clear on undo" has two directions to get wrong.

## Consequences

- A session directory is never made unopenable by its own autosave **or by its own
  save**. An unspellable edit is reported in `journal_error` and dropped from the
  journal; an unreadable entry in an existing journal costs that entry and is reported in
  `last_recovery()`; and a `save` whose text would not apply is refused by name, with
  nothing written.
- **What a `save` refuses is a property of the document; what it tolerates is a property
  of the machine.** A value the applier's own doors reject (a `NaN` param, a param out of
  range, a `take` with a channel count no capture could have recorded, an op illegal in
  this arrangement) still refuses the write, by name, before a byte is written. A `play`
  whose WAV is not on this disk, right now, does not — the save lands and the reader
  reports the file in the shape it always used. The two are the same question asked of
  two different things, and conflating them is what made a session unsaveable over a
  renamed file.
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
- **Owed from this correction:** whether a *baseline* whose material has moved should open
  degraded — the arrangement, the pool and the mixer, minus the `play` — the way the
  journal already does. Today the reader is loud about it, which is defensible (the
  baseline is the one file the session is rebuilt from) but is a product question, not a
  consequence of this fix, and it belongs to a decision about the reader.
- **Owed, and new:** `play`/`splice` lines name **absolute paths that `rebase_pool` does
  not rewrite**, so a saved session's player material is not part of the copy the session
  directory makes — a saved session moved to another machine (or reopened after the pool
  moved) has a `play` pointing at a path that was never copied. The recorder profile is
  built on pool ids for clip *sources* (the
  [pool-id note](2026-09-29-a-clip-source-is-a-pool-id-not-a-path.md)) and `play` is the
  same argument one command later; this note only stops the save from *refusing* such a
  session, which is the correct half to fix here.
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
- **Refined again by the verification of that one.** Three things moved, and the middle one
  narrows a sentence above:
  - **`save` no longer writes a directory `load_session` refuses.** A `NaN`/`inf` param in a
    history is now refused *by name*, and so is any future entry the form spells and the
    applier declines. The trade is deliberate and is the honest verdict: a session holding
    one **plays, seeks, exports and autosaves, and will not save** — the same shape of
    verdict a path with a space has always had, now reached through the applier instead of
    through the spelling. Refusing to write is not refusing the *edit*: the live session
    keeps working, and the previous save on disk keeps opening.
  - **The journal's rule deliberately did not get stronger** — still *reads back as itself*,
    per entry, so an entry the applier would refuse is journalled and then dropped and
    reported by the load side. The two write paths answer different questions on purpose:
    best-effort per entry versus a baseline that must open.
  - **"A refusal stands until the next `save`" is no longer true, and was never the
    mechanism.** A `save` cannot succeed on a history holding a refused entry, so it was
    never going to be the thing that cleared it. The report is **re-derived from the
    history**: it dies with its edit (`undo`), comes back with it (`redo`), survives every
    successful write, and is re-read after `save`'s pool re-point — the one case where a
    save *legitimately* changes the answer, because the journal spells a pool by the
    absolute path the session used while the save spells the copy inside the directory, so
    a pool path with a space is unspellable in the journal and fine in a save. **Refined a
    third time:** that re-derivation was reading only the journal's spelling, so it turned a
    legitimate change of the answer into a *false alarm* instead of honouring it. It asks
    both spellings now, and a save into a directory whose own path holds a space reports
    nothing at all.
- **Costs.** `save` walks the whole script on a fresh session before writing — the work a
  load does, minus the I/O: **no file is opened, no decoder thread is spawned, no ring is
  warmed, no pool is written, and no frame is rendered.** The re-derivation is one
  `parse_script` per history entry (two for an entry the journal cannot spell, the second
  asking the save's spelling), and only where a fault could be outstanding: not with no
  session directory, and not for a `Write` fault. Neither cost is on the render path.
  **Owed, honestly:** the dry run is a *list of three stand-downs* in the sense that
  anything it fails to name is validated for real. The three are at the leaves that do the
  work, and a new media command is in the check by default — but a new door that *writes*
  (a conform, an export, a capture) would be validated for real until someone adds it here.
  The alternative (enumerate the safe arms instead) fails in the other direction and is
  worse; what this note owes is a line in the `validating` doc for each new I/O-bearing
  command, which is the discipline the
  [fade-sum note](2026-09-29-fade-sums-are-checked-not-wrapped.md) already records for
  the same reason.
- **A refused `save` leaves the directory exactly as it was** — the previous `session.txt`,
  the journal tail, no temp file — so retrying after fixing the offending edit costs the
  user nothing but the fix.
- **The shells still read one string for both fault kinds** (`live::HostOutcome`'s
  `journal_error`), and the TUI's prefix — "autosave failed" — is the wrong words for a
  refusal, where the autosave *refused on purpose* and the edit is still in the session.
  Both faults do report a loss of the autosave for an edit, which is what a status line can
  show, so the field is unchanged and its doc now states what moved. **Owed:** expose the
  kind (`JournalFault` is private, so a shell cannot branch on it) — an API decision with
  two shells and a snapshot struct in it, not something to slip in under a bug fix.

*Authored with Space Bunny · OpenCode, 2026-09-29. Refined by the independent verification
of the "save applies the script" half, 2026-09-30: the check is a dry run, so it asks the
document's questions and not the machine's.*
