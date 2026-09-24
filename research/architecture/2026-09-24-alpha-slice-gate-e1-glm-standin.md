# Reviewer gate (stand-in) — alpha slice E1 (utility gestures), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unusable: every API run today died with no output, including one after
> the 5-hour window reset — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: slice E1 (`reverse` as a clip property, plus normalize/invert/silence/trim-to-content),
> working tree at the time — `Clip.reversed` and the mirror-aware ops in `crates/media`, the codec,
> the host's parser/formatter, the five workflow actions, and the TUI's gestures and panel mirror.
>
> **Disposition: `merge with changes`; the must-fix and all three should-fix findings were real and
> are fixed, with tests. The addendum's iced-shell claim is refuted with evidence.**
>
> 1. **The peak scans read only the positive half** (must-fix) — `PeakBuilder::range_minmax` returns
>    the *signed* extremes, and both `normalize_clip` and `trim_to_content` took `max` alone. A clip
>    swinging to −0.9 with a `max` of 0.25 normalized as if its peak were 0.25 (gain 4 → hard
>    clipping), and a bin whose energy is negative-only read as silence and was **trimmed away** —
>    audible content deleted. Fixed with `Source::peak_of = max(|min|, max)` in both scans; test
>    `the_peak_scans_use_both_halves` uses a constant −0.9 fixture (the first draft refused it as
>    silent).
> 2. **A pasted reversed clip silently lost its direction** (should-fix) — the clipboard is a value
>    copy that round-trips through `add_clip`, which has no direction operand, so `y`/`p` on a
>    reversed clip produced a forward clip (in-session and after a reload), with no hint. Fixed: the
>    paste emits a `reverse` line for each reversed entry **in the same group** (one undo); test
>    `a_paste_keeps_a_reversed_clip_reversed`.
> 3. **`AddClip`'s encoding was silently lossy for `reversed: true`** (should-fix) — encode wrote no
>    field and decode pinned `false`, narrowing the codec's "round-trips exactly" contract while the
>    test only fed forward clips. Fixed: the field is carried, and the codec's round-trip test now
>    includes a reversed clip.
> 4. **`U` measured a looped clip's repeats** (should-fix) — `source_region` returned
>    `[src_start, src_start + src_len)` while a looped reader plays one `loop_len`, so the peak (and
>    the +12 dB cap and the silent refusal) came from unheard material. Fixed: one loop; test
>    `the_scan_region_is_what_the_reader_plays`.
> 5. **`T` on a heavily-faded clip failed opaquely** (note) — a clip whose fade-in fills it (reachable
>    with `f`, and preserved by paste) made both trims fail `validate_clip`, refusing the group with
>    "fades exceed the clip length". Fixed: the fades are capped to the new length **first in the
>    group** (the host validates each op against the running value, so the cap must precede the
>    trims) and the status reports it.
> 6. **`i` on a silent clip** reported "already inverted" (note) — now it says silence has no
>    polarity to flip.
> 7. **Panel/engine clamp divergence** and **bin-granularity detection seeing adjacent material**
>    (notes) — recorded as limitations in the note: the panel clamps a source lookup to the last
>    frame where the engine reads on (pre-existing, inherited by the reversed case), and the 256-frame
>    bins can see loud material just outside a clip's edge (within the acknowledged one-bin error).
> 8. **The addendum's claim that iced's new keys are "silently dead"** — **refuted**: iced's key
>    translation maps characters into the shared `WorkflowKey` (`spikes/iced-shell/src/main.rs:333-348`)
>    and `on_key` calls `workflow::action` then `dispatch` (`:369-371`), whose catch-all reports that
>    the iced timeline canvas is not built (`:446-452`). The `_ => return None` the reviewer read is
>    the translation's fallthrough for unmapped *iced* keys (function keys), not for workflow
>    characters.
>
> Reviewer-verified (recorded because they are the slice's load-bearing claims): `source_frame_at`
> at both ends; the mirror arithmetic of all four ops by hand — split (left = the top), trim start
> (`src_start` fixed), trim end (the sign flip, including extend-past-zero refusal), chop (walks
> down) — with extending trims, 1-frame clips and a region at source frame 0; `reversed` preserved on
> duplicate/move/set-clip-fade/split/chop; the loop⇄reverse refusals both ways, with looped+reversed
> unreachable in the panel; host parse (strict arity) + formatter + round-trip; the toggle's replay
> determinism; and `trim_to_content`'s scan termination, silent refusal and the exact (lead, trail)
> swap for a reversed clip.

---

## Verbatim review

Adversarial review of E1 (utility gestures).

## Verdict
merge with changes

## Findings

1. **must-fix — Normalize (and trim-to-content) use only the positive half of the peak.** `normalize_clip` does `let (_, peak) = clip.source.minmax(lo, hi)` (spikes/tui-shell/src/main.rs:1876); `Source::minmax` → `PeakBuilder::range_minmax` returns `(min, max)` of the *samples* (crates/media/src/peaks.rs:131-151; spikes/tui-shell/src/timeline.rs:87-89). True peak is `max(|min|, max)`. The panel's own envelope draws both halves (timeline.rs:830-835 uses `(min, max)`), so the picture shows a −0.9 excursion the gesture ignores. Scenario: a clip with min=−0.9, max=0.25 → gain 4 → post-normalize peak −3.6, hard-clipped audio. Same root cause in `trim_to_content`'s two scans (main.rs:~1960, ~1970): a bin whose energy is only negative (min=−0.5, max=0) reads `peak=0 ≤ FLOOR` and is trimmed away as silence — audible content deleted.

2. **should-fix — Pasting a reversed clip silently loses the reversal (live, not just on reload).** Paste emits `add_clip {track} {id} {source_id} {src_start} {src_len} {at} {fades} {gain}{looped}` (main.rs:1569-1573) — no reversed field — and both the host parser (`crates/host/src/lib.rs:2570`, `reversed: false`) and the clip_editor decoder (`crates/media/src/clip_editor.rs:297-300`, hard-coded `false`) drop the flag. `y` a reversed clip, `p`: the paste is forward, in-session and after reload, with no status hint.

3. **should-fix — `clip_editor` AddClip encode is silently lossy for `reversed: true`.** `encode_op` AddClip writes no `reversed` field (clip_editor.rs:100-115) while decode pins `false` — an `AddClip{reversed:true}` encodes "successfully" and decodes to a different value. The `every_op_round_trips_exactly` test only feeds `reversed:false` clips (clip_editor.rs:490-503, 524-527).

4. **should-fix — `U` on a looped clip normalizes the wrong region.** `source_region()` returns `(src_start, src_start + src_len)` (timeline.rs:161-165), but a looped clip's reader plays only `[src_start, src_start + loop_len)`. The peak (and the +12 dB cap decision, and the silent refusal) is computed over `times`× material the listener never hears.

5. **note — `Placed::source_frame` clamps to `frames−1`; the engine does not.** Panel (timeline.rs:180-183) vs `Clip::source_frame_at` (crates/media/src/timeline.rs:88-95). For a hand-written script whose region runs past the source, forward clips draw the last frame repeated and reversed clips draw the top clamped — the envelope diverges from what the engine will read. Pre-existing divergence class.

6. **note — `T` refuses faded clips via an opaque validate error.** Neither `Trim` nor `trim_to_content` shrinks fades, and `validate_clip` requires `fade_in + fade_out <= src_len` (timeline.rs:248-250). A clip with a fade-in filling its length makes both trim lines fail → the group is refused with "fades exceed the clip length".

7. **note — bin-granularity silence detection can see outside the region.** `range_minmax` covers whole 256-frame base bins overlapping `[start,end)` (peaks.rs:135-146), so loud source material adjacent to (but outside) the clip's edge stops the scan at that edge.

8. **note — cosmetic `set_clip_gain` guard.** `E` (silence, gain 0) then `i` (invert): status "c0 is already inverted (polarity flipped)" and nothing logged — true but misleading.

## Verified
`source_frame_at` reversed mapping at both ends; mirror arithmetic of all four ops checked by hand (split left=top/right=bottom, trim start keeps `src_start`, trim end sign flips incl. extend-past-zero refusal, chop walks down) for extending trims, 1-frame clips and a region at source frame 0; `reverse`→split→`reverse`-each-half and `reverse`→trim→`reverse` concatenate back to the original audio; `reversed` preserved on Duplicate/MoveClip/MoveClipToTrack/SetClipFade/RazorSplit/ChopClip; loop⇄reverse refusals both ways, looped+reversed unreachable in the panel; host parse strict arity 3 + `format_arrange` + round-trip test; clip_editor Reverse codec + `ALL_OPS`; workflow keys/`command_name`, no collisions; toggle replay determinism; normalize cap arithmetic, silent refusal, silence-then-normalize, gain finiteness; trim scan termination, fully-silent refusal, trail-0, one group = one undo, exact (lead,trail) swap for reversed. `cargo test --workspace` fully green.

## Not verified
- Host undo/redo of a `reverse` line end-to-end (reasoned deterministic; now covered by the paste test's one-undo assertion).
- Whether the audio *renderer* honors `reversed` beyond `source_frame_at` (no rendering consumer found in the diff).
- iced-shell: separate workspace; the reviewer first read its key translation as silently dropping the new keys — see the disposition (refuted with evidence).
