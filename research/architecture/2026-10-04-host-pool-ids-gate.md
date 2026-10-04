# Merge gate — PR #11, `fix/host-pool-ids` (GLM-5.3, full)

> Research input, 2026-10-04. Gate: **GLM-5.3** (full, `zai` route) via the `pi` harness, read-only,
> against `fix/host-pool-ids` at `473d944`, base `main` = `3cc2c71`. The author is DeepSeek-V4.1-Flash,
> so the cross-vendor independence rule holds. The gate re-ran the local gates itself (fmt, clippy, the
> notes verifier, workspace 419/419, iced 17/17, tui 57/57) and confirmed the merge base is the `main`
> tip with exactly one commit.
>
> **Verdict: `merge`.** All five deferred items are implemented correctly. Three findings, none a
> regression: one pre-existing hole that both earlier reviews missed (recorded, deferred), one coverage
> gap in this slice's own new test (**fixed in the same PR**), and the compile-time-path tradeoff
> (accepted). The gate could not read CI from its sandbox (`gh` unauthenticated there); the merger read
> the checks — all four green on the PR.

## Disposition

| Gate finding | Verdict | Disposition |
|---|---|---|
| §1 The cache split — rebind clears and lists from the handle in hand; every replay path rebuilds through `set_pool`; the `Take` invariant holds; a stale cache cannot outlive its pool | VERIFIED | No change. |
| §1 **The per-source hole:** `Pool::list` moves an unreadable WAV into `errors` and skips it, and `refresh_pool_ids_from` maps only `index.sources` — so a corrupt or zero-byte `.wav` drops its id from `pool_ids` while the file still occupies the name, and a shell naming against the list proposes that id and eats the free-id refusal. Directory-level failure is covered; per-source failure is not. Both earlier reviews missed it. | CONFIRMED, **pre-existing** | **Deferred to the next slice.** The fix shape is recorded here: the cache must keep the id of a file that **exists**, whether or not its content can be read — the name is occupied either way (derive it from the directory listing, or fold `index.errors`' file names in). Not introduced by this slice and not a regression, so it does not gate. |
| §2 The new test's **rebind-clears** half was unpinned: carrying the old pool's ids across `set_pool` would have passed it (the test bound one pool) | CONFIRMED | **Fixed in the same PR**: the test now binds a second, empty pool and asserts the previous names are gone, then binds back and asserts they return. |
| §3 `demo_pool_dir`'s compiled-in path dangles if the repo moves without a rebuild | INFERRED, cosmetic | Accepted: the standard determinism trade, and the alternative (cwd walk) broke off-repo launches, which the gate itself verified. |
| §5 The new test fails on the parent commit exactly as claimed (the gate transplanted it into a `git archive` copy of `3cc2c71`); it covers the bind listing, the finished-take join, and keep-on-failure | VERIFIED | No change beyond the §2 addition. |
| §2/§4 `demo_pool_dir` is the same directory the old walk found for `cargo run`, `cargo test`, and off-repo launches; the TUI footer reserves the take row at every size that renders the footer at all, and truncates rather than wraps the take line so the `● REC take-N` prefix survives | VERIFIED | No change. |

**Correction (2026-10-04, from the [PR #12 gate](2026-10-04-host-integrity-gate.md)).** §1 below says *"A rebuild that stops an in-progress take (`lib.rs:2985-2988`) commits the `Take` at history end, then replays it under the same `Pool` — the listing sees the files."* The **listing** half is right, but the **declaration** half is false at `473d944`: the rebuild path (`lib.rs:3005-3010`) stops the take and carries only `last_take` through `carry_over`; it commits no `HostCommand::Take`. The cache conclusion the review drew from it still holds; the claim that a rebuild declares the take does not. The gap is recorded as the next slice's finding in the PR #12 archive.

## The review, verbatim

All gates re-run locally: fmt OK, notes verifier OK, `clippy -D warnings` OK on root + both spike workspaces, workspace 419/419, iced 17/17, tui 57/57. Base confirmed: merge-base = `3cc2c71` = `main` tip, one commit.

# Merge gate — PR #11, `fix/host-pool-ids` (GLM-5.3)

## 1. Host cache split — VERIFIED correct, with one pre-existing hole neither review caught

- **Rebind clears and lists from the handle in hand.** `set_pool` (`crates/host/src/lib.rs:783-806`): `Pool::open` and `conform` run under `?` *before* any mutation, so a refused rebind leaves the old pool bound with its own cache intact — no half-state. On success it clears then lists from the `pool` it already opened. Correct.
- **Every replay path rebuilds the cache through `set_pool`.** `rebuild` (`lib.rs:2882-2952`) and `from_script` (`lib.rs:2749-2761`) replay via `process` with `validating=false` (`new_at_with` default, `lib.rs:769`), so `HostCommand::Pool` takes the `set_pool` branch (`lib.rs:2053-2058`); the rebuilt session replaces the live one (`*self = rebuilt`, `lib.rs:3059`), cache included. `save`'s rebind also goes through `set_pool` (`lib.rs:1325`). A rebuild that stops an in-progress take (`lib.rs:2985-2988`) commits the `Take` at history end, then replays it under the same `Pool` — the listing sees the files. `note_pool` (`lib.rs:847-862`) leaves the cache alone, but is reachable only on a fresh validating session where it is empty.
- **The `Take`-arm invariant holds** (`lib.rs:1936-1942`). `Take` enters history only via `RecordStop` (`lib.rs:1925-1937`), which requires a recording, which requires `pool_dir` (`lib.rs:971-975`); `commit_state` appends at the end, so a `Take` always follows the last `Pool` in history order; undo/redo move only `Arrange` entries (`lib.rs:3206-3211, 3240-3243`), so no later path can reorder them.
- **A stale cache cannot outlive its pool:** `pool_dir` changes only through `set_pool` (clear+list) or the validating-only `note_pool`.
- **Found (pre-existing, minor — missed by both reviews and not fixed by this slice):** `Pool::list` moves a WAV it cannot read into `errors` and skips it (`crates/media/src/pool.rs:240-245`), and `refresh_pool_ids_from` maps only `index.sources` (`lib.rs:828-831`). One corrupt or zero-byte `.wav` in the pool drops its id from `pool_ids` while the file still occupies the name: `next_take_id` proposes the id, the host's free-id check (`existing.is_file()`, `lib.rs:983-990`) refuses — the exact "eat a refusal for a name the pool already has" symptom the commit message says this slice removes. Directory-level failure is covered; per-source failure is not. Not introduced here, recoverable, honest error — record, don't gate.

## 2. `demo_pool_dir` — VERIFIED equivalent, strictly better off-repo

`CARGO_MANIFEST_DIR` is `spikes/iced-shell` (its own workspace, `spikes/iced-shell/Cargo.toml:11`), so `../../target/iced-pool` is the repo-root target — the same directory the `crates/`-marker walk found for `cargo run` from the root, from the spike dir, and for `cargo test` (walk starts at the invocation cwd, which cargo keeps; all of these sit under the root, which holds `crates/`). Invoked outside the repo the old walk terminated at `/` and produced `/target/iced-pool` (typically an unwritable create); the new path is always the workspace pool. `from_host` now resolves once and binds the same `PathBuf` it created (`spikes/iced-shell/src/main.rs:299-308`) — VERIFIED. Compile-time-path caveat: if the repo is moved without a rebuild the path dangles; standard trade for determinism, not a defect.

## 3. `record_check` after-state — VERIFIED, no false-fail found

`command()` is a synchronous actor round trip (`iced-shell:990-996`); the stop's refresh runs inside `stop_recording` before the reply (`lib.rs:1069-1070`); the harness refreshes `spike.snap = spike.host.snapshot()` after a 400 ms settle (`iced-shell:1112-1113`). The only failure path is sources that genuinely did not land (pool dir removed/readonly mid-take → refresh keeps old ids → `missing` non-empty → exit 1) — which is precisely the state the instrument exists to catch. `t.frames == 0` is checked first, so an empty take reports the right failure.

## 4. TUI footer — VERIFIED at every size

`Layout::vertical([Min(0), Length(u16::from(take.is_some()))])` over the block's inner (`tui-shell:3164-3180`): with a take running and inner ≥ 1 row, the `Length(1)` row is reserved — only the message area can be squeezed to 0, which is the comment's stated priority. `draw()` supplies inner 3 in both branches while recording: arrangement branch fixed `Length(5)` (`tui-shell:2671-2675`), idle branch grows 4→5 (`tui-shell:2701-2712`). At degenerate sizes where the whole footer is squeezed below 3 rows nothing renders — identical to the old code. The take paragraph has no `.wrap`, so at narrow widths it truncates rather than wraps; the `● REC take-N` prefix survives — a better failure mode than the old clip-out-of-existence. The pinned test renders through the real `draw()` (`tui-shell:3706-3707`) and passes.

## 5. The new test — VERIFIED discriminating; one coverage gap

Run against the parent `3cc2c71` (a `git archive` copy in `/tmp`, only the test file transplanted): **FAILED** at `tests.rs:4835` — "a failed refresh keeps the ids it had" — exactly the assertion targeting the parent's `unwrap_or_default` (`lib.rs@3cc2c71:806-813`). It covers what its doc claims (bind lists, finished take joins, failed refresh keeps). Gap, minor: the **rebind-clears** half of the split is not pinned — a regression to carrying the old pool's ids across `set_pool` would pass this test (the test binds one pool only).

## Process

Note updated in the same commit, present tense, no note rewritten (`2026-09-30-iced-record-mvp.md` diff only adds). Attribution correct (`Assisted-by: DeepSeek-V4.1-Flash · DeepSeek Harness`, author identity per convention). Branch name fits `fix/<topic>`. CI status could not be read from this sandbox (gh unauthenticated) — local equivalents of all four checks plus both spike jobs are green; **the merger must read the PR checks before merging** per the standing order.

## Findings summary

| # | Finding | Verdict | Severity |
|---|---|---|---|
| 1 | `Pool::list` per-source `errors` drops ids of files that exist → the refusal-eating symptom survives through a corrupt WAV (`pool.rs:240-245`, `lib.rs:828-831`) | CONFIRMED, pre-existing, both reviews missed | Minor, follow-up |
| 2 | Rebind-clears not pinned by the new test | CONFIRMED | Minor |
| 3 | `demo_pool_dir` dangles if the repo moves without rebuild | INFERRED, standard compile-time-path trade | Cosmetic |

All five deferred items (#4, M1, M2, M4, M6, T2, T3, §4b) are implemented as specified and verified correct; the findings above are pre-existing or coverage gaps, none a regression.

VERDICT: merge — every deferred item is implemented correctly and verified against the code, the new test fails on the parent exactly as claimed, and the two findings are pre-existing minor gaps that belong on the next slice, not this one.
