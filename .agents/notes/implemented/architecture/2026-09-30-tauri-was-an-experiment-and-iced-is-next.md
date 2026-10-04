# Agent Note: Tauri was an experiment we decided against — and iced is the shell we take to the recorder

Status: implemented

## Problem

The [2026-09-22 shells note](2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md) retired the
Tauri + Vue shell for cost, but kept a warmer framing than the decision deserves: it called Tauri
"the right thing to build first", "the only shell that reached a full editor" and "the porting
reference for the Rust shells". The docs inherited that framing and spread it — the
[explainer](../../../../docs/architecture-explainer.md) calls it the first rich shell and credits
its layout as the ports' reference, the
[theory](../../../../docs/theory-of-the-program.md) still says "**our Tauri app is** a reference
implementation of the host", and
[README](../../../../README.md)/[RESEARCH](../../../../RESEARCH.md) promise it as "the porting
reference" in the present tense. `crates/host` and the spikes still describe their seams as "the
Tauri bridge" as if it were a live client.

Two things are wrong with that. First, the shell was an **experiment the owner tried and decided
against** — the cost that retired it (TypeScript + Vue beside Rust, a webview, an IPC wire, a serde
bridge, a `!Send` session behind a mutex, an npm toolchain in the build, webkit/GPU on the deploy
path) is the complexity the Rust shells exist to remove. A decision against an *approach* is not a
pause in its use, and a doc that keeps inviting the port to consult it keeps the approach alive.

Second, it left the shell direction without a forward edge. "The TUI is primary, iced is second"
says which shell is first without saying what the second is *for*, or when it happens — so the
only forward-looking sentence in the docs is the one promising a Tauri shell that was abandoned.

## Decision

**Tauri + Vue is an approach we tried and decided against.** `crates/shell` is retired code kept
frozen and out of the workspace until someone deletes it; it is **not** a design reference, a
porting target or a present-tense client of the Host API. Docs describe it in the past tense, as
history, and no live document credits it with a role.

**The forward shell plan is iced, taken to the recorder profile — after the recorder
implementation is good enough.** The sequencing, in the owner's words and this note's rule:

1. The **recorder** profile is the current focus (the
   [profiles decision](../../proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md)):
   send a clock to external gear, capture, align, master, export.
2. The **TUI is the working shell** while that is built — it is primary, and it is where the
   workflow can be asserted rather than watched.
3. When the recorder is good enough, **the iced shell is brought up to the recorder profile**:
   the same `host v1` commands, the same `workflow` actions, the same modal key-driven model —
   with iced's GUI-only surfaces (real text, dialogs, multi-window) as the reason it exists.

The parity rule is unchanged and unchanged *by* this: no shell gets a private feature, and iced
reports the actions it cannot render yet rather than dropping them silently. What is new is the
target: iced is not being kept warm as an alternative, it is the shell that carries the recorder
once there is a recorder to carry.

## Alternatives considered

- **Keep "retired, but the porting reference".** Rejected by the owner: it invites the next shell
  author to consult a stack the project decided against, and it keeps a dead approach in the
  present tense of three documents. The dated reasoning in the 2026-09-22 note is untouched; what
  changes is the role the live docs assign it.
- **Delete `crates/shell` in this change.** Not rejected on merit — the 2026-09-22 note already
  calls deletion a one-command follow-up — but it is a large deletion that belongs in its own
  commit with its own review, not folded into a docs pass. Freezing costs nothing meanwhile.
- **Bring iced up to parity alongside the recorder.** Rejected: the
  [alpha finish line](../../proposed/architecture/2026-09-23-alpha-finish-line.md) already rejected
  chasing iced parity during alpha, and the recorder is the focus; two fronts means neither lands.
  Sequencing is the whole point of this note.
- **Make iced the primary shell now.** Rejected: nothing here reopens the primary question. The
  TUI stays primary because the workflow can be asserted in it (deterministic frames, a pty
  harness) and because a terminal cannot cheat with a mouse.
- **Edit the 2026-09-22 note to say this.** Rejected: the standing order is supersede, never
  rewrite a decision. That note keeps the 09-22 reasoning; this one overrides its framing and
  links back to it.
- **Leave the Tauri mentions in source comments alone.** Rejected for the ones written in the
  present tense ("the shell a Tauri frontend will eventually mirror", "the actor the Tauri bridge
  uses"): a comment that names a client that will never exist is a small lie in the code, and the
  fix is a tense.

## Consequences

- **Live docs stop crediting Tauri with a role** — `AGENTS.md`, `README.md`, the explainer, the
  theory, `RESEARCH.md`, `crates/shell/RETIRED.md` and the two spike READMEs describe it as an
  approach tried and decided against, kept as frozen history. Dated notes and the
  `research/` snapshots are left exactly as written, because they are the record.
- **The forward edge is written down**: iced carries the recorder profile when the recorder is good
  enough. This is a plan, not a start date; no iced work begins before that, and if the sequencing
  changes it changes in a new note.
- **The shells note keeps its decision and gains a pointer here** — the superseded framing is
  marked where a reader would otherwise still find it.
- `crates/shell` stays excluded from the workspace build; deleting it remains a one-command
  follow-up whenever the owner prefers the tree without it.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
