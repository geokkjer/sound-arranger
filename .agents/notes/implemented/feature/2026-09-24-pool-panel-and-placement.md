# Agent Note: the pool panel — browse and place (alpha slice D3a)

Status: implemented

## Problem

The pool was reachable only from the outside: `--wave <file>` imported one file on startup and
`pool <dir>` pointed the session at material, and after that the sources were invisible. A user
could not see what the session had, could not choose a clip, and had to *know* a source id and
type an `arrange add_clip` line to place anything. The plan called this exactly right: "also
where the pool-before-arrange requirement becomes an affordance instead of an error string" —
until now, pressing a timeline key with no arrangement produced a message, and the material the
arrangement is made of had no home on screen.

## Decision

**The pool is a panel, and placing a source is one key.**

- `Panel::Pool` joins the focus ring, and the ring is built from the panels that **exist**: the
  mixer always, the timeline when an arrangement is loaded, the pool when a pool has sources. `Tab`
  cycles it, `j`/`k` walk the rows, and the panel's border lights when it owns the keys (the same
  convention as the mixer and timeline).
- The panel lists every source with the facts a placement decision needs: id, length in frames and
  seconds, sample rate, channel count — and marks a crashed take (not finalized) or a missing
  `.peaks`. Sources that cannot be read are not rows; `Pool::list` reports them and the state line
  already says so.
- It draws as a strip **under the timeline** (and under the meters when there is no arrangement):
  the pool is the material *from which* the arrangement is made, so it belongs beside it rather
  than behind a mode. The strip's height follows the source count, bounded, so the timeline keeps
  its space.
- `App.pool` keeps the session's `PoolSource` listing, adopted **before** the arrangement value is
  read (so an arrangement error cannot leave the panel — or the paste check — pointing at the
  previous session's material), with the selection clamped when the pool shrinks. It is also what
  the clipboard's paste checks its sources against (`pool_ids` folded into it — one list, one
  truth), which is why the field replaced the id-only vector.
- `Enter` (`Action::PoolPlace`) places the selected source on the active track at the playhead: a
  logged `add_clip` with the source's full length, the clip id minted as `pool.{n}`, and the **click
  guard** on both new boundaries. It is one command — one undo step — through the host's own parser,
  like every other key. The minted counter is **monotonic** (seeded above the arrangement, never
  decreasing): an undone `pool.1` is out of the arrangement but still in the journal, and re-minting
  it would make journal recovery drop the second `add_clip`, so the re-placed clip would silently
  vanish on a crash. The panel also refuses two rows with the reason rather than minting a command
  that cannot work: an id the `host v1` format cannot carry (a space or a `#` — a hand-filled pool
  can hold `my jam.wav`) and a source with no frames. Both are marked in the row.
- **With no tracks at all**, the same key makes the first track and places the clip in **one
  gesture** (`add_track` + `add_clip` in a `group`): the pool is where material comes from, so
  placing it is how a piece starts. The focus then follows to the timeline, because that is where
  the result is.

## Alternatives considered

- **A full-screen pool mode** (a "browser" the user enters and leaves). Rejected for the alpha: the
  panel's job is to be *visible while arranging* — a source list you can see under the timeline is
  what makes placing a clip an alternative to typing an id. A browser with filters, previews and
  folders is a later surface.
- **A modal overlay on the timeline** (like the help). Rejected: the help is reference material you
  read and close; the pool is working material you place *from*, and an overlay hides the very
  arrangement you are placing into.
- **Placing with the existing `p` (paste) key when the pool is focused.** Rejected: `p` is the
  clipboard's paste, and giving one key two actions based on focus is the drift the workflow crate
  exists to prevent. `Enter` is "choose this row", which is what a list wants, and it was unbound.
- **Importing through the panel** (a file picker, or `--wave` inside the TUI). Deferred: importing
  is a host-side boundary (`Pool::import`) with its own command-line spelling, and the panel is a
  *view* of what the session already has. The next sub-step (audition) is closer to the panel's
  purpose.
