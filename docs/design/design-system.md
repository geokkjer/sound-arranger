# sound-arranger — Design System (the `DESIGN.md` contract)

> **Status: proposed (visual contract for the UI plan).** This is the engineer/minimalist contract. The shape/tradeoffs are in [`ui-plan.md`](ui-plan.md); the wireframes are in [`mocks/`](mocks/). Reference webview: **WebKitGTK 2.52.6** (`webkit2gtk-4.1`).

The design language is deliberately quiet: **flat surfaces, hairline borders, precise typography, restrained accent, generous negative space.** Nothing decorative. The chrome should feel like the housing of an instrument, not a marketing page. And — by design — *every rule here is also a performance decision* (less compositing, fewer layers).

---

## Palette (dark, studio-neutral)

```css
:root {
  /* surfaces — near-black, cool */
  --bg:        #0f1114;   /* app/shadowed backdrop */
  --surface:   #14171b;   /* the workspace surface */
  --panel:     #1a1e24;   /* side panels */
  --raised:    #21262e;   /* elevated: tooltips, inspectors, popovers */
  --hover:     #242a32;   /* hover fill */

  /* hairlines */
  --line:      #2b313a;   /* active border/hairline */
  --line-soft: #22272e;   /* gutters, between lanes */

  /* text */
  --fg:        #dfe3e8;   /* primary */
  --fg-dim:    #9aa3ad;   /* secondary */
  --fg-mute:   #5f6872;   /* disabled/rest */

  /* accent & semantics */
  --accent:      #6b8afd;  /* selection / active (soft blue) */
  --accent-dim:  #3b4a7c;  /* selected-panel tint */
  --playhead:    #e5b567;  /* amber — play position */
  --record:      #e06a76;  /* red — armed/recording */
  --ok:          #7fc9a0;  /* green — meter normal */
  --warn:        #d9a13f;  /* amber — meter near-clip */

  /* metrics */
  --bar-h: 44px;
  --ruler-h: 24px;
  --radius: 5px;
  --radius-sm: 3px;
}
```

Apply the accent with **restraint**: it marks *selection/active* only. Idle chrome is monochrome. A muted single-hue state (blue for selection, amber for playhead, red for record) is the whole color language.

---

## Typography

- **UI labels / controls:** system sans (`-apple-system, "Segoe UI", system-ui, "Helvetica Neue", sans-serif`). Medium (500) for interactive labels; regular (400) for captions.
- **Numerics / time / ruler / clip names / mixer values:** system **monospace** (`ui-monospace, "SF Mono", "Menlo", monospace`) — the engineer tell. Ruler, timecode, gain/dB, transport all mono.
- **Scale:** base 13px (UI), 12px captions, 14px emphasized. Timecode uses tabular figures (mono) so digits don't jitter.

## Spacing & geometry

- **4px grid.** 4/8/12/16/24/32.
- Top bar **44px**; ruler **24px**; track/lane row **32px**; side panel min **220px**, max **400px**.
- **1px hairlines** (`--line`); radius `5px` for panels, `3px` for chips/clips.

## Component rules

- **Flat. No elevation.** No `box-shadow`, no `backdrop-filter`, no `filter: blur`. Distinguish hierarchy with *surface tone* (`--panel` vs `--raised`) and hairlines, never shadows.
- **Border over shadow** for all separations and card edges.
- **Selection:** `--accent` 1px border + `--accent-dim` fill + `--fg` text. Clicked/active states use `--hover`.
- **Hover:** subtle `--hover` fill, no transform, no scale (cheap and quiet).
- **Buttons:** minimal; a primary action is an outlined/flat accent, not a filled brand button.

## Iconography

- Monochrome, hairline-stroke glyphs (or the smallest set of solid glyphs). No decorative icons. Use text glyphs from a mono font where it reads cleanly (▮ ▮ ▶ ⏺ ⟲ ⌗). Prefer literal characters over icon fonts.

## Motion

- **Short and functional.** 80–150ms `ease-out`, opacity/transform on hover/emphasis only.
- **Respect `prefers-reduced-motion`.**
- **Never animate audio-driven things in CSS** (playhead, meters, transport): they render on canvas and are driven by the frame/audio clock, so they never fall behind the render thread.

## Performance mandate (enforced by the aesthetic)

1. No compositor-heavy styles on large surfaces.
2. Anything that repaints at audio/drag rate → **Canvas 2D**, not DOM.
3. The top bar and ruler are **pixel-stable and cheap**; the transport clock paints a small region, never relayouts the bar.
4. Cap DOM node count in the chrome; the timeline/peaks/meters never scale with clip count.
5. Theme layer may use `oklch`/`color-mix()`/`@property` (WebKitGTK 2.52.6 supports them); keep the render loop plain.

## Semantics (what a color means)

| Color | Meaning | Where |
|---|---|---|
| `--accent` (blue) | selection / active | clip border, selected channel, active view |
| `--playhead` (amber) | play position | timeline ruler + playhead line |
| `--record` (red) | armed / recording | record button, armed channel |
| `--ok` / `--warn` | meter level | mixer meters |
| `--fg-mute` | disabled | inactive clip, rest text |
