# Agent Note: a session is a directory — the log, its pool, and a journal

Status: implemented

## Problem

The session log lived only in memory. The host could *load* a `host v1` script, but there was no way
to write one: arranging for an evening and quitting lost the piece, and a crash lost it without
warning. Everything the alpha plan builds after this (clipboard, recording, stretch, export) assumes
the work survives, so persistence was the second foundation slice — and it is also where the plan's
path trap lives: the log records **absolute** paths (`pool /data/takes`), so a saved session could not
be moved, and a save that only wrote the log would leave the audio behind.

## Decision

**A session is a directory, and the log is the file.**

- `mysong.d/session.txt` — the session's **state commands in order**, in the `host v1` text form, with
  gestures bracketed (`group begin` … `group end`, from
  [A1](2026-09-23-compound-gestures-one-undo-step.md)) so a reloaded session still undoes a gesture as
  one step. The loader is the **existing** `parse_script`: no second format and no second versioning —
  the format that the CLI, the `:` prompt and the LLM seam already speak.
- `mysong.d/pool/` — the material the log refers to (WAVs and their `.peaks`), copied in on save when
  the session's pool lives elsewhere, so the directory is self-contained and **movable**. The pool head
  in the script is written **relative to the session** (`pool pool`), and a relative pool path resolves
  against the session directory on load; if the pool cannot be copied the session simply keeps pointing
  where the audio is.
- **Context lines, not edits.** `session_rate <hz>` is read *before* the engine exists
  (`HostSession::from_script` → `new_at`), because the clock, every frame in the log and the device
  negotiation derive from the rate; a live session **refuses** a different rate instead of pretending
  to change. `set_tempo` and `pool` ride in the log as before. The rate line is what lets a 44.1 kHz
  session round-trip.
- **`Save`/`Load` are actions** (like `Bounce`), never state: they write and read, they do not appear in
  the log they write. `save` writes `session.txt.tmp` and renames, then resets the journal — so a crash
  mid-save leaves the *previous* session intact.
- **The journal is the autosave.** Every committed gesture is appended to `mysong.d/journal.txt` and
  flushed; there is no timer and no snapshot cadence. `load` applies the script and then the journal,
  and **an incomplete entry is dropped and reported**, never a parse error that costs the session: the
  journal is a sequence of complete entries, so recovery replays the complete ones and stops
  (`HostSession::last_recovery()` reports how many lines were dropped). That covers both shapes a crash
  can cut — a partial final line, and a cut *inside* a gesture (one member on disk, no `group end`) —
  and the second is why the rule is per **entry** rather than per line: a gesture cut mid-write is
  dropped whole, never half-applied. The same rule covers an entry the `host v1` form cannot **parse**,
  and the write side is held to it: a journal line is read back by `parse_script` before it is written,
  so an edit the word-based form cannot spell is reported in `journal_error` and left out rather than
  landing as a file that will not open (the
  [journal read-back fix](../bug-fix/2026-09-29-a-journal-line-must-be-readable-to-be-written.md)).
  A journal write failure never fails the edit (it is already applied
  and logged) — it is recorded in `journal_error()` instead of being silent.
- **The text form now goes both ways.** `format_command` / `format_arrange` are the inverse of the
  parser (`fmt_f32` prints the shortest form that parses back to the same bits); **a pure action has no
  text form at all**, so a transport command or a bounce can never leak into a log. A round-trip test
  covers every state command and every arrangement op.
- **Not saved** (rebuilt by replay, which is the architecture paying off): wiring, counters, the
  timeline snapshot, the clipboard, grid/UI state, bounce outputs, the redo stack.

## Evidence

- `crates/host/src/lib.rs` tests: `the_text_form_round_trips_every_state_command` (every state command
  and all twelve arrangement ops, plus "an action is not serializable"), `save_and_load_round_trips_a_session`
  (value, folded parameters, tempo-after-render, a **byte-identical bounce**, and a gesture still undoing
  as one step after a reload), `a_saved_session_can_be_moved` (the script says `pool pool`; the session is
  renamed, the original pool deleted, and the bounce is still audible and identical),
  `the_journal_autosaves_and_a_torn_line_is_dropped` (the journal carries the gesture's markers; a
  half-written line is dropped and reported; and a gesture cut mid-write — one member, no `group end` —
  is dropped *whole*, leaving the clip untouched rather than half-trimmed),
  `a_session_at_another_rate_round_trips`
  (44.1 kHz saved, loaded, bounced at 44.1 kHz, and a live session refuses to change rate).
- `spikes/tui-shell`: `the_command_line_saves_and_loads_a_session` — `: save <dir>` from the prompt
  writes the directory (script, pool, journal), a later edit is autosaved, and `: load` brings both edits
  back. 27 tests.
- Workspace: 24 test binaries green, clippy and fmt clean.

## Review

The plan commits architectural slices to the co-work reviewer gate (Kimi K3). A2 was submitted the same
three ways as A1 and **again did not return**: the owner reports the CLI's device login now answers
`403 Forbidden` (a server-side rejection of the client, not a quota error) and the API side has hit its
5-hour limit (reset ~1 h 42 min later) — so the gate is blocked at the provider, with the API route to
retry once the window resets. The stand-in was **GLM-5.3** (cross-vendor to this repo's DeepSeek author, and the model that reviewed
the plan). It returned **`merge with changes`** and found **four real bugs in this slice, none caught by
the tests**, all now fixed with regression tests:

