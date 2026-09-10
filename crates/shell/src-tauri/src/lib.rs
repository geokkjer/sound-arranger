//! The Tauri shell's Host-API bridge — now a **live** session.
//!
//! The shell owns a [`host::live::HostHandle`] (a host thread with the
//! `HostSession` on it — the session is `!Send`, so it cannot be `tauri::State`
//! directly; see the Tauri review's F2 and the live-runtime note). The frontend:
//!
//! - **loads** a versioned host script into the session ([`run_host_script`] —
//!   reset-and-apply, the live equivalent of `run_script`), and
//! - **drives transport** ([`transport_play`]/[`transport_stop`]/[`transport_seek`])
//!   and **polls** it ([`transport_state`]) to tick the playhead and meters.
//!
//! The versioned text format stays the wire schema (the same contract the CLI
//! drives); the transport commands are first-class Tauri commands because they
//! are the live, high-frequency controls, not script text.

use serde::Serialize;
use tauri::{Manager, State};

use host::live::{HostHandle, HostOutcome, Snapshot};
use host::HostCommand;

/// The mixer meter snapshot: per-channel peaks (post-gain, pre-mute/solo) plus
/// the master peak (post-master-gain, max of L/R). Drawn by the mixer panel.
#[derive(Debug, Clone, Serialize)]
pub struct MixerMeters {
    pub channels: Vec<f32>,
    pub master: f32,
}

/// The transport position for the shell's playhead / time readout.
#[derive(Debug, Clone, Serialize)]
pub struct TransportPosition {
    pub frame: u64,
    pub seconds: f64,
    pub beat: f64,
    pub bpm: f64,
    pub playing: bool,
}

/// A poll of the live host: position + meters + the pump's last error. The shell
/// calls this on a timer (~30 Hz) to tick the playhead and meters.
#[derive(Debug, Clone, Serialize)]
pub struct TransportState {
    pub position: TransportPosition,
    pub channels: Vec<f32>,
    pub channel_count: usize,
    pub master: f32,
    pub last_error: Option<String>,
}

/// A runnable outcome the frontend can show: a script's diagnostics plus a
/// summary of the resulting session and its transport position.
#[derive(Debug, Clone, Serialize)]
pub struct ScriptOutcome {
    pub summary: String,
    pub underruns: u64,
    pub deferred: u64,
    pub event_count: usize,
    pub media_commands: usize,
    /// The full arrangement value (tracks/clips with their frames), so the
    /// frontend can draw the timeline canvas without a second round-trip.
    pub arrangement: Option<media::Timeline>,
    /// The arrangement snapshot error, if `arrangement()` errored — the shell
    /// surfaces it rather than showing a silently empty timeline.
    pub arrangement_error: Option<String>,
    /// The mixer's meter peaks, if a mixer was mounted (drives the mixer panel).
    pub mixer_meters: Option<MixerMeters>,
    /// The media pool source listing, if a pool was set (drives the source-pool
    /// panel). Each source carries its id, frames, sample rate, peaks state.
    pub pool_sources: Option<Vec<media::PoolSource>>,
    pub bounce_written: bool,
    /// The transport position after the load.
    pub position: TransportPosition,
    /// How many mixer channels are mounted (drives the mixer strips).
    pub channel_count: usize,
}

/// The live transport poll, mapped from the shared snapshot.
fn to_transport_state(s: &Snapshot) -> TransportState {
    let n = s.channel_count.min(s.channels.len());
    TransportState {
        position: TransportPosition {
            frame: s.frame,
            seconds: s.seconds,
            beat: s.beat,
            bpm: s.bpm,
            playing: s.playing,
        },
        channels: s.channels[..n].to_vec(),
        channel_count: s.channel_count,
        master: s.master,
        last_error: s.last_error.clone(),
    }
}

/// Map a live outcome + the current snapshot into the serializable wire shape.
fn to_wire(handle: &HostHandle, outcome: HostOutcome, commands: &[HostCommand]) -> ScriptOutcome {
    let snap = handle.snapshot();
    let (arrangement, arrangement_error) = match outcome.arrangement {
        Ok(tl) => (Some(tl), None),
        Err(e) => (None, Some(e)),
    };
    let n = snap.channel_count.min(snap.channels.len());
    let mixer_meters = (snap.channel_count > 0).then(|| MixerMeters {
        channels: snap.channels[..n].to_vec(),
        master: snap.master,
    });
    ScriptOutcome {
        summary: outcome.summary,
        underruns: outcome.underruns,
        deferred: outcome.deferred,
        event_count: outcome.event_count,
        media_commands: outcome.media_commands,
        arrangement,
        arrangement_error,
        mixer_meters,
        pool_sources: outcome.pool_sources,
        bounce_written: commands
            .iter()
            .rev()
            .any(|c| matches!(c, HostCommand::Bounce { .. })),
        position: TransportPosition {
            frame: snap.frame,
            seconds: snap.seconds,
            beat: snap.beat,
            bpm: snap.bpm,
            playing: snap.playing,
        },
        channel_count: snap.channel_count,
    }
}

