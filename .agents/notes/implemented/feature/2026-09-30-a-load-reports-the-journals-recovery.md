# Agent Note: a load reports the journal's recovery

Status: implemented

Implemented 2026-09-30 in `ddb03a7` (`slice/recovery-report`).

## Problem

Since the hardening pass the host has counted what a session-directory load recovered from the
journal — commands applied, lines torn, entries refused, and the first refusal's words
(`JournalRecovery`, [the journal
note](../architecture/2026-09-23-session-directory-save-and-journal.md)) — but the
record was API surface no shell printed. A user recovering a crashed session was told nothing about
what the crash cost, while the host held the exact numbers. The gap was found writing
[FIRST_SESSION.md](../../../../docs/FIRST_SESSION.md) Step 5, whose crash-recovery walkthrough had
to say the report was invisible; `docs/beta-acceptance.md`'s checklist item ("a torn last line is
reported") was, in the shells, false as written.

## Decision

The recovery record gets a **read-side sentence**, and the surfaces that already report a load say
it:

1. **`JournalRecovery::describe()`** — one wording (`journal: N applied, M torn, K refused`, plus
   the first refusal's own words when anything was refused), built on the caller's thread. This is
   the same read-side discipline as `ApplyFault::describe`: the record is data; the sentence is a
   read. **`has_story()`** gates it — a clean load of an empty journal has nothing to say and stays
   quiet, so the report means *a crash story*, not a ritual.
2. **`HostHandle::last_recovery()`** — a `Recovery` request to the live actor, so a shell can ask
   the resident session what its last load recovered. Read-only, like `outcome()`.
3. **The terminal shell** appends the counts to the `: load` status line (only when the parsed line
   carried a `Load`, so ordinary commands pay nothing for asking).
4. **The headless binary** prints the line before the bounce, gated the same way.

Test: `a_load_reports_the_journals_recovery` (tui) — a clean load's status line stands alone, one
intact edit plus a torn tail line reports `journal: 1 applied, 1 torn, 0 refused`;
`a_recovery_report_describes_itself` (host) pins the wording.

## Alternatives considered

- **Put the recovery into `HostOutcome`** so every `load` call returns it. Rejected for this slice:
  `outcome()` is the post-edit bridge shape used per keystroke; recovery is a once-per-load fact,
  and widening the outcome for it taxes the common path for a rare one. A dedicated read
  (`last_recovery`) costs one channel round-trip exactly when a `Load` was executed.
- **Print the recovery from `load_session` itself** (host-side, unconditional). Rejected: the host
  is a library; printing belongs to the binary or the shell. It also cannot be gated by "the user
  ran a load line" there.
- **Report the recovery as a status-line warning whenever nonzero** (torn/refused only, ignoring
  `applied`). Rejected: `applied` is half the story the tour tells — "your edits came back" is the
  reassuring half, and hiding it makes the line read as an error when the recovery succeeded.
- **Do nothing (leave it API surface).** Rejected when the docs had to document the invisibility:
  a capability the host measures and the user cannot see is a gap, and the walkthrough was the
  proof.

## Consequences

- A recovered session now tells the user what the crash cost, in both the terminal shell and the
  headless binary, with one shared wording owned by the host.
- `beta-acceptance.md` §5's "a torn last line is reported" is true again as written, and
  FIRST_SESSION Step 5 shows the line in its expected output.
- The iced shell does not surface the report yet; it shares the workflow crate but has its own
  command surface, and its load path can adopt `HostHandle::last_recovery()` when it grows one —
  the seam is deliberately small so that is a one-call change.

Authored with GLM-5.3 · OpenCode, 2026-09-30.
