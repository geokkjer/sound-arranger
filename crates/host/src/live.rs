//! The live host runtime — a host thread (actor) that owns a [`HostSession`] and
//! pumps it in time, into a real audio output when one is available.
//!
//! [`HostSession`] is `!Send`: the engine holds `Box<dyn FnOnce>`/`Box<dyn FnMut>`
//! disposers + op handlers and a `Box<dyn Any>` context, so the session cannot be
//! moved into `thread::spawn` nor stored in `tauri::State<Mutex<..>>` (the Tauri
//! review's F2 — a compile error, not a runtime hazard). The fix is an **actor**:
//! the session is *constructed on its own thread*, commands arrive over an `mpsc`
//! channel ([`HostCommand`] is `Send`), and a real-time pump advances the clock
//! while the transport runs. The shell polls a shared, `Send + Sync`
//! [`Snapshot`] (position + meters + audio) and never touches the session.
//!
//! Audio: [`HostHandle::spawn_with_audio`] opens the default output device
//! (negotiated to the session rate — see `media::devices`) and the pump fills a
//! stereo ring the device callback drains, so the pump is paced by the **device**
//! clock rather than the wall clock. The cpal stream is `!Send`, so it is created
//! and kept alive here, on the actor thread. [`HostHandle::spawn`] stays silent
//! (headless tests, and the fallback when no device is available).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use engine::MIXER_CHANNELS_MAX;
use media::Spsc;
use media::devices::OutputHandle;

use crate::{HostCommand, HostSession};

/// The published state a shell reads: transport position + the mixer meters +
/// the audio state. Updated by the pump and by command handling; cheap to clone.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub frame: u64,
    pub seconds: f64,
    pub beat: f64,
    pub bpm: f64,
    pub playing: bool,
    /// The session's tempo/meter map, so a shell can quantize in the **beat**
    /// domain (grid snapping, a bar/beat ruler) against the same segments the
    /// engine plays — not a second copy that could drift. Cloned with the snapshot.
    pub tempo_map: engine::TempoMap,
    /// The session's sample rate (the engine clock's) — the frame side of the
    /// musical math, available even when no device was opened.
    pub sample_rate: u32,
    /// Per-channel meter peaks (post-gain, pre-mute/solo). Only the first
    /// `channel_count` are meaningful; the rest are zero.
    pub channels: [f32; MIXER_CHANNELS_MAX],
    /// The mixer's mounted channel count (0 when no mixer is mounted).
    pub channel_count: usize,
    pub master: f32,
    /// The mastering stage's readings (`master` plugin), `None` when it is not
    /// mounted: output peaks per side and the gain reduction the compressor +
    /// limiter applied to the last block, in dB (≥ 0, `0` = transparent).
    pub mastering: Option<MasteringStatus>,
    /// The audio output's state: `None` when the host runs silent (no device was
    /// requested), otherwise the negotiated rate/layout and the played counters.
    pub audio: Option<AudioStatus>,
    /// Whether an arrangement edit can be undone / redone (the shell's buttons).
    pub can_undo: bool,
    pub can_redo: bool,
    /// The last error the pump hit while rendering (a wiring failure) — recorded
    /// so the shell surfaces it instead of a silently moving, silent playhead.
    pub last_error: Option<String>,
}

/// What the mastering stage is doing, as a shell reads it: the output peaks and
/// the gain reduction of the last rendered block.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MasteringStatus {
    pub peak_l: f32,
    pub peak_r: f32,
    /// Gain reduction in dB (≥ 0): how much the chain pulled the block down.
    pub reduction_db: f32,
}

/// The audio output's negotiated state and counters.
#[derive(Debug, Clone, Default)]
pub struct AudioStatus {
    /// The device's actual sample rate.
    pub sample_rate: u32,
    /// The device's channel count.
    pub channels: u16,
    /// The rate the host asked for (the session rate).
    pub requested_rate: u32,
    /// True when the device could not run at `requested_rate` — the shell must
    /// surface this (a mismatch plays at the wrong speed).
    pub rate_mismatch: bool,
    /// Source frames the device filled with silence because the ring was empty.
    pub underruns: u64,
    /// Source frames the pump could not push because the ring was full.
    pub drops: u64,
    /// Why audio is unavailable, when it could not be opened at all.
    pub error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            frame: 0,
            seconds: 0.0,
            beat: 0.0,
            bpm: 120.0,
            playing: false,
            // The demo/default session shape (4/4 at 120 bpm, 48 kHz) — the same
            // defaults the shells start from.
            tempo_map: engine::TempoMap::new(48_000, 120.0, 4),
            sample_rate: 48_000,
            channels: [0.0; MIXER_CHANNELS_MAX],
            channel_count: 0,
            master: 0.0,
            mastering: None,
            audio: None,
            can_undo: false,
            can_redo: false,
            last_error: None,
        }
    }
}

