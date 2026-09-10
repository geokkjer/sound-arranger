//! The live host runtime — a host thread (actor) that owns a [`HostSession`] and
//! pumps it in time.
//!
//! [`HostSession`] is `!Send`: the engine holds `Box<dyn FnOnce>`/`Box<dyn FnMut>`
//! disposers + op handlers and a `Box<dyn Any>` context, so the session cannot be
//! moved into `thread::spawn` nor stored in `tauri::State<Mutex<..>>` (the Tauri
//! review's F2 — a compile error, not a runtime hazard). The fix is an **actor**:
//! the session is *constructed on its own thread*, commands arrive over an `mpsc`
//! channel ([`HostCommand`] is `Send`), and a real-time pump advances the clock
//! while the transport runs. The shell polls a shared, `Send + Sync`
//! [`Snapshot`] (position + meters) and never touches the session.
//!
//! This is deliberately **host-crate only** (the review's "live runtime, still no
//! Tauri"): it is exercised headless by the tests below, and the Tauri bridge is a
//! thin adapter over [`HostHandle`]. Device audio output is a separate, later step
//! — the pump renders into the void today, but it is the same pump.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use engine::MIXER_CHANNELS_MAX;

use crate::{HostCommand, HostSession};

/// The published state a shell reads: transport position + the mixer meters.
/// Updated by the pump and by command handling; cheap to clone.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub frame: u64,
    pub seconds: f64,
    pub beat: f64,
    pub bpm: f64,
    pub playing: bool,
    /// Per-channel meter peaks (post-gain, pre-mute/solo). Only the first
    /// `channel_count` are meaningful; the rest are zero.
    pub channels: [f32; MIXER_CHANNELS_MAX],
    /// The mixer's mounted channel count (0 when no mixer is mounted).
    pub channel_count: usize,
    pub master: f32,
    /// The last error the pump hit while rendering (a wiring failure) — recorded
    /// so the shell surfaces it instead of a silently moving, silent playhead.
    pub last_error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            frame: 0,
            seconds: 0.0,
            beat: 0.0,
            bpm: 120.0,
            playing: false,
            channels: [0.0; MIXER_CHANNELS_MAX],
            channel_count: 0,
            master: 0.0,
            last_error: None,
        }
    }
}

/// A message to the host thread. `HostCommand` is `Send`, so the request is too.
enum Request {
    /// Apply a command; the result is sent back on the reply channel.
    Command(HostCommand, Sender<Result<(), String>>),
    Shutdown,
}

/// A `Send + Sync` handle to the live host thread.
pub struct HostHandle {
    tx: Mutex<Sender<Request>>,
    shared: Arc<Mutex<Snapshot>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl HostHandle {
    /// Spawn the host thread. The session is created **inside** the thread, so
    /// nothing `!Send` crosses the thread boundary.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let pump_shared = Arc::clone(&shared);
        let join = std::thread::spawn(move || run(HostSession::new(), rx, pump_shared));
        HostHandle {
            tx: Mutex::new(tx),
            shared,
            join: Mutex::new(Some(join)),
        }
    }

    /// Apply a command, blocking until the actor returns its result.
    pub fn execute(&self, cmd: HostCommand) -> Result<(), String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .lock()
            .map_err(|_| "host actor poisoned".to_string())?
            .send(Request::Command(cmd, reply_tx))
            .map_err(|_| "host actor exited".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "host actor exited".to_string())?
    }

    /// The latest published snapshot (position + meters).
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Stop the actor thread and join it. Idempotent.
    pub fn shutdown(&self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(Request::Shutdown);
        }
        if let Ok(mut join) = self.join.lock()
            && let Some(h) = join.take()
        {
            let _ = h.join();
        }
    }
}

impl Drop for HostHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The pump tick: how long the actor waits for a command before checking the
/// transport. ~4 ms keeps the playhead smooth without burning a core.
const TICK: Duration = Duration::from_millis(4);
/// The largest catch-up burst in one tick — caps the cost of catching up after a
/// stall.
const MAX_CHUNK: usize = 4096;

