# External review — `slice/iced-record-mvp` (the iced record MVP)

> Research input, recorded 2026-10-03. **Reviewer: not named in the handoff.** Delivered as feedback
> on the three-commit slice (`095078f`, `46d0675`, `a49cc6a`, +645/−33) after its local gates passed
> (fmt, `clippy -D warnings` on the root and both spike workspaces, 418 workspace tests + 59 spike
> tests). Scope: the slice's two shells, the shared `workflow` verb, the host snapshot extension, the
> Agent Note, and the tests. The supervisor (**DeepSeek-V4.1-Flash**) reproduced every claim against
> the code rather than against the review's prose, and fixed what it confirmed in `73bed84`.
>
> **Disposition: `fix 1–3, defer 4 and the polish`.** Findings #1–#3 were confirmed and fixed before
> merge; #4 and the mechanical nits ride a later slice. Five defects the review missed were also
> found and fixed. One finding (the TOCTOU minor) did not hold as written.

## How the claims were verified

Every claim was read against `a49cc6a`, then against `73bed84` once fixed. The verdicts are the
supervisor's, with `REFUTED` meaning the claim's stated consequence is not reachable in the shipped
code. Two corrections are worth recording because the review is otherwise precise about ordering:

- **`:404` is not a clobber source.** `take_arrangement` writes `journal_error` *first* and the
  recording branch overwrites it, so an autosave failure cannot hide the REC status. The real clobber
  sources are the export (`:437`), the finished take (`:454`), the source-read error (`:502`), and the
  arrangement error (`:474`).
- **The boot-time TOCTOU is not reachable.** `Spike::from_host` ends with `adopt()`, a synchronous
  `host.outcome()` round trip, and the actor publishes before it reads the next request — so the
  `Pool` command's refresh is on the published snapshot before `from_host` returns, and
  `toggle_record` re-reads it anyway. The residual race is the reply-before-publish window *inside*
  `execute`, which is real but far narrower than "the snapshot does not pump".

## The review, verbatim

All gates pass (fmt, clippy -D warnings on root + both spike workspaces, 418 workspace tests + 59 spike tests green). Here's the review of slice/iced-record-mvp (3 commits, +645/−33).

### Verdict

Solid slice: the right verb in the shared workflow, the snapshot extended for polling shells with the caching rationale documented, and a note that reads honestly. But the TUI half reintroduces — in the same slice — the exact stale-state bug the note says was fixed in the iced shell, and the iced shell's toggle has an error-handling asymmetry worth fixing before merge.

### Bugs

1. The TUI decides start-vs-stop from a status string, not from state — spikes/tui-shell/src/main.rs:1362. record_toggle tests self.status.starts_with("● recording"), but that string is written from the one-shot HostOutcome in take_arrangement (:408), which runs on load/command/edit — not per frame. Two failure modes:
   - Any intervening outcome rewrites the status (an export at :437, an arrangement error at :474, a journal failure at :404), so the next `o` offers to start a take while one is running → host refusal.
   - While a take runs, the REC frame counter is frozen at the last command's value — even though the event loop refreshes app.snap every frame (:3199) and this slice added Snapshot::recording for precisely this ("a polling shell needs it every frame", crates/host/src/live.rs:65). The TUI never reads snap.recording/last_take/pool_ids at all.

   The note's consequence section claims the stale-state class was fixed "by reading the host at the moment of the decision" — that's true only of the iced shell. Fix: decide from self.snap.recording (refreshing self.snap = self.host.snapshot() first, as the iced shell now does) and draw the indicator from the snapshot.

2. The iced toggle's stop path swallows a refused stop — spikes/iced-shell/src/main.rs, the if let Some(rec) = self.snap.recording.clone() branch. After command(RecordStop) it unconditionally overwrites the status with finished {take_id} — … built from the stale snapshot's counters. If the stop was refused (take ended between the snapshot read and the command), command() writes command refused: … and this clobbers it — the user is told a take finished that didn't, with the last repaint's frame count. The start path checks !self.status.starts_with("command refused"); the stop path needs the same check.

3. Error detection by status-string sniffing — Spike::command discards host.execute()'s Result even though execute is a synchronous round trip through the actor (crates/host/src/live.rs:293): the authoritative answer is right there. Worse, any earlier refused command leaves command refused: … in the status, so a successful record start reads as a refusal and never sets the ● recording status. Make command() return Result<(), String> and match on it in both paths.

4. refresh_pool_ids erases the cache on error — crates/host/src/lib.rs:801, .unwrap_or_default(). A transient Pool::open/list failure forgets every id the cache knew, so next_take_id returns take-1 and the user eats a refusal for a name that should have been skipped. The host's free-id check (lib.rs:964) is the safety net against overwrite, but the cache should keep its previous value on error, not reset to empty.

### Minor

- set_pool re-opens the pool it already holds in hand (lib.rs:783 vs refresh_pool_ids) — list from the existing pool.
- Replayed Take declarations don't refresh the cache; correct today only because a Pool command always precedes them in history. Implicit invariant — worth a comment at the Take arm or a refresh in status_take.
- TOCTOU: HostHandle::snapshot() clones the last published snapshot, it doesn't pump (live.rs:349). The first o after boot can name from a stale-empty pool_ids → one spurious refusal, then success. Fine for humans; --record-check can flake on it.
- Duplicated comment in from_host ("The pool is named only when it failed" appears twice; the first sits above the pool-creation block where it belongs to nothing); demo_pool_dir() evaluated twice in that block.
- command_name's doc now has two stacked, contradictory paragraphs (crates/workflow/src/lib.rs:313-322): the old "The host v1 operation this action dispatches" vs the new "The arrange-line verb" — RecordToggle dispatches a host op yet returns None. Merge them.
- demo_pool_dir walks up from the cwd looking for crates/; env!("CARGO_MANIFEST_DIR") is deterministic regardless of where the binary is invoked.