/// A point-in-time summary of the session — the on-demand values a shell reads
/// after loading a script (arrangement, pool listing, diagnostics). Built on the
/// actor thread from the live session.
#[derive(Debug, Clone)]
pub struct HostOutcome {
    pub summary: String,
    pub underruns: u64,
    pub deferred: u64,
    pub event_count: usize,
    pub media_commands: usize,
    /// The arrangement value; `Err` if the snapshot errors (a poisoned timeline)
    /// — never a silent empty value.
    pub arrangement: Result<media::Timeline, String>,
    /// The tempo each pool source was performed at (`source_tempo <id> <bpm>`), sorted
    /// by id so a shell can cache it deterministically — what tempo match derives its
    /// ratio from.
    pub source_tempos: Vec<(String, f64)>,
    pub pool_sources: Option<Vec<media::PoolSource>>,
    /// Sources the pool pass resampled to the session rate while adopting the
    /// pool (`set_pool`); empty when the pool already fitted.
    pub pool_conformed: Vec<media::Conform>,
    pub mixer_channels: Option<usize>,
    /// Where the session is saved (`None` until a `save`), and the last journal
    /// (autosave) failure — so a shell can say what happened to the session file
    /// instead of leaving autosave silent.
    pub session_dir: Option<std::path::PathBuf>,
    pub journal_error: Option<String>,
    /// The take in progress (a live indicator) and the last finished one (a shell
    /// reports it once, then it is pool material like any other source).
    pub recording: Option<crate::RecordingStatus>,
    pub last_take: Option<crate::TakeReport>,
    /// The session's current parameter values, folded from the log — what a
    /// shell reads instead of keeping its own copy (see `HostSession::params`).
    /// A live fader change is just the next `SetParam` in the log.
    pub params: Vec<(&'static str, &'static str, f32)>,
}

/// A message to the host thread. `HostCommand` is `Send`, so the request is too.
enum Request {
    /// Apply a command; the result is sent back on the reply channel.
    Command(HostCommand, Sender<Result<(), String>>),
    /// Replace the session with a fresh one and apply `commands`, returning the
    /// resulting outcome — the live equivalent of `run_script`, leaving the
    /// session resident for transport.
    Load(Vec<HostCommand>, Sender<Result<HostOutcome, String>>),
    /// Read the current outcome (arrangement + diagnostics) without changing
    /// anything — what the bridge returns after a single incremental edit.
    Outcome(Sender<HostOutcome>),
    Shutdown,
}

/// The actor thread's audio state. The cpal stream is `!Send`, so it lives here
/// and only here.
enum AudioState {
    /// Silent: audio was not requested (headless tests).
    Off,
    /// Audio was requested but the device could not be opened.
    Failed(String),
    /// A live output stream and the ring the pump fills.
    Open(Audio),
}

struct Audio {
    handle: OutputHandle,
    ring: Arc<Spsc<f32>>,
    /// Source frames the pump could not push (ring full).
    drops: Arc<AtomicU64>,
}

