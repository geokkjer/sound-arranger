# Agent Note: UI revision — project/library split, the unified clip-as-loop model, and the relaxed CDP seam

Status: proposed

## Problem

The UI plan (`docs/design/ui-plan.md`) predates a working answer to three things that the Ableton
prior-art study (2026-09-09) surfaced, and the "Next" scope it assumes is stale on each:

1. **The source model is one flat, reference-addressed "pool."** There is no notion of a *reusable
   library* of loops/samples/shorter sounds that outlives a single project and is dragged into any
   timeline, versus a *project's own* recorded material. Treating "library" and "project" as the
   same object muddles the mine-then-reuse workflow.
2. **The clip model distinguishes "region" from "loop."** The plan's vocabulary (cut/split regions
   vs. loop regions) implies two kinds. The wanted behavior — an ACID-style "liquid" resize where
   everything is a loop and some loops simply play once — is a single model, not a split.
3. **CDP is locked out of the arranger, but the seam that would later admit it is unstated.** The
   umbrella-first decision keeps offline processing in the deferred sound-sculptor profile. That
   stands; but there was no design answer for *where* a future "CDP → new clip" would surface, so
   the UI could grow in a direction that later needs a rewrite.

This note owns the revision; the prior-art study owns the observations it is based on.

## Proposal

### 1. Source model — Project (copies) vs Library (referenced folder)

- **Library** = a folder the user points the app at, default `$XDG_MUSIC_DIR/Samples`
  (`~/Music/Samples`), overridable via a chooser and persisted as a preference. The app **indexes**
  it (reads files, names, loop metadata, tags) and the browser **watches the folder and re-indexes
  on change**. Files live where they are; the browser references them — the "reference, don't copy"
  behavior for the library.
- **Project** = **self-contained.** Using a library item **copies** it into the project; promoting a
  usable clip **copies** it into the library. Two symmetric, explicit actions: *promote to library*
  and *copy into project*. Every copy carries a **provenance association** (where it came from) as
  metadata only — never a live reference, so a project can never break if the library moves or a
  file is deleted.
- **Deferred but locked:** content-hash **dedup** so identical bytes are stored once. "Copy" is an
  ownership guarantee, not a requirement that bytes be duplicated. (v1 = plain copies; dedup is the
  lever when storage becomes a problem.)

### 2. Clip model — everything is a loop

A clip is one type: `source · loop_in · loop_out · fill_rule (loop | trim) · mode (loop | once) ·
length · gain · fades`. There is no "region" type — a region is a loop with `mode = once`.

- **Edge drag → `length`.** What fills the span is the **fill rule**: `loop` repeats the loop span;
  `trim` shows more/less of the source. The "liquid" feel is the fill being continuous.
- **Loop-span handles → `loop_in`/`loop_out` in the source.** A separate drag adjusts which part of
  the source loops, independent of the edge.
- **Time-warp/stretch — deferred but locked as a feature request.** v1 ships `loop` and `trim` fill
  rules only; the `stretch` fill rule (and the source tempo/pitch metadata it needs) is a locked
  future feature, not built now. We are deliberately moving in the right direction, not shipping it
  this release.

### 3. Detail View — bottom, contextual on focus

The bottom slot is the **Detail View**, keyed to what has focus, and it **collapses** when nothing
is focused (the timeline stays the hero):

| Focus | Detail View shows |
|---|---|
| A clip/region in the timeline | **Region editor** — gain, fades, fill rule, mode, loop points; **+ "Process offline →"** (the future CDP slot) |
| A track (strip, header, lane) | **Track effect-chain editor** — real-time `fundsp` nodes, order, in/out, sends |
| A library/loop item being previewed | **Source editor** — loop points, mode, gain, audition |

This **replaces the "Clip Inspector (overlay)"** idea from the earlier plan, and it **frees the
bottom slot from transport/undo** — transport and undo/redo move to the top bar.

### 4. CDP — offline chain → new source, out of arranger v1

- CDP stays **out of the clip-arranger** (the umbrella-first / sound-sculptor decision stands). But
  the **seam is reserved now** so it slots in later without a re-architecture.
- When it arrives, it is a **region-level, offline render**: "Process offline" on a clip opens a
  **chain of CDP devices** (the superpower is chaining, not one program), each a macro-mapped device;
  rendering produces a **new clip/source**; the original is untouched. CDP is **never a realtime
  graph node**.
- The heavy lift is **per-program parameter maps**, not UI: ship a **curated subset** of programs,
  each with a hand-authored `macro → arg` map (ranges normalized to 0–1), exposed through the same
  macro-knob interaction as `fundsp`. Not ~800 programs, not ad-hoc UI.