/// The actor loop: drain commands, then (while playing) render the frames the
/// wall clock says are due, and publish the snapshot.
fn run(mut session: HostSession, rx: Receiver<Request>, shared: Arc<Mutex<Snapshot>>) {
    let rate = session.sample_rate() as f64;
    // Wall-clock anchor for the playing clock: (wall, frame) at play/last change.
    let mut anchor: Option<(Instant, u64)> = None;
    publish(&session, &shared);
    loop {
        match rx.recv_timeout(TICK) {
            Ok(Request::Command(cmd, reply)) => {
                // Re-anchor on a transport change so the pump tracks from the new
                // frame instead of jumping by the wall-clock delta since play.
                let reanchor = matches!(
                    cmd,
                    HostCommand::TransportPlay
                        | HostCommand::TransportStop
                        | HostCommand::TransportSeek { .. }
                );
                let r = session.execute(&cmd);
                if reanchor {
                    anchor = None;
                }
                let _ = reply.send(r);
                publish(&session, &shared);
            }
            Ok(Request::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }

        if session.is_playing() {
            let (wall0, frame0) = *anchor
                .get_or_insert_with(|| (Instant::now(), session.position().frame));
            let target = frame0 + (wall0.elapsed().as_secs_f64() * rate) as u64;
            let now = session.position().frame;
            if target > now {
                let want = (target - now).min(MAX_CHUNK as u64) as usize;
                if let Err(e) = session.render(want) {
                    // A wiring failure must not silently yield a moving, silent
                    // playhead: record it and stop the transport.
                    let _ = session.execute(&HostCommand::TransportStop);
                    anchor = None;
                    if let Ok(mut s) = shared.lock() {
                        s.last_error = Some(e);
                    }
                }
            }
            publish(&session, &shared);
        }
    }
}

/// Publish the session's current position + meters into the shared snapshot.
fn publish(session: &HostSession, shared: &Mutex<Snapshot>) {
    let p = session.position();
    let Ok(mut s) = shared.lock() else { return };
    s.frame = p.frame;
    s.seconds = p.seconds;
    s.beat = p.beat;
    s.bpm = p.bpm;
    s.playing = p.playing;
    s.channel_count = session.mixer_channels().unwrap_or(0);
    match session.meters() {
        Some(bank) => {
            for k in 0..MIXER_CHANNELS_MAX {
                s.channels[k] = bank.channel_peak(k);
            }
            s.master = bank.master_peak();
        }
        None => {
            s.channels = [0.0; MIXER_CHANNELS_MAX];
            s.master = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pump advances the clock while playing and holds it still when stopped.
    #[test]
    fn live_host_plays_advances_and_stops() {
        let host = HostHandle::spawn();
        assert!(!host.snapshot().playing, "a fresh host is stopped");

        host.execute(HostCommand::TransportPlay).expect("play");
        std::thread::sleep(Duration::from_millis(150));
        let s = host.snapshot();
        assert!(s.playing, "the transport is running");
        assert!(s.frame > 0, "the pump advanced the clock (frame={})", s.frame);

        host.execute(HostCommand::TransportStop).expect("stop");
        std::thread::sleep(Duration::from_millis(80));
        let stopped = host.snapshot().frame;
        assert!(!host.snapshot().playing, "the transport stopped");
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(host.snapshot().frame, stopped, "the clock is still while stopped");

        host.shutdown();
    }

    /// Commands are applied by the actor and their effect is published; a refused
    /// command returns `Err` and leaves the host usable.
    #[test]
    fn live_host_executes_commands_and_refusals_survive() {
        let host = HostHandle::spawn();
        host.execute(HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        assert_eq!(host.snapshot().channel_count, 2, "the mixer channel count is published");

        // The tone plugin is not mounted → the param is refused, but the host
        // (and its session) survive and keep serving commands.
        let err = host
            .execute(HostCommand::SetParam {
                plugin: "tone",
                param: "gain",
                value: 1.0,
                at_frame: None,
            })
            .expect_err("an unmounted plugin's param is refused");
        assert!(err.contains("tone"), "the refusal names the plugin: {err}");
        host.execute(HostCommand::TransportStop).expect("host still usable");

        host.shutdown();
    }
}