/// A `Send + Sync` handle to the live host thread.
pub struct HostHandle {
    tx: Mutex<Sender<Request>>,
    shared: Arc<Mutex<Snapshot>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl HostHandle {
    /// Spawn the host thread **silent** (no device): the headless form, used by
    /// tests and as a fallback.
    pub fn spawn() -> Self {
        Self::spawn_inner(false)
    }

    /// Spawn the host thread with a real audio output: it opens the default
    /// output device, negotiated to the session rate, and the pump feeds it. If
    /// the device cannot be opened, playback stays silent and the reason is
    /// published in [`Snapshot::audio`] (never a hard failure).
    pub fn spawn_with_audio() -> Self {
        Self::spawn_inner(true)
    }

    fn spawn_inner(want_audio: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let pump_shared = Arc::clone(&shared);
        let join = std::thread::spawn(move || run(HostSession::new(), rx, pump_shared, want_audio));
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

    /// Replace the live session with a fresh one and apply `commands`, returning
    /// the resulting outcome. The live equivalent of `run_script`: each run loads
    /// a fresh session (so re-running a script does not double-mount), and the
    /// session stays resident for transport afterwards.
    pub fn load(&self, commands: &[HostCommand]) -> Result<HostOutcome, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .lock()
            .map_err(|_| "host actor poisoned".to_string())?
            .send(Request::Load(commands.to_vec(), reply_tx))
            .map_err(|_| "host actor exited".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "host actor exited".to_string())?
    }

    /// The session's current outcome (arrangement + diagnostics) — what the
    /// bridge returns after a single incremental edit. Nothing changes.
    pub fn outcome(&self) -> Result<HostOutcome, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .lock()
            .map_err(|_| "host actor poisoned".to_string())?
            .send(Request::Outcome(reply_tx))
            .map_err(|_| "host actor exited".to_string())?;
        reply_rx.recv().map_err(|_| "host actor exited".to_string())
    }

    /// The latest published snapshot (position + meters + audio).
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
/// The largest catch-up burst in one tick for the **silent** (wall-clock) pump.
const MAX_CHUNK: usize = 4096;
/// The output ring's capacity in samples (a power of two). ~0.34 s of stereo at
/// 48 kHz — enough to absorb scheduler jitter, small enough to stay responsive.
const AUDIO_RING_SAMPLES: usize = 1 << 15;
/// The output ring's channel count (the master is mono or stereo; the ring is
/// always stereo, so the device can map it to whatever it has).
const OUTPUT_CHANNELS: u16 = 2;
/// Frames rendered per audio chunk.
const AUDIO_CHUNK_FRAMES: usize = 1024;

/// The actor loop: drain commands, then (while playing) render — into the device
/// ring when audio is open (device-paced), else to the wall clock — and publish.
fn run(
    mut session: HostSession,
    rx: Receiver<Request>,
    shared: Arc<Mutex<Snapshot>>,
    want_audio: bool,
) {
    let rate = session.sample_rate() as f64;
    // Wall-clock anchor for the silent pump: (wall, frame) at play/last change.
    let mut anchor: Option<(Instant, u64)> = None;
    let audio = if want_audio {
        match open_audio(&session) {
            Ok(a) => AudioState::Open(a),
            Err(e) => AudioState::Failed(e),
        }
    } else {
        AudioState::Off
    };
    // Start stopped: the stream is paused until the transport plays, so an idle
    // host does not rack up underruns on an empty ring.
    if let AudioState::Open(a) = &audio {
        let _ = a.handle.pause();
    }
    publish(&session, &shared, &audio);
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
                let is_play = matches!(cmd, HostCommand::TransportPlay);
                let is_stop = matches!(cmd, HostCommand::TransportStop);
                let r = session.execute(&cmd);
                if reanchor {
                    anchor = None;
                }
                if r.is_ok()
                    && let AudioState::Open(a) = &audio
                {
                    if is_play {
                        let _ = a.handle.play();
                    }
                    if is_stop {
                        let _ = a.handle.pause();
                        // Drop the tail so the next play starts from the new
                        // position instead of replaying buffered audio.
                        while a.ring.try_pop().is_some() {}
                    }
                }
                let _ = reply.send(r);
                publish(&session, &shared, &audio);
            }
            Ok(Request::Load(commands, reply)) => {
                // `from_script` honours a `session_rate` line, so a loaded session
                // runs at the rate it was saved at.
                let mut applied = Ok(());
                let fresh = match HostSession::from_script(&commands) {
                    Ok(fresh) => fresh,
                    Err(e) => {
                        applied = Err(e);
                        HostSession::new()
                    }
                };
                let outcome = applied.map(|()| build_outcome(&fresh));
                session = fresh;
                anchor = None;
                // A load is a fresh, stopped session: clear any buffered audio.
                if let AudioState::Open(a) = &audio {
                    let _ = a.handle.pause();
                    while a.ring.try_pop().is_some() {}
                }
                let _ = reply.send(outcome);
                publish(&session, &shared, &audio);
            }
            Ok(Request::Outcome(reply)) => {
                let _ = reply.send(build_outcome(&session));
                publish(&session, &shared, &audio);
            }
            Ok(Request::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }

