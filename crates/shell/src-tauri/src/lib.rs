//! The Tauri shell's Host-API bridge.
//!
//! The shell is the *host_* side of the [UI-as-plugin note]. It owns a live
//! [`host::HostSession`] over the engine and exposes it to the Vue frontend
//! through Tauri commands. The versioned text format (`host::parse_script` /
//! `host::run_script`) is the shell's wire schema — the same contract the CLI
//! smoke binary drives, so a script that bounces byte-identically on the CLI
//! behaves the same here.
//!
//! This is the transport layer only: it does not render the timeline canvas,
//! source pool, or mixer views (the `ui-plugin`s). It makes the engine reachable
//! from the frontend so those can be built on top.

use serde::Serialize;

/// A runnable outcome the frontend can show: a one-shot script's diagnostics plus
/// a summary of the resulting session.
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
    pub bounce_written: bool,
}

impl ScriptOutcome {
    /// The number of tracks in the arrangement (a convenience for CLI fallback).
    pub fn arrangement_tracks(&self) -> usize {
        self.arrangement.as_ref().map(|t| t.tracks.len()).unwrap_or(0)
    }
}

/// Parse and run a versioned host script, returning the session diagnostics.
/// Public (not `#[tauri::command]`) so it is unit-testable without a Tauri
/// runtime.
pub fn exec_host_script(script_text: &str) -> Result<ScriptOutcome, String> {
    let commands = host::parse_script(script_text)?;
    let session = host::run_script(&commands)?;
    let arrangement = session.arrangement().ok();
    let bounce_written = commands
        .iter()
        .rev()
        .any(|c| matches!(c, host::HostCommand::Bounce { .. }));
    Ok(ScriptOutcome {
        summary: host::summarize(&session),
        underruns: session.underruns(),
        deferred: session.deferred(),
        event_count: session.event_count(),
        media_commands: session.media_command_count(),
        arrangement,
        bounce_written,
    })
}

/// Run a host script from the frontend. Rejections (bad script, refused op) are
/// surfaced as `Err` so the UI can show them without crashing the shell.
#[tauri::command]
fn run_host_script(script_text: String) -> Result<ScriptOutcome, String> {
    exec_host_script(&script_text)
}

/// Build and run the Tauri application, registering the Host-API bridge.
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![run_host_script])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone bounced through the mixer is a valid script; the bridge returns
    /// clean diagnostics, no underruns, and reports the bounce was written.
    #[test]
    fn exec_script_bounces_a_tone_with_no_underruns() {
        let wav = std::env::temp_dir().join(format!("shell-bridge-{}.wav", std::process::id()));
        let script = format!(
            "host v1\nmount mixer channels=4 @0\nbounce 512 {}\n",
            wav.display()
        );
        let outcome = exec_host_script(&script).expect("script must run");
        assert!(outcome.bounce_written, "a Bounce command was in the script");
        assert_eq!(outcome.underruns, 0, "no underruns on a clean tone bounce");
        assert_eq!(outcome.event_count, 1, "one logged engine event (the mount)");
        assert!(outcome.arrangement_tracks() == 0, "no arrangement in this script");
        assert_eq!(outcome.media_commands, 1, "one media command (the bounce)");
        assert!(outcome.summary.contains("underruns: 0"));
        let _ = std::fs::remove_file(&wav);
    }

    /// A malformed script is refused cleanly — no panic, an `Err` the UI can show.
    #[test]
    fn exec_script_rejects_a_without_version_header() {
        let err = exec_host_script("mount mixer channels=4 @0\n").unwrap_err();
        assert!(err.contains("host v"), "the versioned header is required: {err}");
    }
}
