import { reactive } from "vue";

// One shared bridge snapshot, read by the shell and the ui-plugins. The
// timeline canvas is the only writer (running the host script lifts the
// resulting meters / pool-sources / arrangement here); the source-pool and
// mixer panels read it. Keeping this in one reactive store means the slot
// renderer needs no per-view prop plumbing — plugins share state through a
// declared slice, not through the shell.
//
// These mirror the Host-API bridge's `ScriptOutcome` (serde snake_case) in
// `src-tauri/src/lib.rs`.

export interface MixerMeters {
  channels: number[];
  master: number;
}

export interface PoolSource {
  id: string;
  wav?: string;
  peaks?: string;
  frames: number;
  sample_rate: number;
  peaks_missing: boolean;
  finalized: boolean;
}

export interface Clip {
  id: string;
  source: string;
  src_start: number;
  src_len: number;
  at_frame: number;
  fade_in: number;
  fade_out: number;
  gain: number;
  loop_len: number | null;
}

export interface Track {
  id: string;
  clips: Clip[];
}

export interface Timeline {
  tracks: Track[];
}

/** The live transport position (mirrors the bridge's `TransportPosition`). */
export interface TransportPosition {
  frame: number;
  seconds: number;
  beat: number;
  bpm: number;
  playing: boolean;
}

export const bridgeState = reactive({
  /** human-readable transport/run status shown in the top bar. */
  status: "showing demo arrangement",
  /** the mixer meter snapshot (drives the mixer panel), if a mixer is mounted. */
  meters: null as MixerMeters | null,
  /** the media-pool source listing (drives the source pool), if a pool is set. */
  sources: [] as PoolSource[],
  /** the arrangement value (drives the timeline canvas), if the host produced one. */
  arrangement: null as Timeline | null,
  /** the live transport position, polled from the bridge (drives the playhead). */
  position: { frame: 0, seconds: 0, beat: 0, bpm: 120, playing: false } as TransportPosition,
  /** the mixer's mounted channel count (drives the strip count). */
  channelCount: 0,
  /** the pump's last unrecoverable error, if any (surfaced in the top bar). */
  lastError: null as string | null,
});

/** [dB → 0..1] meter fill, clamped to [-60, 0] dB (matches the mixer panel). */
export function meterFill(p: number): number {
  return Math.min(1, Math.max(0, (20 * Math.log10(Math.max(p, 1e-9)) + 60) / 60));
}
