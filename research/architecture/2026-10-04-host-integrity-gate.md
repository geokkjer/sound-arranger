# Merge gate — PR #12, `fix/host-integrity` (GLM-5.3, full)

> Research input, 2026-10-04. Gate: **GLM-5.3** (full, `zai` route) via the `pi` harness, read-only,
> against `fix/host-integrity` at `1dbda91`, base `main` = `a075726`. The author is DeepSeek-V4.1-Flash,
> so the cross-vendor independence rule holds. The gate re-ran the local gates on the tip (fmt, clippy,
> the notes verifier, host 88 passed + 1 ignored) and transplanted both new tests into a `git archive`
> copy of the parent, where both **fail**.
>
> **Verdict: `merge`.** Both fixes are exact against the recorded findings, each with a discriminating
> test. Three things are recorded rather than fixed: the two residual cache blind spots, the third
> `stop_recording` path that still drops the declaration, and the test coverage gaps. The gate could
> not read CI from its sandbox; the merger read the checks — all four green on the PR.

## Disposition

| Gate finding | Verdict | Disposition |
|---|---|---|
| §1 The id derivation from `errors` is byte-identical to the readable branch's, every `errors` entry is a top-level `.wav`, `conform`'s errors are a different struct, and no consumer reads `pool_ids` order | VERIFIED | No change. |
| §1 Residual: a `read_dir` **entry-level** error is swallowed by `e.ok()` (`pool.rs:230`) — file present, cache blind; and an uppercase `.WAV` is invisible to both `list()` and the exact-case free-id check, so they agree on Linux | CONFIRMED, pre-existing, marginal | **Recorded for the next slice.** Neither is introduced or worsened here; both need a `media` change (surface entry errors; decide the name-matching case rule). |
| §2 `last_take` is this take whenever `was_recording`; a stray stop cannot re-declare; commit-before-`Err` is safe and the actor still publishes | VERIFIED | No change. |
| §3 **Load and seek/undo/redo report a stopped take without declaring it:** `live.rs:464-470` sets only the in-memory report on the fresh session, and `replay_to_kind`'s `let _ = self.stop_recording()` (`lib.rs:3005-3010`) commits nothing — WAV in the pool, no `take` line, lost on reload, complaint or not | CONFIRMED, **pre-existing**, outside this slice's findings | **Deferred to the next slice.** It is a design decision, not a one-liner: a seek must not mutate the history, so making the declaration survive a rebuild means deciding where a seek's take declaration belongs. The gate also found two stale claims that would imply it is already covered — corrected below. |
| §3 Stale claim A: the PR #11 gate's "a rebuild that stops an in-progress take commits the `Take` at history end" is **false** at the commit it reviewed | CONFIRMED | **Corrected** in [that archive's disposition](2026-10-04-host-pool-ids-gate.md) rather than in its verbatim body. |
| §3 Stale claim B: `undo`'s comment at `lib.rs:3255-3257` — "a take finalized on the way into the rebuild clears the branch for itself (`commit_state`)" — is false today and was false when written | CONFIRMED | **Left as written, recorded here.** It states the *intended* design; the next slice either implements it (making the comment true) or corrects it. Rewriting it now would be a second rewrite when the path is fixed. |
| §4 Both new tests are discriminating; the take test also pins that the complaint still surfaces | VERIFIED | No change. |
| §4 Coverage gaps: the `was_recording = false` guard is unpinned (deleting it passes both tests); the post-take refresh path joining `errors` is unpinned (the pool test exercises only the bind listing); the Load/rebuild declaring is unpinned | CONFIRMED | **Deferred with §3.** The first two are cheap; they ride the next slice so the fix and its full contract land together. |
| Process | — | One commit on the `main` tip, notes updated in-commit in present tense, no note rewritten, attribution correct, branch fits `fix/<topic>`. CI (which the gate could not read) was green on the PR before merge. |

## The review, verbatim

# Merge gate review — PR #12, `fix/host-integrity` (1 commit, `1dbda91`, base `a075726` = `main` tip, verified)

Local gates re-run on the tip: fmt OK, clippy `-D warnings` OK, notes verifier OK, host 88 pass + 1 ignored. Both new tests transplanted into a `git archive` copy of the parent **fail on `a075726`** exactly as the commit message claims.

## 1. The unreadable-name fold-in — VERIFIED correct, exact, no consumer breaks