### Tests

- The TUI's record_toggle has no test, while the iced toggle got a device-gated one — and the TUI branch is headless-testable (set status, press o, assert the prefill). That's the one place a regression in finding #1 would be caught.
- The host-side cache (set_pool → pool_ids(), finished-take refresh) has no direct test; --record-check prints pool sources before/after but asserts nothing about the after-state.
- The iced tests are genuinely good — the "don't pin take-1" reasoning and the refusal-path-on-CI design are exactly right.

### What's good

The host validating the take id is free (lib.rs:964) makes the shell's naming an optimization rather than the contract — the right layering. The snapshot's three new fields carry their own caching rationale. The placeholder timeline refuses to invent an arrangement and says why. The note documents the two real bugs the build surfaced and the "capture needs the transport running" consequence.

Suggested merge gate: fix #1–#3 (or record the TUI's decision source as a follow-up note — this repo superseds rather than rewrites, so a one-line addendum to the 2026-09-30 note would be the honest minimum), and add the TUI toggle test. #4 and the minors can ride a later slice.

## Disposition

Fixed in `73bed84` (one commit, `fix(record-mvp): the record key decides from the host, not the status line`), except where noted.

| # | Finding | Verdict | Disposition |
|---|---|---|---|
| 1 | TUI decides from the status string; counters frozen | **CONFIRMED** | Fixed: `record_intent()` is a pure function of `snap.recording`, read after `self.snap = self.host.snapshot()`; `recording_line()` draws the live take in the footer. The `:404` example was wrong (see above); the real clobber sources are `:437`, `:454`, `:474`, `:502`. |
| 2 | iced stop path swallows a refusal | **CONFIRMED** | Fixed: the stop matches the `Result` and returns on `Err`, keeping the host's words. The finished report now comes from `host.outcome()` — the stop drains the device, so the pre-stop count was short and disagreed with `take_label`. |
| 3 | Error detection by status sniffing | **CONFIRMED** | Fixed: `command() -> Result<(), String>`; all 12 fire-and-forget sites take an explicit `let _`; both toggle paths match on the `Result`. This is the root cause of #2 as well. |
| 4 | `refresh_pool_ids` erases the cache on error | **CONFIRMED** | **Deferred**, ranking agreed. The host free-id check is the safety net, so this is a follow-up fix slice with the two minor items below it. |
| M1 | `set_pool` re-opens the pool it holds | CONFIRMED | Deferred (same slice as #4). |
| M2 | Replayed `Take` does not refresh the cache | CONFIRMED | Deferred; the invariant is real (a `Take` is only logged after a `Pool`), and the comment or refresh belongs with #4. |
| M3 | TOCTOU on `snapshot()` / stale-empty `pool_ids` | **REFUTED in effect** | Not reachable as written: `adopt()` is a synchronous `outcome()` round trip, and the actor publishes before reading the next request. The narrow reply-before-publish race inside `execute` is real and worth knowing, but it did not matter here. |
| M4 | Duplicated `from_host` comment; `demo_pool_dir()` twice | CONFIRMED | Deferred (mechanical, same follow-up). |
| M5 | `command_name`'s stacked doc paragraphs | **CONFIRMED** | Fixed: merged into one paragraph. |
| M6 | `demo_pool_dir` walks up from the cwd | CONFIRMED | Deferred (mechanical, same follow-up). |
| T1 | No TUI test for the toggle | **CONFIRMED** | Fixed, but the suggested recipe was the wrong shape: "set status, press `o`" passes on the buggy code. The test drives `record_intent` with a status line that *contradicts* the snapshot in both directions, which fails on the old predicate. |
| T2 | No direct test for the host cache | CONFIRMED | Deferred with #4 (the device-gated iced test asserts the take reaches `pool_ids`, but on CI that branch is skipped). |
| T3 | `--record-check` asserts nothing about the after-state | CONFIRMED | Still true; it prints the pool sources after the stop but asserts only that a take finished and captured frames. |
| T4 | The iced tests are good | AGREED, with a caveat | They cannot catch #2/#3: the test starts from a fresh status and never exercises a lingering refusal. A `command()` contract test was added (`command_result::the_result_is_the_answer_not_the_status`). |

### Defects the review missed (also fixed)

1. **The slice stranded a doc comment.** `record_toggle` was inserted *between* the `R` doc and
   `rename_track_prompt`, so `rename_track_prompt` lost its doc and `record_toggle` gained the `R`
   text. Restored.
2. **A false claim in shipped source.** iced `take_label`'s doc said "the same split the TUI makes"
   while the TUI read a status string. It is true now, and says so precisely.
3. **The TUI stop path did not do what its comment said.** "stopping needs no name, so it runs
   immediately" opened a prompt and waited for Enter. Stopping is now one key in both shells.
4. **The finished status disagreed with the take line.** The status was built from the pre-stop
   counters; the report now comes from the host, so the two agree.
5. **`lib.rs` line refs were off** by 6–13 lines (`:801` → `:813`, `:783` → `:789`). The TUI, `live.rs`
   and `workflow` refs were exact.

### Left, deliberately

The follow-up slice (`fix/host-pool-ids`) carries #4, M1, M2, M4, M6, and T2: keep the previous
`pool_ids` on a refresh error, list from the pool `set_pool` already holds, and refresh or comment
the `Take` replay arm. The scope is small and separable, and the review itself ranked it below the
merge gate.