### 5. Mixer — toggleable and resizable

The mixer is not permanently on. It is **toggleable** and, when shown, **resizable within the slot
range** like the rest of the chrome. Rationale: the profile is record → arrange → mix; the edit is
the hero, so the mixer is brought in for the pass that needs it.

### 6. Per-track color

A consistent `--track-N` hue is assigned per track and reused across the **timeline lane header**,
the **timeline clips** on that lane, the **mixer strip**, and the **track header**. Add the palette
to the design system; the user can override per track.

### 7. Track/lane structure resolution

The earlier plan's separate "Track/Layer column" slot is **dropped**; **tracks are the timeline
lanes**, each lane carrying a header strip (name, color, mute, record-arm). Tracks map 1:1 to mixer
channels via the shared per-track color. Takes/comping is recorded as an open question, not a v1
decision.

## Alternatives considered

- **Content-hash single-store reference model** (Option A in discussion). Rejected: a project built
  on live references can break if a source moves or is deleted; **copy semantics** win on
  robustness. Dedup is deferred, so reference-addressing only returns as a *storage* optimization
  under an ownership guarantee.
- **Region vs loop as two clip types.** Rejected: everything is a loop; `mode` is the only
  difference, and splitting types leaks into every interaction (resize, fill, snap).
- **CDP as a realtime graph node.** Rejected: CDP is offline/non-realtime. A live node would force
  sample-accurate semantics it cannot meet. Render-to-new-source keeps it off the realtime path and
  non-destructive.
- **Clip Inspector as a transient overlay.** Rejected: the thing you edit appears constantly in an
  edit-as-composition workflow; a persistent contextual Detail View is closer to the actual beat and
  avoids fighting the hero timeline.
- **Mixer always-visible (right column).** Rejected for the profile: mixing is a pass, not the
  default state; toggle-in keeps the timeline rich.
- **App-managed library store.** Rejected: prefer a referenced user folder (XDG default, user
  choice), which respects existing sample files and the separation-of-concerns rule instead of
  copying the library into app data.
- **Time-warp/stretch now.** Rejected: the source tempo/pitch metadata and the stretch DSP are a
  real step; deferred but locked as a feature request so direction is fixed.

## Acceptance criteria

- The default `sound-arranger` profile renders the four slots — **Source Pool** (Project | Library, left), **Timeline** (fill), **Mixer** (right, toggle + resize), **Detail View** (bottom, contextual) — with transport and undo/redo on the **top bar** and none in the bottom slot.
- A clip is one type with `mode` (loop/once) and a fill rule (`loop`/`trim`); there is no separate "region" type. Dragging a clip edge changes `length` per the fill rule; a separate loop-span handle edits `loop_in`/`loop_out`. (`stretch` is deferred but locked.)
- Dragging a library item into the timeline copies it into the project and records a **provenance association**; promoting a clip copies it into the configured library folder. Deleting a library file does not break a project.
- The library default path is `$XDG_MUSIC_DIR/Samples` (`~/Music/Samples`), overridable via a preference.
- The library browser re-indexes when the library folder changes (watch, not index-on-open).
- A track's `--track-N` hue is identical across its lane header, its clips, its mixer strip, and its track header.
- The Detail View collapses when nothing is focused and shows region / track effect-chain / source editor per the focus table above.

## Consequences

- The profile layout becomes: **top bar** (brand, transport, tempo, undo/redo, snap, zoom-fit,
  profile switch) · **left** = Source Pool (Project \| Library) · **fill** = Timeline (lane-tracked
  clips) · **right** = Mixer (toggle) · **bottom** = Detail View (contextual).
- Sources carry **loop-point metadata** (loop_in/out, mode, and later tempo/pitch) that the current
  float-WAV pool sources do not — a source-schema addition to schedule.
- The timeline canvas interaction spec gains **edge-drag resize** and **loop-span handles**, both
  snap-aware, on top of the existing snap/marquee/move model.
- The `docs/design/ui-plan.md` profile section, `docs/design/design-system.md` (per-track colors),
  and `RESEARCH.md` (link) are updated to match.

## Risks

- **Detail View contention.** Region editor, effect chain, and source editor share one slot; the
  focus rules must be unambiguous or the panel feels arbitrary. Mitigation: focus = last-interacted
  object; deterministic fallback when focus is ambiguous.
- **Copy semantics storage growth.** Dedup is deferred; early projects with many copied loops will
  duplicate bytes. Mitigation: accepted for now; content-hash dedup is the locked later lever.
- **CDP seam inferred, not built.** We are reserving a seam for a deferred feature; if the sound
  sculptor profile later wants a different surface, the seam is advisory, not binding.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-09.