- **Playing the source as the placement's preview** (audition now). Deferred to its own slice
  (D3b) because it is a *host* change, not a panel change: the existing `play` command is logged
  **state** (it is replayed on seek), so an audition needs an unlogged action path — a decision
  worth its own note rather than a flag on this one.
- **Clamping the placement frame to the arrangement** (like a seek). Rejected: placing past the end
  is how a piece grows, exactly as `p`/`P` already do it.
- **A pool row per channel of a stereo source.** Already the truth: a split import writes one
  source per channel, so the panel lists them separately and each can be placed on its own track.

## Consequences

- "Load clips from the pool" is a key: `Tab`, `j`/`k`, `Enter`. The session's material is visible
  while arranging, with the length and rate a placement decision needs, and a crashed take is
  marked rather than silently listed.
- The pool can be the *start* of a session: a `pool <dir>` script with no arrangement plus `Enter`
  produces the first track and the first clip in one gesture.
- Tests: the panel places a source at the playhead with the click guard and the source id, one undo
  removes it while the pool row survives, and the panel draws the source it lists; a pool-only
  session gets its first track from the same key and the focus follows; `Tab` reaches the pool and
  `Shift-Tab` comes back through it.
- The clipboard's source check now reads the same listing the panel shows, so a paste cannot name a
  source the panel would not be able to show.
- The row list is handed to ratatui **whole**, so the widget scrolls the selected row into view;
  the panel title says `· j/k scrolls` when there are more sources than rows (truncating the items
  in the shell would have defeated that scrolling — the gate caught the first draft doing exactly
  that while its own comment claimed otherwise). A focus whose panel disappears (a load with no
  pool) falls back to one that exists instead of leaving the keys dead.
- **Audition is cut from the alpha, with the reason recorded** (the plan's item 9): the mixer's channel
  inputs are single-connection, so a preview needs a monitor path of its own (a mixing decision), and
  the existing `play` command is logged *state*, so an audition needs an unlogged action (a second
  decision) — neither is in the owner's ask list, where "load clips from the pool" is.
- **Still open**: importing from the panel
  (a picker, or the `--wave` path surfaced as a command); a filter/search for a large pool; mouse
  selection in the pool (the panel records its rect but only the keys act on it); a preview/audition
  (see above). *Closed 2026-09-24:* `Pool::import` no longer accepts a whitespace-bearing stem — the
  pool's own id rule now refuses whitespace (an id the `host v1` log cannot name back is not an id)
  and the importer **sanitizes** the stem instead of failing, so `My Take.wav` imports as `My_Take`
  ([media-pool note](2026-08-24-p1-3-3-media-pool.md)). A file that was *already* in the pool under
  such a stem is still listed and still unaddressable; the panel marks it.

## The gate's findings

The slice's gate returned `merge with changes`
([review + disposition](research/architecture/2026-09-24-alpha-slice-gate-d3a-glm-standin.md)):

1. **The pool was adopted *after* the arrangement branch** — an arrangement error returned early,
   leaving the panel rows and the paste check on the previous session. Fixed by adopting the pool
   first.
2. **The pool's minted id could repeat after an undo** (the journal is append-only, so the id is
   still there, and a crash-recovery replay would drop the second `add_clip` — the clip silently
   vanishing). Fixed with a monotonic counter, mirroring the clipboard's `next_paste`.
3. **A source id with a space or `#` could not be placed** (the panel listed it as an ordinary row
   and the key produced a parse error). Fixed: the row is marked unplaceable and the key refuses it
   by name; the paste path got the same guard.
4. A zero-frame source is refused with the reason, and marked in its row. (The value op already
   refused it; the panel now says why before the key is pressed.)
5. A focus on a panel that no longer exists falls back instead of leaving the keys dead.
6. The first draft truncated the rows in the shell, which defeated ratatui's scroll-the-selection-
   into-view — the opposite of what the note claimed. Fixed by handing the list over whole, with the
   overflow hint in the title.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
