# Reviewer gate (stand-in) — alpha slice D3a (pool panel), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (its API window had just reset; the retry is in flight — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md), which now also
> records the owner's note that the CLI's `403` is a fetch-security block on the login URL rather
> than a quota error, so the **API** route is the one that matters).
> Scope: slice D3a (the pool panel — browse and place), working tree at the time —
> `Panel::Pool` and the focus ring, `App.pool`, `Action::PoolPlace`, the panel's draw, and the tests.
> The reviewer was told to attack the claims, cite file:line, separate verified from inferred, and
> end with a verdict.
>
> **Disposition: `merge with changes`; all three should-fix findings were real and are fixed, and
> the three notes are dispositioned.**
>
> 1. **The pool was adopted *after* the arrangement branch** (should-fix) — `take_arrangement`
>    returned early on `outcome.arrangement == Err` *before* refreshing `self.pool`, so an
>    arrangement error left the panel rows and the paste check on the previous session's material
>    (and the note's "refreshed on every adoption" was false on that path). Fixed: the pool listing
>    and the focus fallback are adopted first.
> 2. **`mint_clip_id` could re-mint an id the journal still holds** (should-fix) — place `pool.1`,
>    undo, place again: the id is free in the arrangement but the journal is append-only, so a crash
>    before the next save makes recovery replay both `add_clip` lines, refuse the second, and the
>    re-placed clip silently vanishes (only a `refused` count records it). Fixed with a
>    **monotonic** counter, mirroring the clipboard's `next_paste` (which was engineered against
>    exactly this). Test: place, undo, place → `pool.2`.
> 3. **A source id containing a space or `#` could not be placed** (should-fix) — the id is a file
>    stem and `Pool::import` only rejects path separators, so a hand-filled pool can hold
>    `my jam.wav`; the panel listed it as an ordinary row and `Enter` produced a parse error. Fixed:
>    the row is marked unplaceable, the key refuses it **by name**, and the paste path got the same
>    guard (a clip whose source id came from a hand-written script).
> 4. **A zero-frame source** (note) — the value op already refused it cleanly; the row now says
>    "no frames" and the key explains before the host is asked.
> 5. **A focus on a panel that no longer exists** (note) — a load with no pool left the keys dead on
>    a panel that is not drawn. Fixed: `focus_that_exists` falls back (mixer → timeline → pool).
> 6. **`draw_pool` truncated the rows in the shell** (note) — which defeated ratatui's
>    scroll-the-selection-into-view and contradicted the note's own claim; the dead `rows` binding
>    and a promised "… more" line that was never drawn were the visible symptoms. Fixed: the list is
>    handed over whole and the title says `· j/k scrolls` when rows do not all fit.
>
> Reviewer-verified: the focus ring is built from the panels that exist (with and without an
> arrangement); `j`/`k` clamp both ways and the selection is clamped on shrink; `zoom`/`zero` are
> no-ops on the pool; the first-track gesture is one atomic `group` (validated on a snapshot before
> applying) and its success status is gated on the host's answer, so the D1 bug class is absent;
> placement goes through the host's parser and log (one `add_clip`, `group begin/end` on save) and
> nothing panel-side reaches the log; paste and the panel read the same `self.pool`; a rate mismatch
> cannot bite placement (`set_pool` conforms the pool); the layout degrades safely at tiny
> terminals; and a pool-only session yields an empty arrangement, so the no-track branch is
> reachable exactly as claimed.

---

## Verbatim review

Adversarial review of D3a (pool panel + placement). Verdict: **merge with changes**. All tests pass (45 tui-shell, 250 core workspace, iced-shell builds). Findings:

1. **should-fix — pool refresh skipped when the arrangement value errors.** `take_arrangement` returns early on `outcome.arrangement == Err` (spikes/tui-shell/src/main.rs:415-421) BEFORE `self.pool`/`pool_selected` are refreshed (main.rs:438-439). On a poisoned-timeline error the panel rows and the paste source-check go stale (previous session's sources), and `self.arrangement` stays `Some(stale)` — the "refreshed on every adoption" claim (note line 31) is false on this path. Move the pool adoption above the arrangement branch. (The Err→stale-arrangement lie itself is pre-existing, not introduced here.)

2. **should-fix — `mint_clip_id` re-mints an id still present in the journal.** It seeds only from the live arrangement (main.rs:1166-1180). Sequence: place `pool.1` → undo (entry leaves history but stays in the append-only journal) → place again → mints `pool.1` again (valid now) → crash before the next save: journal recovery replays both `add_clip … pool.1` lines, the second is refused-and-dropped (crates/host/src/lib.rs:844-851) and the re-placed clip silently vanishes (a recovery `refused` count is the only trace). Paste explicitly engineered against this with a monotonic `next_paste` (main.rs:1464); the pool minter reintroduces the hazard. Redo is safe (redo stack clears on a new edit, lib.rs:1715 — verified). Fix: a monotonic `next_pool` like `next_paste`. Saved/reloaded/user-typed `pool.1` collisions are covered (they're in the arrangement when minted).

3. **should-fix (small) — source ids containing whitespace cannot be placed.** Pool id = file stem (crates/media/src/pool.rs:197-202); `place_pool_source` builds an unquoted `add_clip {track} {id} …` line and `parse_arrange_line` splits on whitespace (crates/host/src/lib.rs:2404) — `Enter` on "my jam" always fails with a parse error while the panel lists it as a normal row. Same latent hole affects paste. Either refuse/mark such rows or document the constraint.

4. **note — zero-frame source:** `micro = 0` is fine and `validate_clip` refuses `src_len == 0` (crates/media/src/timeline.rs:216-218) with a clean "arrange refused" status, but the panel gives the row a working-looking Enter. Unfinalized (crashed) takes are placeable by design and marked — fine.

5. **note — focus can be stranded on `Panel::Pool` after the pool empties** (a load with no pool): `cycle_focus` rebuilds the ring but nothing redirects a dead focus (`adopt` only redirects Mixer→Timeline, main.rs:516-518). Keys are dead-but-explained until Tab. Also no test Tabs to the pool in a no-arrangement session (the first-track test sets `focus` directly).

6. **note — draw_pool dead code / stale comment:** `rows` computed then `let _ = rows;`, and the layout comment promises a "…" overflow line that is never drawn — with >5 sources the title count is the only hint the list is longer. Selection-into-view is actually already handled by ratatui's ListState scrolling, so the note's "still open" item (line 83-84) is stale.

## Verified
- Focus ring built from existing panels incl. no-arrangement+pool; `j`/`k` clamp both ways; selection clamped on shrink (main.rs:439); zoom/zero no-ops.
- The group is atomic (validated on a snapshot, one history entry, one undo — host lib.rs:473+, 1602-1615) and refusals keep their status (`command()` main.rs:624-639, success gated on the bool); placement goes through the host parser (one `add_clip` line; group wraps as `group begin/end` on save/journal); nothing panel-side reaches the log.
- Rate mismatch is handled at `set_pool` (conform to session rate, lib.rs:430); paste and the panel read the same `self.pool`; layout degrades safely at tiny terminals (pool Length shrinks, `inner.height == 0` guard); prompt/help modality intercept Enter before placement.
- The all-green suite: 45 tui-shell, 250 core workspace, iced-shell builds.

## Not verified
`spikes/tui-shell/scripts/pty-check.sh` run, mouse behaviour at runtime, iced-shell runtime (build only).