        if session.is_playing() {
            match &audio {
                AudioState::Open(a) => {
                    if let Err(e) = fill_audio(&mut session, &a.ring, &a.drops) {
                        let _ = session.execute(&HostCommand::TransportStop);
                        let _ = a.handle.pause();
                        anchor = None;
                        if let Ok(mut s) = shared.lock() {
                            s.last_error = Some(e);
                        }
                    }
                }
                _ => {
                    let (wall0, frame0) =
                        *anchor.get_or_insert_with(|| (Instant::now(), session.position().frame));
                    let target = frame0 + (wall0.elapsed().as_secs_f64() * rate) as u64;
                    let now = session.position().frame;
                    if target > now {
                        let want = (target - now).min(MAX_CHUNK as u64) as usize;
                        if let Err(e) = session.render(want) {
                            // A wiring failure must not silently yield a moving,
                            // silent playhead: record it and stop the transport.
                            let _ = session.execute(&HostCommand::TransportStop);
                            anchor = None;
                            if let Ok(mut s) = shared.lock() {
                                s.last_error = Some(e);
                            }
                        }
                    }
                }
            }
            publish(&session, &shared, &audio);
        }
    }
    if let AudioState::Open(a) = &audio {
        let _ = a.handle.pause();
    }
}

/// Open the default output device, negotiated to the session rate.
fn open_audio(session: &HostSession) -> Result<Audio, String> {
    let ring = Arc::new(Spsc::new(AUDIO_RING_SAMPLES));
    let handle =
        media::devices::open_output(Arc::clone(&ring), OUTPUT_CHANNELS, session.sample_rate())?;
    Ok(Audio {
        handle,
        ring,
        drops: Arc::new(AtomicU64::new(0)),
    })
}

/// Keep the output ring about half full. The device callback drains it, so this
/// paces the pump to the **device** clock — the ring's free space is the timing
/// signal, not the wall clock.
fn fill_audio(
    session: &mut HostSession,
    ring: &Spsc<f32>,
    drops: &AtomicU64,
) -> Result<(), String> {
    let target = ring.capacity() / 2;
    while session.is_playing() && ring.len() < target {
        let samples = session.render(AUDIO_CHUNK_FRAMES)?;
        push_stereo(ring, &samples, session.master_channels(), drops);
    }
    Ok(())
}

/// Push a rendered master block into the stereo output ring: a mono master is
/// duplicated, a stereo one passes through, extra channels are dropped. A whole
/// frame is pushed or none is, so L/R never swap on a full ring.
fn push_stereo(ring: &Spsc<f32>, samples: &[f32], master_channels: usize, drops: &AtomicU64) {
    let ch = master_channels.max(1);
    for frame in samples.chunks(ch) {
        if ring.len() + OUTPUT_CHANNELS as usize > ring.capacity() {
            drops.fetch_add(1, Ordering::Relaxed);
            return; // never leave a half frame in the ring
        }
        let (l, r) = match frame {
            [] => (0.0, 0.0),
            [m] => (*m, *m),
            [l, r, ..] => (*l, *r),
        };
        let _ = ring.try_push(l);
        let _ = ring.try_push(r);
    }
}

/// Build the on-demand outcome the shell reads after loading a script.
fn build_outcome(session: &HostSession) -> HostOutcome {
    HostOutcome {
        summary: crate::summarize(session),
        underruns: session.underruns(),
        deferred: session.deferred(),
        event_count: session.event_count(),
        media_commands: session.media_command_count(),
        arrangement: session.arrangement(),
        source_tempos: {
            let mut tempos: Vec<(String, f64)> = session
                .source_tempos()
                .iter()
                .map(|(id, bpm)| (id.clone(), *bpm))
                .collect();
            tempos.sort_by(|a, b| a.0.cmp(&b.0));
            tempos
        },
        pool_sources: session.pool_sources(),
        pool_conformed: session.pool_conformed().to_vec(),
        mixer_channels: session.mixer_channels(),
        session_dir: session.session_dir().map(std::path::Path::to_path_buf),
        journal_error: session.journal_error().map(str::to_string),
        recording: session.recording(),
        last_take: session.last_take().cloned(),
        params: session.params(),
    }
}