- **Every `errors` entry is a top-level `.wav`.** `Pool::list` builds `wavs` from a non-recursive `read_dir` filtered on extension exactly `wav` (`crates/media/src/pool.rs:228-237`); errors are a subset of that (`pool.rs:237`). The fold-in's derivation — `file_name → to_str → strip_suffix(".wav")` (`crates/host/src/lib.rs:836-841`) — is byte-identical to the readable branch's id derivation (`pool.rs:243-248`). No `errors` path can be a subdirectory, a non-`.wav`, or a non-top-level file.
- **`conform`/`expand_one` errors are a different struct** (`ConformReport.errors`, `pool.rs:562, 570-573`); the cache reads only `pool.list()`. Conform's `.pre*`/`.converting` files are non-`.wav` by design (`pool.rs:549-554`) and don't occupy a take name under the free-id check (`<id>.ch0.wav`.is_file, `lib.rs:997-1003`) — excluding them is correct.
- **Occupancy coverage is complete for what `list()` sees:** every top-level `.wav` lands in `sources` ∪ `errors`, and the free-id check is the occupancy oracle — so the cache now agrees with the refusal everywhere `list()` can enumerate. Residual (pre-existing, marginal): a `read_dir` *entry*-level error is swallowed by `e.ok()` (`pool.rs:230`) — file present, cache blind; and an uppercase `.WAV` is invisible to both `list()` and the exact-case free-id check, so they agree on Linux. Neither is introduced or worsened here.
- **Consumers are order-insensitive.** `pool_ids` reaches only the live snapshot copy (`crates/host/src/live.rs:707-710`) and the iced spike, which uses `contains` and a max-number scan (`spikes/iced-shell/src/main.rs:936-946, 1134-1139`). The re-sort (path order → id sort) reorders e.g. `[jam.ch0, jam]` → `[jam, jam.ch0]`, but nothing reads order. Duplicates are impossible within one listing (sources/errors disjoint, dir names unique); `dedup()` is belt.

## 2. The complained-take commit — VERIFIED sound

- **`last_take` is this take whenever `was_recording`.** `start_recording` clears it (`lib.rs:1017`); the only mid-recording writer is a hand-sent `Take` via `status_take` (`lib.rs:1112-1115, 1988`), and `stop_recording` unconditionally overwrites `last_take` with the stopped take's report *before* propagating any complaint (`lib.rs:1083, 1087-1095`) — and `refresh_pool_ids` inside doesn't touch it. The commit at `lib.rs:1943-1956` therefore always names the take being stopped.
- **A stray stop cannot re-declare.** With nothing recording, `stop_recording` early-returns at `lib.rs:1058-1061` without touching `last_take`, and `was_recording=false` skips the commit; `stopped?` still surfaces "no take is recording". A stale `last_take` is pre-existing behavior, cleared by the next `start_recording`.
- **Commit-before-`Err` is safe.** `commit_state` (`lib.rs:3309-3316`) journals and histories the `Take`; `RecordStop` is not `is_state` (`lib.rs:440-459`) so `process` cannot double-commit. The actor sends the `Err` and still publishes (`live.rs:436-442`); the iced shell returns early on refusal (`iced-shell:884-887`) but the next publish carries `last_take` (`live.rs:647`). No path rolls back a pushed history entry.

## 3. The other `stop_recording` callers — CONFIRMED gap, pre-existing, outside both gates' recorded findings

This slice fixes one of three production stop paths. Quoting the other two:

- **Load** (`crates/host/src/live.rs:464-470`): `let _ = session.stop_recording(); if let Some(take) = session.last_take().cloned() { fresh.status_take(&take); }` — `status_take` sets the in-memory report only; no `HostCommand::Take` enters the fresh session's history, so the document never declares the take.
- **Seek/undo/redo** (`crates/host/src/lib.rs:3005-3010`): `if self.recording.is_some() { let _ = self.stop_recording(); }` — no commit; the rebuild replays a history lacking the declaration. (`replay_full`, `lib.rs:3140-3143`, is test-only, same shape.)

Same symptom the commit message indicts — WAV in the pool, no `take` line, lost on reload — complaint or not. **Not a regression**: the PR #9 gate's recorded finding (§5b) was specifically the `RecordStop` `?`, and this slice's message and notes scope themselves to it. But two stale claims could mislead a future reader into thinking the rebuild path is covered: the PR #11 gate's "a rebuild that stops an in-progress take commits the `Take` at history end" is false at the commit it reviewed (checked `473d944:lib.rs:2982-2984`), and `undo`'s comment `lib.rs:3254-3257` — "a take finalized on the way into the rebuild clears the branch for itself (`commit_state`)" — is false in the tree today and was false when `0f4bc64` wrote it. **Record as the next slice's finding.**

## 4. The tests — VERIFIED discriminating; three coverage gaps

On parent `a075726` (transplanted test file): the pool test fails with `an unreadable file's name is still held: []`; the take test fails with a three-line header-only script. On the PR both pass. What they do **not** cover:

1. The `was_recording=false` guard is unpinned — deleting the guard (re-declaring `last_take` on a stray stop) passes both tests.
2. The refresh-joins-errors half is unpinned — the pool test exercises only the `set_pool` bind listing, not the post-take `refresh_pool_ids` (`lib.rs:1086`).
3. No test pins the Load/rebuild paths declaring (§3), and nothing pins ordering/dedup (harmless — no order-dependent consumer exists).

The take test also honestly asserts the `Err` still surfaces (`execute(...).is_err()`), pinning the commit-before-complaint ordering.

## Process

One commit on `main` tip; notes updated in-commit, present tense, no note rewritten; attribution trailer correct and matches the declared author family; branch name fits `fix/<topic>`. CI must be read before merging per the standing order.

VERDICT: merge — both fixes are exact against the recorded gate findings, verified end-to-end with discriminating tests and green gates, and the one confirmed gap (Load/seek/undo still drop the declaration) is pre-existing, outside the findings this slice set out to close, and belongs recorded on the next slice.
