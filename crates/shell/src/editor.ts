import { invoke } from "@tauri-apps/api/core";
import {
  bridgeState,
  type MixerMeters,
  type PoolSource,
  type Timeline,
  type TransportPosition,
} from "./bridge";
import { pollTransport } from "./transport";

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
  /** Whether an edit can be undone / redone (the top bar's ⟲/⟳ buttons). */
  edit: { can_undo: boolean; can_redo: boolean };
}

/** Fold a bridge outcome into the shared bridge state. */
export function foldOutcome(outcome: ScriptOutcome): void {
  bridgeState.meters = outcome.mixer_meters;
  bridgeState.sources = outcome.pool_sources ?? [];
  bridgeState.arrangement = outcome.arrangement;
  bridgeState.position = outcome.position;
  bridgeState.channelCount = outcome.channel_count;
  bridgeState.lastError = outcome.arrangement_error;
  bridgeState.canUndo = outcome.edit.can_undo;
  bridgeState.canRedo = outcome.edit.can_redo;
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

/** Undo the most recent arrangement edit (the host rebuilds to the same frame). */
export async function undoEdit(): Promise<void> {
  await invoke("edit_undo");
  await pollTransport();
}

/** Redo the most recently undone edit. */
export async function redoEdit(): Promise<void> {
  await invoke("edit_redo");
  await pollTransport();
}