/// Publish the session's current position + meters + audio state into the shared
/// snapshot.
fn publish(session: &HostSession, shared: &Mutex<Snapshot>, audio: &AudioState) {
    let p = session.position();
    let Ok(mut s) = shared.lock() else { return };
    s.frame = p.frame;
    s.seconds = p.seconds;
    s.beat = p.beat;
    s.bpm = p.bpm;
    s.playing = p.playing;
    // One clone per publish: the map is the shell's musical time base, and the
    // sample rate comes with it (they must not disagree).
    let tempo = session.tempo_map();
    s.sample_rate = tempo.sample_rate();
    s.tempo_map = tempo;
    s.channel_count = session.mixer_channels().unwrap_or(0);
    s.can_undo = session.can_undo();
    s.can_redo = session.can_redo();
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
    s.mastering = session.master_meters().map(|m| MasteringStatus {
        peak_l: m.peak_l(),
        peak_r: m.peak_r(),
        reduction_db: m.reduction_db(),
    });
    s.audio = match audio {
        AudioState::Off => None,
        AudioState::Failed(e) => Some(AudioStatus {
            error: Some(e.clone()),
            ..AudioStatus::default()
        }),
        AudioState::Open(a) => Some(AudioStatus {
            sample_rate: a.handle.sample_rate,
            channels: a.handle.channels,
            requested_rate: a.handle.requested_rate,
            rate_mismatch: a.handle.rate_mismatch,
            underruns: a.handle.underruns.load(Ordering::Relaxed),
            drops: a.drops.load(Ordering::Relaxed),
            error: None,
        }),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pump advances the clock while playing and holds it still when stopped.
    #[test]
    fn live_host_plays_advances_and_stops() {
        let host = HostHandle::spawn();
        let s = host.snapshot();
        assert!(!s.playing, "a fresh host is stopped");
        assert!(s.audio.is_none(), "spawn() is the silent form");

        host.execute(HostCommand::TransportPlay).expect("play");
        std::thread::sleep(Duration::from_millis(150));
        let s = host.snapshot();
        assert!(s.playing, "the transport is running");
        assert!(
            s.frame > 0,
            "the pump advanced the clock (frame={})",
            s.frame
        );

        host.execute(HostCommand::TransportStop).expect("stop");
        std::thread::sleep(Duration::from_millis(80));
        let stopped = host.snapshot().frame;
        assert!(!host.snapshot().playing, "the transport stopped");
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            host.snapshot().frame,
            stopped,
            "the clock is still while stopped"
        );

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
        assert_eq!(
            host.snapshot().channel_count,
            2,
            "the mixer channel count is published"
        );

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
        host.execute(HostCommand::TransportStop)
            .expect("host still usable");

        host.shutdown();
    }

    /// `load` replaces the session, so re-running the same script does not
    /// double-mount — the live equivalent of the stateless `run_script`.
    #[test]
    fn live_host_loads_a_script_and_reloads_cleanly() {
        let host = HostHandle::spawn();
        let script = crate::parse_script("host v1\nmount mixer channels=2 @0\n").expect("parse");
        let first = host.load(&script).expect("first load");
        assert_eq!(
            first.mixer_channels,
            Some(2),
            "the mixer channel count is reported"
        );
        let second = host.load(&script).expect("re-load starts a fresh session");
        assert_eq!(second.mixer_channels, Some(2));
        assert!(second.summary.contains("underruns: 0"));
        host.shutdown();
    }

    /// `spawn_with_audio` always publishes an audio status — `Open` with a real
    /// device, `Failed` (with the reason) without one — never a panic. Ignored by
    /// default: it opens real hardware.
    #[test]
    #[ignore = "opens real audio hardware; run: cargo test -p host -- --ignored audio"]
    fn live_host_with_audio_publishes_a_status() {
        let host = HostHandle::spawn_with_audio();
        let deadline = Instant::now() + Duration::from_secs(5);
        let audio = loop {
            if let Some(a) = host.snapshot().audio {
                break a;
            }
            assert!(
                Instant::now() < deadline,
                "no audio status published within 5 s"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        eprintln!(
            "audio: {} Hz requested {} Hz, {} ch, mismatch={} — {:?}",
            audio.sample_rate,
            audio.requested_rate,
            audio.channels,
            audio.rate_mismatch,
            audio.error
        );
        host.shutdown();
    }
}
