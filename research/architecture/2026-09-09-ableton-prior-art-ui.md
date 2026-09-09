# Ableton Live — prior art for the sound-arranger UI (Session vs Arrangement framing)

Status: research / prior-art study (2026-09-09). Observations only — the decisions this informs live in the
[UI revision note](../../.agents/notes/proposed/architecture/2026-09-09-ui-revision-project-library-and-clip-model.md).

Reference material: two public Ableton Live 12 screenshots reviewed on 2026-09-09.

---

## The framing that matters: Session vs Arrangement

The two screenshots are **different views with opposite purposes**:

- **Session view** — a live-performance grid: rows of launchable clip cells, track headers, a
  console mixer and large device panels along the bottom. Its purpose is *trigger things live, in
  the moment*. **This is the wrong half for us.** We are the inverse: record, then leave "live"
  and enter the edit.
- **Arrangement view** — the linear timeline: a ruler, per-track lanes holding clips with loop
  brackets and take labels, a console mixer, and a contextual detail panel. **This is the half
  that parallels the clip-arranger.**

So the honest comparison is against the **Arrangement view**, and the single thing unique to the
Session view (the clip-grid and scene-launch cell) is the thing to **reject**, not adapt. Session
view's density is a *performance* virtue; for an edit-as-composition editor the timeline is the hero and
recedes nothing.

## Borrowed elements and why

| Ableton element | Observation | Disposition for us |
|---|---|---|
| **Browser / library panel** | Nav strip, category + tag filter chips, a search box, a results list with thumbnails that previews in place, drag-to-place. | **Borrow (top priority).** For us the "library" is not presets — it is recorded/looped material and sample files. Needs `Cmd+F` search, tag filter chips, in-place audition, drag-to-timeline. Drive the project/library split decision (see the note). |
| **Arrangement ruler + loop bracket** | Bar/beat ruler; a highlighted loop bracket to audition a span. | **Borrow.** A loop bracket for auditioning a section before committing is cheap and fits edit-as-composition. |
| **Timeline loop / edge handles ("liquid" drag)** | Clips carry a loop bracket and grab handles; dragging an edge extends length while the loop fills the span. | **Borrow (the "everything is a loop" model).** Edge-drag edits length; loop-span handles edit the loop points. See the clip model in the note. |
| **Mixer strips / per-track color** | Strips share a hue with their timeline clips and track headers; M/S/gain/pan, dB and pan readouts, green/yellow/red meter zones. | **Borrow.** A consistent per-track color across lane, clip, and strip is the cheapest legibility win for many material pieces. Add the `--track-N` palette to the design system. |
| **Contextual Detail View + rack/macro knobs** | The bottom panel is contextual on the selection (a synth editor, an effect rack, or clip properties). The **Audio Effect Rack** exposes a curated set of inner params as **macro knobs** with a **Map** action. | **Borrow (the key one).** Contextual-on-focus, persistent bottom slot. The **macro-knob map** is the established pattern for taming a complex device — and the answer to exposing a curated set of params of a big program set like CDP. |
| **Comping / takes** | Clips labelled "Take 4 / Take 2 / …" and a per-section take choice across passes. | **Noted, not decided.** This may be a closer match to "record a lot, then mine the best bits" than cutting one long jam. Recorded as an open question. |
| **Session clip-grid / scene launch** | The live trigger surface. | **Reject.** Non-live; the one session-specific element. |
| **Per-synth wavetable / osc / LFO / MPE editors** | Very dense, animated, per-instrument editors. | **Reject for v1.** Our material is recorded audio, not synthesis. The Detail View should be clip/effect oriented, never a wavetable editor. Heavy and off-mandate (WebKit compositing). |

## The CDP read

The macro/rack pattern is the concrete answer to "CDP is a heavy lift because you must map and
expose parameters." Rather than expose ~800 CDP programs generically, wrap each program we ship as
a **device** with a hand-authored **parameter map** (`macro → program-arg`, ranges normalized to
0–1), and ship a **curated subset**. The chain editor then stacks real-time `fundsp` and offline
`OfflineProcess` devices with the **same** macro-knob interaction. Parameter exposure becomes
per-device metadata, not ad-hoc UI. **Offline** is the pivot: CDP is a render-to-new-source
operation, not a live graph node, so it never enters the realtime path.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-09.
