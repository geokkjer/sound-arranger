import { invoke } from "@tauri-apps/api/core";
import { bridgeState, type AudioInfo, type TransportPosition } from "./bridge";

// The live transport client. The host session runs on its own thread
// (`host::live`); the shell polls a shared snapshot (~25 Hz) to tick the playhead
// and meters, and drives play/stop/seek over the bridge. The UI never touches
// the render path — it only reads the published snapshot and sends commands.

interface WireTransportState {
  position: TransportPosition;
  channels: number[];
  channel_count: number;
  master: number;
  last_error: string | null;
  audio: AudioInfo | null;
  edit: { can_undo: boolean; can_redo: boolean };
}

/** Poll the host once and fold the result into the shared bridge state. */
export async function pollTransport(): Promise<void> {
  const st = await invoke<WireTransportState>("transport_state");
  bridgeState.position = st.position;
  bridgeState.channelCount = st.channel_count;
  bridgeState.meters = { channels: st.channels, master: st.master };
  bridgeState.lastError = st.last_error;
  bridgeState.audio = st.audio;
  bridgeState.canUndo = st.edit.can_undo;
  bridgeState.canRedo = st.edit.can_redo;
}

/**
 * Start the transport poll; returns a stop function. If the bridge is
 * unreachable (the UI is running outside Tauri, e.g. plain `pnpm dev` in a
 * browser), the poll stops itself after a few failures so it does not spin.
 */
export function startTransportPoll(): () => void {
  let timer: number | null = null;
  let failures = 0;
  const tick = async () => {
    try {
      await pollTransport();
      failures = 0;
    } catch {
      failures += 1;
      if (failures >= 3 && timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    }
  };
  void tick();
  timer = window.setInterval(tick, 40);
  return () => {
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  };
}

export function transportPlay(): Promise<void> {
  return invoke("transport_play");
}

export function transportStop(): Promise<void> {
  return invoke("transport_stop");
}

export function transportSeek(frame: number): Promise<void> {
  return invoke("transport_seek", { frame });
}

/** Format a seconds position as `M:SS.mmm` for the transport readout. */
export function formatTime(seconds: number): string {
  const s = Math.max(0, seconds);
  const m = Math.floor(s / 60);
  const rem = s - m * 60;
  return `${m}:${rem.toFixed(3).padStart(6, "0")}`;
}