1. **Autosave silently stopped after an undo, redo or seek.** `replay_to` rebuilds the session and
   `*self = rebuilt` carried only `playing`/`redo`, so `session_dir` (and the journal state) was wiped:
   after the first rebuild, `journal_append` returned early forever. Fixed by carrying the persistence
   fields across the rebuild — `autosave_survives_an_undo_and_a_seek`.
2. **A replay after a save re-adopted the old pool** (and failed once it was gone): `save` re-pointed the
   live session at the copy but the history still named the original. Fixed with `rebase_pool`, which
   rewrites the `pool` command in the history (recursing into gestures) so history, script and the live
   session agree — `an_undo_after_a_save_uses_the_session_pool` (the original pool is deleted before the
   seek, undo and bounce).
3. **A `pool` line in the journal could stop a moved session from opening.** Journal recovery now
   **drops and reports** a refused command (`JournalRecovery::refused`/`refused_reason`) instead of
   failing the load — `a_stale_journal_is_dropped_not_fatal`.
4. **The save's rename→journal-reset window** left a stale journal replaying on the new baseline; the
   same tolerance covers it, the script is `sync_all`ed before the rename, and the ordering is
   documented.

Two refinements came out of the same review: `save` now **verifies its own output** (it parses the text
back and compares it to the history, refusing a lossy save — an id with whitespace, or a region play,
is refused rather than written) and `copy_pool` skips a file only when size *and* mtime match. The
review's A1 verdict was clean apart from a comment that overstated the fold-then-apply invariant.
The full review, verbatim, with the disposition and the debt it leaves open (journal `fsync`,
non-strict arity for `pool`/`save`/`load`) is
[`research/architecture/2026-09-23-alpha-slice-gate-glm-standin.md`](../../../../research/architecture/2026-09-23-alpha-slice-gate-glm-standin.md);
the designated gate stays **owed** for A1 and A2, with the retry order in
[the gate archive](../../../../research/architecture/2026-09-23-kimi-gate-alpha-slices.md).

## Alternatives considered

- **A bespoke serde/JSON snapshot of the session.** Rejected: the log *is* the document, and it is
  already a versioned text format with a parser and replay tests. A second representation would need its
  own versioning, could disagree with the log, and would not be the thing the CLI and the LLM seam read.
- **SQLite (or an append-only binary log) for the journal.** Rejected for alpha: the journal is a text
  file that can be read, diffed and repaired with `cat`, and the whole recovery path is then
  `parse_script` — the same parser, tested by the same tests. A database is a dependency and a second
  format for a problem the text form already solves.
- **One file with the audio embedded** (WAVs base64'd or concatenated). Rejected: it makes a large
  binary blob out of the one artifact a human and an LLM are supposed to read, and it duplicates writes
  of gigabytes on every save.
- **Save the derived `Timeline` snapshot alongside the log** (for a faster open). Rejected: a derived
  value that is not replayed will eventually disagree with the log — which is exactly the failure the
  architecture avoids. Open cost is replay, and improving it is the seek-checkpoint slice (E15).
- **Persist the pool by reference only** (absolute paths). Rejected: the plan's trap — a session that
  cannot be moved, backed up or handed to someone else. Copies cost disk and make the directory portable;
  files are skipped when they are already there at the same size.
- **Rewrite the *history's* `pool` command when a save re-points the live session at the copy.** Rejected:
  the in-session log stays authoritative for what the live session did, and the rewrite belongs in the
  *script* (which is context, like the rate). The two paths point at the same bytes, so a replay after an
  undo is still correct; and a session loaded from the directory has the relative path everywhere.
- **Autosave on a timer or on a fixed cadence.** Rejected: a timer writes when nothing happened and can
  still miss the last edit; the journal writes exactly what was committed, in order, immediately.
- **A binary journal with a checksum per record.** Deferred: the torn-line rule covers the failure that
  matters (a partial append), and a checksummed journal is a second format to version. Revisit if a
  partial write is ever seen mid-line rather than at the end.

## Consequences

- A session is a directory that can be moved, copied to a backup, or handed over — and reopening it is
  a replay, so it is the same session by construction.
- Crash recovery is `session.txt` + `journal.txt`, with the torn tail reported rather than guessed at.
- A save **re-points the live session at the copy** of its pool: from then on, what the script says and
  what the session reads are the same directory (the history may still name the original path, which
  exists because a save copies rather than moves).
- The rate is now context with a clear rule: it is fixed at construction, so a rate change is a new
  session — and `host v1` gains three lines (`session_rate`, `save`, `load`) additively, which an older
  host refuses at the exact line rather than misreading.
- **Two bugs found by self-review while the reviewer gate read the slice**, both now tests:
  1. *A crash inside a gesture* made the session unopenable (an unterminated `group begin` in the
     journal was a parse error). Recovery is now per **entry**: the incomplete gesture is dropped
     whole, so a cut mid-trim leaves the clip untouched rather than half-trimmed.
  2. *A load built state but not history.* `from_script` fed commands through `process`, which applies
     and logs but does not record the host history — so a session opened from a file could not be
     undone, and **saving it again wrote only what happened after the load** (losing the whole
     session). `from_script` now uses `execute`, and the test saves a reloaded session and asserts the
     baseline is still in its script.
- Still owed: session *versioning/migration* (only additive changes so far, so `v1` holds), a session
  browser / "recent sessions", autosave from the very first edit (today the journal begins at the first
  `save`), and pruning old journal lines once a save has folded them into the script.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-23.
