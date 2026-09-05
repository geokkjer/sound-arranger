> External review (verbatim). Source: GLM-5.3 (full) via the `pi` harness,
> `--model glm-5.3 --thinking high`, 2026-09-05. Scoped by the driver to this
> session's work: stereo master + pan, the Tauri shell↔Host-API bridge, the
> incremental session API + serializable arrangement, and the ChopClip op.
>
> **Disposition:** the four **Blocking issues** are real. #1 (byte-identical
> replay across a mid-session channel-changing mount, single render call),
> #2 (pan unreachable from the text format — the host's `HOST_PARAMS` closed
> vocabulary never gained `ch{i}.pan`), and #4 (a refused mixer mount mutates
> `mixer_channels` during `process` before a successful `apply`) were each
> re-confirmed against the code by the driver. #3 (`play` after any render breaks
> the session due to topological-order violation) is plausible and matches the
> `wire_arranger`/`insert_before` pattern; #5 (arranger underruns counted but
> never surfaced) is a host-boundary gap. The `**What's sound**` list and the
> nits are accepted as-is; a fix round should address #1–#5, then the nits
> (chop discarding fades, shell swallowing arrangement errors, no channel-count
> check on audio cords, `MAX_BOUNCE_BYTES` not accounting for stereo samples,
> solo-on-disconnected-channel semantics).

---

# Value-pass review — stereo master, host bridge, incremental session, ChopClip

## Blocking issues

### 1. Byte-identical replay is broken across a channel-changing event — `crates/engine/src/render.rs:712–761`
`render_block` captures `ch = graph.out_channels()` at block entry and slices every subsequent chunk with `slen = flen * ch` (`render.rs:717, 730`), but `Graph::render` recomputes `frames = out.len()/channels` from the *current* master (`graph.rs:1005`). If the master's width changes mid-call, the two disagree. The live path never hits this (`process` renders up to each command's frame, so mounts are always due at a render-call boundary and `channels_for_render` flushes them first). **Replay does hit it**: `replay_from` schedules the logged mount at its recorded frame, and a single render call crosses it. Result: half the frames rendered, clock advances half as much — the render is no longer a pure function of the log.

This is not hypothetical: the untracked `crates/engine/tests/zz_review_repro.rs` (left by a prior review pass) fails at HEAD — live clock 2048, replay clock 1536 for the same log and same requested frames. So **the workspace test suite is red right now**, contradicting commit 5e8d4bd's "cargo test --workspace green". The note's claim (`2026-09-05-stereo-pan-foundation.md`: "verified by `replay_is_exact_for_mid_session_mount` — rendering in two calls at the mount frame") only verifies the case where the caller happens to split the render at the mount frame; the replay harness renders in one call. Sibling repro B (`schedule_unmount` mid-call) "survives" but is also wrong: a 224-frame post-unmount segment advances the clock by 448 and silently skips frames. Fix direction: re-read `out_channels()` per chunk in `render_block`, or park channel-changing events at the render-call boundary the way arrangement ops are parked.

### 2. Pan is unreachable from the Host API — `crates/host/src/lib.rs:42–49`
`HOST_PARAMS` (the closed vocabulary `parse_script` validates `set_param` against) never gained `ch{i}.pan`; `MIXER_PARAMS` did. Verified: `set_param mixer ch0.pan -0.5` → `unknown param 'ch0.pan'`. The headline feature of commit 1 cannot be set from the text format — the only surface the Tauri shell exposes — so the UI can never pan. One-line-per-channel fix plus a test.

### 3. `play` after any render permanently breaks the session — `crates/host/src/lib.rs:289`
`Play` appends the player node with `graph.add_node` (end of topological order), and `wire_pending` then connects player→mixer. It only works today because mounts stay *scheduled* until the first render, so the player happens to land before the mixer. Once the mixer has materialized (any bounce/render between mount and play), the player lands after it → `connect: patch cords must go forward` → every subsequent render fails. Verified: `mount mixer @0; bounce 512; play a.wav ch0 @512; bounce 512` → `Err("player→mixer cord: …")`. `wire_arranger` already solves this exact problem with `insert_before(mixer)`; Play needs the same treatment.

### 4. A refused mixer mount mutates session state — `crates/host/src/lib.rs:571`
`process` sets `self.mixer_channels = Some(channels)` *before* `apply` → `engine.mount` refuses ("already mounted"), but the field is already overwritten. Verified on the persistent path: mount 4ch → bounce → refused `mount mixer channels=1` → legal `play … ch3` is refused with "beyond the mixer's 1 channels". Direct violation of "a refused op returns Err and changes nothing". Set the field only after a successful `apply`.

