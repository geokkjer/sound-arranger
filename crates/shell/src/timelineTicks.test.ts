import { describe, expect, it } from "vitest";
import { MIN_TICK_PX, NICE_SECONDS, tickLabel, tickStepFrames } from "./timelineTicks";
import { RATE } from "./timelineView";

describe("tickStepFrames", () => {
  it("picks the smallest nice step whose labels are at least MIN_TICK_PX apart", () => {
    // 100 px/s: 0.5 s labels are 50 px (too tight), 1 s labels are 100 px.
    expect(tickStepFrames(100 / RATE)).toBe(RATE);
  });

  it("steps down to sub-second ticks when zoomed in", () => {
    // 1000 px/s: 0.05 s = 50 px (too tight), 0.1 s = 100 px.
    expect(tickStepFrames(1000 / RATE)).toBe(0.1 * RATE);
  });

  it("steps up to minutes when zoomed out", () => {
    // 1 px/s: 60 s = 60 px (too tight), 120 s = 120 px.
    expect(tickStepFrames(1 / RATE)).toBe(120 * RATE);
  });

  it("falls back to the largest step when nothing is far enough apart", () => {
    expect(tickStepFrames(1e-9)).toBe(NICE_SECONDS[NICE_SECONDS.length - 1] * RATE);
  });

  it("keeps labels at least MIN_TICK_PX apart wherever a nice step suffices", () => {
    for (const zoom of [0.0001, 0.001, 0.01, 0.1, 1]) {
      expect(tickStepFrames(zoom) * zoom).toBeGreaterThanOrEqual(MIN_TICK_PX - 1e-6);
    }
  });
});

describe("tickLabel", () => {
  it("labels sub-second ticks with two decimals", () => {
    expect(tickLabel(0)).toBe("0.00s");
    expect(tickLabel(0.25 * RATE)).toBe("0.25s");
  });

  it("labels whole seconds without decimals", () => {
    expect(tickLabel(RATE)).toBe("1s");
    expect(tickLabel(30 * RATE)).toBe("30s");
  });

  it("labels fractional seconds with one decimal", () => {
    expect(tickLabel(1.5 * RATE)).toBe("1.5s");
  });

  it("labels a minute and beyond as m:ss", () => {
    expect(tickLabel(60 * RATE)).toBe("1:00");
    expect(tickLabel(90 * RATE)).toBe("1:30");
  });
});
