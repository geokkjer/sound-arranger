# Agent Note: the editing model is modal — audio's vim/helix, not a DAW's mouse

Status: proposed

## Problem

The shell question — Tauri + Vue, iced, or ratatui — is a decision about **rendering**. The app has
never decided its **interaction model**, and today it does not have one: the shipped Vue shell is
GUI-shaped, with panels, clicks and a canvas timeline driven by pointer events, and 38 frontend
tests behind its viewport/editing maths. That is a conventional DAW arrangement view. It works, and
it is the right *default* for a mouse user; it is not a model anyone chose.

The direction from the human (2026-09-21): **the UI should be for audio what vim / helix / emacs is
for text**, with the modal model — **visual mode for selecting** — as the likely fit.

This is not only taste, because the architecture is already editor-shaped in every respect except
its interface:

- the **session log is a typed, versioned command list** (`host v1`: `mount`, `patch`, `set_param`,
  `set_tempo`, `unmount`, `transport`, `undo`, `redo`, `play`, `splice`, `record`, `bounce`, `pool`,
  `arrange`), validated per slot against registries, with line-numbered errors
  (`crates/host/src/lib.rs`);
- the **arrangement is a value** (a `Timeline`), and every edit is a logged command;
- **undo/redo already replay the log** — the arrangement history exists, and the live shell drives
  it over `HostCommand`.

Text editors won their ergonomics by making exactly that — *commands over a value* — the interface:
modes, selections, registers, macros, `:`, scriptability, generated documentation. This repo has the
value and the command vocabulary and exposes them through a pointer. The engine thinks in vim;
the shell thinks in windows.

## Proposal

**Adopt a modal, selection-first, log-backed interaction model as the contract for *whichever* shell
wins**, and validate it in the TUI spike (the cheapest place to test a keyboard model). Proposed
concretely:

1. **Four modes.** `Normal` (navigate, select, act), `Visual` (extend/refine a selection, then act),
   `Insert` (text only: clip names, markers, search), `Command` (`:` prompt). The mode is always
   visible in the statusline; `Esc` always returns to `Normal`; nothing destructive happens in a
   mode the user cannot see.
2. **Selection-first (noun, then verb).** One `Selection` value with kinds — *clip*, *time span on a
   track*, *track*, *mixer channel*, *pool source* — always rendered, and every destructive command
   acts on it. Split / trim / move / duplicate / delete / fade become "select, then act", so the
   acted-on set is verifiable **before** it changes. This is the property that removes the need for
   confirmation dialogs a TUI has no room for.
