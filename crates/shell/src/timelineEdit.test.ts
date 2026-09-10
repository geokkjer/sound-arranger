import { describe, expect, it } from "vitest";
import type { Clip, Timeline } from "./bridge";
import {
  EDGE_GRAB_PX,
  SNAP_PX,
  laneAt,
  clipAt,
  freshIds,
  hitTest,
  maxSrcLen,
  opDelete,
  opDuplicate,
  opMove,
  opMoveToTrack,
  opRazorSplit,
  opTrimEnd,
  opTrimStart,
  snapContext,
  snapFrame,
  snapMove,
  type SnapContext,
} from "./timelineEdit";

function clip(id: string, at: number, len: number): Clip {
  return { id, source: "s1", src_start: 0, src_len: len, at_frame: at, fade_in: 0, fade_out: 0, gain: 1, loop_len: null };
}

const tl: Timeline = {
  tracks: [
    { id: "t0", clips: [clip("a", 0, 48_000), clip("b", 96_000, 24_000)] },
    { id: "t1", clips: [clip("c", 12_000, 12_000)] },
  ],
};

const ctx = (step: number, targets: number[], tolerance: number): SnapContext => ({ step, targets, tolerance });

describe("laneAt", () => {
  it("resolves the lane under a y, and null in the ruler / past the lanes", () => {
    const laneH = 100;
    expect(laneAt(10, 2, 20, laneH, 0)).toBeNull(); // in the ruler
    expect(laneAt(20, 2, 20, laneH, 0)).toBe(0);
    expect(laneAt(20 + laneH, 2, 20, laneH, 0)).toBe(1);
    expect(laneAt(20 + 2 * laneH, 2, 20, laneH, 0)).toBeNull(); // past the last track
  });

  it("accounts for the vertical scroll", () => {
    const laneH = 100;
    expect(laneAt(20, 2, 20, laneH, laneH)).toBe(1); // scrolled down one lane
  });
});

describe("hitTest", () => {
  it("finds a clip's body, and its edges within the grab width", () => {
    expect(hitTest(tl, 0, 24_000, 1)?.zone).toBe("body");
    expect(hitTest(tl, 0, 3, 1)?.zone).toBe("start");
    expect(hitTest(tl, 0, 47_998, 1)?.zone).toBe("end");
    expect(hitTest(tl, 0, 48_000, 1)?.zone).toBe("end"); // the end handle is inclusive
  });

  it("returns null in a gap or on an empty track", () => {
    expect(hitTest(tl, 0, 50_000, 1)).toBeNull();
    expect(hitTest(tl, 1, 0, 1)).toBeNull();
    expect(hitTest(tl, 9, 0, 1)).toBeNull();
  });

  it("keeps the edge grab ~EDGE_GRAB_PX wide by scaling it with the zoom", () => {
    const zoom = 0.01; // 1 px = 100 frames, so the grab is EDGE_GRAB_PX * 100 frames
    expect(hitTest(tl, 0, EDGE_GRAB_PX / zoom - 1, zoom)?.zone).toBe("start");
    expect(hitTest(tl, 0, EDGE_GRAB_PX / zoom + 1, zoom)?.zone).toBe("body");
  });

  it("reports the clip it hit", () => {
    expect(clipAt(tl, hitTest(tl, 0, 100_000, 1)!)?.id).toBe("b");
  });
});

describe("snapping", () => {
  it("snaps a frame to the nearest grid tick", () => {
    expect(snapFrame(47_995, ctx(48_000, [], SNAP_PX))).toBe(48_000);
  });

  it("snaps a frame to an explicit target (a clip edge)", () => {
    expect(snapFrame(99_996, ctx(1e9, [100_000], SNAP_PX))).toBe(100_000);
  });

  it("leaves a frame alone when nothing is within tolerance", () => {
    expect(snapFrame(20_000, ctx(48_000, [], SNAP_PX))).toBe(20_000);
  });

  it("snaps a move by whichever edge is nearer a target", () => {
    // the end butts onto b's start at 96 000
    expect(snapMove(24_000, 71_998, ctx(1e9, [96_000], SNAP_PX))).toBe(72_000);
    // the start lands on the grid
    expect(snapMove(24_000, 47_995, ctx(48_000, [], SNAP_PX))).toBe(48_000);
    // nothing near → unmoved
    expect(snapMove(24_000, 20_000, ctx(48_000, [], SNAP_PX))).toBe(20_000);
  });

  it("builds a context from the grid step plus every other clip's edges", () => {
    const c = snapContext(tl, 1, { track: 0, clip: 0, zone: "body" });
    expect(c.tolerance).toBe(SNAP_PX);
    expect(c.targets).toContain(0);
    expect(c.targets).toContain(96_000); // clip b's start
    expect(c.targets).toContain(120_000); // clip b's end
    expect(c.targets).toContain(12_000); // clip c (another track)
    expect(c.targets).not.toContain(48_000); // the dragged clip's own end is excluded
  });
});

describe("freshIds", () => {
  it("returns unused ids from a base", () => {
    expect(freshIds(tl, "x", 2)).toEqual(["x.1", "x.2"]);
  });

  it("skips ids already in the arrangement", () => {
    const withOne: Timeline = { tracks: [{ id: "t0", clips: [clip("a.1", 0, 1000)] }] };
    expect(freshIds(withOne, "a", 2)).toEqual(["a.2", "a.3"]);
  });
});

describe("maxSrcLen", () => {
  it("bounds a resize by the source's length (when known)", () => {
    expect(maxSrcLen(48_000, 1_000)).toBe(47_000);
    expect(maxSrcLen(undefined, 0)).toBeUndefined();
  });
});

describe("op builders", () => {
  it("emit the text grammar", () => {
    expect(opMove("t0", "c0", 9600.4)).toBe("move_clip t0 c0 9600");
    expect(opMoveToTrack("t0", "c0", "t1", 0)).toBe("move_clip_to_track t0 c0 t1 0");
    expect(opTrimEnd("t0", "c0", -24_000)).toBe("trim t0 c0 end -24000");
    expect(opTrimStart("t0", "c0", 4_800)).toBe("trim t0 c0 start 4800");
    expect(opRazorSplit("t0", "c0", "c0.1", "c0.2", 24_000)).toBe("razor_split t0 c0 c0.1 c0.2 24000");
    expect(opDelete("t0", "c0")).toBe("delete t0 c0");
    expect(opDuplicate("t0", "c0", "c0.1")).toBe("duplicate t0 c0 c0.1");
  });
});