### 5. Arranger underruns are counted but never surfaced — `crates/host/src/lib.rs:488–491`
`HostSession::underruns()` reads only the `PlaybackNode` counter. The per-track `ArrangerNode` counters built in `wire_arranger` go into the graph and nothing ever reads them (the host keeps only `NodeId`s; the nodes — and their counters — are destroyed on every re-wire). A reader slip therefore *silently glitches* the bounce (`pop_sample` → 0.0) with zero visibility in `underruns()`/`summarize()`/`ScriptOutcome.underruns`, violating the no-silent-glitch invariant at the host boundary. The reference test asserts `underruns() == 0` and passes while checking the wrong counter.

## Nits

- **Chop discards fades silently** — `timeline.rs:450–451`: every piece gets `fade_in: 0, fade_out: 0`. `RazorSplit` deliberately preserves the outer fades; chop changing audible content (fades vanish) with no refusal and no documentation is inconsistent. Preserve the outer fades or document/refuse.
- **Shell swallows arrangement errors** — `shell/src-tauri/src/lib.rs:44`: `session.arrangement().ok()` converts a snapshot failure into `None`, directly contradicting the accessor's own doc ("a shell must see 'could not build the arrangement', never a silent empty value") and conflating "nothing built" with "build failed".
- **No channel-count check on audio cords** — `Graph::connect` / `Engine::validate_patch` check signal *kind* but not `channels`; a stereo→mono audio cord is accepted and the fan-in sums the first `frames` samples of an interleaved stereo buffer as mono (garbage, silently). Unreachable through the current host (the mixer is terminal), but `Port.channels` exists precisely to make this fail loud — add the check until stereo cords are implemented.
- `MAX_BOUNCE_BYTES` (`host/src/lib.rs`) bounds `frames * 4` but a stereo master allocates `frames * 2 * 4` — the real budget is 2× intended.
- Solo semantics: `any_solo` scans only `solos[..n]` (`mixer.rs:216`) — a solo set on a disconnected channel is ignored rather than muting the bus. Defensible ("accepted but ignored" for unconnected ports), but worth a sentence in the docs.

## What's sound (checked, no action)

- **Pan law is correct**: `a = (pan+1)·π/4` gives (-1 → (1,0), 0 → (√2/2, √2/2), +1 → (0,1)); master meter `max(|L|,|R|)` post-master-gain as documented.
- **Partial last block holds**: `render_into` chunks by `BLOCK*ch`; the remainder of `frames*ch` samples is always a multiple of `ch`, so `frames = out.len()/channels` is exact; bounce reads the width after render flushes (`media/src/lib.rs`, host `Bounce`) — correct in both.
- **Mono-by-default preserved**: `Port::audio` is 1-channel; a graph without the mixer takes the identical code path as before (existing suites pass unchanged apart from intended mixer-stereo updates).
- **ChopClip value logic is solid**: validate-all-then-mutate (derived ids checked across all tracks before any removal/extension), deterministic ids `{prefix}.{i}`, contiguous tiling with the first `rem` pieces taking the extra frame, `times==0`/`times>src_len`/looped-clip refusals, overflow-safe accumulation; codec, `ALL_OPS`, and text format all wired (`arrange chop …` parses).
- **Render path allocation**: the counting-allocator test covers the stereo mixer path; `Graph::render` slices preallocated buffers; the mixer renders via `chunks_exact_mut` with no allocation. PDC delay loops are sample-wise, so interleaved stereo passes through correctly.
- **`execute` vs `run_script`**: both funnel through `process` — the incremental path cannot diverge from the one-shot path by construction. `ClipEditor::apply`'s live-apply-then-log ordering is safe as wired (`ensure_editor` registers handlers first), though it would diverge value-from-log if `arrange_logged` ever refused after a successful live apply — worth a guard comment at minimum.
- Pre-existing, out of scope but noting: `RazorSplit` can produce halves violating the fade invariant (`fade_in > src_len` — no `validate_clip` on the halves; the arranger's `fade_gain` tolerates it, so no crash, but the value model holds an invalid clip).

## Test gaps

- No `ChopClip` in `every_op_round_trips_exactly` (`clip_editor.rs` tests) — the W1 codec guard exists but wasn't extended to the new op; encode/decode for chop is only covered transitively.
- Pan law asserted only at center; hard-left/hard-right (and the `-1..1` clamp in `set_param`) untested.
- No test that a refused command leaves host state untouched (would have caught #4), nor play-after-render (#3), nor replay-across-mount in a single call (#1 — the existing test deliberately renders in two calls, which is exactly the hole).
- The failing `zz_review_repro.rs` is untracked; whoever keeps it should commit it as a regression test once fixed.

## Verdict: **ISSUES**

#1 is a genuine determinism regression with a failing in-tree test; #2–#5 are contract violations (pan unreachable, broken play ordering, refused-op state mutation, unsurfaced underruns). All are localized and fixable without architectural change — the underlying stereo design (interleaved master, per-port channels, equal-power pan) is sound where it's actually reachable.