3. **`:` is the existing vocabulary — do not invent a second one.** The prompt types what the CLI
   already parses, bindings dispatch the same commands, and both land in the same log. Keys, macros,
   tests, the LLM seam and the undo stack then share one vocabulary (the Emacs "everything is a
   command" property, which this repo already has latent).
4. **Registers, repeat, macros.** Named registers for clip selections (vim's `"a`), `.` to repeat the
   last edit, and recorded command sequences — all of which fall out of having a command list.
5. **Discoverability is part of the contract, not a nicety.** A prefix infobox (which-key style:
   show the labelled children of the key you just pressed), a command palette whose second column
   shows the bound keys, `:help`, an interactive tutor, and **documentation generated from the live
   keymap** so it cannot drift. A modal model without these is a maze.
6. **The mouse is a second input to the same commands.** A click dispatches the command its key
   would; there is no parallel pointer path, one log, one undo stack. (This is how the model stays
   honest in a GUI shell, and it is why the shipped shell does not have to be thrown away.)
7. **The Emacs half is extensibility.** Plugins contribute *commands + bindings + views*, not
   bespoke UI — which is the composition thesis again, and makes runtime-contributed UI an explicit
   open question in a compiled shell (see the [iced evaluation](2026-09-21-iced-shell-evaluation.md),
   criterion 4).
8. **Where the model must bend, it bends deliberately.** Live capture/recording is not text-like: the
   transport is global, always available, and identical in every mode — **recording is never modal**.
   Mixing is fader-like: a momentary grab mode (or per-channel command surface) rather than pretending
   a fader is a text motion.

## What already holds (2026-09-21, in the TUI spike)

Two of the criteria below are partly satisfied by the first slice built — which is why this note is a
proposal to *finish* a model rather than to start one:

- **Keys already dispatch the host's text format.** `x` (split) and `d` (delete) in the spike do not
  call engine internals: they build an `arrange …` line and hand it to the host's own parser
  (`host::parse_arrange_line`), which returns a `HostCommand::Arrange` the host logs like any other
  command. A refused op (a split at frame 0) is reported in the state line and never logged. **The
  `:` prompt is therefore a widget away, not a project**: it types the same vocabulary. *(`:` itself
  is not built.)*
- **Modes are visible and non-destructive to leave.** `NORMAL`/`VISUAL` is always on the state line,
  visual mode carries a frame-span selection with its duration, and `Esc` now leaves state and never
  quits — the safety key is not the destructive one.
- **Panel focus exists** (`Tab`/`Shift-Tab`, click-to-focus, a lit border), so one key space serves
  two panels: `j`/`k` mean "mixer channel" or "active track" depending on focus. That is the
  structural prerequisite for a multi-panel editor, and it is what makes "the session is a document"
  concrete.
- **Clip motions exist in miniature**: `n`/`N` jump the playhead to the next/previous clip — the
  arrangement's answer to "next word".
- **Move and trim are modal gestures** (2026-09-22): `<`/`>` trim a clip's start/end to the
  playhead, `H`/`L` move it one beat (the step comes from the session clock, so a nudge is musical,
  not a cell width), `J`/`K` move it to the track below/above keeping its time, and in visual mode
  **`t` trims the clip to the selection** — select, then act — consuming the selection and leaving
  visual mode, as Helix does. Every one of them is an `arrange …` line dispatched through the host's
  parser; the model has no private mutation path.

What is *not* built: the `:` prompt, registers/repeat/macros, the keymap as loadable data, the
prefix infobox, the palette, and the Emacs-style extensibility surface.

## One workflow, two shells (2026-09-22)

The owner's direction: **the modal, key-driven workflow is the product, and both shells implement
the same one.** The TUI is the reference implementation (it is where the model is proved, and it is
the primary shell); iced follows it key for key, and only then leans into what a GUI can do that a
terminal cannot — the emacs move: same keys and same commands, better rendering, real text
(completion, selection, dialogs), multiple windows.

Consequences to hold the two together:

- **No shell-only features.** A capability lands in the workflow (an action in the vocabulary, a key
  for it, a line in the keymap table) before or with the shell that shows it; a shell that cannot
  express something (a TUI cannot show a plugin editor, a GUI can hide the log) is a *rendering*
  difference, never a workflow difference.
- **The `host v1` vocabulary is the floor.** Every action is an `arrange`/`set_param`/`transport`
  line, so the `:` prompt, a script, and a key are the same thing — and a key that exists in one
  shell and not the other is a bug in the shell, not a feature.
- **The keymap is data, and today it is one table.** The TUI's `KEYMAP` const renders the `?`
  overlay *and* is the table the handler matches against, so help cannot drift from behaviour. When
  iced grows keys, the table is what moves: the honest next step is a shared definition (a small
  crate the shells' isolated workspaces both path-depend on, like `host`) rather than two tables that
  agree by discipline. Until then, the TUI's table is the reference, and a divergence is a bug.

The GUI-only upside is real and deferred, not denied: text entry and completion for the `:` prompt,
menus/dialogs, tooltips, multi-window layouts, and reusing the same widgets as a CLAP plugin editor
later ([export note](../architecture/2026-09-22-clap-export-via-nice-plug.md)).

## Alternatives considered

- **Pointer-first DAW UI (today, by default).** Panels, drag, canvas editing. Rejected as the
  *primary* model — it hides the log, makes the shell the product, and throws away an ergonomic
  advantage the architecture already paid for. **Kept as the baseline the model must not break**:
  the GUI shell keeps working, with the modal model layered on as keys + a visible mode + the same
  commands.
- **Pure REPL / live-coding (Tidal-style).** Everything is a typed command, no visual arrangement.
  Rejected: the arrangement **is** the product; a timeline has to be visible and shaped on screen.
  (This is also why the Helix study matters more than the Tidal lineage here.)
- **Tracker model (pattern grid).** A character-grid editing surface with decades of proven
  ergonomics — but it models *step/pattern* music, not a clip arrangement over recorded audio. Not
  rejected as an inspiration (the grid is a good visual vocabulary for a clip matrix); rejected as
  the editing model for timeline audio.
- **Menu-driven TUI with no modes** (nano/micro style). Fewer concepts, trivially discoverable, but
  the reason to consider a TUI at all is speed and composability; vim/helix answered "powerful and
  discoverable" with modes *plus* strong discovery aids, which is the combination proposed here.
- **Modal keys + drag-first timeline (a true hybrid) as the *first* step.** Not rejected — arguable
  end state for the GUI shell, and criterion 6 in the proposal is the bridge. Not chosen as the
  first step because a modal model is cheapest to test where there is no mouse to fall back on.

## Acceptance criteria

The model is decided (this note becomes implemented, or is rejected) when:

1. **A keymap spec exists as data** — modes × bindings × commands — loadable by the shell, with the
   `host v1` verb set as the command vocabulary, and a test asserting every binding names a real
   command (no binding can drift from the log).
2. **`:` and the keys dispatch the same commands** in the spike, both visible in the log; a scripted
   session and a hand-played session produce the same event sequence.
3. **Everything is reachable and documented from the model**: every command appears in the palette
   with its bound keys, and the in-app help is generated from the keymap (no hand-written second
   copy).
4. **A real session works keyboard-only** — select a clip, split, move, duplicate, mute a channel,
   set a marker, bounce — with no mouse, and the session reports **keystrokes and mode switches per
   gesture** so the claim ("fewer, and composable") is measured rather than asserted.
5. **One gesture is one undo step**, verified in the log: a drag-equivalent operation appears as a
   single logged command, not one per key repeat.
6. **The shipped GUI shell still works**, and the model is additive to it (keys + a visible mode over
   the existing commands), not a replacement of the pointer path.

## Risks

- **The learning cliff is real.** Modality is a bet on a user who will invest; vim's ergonomics are
  also its reputation. The mitigation is criteria 3 and 5 of the proposal (discovery aids are not
  optional), and shipping `Normal` mode as fully usable without `Visual`.
- **Audio is two-dimensional and continuous; text is one-dimensional and discrete.** "Next word" has
  no obvious audio analogue — it has several (next clip boundary, next beat, next marker, next
  transient, next channel). Choosing those motions *is* the design work, and getting them wrong is
  how a vim-for-audio becomes unusable rather than powerful.
- **Recording and mixing resist modes.** Handled by proposal item 8, but it means the model is not
  uniform: there will be a global transport rule and a fader exception, and inconsistency is exactly
  what makes modal UIs frustrating.
- **The GUI shell could rot into a second-class citizen.** If the modal model is the "real" model,
  the Vue shell risks becoming a viewer. Criterion 6 exists to prevent that, but it needs real
  attention, not a checkbox.
- **Scope.** This is a genuine product-level bet: it shapes the keymap, the help system, the plugin
  surface and the shell choice. It should be validated on the TUI spike (cheap, keyboard-native)
  before any of it is built into the shipped shell — which is exactly why the shell evaluation and
  this note have to be decided together.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21. The direction is the human's
("what we want from ui/tui is to be for audio what helix/vim/emacs is for text … visual mode for
selecting"); the supporting evidence is the [Helix + terminal-drawing study](../../../../research/architecture/2026-09-21-tui-audio-prior-art.md)
and the existing `host v1` command contract.
