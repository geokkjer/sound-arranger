# Prior art & references for a terminal shell: Helix's keyboard model, and what a terminal can draw

Status: research / prior-art study (2026-09-21). Observations and design inputs only — the decisions
this informs live in the [TUI evaluation note](../../.agents/notes/proposed/architecture/2026-09-21-tui-shell-evaluation.md)
and, for the shell choice overall, the [iced evaluation note](../../.agents/notes/proposed/architecture/2026-09-21-iced-shell-evaluation.md).

**Why:** the TUI spike proved the cheap half (window, host thread, transport, live meters — see
[`spikes/tui-shell/`](../../spikes/tui-shell/)) and left the decisive half open: *what is the
timeline in a terminal, and can keys carry the workflow?* This pass gathers the two things that
answer those: a keyboard-interaction model worth stealing, and the measured drawing resolution of a
terminal.

**Sources:** the [Helix](https://github.com/helix-editor/helix) source at
`079a789e` (master, 2026-07-23) read directly plus its book; `ratatui` 0.30.2 / `ratatui-core`
0.1.2 read from the crate sources vendored in `~/.cargo/registry` — i.e. the exact versions this
repo resolves; and a web survey of terminal audio tools (see §3). Every claim is marked **[V]**
verified by reading the cited source/doc, or **[I]** inferred.

---

## 1. Helix — the keyboard model, as a reference for a keyboard-driven audio editor

Helix does **not** answer the audio-file view — there is no waveform, no time axis, nothing spatial.
What it answers is the part a bespoke TUI normally gets wrong: how a large command surface stays
learnable, discoverable, and remappable.

### 1.1 Selection-first ("noun, then verb")

**[V]** From `book/src/from-vim.md` and `book/src/usage.md`: whatever you are going to act on is
**selected first, and the action comes second**; a cursor is simply a one-width selection. This is
the Kakoune model, not vim's.

**[V]** Mechanics: `Mode::{Normal, Select, Insert}` (`helix-view/src/document.rs`). The select-mode
keymap is literally the normal map cloned and then overridden with `extend_*` motions
(`helix-term/src/keymap/default.rs`): `v` enters select mode, `h/j/k/l/w/b/e/t/f` become
`extend_*`, `esc`/`v` leave it. Selection shaping: `x` line, `%` whole buffer, `;` collapse to a
one-char cursor, `,` keep only the primary selection.

**What it buys [V]:** the acted-on set is *rendered* before anything is destroyed, so `d`/`c`/`y`
need no operator grammar and no pending-operator state; counts and `.` (repeat) apply to a stable,
re-selectable thing; `wd` reads left-to-right as "point, then act".

**What it costs [V]** (`from-vim.md`): `dw` becomes `wd`; "delete to end of line" needs `vgl d`;
there is no block-select mode; a select-mode toggle is needed when sharing the current selection.

**[I] For clips this is a better fit than for text.** A clip selection *is* `{track, clip, start,
end}` — a value this project already has. `d` delete, `y`/`p` copy/paste, `c` trim-to-selection,
`x` select the clip under the playhead, `%` select all clips on a track, `;` collapse to the
playhead. The safety property a destructive editor wants — *see exactly what will change before it
changes* — falls out of the model instead of being bolted on as a confirmation dialog.

### 1.2 The keymap is data (and the defaults are labelled data)

**[V]** The core type is a **trie keyed by key events**, with labels:
`KeyTrieNode { name, map: IndexMap<KeyEvent, KeyTrie>, is_sticky }` and
`enum KeyTrie { MappableCommand, Sequence(..), Node }` (`helix-term/src/keymap.rs`). Defaults are
built by a `keymap!` macro with human labels that become popup titles
(`helix-term/src/keymap/default.rs`, `macros.rs`).

**[V]** User config deserializes into the *same* trie (`KeyTrieVisitor`,
`helix-term/src/keymap.rs`), so the TOML can express a command, a **sequence** of commands (a
mini-macro), or a nested prefix node — with labels:

```toml
[keys.normal]
C-s = ":w"                              # a typed command as a binding
g = { a = "code_action" }               # a prefix node
"ret" = ["open_below", "normal_mode"]   # a sequence
[keys.normal."+"]                       # a new minor mode under '+'
t = ":run-shell-command cargo test"
```

**[V]** Overrides **merge, user wins** (`merge_keys` → `KeyTrieNode::merge`): a leaf replaces a
leaf or a node, nodes merge recursively, and `no_op` unbinds. There is no "key already bound"
error — conflicts are only asserted inside the *default* keymap (a startup panic). Config layers
(global then per-project) merge, and the whole map sits behind an `ArcSwap` so a config edit
**swaps keymaps without a restart**.

**[I] The sharp bit:** keys are data, *commands are a closed enum*. Users can bind, sequence and
remap; they cannot define new commands. That is the right split for us too — the vocabulary should
be the app's own, and no user TOML should be able to invent an operation the session log cannot
replay.

### 1.3 Discoverability — the part a bespoke TUI usually fails

**[V] Which-key popup for free.** On any prefix, `KeymapResult::Pending(node)` sets the editor's
`autoinfo` to `node.infobox()`; `infobox()` walks the node and **merges keys that share a
description onto one row**, titled by the node label (`helix-term/src/keymap.rs`,
`ui/editor.rs`, rendered by `ui/info.rs`). Pressing `g`, `m`, `z` or `Space` therefore *shows you
the menu*; there is no help screen to memorise. Sticky nodes keep the popup open.

**[V] A command line as the escape hatch.** `:` opens a prompt with completion over the typable
command list, aliases, flags and per-argument completers, plus a doc pane that prints the command's
documentation as you type (`commands/typed.rs`, `helix-core/src/command_line.rs`). Commands are
declared as data — `TypableCommand { name, aliases, doc, fun, completer, signature }` — including
**arity/flag validation**.

**[V] A command palette that answers both directions.** `Space-?` chains static + typable commands
into a picker whose second column prints the keys currently bound to each, built from a reverse
command→keys map (`commands.rs`). One fuzzy search answers "how do I X" *and* "what does this key
do".

**[V] Pickers are the primary navigation**, not a feature: the `Space` node is almost all pickers
(files, buffers, diagnostics, symbols) fuzzy-matched with fzf syntax.

**[V] The statusline is configured data**: `[editor.statusline] left/center/right` lists elements
from `StatusLineElement` (`Mode`, `Selections`, `Position`, `Diagnostics`, …) — the selection count
is a first-class element.

**[V] The docs cannot drift**: `xtask/src/docgen.rs` generates the book's command tables with a
"Default keybinds" column produced from the live keymap; an interactive in-buffer tutor
(`runtime/tutor`) ships as data.

### 1.4 UI primitives Helix implements (for a TUI of this shape)

**[V]** A **compositor layer stack** (`helix-term/src/compositor.rs`): `Vec<Box<dyn Component>>`
with push/replace/remove, events bubbled from the front layer backwards, and a `Component` trait of
`handle_event -> Consumed | Ignored`, `render`, `cursor`, `required_size`. On top of it: **Prompt**
(minibuffer with completion, history, `Update/Validate/Abort` events), **Menu** (fuzzy-filtered
option table), **Popup** (anchored container with size negotiation, position bias, auto-close),
**Picker** (fuzzy, columns, preview), **Info** (the infobox), **Statusline**. Everything draws
through `helix-tui`, a vendored fork of tui-rs.

**[I]** We would use plain `ratatui` (already the spike's base) and take the *architecture*:
compositor + keymap-trie + infobox/prompt/picker, not the text engine.

### 1.5 What does not transfer

**[V]** tree-sitter syntax-node selection and text objects, surround, LSP (hover/rename/code
actions/diagnostics), the rope + `ChangeSet` + grapheme-boundary machinery, multi-cursor regex
selection, word motions, registers and shell pipes, file previews, `languages.toml`. **Note also
[V]: Helix has no `runtime/keymap/` file — defaults live in Rust; `runtime/` holds only grammars,
queries, themes and the tutor.**

**[I]** The parts worth carrying over: selection-first for clips, a labelled keymap trie with
user-wins merging, the prefix infobox, a `:` prompt, a command palette with a bindings column, the
statusline-as-data idea, and generated documentation.

---

## 2. What a terminal can actually draw (verified against our pinned versions)

**[V]** `ratatui-core` 0.1.2 `symbols/marker.rs` + `ratatui-widgets` 0.3.2 `canvas.rs`
(`marker_to_grid`) already ship the sub-cell plotting primitives, so "can a terminal show a
waveform" is a *density and colour-granularity* question, not a feasibility one. The grid
resolutions below are read from `Canvas::marker_to_grid` and each grid's `resolution()`, not from
prose:

| `Marker` | grid per cell | notes |
|---|---|---|
| `Dot` / `Block` / `Bar` / `Custom` | 1×1 | one point per cell |
| `HalfBlock` | **1×2** | `resolution() = (width, height * 2)` — U+2580/U+2584; **colour per sub-pixel** |
| `Quadrant` | 2×2 | dense, no visible bands between cells |
| `Braille` | **2×4** | Unicode Braille Patterns; needs a font with them |
| `Sextant` | 2×3 | legacy-computing block (2021) — patchy font support |
| `Octant` | 2×4 | same resolution as braille, dense (no bands) |

**[V] The colour asymmetry matters and is easy to miss.** `PatternGrid` (braille, quadrant,
sextant, octant) stores `Vec<PatternCell>` — **one cell per terminal cell, therefore one foreground
colour per cell**; `paint()` is last-write-wins and there is no per-pseudo-pixel background.
`HalfBlockGrid` instead stores `Vec<Vec<Option<Color>>>` at `height * 2` — **a colour per
sub-pixel** (upper/lower half plus background). So the two are good at different things: braille
and quadrant for *shape at resolution*, half-block for *colour at low resolution*. That maps
directly onto a timeline: envelope lanes want braille; clip colour bars, meters and the playhead
want blocks.

**[V]** `Canvas` (`ratatui-widgets/src/canvas.rs`) offers `marker()`, `draw()`, `layer()`,
`paint()`, `print()`, `get_point()`, with shape modules (circle, line, points, rectangle, world
map); `Sparkline` gives a one-row overview; `Chart` gives datasets with axes.

**[I] The arithmetic for a 200×50 terminal.** Braille/octant gives 400×200 addressable points. A
10-minute arrangement across 200 columns is **1.5 s per horizontal dot** — useless for individual
transients, exactly right for the *envelope* a DAW shows when zoomed out. Per track, 8 rows of
lanes × 4 dots = 32 vertical dots of amplitude, which is more than a SoundCloud waveform uses. So
the honest framing is: **a terminal can draw a convincing overview/envelope timeline; it cannot
draw samples, and it cannot draw the fine peak detail a 4K GUI shows when zoomed in.**

**[V]** The data is already in this repo: `media::peaks::PeakBuilder::{push,finalize}` build a
pyramid, `range_minmax(start, end)` returns the min/max over a frame range (exactly one column's
worth), `media::peaks::PeakData::read(path)` reads the pool's `.peaks` sidecars, and
`media::wav::WavReader::open(path)` + `read_into(&mut [f32])` reads any WAV. A waveform view is a
small step from the existing spike.

## 3. Prior art: terminal audio tools, and the gap

Method: a survey of terminal audio software run as part of this study, plus first-hand reads of the
projects that matter most. **[V]** here means the repo/README/source was fetched and read (by me or
by the survey, from primary sources — raw files, manpages, crates.io); **[I]** is inference. GitHub's
REST API rate-limited the survey mid-run, so a couple of star counts are unconfirmed; every project
below was reached at a real URL.

### 3.1 The two the human found

| | `MrDopey/audio-tui-editor` ("audioedit") | `ematth/audio-tui` |
|---|---|---|
| Language | Rust, MIT | Python, **no license** |
| Activity | created 2026-08-30, **80 commits in a week**, releases v0.1.0 → **v1.2.2 (2026-09-06)**, deb/rpm/macOS/Windows artifacts **[V]** | **exactly one commit, "initial commit", 2026-01-15**, untouched since **[V]** |
| Users | **0★** | **1★** |
| Provenance | unambiguously agent-built *and maintained*: `vibe-coded` topic, `.claude/CLAUDE.md`, an `AGENT.md` written "from a review of past sessions", `Co-Authored-By: Claude Sonnet 5` on every commit, and a `design.md` that is a spec with numbered acceptance criteria **[V]** | unambiguously one unsupervised agent run: the `AGENTS.md` is an **unfilled template** (literal `[[ interaction 1 ]]` placeholders, `MAX_ITERATIONS = 10`, "do not stop until these are completed"), and there is **no README at all** **[V]** |
| Scope | browse → audition → locate → trim → metadata → safe save; Vim-modal (BROWSE/PLAY/EDIT/METADATA + `:` command line); ffmpeg/ffprobe for all codec work; rodio playback; on-disk waveform cache; folder-wide batch trim with dry-run; atomic staged save **[V]** | effects over a single file: gain/delay/highpass/lowpass/normalize/reverse on numpy/scipy, `blessed` UI, 8 pytest modules **[V]** |
| Waveform | `src/ui/waveform.rs`: one bucket per column, **peak outline + RMS fill** using **1/8-block glyphs**; no braille, no sixel. Its only image work is hand-rolled **Kitty graphics for embedded cover art** **[V]** | `ui/waveform.py`: **mirrored `█` bars** around a centre row, 1–100× zoom, scroll, playback column, selection markers; playhead is **wall-clock derived**, not sample-accurate **[V]** |
| Verdict | the most *disciplined* small TUI editor: real tests, real release engineering, explicit non-goals ("not a DAW, not multitrack, no timeline") — but ~a week old, one author, zero users, and its edit model is **destructive in-place trimming of one file** | a creditable student-scale sample published as-is — **a dump, as suspected** |

**The contrast is the lesson:** the same "agent + spec" method produced a maintained, release-managed
project in one case and a single-shot artifact in the other. Neither has a timeline.

### 3.2 The one that matters most: `tui-wave`

`biomassa/tui-wave` (Rust, MIT) is the closest prior art to *any* of this repo's shells, and it is
alive: **releases to v2.14.0 on 2026-09-21** [V]. From its README [V]:

- "A keyboard-driven audio editor that runs in a terminal (**mouse works too!**)".
- **Zoom from the whole file down to single samples.** "Terminals such as kitty and ghostty get
  graphics. **Every other terminal gets approximation via braille glyphs.**" — i.e. it ships exactly
  the two-tier rendering this study recommends, with braille as the floor.
- **Editing**: cut, copy, paste, delete, undo, **with a separate undo stack per open file.**
- Streaming mode for >4 GB files, so it never loads them into memory; RF64/BW64; mono/stereo/multi.
- A front end to **CDP** (405 processes), Praat (457) and Airwindows (500) — **1362 processes** in one
  browser, chainable.
- And, plainly: **"An LLM helped to write this program. I am not a Rust developer."** [V]

Its limits are the same as everything else here: **one file, one buffer, in-place edits.** No tracks,
no clips, no arrangement.

### 3.3 The wider field

**Terminal DAWs.** `tek` (Codeberg, Rust + ratatui, AGPL3, 1976 commits, 2026-09-15) has an
**Arranger Mode** and JACK/PipeWire + LV2 hosting — but **no waveform module at all** [V].
**Phosphor** (`joshjetson/phosphor`, Rust + ratatui, ~60 crates.io releases 2026-03→09) bills itself
as "a DAW that runs entirely in your terminal" with synths, a track view and a **clip view with piano
roll + automation** — but it is a **MIDI/synthesis** DAW; no audio-waveform editing was verified, and
its star count is unconfirmed [I]. `imbolc` (Rust + SuperCollider, alpha) and `tuidio` (Python
multitrack *recording*) fill out the class; `tuidio` has no clip system [V].

**Trackers** are the character-grid lineage but mostly **not terminal apps**: Impulse Tracker is
DOS/VGA text mode, Schism Tracker and MilkyTracker render through SDL windows, OpenMPT is Windows
GUI. `openmpt123` *is* a terminal player (line-redraw, per-channel VU meters in ASCII, live pattern
grid) with **no waveform**. Genuinely ncurses trackers exist but are tiny and self-declared unstable
(PLEBTracker, 193★; ColliderTracker, "definitely unstable and chock full of bugs") [V].

**Waveform/spectrum rendering** is the *solved* part, which is the opposite of what the premise
assumed:

- **Prism** (`Boof2015/prism`, C++/FTXUI, active 2026-09-18) draws Spectrum, Waterfall, Oscilloscope,
  Vectorscope, Spectrogram, **Waveform**, VU and LUFS **at 60 FPS in a plain terminal** [V] — a meter
  rack, not an editor, but proof that the rendering ceiling is high without any image protocol.
- **cava** (6427★, pushed 2026-09-21) does spectrum bars with the 1/8-block ladder, **no braille**,
  and its reusable piece is the **raw output mode** that pipes bar heights to stdout [V].
- **Open Cubic Player** (440★) has an ncurses FFT analyzer, oscilloscope and peak meter — the
  strongest visualizer precedent [V]; **ncmpcpp** draws both time-domain wave and spectrum via fftw
  (off by default) [V]; **scope-tui** (Rust, 720★) is an oscilloscope/vectorscope TUI [V].
- **[`waveformchart`](https://github.com/bcherb2/waveformchart)** (Rust, MIT, v0.1.0 2025-12-09,
  ~338 lines) is a **ratatui widget**: `WaveformWidget` / `WaveformMode`, braille (4× vertical
  resolution) or block, dual-channel — i.e. the drop-in building block [V]. **Caveat, verified from
  its `Cargo.toml`: it depends on `ratatui = "0.29"`, and this repo is on 0.30.2** — so it is a
  *pattern to copy or fork*, not a dependency, unless it is bumped.

**Players with library TUIs** (`cmus` 6247★, `termusic` 2213★, `musikcube`, `moc`, `spotify-tui`)
have **no waveform/spectrum** — verified by grep for cmus/termusic; musikcube's only spectrum view is
Win32 GDI, not terminal [V]. They are relevant for *browsing/queue* patterns only.

**Interactive/REPL audio**: `ecasound -c` (EIAM) is a readline command language and curses is used
only for terminal width — a grep for `cut|copy|paste|undo|redo|split|trim` over the tree returns
**zero hits**, and the `cop-*`/`ctrl-*` prefixes are **chain operators (effects)** and
**controllers**, not clip copy [V]. **Nama** (Perl, on Ecasound) is the real text-based *multitrack*
recorder/mixer with marks and regions — but a command REPL with no visual arrangement, and dormant
[V]. Terminal-native REPLs (sclang, ChucK, Csound, Extempore, Overtone, Sardine, Orca) **none** render
a waveform or spectrum in the terminal; SuperCollider's scopes are Qt [V]. `sox` and `ffmpeg` are
batch CLIs whose spectrogram/waveform outputs are **PNG files** [V].

### 3.4 The gap, stated precisely

**No terminal program renders an audio waveform in a multi-track arrangement/timeline view.** Every
waveform-capable terminal program is exactly one of three things: (a) a **single-file sample editor
with one horizontal playback timeline** — `tui-wave`, and destructively `audioedit`; (b) a **live
scope/spectrum with no file at all** — cava, Open Cubic Player, ncmpcpp, Prism, scope-tui, sgram-tui;
or (c) a **MIDI/piano-roll DAW** — Phosphor, imbolc. No timeline, therefore no arrangement view.

Specifically absent, and each was looked for:

1. **A non-destructive clip model** — clip = (source region, timeline position) with
   move/split/duplicate/trim that never rewrites the source, plus undo *over the arrangement*.
   ecasound has zero such vocabulary; Nama has "regions" but no visual arrangement; `tui-wave` edits
   one buffer in place.
2. **Capture and arrange in one TUI** — `tuidio` records without a clip system; `tek` has an
   arranger without a waveform; `tui-wave` has waveform + cut for one file.
3. **Auditioning an arrangement** — scrubbing a multi-clip timeline and *hearing* it. Nothing does
   it.

So the gap is **narrow but precisely located**: not "terminals can't show waveforms" — `tui-wave`
proves they can, `waveformchart` is a ~338-line braille widget, and Prism runs a full meter rack at
60 FPS — but **"no terminal program has a multitrack timeline with an audio clip model."** Offline
processing (this repo's deferred *sound sculptor* profile) is already well covered by tui-wave + CDP,
`sox` and `ffmpeg`; that is not the gap either.

**Two consequences for this project's plan.** First, any claim on our side that a terminal needs an
image protocol to draw audio usefully is **too strong** — plain-terminal rendering is proven. Second,
**two of the three closest projects (`tui-wave`, Phosphor) were LLM-assisted** and `audioedit` was
agent-built outright: the *rendering* is now cheap for anyone. The moat is the **clip/timeline model
and the log**, not the drawing.

## 4. Design inputs — interaction and discoverability

These are the parts of Helix's model that translate, stated as decisions the TUI shell has to make.
None of them are implemented yet — the spike deliberately ships the smallest thing that proves the
seam (`?` overlay, plain keys, mouse hit regions).

1. **Selection-first for clips.** Make one selection value (`{track, clip or span, anchor, head}`)
   the shell's primitive, render it always, and let `d`/`y`/`p`/`c` act on it. This is the
   single highest-value import: split, nudge and duplicate become *verifiable before destructive*,
   which is exactly the safety property an editor over someone's recorded material needs — and it
   removes the need for confirmation dialogs, which a TUI has no room for.
2. **Bulk selection must be escapable.** Multi-selection (all clips on a track, all clips at a
   marker) needs collapse-to-one and keep-primary keys, or a mis-fire becomes a mass edit.
3. **The keymap is data, and defaults are labelled data.** Rust defaults so the app is never
   unusable, `[keys.<mode>]` TOML overriding with "user wins" merge, `no_op` to unbind, sequences
   for mini-macros. **Commands stay a closed vocabulary** — Helix lets users compose bindings but
   not define commands, and that is the right split for us for a second reason: a user-defined
   command is a session-log value that cannot be replayed.
4. **Replace the `?` help screen with a prefix infobox.** The spike's overlay is a full-screen
   list; Helix shows the *labelled children of the prefix you just pressed*, merged by description,
   bottom-corner, on the key itself (`KeyTrieNode` labels + `infobox()`). That is strictly better
   discoverability for the same cost, and it grows with the keymap instead of being a second thing
   to maintain.
5. **A `:` prompt — whose vocabulary already exists.** **[V]** The host already defines a typed,
   versioned command vocabulary with per-slot validation: `parse_script` accepts
   `mount`, `patch`, `set_param`, `set_tempo`, `unmount`, `transport play|stop|seek`, `undo`,
   `redo`, `play`, `splice`, `record`, `bounce`, `pool`, `arrange` — plugin/port/param names
   validated against registries, with line-numbered errors (`crates/host/src/lib.rs`). That is
   Helix's `TypableCommand` role, already shipped, and it is also the LLM seam and the replay log.
   **So the TUI needs a prompt widget, not a new command language**: `:` should type the same text
   the CLI reads, and the log gets the same events.
6. **A command palette with a bindings column.** Build the reverse command→keys map from the
   keymap trie and show it in the palette — one fuzzy search answers "how do I X" and "what does
   this key do", which is what makes a large key surface learnable.
7. **Statusline as data, with the selection always visible.** Helix's statusline is a configured
   list of elements and the selection count is first-class. For us: transport, position, selection,
   and the last command's UI-thread latency (the spike already shows the last of these).
8. **Derive documentation from the keymap.** Helix generates its command tables' "default keybinds"
   column from the live keymap, so docs cannot drift. The spike already renders its `?` overlay from
   the same `KEYMAP` table the handler matches, and a test asserts every entry appears — extending
   that idea to the README table (or generating it) removes the last manual copy.
9. **One gesture = one undo step.** Helix checkpoints undo on mode exit rather than per keystroke.
   Our host's `Undo`/`Redo` operate on the arrangement history, so the shell must group a drag/DSP
   gesture into one logged edit rather than emitting a command per key repeat.

## 5. Design inputs — the timeline

**The headline [V/I]:** at 200×50 cells a terminal gives **200–400 distinguishable time columns and
100–200 amplitude steps**. That is a genuinely useful clip-arranger *overview*; it is not
sample-level editing. Everything below follows from that.

**Built, not just argued (2026-09-21).** The prototype now exists in
[`spikes/tui-shell`](../../spikes/tui-shell/) — `--wave <file.wav>` renders a real file as a braille
min/max envelope with a ruler, zoom/scroll, a playhead and a visual-mode selection. It was verified
against a known signal (loud → silence, which draws as the *centre line* → quiet) and driven
interactively under a pty (`v`, `h`/`l`, `+`). Two numbers come out of it: a 3-second file fitted to
a 100-cell panel is **30 ms/col**, and the honest zoom floor is **5.333 ms/col** — because of the
peak pyramid's bin size, which is the surprising part (§2).

1. **The overview is a min/max envelope per column, never samples.** 400 columns over a 3-minute
   clip at 48 kHz is 21,600 samples per column (0.45 s); a 1 Hz waveform is unresolvable. This is
   exactly `audiowaveform`'s model — min/max pairs per N samples, `--pixels-per-second 100` by
   default **[V]** — and this repo already has the primitive: `PeakBuilder::range_minmax(start,
   end)` returns one column's min/max. **No new audio work is needed; the peak pyramid is the
   timeline's data source.**
2. **Horizontal resolution sets what the timeline *is*.** 400 columns over 10 s ≈ 25 ms/column
   (comfortable bar-level editing); over 1 s ≈ 2.5 ms; over 250 ms ≈ 0.6 ms ≈ 30 samples — the
   terminal's *display* floor. **Design consequence: the TUI timeline is an overview + coarse-trim
   surface** (bars, beats, seconds), and sample-accurate work belongs either in a numeric detail
   view with fine nudge keys or in a GUI shell. Claiming otherwise would be the classic TUI
   overreach.

   **Measured, after building it (2026-09-21, `spikes/tui-shell`):** the binding constraint is not
   the terminal — it is the **data**, and it arrives one level earlier than this study assumed.
   `PeakBuilder::range_minmax` walks whole **256-sample base bins**, so the finest honest column is
   one bin: **256 frames = 5.333 ms at 48 kHz**, nine times coarser than what braille can display.
   A narrower column would redraw the same bin and claim resolution the pyramid does not have, so the
   prototype clamps its zoom there and shows `ms/col` in the status line. Going to the display floor
   needs a **raw-sample read path** for the zoomed-in window (exactly how `tui-wave` reaches single
   samples) — a decimated/summed cache over the pyramid for overview plus direct reads for detail.
   That is the next step, and it is a *data* step, not a rendering one.
3. **Amplitude needs ≥4 rows per lane to mean anything.** A 4-row braille lane is 16 vertical steps
   ≈ 4 dB/step across a 70 dB range **[V/I]**; 3-row lanes are for density, 1-row lanes are a tape
   map (boundaries + playhead), not a waveform.
4. **Use the right marker for the job** (see §2's colour asymmetry **[V]**): **braille** for envelope
   lanes (resolution, one colour per cell — which is all a single-colour waveform needs), **block/
   half-block** for clip colour bars, meters and the playhead (colour per sub-pixel). Prefer braille
   or quadrant as the default: **sextant/octant come from the 2021 Legacy Computing Supplement and
   are not reliably in fonts** — a capability switch, not a hard dependency.
5. **Layout budget at 200×50 [I]:** 2–3 rows of chrome (transport, ruler, status), then either
   **8 lanes × 5 rows** (4-row envelope + header), or **12–15 lanes × 3 rows** for a denser
   arrangement, or **20–30 single-row tape lanes** as a coarse map. Realistic visible counts:
   **10–30 clips, 8–15 tracks** — more needs scroll/zoom, which is what real DAWs do too. Clip
   labels cap at ~8 characters in a 9-column clip.

   **Measured, after building it:** rows are shared as `min(4, rows ÷ tracks)` per track, so a
   100×26 terminal gives 3 tracks **2 rows each** (8 braille sub-rows of amplitude — enough to read
   a fade and a clip's dynamics) with the gutter taking 7 columns. A 9-second, 5-clip, 3-track
   arrangement fitted the width at **98.917 ms/cell** (≈10 cells per second), playhead and ruler
   legible, every clip distinguishable by colour and its `▏`/`▕` edges — so the budget above holds
   in practice at the small end of the terminal range.
6. **Images are a detail-pane luxury, never the timeline [V]** — and that is a *preference*, not a
   ceiling. `tui-wave` ships exactly the two-tier design worth copying: zoom to single samples, with
   Kitty/Ghostty graphics where available and **braille approximation everywhere else** **[V]**; and
   Prism draws spectrum/waterfall/oscilloscope/spectrogram/**waveform** at **60 FPS in a plain
   terminal** **[V]**. The protocol caveats still decide *whether* to add the image tier: Kitty is
   stateful (transmit once, re-place by id, negative z-index draws *under* text so text layers on
   top); sixel is immediate-mode and **text drawn over it erases it**; **tmux has no kitty-graphics
   support at all**, and sixel under tmux needs a `--enable-sixel` build plus `allow-passthrough`
   (3.3+); Alacritty and Konsole are unusable for images. If adopted: `ratatui-image` **11.1.0**
   (MSRV 1.86, `ratatui ^0.30.1` + `crossterm 0.29` — exactly our stack **[V]**), built with
   `--no-default-features --features image-defaults,crossterm` so it does not link `libchafa.so` at
   runtime. **Never a hard requirement — the braille tier is the product.**
7. **The fallback is the product, not a stub.** Numeric **clip list** (duration, peak/RMS, gain —
   keyboard-driven) + **coarse tape map** (one row per clip/track, braille envelope, boundaries,
   playhead) + **per-clip detail view** (zoomable braille/quadrant envelope, numeric dB readout) +
   optionally a text spectrum (**`sgram-tui`** is the reference: calibrated dBFS at 2×2 quadrant
   sub-pixels per cell **[V]**). All of it renders on any block-capable terminal — which is the
   whole point of the TUI option.
8. **The unclaimed ground cuts both ways [V].** No terminal program draws a waveform in a
   **multi-track arrangement**: the field splits cleanly into single-file sample editors (`tui-wave`,
   and destructively `audioedit`), live meters/scopes with no file at all (cava, Open Cubic Player,
   ncmpcpp, Prism, scope-tui, sgram-tui), and **MIDI-only** DAWs (Phosphor, imbolc) — no timeline,
   therefore no arrangement view anywhere. So there is no proven UX to copy: freedom, and risk. The
   nearest ancestors are `audiowaveform` (the data model), `cava` (sub-cell bars at 60 fps, the
   8-step U+2581–2587 ladder), `sgram-tui` and **`waveformchart`** (braille ratatui widgets — a
   *pattern to fork*, not a dependency: it pins `ratatui 0.29` while this repo is on 0.30.2 **[V]**),
   and the tracker tradition (the pattern grid as an editing surface).
9. **The moat is the model, not the rendering [V/I].** The closest projects are agent-built or
   LLM-assisted — `tui-wave` says so outright, Phosphor was LLM-assisted, `audioedit` carries Claude
   co-author trailers on every commit — so the drawing, the file I/O and the ffmpeg plumbing are now
   cheap for anyone. What none of them has, and what this repo already has in its engine, is the
   **non-destructive clip model over a replayable log**: clip = source region + timeline position,
   edits as logged commands, arrangement-level undo. That is the thing to defend, and the thing the
   shell exists to expose.
10. **Be explicit about where a TUI loses** — sample-accurate scrub and trim, zoom past ~1 ms/column,
    20+ tracks with vertical detail, drag-and-drop, per-pixel spectral colour. It wins on keyboard
    speed, scriptability, remote/low-bandwidth work, and living in the same shell as the engine. The
    evaluation note's criteria should be judged against exactly this list.

---

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21. The Helix reading and the prior-art
survey were done by model subagents (same harness), from primary sources — repo sources, docs, the
crates.io API. The `ratatui` marker/grid/colour facts in §2 were read from the crate sources vendored
in `~/.cargo/registry` (the versions this repo resolves), the `media` facts from this repository's
code, and the two repos in §3.1, `tui-wave` (§3.2) and `waveformchart` were re-verified first-hand
after the survey. Coverage limits: GitHub's REST API rate-limited the survey, so a few star counts in
§3.3 are unconfirmed, and §3 is a survey of what is *findable*, not a proof of absence.
