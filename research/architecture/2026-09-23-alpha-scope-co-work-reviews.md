# Alpha scope — the co-work reviews (2026-09-23)

> Research input, 2026-09-23. Co-workers, **read-only**, over the intended alpha scope ("the finish
> line"): cut/copy/paste/append with snapping, new tracks, grid snap, time-stretch/tempo match, loading
> pool clips, mix export with a mastering chain, plus rudimentary manipulation (reverse, …).
>
> - **GLM-5.3-Flash** (value pass, `zai/glm-5.3-flash` via the subagent API) — the gap inventory.
> - **GLM-5.3** (depth pass, `zai/glm-5.3`) — designs for the four hard capabilities.
> - **Kimi K3** (reviewer gate) — **did not return**; see §3. The gate's documented slot is the *merge*
>   gate on a substantive slice, and it is now scheduled on slice A1 (compound gestures).
>
> Signals at review time: repo clean at `31fef35`; 24 core test binaries, 7 `workflow` tests, 26 TUI
> tests, 5 iced tests; clippy and fmt clean. Each reviewer was asked to verify claims against the code
> it read and to label inference.
>
> **Disposition** (the driver's, in the plan
> [`.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md`](../../.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md)):
> the reviews changed the plan's *order* (foundations first), added **four** capabilities the owner had
> not listed (recording into the host, session save/open, stereo material, 24-bit import) and **removed**
> two (zero-crossing snap, auto-crossfade-on-paste → default micro-fades). The per-review dispositions
> are with each text below. Critical claims were re-verified in this repo before being adopted: the
> `Record` stub (`crates/host/src/lib.rs:537-539`), the channel-0-only `WavReader`, the 24-bit refusal
> (`crates/media/src/wav.rs:100`), `duplicate` stacking at the same `at_frame`, the absent save path, and
> `TransportSeek == replay_to` (rebuild + replay + render-to-target).

## 1. Value pass — GLM-5.3-Flash: the gap inventory

> **Disposition: adopted, in full, and it reprioritised the plan.** Its first finding (recording is not
> wired into the host) became slice **A3** and is the single most important correction to the owner's
> list; session save/open became **A2**; snap-in-the-language became **B5**; the clipboard **B6**; stereo
> and 24-bit **A4**; the mastering plugin **D12**; reverse as a **reader mode** rather than a pool copy
> **C10**; the import-ordering and `@frame` traps went into the export/pool slices. Two adjustments:
> (a) snap lives in *both* the language (`snap=` modifier) and the shell (the playhead and the ruler need
> a grid), because the shell-side helper still has to snap the playhead; (b) paste is a shell clipboard
> plus a *compound gesture* rather than a new `PasteRegion` op — the group mechanism (**A1**) covers it,
> and a host op is only worth adding if it earns its place later. Its "already built / don't rebuild" list
> (the whole ACID op set, rate conformance, peaks, undo-by-replay, the `:` line, help-from-keymap) is why
> the plan does not restate those as work.

### The review, verbatim

# Gap inventory — sound-arranger → usable alpha/beta (cross-vendor value pass)

Evidence base: `crates/host/src/lib.rs`, `crates/media/src/timeline.rs`, both spike READMEs, README "Honest gaps", RESEARCH §0/§7/§11, and the no-sidecars + P1.3 clip-editor notes. Everything below is verified against those files unless labelled *(inferred)*.

## 0. The single biggest finding

**The owner's list is mostly a *shells/wiring* list, not a *feature* list — except four real engine gaps (record, snap/tempo-fit, reverse, mastering).** The arrange vocabulary (`ArrangeOp`, `timeline.rs:110`) already covers cut/trim/move/cross-track/duplicate/fade/loop/chop, and the TUI dispatches it. What blocks "sit down and arrange a piece end to end" is that the session has no *life* around the log: no save/open, no recording through the host, no snap, no effects, and fragile import.

## 1. Must-have for alpha (blocks the loop; ordered)

1. **Session save/open.** The log lives only in memory; host has no save/load (grep of `host/src/lib.rs`: only `Pool::open`). A crash or `q` loses the piece. Smallest: serialize the command list + pool dir to one file; open = replay. **host**. Test: script → save → open → `bounce` byte-identical.
2. **Recording through the host.** `HostCommand::Record` returns `Err("recording requires a device — … not wired into the host")` (`host/src/lib.rs:537-539`). Capture machinery exists (drift compensation, crash recovery) but no shell can reach it. Alpha is *record → arrange*; the loop's first half doesn't exist. Smallest: host owns a `CaptureNode` route into the pool; `record` logs, take lands as a pool id. **host (wiring) + media (built)**. Test: probe asserts a new pool source with nonzero frames/peaks.
3. **Snap-to-grid / musical time.** Tempo map exists in the engine, but `ArrangeOp` positions are raw frames and there is zero snap code. `H`/`L` "one beat" lives in the TUI — the workflow vocabulary can't express it, and iced has no beat nudge at all (a one-shared-workflow violation forming). Smallest: a `snap=` modifier in `parse_arrange`/`parse_arrange_line` so it's in the *language*. **host + workflow**. Test: `move_clip … 48213 snap=480` lands on 48000.
4. **Copy/paste of a selection (clipboard).** `Duplicate` clones one clip in place; no clipboard, no multi-clip copy, no paste-at-frame — the core ACID gesture, absent from both ops and workflow `Action`s. Smallest: shell-level clipboard of `Clip` values + composed `add_clip`s (clips are pure values — cheap); logged `PasteRegion` op later. **workflow + shell now, host later**.
5. **Mix-out processing.** Mastering is Phase 4, but the owner's compressor is cheap: one fundsp compressor mounted pre-master, `set_param comp.*`. Do not build an FX rack. **engine (plugin) + host**. Test: bounce a transient, assert peak reduced and tail present (drain path shipped).
6. **Stereo clips.** README's honest gaps admits it; every stereo take is unusable as-is, and it gates "load clips from the pool" for real material. **media** (`ArrangerNode`, pool). Test: stereo source renders both channels to the 2-ch bounce.

## 2. Should-have (beta: someone else can use it)

7. **Reverse** (owner asked): logged, non-destructive `ArrangeOp::Reverse` — implemented as a per-clip reverse-read mode in `ArrangerNode`, **not** a reversed pool copy (the latter is a destructive sidecar-file workflow that violates pool immutability). Test: reversed clip of a ramp plays descending.
8. **Tempo match / time-stretch.** `rubato` is already the research pick. Offline "render stretched clip to a *new pool source*" fits the no-sidecars pattern. Keep it out of the render path — realtime stretch breaks no-alloc and PDC assumptions *(inferred from the stated core constraints, not verified in `ArrangerNode`)*.
9. **Undo coverage for fader rides.** Undo is arrangement-edits only (TUI README); param moves are permanent. Smallest: document + bounded `set_param` undo, not full param undo.
10. **Seek/latency hygiene.** `TransportSeek` is O(target) — measured 92 ms stall, bound to `,`/`.` and mouse wheel. Beta needs incremental rebuild or debounce. **host live actor**. Measure: the shell already prints command latency — assert < 10 ms in a probe.
11. **Import breadth.** Pool is float-WAV-only; real takes are 16/24-bit. One decode step on `import` (symphonia is in the research matrix). **media**. Test: 24-bit import conformed to session rate.
12. **Markers / clip naming.** Arranging a 20-minute jam blind is miserable; `ChopClip` exists but no naming/annotation in the vocabulary. Smallest: `set_clip_name` op + TUI label.

## 3. Audit of the owner's list

- *"cut and paste"* — cut yes, **paste no** (gap 4). Their word "paste" is doing more work than the vocabulary supports.
- *"snap to grid"* — missing entirely (gap 3); **highest-leverage item on their list**.
- *"timestretch/tempo match"* — real gap but **wrong priority**: nothing to stretch until record + stereo work.
- *"load clips from the pool"* — works and is proven (probe amplitude asserts in the TUI README); but the pool-before-arrange ordering is a trap (`host/src/lib.rs:543` "arrange requires set_pool first") — surface it in the shell, don't leave it an error string.
- *"export final mix"* — bounce works (16-bit only). Add float/24-bit before calling it alpha-complete.
- *"compressor on mix out"* — gap 5; cheap.
- *"reverse"* — gap 7; later-alpha.

**Already built, don't rebuild:** the whole ACID op set, rate conformance + resampler, peaks, undo-by-replay, the `:` line, help-from-keymap. The owner under-credits what exists.

**Traps:** (a) paste/snap as shell-only helpers — they must be logged ops or replay diverges from what the user did; (b) external sox/ffmpeg round-trips for the *mix-out* — fine for sound design, wrong for a master bus; (c) the `@frame` + bounce ordering trap documented in the TUI README — export UX must not expose it; (d) new verbs in `parse_script` are format growth — the versioned `host v1` needs an explicit compat note, not silent additions.

## 4. Architecture friction (specific)

- `HostHandle::execute` applies control mutations on the call stack and rebuilds readers per edit (README honest gaps); record-while-arranging (gap 2) stresses this first.
- Snap must be in the language, not keys: `H`/`L` beat-nudge living in the TUI is the existing violation of the one-shared-workflow rule.
- No-alloc render path: per-clip DSP (reverse reader, stretch) must be pull-based like the existing reader reuse, or the counting-allocator test in engine breaks.

## 5. Later / cut

- iced timeline (TUI is primary; don't split effort pre-alpha).
- CLAP export, MIDI clock out, comping, FLAC/MP3 export, TUI macros/registers — correctly deferred by notes; keep them there until beta.
- **Cut:** any plugin *hosting* — the no-sidecars note already killed it; keep it dead.

## 2. Depth pass — GLM-5.3: designs for the four hard capabilities

> **Disposition: adopted, with one synthesis.** The compound-gesture mechanism (host history as groups,
> `group begin`/`group end` as parser lines, engine log untouched) became slice **A1** and its argument
> for putting it *first* (persistence journals per group; stretch logs as a compound op) is why it now
> precedes save/open. The session-directory + append-only-journal persistence became **A2**. The `master`
> plugin plus `export`-as-a-`bounce`-sibling (f32 default, fixed-seed TPDF for s16, render from frame 0,
> fail rather than clip) became **D12–D13**. The stretch design (WSOLA, materialised into the pool, a
> logged `u32/u32` ratio, `source_tempo`, refuse loops, re-validate fades) became **C11**, and its
> ordering — group → persistence → export → stretch — is the plan's order. Its "most expensive is
> stretch, and the cost is the determinism discipline, not the kernel" is recorded in the risks. One
> synthesis with the value pass: the utilities are **gestures over existing ops** (normalize = measure +
> `set_clip_gain`, invert = gain −1, silence = gain 0, trim-to-content = peak scan + compound `trim`) and
> only **reverse** needs a new clip property + reader mode, so phase C is much cheaper than either review
> assumed on its own. Its alpha cuts are adopted as the plan's explicit cut list (except auto-crossfade,
> replaced by default micro-fades).

### The review, verbatim

# 1. Snap, cut/copy/paste, append — and the compound gesture

**Snap is a pre-log transform, never a logged op.** The log stores absolute frames; a snapped edit is just an edit whose frame was quantized before the `Arrange` command was issued. This keeps determinism untouched. Two quantizers, both control-side in `crates/media` (a new `snap.rs`) + `crates/workflow` keymap exposure:

- *Grid snap*: `snap::to_grid(frame, grid, tempo_map, rate) -> u64`. Beat = `rate * 60 / bpm` from the engine's `TempoMap`; grid is a division (1/4, 1/8, 1/16, triplets). The grid setting itself is UI state — do **not** log it (it changes no audio; logging it would pollute the "log = audio-relevant state" contract). Label: inference from "the log is the document" — I read that as *audio-relevant* state.
- *Zero-crossing snap*: `snap::zero_cross(source_id, frame, window) -> u64` — scan the pool WAV (control side, `WavReader`) within ±`window` (default ~5 ms) for the nearest sign change, tie-break toward the grid candidate. Deterministic given the pool bytes, so replay is unaffected: only the resulting absolute frame appears in the log.

**Clipboard = a sub-timeline value, shell-owned, never logged.** `Copy` mutates nothing, so it must not enter the log — the log is the command list of *state*. The clipboard is `struct Clipboard { base: Frame, items: Vec<(track_delta: usize, Clip)> }` — copied `Clip` values (they're already `Clone + PartialEq` value types in `timeline.rs`) with positions normalized to the selection's first frame. `Paste` is one logged compound op (below) that re-issues `AddClip`s with minted ids. Id minting follows the existing `ChopClip` precedent (`prefix.N` is a pure function of a logged operand): `paste.{k}` inside a `paste` group op. Replay: paste *is* logged (it changes state), so a replayed log reproduces the pasted clips identically — the clipboard itself never needs to survive a replay.

**Append** needs no new op: it is `MoveClip`/`AddClip` with `at_frame = clip.end()` of the preceding clip, or `arrangement_end() = max(end())` — a pure function of the current `Timeline` computed on the shell side. Keep the value ops absolute-frame-only; "append" is a shell gesture in `crates/workflow`, not a timeline concept.

**The compound gesture — the actual log-level mechanism.** Today `HostSession::undo` (`host/src/lib.rs`) rpositions the last `HostCommand::Arrange` in `self.history: Vec<HostCommand>` and `replay_to`s. One gesture = N ops = N undo steps is the bug. Fix at the **host history level**, not the engine log:

- Change `history: Vec<HostCommand>` to `history: Vec<Vec<HostCommand>>` (each entry = one atomic unit; a single command is a one-element unit). A gesture opens a group on the host, streams its `Arrange` commands, closes it, and the closed group is pushed as one entry — applied all-or-nothing (fold `Timeline::apply` over a clone first; a mid-group `Err` means the group never enters history, matching "a refused op is never logged").
- What appears in the log: the same N engine events as today — grouping adds nothing to the engine log, so replay and byte-identical bounce tests are untouched by construction. What changes is `undo`/`redo`: rposition over *entries*, remove the whole group, `replay_to(current_frame)` (which is exactly the existing rebuild path — playhead preserved, pool/mounts survive). Redo re-inserts the group at its recorded index; a new edit clears `redo` as now.
- Text form (needed for persistence, §4): group markers as real lines the parser accepts, so a saved script round-trips gesture structure:
  ```
  group begin
  arrange razor_split t0 c0 cL cR 2998
  arrange trim t0 cL end -500
  group end
  ```
  Markers are host-level (not audio-touching `HostCommand`s); they only bracket history entries.

Traps: (a) `replay_to` skips commands with `at_frame > target` — group members must share the group's effective frame, or the skip must operate on whole entries; (b) paste onto a *target* track vs source tracks — encode as a paste field (`paste_track: Option<Id>`), don't re-derive from the clipboard; (c) `clip_id_exists` uniqueness — minted paste ids must be checked against the live timeline, and the paste fails atomically if any collides.

# 2. Time-stretch / tempo match

**Argue: offline, render-to-pool, WSOLA-class.** Realtime stretch on the reader thread breaks the render contract (lookahead buffering, per-block allocation or a fixed latency that complicates the `popped + off0 == off` alignment invariant in `ArrangerNode`); phase vocoders smear transients on exactly this project's material (recorded generative jams); élastique-class is proprietary. WSOLA (overlap-add with a correlation search of ±~10 ms around a ~40 ms hop) is ~200 lines of pure f32 integer-indexed arithmetic, cheap, and its artifacts are acceptable for an alpha. Varispeed (pitch changes) already exists for free via `media::Resampler` — expose it as an import-time choice, not a clip property.

**Frame domain: preserve the one-domain rule by materializing the stretch into the pool.** Do **not** add a `playback_rate` to `Clip`. Stretching a clip is a control-side render that writes a *new pool source* (`s1.x3-2.wav`, named by source + ratio), and the logged op rewrites the clip's reference:

```
arrange stretch t0 c0 s1x3-2 6000 3 2 @0
```

operands: track, clip, new source id, new `src_len`, ratio num, ratio den. After it applies, the clip is an ordinary clip in the one frame domain — `ArrangerNode` never knows a stretch happened. This is the same shape as import resampling (`Pool::conform`): foreign material converted once at adoption; edited material converted once at gesture time. Tempo match is `stretch` with num/den derived from the tempo map (`set_tempo` is already logged) plus a per-source tempo: add a logged pool-context line `source_tempo s1 120` (state, replayed, no audio effect until used).

**Determinism / byte reproducibility:** the ratio is logged as a *rational* (`u32/u32`), never an f32 — float ratios would make output length and interpolation phase platform/order sensitive. The WSOLA kernel is pure arithmetic over the source's f32 bytes, so same (pool bytes, num, den) → byte-identical stretched source; test it exactly like the bounce tests (stretch, hash, re-stretch on a fresh session, compare). Clamp ratios outside [1/8, 8] with a fail-loud refusal.

Traps: refuse `stretch` on a clip with `loop_len` set (loop phase not representable — mirror `RazorSplit`'s refusal); fades/gain carry over but `src_len` changed, so the op must re-validate `fade_in + fade_out <= src_len`; trimming a stretched clip can exceed the stretched source and play silence (already true of `Trim End` today — acceptable, but document it).

# 3. Export + mastering

**The chain lives in the engine graph, not an offline pass.** Add a `master` plugin in `crates/engine/src/plugins` (registered in `HOST_PLUGINS`), mounted after the mixer so it claims `out_node`; bounce and the live pump (`live.rs` `fill_audio`) then both flow through it with zero new code paths — determinism relative to the bounce tests is inherited because the same graph renders both. The live path gets monitoring compression for free.

Minimal chain, one node, all params `f32` in `HOST_PARAMS`:
- compressor: `threshold` (dB), `ratio`, `attack_ms`, `release_ms`, `makeup` (linear);
- lookahead brickwall limiter: `ceiling` (linear, default ~0.981 ≈ −0.4 dBFS), fixed ~5 ms lookahead — the node reports `latency()` and the bounce must compensate exactly like any latency-carrying node;
- offline-only normalize: not a graph param — the export handler measures the rendered buffer and applies a scalar + re-limit pass, keeping the graph sample-exact.

```
mount master threshold=-18 ratio=4 attack=5 release=100 makeup=1.0 ceiling=0.981 @0
export /abs/final.wav f32
```

Add `export` as a *sibling* of `bounce`, do not overload it: `bounce` keeps its 16-bit WAV, `MAX_DRAIN_FRAMES` drain semantics and byte-identical test role. `export` defaults to **f32 WAV** (no dither, bit-exact → testable byte-identical against a golden file, satisfying determinism by the same mechanism as the bounce tests); `export … s16` applies TPDF dither from a **fixed-seed** PRNG (never `rand` thread-local) so 16-bit output is also reproducible. Guarantees `export` makes: peak ≤ ceiling (limiter), optional loudness target (alpha: simple peak/RMS normalize; full LUFS is cut), no clipped frames (assert in the handler — fail the export rather than write a clipped file), identical bytes across runs.

Traps: compressor state at bounce start — the graph must render from frame 0 or the ballistics differ, so `export` replay-renders from 0 like `seek_to`, not from the playhead; dither order is L-then-R per channel with the same seed or stereo files differ run-to-run; the mixer's existing `master.gain` vs `makeup` — document one truth (master.gain is the fader, makeup belongs to the comp).

# 4. Session persistence

**A session is a directory: `mysong.d/` with `session.txt` + `pool/`.** The log already *is* the document; persistence is "serialize `history` and the pool":

- `session.txt`: a `host v1` script (the existing `parse_script` is the loader — no second format). Content: the state-command history verbatim (`is_state()` commands only, plus the `group` markers from §1), preceded by context lines that must become real commands because `HostSession::new` hardcodes 48 kHz/120 bpm: `session_rate 48000`, then the existing `set_tempo`, and `pool pool/` (relative to the session dir — resolved on load).
- `pool/`: the content-addressed WAVs, including conformed (resampled) and stretched material. What is **not** saved: `wired_tracks`, `arranger_underruns`, `last_drain`, the derived `Timeline` snapshot (replayed), bounce outputs, the `pool_conformed` report, the clipboard, grid/snap settings. Everything derived is rebuilt by replay — the architecture paying off.
- Save/load: `HostCommand::Save { dir }` / `Load { dir }` (actions, not state — mirror `Bounce`). Save writes `session.txt` atomically (temp file + rename); nothing already in `pool/` is copied.
- **Crash safety via an append-only journal**: after every committed state entry (§1 group), append its lines to `mysong.d/journal.txt` and flush. Autosave *is* the journal — no timer, no snapshot cadence. On load, tolerate a truncated trailing line (torn write) by dropping it and reporting. This dovetails with the pool's existing crash posture (import keeps the original beside the converted file as `.pre{rate}`; imports are idempotent because sources are content-hashed — label: inference from the `set_pool`/conform code; I did not re-read the pool notes this pass). A journal replay after a crash re-runs deterministic commands over an intact pool, so recovery = ordinary `run_script`.
- Versioning: the `host v{N}` gate already refuses mismatches. Alpha migration = none: keep v1, only *additive* ops (the parser refuses unknown ops, so an old script in a new host loads; a new script in an old host fails loudly at the exact line — honest and correct). Introduce `v2` only when an existing line's *meaning* changes.

Traps: paths in the log (`pool`, `bounce`, `play`) are absolute today — a moved session breaks; make `pool` session-relative on save (re-emit the line at save time; the in-session log stays authoritative). Undo/redo history: persist the `redo` stack only if cheap; cutting it is fine for alpha (document that loading collapses history into one baseline plus journal-going-forward).

# Ordering, cost, and alpha cuts

Implement in this order: **(1) group/compound gesture first** — it is the mechanism §2's `stretch` op and §4's journal entry both log through, and it fixes the standing one-gesture-two-undos bug; **(4) persistence second** — once history is `Vec<Vec<HostCommand>>`, the journal is a trivial serializer and everything after rides on it; **(3) export/mastering third** — small, self-contained engine plugin plus two commands; **(2) stretch last**. Quietly the most expensive is **stretch**: the WSOLA kernel is a week, but the determinism discipline (rational ratios, byte-identical re-render tests, pool materialization naming, loop refusal, fade revalidation) is where the time actually goes — and the only one of the four whose quality has an open-ended tuning tail.

Cut from the alpha: zero-crossing snap (grid snap covers the owner's stated need; add ZC later as a second quantizer behind the same shell seam), clip-level varispeed (import-time resample suffices), LUFS/true-peak (peak normalize + ceiling limiter is enough), formats beyond WAV, dither beyond fixed-seed TPDF s16, stretching looped clips, auto-crossfade-on-paste (a `set_clip_fade` line after paste is explicit and already deterministic), and redo-stack persistence.

## 3. Reviewer gate — Kimi K3: attempted, did not return

> **Disposition: deferred to the first architectural slice (A1), before that commit merges.** Two routes
> were tried against the co-work note's own invocation table:
>
> 1. **`opencode-go/kimi-k3`** via the subagent API — failed twice before finishing (no closing message).
>    The owner confirms the `opencode-go` provider is currently broken.
> 2. **`kimi -p "<brief>"`** (the note's documented kimi-cli route) — the process planned its pass
>    ("read the big files … write the report") but then sat at ~0 % CPU for several minutes with no
>    output: in `--prompt` mode the CLI cannot be combined with `--auto` ("Cannot combine --prompt with
>    --auto"), so its tool approval had no TTY to answer it. Killed after 3.5 minutes of no progress.
>
> What this means for the plan: it is reviewed by **two cross-vendor passes** (GLM-5.3-Flash and
> GLM-5.3 — independent of the DeepSeek author) and **not** by the designated gate. The gate is not
> skipped forever: the co-work note puts Kimi on *substantive slices before merge*, and slice A1
> (compound gestures — a host history/log-shape change) is exactly that, so it goes to Kimi first.
>
> **Fix for the next attempt** (so the gate is not blocked again): drive `kimi --auto` inside a pty
> (`script -qec "kimi --auto" /dev/null` with the brief fed as the first message) so approvals
> self-answer, or set the CLI's permission mode in `~/.kimi-code/config.toml`; `--prompt` is
> unattended-but-read-only-blocked, `--auto` is unattended-but-interactive-only.
