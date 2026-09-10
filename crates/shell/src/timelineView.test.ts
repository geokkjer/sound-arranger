import { beforeEach, describe, expect, it } from "vitest";
import {
  MAX_PX_PER_SEC,
  RATE,
  clampView,
  fitTimeline,
  fitZoom,
  followPlayhead,
  frameAt,
  panBy,
  timelineView as view,
  xFor,
  zoomAt,
} from "./timelineView";

const MAX_ZOOM = MAX_PX_PER_SEC / RATE;

/** A known viewport: 1000 px wide, 10 s long, fitted. */
function reset(overrides: Partial<typeof view> = {}): void {
  view.width = 1000;
  view.height = 400;
  view.duration = 10 * RATE;
  view.zoom = 1000 / (10 * RATE); // fit
  view.t0 = 0;
  view.vScroll = 0;
  view.follow = true;
  Object.assign(view, overrides);
}

beforeEach(() => reset());

describe("the viewport mapping", () => {
  it("fitZoom makes the arrangement fill the width", () => {
    expect(fitZoom()).toBeCloseTo(1000 / (10 * RATE), 12);
  });

  it("frameAt and xFor are inverses", () => {
    reset({ zoom: 0.01, t0: 12_345 });
    for (const frame of [0, 1, 48_000, 123_456]) {
      expect(frameAt(xFor(frame))).toBeCloseTo(frame, 6);
    }
  });

  it("fitTimeline frames the whole arrangement (zoom = fit, t0 = 0)", () => {
    reset({ zoom: 0.05, t0: 40_000 });
    fitTimeline();
    expect(view.zoom).toBeCloseTo(fitZoom(), 12);
    expect(view.t0).toBe(0);
    expect(view.vScroll).toBe(0);
  });
});

describe("zoomAt", () => {
  it("keeps the frame under the cursor fixed", () => {
    const px = 400;
    const under = frameAt(px);
    zoomAt(px, 2);
    expect(xFor(under)).toBeCloseTo(px, 6);
  });

  it("is a hard floor at fit: you cannot zoom out past the arrangement", () => {
    zoomAt(500, 0.1);
    expect(view.zoom).toBeCloseTo(fitZoom(), 12);
  });

  it("caps at the pixels-per-second ceiling", () => {
    zoomAt(500, 1e6);
    expect(view.zoom).toBeCloseTo(MAX_ZOOM, 12);
  });

  it("never scrolls past an arrangement edge", () => {
    zoomAt(999, 100);
    const visible = view.width / view.zoom;
    expect(view.t0).toBeGreaterThanOrEqual(0);
    expect(view.t0).toBeLessThanOrEqual(Math.max(0, view.duration - visible) + 1e-6);
  });

  it("lets fit win when fit is above the ceiling (a tiny arrangement)", () => {
    reset({ duration: 100 }); // 100 frames in 1000 px → fit = 10 px/frame ≫ the ceiling
    clampView();
    expect(view.zoom).toBeCloseTo(10, 9); // fit, not the ceiling — no dead space
    expect(view.t0).toBe(0);
  });
});

describe("panBy and clampView", () => {
  it("pans by pixels ÷ zoom", () => {
    reset({ zoom: 0.01, t0: 1000 });
    panBy(100); // 100 px at 0.01 px/frame = 10 000 frames
    expect(view.t0).toBeCloseTo(11_000, 3);
  });

  it("clamps at both ends and never shows dead space", () => {
    reset({ zoom: 0.01 });
    panBy(-1e9);
    expect(view.t0).toBe(0);
    panBy(1e9);
    expect(view.t0).toBeCloseTo(view.duration - view.width / view.zoom, 6);
  });
});

describe("followPlayhead", () => {
  it("does nothing when follow is off", () => {
    reset({ follow: false, zoom: 0.01, t0: 0 });
    followPlayhead(400_000);
    expect(view.t0).toBe(0);
  });

  it("does nothing while the playhead is comfortably in view", () => {
    reset({ zoom: 0.01, t0: 0 });
    followPlayhead(10_000); // x = 100 px, inside the right margin (850)
    expect(view.t0).toBe(0);
  });

  it("parks the playhead at the left margin once it passes the right one", () => {
    reset({ zoom: 0.01, t0: 0 });
    const frame = 95_000;
    followPlayhead(frame);
    expect(xFor(frame)).toBeCloseTo(view.width * 0.15, 6);
  });

  it("recentres when the playhead is behind the view (a seek backwards)", () => {
    reset({ zoom: 0.01, t0: 50_000 });
    followPlayhead(20_000);
    expect(xFor(20_000)).toBeCloseTo(view.width * 0.15, 6);
  });
});
