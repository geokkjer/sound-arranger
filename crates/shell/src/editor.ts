import { invoke } from "@tauri-apps/api/core";
import {
  bridgeState,
  type MixerMeters,
  type PoolSource,
  type Timeline,
  type TransportPosition,
} from "./bridge";

// The client side of the bridge: load a session, and apply ONE arrange op per
// gesture. Both return the same outcome and fold it into the shared state, so the
// timeline, mixer and source pool all see the result without a second round-trip.

/** The wire outcome `run_host_script` and `arrange` both return. */
export interface ScriptOutcome {
  arrangement: Timeline | null;
  arrangement_error: string | null;
  mixer_meters: MixerMeters | null;
  pool_sources: PoolSource[] | null;
  position: TransportPosition;
  channel_count: number;
}

/** Fold a bridge outcome into the shared bridge state. */
export function foldOutcome(outcome: ScriptOutcome): void {
  bridgeState.meters = outcome.mixer_meters;
  bridgeState.sources = outcome.pool_sources ?? [];
  bridgeState.arrangement = outcome.arrangement;
  bridgeState.position = outcome.position;
  bridgeState.channelCount = outcome.channel_count;
  bridgeState.lastError = outcome.arrangement_error;
}

/** Load a host script into the live session (reset-and-apply). */
export async function loadScript(scriptText: string): Promise<ScriptOutcome> {
  const outcome = await invoke<ScriptOutcome>("run_host_script", { scriptText });
  foldOutcome(outcome);
  return outcome;
}

/** Apply one arrange op line (the text grammar) — one gesture, one op. */
export async function applyArrange(line: string): Promise<ScriptOutcome> {
  const outcome = await invoke<ScriptOutcome>("arrange", { line });
  foldOutcome(outcome);
  return outcome;
}
