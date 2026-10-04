# Merge gate — PR #9, `slice/iced-record-mvp` (GLM-5.3, full)

> Research input, 2026-10-03. Gate: **GLM-5.3** (full, `zai` route) via the `pi` harness, read-only,
> against `slice/iced-record-mvp` at `44c1f1c`, base `aec0498`. The author is DeepSeek-V4.1-Flash, so
> the cross-vendor independence rule holds. The gate re-ran the local gates on the tip itself (fmt,
> clippy, the notes verifier, tui-shell 57/57, iced-shell 17/17) and took the device branch of the
> device-gated toggle test.
>
> **Verdict: `merge with changes`.** One blocker, and it was real: `095078f` silently deleted
> `--sweep`'s varied-delay idle-starve probe, which `aec0498` — the commit directly beneath this
> slice — added as the regression guard, and whose note still credits it. Restored in the same PR,
> which is the stronger remedy than amending the note: the guard comes back and the note stays true.
> Everything else the gate checked was VERIFIED correct. This is the **sixth** defect on this slice
> that the first review and the supervisor's pass both missed.
>
> The review below is verbatim from the gate's stdout; only the harness's npm startup noise is
> elided.

## Disposition

| Gate finding | Verdict | Disposition |
|---|---|---|
| §1 `command()` / refusal paths in both shells are correct; no decision-path status sniffing remains | VERIFIED | No change. |
| §2 Rapid double `o`: reply-before-publish window is real but microseconds against a ≥30 ms key repeat; a hit is honestly refused | Mechanism VERIFIED, reachability INFERRED-unreachable | No change; the first archive's "far narrower" assessment stands. |
| §3 `record_check` decides success by `status.contains("recording")` — the refusal text contains that substring | CONFIRMED, not reachable from a fresh spike | **Fixed**: the harness now matches the `● recording` marker, and the device-gated test's assertion was tightened the same way. |
| §4 TUI decides from the snapshot; both footer layout branches render the take line; removing the recording write from `take_arrangement` lost nothing | VERIFIED | No change. |
| §4b `draw_foot` wraps, so a status longer than the terminal clips the take line out of the 3-row inner height | CONFIRMED, cosmetic; state not lost | **Deferred** to the follow-up slice with #4 (it needs a layout split, not a one-liner). |
| §5 The `outcome()` read after `RecordStop` is sound: the actor is sequential, so the report and the published snapshot agree; the pre-stop count is genuinely short by the drain | VERIFIED | No change. |
| §5b A finalizing complaint skips `commit_state`, so a take that landed with a complaint never enters the history | CONFIRMED, pre-existing host behaviour | Not this slice's; recorded here. |
| §6 The three new unit tests discriminate (they fail to compile on `a49cc6a`; transplanted, the old predicate fails both new cases); the device-gated toggle test does not discriminate | VERIFIED | No change; already conceded as T4 in [the first archive](2026-10-03-iced-record-mvp-review.md). |
| §6b Still untested: the TUI `record_toggle` *wiring* (only the pure function is pinned), the host cache directly, `--record-check`'s after-state, and the `outcome()` error branch | CONFIRMED | The first three ride the follow-up slice; the `Err` branch is a two-line defensive path with no reachable trigger. |
| §7 `a49cc6a`'s commit message claims the device-gated test "records a real take on this machine" | CONFIRMED | The test never captures a frame — every `target/iced-pool` WAV is 44 bytes, header-only, verified. The note itself does not overclaim (its 57,856-frame claim belongs to the owner-run `--record-check`). History: recorded here, not rewritten. |
| §8 The sixth defect: `--sweep`'s varied-delay idle-starve probe was deleted silently | CONFIRMED | **Fixed**: the probe is restored verbatim, and [the owning note](../../.agents/notes/implemented/bug-fix/2026-09-30-an-idle-host-fed-the-device-silence.md) now names both probes explicitly so a second silent removal is a contradiction, not an oversight. |
| §9 Deferred items (#4 and the mechanical nits) — none blocks this merge; #4 should be next | AGREED | Deferred as planned. |

## The review, verbatim

# Merge gate review — PR #9, `slice/iced-record-mvp` (5 commits, base `aec0498`)

Gates re-run on the tip: `cargo fmt --all --check` OK, `clippy -D warnings` (host, workflow) OK, notes verifier OK, tui-shell 57/57, iced-shell 17/17 — and the device-gated toggle test takes the **device branch** on this machine (it wrote `take-4`/`take-5` into `target/iced-pool` during my runs).

## 1. The iced `command()` / refusal path — VERIFIED correct

`command()` returns the host's `Result` and writes the refusal only on `Err` (`spikes/iced-shell/src/main.rs:1000-1010`); the stop path returns before touching status on `Err` (`:905-911`), the start path writes `● recording` only on `Ok` (`:938-953`). No decision-path status sniffing remains in either shell — every surviving `status.contains/starts_with` is in `#[cfg(test)]` or the `--record-check` harness.

**Rapid double `o`:** the reply-before-publish window is real — the actor sends the reply at `crates/host/src/live.rs:441` and publishes at `:442`, while `snapshot()` (`live.rs:349-351`) clones without pumping. But the window is one mutex lock plus field writes (microseconds) against a human key repeat ≥30 ms; if somehow hit, the host refuses "a take is already recording" (`crates/host/src/lib.rs:1002-1003`) — honest, no data loss. Mechanism VERIFIED, reachability INFERRED-unreachable. The archive's "far narrower" assessment holds.

**Irony worth recording:** `record_check` itself decides success by `spike.status.contains("recording")` (`iced-shell:1099`) — and the host's refusal "a take is **already recording** — run `record stop` first" contains that substring. Not reachable from a fresh spike, but it is exactly the smell this slice removed from the product.

## 2. TUI — VERIFIED the snapshot is the only source

`record_toggle` refreshes `self.snap = self.host.snapshot()` (`tui-shell:1371-1372`) and matches `record_intent()`, a pure function of `snap.recording` (`:1392-1398`); the prompt is modal (`:800-824`), so `o` types while it is open. Both layout branches render the take line: the else branch grows 4→5 on demand (`:2703-2712`), the arrangement branch is fixed `Length(5)` (`:2671`) — pre-existing on main, costs one spare row idle. Removing the recording write from `take_arrangement` lost nothing: the `Record` command's own publish (`live.rs:442`) puts `recording` on the snapshot even with the transport stopped, and the hint moved into the footer line itself (`:3228-3236`). One minor wrinkle both passes missed: `draw_foot` wraps (`:3171`), so a status longer than the terminal width wraps to two rows and clips the take line out of the 3-row inner height — cosmetic, state not lost.

## 3. The `outcome()` read after stop — VERIFIED sound

The actor is sequential: the `RecordStop` iteration completes its publish (`live.rs:442`) before `Request::Outcome` (`:488-490`) is dequeued, so outcome and published snapshot agree — `outcome.recording` is `None`, `outcome.last_take` the report. Status written once, next Tick republishes the same values: no staleness, no double-report. The pre-stop count is genuinely short: `stop_recording` drops the handle, drains via `capture.stop()`, and only then reads `capture.frames()` for the report (`lib.rs:1034-1046`), while the published `recording` predates the drain. Out-of-scope edge noted: on a finalizing complaint the `?` at `lib.rs:1899` skips `commit_state`, so the take's declaration never enters the history — pre-existing host behavior, handled honestly by the shell.

## 4. Tests — VERIFIED discriminating

On `a49cc6a` the three new tests fail to compile (they use `record_intent`, `recording_line`, `command() -> Result`); transplanted to the old logic they also fail: the old predicate `status.starts_with("● recording")` (verified at `a49cc6a:record_toggle`) yields Name for the export-report case and Stop for the stale-REC case, and `"● REC take-3"` cannot match the old `"● recording"` string. The device-gated `record_toggle` test does **not** discriminate — it existed at `a49cc6a` and passed there, as the archive's T4 caveat concedes. Still untested: the TUI `record_toggle` *wiring* (only the pure function is tested — a branch swap in the `match` would pass headlessly), the host cache directly (T2), `--record-check`'s after-state (T3), and the `outcome()`-Err branch.

## 5. Note vs reality

The note is accurate — the "real take, 57,856 frames" claim belongs to `--record-check`, not the unit test. But `a49cc6a`'s commit message overclaims: "It still records a real take on this machine (the pool shows take-1.ch0/.ch1)". **The device-gated test has never captured a frame** — `take-1`…`take-5.ch{0,1}.wav` are all 44-byte header-only files; play→`o`→stop completes inside one pump tick. The test exercises the state machine, not capture. History now; record it, don't rewrite it.

## 6. The sixth defect the review missed — VERIFIED

`095078f` silently deleted `--sweep`'s idle-starve regression probe (the varied pre-play delay block: 300/1200/2400 ms, three fresh sessions, "a non-zero value here is the regression"). `git log -S "idle {delay_ms"` shows it was **added by `aec0498` — the commit directly beneath this slice, the very fix the probe guards** — and removed by the feature commit, unmentioned in its message or the note. What survives is the single fixed idle-rate read — exactly the "repeated sample" shape the idle-starve note indicts: "Two false negatives from the same instrument is the argument for varying a parameter rather than repeating a sample" (`2026-09-30-an-idle-host-fed-the-device-silence.md:83-88`), whose present-tense consequence still credits `--sweep` as the regression instrument (`:74-75`). Under the standing orders an implemented note must state shipped reality in the change that alters it — the slice altered `--sweep`'s coverage and left that note claiming more than exists.

## Deferred items — none blocks

#4 (cache reset on error): needs a transient IO failure at a refresh point; consequence is the auto-namer wedging on `take-1` refusals with the host's free-id check (`lib.rs:962-967`) preventing overwrite — low likelihood, recoverable, deferral stands but should be next. M2's invariant holds (a `Take` only follows a `Pool` in history, and `set_pool`'s listing sees the files on disk). M1/M4 (duplicated comment still present, verified) and M6 (worst case: `/target/iced-pool` or a foreign project's `target/` when run outside the repo) are mechanical.

The code is mergeable; the debt is documentary: the sweep-probe removal needs its one-line note amendment riding this PR before merge, per the repo's own rule that notes own shipped reality.

VERDICT: merge with changes — the slice is sound, but it silently deleted the `--sweep` idle-starve probe that `aec0498`'s note still credits as the regression instrument, and that note must be amended in this PR before it merges.
