// The timeline editor's pure logic — hit-testing a pointer against the
// arrangement value, snapping a drag, and building the text-format op line for a
// gesture. The canvas owns the pixels and the gestures; this module owns the
// decisions, so they are unit-tested (see timelineEdit.test.ts).

import type { Clip, Timeline } from "./bridge";
import { tickStepFrames } from "./timelineTicks";

/** A pointer's grip on a clip: its body (move) or an edge (resize). */
export interface Hit {
  /** track index */
  track: number;
  /** clip index within the track */
  clip: number;
  zone: "body" | "start" | "end";
}

/** Grab an edge when the pointer is within this many pixels of it. */
export const EDGE_GRAB_PX = 6;
/** Snap a drag when a candidate frame is within this many pixels of a target. */
export const SNAP_PX = 8;

/** The clip a hit refers to, or undefined. */
export function clipAt(tl: Timeline, hit: Hit): Clip | undefined {
  return tl.tracks[hit.track]?.clips[hit.clip];
}

/** The track index under a viewport y, or null in the ruler / past the lanes. */
export function laneAt(
  y: number,
  trackCount: number,
  rulerH: number,
  laneH: number,
  vScroll: number,
): number | null {
  const local = y - rulerH + vScroll;
  if (local < 0 || laneH <= 0) return null;
  const ti = Math.floor(local / laneH);
  return ti >= 0 && ti < trackCount ? ti : null;
}

/**
 * What the pointer at `frame` on `trackIndex` is over — the last clip wins when
 * clips overlap, and the edge grab width is a pixel width converted by `zoom`
 * (so it stays ~6 px however far you zoom).
 */
export function hitTest(tl: Timeline, trackIndex: number, frame: number, zoom: number): Hit | null {
  const track = tl.tracks[trackIndex];
  if (!track) return null;
  const grabFrames = EDGE_GRAB_PX / zoom;
  for (let ci = track.clips.length - 1; ci >= 0; ci--) {
    const clip = track.clips[ci];
    const end = clip.at_frame + clip.src_len;
    if (frame < clip.at_frame || frame > end) continue;
    if (frame - clip.at_frame <= grabFrames) return { track: trackIndex, clip: ci, zone: "start" };
    if (end - frame <= grabFrames) return { track: trackIndex, clip: ci, zone: "end" };
    return { track: trackIndex, clip: ci, zone: "body" };
  }
  return null;
}

/** The snap context for a drag: the grid step plus explicit edge targets. */
export interface SnapContext {
  /** the grid step in frames (the ruler's step, so ticks and snaps agree) */
  step: number;
  /** explicit targets: 0 and every *other* clip's edges */
  targets: number[];
  /** the tolerance in frames */
  tolerance: number;
}

/** Build the snap context: the grid step plus every clip edge except `exclude`. */
export function snapContext(tl: Timeline, zoom: number, exclude?: Hit): SnapContext {
  const targets: number[] = [0];
  tl.tracks.forEach((track, ti) =>
    track.clips.forEach((clip, ci) => {
      if (exclude && exclude.track === ti && exclude.clip === ci) return;
      targets.push(clip.at_frame, clip.at_frame + clip.src_len);
    }),
  );
  return { step: tickStepFrames(zoom), targets, tolerance: SNAP_PX / zoom };
}

/** The nearest grid tick or explicit target within tolerance, or null. */
function nearestTarget(frame: number, ctx: SnapContext): { frame: number; dist: number } | null {
  let best: { frame: number; dist: number } | null = null;
  const consider = (target: number) => {
    const dist = Math.abs(target - frame);
    if (dist <= ctx.tolerance && (!best || dist < best.dist)) best = { frame: target, dist };
  };
  consider(Math.round(frame / ctx.step) * ctx.step);
  for (const target of ctx.targets) consider(target);
  return best;
}

/** Snap a single frame (a razor cut, a playhead drop) to the nearest target. */
export function snapFrame(frame: number, ctx: SnapContext): number {
  return nearestTarget(frame, ctx)?.frame ?? frame;
}

/**
 * Snap a clip **move**: either edge may land on a target, so take whichever edge
 * is nearer one (and leave the clip alone when neither is close).
 */
export function snapMove(length: number, desiredStart: number, ctx: SnapContext): number {
  const byStart = nearestTarget(desiredStart, ctx);
  const byEnd = nearestTarget(desiredStart + length, ctx);
  if (!byStart) return byEnd ? byEnd.frame - length : desiredStart;
  if (!byEnd || byStart.dist <= byEnd.dist) return byStart.frame;
  return byEnd.frame - length;
}

/** `count` fresh clip ids from `base` (`base.1`, `base.2`, …), skipping used ones. */
export function freshIds(tl: Timeline, base: string, count: number): string[] {
  const used = new Set<string>();
  for (const track of tl.tracks) for (const clip of track.clips) used.add(clip.id);
  const out: string[] = [];
  for (let k = 1; out.length < count; k++) {
    const id = `${base}.${k}`;
    if (!used.has(id)) {
      out.push(id);
      used.add(id);
    }
  }
  return out;
}

/**
 * The longest a clip may run: the source's frame count less its start frame. The
 * timeline value does not know a source's length (only the pool does), so the
 * caller passes it when available — otherwise the resize is unbounded.
 */
export function maxSrcLen(sourceFrames: number | undefined, srcStart: number): number | undefined {
  if (sourceFrames === undefined) return undefined;
  return Math.max(1, sourceFrames - srcStart);
}

// ---- op builders (the text grammar the bridge parses) ----------------------
// Ids are plain words here; the bridge validates them (and the pool refuses a
// non-plain stem), so the builders do not sanitize.

export const opMove = (track: string, clip: string, at: number): string =>
  `move_clip ${track} ${clip} ${Math.round(at)}`;

export const opMoveToTrack = (from: string, clip: string, to: string, at: number): string =>
  `move_clip_to_track ${from} ${clip} ${to} ${Math.round(at)}`;

export const opTrimEnd = (track: string, clip: string, byFrames: number): string =>
  `trim ${track} ${clip} end ${Math.round(byFrames)}`;

export const opTrimStart = (track: string, clip: string, byFrames: number): string =>
  `trim ${track} ${clip} start ${Math.round(byFrames)}`;

export const opRazorSplit = (
  track: string,
  clip: string,
  left: string,
  right: string,
  at: number,
): string => `razor_split ${track} ${clip} ${left} ${right} ${Math.round(at)}`;

export const opDelete = (track: string, clip: string): string => `delete ${track} ${clip}`;

export const opDuplicate = (track: string, clip: string, newId: string): string =>
  `duplicate ${track} ${clip} ${newId}`;