/// Parse and load a versioned host script into the live session, returning the
/// outcome. Public (not `#[tauri::command]`) so it is unit-testable without a
/// Tauri runtime.
pub fn load_script(handle: &HostHandle, script_text: &str) -> Result<ScriptOutcome, String> {
    let commands = host::parse_script(script_text)?;
    let outcome = handle.load(&commands)?;
    Ok(to_wire(handle, outcome, &commands))
}

/// Load a host script into the live session (reset-and-apply). Rejections (bad
/// script, refused op) are surfaced as `Err` so the UI can show them without
/// crashing the shell.
#[tauri::command]
fn run_host_script(script_text: String, host: State<'_, HostHandle>) -> Result<ScriptOutcome, String> {
    load_script(&host, &script_text)
}

/// Start the transport (the pump advances the clock while it runs).
#[tauri::command]
fn transport_play(host: State<'_, HostHandle>) -> Result<(), String> {
    host.execute(HostCommand::TransportPlay)
}

/// Stop the transport at the current position.
#[tauri::command]
fn transport_stop(host: State<'_, HostHandle>) -> Result<(), String> {
    host.execute(HostCommand::TransportStop)
}

/// Seek to an absolute frame (rebuild + render-to-target — see the host note).
#[tauri::command]
fn transport_seek(frame: u64, host: State<'_, HostHandle>) -> Result<(), String> {
    host.execute(HostCommand::TransportSeek { frame })
}

/// Poll the live host for the playhead + meters (non-blocking).
#[tauri::command]
fn transport_state(host: State<'_, HostHandle>) -> TransportState {
    to_transport_state(&host.snapshot())
}

/// Whether the native title bar should be **dropped** on this session.
///
/// The shell draws its own top bar, so server-side decorations are redundant
/// chrome — and on a Wayland *tiling* compositor they look out of place. Detect
/// the tiling compositors we know and drop them; keep decorations on a floating
/// desktop so the window can still be moved and resized. (A user preference can
/// replace this later; the ui-plan's `Preference` contribution is the home.)
fn prefer_undecorated() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    std::env::var_os("NIRI_SOCKET").is_some()                     // niri
        || std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() // Hyprland
        || std::env::var_os("SWAYSOCK").is_some()                 // sway
        || desktop.contains("niri")
}

/// Build and run the Tauri application with a live host session.
pub fn run() {
    tauri::Builder::default()
        .manage(HostHandle::spawn())
        .setup(|app| {
            // The window is created undecorated (tauri.conf.json); restore the
            // native title bar on a floating desktop.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_decorations(!prefer_undecorated());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            run_host_script,
            transport_play,
            transport_stop,
            transport_seek,
            transport_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A tone bounced through the mixer is a valid script; the bridge returns
    /// clean diagnostics and reports the bounce was written.
    #[test]
    fn load_script_bounces_a_tone_with_no_underruns() {
        let host = HostHandle::spawn();
        let wav = std::env::temp_dir().join(format!("shell-bridge-{}.wav", std::process::id()));
        let script = format!(
            "host v1\nmount mixer channels=4 @0\nbounce 512 {}\n",
            wav.display()
        );
        let outcome = load_script(&host, &script).expect("script must run");
        assert!(outcome.bounce_written, "a Bounce command was in the script");
        assert_eq!(outcome.underruns, 0, "no underruns on a clean tone bounce");
        assert_eq!(outcome.event_count, 1, "one logged engine event (the mount)");
        assert_eq!(outcome.media_commands, 1, "one media command (the bounce)");
        assert_eq!(
            outcome.arrangement.as_ref().map_or(0, |t| t.tracks.len()),
            0,
            "no arrangement tracks in this script"
        );
        assert_eq!(outcome.channel_count, 4, "the mixer's channel count is reported");
        assert!(outcome.summary.contains("underruns: 0"));
        host.shutdown();
        let _ = std::fs::remove_file(&wav);
    }

    /// A malformed script is refused cleanly — no panic, an `Err` the UI can show.
    #[test]
    fn load_script_rejects_a_script_without_version_header() {
        let host = HostHandle::spawn();
        let err = load_script(&host, "mount mixer channels=4 @0\n").unwrap_err();
        assert!(err.contains("host v"), "the versioned header is required: {err}");
        host.shutdown();
    }

    /// Transport is reachable over the same handle the commands use: load, play,
    /// poll (the playhead advances), stop.
    #[test]
    fn transport_plays_over_the_live_handle() {
        let host = HostHandle::spawn();
        load_script(&host, "host v1\nmount mixer channels=2 @0\n").expect("load");
        host.execute(HostCommand::TransportPlay).expect("play");
        std::thread::sleep(Duration::from_millis(120));
        let st = to_transport_state(&host.snapshot());
        assert!(st.position.playing, "the transport is running");
        assert!(st.position.frame > 0, "the playhead advanced (frame={})", st.position.frame);
        assert_eq!(st.channel_count, 2);
        host.execute(HostCommand::TransportStop).expect("stop");
        host.shutdown();
    }
}
