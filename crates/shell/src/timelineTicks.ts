// The timeline ruler's tick maths — pure, and extracted from the canvas so it is
// unit-tested. The step is the smallest "nice" interval whose labels have at
// least `MIN_TICK_PX` of room, which is also the step a snap grid would use.

import { RATE } from "./timelineView";

/** A tick's label needs at least this much room on screen. */
export const MIN_TICK_PX = 64;
/** "Nice" tick steps, in seconds. */
export const NICE_SECONDS = [0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];

/** The smallest "nice" tick step (in frames) whose labels are ≥ `MIN_TICK_PX` apart. */
export function tickStepFrames(zoomPxPerFrame: number): number {
  const pxPerSec = zoomPxPerFrame * RATE;
  const sec = NICE_SECONDS.find((s) => s * pxPerSec >= MIN_TICK_PX) ?? NICE_SECONDS[NICE_SECONDS.length - 1];
  return sec * RATE;
}

/** A ruler label for an absolute frame: `0.25s`, `1.5s`, `2s`, `1:30`. */
export function tickLabel(frame: number): string {
  const s = frame / RATE;
  if (s >= 60) {
    const m = Math.floor(s / 60);
    return `${m}:${Math.round(s - m * 60).toString().padStart(2, "0")}`;
  }
  return s < 1 ? `${s.toFixed(2)}s` : `${Number.isInteger(s) ? s : s.toFixed(1)}s`;
}
