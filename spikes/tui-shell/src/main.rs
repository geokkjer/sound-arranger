//! The ratatui spike — the terminal counterpart of the iced spike, and the
//! second half of the shell evaluation: can a **TUI** be a sound-arranger shell?
//!
//! The evaluation is written down in
//! `.agents/notes/proposed/architecture/2026-09-21-tui-shell-evaluation.md`. The
//! scope is deliberately the *same* as the iced spike, so the two are comparable
//! side by side:
//!
//! 1. **It runs in a terminal** (the TUI's version of "the window opens").
//! 2. **The host thread is unchanged** — the same [`host::live::HostHandle`] the
//!    Tauri bridge and the iced spike used.
//! 3. **Transport** (play / stop / rewind / seek) drives it.
//! 4. **Meters and the playhead follow the audio**, polled from the published
//!    `Snapshot` once per frame.
//!
//! What this spike adds, because it is the thing a TUI has to answer for: an
//! explicit **key scheme** (with a `?` overlay that is the single source of
//! truth for it) and **mouse support** — clickable transport buttons and
//! click-to-select meters — so the human can judge whether the mouse in a
//! terminal is workable or the keys carry it. It also times every command, so
//! the cost of the host's O(target) `TransportSeek` shows up as UI latency
//! rather than as a mystery stall.
//!
//! ```sh
//! cd spikes/tui-shell
//! cargo run              # the TUI (any terminal)
//! cargo run -- --probe   # headless: host thread + transport + meters, no TTY
//! cargo run -- --dump    # render one deterministic frame and print it as text
//! cargo run -- --keys    # start with the keymap overlay open
//! cargo test             # asserts the rendered frame (no TTY needed)
//! ```

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use host::HostCommand;
use host::live::{AudioStatus, HostHandle, Snapshot};

mod mixer;
mod timeline;
use mixer::Mixer;
use timeline::{Arrangement, Mode, Placed, Sources, View};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Clear, Gauge, List, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

/// The demo profile — identical to the iced spike's, so the two shells are
/// watching the same signal: euclidean → scale → tone → mixer channel 0.
const DEMO_SCRIPT: &str = "\
host v1
mount euclidean steps=8 pulses=5
mount scale root=0 note_len=2400
mount tone gain=0.9 blip_len=1800
mount mixer channels=4
patch euclidean.triggers scale.trigger
patch scale.note tone.note
patch tone.audio mixer.ch0
set_param mixer master.gain 0.8 @0
";

/// Repaint interval for the event loop. The loop is input-driven with a 16 ms
/// poll timeout, so the meters and the playhead keep up with the audio without
/// a busy spin.
const FRAME: Duration = Duration::from_millis(16);

/// The host session's rate, for turning a seek in seconds into a frame. The
/// snapshot carries the device rate only when audio is open, and a TUI should
/// work headless too.
const SAMPLE_RATE: u64 = 48_000;
/// The click guard a **new** clip boundary gets (a paste): ~1.3 ms at 48 kHz —
/// long enough to kill the discontinuity, short enough not to be a fade. A copied
/// fade is kept instead of this, and a clip property set by hand is `set_clip_fade`
/// (logged, undoable).
const MICRO_FADE: u64 = 64;

/// The keymap now lives in the **shared workflow** (`workflow::KEYMAP`): the
/// handler matches on its [`workflow::Action`]s and the `?` overlay renders its rows,
/// so both shells read one table and the help cannot drift from the behaviour.
use workflow::Action;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keys = args.iter().any(|arg| arg == "--keys");
    let value_of = |flag: &str| {
        args.windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| PathBuf::from(&pair[1]))
    };

    let wave = value_of("--wave");
    let script = value_of("--script");

    if args.iter().any(|arg| arg == "--probe") {
        std::process::exit(probe(wave, script));
    }

    if args.iter().any(|arg| arg == "--dump") {
        return dump(keys, wave, script);
    }

    run(keys, wave, script)
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

/// What the state is and how messages change it. This is the Elm shape the iced
/// spike uses too (state · message · update · view), just without iced: `Action`
/// is the message, `App::apply` is `update`, `App::draw` is `view`, and the
/// event loop is the runtime.
struct App {
    host: HostHandle,
    snap: Snapshot,
    /// The last load/refusal message, shown in the state panel.
    status: String,
    /// The selected mixer channel.
    selected: usize,
    help: bool,
    /// How far the `?` overlay is scrolled (the keymap no longer fits a 24-row
    /// terminal, and a truncated help is a lying help).
    help_scroll: usize,
    /// The `:` command line, `Some(text)` while it is open. It types the **same
    /// `host v1` text format** the keys dispatch, so every action is reachable even
    /// before a key exists for it — the modal note's "a widget away, not a project".
    prompt: Option<String>,
    /// The last finished take the shell has announced, so it says it once (the host
    /// keeps reporting it until the next take starts).
    last_take_seen: Option<String>,
    /// Executed command lines, oldest first; `↑`/`↓` walk them.
    history: Vec<String>,
    /// Where the history walk currently is (`history.len()` = the live line).
    history_at: usize,
    mouse: bool,
    quit: bool,
    /// The last command's label and how long the UI thread was blocked in it.
    last_command: Option<(&'static str, Duration)>,
    /// Rects recorded by the last draw, for mouse hit-testing: the terminal has
    /// no widget tree to query, so the view publishes what is clickable.
    buttons: Vec<(Rect, Button)>,
    meter_rows: Vec<(usize, Rect)>,
    mixer_rect: Rect,
    /// The console: fader positions, mutes and solos the shell has asked the
    /// host for (the host owns the audio state; this is the view of the ask).
    mixer: Mixer,
    /// What the mixer strips drew, for hit-testing.
    strips: mixer::Strips,
    /// The timeline: the arrangement the host holds, its viewport, the focused
    /// panel, the active track, and the mode.
    arrangement: Option<Arrangement>,
    sources: Sources,
    /// The **declared source** the next take will capture, when one is armed (`A`
    /// cycles `default → the rig's sources → default`). `None` is the default input.
    /// Shell state, like the take id: the host binds the name at `record` time.
    record_source: Option<String>,
    view: Option<View>,
    mode: Mode,
    focus: Panel,
    active_track: usize,
    /// Where the timeline came from, for the panel title.
    origin: String,
    /// A counter for ids the shell generates (split halves) — ids are logged, so
    /// they must be unique and deterministic per session.
    next_id: u64,
    /// The **snap grid** (alpha slice C): which musical division an edit lands on.
    /// UI state, never logged — a snapped edit is an edit whose frame was
    /// quantized *before* the command was issued, so replay is untouched.
    grid: workflow::Grid,
    /// The shell's **clipboard** (alpha slice D): `(track delta, clip)` values
    /// measured from `clipboard_origin`. A *value*, never logged — copying changes
    /// nothing, and only the paste reaches the log. The delta is zero for every
    /// copy today (a selection is one track); it is in the shape so a multi-track
    /// selection is a change to the copy, not to the clipboard.
    clipboard: Vec<(i32, Placed)>,
    /// The frame the clipboard's offsets are measured from (the earliest copied
    /// clip's start), so a paste keeps the copied clips' relative spacing.
    clipboard_origin: u64,
    /// The next minted paste id (`paste.{n}`), seeded above anything the session
    /// already has and **never decreasing**, so a paste after an undo cannot mint an
    /// id the journal still holds (a replay would refuse the second `add_clip` and the
    /// clip would vanish on recovery).
    next_paste: u64,
    /// The same counter for the pool panel's placements (`pool.{n}`).
    next_pool: u64,
    /// The session pool's **listing** as of the last adoption: the pool panel's
    /// rows, and what a paste checks its sources against (the clipboard is a value
    /// and survives a load, but the pool a source id names may not).
    pool: Vec<media::PoolSource>,
    /// The last export the shell has reported (`frames`, `format`), so the status
    /// line announces a *new* one once instead of on every refresh.
    last_export_seen: Option<(u64, &'static str, u32, u32)>,
    /// The tempo each pool source was performed at (`source_tempo`), as of the last
    /// adoption — what `W` (warp) derives its stretch ratio from.
    source_tempos: std::collections::HashMap<String, f64>,
    /// The selected row in the pool panel.
    pool_selected: usize,
    /// What a *prefilled* prompt was opened with (`R`'s rename line): walking the
    /// history and coming back to the live line must return this, not blank.
    prompt_prefill: Option<String>,
    panel: timeline::PanelRects,
    /// The pool panel's rectangle, for mouse hit-testing.
    pool_rect: Rect,
}

/// Which panel the keys act on. `Tab`/`Shift-Tab` cycle; a click focuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Mixer,
    Timeline,
    /// The session pool: the material the arrangement is made *from*. It appears
    /// only when a pool is loaded, and it is where "load clips from the pool" stops
    /// being an error string.
    Pool,
}

impl Panel {
    fn label(self) -> &'static str {
        match self {
            Panel::Mixer => "mixer",
            Panel::Timeline => "timeline",
            Panel::Pool => "pool",
        }
    }
}

/// A mouse target: the transport buttons. Distinct from a workflow
/// [`workflow::Action`] on purpose — one is a rectangle to click, the other is what the
/// workflow says a key means (and both can reach the same method).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Button {
    Play,
    Stop,
    Rewind,
}

/// What `o` does, as the **host's** state declares it (see `App::record_intent`).
///
/// An enum rather than a test on the status string: the decision is engine state, and
/// naming it is what lets a headless test drive it without a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordIntent {
    /// A take is running: stop it.
    Stop,
    /// No take is running: ask for a name.
    Name,
}

impl App {
    /// Boot against the live host, with a real audio device when one exists, and
    /// load the demo profile. Failures land in `status`, never as a panic.
    fn boot(wave: Option<PathBuf>, script: Option<PathBuf>) -> Self {
        let host = HostHandle::spawn_with_audio();
        let status = match host::parse_script(DEMO_SCRIPT) {
            Ok(script) => match host.load(&script) {
                Ok(outcome) => format!(
                    "demo loaded — mixer {} ch, {} log events, {} underruns",
                    outcome.mixer_channels.unwrap_or(0),
                    outcome.event_count,
                    outcome.underruns,
                ),
                Err(e) => format!("load failed: {e}"),
            },
            Err(e) => format!("demo script does not parse: {e}"),
        };

        let mut app = App {
            snap: host.snapshot(),
            host,
            status,
            selected: 0,
            help: false,
            help_scroll: 0,
            prompt: None,
            last_take_seen: None,
            history: Vec::new(),
            history_at: 0,
            mouse: true,
            quit: false,
            last_command: None,
            buttons: Vec::new(),
            meter_rows: Vec::new(),
            mixer: Mixer::new(4),
            strips: mixer::Strips::default(),
            mixer_rect: Rect::default(),
            arrangement: None,
            sources: Sources::default(),
            record_source: None,
            view: None,
            mode: Mode::Normal,
            focus: Panel::Mixer,
            active_track: 0,
            origin: String::new(),
            next_id: 0,
            // Off by default: snapping silently would change every existing
            // gesture, so the grid is armed explicitly with `b`.
            grid: workflow::Grid::default(),
            clipboard: Vec::new(),
            clipboard_origin: 0,
            next_paste: 1,
            next_pool: 1,
            pool: Vec::new(),
            pool_selected: 0,
            source_tempos: std::collections::HashMap::new(),
            last_export_seen: None,
            prompt_prefill: None,
            panel: timeline::PanelRects::default(),
            pool_rect: Rect::default(),
        };

        if let Some(path) = script {
            app.open_script(&path);
        } else if let Some(path) = wave {
            app.open_wave(&path);
        }

        app
    }

    /// Load a single audio file as a **one-clip arrangement in the host**
    /// (`--wave`): the shell imports it into the spike's own **session pool** at
    /// the session rate — the same boundary a product import crosses — then hands
    /// the host the script a user would write, so the file is *audible* and the
    /// arrangement the panel draws is the engine's own value. A 44.1 kHz file is
    /// resampled here, once, instead of being refused by the transport.
    fn open_wave(&mut self, path: &std::path::Path) {
        let (script, note) = match wave_script(path) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.status = format!("cannot open {}: {e}", path.display());
                return;
            }
        };
        self.origin = path.display().to_string();

        match host::parse_script(&script) {
            Ok(commands) => match self.host.load(&commands) {
                Ok(outcome) => {
                    let clips = outcome
                        .arrangement
                        .as_ref()
                        .map(|t| t.tracks.iter().map(|t| t.clips.len()).sum::<usize>())
                        .unwrap_or(0);
                    self.status = format!(
                        "{note} — {clips} clip(s), {} log events (press space to hear it)",
                        outcome.event_count,
                    );
                    self.take_arrangement(outcome);
                }
                Err(e) => self.status = format!("loading {}: {e}", path.display()),
            },
            Err(e) => self.status = format!("{}: {e}", path.display()),
        }
    }

    /// Load a **host script** (`pool`/`arrange` lines) into the live host and
    /// draw the arrangement the host then holds — the same value the engine
    /// renders, not a copy of it.
    fn open_script(&mut self, path: &std::path::Path) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                self.status = format!("cannot read {}: {e}", path.display());
                return;
            }
        };
        let script = match host::parse_script(&text) {
            Ok(script) => script,
            Err(e) => {
                self.status = format!("{}: {e}", path.display());
                return;
            }
        };

        self.origin = path.display().to_string();

        match self.host.load(&script) {
            Ok(outcome) => {
                self.status = format!(
                    "script loaded — {} tracks, {} clips, {} log events",
                    outcome
                        .arrangement
                        .as_ref()
                        .map(|t| t.tracks.len())
                        .unwrap_or(0),
                    outcome
                        .arrangement
                        .as_ref()
                        .map(|t| t.tracks.iter().map(|t| t.clips.len()).sum::<usize>())
                        .unwrap_or(0),
                    outcome.event_count,
                );
                self.take_arrangement(outcome);
            }
            Err(e) => self.status = format!("script failed: {e}"),
        }
    }

    /// Re-read the arrangement from the host after an edit, so the panel always
    /// shows the engine's value.
    fn refresh_arrangement(&mut self) {
        match self.host.outcome() {
            Ok(outcome) => self.take_arrangement(outcome),
            Err(e) => self.status = format!("cannot read the arrangement: {e}"),
        }
    }

    /// Adopt a host outcome's arrangement into the panel, keeping the viewport.
    fn take_arrangement(&mut self, outcome: host::live::HostOutcome) {
        // Autosave must never fail silently: the host records the last journal
        // failure, and the shell says so (an edit is not failed by it).
        if let Some(err) = &outcome.journal_error {
            self.status = format!("autosave failed: {err}");
        }
        // A take in progress is **not** written into the status line: it is live
        // state, so the footer draws it from the snapshot (`recording_line`) and a
        // message — an export report, a source error — cannot hide it. The finish is
        // still announced once, below.
        // A finished export is a deliverable: say what was written, once (the report
        // replaces the command line's echo, because it is what the user wanted to know).
        // The dedupe key is the whole report, not just (length, format): two exports of
        // the same shape but different content must both be announced. And when the host
        // has no report at all (a fresh session after a load), forget the last one — so
        // repeating an export after a rebuild is announced again.
        match &outcome.last_export {
            Some(export) => {
                let seen = (
                    export.frames,
                    export.format,
                    export.peak.to_bits(),
                    export.rms.to_bits(),
                );
                if self.last_export_seen != Some(seen) {
                    self.last_export_seen = Some(seen);
                    let db = |x: f32| {
                        if x > 0.0 {
                            20.0 * x.log10()
                        } else {
                            f32::NEG_INFINITY
                        }
                    };
                    self.status = format!(
                        "exported {} frames ({}, +{} tail) — peak {:.1} dBFS, rms {:.1} dBFS",
                        export.frames + export.drained_frames,
                        export.format,
                        export.drained_frames,
                        db(export.peak),
                        db(export.rms)
                    );
                }
            }
            None => self.last_export_seen = None,
        }
        // A finished take is pool material: announce it once, and say what to do next.
        if let Some(take) = &outcome.last_take
            && self.last_take_seen.as_deref() != Some(take.take_id.as_str())
        {
            self.last_take_seen = Some(take.take_id.clone());
            self.status = format!(
                "take {} — {} ch, {:.1} s ({} frames) → sources {}: place one with an `arrange add_clip` line",
                take.take_id,
                take.channels,
                take.frames as f64 / take.sample_rate.max(1) as f64,
                take.frames,
                take.sources.join(", "),
            );
        }
        // The pool listing is adopted **first**: it is the pool panel's rows *and*
        // what a paste checks its sources against, so an arrangement error below must
        // not leave both pointing at the previous session's material.
        self.pool = outcome.pool_sources.clone().unwrap_or_default();
        self.source_tempos = outcome.source_tempos.iter().cloned().collect();
        self.pool_selected = self.pool_selected.min(self.pool.len().saturating_sub(1));
        self.focus = self.focus_that_exists();

        let timeline = match outcome.arrangement {
            Ok(timeline) => timeline,
            Err(e) => {
                self.status = format!("arrangement: {e}");
                return;
            }
        };

        let rate = self
            .snap
            .audio
            .as_ref()
            .map(|audio| audio.sample_rate)
            .filter(|rate| *rate > 0)
            .unwrap_or(SAMPLE_RATE as u32);

        // The faders are a **fold of the log**, not the shell's memory: the host
        // folded it for us, so a reload (or a replay) restores exactly what was
        // last asked for.
        self.apply_params(&outcome.params);

        let arrangement = Arrangement::from_host(
            &timeline,
            outcome.pool_sources.as_deref(),
            &mut self.sources,
            rate,
            self.origin.clone(),
        );

        // Sources that could not be read are the panel's problem to report.
        if let Some((id, error)) = self.sources.errors().last() {
            self.status = format!("source {id}: {error}");
        }

        self.adopt(arrangement, self.origin.clone());
    }

    /// Adopt the host's parameter values into the console. Only the mixer's
    /// params are understood; anything else is ignored rather than guessed at.
    fn apply_params(&mut self, params: &[(&'static str, &'static str, f32)]) {
        for (plugin, param, value) in params {
            if *plugin != "mixer" {
                continue;
            }

            match *param {
                "master.gain" => {
                    let master = self.mixer.channels();
                    self.mixer.set_value(master, *value);
                    continue;
                }
                "channels" => {
                    self.mixer.resize((*value).max(0.0) as usize);
                    continue;
                }
                _ => {}
            }

            let Some((channel, field)) = param.split_once('.') else {
                continue;
            };
            let Some(index) = channel
                .strip_prefix("ch")
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            if index >= self.mixer.channels() {
                continue;
            }

            match field {
                "gain" => self.mixer.set_value(index, *value),
                "mute" => self.mixer.set_muted(index, *value != 0.0),
                "solo" => self.mixer.set_soloed(index, *value != 0.0),
                _ => {}
            }
        }
    }

    /// Install a new arrangement, keeping the viewport when there is one (an
    /// edit should not throw the user's zoom and scroll away).
    fn adopt(&mut self, arrangement: Arrangement, origin: String) {
        let frames = arrangement.frames;
        self.origin = origin;
        self.arrangement = Some(arrangement);

        match self.view.as_mut() {
            Some(view) => {
                if view.start > frames {
                    view.start = 0;
                }
            }
            None => self.view = Some(View::new(frames)),
        }

        if self.focus == Panel::Mixer {
            self.focus = Panel::Timeline;
        }
        self.active_track = self.active_track.min(
            self.arrangement
                .as_ref()
                .map_or(0, |a| a.lanes.len().saturating_sub(1)),
        );
    }

    /// The deterministic frame the tests render: a synthetic snapshot with
    /// signal in it, so the output is reproducible and does not depend on an
    /// audio device or on timing.
    #[cfg(test)]
    fn demo() -> Self {
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.status = "demo loaded — mixer 4 ch, 8 log events, 0 underruns".to_string();
        app.selected = 1;
        app
    }

    /// The synthetic transport/meter state: 2.000 s in, playing, with signal.
    fn demo_snapshot() -> Snapshot {
        let audio = AudioStatus {
            sample_rate: SAMPLE_RATE as u32,
            channels: 2,
            requested_rate: SAMPLE_RATE as u32,
            ..AudioStatus::default()
        };

        let mut snap = Snapshot {
            frame: 96_000,
            seconds: 2.0,
            beat: 4.0,
            bpm: 120.0,
            playing: true,
            channel_count: 4,
            master: 0.50,
            audio: Some(audio),
            can_undo: true,
            ..Snapshot::default()
        };
        // The host publishes one meter per mounted mixer channel, so the snapshot's
        // channel vector is as long as the mount (empty before a mixer exists) — the
        // fixture sizes it before indexing, the way a live mount would.
        snap.channels = vec![0.0; 4];
        snap.channels[0] = 0.88;
        snap.channels[1] = 0.12;
        snap.channels[3] = 0.94;
        snap
    }

    /// A silent, idle host for the tests: the rendered state comes from the
    /// synthetic snapshot, so the host only exists to satisfy the type (and to
    /// answer the transport/arrange commands the tests exercise).
    #[cfg(test)]
    fn idle() -> Self {
        let host = HostHandle::spawn();
        // The test shell shares the live constructors' shape, so a field added
        // there must be added here (this is the compiler's checklist, not a fork).
        App {
            snap: host.snapshot(),
            host,
            status: String::new(),
            selected: 0,
            help: false,
            help_scroll: 0,
            prompt: None,
            last_take_seen: None,
            history: Vec::new(),
            history_at: 0,
            mouse: true,
            quit: false,
            last_command: None,
            buttons: Vec::new(),
            meter_rows: Vec::new(),
            mixer: Mixer::new(4),
            strips: mixer::Strips::default(),
            mixer_rect: Rect::default(),
            arrangement: None,
            sources: Sources::default(),
            record_source: None,
            view: None,
            mode: Mode::Normal,
            focus: Panel::Mixer,
            active_track: 0,
            origin: String::new(),
            next_id: 0,
            // Off by default: snapping silently would change every existing
            // gesture, so the grid is armed explicitly with `b`.
            grid: workflow::Grid::default(),
            clipboard: Vec::new(),
            clipboard_origin: 0,
            next_paste: 1,
            next_pool: 1,
            pool: Vec::new(),
            pool_selected: 0,
            source_tempos: std::collections::HashMap::new(),
            last_export_seen: None,
            prompt_prefill: None,
            panel: timeline::PanelRects::default(),
            pool_rect: Rect::default(),
        }
    }

    // -- update ------------------------------------------------------------

    /// Run a host command and record how long it blocked the UI thread. A shell
    /// that binds a command to a key owns this number: `TransportSeek` is
    /// O(target) in the host, so it is the one that will be felt.
    ///
    /// Returns whether the host **applied** it, and puts a refusal in the status
    /// line. A caller that wants to announce its own success must check this: a
    /// gesture that overwrites a refusal with "done" is how a refused edit looks
    /// like data loss.
    fn command(&mut self, label: &'static str, command: HostCommand) -> bool {
        let started = Instant::now();
        let outcome = self.host.execute(command);
        let elapsed = started.elapsed();

        match outcome {
            Ok(()) => {
                self.last_command = Some((label, elapsed));
                true
            }
            Err(e) => {
                self.status = format!("{label} refused: {e}");
                false
            }
        }
    }

    fn play(&mut self) {
        self.command("play", HostCommand::TransportPlay);
    }

    fn stop(&mut self) {
        self.command("stop", HostCommand::TransportStop);
    }

    fn toggle(&mut self) {
        if self.snap.playing {
            self.stop();
        } else {
            self.play();
        }
    }

    fn rewind(&mut self) {
        // Seek is "rebuild + render to target", so it is intended for a stopped
        // transport: stop first, then move the playhead home.
        self.stop();
        self.command("rewind", HostCommand::TransportSeek { frame: 0 });
    }

    /// Seek by whole seconds, clamped to the arrangement, landing on the armed
    /// grid. Stops first (see `rewind`). The step is the **session's** second
    /// (`sample_rate`), not the spike's default: a 44.1 kHz session must move 44 100
    /// frames per second, and the grid math already uses the session's map.
    fn nudge(&mut self, seconds: i64) {
        let rate = self.sample_rate() as i64;
        let raw = (self.snap.frame as i64 + seconds * rate).max(0) as u64;
        let target = self.clamp_frame(self.snap_frame(raw));

        self.stop();
        self.command("seek", HostCommand::TransportSeek { frame: target });
    }

    /// Keep a seek target inside the arrangement (`0..=frames`) — a playhead past
    /// the end is a state with nothing to show, and the transport tolerates it
    /// silently.
    fn clamp_frame(&self, frame: u64) -> u64 {
        match self.arrangement.as_ref() {
            Some(arrangement) => frame.min(arrangement.frames),
            None => frame,
        }
    }

    fn select(&mut self, delta: i64) {
        // Strips include the master, so the console's last strip is reachable.
        let count = self.mixer.strips() as i64;
        if count == 0 {
            return;
        }

        self.selected = (self.selected as i64 + delta).rem_euclid(count) as usize;
    }

    fn apply(&mut self, action: Button) {
        match action {
            Button::Play => self.play(),
            Button::Stop => self.stop(),
            Button::Rewind => self.rewind(),
        }
    }

    /// Translate a toolkit key into the shared workflow's neutral key. Everything
    /// toolkit-specific stops here: the workflow crate knows nothing about crossterm.
    fn workflow_key(key: &KeyEvent) -> Option<workflow::Key> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        Some(match key.code {
            KeyCode::Char(c) if ctrl => workflow::Key::Ctrl(c.to_ascii_lowercase()),
            KeyCode::Char(' ') => workflow::Key::Space,
            KeyCode::Char(c) => workflow::Key::Char(c),
            KeyCode::Enter => workflow::Key::Enter,
            KeyCode::Esc => workflow::Key::Esc,
            KeyCode::Backspace => workflow::Key::Backspace,
            KeyCode::Tab => workflow::Key::Tab,
            KeyCode::BackTab => workflow::Key::BackTab,
            KeyCode::Up => workflow::Key::Up,
            KeyCode::Down => workflow::Key::Down,
            KeyCode::Left => workflow::Key::Left,
            KeyCode::Right => workflow::Key::Right,
            KeyCode::Home => workflow::Key::Home,
            _ => return None,
        })
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // The command line is **modal** (as vim's `:` is): it takes every key while
        // it is open. Enter runs the line, Esc/Ctrl+c cancels, `↑`/`↓` walk the
        // history. This is the one place the shell handles *text*, so it stays here.
        if self.prompt.is_some() {
            match key.code {
                KeyCode::Enter => self.run_prompt(),
                KeyCode::Esc => {
                    self.prompt = None;
                    self.status = "command line cancelled".to_string();
                }
                KeyCode::Char('c') if ctrl => {
                    self.prompt = None;
                    self.status = "command line cancelled".to_string();
                }
                KeyCode::Backspace => {
                    if let Some(text) = self.prompt.as_mut() {
                        text.pop();
                    }
                }
                KeyCode::Up => self.history_step(-1),
                KeyCode::Down => self.history_step(1),
                KeyCode::Char(c) if !ctrl => {
                    if let Some(text) = self.prompt.as_mut() {
                        text.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        // The keymap overlay is modal too: while it is up, `j`/`k` scroll it and
        // anything else closes it or does nothing. (It cannot be dismissed by accident
        // into an edit — the help is where you read before you act.)
        if self.help {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    // The overlay scrolls **wrapped lines**, and a help row can wrap to
                    // two or three, so clamping to the entry count left the last
                    // bindings unreachable on a short terminal. Three lines per entry is
                    // the safe bound (the exact count needs the draw width).
                    self.help_scroll = (self.help_scroll + 1).min(workflow::help_len() * 3);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                }
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.help = false,
                _ => {}
            }
            return;
        }

        // Everything else is the **shared workflow**: one table, mapped to this
        // shell's methods. There is no second keymap here to drift from it.
        let Some(pressed) = Self::workflow_key(&key) else {
            return;
        };
        let Some(action) = workflow::action(pressed) else {
            return;
        };
        self.dispatch(action);
    }

    /// Run one workflow action. This is the shell's whole key surface: the workflow
    /// says what a key means, and every arm below is one method (and one `host v1`
    /// line) the iced shell will call the same way.
    fn dispatch(&mut self, action: Action) {
        match action {
            Action::PlayToggle => self.toggle(),
            Action::Stop => self.stop(),
            Action::RecordToggle => self.record_toggle(),
            Action::CycleRecordSource => self.cycle_record_source(),
            Action::Rewind => self.rewind(),
            Action::SeekSeconds(seconds) => self.nudge(seconds),
            Action::SeekClip(forward) => self.timeline_key(|app| app.seek_clip(forward)),
            Action::CycleFocus(direction) => self.cycle_focus(direction),
            Action::Vertical(direction) => self.vertical(direction),
            Action::Timeline(direction) => self.timeline_key(|app| app.timeline_move(direction)),
            Action::Zoom(direction) => self.zoom_or_ride(direction),
            Action::Fit => self.zero(),
            Action::MixerMute => self.mixer_key(|app| app.mute()),
            Action::MixerSolo => self.mixer_key(|app| app.solo()),
            Action::Visual => self.timeline_key(|app| app.visual()),
            Action::Split => self.timeline_key(|app| app.split_at_playhead()),
            Action::Delete => self.timeline_key(|app| app.delete_at_playhead()),
            Action::TrimStart => self.timeline_key(|app| app.trim_to_playhead(media::Edge::Start)),
            Action::TrimEnd => self.timeline_key(|app| app.trim_to_playhead(media::Edge::End)),
            Action::TrimToSelection => self.timeline_key(|app| app.trim_to_selection()),
            Action::Nudge(direction) => self.timeline_key(|app| app.nudge_clip(direction)),
            Action::SeekGrid(direction) => self.timeline_key(|app| app.seek_grid(direction)),
            Action::GridCycle => self.cycle_grid(),
            Action::Yank => self.timeline_key(|app| app.yank()),
            Action::Cut => self.timeline_key(|app| app.cut()),
            Action::Paste => self.timeline_key(|app| app.paste(false)),
            Action::PasteAppend => self.timeline_key(|app| app.paste(true)),
            Action::TrackAdd => self.timeline_key(|app| app.add_track()),
            Action::TrackRename => self.timeline_key(|app| app.rename_track_prompt()),
            Action::TrackDelete => self.timeline_key(|app| app.delete_track()),
            Action::ReorderTrack(direction) => self.timeline_key(|app| app.move_track(direction)),
            Action::PoolPlace => self.pool_key(|app| app.place_pool_source()),
            Action::Reverse => self.timeline_key(|app| app.reverse_clip()),
            Action::Normalize => self.timeline_key(|app| app.normalize_clip()),
            Action::Invert => self.timeline_key(|app| app.invert_clip()),
            Action::Silence => self.timeline_key(|app| app.silence_clip()),
            Action::TrimToContent => self.timeline_key(|app| app.trim_to_content()),
            Action::StretchToTempo => self.timeline_key(|app| app.stretch_to_tempo()),
            Action::ExportMix => self.export_prompt(),
            Action::MarkerSet => self.marker_prompt(),
            Action::MarkerSeek(direction) => self.seek_marker(direction),
            Action::ClipRename => self.rename_clip_prompt(),
            Action::MoveTrack(offset) => self.timeline_key(|app| app.move_clip_to_track(offset)),
            Action::Gain(direction) => {
                self.timeline_key(|app| app.step_clip_gain(direction as f32))
            }
            Action::Fade(fade_in) => self.timeline_key(|app| app.fade_to_playhead(fade_in)),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Prompt => {
                self.prompt = Some(String::new());
                self.prompt_prefill = None;
                self.history_at = self.history.len();
            }
            Action::Help => {
                self.help_scroll = 0;
                self.help = !self.help;
            }
            Action::ToggleMouseCapture => self.toggle_mouse(),
            Action::Quit => self.quit = true,
            Action::Cancel => self.cancel(),
        }
    }

    /// Esc: leave whatever state is on top — and never quit (the safety key must not
    /// be the destructive one).
    fn cancel(&mut self) {
        if self.help {
            self.help = false;
        } else if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
            if let Some(view) = self.view.as_mut() {
                view.selection = None;
            }
        }
    }

    /// Run `action` only when the timeline is the focused panel.
    fn timeline_key(&mut self, action: impl FnOnce(&mut App)) {
        if self.focus == Panel::Timeline {
            action(self);
        }
    }

    /// Run `action` only when the **pool panel** is the focused panel. A key pressed
    /// at the wrong panel explains itself rather than vanishing.
    fn pool_key(&mut self, action: impl FnOnce(&mut App)) {
        if self.focus == Panel::Pool {
            action(self);
        } else if self.pool.is_empty() {
            self.status = "no pool to browse — open one with `: pool <dir>`".to_string();
        } else {
            self.status = "`Tab` to the pool panel first (it owns the source list)".to_string();
        }
    }

    /// Run `action` only when the mixer is the focused panel.
    fn mixer_key(&mut self, action: impl FnOnce(&mut App)) {
        if self.focus == Panel::Mixer {
            action(self);
        }
    }

    /// `+` / `-` / `0` mean different things per panel — the payoff of the focus
    /// ring: zoom the timeline, or ride the selected fader.
    /// `+`/`-`: the *focused* panel's more/less — zoom the timeline, or ride the
    /// fader. One workflow action, two meanings, chosen by focus (never by the key).
    fn zoom_or_ride(&mut self, direction: i32) {
        match self.focus {
            Panel::Timeline => self.timeline_zoom(direction > 0),
            Panel::Mixer => self.ride(0.05 * direction as f32),
            // The pool has no "more/less"; walking it is `j`/`k`.
            Panel::Pool => {}
        }
    }

    fn zero(&mut self) {
        match self.focus {
            Panel::Timeline => self.timeline_fit(),
            Panel::Mixer => self.set_fader(self.selected, 1.0),
            Panel::Pool => {}
        }
    }

    /// Move the selected fader and tell the host. A fader ride is a stream of
    /// logged `set_param` commands — which is what automation is.
    fn ride(&mut self, delta: f32) {
        let strip = self.selected;
        let before = self.mixer.value(strip);
        self.mixer.nudge(strip, delta);
        let after = self.mixer.value(strip);
        if (after - before).abs() > f32::EPSILON {
            let param = self.mixer.gain_param(strip);
            self.set_param(&param, after);
        }
    }

    fn set_fader(&mut self, strip: usize, value: f32) {
        let before = self.mixer.value(strip);
        self.mixer.set_value(strip, value);
        let after = self.mixer.value(strip);
        if (after - before).abs() < f32::EPSILON {
            return;
        }
        let param = self.mixer.gain_param(strip);
        self.set_param(&param, after);
    }

    fn mute(&mut self) {
        let strip = self.selected;
        self.mixer.toggle_mute(strip);
        let param = self.mixer.mute_param(strip);
        let value = f32::from(u8::from(self.mixer.muted(strip)));
        self.set_param(&param, value);
    }

    fn solo(&mut self) {
        let strip = self.selected;
        self.mixer.toggle_solo(strip);
        let param = self.mixer.solo_param(strip);
        let value = f32::from(u8::from(self.mixer.soloed(strip)));
        self.set_param(&param, value);
    }

    /// Send one `set_param` **through the host's own text format**, so the
    /// console speaks the same language a script does and the engine logs it.
    fn set_param(&mut self, param: &str, value: f32) {
        let script = format!("host v1\nset_param mixer {param} {value:.4}\n");
        match host::parse_script(&script) {
            Ok(commands) => match commands.into_iter().next() {
                Some(command) => {
                    self.command("set_param", command);
                }
                None => self.status = format!("set_param {param}: nothing parsed"),
            },
            Err(e) => self.status = format!("set_param {param}: {e}"),
        }
    }

    /// `j`/`k`: the focused panel's vertical movement — mixer channel, or the
    /// timeline's active track.
    fn vertical(&mut self, direction: i32) {
        match self.focus {
            Panel::Mixer => self.select(direction as i64),
            Panel::Timeline => {
                let lanes = self.arrangement.as_ref().map_or(0, |a| a.lanes.len());
                if lanes == 0 {
                    return;
                }
                self.active_track = (self.active_track as i64 + direction as i64)
                    .clamp(0, lanes as i64 - 1) as usize;
            }
            Panel::Pool => {
                if self.pool.is_empty() {
                    return;
                }
                self.pool_selected = (self.pool_selected as i64 + direction as i64)
                    .clamp(0, self.pool.len() as i64 - 1)
                    as usize;
            }
        }
    }

    /// The focused panel if it still exists, else the nearest one that does — a focus
    /// on a panel that is no longer drawn would leave every key dead-but-explained
    /// until the user pressed `Tab`.
    fn focus_that_exists(&self) -> Panel {
        let exists = match self.focus {
            Panel::Mixer => true,
            Panel::Timeline => self.arrangement.is_some(),
            Panel::Pool => !self.pool.is_empty(),
        };
        if exists {
            return self.focus;
        }
        if self.arrangement.is_some() {
            Panel::Timeline
        } else if !self.pool.is_empty() {
            Panel::Pool
        } else {
            Panel::Mixer
        }
    }

    /// `Tab`/`Shift-Tab`: move the focus ring.
    fn cycle_focus(&mut self, direction: i32) {
        // The panels that *exist*: the mixer is always there, the timeline needs an
        // arrangement, and the pool needs a pool. The direction comes from the
        // workflow (Tab vs Shift-Tab), so a fourth panel will not need a new key.
        let mut panels = vec![Panel::Mixer];
        if self.arrangement.is_some() {
            panels.push(Panel::Timeline);
        }
        if !self.pool.is_empty() {
            panels.push(Panel::Pool);
        }
        let index = panels
            .iter()
            .position(|panel| *panel == self.focus)
            .unwrap_or(0) as i32;
        self.focus = panels[(index + direction).rem_euclid(panels.len() as i32) as usize];
    }

    // -- the timeline's update side ----------------------------------------

    /// `h`/`l`: scroll in normal mode, extend the selection in visual mode. The
    /// two behaviours are the whole point of the mode being visible.
    fn timeline_move(&mut self, direction: i64) {
        let Some(frames) = self.arrangement.as_ref().map(|a| a.frames) else {
            return;
        };
        let width = self.panel_width();
        let Some(view) = self.view.as_mut() else {
            return;
        };

        if self.mode == Mode::Visual {
            let head = view
                .selection
                .map(|(_, head)| head)
                .unwrap_or(self.snap.frame);
            let step = view.frames_per_cell as i64 * 4 * direction;
            let next = (head as i64 + step).clamp(0, frames as i64) as u64;
            let anchor = view.selection.map(|(anchor, _)| anchor).unwrap_or(head);
            view.selection = Some((anchor, next));
            // Keep the head in view.
            if next < view.start || next >= view.start + view.visible(width) {
                view.start = next.saturating_sub(view.visible(width) / 2);
                view.clamp(frames, width);
            }
        } else {
            view.scroll(frames, width, direction * (width as i64 / 8).max(1));
        }
    }

    fn timeline_zoom(&mut self, in_: bool) {
        let Some(frames) = self.arrangement.as_ref().map(|a| a.frames) else {
            return;
        };
        let width = self.panel_width();
        let anchor = self.snap.frame;
        if let Some(view) = self.view.as_mut() {
            view.zoom(frames, width, anchor, in_);
        }
    }

    fn timeline_fit(&mut self) {
        let Some(frames) = self.arrangement.as_ref().map(|a| a.frames) else {
            return;
        };
        let width = self.panel_width();
        if let Some(view) = self.view.as_mut() {
            view.fit(frames, width);
        }
    }

    /// `v`: enter visual mode, anchoring a selection at the playhead.
    fn visual(&mut self) {
        let Some(view) = self.view.as_mut() else {
            self.status =
                "visual mode needs a timeline: run with --wave <file> or --script <script>"
                    .to_string();
            return;
        };
        self.mode = Mode::Visual;
        view.selection = Some((self.snap.frame, self.snap.frame));
    }

    // -- editing: the shell speaks the host's own text format ---------------

    /// Dispatch several `arrange` lines as **one gesture**: the host applies them
    /// all-or-nothing and records one history entry, so the whole thing is one undo
    /// step. (A paste or a stretch will need exactly this; trim-to-selection is the
    /// first gesture that takes more than one op.)
    fn arrange_group(&mut self, lines: &[String]) -> bool {
        if lines.is_empty() {
            return false;
        }
        let mut commands = Vec::with_capacity(lines.len());
        for line in lines {
            match host::parse_arrange_line(line) {
                Ok((op, at_frame)) => commands.push(HostCommand::Arrange { op, at_frame }),
                Err(e) => {
                    self.status = format!("arrange: {e}");
                    return false;
                }
            }
        }
        let applied = self.command("arrange", HostCommand::Group { commands });
        self.refresh_arrangement();
        applied
    }

    /// Dispatch an `arrange` line **through the host's parser**, so the shell and
    /// the CLI share one vocabulary: the ops a key runs are the ops a script
    /// writes, and the host logs them like any other command.
    fn arrange(&mut self, line: &str) -> bool {
        match host::parse_arrange_line(line) {
            Ok((op, at_frame)) => {
                let applied = self.command("arrange", HostCommand::Arrange { op, at_frame });
                self.refresh_arrangement();
                applied
            }
            Err(e) => {
                self.status = format!("arrange: {e}");
                false
            }
        }
    }

    // -- the pool panel (alpha slice D3) --------------------------------------

    /// `Enter` in the pool panel: place the selected source on the active track at
    /// the playhead — a logged `add_clip` with the source's full length and the click
    /// guard on both new boundaries. With **no arrangement yet** it makes a track
    /// first (the plan's "pool before arrange" as an affordance, not an error string:
    /// the pool is where material comes from, so placing it is how a piece starts).
    fn place_pool_source(&mut self) {
        let Some(source) = self.pool.get(self.pool_selected).cloned() else {
            self.status = "no pool source selected".to_string();
            return;
        };
        if !Self::placeable_source(&source.id) {
            self.status = format!(
                "'{}' cannot be named in a `host v1` line (a space or a `#`) — rename the file in the pool",
                source.id
            );
            return;
        }
        if source.frames == 0 {
            self.status = format!("'{}' has no audio frames", source.id);
            return;
        }
        let frames = source.frames;
        let micro = MICRO_FADE.min(frames / 2);
        let id = self.mint_clip_id("pool");

        let Some(track) = self.active_lane_id() else {
            // Nothing to place into: mint a track in the same gesture.
            let track = self.free_track_id();
            let lines = vec![
                format!("add_track {track}"),
                format!(
                    "add_clip {track} {id} {} 0 {frames} 0 {micro} {micro} 1.0",
                    source.id
                ),
            ];
            if self.arrange_group(&lines) {
                self.active_track = 0;
                self.focus = Panel::Timeline;
                self.status = format!(
                    "placed {} on a new track {track} — {frames} frames at {} Hz",
                    source.id, source.sample_rate
                );
            }
            return;
        };

        let at = self.snap_frame(self.snap.frame);
        let line = format!(
            "add_clip {track} {id} {} 0 {frames} {at} {micro} {micro} 1.0",
            source.id
        );
        if self.arrange(&line) {
            self.status = format!(
                "placed {} on {track} at {at}{} — {frames} frames at {} Hz",
                source.id,
                snapped_note(self.snap.frame, at, self.grid.label()),
                source.sample_rate,
            );
        }
    }

    /// Mint a clip id for a gesture that adds clips (`pool.{n}`): the shell names the
    /// clips because the ids are logged. The counter is seeded above anything the
    /// session has **and only ever moves forward** — an undone `pool.1` is gone from
    /// the arrangement but still in the journal, and re-minting it would make journal
    /// recovery drop the second `add_clip` (the clip silently vanishes on a crash).
    fn mint_clip_id(&mut self, prefix: &str) -> String {
        let mut highest = 0;
        if let Some(arrangement) = self.arrangement.as_ref() {
            for lane in &arrangement.lanes {
                for clip in &lane.clips {
                    if let Some(n) = clip.id.strip_prefix(&format!("{prefix}."))
                        && let Ok(n) = n.parse::<u64>()
                    {
                        highest = highest.max(n);
                    }
                }
            }
        }
        let next = self.next_pool.max(highest + 1);
        self.next_pool = next + 1;
        format!("{prefix}.{next}")
    }

    /// Whether a pool source id can be **written into a `host v1` line**: the id is a
    /// file stem (`Pool::import` only rejects path separators), so a hand-filled pool
    /// can hold `my jam.wav` or `a#b.wav`, and a line naming either would be split (or
    /// truncated at the `#`) by the parser. The panel marks such a row instead of
    /// offering a key that cannot work.
    fn placeable_source(id: &str) -> bool {
        !id.is_empty() && id.split_whitespace().count() == 1 && !id.contains('#')
    }

    // -- tracks (alpha slice D2) ---------------------------------------------

    /// The first free `t{n}` — the shell mints track ids (they are logged), so it
    /// fills a gap rather than counting up blindly.
    fn free_track_id(&self) -> String {
        let taken: Vec<&str> = self
            .arrangement
            .as_ref()
            .map(|arrangement| {
                arrangement
                    .lanes
                    .iter()
                    .map(|lane| lane.id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        for n in 0.. {
            let candidate = format!("t{n}");
            if !taken.contains(&candidate.as_str()) {
                return candidate;
            }
        }
        unreachable!("a free track id always exists")
    }

    /// `a`: add a track and make it the active one (you just made it, so that is
    /// where the next clip goes).
    fn add_track(&mut self) {
        let Some(arrangement) = self.arrangement.as_ref() else {
            self.status = "nothing to add a track to — load a timeline first".to_string();
            return;
        };
        let track = self.free_track_id();
        let index = arrangement.lanes.len();
        let channels = self.snap.channel_count;
        if self.arrange(&format!("add_track {track}")) {
            self.active_track = index;
            // The value op is permissive; the *mixer* decides whether the track can
            // render (track `ti` feeds `ch{ti}`). Announcing "its mixer channel is
            // 4" when the mixer has four channels is a lie the user only discovers
            // when the transport fails.
            self.status = if channels > 0 && index >= channels {
                format!(
                    "added track {track} — the mixer has {channels} channels, so it will not \
                     render until it is wider (`: mount mixer channels={}`)",
                    index + 1,
                )
            } else {
                format!("added track {track} — `R` renames it, ch{index} is its mixer channel")
            };
        }
    }

    /// `o`: stop the take in progress, or open the command line to name a new one.
    ///
    /// The iced shell auto-names takes (`take-N`, checked against the pool it reads);
    /// this shell asks, because a take's id is **declared state** — the session
    /// replays without the device — so the name is a real decision and the prompt is
    /// where this shell makes decisions. Stopping needs no name, so it is one key in
    /// both shells: both send the same `record` verb.
    fn record_toggle(&mut self) {
        // The decision is **engine state**, read from the host at the moment of the
        // press — not from the status line, which is a message channel an export or a
        // source error overwrites. The event loop refreshes `snap` every frame, but a
        // key press is not a frame.
        self.snap = self.host.snapshot();
        match self.record_intent() {
            RecordIntent::Stop => {
                // Stop needs no name, so it runs on the key. Refresh afterwards the
                // way the command line does, so the finished take's report and the
                // new pool sources are visible rather than waiting for the next
                // command.
                if self.command("record stop", HostCommand::RecordStop) {
                    self.refresh_arrangement();
                }
            }
            RecordIntent::Name => {
                // The line is prefilled with the **armed source** and the shell's own
                // next take id, so the typed text is what runs — the source is visible
                // before Enter, not appended behind the user's back.
                let take_id = self.next_take_id();
                let prefill = match &self.record_source {
                    Some(name) => format!("record {take_id} source={name}"),
                    None => format!("record {take_id}"),
                };
                self.prompt = Some(prefill.clone());
                self.prompt_prefill = Some(prefill);
                self.history_at = self.history.len();
                self.status = match &self.record_source {
                    Some(name) => format!(
                        "recording from '{name}' — edit the take id if you like, then Enter"
                    ),
                    None => {
                        "name the take and press Enter (e.g. `record take-1`)".to_string()
                    }
                };
            }
        }
    }

    /// What `o` does, as the host's state declares it — a pure function of the
    /// snapshot, so the decision is testable without a device.
    fn record_intent(&self) -> RecordIntent {
        if self.snap.recording.is_some() {
            RecordIntent::Stop
        } else {
            RecordIntent::Name
        }
    }

    /// `A`: arm the source the next take captures — `default input → the rig's first
    /// declared source → … → default`.
    ///
    /// Shell state, like the take id: the host publishes the declarations (`snap.sources`)
    /// and binds the name only when the take starts. A rig with nothing declared cycles
    /// to `default` and says so, rather than arming a name that could never bind.
    fn cycle_record_source(&mut self) {
        self.snap = self.host.snapshot();
        // Cloned: the names are read to choose the next one, then `record_source` is
        // written — a handful of short strings, once per keypress.
        let names = self.snap.sources.clone();
        self.record_source = match &self.record_source {
            None => names.first().cloned(),
            Some(current) => {
                let idx = names.iter().position(|n| n == current);
                match idx {
                    Some(i) if i + 1 < names.len() => Some(names[i + 1].clone()),
                    // The last declared source (or one no longer declared) goes back
                    // to the default input.
                    _ => None,
                }
            }
        };
        self.status = match &self.record_source {
            Some(name) => format!("record source: {name} — the next `o` captures it"),
            None if names.is_empty() => {
                "record source: default input (no sources declared — try `source add …`)"
                    .to_string()
            }
            None => "record source: default input".to_string(),
        };
    }

    /// The next `take-N` this session does not already hold — the id the record prompt
    /// is prefilled with (the host needs the id before the take starts).
    fn next_take_id(&self) -> String {
        let mut next = 1u32;
        for id in &self.snap.pool_ids {
            // `take-7.ch0` names take 7 in the capture convention.
            let Some(number) = id
                .strip_prefix("take-")
                .and_then(|rest| rest.split(['.', '-']).next())
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            next = next.max(number + 1);
        }
        format!("take-{next}")
    }

    /// `R`: open the command line **prefilled** with the rename line, so the name is
    /// typed through the host's own format (one prompt, one parser, no second text
    /// widget to keep in sync).
    fn rename_track_prompt(&mut self) {
        let Some(track) = self.active_lane_id() else {
            self.status = "no active track to rename".to_string();
            return;
        };
        let prefill = format!("arrange rename_track {track} ");
        self.prompt = Some(prefill.clone());
        self.prompt_prefill = Some(prefill);
        self.history_at = self.history.len();
        self.status = format!("rename {track}: type the new name and press Enter");
    }

    /// `X`: **export the whole arrangement** — opens the command line prefilled with
    /// `export <dir>/mix.wav f32`, so what will be written (and in which format) is on
    /// screen before anything is. Enter runs it; typing `s16` before Enter asks for the
    /// dithered 16-bit file instead. The default path is beside the script the session
    /// was opened from (or `mix.wav` in the working directory for the demo snapshot).
    fn export_prompt(&mut self) {
        let path = match std::path::Path::new(&self.origin) {
            p if p.parent().is_some_and(|d| !d.as_os_str().is_empty()) => {
                p.parent().expect("checked").join("mix.wav")
            }
            _ => std::path::PathBuf::from("mix.wav"),
        };
        let prefill = format!("export {} f32", path.display());
        self.prompt = Some(prefill.clone());
        self.prompt_prefill = Some(prefill);
        self.history_at = self.history.len();
        self.status = "export the whole arrangement: edit the path/format, then Enter".to_string();
    }

    /// `'`: **name a point on the timeline** — opens the command line prefilled with
    /// `arrange set_marker <playhead> ` (the name is the missing word), so a section
    /// list is built from the keyboard without memorising frame numbers.
    fn marker_prompt(&mut self) {
        let prefill = format!("arrange set_marker {} ", self.snap_frame(self.snap.frame));
        self.prompt = Some(prefill.clone());
        self.prompt_prefill = Some(prefill);
        self.history_at = self.history.len();
        self.status = "name this marker: one word (dashes are fine), then Enter".to_string();
    }

    /// `;` / `"`: jump the playhead to the **next / previous marker** — the section
    /// navigation a long piece needs. Reports the marker's name, and says so when there
    /// is none (a silent no-op reads as a broken key).
    fn seek_marker(&mut self, direction: i32) {
        let Some(arrangement) = self.arrangement.as_ref() else {
            self.status = "no arrangement to navigate".to_string();
            return;
        };
        let here = self.snap.frame;
        let marker = match direction {
            d if d > 0 => arrangement.marker_after(here),
            _ => arrangement.marker_before(here),
        };
        match marker {
            Some(m) => {
                let (frame, name) = (m.at_frame, m.name.clone());
                // The playhead lands **exactly** on the marker (snapping it to the grid
                // would move the very point the key is for).
                self.snap.frame = frame;
                let seconds = frame as f64 / self.sample_rate() as f64;
                self.status = format!(
                    "marker {name} @ {:02}:{:05.2} (frame {frame})",
                    (seconds / 60.0) as u64,
                    seconds % 60.0
                );
            }
            None => {
                self.status = if self
                    .arrangement
                    .as_ref()
                    .map(|a| a.markers.is_empty())
                    .unwrap_or(true)
                {
                    "no markers yet — `'` names one at the playhead".to_string()
                } else if direction > 0 {
                    "no marker after the playhead".to_string()
                } else {
                    "no marker before the playhead".to_string()
                };
            }
        }
    }

    /// `C`: **name the clip under the playhead** — the same prefilled-prompt shape as a
    /// marker, so a label never has to be typed blind.
    fn rename_clip_prompt(&mut self) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let prefill = format!("arrange rename_clip {track} {} ", clip.id);
        self.prompt = Some(prefill.clone());
        self.prompt_prefill = Some(prefill);
        self.history_at = self.history.len();
        self.status = format!("name {}: one word, then Enter", clip.id);
    }

    /// `D`: delete the active track **and its clips**, as one gesture — the op drops
    /// the track, so the clips have to go explicitly, and one `u` brings both back.
    fn delete_track(&mut self) {
        let Some(arrangement) = self.arrangement.as_ref() else {
            self.status = "no arrangement to delete a track from".to_string();
            return;
        };
        let Some(lane) = arrangement.lanes.get(self.active_track) else {
            self.status = "no active track to delete".to_string();
            return;
        };
        let (track, clips) = (lane.id.clone(), lane.clips.len());
        let mut lines: Vec<String> = lane
            .clips
            .iter()
            .map(|clip| format!("delete {track} {}", clip.id))
            .collect();
        lines.push(format!("remove_track {track}"));
        if self.arrange_group(&lines) {
            self.active_track = self.active_track.saturating_sub(1);
            self.status = format!(
                "deleted track {track} and {clips} clip{} (one `u` brings them back)",
                if clips == 1 { "" } else { "s" },
            );
        }
    }

    /// `{`/`}`: move the active track up / down. The mixer channel each track feeds
    /// follows its position, so the console reorders with the timeline — and the
    /// *view* stays on the track that moved.
    fn move_track(&mut self, direction: i32) {
        let Some(arrangement) = self.arrangement.as_ref() else {
            self.status = "no arrangement to reorder".to_string();
            return;
        };
        let Some(lane) = arrangement.lanes.get(self.active_track) else {
            self.status = "no active track to move".to_string();
            return;
        };
        let track = lane.id.clone();
        let from = self.active_track as i64;
        let to = from + direction as i64;
        if to < 0 || to as usize >= arrangement.lanes.len() {
            self.status = format!(
                "track {track} is already {}",
                if direction < 0 { "first" } else { "last" }
            );
            return;
        }
        if self.arrange(&format!("move_track {track} {to}")) {
            self.active_track = to as usize;
            self.status =
                format!("moved track {track} to position {to} (its mixer channel follows)");
        }
    }

    /// The active lane's id (a track is a mixer channel `ch{ti}`).
    fn active_lane_id(&self) -> Option<String> {
        self.arrangement
            .as_ref()?
            .lanes
            .get(self.active_track)
            .map(|lane| lane.id.clone())
    }

    // -- the clipboard (alpha slice D) ---------------------------------------

    /// The clips a copy takes: those **intersecting the visual selection** on the
    /// active track, or the clip under the playhead when there is no selection (a
    /// selection with no width means "here", so it falls back to the same rule).
    fn clipboard_scope(&self) -> Vec<Placed> {
        let Some(arrangement) = self.arrangement.as_ref() else {
            return Vec::new();
        };
        let Some(lane) = arrangement.lanes.get(self.active_track) else {
            return Vec::new();
        };
        let selection = self.view.as_ref().and_then(|view| view.selection);
        let range = selection.map(|(anchor, head)| (anchor.min(head), anchor.max(head)));
        match range {
            Some((from, to)) if to > from => lane
                .clips
                .iter()
                .filter(|clip| clip.at_frame < to && clip.end_frame() > from)
                .cloned()
                .collect(),
            _ => arrangement
                .clip_at(self.active_track, self.snap.frame)
                .cloned()
                .into_iter()
                .collect(),
        }
    }

    /// Take the clips into the clipboard, normalised to their earliest start.
    /// Returns what was taken (empty when there was nothing to copy).
    fn fill_clipboard(&mut self) -> Vec<Placed> {
        let clips = self.clipboard_scope();
        if clips.is_empty() {
            self.status =
                "nothing to copy — select a range with `v`, or put the playhead on a clip"
                    .to_string();
            return Vec::new();
        }
        self.clipboard_origin = clips.iter().map(|clip| clip.at_frame).min().unwrap_or(0);
        self.clipboard = clips.iter().cloned().map(|clip| (0, clip)).collect();
        self.mode = Mode::Normal;
        if let Some(view) = self.view.as_mut() {
            view.selection = None;
        }
        clips
    }

    /// `y`: copy the selection (or the clip under the playhead) to the shell's
    /// clipboard — a value, so nothing is logged and nothing can be undone.
    fn yank(&mut self) {
        let clips = self.fill_clipboard();
        if clips.is_empty() {
            return;
        }
        let span = clips
            .iter()
            .map(|clip| clip.end_frame())
            .max()
            .unwrap_or(0)
            .saturating_sub(self.clipboard_origin);
        self.status = format!(
            "copied {} clip{} ({} frames) — `p` pastes at the playhead, `P` appends",
            clips.len(),
            if clips.len() == 1 { "" } else { "s" },
            span,
        );
    }

    /// `c`: copy **and** delete, as one gesture — the clipboard is a value, the
    /// delete is the log entry, and the whole cut is one undo step.
    fn cut(&mut self) {
        let clips = self.fill_clipboard();
        if clips.is_empty() {
            return;
        }
        let Some(lane) = self
            .arrangement
            .as_ref()
            .and_then(|arrangement| arrangement.lanes.get(self.active_track))
        else {
            return;
        };
        let track = lane.id.clone();
        let lines: Vec<String> = clips
            .iter()
            .map(|clip| format!("delete {track} {}", clip.id))
            .collect();
        if self.arrange_group(&lines) {
            self.status = format!(
                "cut {} clip{} — `p` pastes them back (one `u` undoes the cut)",
                clips.len(),
                if clips.len() == 1 { "" } else { "s" },
            );
        }
    }

    /// The next free `paste.{n}`: the shell mints the ids (they are part of the
    /// log), so it starts above anything the session already names that way and
    /// never reuses one within a run.
    fn max_paste_id(&self) -> u64 {
        let mut highest = 0;
        if let Some(arrangement) = self.arrangement.as_ref() {
            for lane in &arrangement.lanes {
                for clip in &lane.clips {
                    if let Some(n) = clip.id.strip_prefix("paste.")
                        && let Ok(n) = n.parse::<u64>()
                    {
                        highest = highest.max(n);
                    }
                }
            }
        }
        highest
    }

    /// `p` (`append = false`) at the playhead / `P` (`append = true`) after the
    /// active track's last clip — one **gesture**, so the whole paste is one undo
    /// step. New boundaries get a default micro-fade (a copied fade is kept), and
    /// the ids are minted here because they are logged.
    fn paste(&mut self, append: bool) {
        if self.clipboard.is_empty() {
            self.status = "the clipboard is empty — `y` copies a clip or a selection".to_string();
            return;
        }
        let Some(arrangement) = self.arrangement.as_ref() else {
            self.status = "nothing to paste into — load a timeline first".to_string();
            return;
        };
        let Some(lane) = arrangement.lanes.get(self.active_track) else {
            self.status = "no active track".to_string();
            return;
        };
        // Appending follows the track's own last clip; on an empty track it
        // appends to the **piece** (the arrangement's end), which is what "add
        // this at the end" means when there is nothing to follow.
        let target = if append {
            lane.clips
                .iter()
                .map(|clip| clip.end_frame())
                .max()
                .unwrap_or(arrangement.frames)
        } else {
            // **Not** clamped to the arrangement: a paste past the end is how the
            // piece grows. (A *seek* is clamped; there, past the end is a state
            // with nothing to show.)
            self.snap_frame(self.snap.frame)
        };

        self.next_paste = self.next_paste.max(self.max_paste_id() + 1);
        let ids: Vec<String> = (0..self.clipboard.len())
            .map(|n| format!("paste.{}", self.next_paste + n as u64))
            .collect();

        // A collision is refused **whole** before anything is built: the host's
        // group is atomic, but a silent no-op would look like a broken key.
        for (n, (delta, _)) in self.clipboard.iter().enumerate() {
            let Some(lane) = self.lane_at_delta(*delta) else {
                self.status =
                    "paste: the clipboard spans tracks this session does not have".to_string();
                return;
            };
            if lane.clips.iter().any(|existing| existing.id == ids[n]) {
                self.status = format!("paste: a clip is already called {}", ids[n]);
                return;
            }
        }

        let mut lines = Vec::with_capacity(self.clipboard.len());
        for (n, (delta, clip)) in self.clipboard.iter().enumerate() {
            let Some(track) = self.lane_at_delta(*delta).map(|lane| lane.id.clone()) else {
                self.status =
                    "paste: the clipboard spans tracks this session does not have".to_string();
                return;
            };
            let at = target + clip.at_frame.saturating_sub(self.clipboard_origin);
            // A copied fade is kept; a **bare** boundary gets the click guard.
            // The guard is what shrinks when the pair is tight: the model requires
            // `fade_in + fade_out <= src_len`, so a kept `fade_in = src_len` (legal,
            // and reachable with `f` at the clip's end) must leave the new fade-out
            // zero rather than mint an op the host refuses. A one- or two-frame clip
            // has no room for a guard at all.
            let micro = MICRO_FADE.min(clip.src_len / 2);
            let mut fade_in = clip.fade_in;
            let mut fade_out = clip.fade_out;
            if clip.fade_in == 0 {
                fade_in = micro.min(clip.src_len.saturating_sub(fade_out));
            }
            if clip.fade_out == 0 {
                fade_out = micro.min(clip.src_len.saturating_sub(fade_in));
            }
            let looped = clip
                .loop_len
                .map(|len| format!(" {len}"))
                .unwrap_or_default();
            lines.push(format!(
                "add_clip {track} {} {} {} {} {at} {} {} {:.6}{looped}",
                ids[n], clip.source_id, clip.src_start, clip.src_len, fade_in, fade_out, clip.gain,
            ));
            // `add_clip` has no direction operand (a clip is added forward and the
            // `reverse` op flips it), so a reversed clipboard entry carries its own
            // re-reverse **in the same gesture** — otherwise a paste would silently
            // play it forwards (the gate caught exactly that).
            if clip.reversed {
                lines.push(format!("reverse {track} {}", ids[n]));
            }
        }

        // A paste names pool sources: one this session does not have would be
        // logged by the media layer and then dropped by the panel (and refuse to
        // wire), so say it *here*, before minting anything.
        if let Some(unnameable) = self.clipboard.iter().find_map(|(_, clip)| {
            (!Self::placeable_source(&clip.source_id)).then(|| clip.source_id.clone())
        }) {
            self.status = format!(
                "paste: the source id '{unnameable}' cannot be written into a `host v1` line"
            );
            return;
        }
        if let Some(missing) = self.clipboard.iter().find_map(|(_, clip)| {
            (!self.pool.iter().any(|source| source.id == clip.source_id))
                .then(|| clip.source_id.clone())
        }) {
            self.status = format!(
                "paste: this session's pool has no source '{missing}' — the clipboard came from another session (import it, or copy from a clip here)"
            );
            return;
        }

        self.next_paste += self.clipboard.len() as u64;
        let count = lines.len();
        if self.arrange_group(&lines) {
            // The snap note belongs to the playhead branch: an append target is
            // the track's last clip, not a quantized playhead.
            let snapped = if append {
                String::new()
            } else {
                snapped_note(self.snap.frame, target, self.grid.label())
            };
            self.status = format!(
                "{} {count} clip{} at {target}{snapped} (one `u` undoes the paste)",
                if append { "appended" } else { "pasted" },
                if count == 1 { "" } else { "s" },
            );
        }
    }

    /// The lane `delta` tracks away from the active one (`None` when the clipboard
    /// names a track this session does not have).
    fn lane_at_delta(&self, delta: i32) -> Option<&timeline::Lane> {
        let lanes = &self.arrangement.as_ref()?.lanes;
        let index = self.active_track as i32 + delta;
        if index < 0 || index as usize >= lanes.len() {
            return None;
        }
        lanes.get(index as usize)
    }

    /// `x`: razor-split the clip under the playhead on the active track. Both
    /// halves are named here and logged with the op — ids are part of the log.
    fn split_at_playhead(&mut self) {
        let Some((track, clip_id)) = self.clip_under_playhead() else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };

        self.next_id += 1;
        let left = format!("{clip_id}-a{}", self.next_id);
        let right = format!("{clip_id}-b{}", self.next_id);
        let at = self.snap_frame(self.snap.frame);

        self.status = format!(
            "split {clip_id} at {at}{} → {left} + {right}",
            snapped_note(self.snap.frame, at, self.grid.label()),
        );
        self.arrange(&format!(
            "razor_split {track} {clip_id} {left} {right} {at}"
        ));
    }

    /// `d`: delete the clip under the playhead on the active track.
    fn delete_at_playhead(&mut self) {
        let Some((track, clip_id)) = self.clip_under_playhead() else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        self.status = format!("delete {clip_id}");
        self.arrange(&format!("delete {track} {clip_id}"));
    }

    /// `u`: undo the last arrangement edit. The host does it by replaying the
    /// log; the shell re-reads both values the log defines.
    fn undo(&mut self) {
        if !self.snap.can_undo {
            self.status = "nothing to undo".to_string();
            return;
        }
        self.command("undo", HostCommand::Undo);
        self.refresh_arrangement();
    }

    /// `Ctrl+r`: redo the most recently undone edit, again by replaying the log.
    fn redo(&mut self) {
        if !self.snap.can_redo {
            self.status = "nothing to redo".to_string();
            return;
        }
        self.command("redo", HostCommand::Redo);
        self.refresh_arrangement();
    }

    /// The (track id, clip id) at the playhead on the active track.
    fn clip_under_playhead(&self) -> Option<(String, String)> {
        self.active_clip_at(self.snap.frame)
            .map(|(track, clip)| (track, clip.id))
    }

    /// The clip under `frame` on the active track, as an edit needs it: the track
    /// id and the clip itself (cloned — a copy is one small struct, and the caller
    /// is about to send the host a command that re-reads everything anyway).
    fn active_clip_at(&self, frame: u64) -> Option<(String, Placed)> {
        let arrangement = self.arrangement.as_ref()?;
        let lane = arrangement.lanes.get(self.active_track)?;
        let clip = arrangement.clip_at(self.active_track, frame)?;
        Some((lane.id.clone(), clip.clone()))
    }

    /// The lane id `offset` tracks away from the active one, when it exists.
    fn lane_id_offset(&self, offset: i32) -> Option<String> {
        let lanes = &self.arrangement.as_ref()?.lanes;
        let index = self.active_track as i32 + offset;
        if index < 0 || index as usize >= lanes.len() {
            return None;
        }
        Some(lanes[index as usize].id.clone())
    }

    /// `<` / `>`: trim the clip under the playhead so one edge lands on it. The op
    /// is a **delta** in frames (`trim <track> <clip> start|end <by>`), which is
    /// exactly what a playhead-relative trim is; a playhead already on the edge is
    /// a no-op. Trimming the *start* moves `at_frame` and `src_start` together, so
    /// the audio does not shift under the cut.
    fn trim_to_playhead(&mut self, edge: media::Edge) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let at = self.snap_frame(self.snap.frame);
        let playhead = at as i64;
        let (name, by) = match edge {
            media::Edge::Start => ("start", playhead - clip.at_frame as i64),
            media::Edge::End => ("end", playhead - clip.end_frame() as i64),
        };
        if by == 0 {
            self.status = format!("{}: the playhead is already its {name}", clip.id);
            return;
        }
        self.status = format!(
            "trim {} {name} by {by:+} frames{}",
            clip.id,
            snapped_note(self.snap.frame, at, self.grid.label()),
        );
        self.arrange(&format!("trim {track} {} {name} {by}", clip.id));
    }

    /// `t` in visual mode: trim the clip to the **selection** — the modal payoff
    /// ("select, then act"). The clip is the one the selection *starts* in on the
    /// active track, and the selection's edges become the clip's, clamped to it:
    /// a selection can only shrink a clip, never extend it. That is up to two
    /// logged ops (one per moving edge, so `u` unwinds it in steps), and the
    /// action consumes the selection — back to normal mode, as Helix does.
    fn trim_to_selection(&mut self) {
        let Some((anchor, head)) = self.view.as_ref().and_then(|view| view.selection) else {
            self.status = "no selection — v anchors one at the playhead".to_string();
            return;
        };
        let (sel_start, sel_end) = (anchor.min(head), anchor.max(head));
        let Some((track, clip)) = self.active_clip_at(sel_start) else {
            self.status = format!("no clip on the active track at frame {sel_start}");
            return;
        };
        let new_start = sel_start.max(clip.at_frame);
        let new_end = sel_end.min(clip.end_frame());
        if new_end <= new_start {
            self.status = format!("the selection does not overlap {}", clip.id);
            return;
        }

        let (start_by, end_by) = (
            new_start as i64 - clip.at_frame as i64,
            new_end as i64 - clip.end_frame() as i64,
        );
        self.status = format!(
            "trim {} to {new_start}–{new_end} ({} frames, one undo)",
            clip.id,
            new_end - new_start
        );
        // One gesture: both edges (when both move) are a single history entry, so
        // `u` after a trim-to-selection restores the clip whole.
        let mut lines = Vec::new();
        if start_by != 0 {
            lines.push(format!("trim {track} {} start {start_by}", clip.id));
        }
        if end_by != 0 {
            lines.push(format!("trim {track} {} end {end_by}", clip.id));
        }
        self.arrange_group(&lines);

        self.mode = Mode::Normal;
        if let Some(view) = self.view.as_mut() {
            view.selection = None;
        }
    }

    /// `H` / `L`: move the clip under the playhead one **grid step** earlier /
    /// later — one beat when no grid is armed. The step comes from the session's
    /// tempo map (beats × bpm × sample rate), so a nudge is musical rather than a
    /// cell width, and moving left clamps at frame 0. The *target* is quantized, so
    /// a clip that sat off the grid lands on it.
    fn nudge_clip(&mut self, direction: i32) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let step = self.grid_step_frames(clip.at_frame);
        let raw = (clip.at_frame as i64 + direction as i64 * step).max(0) as u64;
        let target = self.snap_frame(raw);
        self.status = format!(
            "move {} {:+} {} ({step} frames) → {target} [grid {}]",
            clip.id,
            direction,
            if self.grid.is_on() {
                "grid step"
            } else {
                "beat"
            },
            self.grid.label(),
        );
        self.arrange(&format!("move_clip {track} {} {target}", clip.id));
    }

    /// `J` / `K`: move the clip under the playhead to the track below / above,
    /// keeping its position in time, and follow it with the cursor so the panel
    /// stays on the clip.
    fn move_clip_to_track(&mut self, offset: i32) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let Some(target) = self.lane_id_offset(offset) else {
            let which = if offset > 0 { "below" } else { "above" };
            self.status = format!("there is no track {which}");
            return;
        };
        self.status = format!("move {} {track} → {target} at {}", clip.id, clip.at_frame);
        self.arrange(&format!(
            "move_clip_to_track {track} {} {target} {}",
            clip.id, clip.at_frame
        ));
        self.active_track = (self.active_track as i32 + offset).max(0) as usize;
    }

    /// `g` / `G`: step the clip's gain by a dB. The op carries a linear
    /// multiplier, so the step is taken in dB (where a step means something) and
    /// converted back; the range is the console fader's (-60…+12 dB).
    fn step_clip_gain(&mut self, step_db: f32) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let current_db = if clip.gain > 0.0 {
            20.0 * clip.gain.log10()
        } else {
            f32::NEG_INFINITY
        };
        let next_db = (current_db + step_db).clamp(-60.0, 12.0);
        if !next_db.is_finite() {
            self.status = format!("{}: gain is silent (-∞ dB)", clip.id);
            return;
        }
        if (next_db - current_db).abs() < 1e-3 {
            self.status = format!("{} gain is already at {current_db:+.1} dB", clip.id);
            return;
        }
        let gain = 10f32.powf(next_db / 20.0);
        self.status = format!("{} gain {next_db:+.1} dB ({gain:.4})", clip.id);
        self.arrange(&format!("set_clip_gain {track} {} {gain:.6}", clip.id));
    }

    // -- utility gestures (alpha slice E1) -----------------------------------

    /// `V`: play the clip backwards (or forwards again). One log line, one undo —
    /// and the *panel* mirrors its envelope, because the value it holds says which
    /// way the engine will read.
    fn reverse_clip(&mut self) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        if clip.loop_len.is_some() {
            self.status = format!(
                "{} is a looped clip — a reversed loop has no representable phase",
                clip.id
            );
            return;
        }
        let direction = if clip.reversed {
            "forwards"
        } else {
            "backwards"
        };
        if self.arrange(&format!("reverse {track} {}", clip.id)) {
            self.status = format!("{} plays {direction}", clip.id);
        }
    }

    /// Set a clip's gain from a linear multiplier, with one status line — the shared
    /// tail of the three gain gestures below.
    fn set_clip_gain(&mut self, track: &str, clip: &Placed, gain: f32, what: &str) {
        if (gain - clip.gain).abs() < 1e-6 {
            self.status = format!("{} is already {what}", clip.id);
            return;
        }
        if self.arrange(&format!("set_clip_gain {track} {} {gain:.6}", clip.id)) {
            self.status = format!("{} {what} (gain {gain:.4})", clip.id);
        }
    }

    /// `U`: normalize — scale the clip so its **loudest sample** hits full scale.
    /// The peak comes from the pyramid the panel already has (a scan of the clip's
    /// source region, not a re-read of the audio), and the result is one logged
    /// `set_clip_gain`. A silent clip is refused rather than amplified to noise.
    fn normalize_clip(&mut self) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let (lo, hi) = clip.source_region();
        let peak = clip.source.peak_of(lo, hi);
        if peak <= 1e-5 {
            self.status = format!("{} is silent — nothing to normalize", clip.id);
            return;
        }
        let gain: f32 = 1.0 / peak;
        // The console's fader range is the ceiling for a clip gain too, so a very
        // quiet clip is lifted as far as the range allows and the status says so.
        let ceiling = 10f32.powf(12.0 / 20.0);
        let gain = gain.min(ceiling);
        let note = if (gain * peak - 1.0).abs() > 1e-3 {
            format!("normalized to the +12 dB ceiling (peak was {peak:.4})")
        } else {
            format!("normalized from peak {peak:.4}")
        };
        self.set_clip_gain(&track, &clip, gain, &note);
    }

    /// `i`: invert the clip's polarity — a gain of −1 (one logged line, reversible
    /// with the same key).
    fn invert_clip(&mut self) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        if clip.gain == 0.0 {
            self.status = format!("{} is silent — no polarity to flip", clip.id);
            return;
        }
        let gain = -clip.gain;
        self.set_clip_gain(&track, &clip, gain, "inverted (polarity flipped)");
    }

    /// `E`: silence the clip — gain 0, keeping its span, fades and place in the
    /// arrangement (delete is a different key, and a different intent).
    fn silence_clip(&mut self) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        self.set_clip_gain(&track, &clip, 0.0, "silenced");
    }

    /// `T`: trim the clip to its **audible content** — scan the peak pyramid in from
    /// each edge until a bin is above the noise floor, then trim both edges to it in
    /// one gesture. The scan is in *source* frames; the timeline deltas are mirrored
    /// for a reversed clip, so "the start of the clip" means what the listener hears.
    fn trim_to_content(&mut self) {
        const FLOOR: f32 = 1e-4; // ≈ −80 dBFS: below this a bin is silence
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        if clip.loop_len.is_some() {
            self.status = format!("{} is a looped clip — trim its loop first", clip.id);
            return;
        }
        let (lo, hi) = clip.source_region();
        // Walk in from both ends of the region, one peak bin at a time.
        let bin = media::PEAK_BASE_BIN as u64;
        let mut first = lo;
        let mut last = hi; // exclusive
        while first < last {
            let end = (first + bin).min(last);
            if clip.source.peak_of(first, end) > FLOOR {
                break;
            }
            first = end;
        }
        while last > first {
            let start = last.saturating_sub(bin).max(first);
            if clip.source.peak_of(start, last) > FLOOR {
                break;
            }
            last = start;
        }
        if last - first < bin / 2 {
            self.status = format!("{} is (near) silent — nothing to trim to", clip.id);
            return;
        }
        // Source silence at each edge, mapped back to *timeline* offsets: for a
        // reversed clip the clip's start is the region's top, so the two swap.
        let (lead, trail) = if clip.reversed {
            (hi - last, first - lo)
        } else {
            (first - lo, hi - last)
        };
        if lead == 0 && trail == 0 {
            self.status = format!("{} already starts and ends with audio", clip.id);
            return;
        }
        // A trim can invalidate the clip's fades (`fade_in + fade_out <= src_len`),
        // and the gesture is the place to fix that: cap them to the new length. The
        // cap comes **first** in the group — the host validates each op against the
        // running value, so a trim before the cap would be refused. (Refusing the
        // gesture instead would make `T` unusable on a clip whose fade-in fills it —
        // a state `f` and paste both produce.)
        let new_len = clip.src_len.saturating_sub(lead + trail);
        let fade_in = clip.fade_in.min(new_len);
        let fade_out = clip.fade_out.min(new_len.saturating_sub(fade_in));
        let capped = (fade_in, fade_out) != (clip.fade_in, clip.fade_out);
        let mut lines = Vec::new();
        if capped {
            lines.push(format!(
                "set_clip_fade {track} {} {fade_in} {fade_out}",
                clip.id
            ));
        }
        if lead > 0 {
            lines.push(format!("trim {track} {} start {lead}", clip.id));
        }
        if trail > 0 {
            lines.push(format!("trim {track} {} end -{trail}", clip.id));
        }
        if self.arrange_group(&lines) {
            self.status = format!(
                "{} trimmed to its content ({} frame{} in, {} out{})",
                clip.id,
                lead,
                if lead == 1 { "" } else { "s" },
                trail,
                if capped {
                    format!(", fades capped to {fade_in}/{fade_out}")
                } else {
                    String::new()
                },
            );
        }
    }

    // -- time-stretch (alpha slice E2) ---------------------------------------

    /// `W`: **warp** — stretch the clip under the playhead so its material plays at the
    /// session's tempo. The ratio is `source_bpm / session_bpm` (a 90 bpm take in a
    /// 120 bpm session becomes 3/4 as long), reduced to a rational because the log
    /// carries it verbatim and the render must be reproducible.
    fn stretch_to_tempo(&mut self) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let Some(source_bpm) = self.source_tempos.get(&clip.source_id).copied() else {
            self.status = format!(
                "no tempo recorded for source {} — set it with `: source_tempo {} <bpm>`",
                clip.source_id, clip.source_id
            );
            return;
        };
        let session_bpm = self.snap.tempo_map.tempo_at(clip.at_frame);
        let Some((num, den)) = tempo_ratio(source_bpm, session_bpm) else {
            self.status =
                format!("cannot derive a ratio from {source_bpm} bpm → {session_bpm} bpm");
            return;
        };
        if num == den {
            self.status = format!(
                "{} is already at the session tempo ({session_bpm:.1} bpm)",
                clip.source_id
            );
            return;
        }
        self.stretch_clip(&track, &clip, num, den, source_bpm, session_bpm);
    }

    /// Render a clip through the host's stretch and report what happened. The render
    /// is offline (it reads the region, transforms it and writes a pool source), so a
    /// long clip takes a moment — the shell says so by reporting the time it took.
    fn stretch_clip(
        &mut self,
        track: &str,
        clip: &Placed,
        num: u32,
        den: u32,
        source_bpm: f64,
        session_bpm: f64,
    ) {
        let started = Instant::now();
        let outcome = self.host.execute(HostCommand::Stretch {
            track: track.to_string(),
            clip: clip.id.clone(),
            num,
            den,
        });
        let elapsed = started.elapsed();
        match outcome {
            Ok(()) => {
                self.last_command = Some(("stretch", elapsed));
                self.refresh_arrangement();
                let frames = self
                    .arrangement
                    .as_ref()
                    .and_then(|arrangement| arrangement.lanes.get(self.active_track))
                    .and_then(|lane| lane.clips.iter().find(|c| c.id == clip.id))
                    .map(|c| c.src_len)
                    .unwrap_or(0);
                self.status = format!(
                    "warped {} {source_bpm:.1} → {session_bpm:.1} bpm ({num}/{den}, {} → {frames} frames, {:.0} ms)",
                    clip.id,
                    clip.src_len,
                    elapsed.as_secs_f64() * 1000.0
                );
            }
            Err(e) => self.status = format!("stretch refused: {e}"),
        }
    }

    /// `f` / `F`: put the clip's **fade-in / fade-out** at the playhead — the DAW
    /// gesture ("the fade ends here"), so it is absolute like the trims rather than
    /// a nudge. The model requires `fade_in + fade_out <= src_len`, so the other
    /// fade caps this one and a capped gesture says so.
    fn fade_to_playhead(&mut self, fade_in: bool) {
        let Some((track, clip)) = self.active_clip_at(self.snap.frame) else {
            self.status = "no clip under the playhead on the active track".to_string();
            return;
        };
        let playhead = self.snap_frame(self.snap.frame);
        let (mut new_in, mut new_out) = (clip.fade_in, clip.fade_out);
        let (wanted, capped) = if fade_in {
            let wanted = playhead.saturating_sub(clip.at_frame);
            (wanted, clip.src_len.saturating_sub(clip.fade_out))
        } else {
            let wanted = clip.end_frame().saturating_sub(playhead);
            (wanted, clip.src_len.saturating_sub(clip.fade_in))
        };
        let value = wanted.min(capped);
        if fade_in {
            new_in = value;
        } else {
            new_out = value;
        }

        let which = if fade_in { "fade in" } else { "fade out" };
        let clamped = if value < wanted {
            format!(" (capped at {value} by the other fade)")
        } else {
            String::new()
        };
        self.status = format!(
            "{} {which} = {value} frames ({:.3} s){clamped}{}",
            clip.id,
            value as f64 / self.sample_rate() as f64,
            snapped_note(self.snap.frame, playhead, self.grid.label()),
        );
        self.arrange(&format!(
            "set_clip_fade {track} {} {new_in} {new_out}",
            clip.id
        ));
    }

    /// The session's sample rate (the arrangement's, or the fallback).
    fn sample_rate(&self) -> u32 {
        self.arrangement
            .as_ref()
            .map_or(48_000, |a| a.sample_rate)
            .max(1)
    }

    // -- the snap grid ------------------------------------------------------

    /// `b`: arm the next grid division (off → bar → beat → 1/2 → 1/4 → off). The
    /// cycle is the shared workflow's; the shell only says what it is now.
    fn cycle_grid(&mut self) {
        self.grid.cycle();
        self.status = if self.grid.is_on() {
            let era = self.snap.tempo_map.meter_at(self.snap.frame);
            format!(
                "snap grid: {} ({} beat{} per bar — `H`/`L` and every edit land on it)",
                self.grid.label(),
                era,
                if era == 1 { "" } else { "s" },
            )
        } else {
            "snap grid off — edits land on the exact frame".to_string()
        };
    }

    /// The frame an **edit** lands on: quantized to the armed grid in the *beat*
    /// domain, through the session's own tempo map (so a tempo change moves the
    /// grid with the music). With no grid armed the frame is returned unchanged.
    fn snap_frame(&self, frame: u64) -> u64 {
        let map = &self.snap.tempo_map;
        let Some(grid) = self.grid.grid(map.meter_at(frame)) else {
            return frame;
        };
        map.frame_at(grid.nearest(map.beat_at(frame)))
    }

    /// One grid step in frames at `frame`'s tempo — the nudge distance when the
    /// grid is armed, and one beat when it is not.
    fn grid_step_frames(&self, frame: u64) -> i64 {
        let map = &self.snap.tempo_map;
        match self.grid.grid(map.meter_at(frame)) {
            Some(grid) => {
                let here = map.beat_at(frame);
                (map.frame_at(grid.nearest(here) + grid.step_beats())
                    - map.frame_at(grid.nearest(here))) as i64
            }
            None => (60.0 / self.snap.bpm.max(1e-9) * self.sample_rate() as f64).round() as i64,
        }
    }

    /// `[`/`]`: move the playhead to the previous/next grid line (a beat when the
    /// grid is off). Stops first, like every other seek.
    fn seek_grid(&mut self, direction: i32) {
        let map = &self.snap.tempo_map;
        let here = self.snap.frame;
        let target = match self.grid.grid(map.meter_at(here)) {
            Some(grid) => {
                let beat = map.beat_at(here);
                let line = if direction > 0 {
                    grid.ceil(beat + grid.step_beats() / 2.0)
                } else {
                    grid.floor(beat - grid.step_beats() / 2.0)
                };
                map.frame_at(line.max(0.0))
            }
            None => {
                let step = self.grid_step_frames(here);
                (here as i64 + direction as i64 * step).max(0) as u64
            }
        };
        let target = self.clamp_frame(target);
        self.status = format!(
            "seek {} to frame {target} (grid {})",
            if direction > 0 { "forward" } else { "back" },
            self.grid.label()
        );
        self.stop();
        self.command("seek", HostCommand::TransportSeek { frame: target });
    }

    // -- the `:` command line ----------------------------------------------

    /// Run the typed line **through the host's own parser**: the prompt types the
    /// `host v1` script format, so a line here is the same command a script writes,
    /// a key dispatches, and the log records. One line is one command; a parse
    /// error is shown and nothing is logged.
    fn run_prompt(&mut self) {
        let Some(raw) = self.prompt.take() else {
            return;
        };
        let prefill = self.prompt_prefill.take();
        let line = raw.trim().to_string();
        if line.is_empty() {
            self.status.clear();
            return;
        }
        // A prefilled line the user did not finish (`R` then Enter) gets a hint
        // instead of the parser's operand count. The trailing space is the tell: `R`
        // pre-fills `arrange rename_track t0 ` (a value still to come), while `X`
        // pre-fills a *complete* `export <path> f32` line that Enter may run as-is.
        if let Some(prefill) = &prefill
            && prefill.ends_with(' ')
            && line == prefill.trim()
        {
            self.status = format!("{line} needs the new value after the last space");
            return;
        }
        self.history.push(line.clone());
        self.history_at = self.history.len();

        // The header the format requires; the parser rejects anything else.
        let script = format!("host v1\n{line}\n");
        let commands = match host::parse_script(&script) {
            Ok(commands) => commands,
            Err(e) => {
                self.status = format!(": {line} — {e}");
                return;
            }
        };

        // Only a load can set the journal-recovery record, so only a load pays
        // for asking (checked before the loop consumes the batch).
        let loads = commands
            .iter()
            .any(|c| matches!(c, host::HostCommand::Load { .. }));

        let started = Instant::now();
        for command in commands {
            if let Err(e) = self.host.execute(command) {
                self.status = format!(": {line} — {e}");
                return;
            }
        }
        self.last_command = Some(("prompt", started.elapsed()));
        self.status = format!(": {line}");
        // A load replays the session directory's journal, and the host counts
        // what that recovery applied, tore and refused. The counts are the
        // crash story — say them beside the line that triggered them, so a
        // recovered session tells the user what the crash cost.
        if loads
            && let Ok(Some(rec)) = self.host.last_recovery()
            && rec.has_story()
        {
            self.status = format!(": {line} — {}", rec.describe());
        }
        // A line may have edited the arrangement, the parameters, or the transport:
        // re-read what the log now says rather than guessing which.
        self.refresh_arrangement();
    }

    /// `↑` / `↓`: walk the command history, ending on the blank live line.
    fn history_step(&mut self, direction: i32) {
        if self.history.is_empty() {
            return;
        }
        let next = if direction < 0 {
            self.history_at.saturating_sub(1)
        } else {
            (self.history_at + 1).min(self.history.len())
        };
        self.history_at = next;
        // Past the end is the "live line": for a prefilled prompt that is the
        // prefill, not a blank line that silently drops what the key opened.
        self.prompt = Some(match self.history.get(next) {
            Some(line) => line.clone(),
            None => self.prompt_prefill.clone().unwrap_or_default(),
        });
    }

    /// `n`/`N`: jump the playhead to the next/previous clip boundary on the
    /// active track — the motion an arrangement needs where text has "next word".
    fn seek_clip(&mut self, forward: bool) {
        let target = {
            let Some(arrangement) = self.arrangement.as_ref() else {
                return;
            };
            let here = self.snap.frame;
            let clip = if forward {
                arrangement.clip_from(self.active_track, here)
            } else {
                arrangement.clip_before(self.active_track, here)
            };
            clip.map(|clip| clip.at_frame)
        };

        match target {
            Some(frame) => {
                self.stop();
                self.command("seek", HostCommand::TransportSeek { frame });
            }
            None => {
                self.status = if forward {
                    "no later clip on this track".to_string()
                } else {
                    "no earlier clip on this track".to_string()
                }
            }
        }
    }

    /// The envelope area's width in terminal cells.
    fn panel_width(&self) -> u64 {
        self.panel.ruler.width.max(1) as u64
    }

    /// A click on the timeline seeks the transport to that point — the mouse
    /// dispatching exactly the command a key would, so there is one log. An armed
    /// grid quantizes the click like every other playhead move (the status line
    /// says so when it moved), and the target stays inside the arrangement.
    fn timeline_seek(&mut self, position: Position) {
        let (Some(arrangement), Some(view)) = (&self.arrangement, &self.view) else {
            return;
        };
        let cell = position.x.saturating_sub(self.panel.ruler.x) as u64;
        let raw = view.frame_at(cell).min(arrangement.frames);
        let frame = self.clamp_frame(self.snap_frame(raw));
        self.status = format!(
            "seek to frame {frame}{}",
            snapped_note(raw, frame, self.grid.label())
        );
        self.stop();
        self.command("seek", HostCommand::TransportSeek { frame });
    }

    fn toggle_mouse(&mut self) {
        self.mouse = !self.mouse;
        let _ = if self.mouse {
            execute!(io::stdout(), EnableMouseCapture)
        } else {
            execute!(io::stdout(), DisableMouseCapture)
        };
    }

    /// The mouse's whole role: hit-test the rects the last draw published. No
    /// widget tree, no focus, no hit regions — just the areas the view recorded.
    fn action_at(&self, position: Position) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(rect, _)| rect.contains(position))
            .map(|(_, action)| *action)
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let position = Position::new(mouse.column, mouse.row);

        // A click or drag on a strip sets that fader from the row under the
        // pointer — the one gesture a console is expected to have.
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
        ) && let Some((strip, ratio)) = self.strips.at(position)
        {
            self.focus = Panel::Mixer;
            self.selected = strip;
            self.set_fader(strip, ratio);
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self.action_at(position) {
                    self.apply(action);
                    return;
                }

                // Clicking a panel focuses it; clicking the ruler or a lane also
                // acts on what was clicked (seek, and make that track active).
                if self.panel.outer.contains(position) {
                    self.focus = Panel::Timeline;
                } else if self.strips.contains(position) || self.mixer_rect.contains(position) {
                    self.focus = Panel::Mixer;
                }

                if self.panel.contains(position) {
                    if let Some(lane) = self.panel.lane_at(position) {
                        self.active_track = lane;
                    }
                    self.timeline_seek(position);
                    return;
                }

                if let Some((channel, _)) = self
                    .meter_rows
                    .iter()
                    .find(|(_, rect)| rect.contains(position))
                {
                    self.selected = *channel;
                }
            }
            MouseEventKind::ScrollUp => self.nudge(-1),
            MouseEventKind::ScrollDown => self.nudge(1),
            _ => {}
        }
    }

    // -- view --------------------------------------------------------------

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // The timeline panel appears when an arrangement is loaded; the meters
        // take its space otherwise.
        let pool_rows = if self.pool.is_empty() {
            0
        } else {
            // Six rows: a border, the column header, three sources, and a "…" line.
            // The pool is a *list*, not a browser: the alpha browses by scrolling.
            (self.pool.len() + 2).clamp(4, 8) as u16
        };

        if self.arrangement.is_some() {
            let [head, controls, body, foot] = Layout::vertical([
                Constraint::Length(5),
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(5),
            ])
            .areas(area);

            // The console sits on the right, as on a desk: strips wide enough to
            // carry a fader, a meter, a name and a value — or as much of that as
            // half the terminal allows.
            let strips = self.mixer.strips() as u16;
            let mixer_width = (strips * 6 + 2).min(area.width / 2).max(10);
            let [timeline_area, mixer_area] =
                Layout::horizontal([Constraint::Min(20), Constraint::Length(mixer_width)])
                    .areas(body);

            self.draw_head(frame, head);
            self.draw_controls(frame, controls);
            if pool_rows > 0 {
                // The pool under the timeline: material *from which* the arrangement
                // is made, so it belongs beside it, not in a separate screen.
                let [timeline_area, pool_area] =
                    Layout::vertical([Constraint::Min(6), Constraint::Length(pool_rows)])
                        .areas(timeline_area);
                self.draw_timeline(frame, timeline_area);
                self.draw_pool(frame, pool_area);
            } else {
                self.draw_timeline(frame, timeline_area);
            }
            self.draw_mixer(frame, mixer_area);
            self.draw_foot(frame, foot);
        } else {
            // The footer gains a row for the live take line only while a take runs;
            // idle it keeps the two-line state block it has always had.
            let foot_rows = if recording_line(&self.snap).is_some() {
                5
            } else {
                4
            };
            let [head, controls, meters, foot] = Layout::vertical([
                Constraint::Length(5),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(foot_rows),
            ])
            .areas(area);

            self.draw_head(frame, head);
            self.draw_controls(frame, controls);
            if pool_rows > 0 {
                let [meters, pool_area] =
                    Layout::vertical([Constraint::Min(3), Constraint::Length(pool_rows)])
                        .areas(meters);
                self.draw_meters(frame, meters);
                self.draw_pool(frame, pool_area);
            } else {
                self.draw_meters(frame, meters);
            }
            self.draw_foot(frame, foot);
        }

        if self.help {
            self.draw_help(frame, area);
        }
    }

    /// The console: channel strips with faders and meters, on the right. The
    /// host owns the audio; the strips show the levels it publishes next to the
    /// positions the shell has asked for.
    fn draw_mixer(&mut self, frame: &mut Frame, area: Rect) {
        self.mixer.resize(self.snap.channel_count);
        self.mixer_rect = area;

        let levels: Vec<f32> = (0..self.snap.channel_count)
            .map(|channel| self.snap.channels[channel])
            .collect();

        self.strips = mixer::draw(
            frame,
            area,
            &self.mixer,
            &levels,
            self.snap.master,
            self.selected,
            self.focus == Panel::Mixer,
        );
    }

    /// The timeline: origin, ruler, one envelope lane per track, clip
    /// boundaries, the selection, the playhead, the active track. The panel
    /// publishes its rects for the mouse.
    fn draw_timeline(&mut self, frame: &mut Frame, area: Rect) {
        let Some(arrangement) = self.arrangement.as_ref() else {
            return;
        };
        // The real panel width is only known once drawn: minus the border and the
        // track gutter.
        let inner_width = area.width.saturating_sub(2 + timeline::GUTTER).max(1) as u64;
        let frames = arrangement.frames;
        let Some(view) = self.view.as_mut() else {
            return;
        };
        view.fit_if_needed(frames, inner_width);

        let panel = timeline::draw(
            frame,
            area,
            arrangement,
            view,
            self.snap.frame,
            self.mode,
            self.active_track,
            self.focus == Panel::Timeline,
            self.grid
                .grid(self.snap.tempo_map.meter_at(self.snap.frame)),
            &self.snap.tempo_map,
        );
        self.panel = panel;
    }

    fn draw_head(&self, frame: &mut Frame, area: Rect) {
        let transport = if self.snap.playing {
            "playing"
        } else {
            "stopped"
        };

        // The marker the playhead is standing on, if any: the readout names the section
        // you are in, which is what a marker is for.
        let section = self
            .arrangement
            .as_ref()
            .and_then(|a| a.marker_at(self.snap.frame))
            .map(|m| format!("   ▼ {}", m.name))
            .unwrap_or_default();
        let line = format!(
            "position {:.3} s   beat {:.2}   tempo {:.1} bpm   frame {}   {}{section}",
            self.snap.seconds, self.snap.beat, self.snap.bpm, self.snap.frame, transport,
        );

        let paragraph = Paragraph::new(vec![
            line.into(),
            audio_label(&self.snap).into(),
            format!(
                "selected ch{}   undo {}   redo {}",
                self.selected,
                yes_no(self.snap.can_undo),
                yes_no(self.snap.can_redo),
            )
            .into(),
        ])
        .block(Block::bordered().title(" sound-arranger · tui spike "))
        .wrap(Wrap { trim: true });

        frame.render_widget(paragraph, area);
    }

    fn draw_controls(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered().title(" transport ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        self.buttons.clear();

        let playing = self.snap.playing;
        let buttons = [
            ("Play", Button::Play, playing),
            ("Stop", Button::Stop, !playing),
            ("Rewind", Button::Rewind, false),
        ];

        let mut spans = Vec::new();
        let mut x = inner.x;

        for (label, action, active) in buttons {
            let width = label.len() as u16 + 2;
            if x + width > inner.x + inner.width {
                break;
            }

            // The active transport state is highlighted, so the button row is
            // also a state readout — a TUI has no hover to lean on.
            let style = if active {
                Style::new()
                    .bg(Color::LightBlue)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().bg(Color::DarkGray).fg(Color::White)
            };

            spans.push(Span::styled(format!(" {label} "), style));
            spans.push(Span::raw("  "));

            self.buttons.push((Rect::new(x, inner.y, width, 1), action));
            x += width + 2;
        }

        spans.push(Span::styled(
            "space play/stop · ? keys · q quit",
            Style::new().fg(Color::Gray),
        ));

        frame.render_widget(
            Paragraph::new(ratatui::text::Line::from(spans)).wrap(Wrap { trim: true }),
            inner,
        );
    }

    /// The pool panel: one row per source — id, length (frames and seconds), rate,
    /// channels — with the selected row marked. A source that could not be read is
    /// not here at all (the pool's `list` reports it and the state line says so).
    fn draw_pool(&mut self, frame: &mut Frame, area: Rect) {
        let border = if self.focus == Panel::Pool {
            Color::LightBlue
        } else {
            Color::DarkGray
        };
        // The list is handed **whole** to ratatui, so the widget scrolls the selected
        // row into view; the title carries the hint when there are more rows than fit
        // (truncating the items here would defeat that scrolling).
        let block = Block::bordered()
            .title(format!(
                " pool — {} source{}{} ",
                self.pool.len(),
                if self.pool.len() == 1 { "" } else { "s" },
                if self.pool.len() > area.height.saturating_sub(3) as usize {
                    " · j/k scrolls"
                } else {
                    ""
                }
            ))
            .border_style(Style::new().fg(border));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.pool_rect = area;

        if inner.height == 0 {
            return;
        }
        let header = vec![ratatui::text::Line::from(Span::styled(
            "  id                     length        rate   ch",
            Style::new().fg(Color::DarkGray),
        ))];
        let items: Vec<ratatui::text::Line> = self
            .pool
            .iter()
            .map(|source| {
                let seconds = source.frames as f64 / source.sample_rate.max(1) as f64;
                let mut flags = String::new();
                if !source.finalized {
                    flags.push_str(" (crashed take)");
                } else if source.peaks_missing {
                    flags.push_str(" (no peaks)");
                }
                if !Self::placeable_source(&source.id) {
                    flags.push_str(" (unplaceable: space or # in the name)");
                }
                if source.frames == 0 {
                    flags.push_str(" (no frames)");
                }
                ratatui::text::Line::from(format!(
                    "▸{:<22} {:>7} {:>7.3}s  {:>5}  {:>2}{flags}",
                    source.id, source.frames, seconds, source.sample_rate, source.channels,
                ))
            })
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("");
        let mut state = ratatui::widgets::ListState::default()
            .with_selected((!self.pool.is_empty()).then_some(self.pool_selected));
        let [header_area, list_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        frame.render_widget(Paragraph::new(header), header_area);
        frame.render_stateful_widget(list, list_area, &mut state);
    }

    fn draw_meters(&mut self, frame: &mut Frame, area: Rect) {
        // The focus ring: the mixer's border is lit when its keys are live.
        let border = if self.focus == Panel::Mixer {
            Color::LightBlue
        } else {
            Color::DarkGray
        };
        let block = Block::bordered()
            .title(" meters ")
            .border_style(Style::new().fg(border));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.mixer_rect = area;

        self.meter_rows.clear();

        let count = self.snap.channel_count;
        if count == 0 {
            frame.render_widget(
                Paragraph::new("no mixer mounted").style(Style::new().fg(Color::Gray)),
                inner,
            );
            return;
        }

        // The rows that fit: one per channel plus the master. When they do not
        // all fit, the last row is reserved for the notice instead of drawing a
        // meter under it (a small-terminal overlap, same class as the
        // zero-height crash the pty run found).
        let entries = count + 1;
        let truncated = entries > inner.height as usize;
        let rows = if truncated {
            (inner.height as usize).saturating_sub(1)
        } else {
            entries
        };
        let constraints = vec![Constraint::Length(1); rows];
        let row_areas = Layout::vertical(constraints).split(inner);

        for (row, row_area) in row_areas.iter().enumerate() {
            let (label, level, channel) = if row < count {
                (format!("ch{row}"), self.snap.channels[row], Some(row))
            } else {
                ("master".to_string(), self.snap.master, None)
            };

            let [name, bar, value] = Layout::horizontal([
                Constraint::Length(8),
                Constraint::Min(10),
                Constraint::Length(6),
            ])
            .areas(*row_area);

            let selected = channel == Some(self.selected);
            let name_style = if selected {
                Style::new().fg(Color::Black).bg(Color::LightBlue)
            } else {
                Style::new().fg(Color::Gray)
            };

            frame.render_widget(
                Paragraph::new(format!("{label:<6}")).style(name_style),
                name,
            );

            let color = if level > 0.9 {
                Color::LightRed
            } else if selected {
                Color::LightBlue
            } else {
                Color::LightGreen
            };

            // No label on the gauge: the number gets its own column, so the bar
            // reads as a bar instead of text on a filled background. (An empty
            // label suppresses ratatui's default percentage.)
            frame.render_widget(
                Gauge::default()
                    .ratio(level.clamp(0.0, 1.0) as f64)
                    .label("")
                    .gauge_style(Style::new().fg(color))
                    .use_unicode(true),
                bar,
            );

            frame.render_widget(
                Paragraph::new(format!("{level:.2}")).style(Style::new().fg(if level > 0.9 {
                    Color::LightRed
                } else {
                    Color::Gray
                })),
                value,
            );

            if let Some(channel) = channel {
                // The whole row selects the channel — a bigger target than the
                // 8-column name, which is the point of trying the mouse at all.
                self.meter_rows.push((channel, *row_area));
            }
        }

        // A terminal can be resized to almost nothing; the notice goes on the
        // last visible row only when there is one. (A zero-height area here used
        // to underflow — found by running the spike under a pty.)
        if truncated && inner.height > 0 {
            let hidden = entries - rows;
            frame.render_widget(
                Paragraph::new(format!("… {hidden} more (resize for the master)"))
                    .style(Style::new().fg(Color::DarkGray)),
                Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            );
        }
    }

    fn draw_foot(&self, frame: &mut Frame, area: Rect) {
        let latency = match &self.last_command {
            Some((label, elapsed)) => {
                format!(
                    "last command: {label} ({:.1} ms in the UI thread)",
                    ms(elapsed)
                )
            }
            None => "last command: —".to_string(),
        };

        // The mode is always on screen — a modal UI that hides its mode is a trap
        // (the modal editing note's first rule).
        // The mode is always on screen — and the command line is a mode.
        let mode_label = if self.prompt.is_some() {
            " COMMAND ".to_string()
        } else {
            format!(" {} ", self.mode.label())
        };
        let mode = if self.mode == Mode::Visual || self.prompt.is_some() {
            Span::styled(
                mode_label,
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                mode_label,
                Style::new().fg(Color::Black).bg(Color::DarkGray),
            )
        };

        // Which panel the keys act on, the armed snap grid, the active track, the
        // viewport, and the selection — all of it on one line so the state is never
        // guessed. The grid is on the line because it is invisible otherwise: an
        // edit that lands somewhere unexpected must be explained by the state.
        let focus = format!(
            "focus {}   grid {}   ",
            self.focus.label(),
            self.grid.label()
        );
        let timeline = match (&self.arrangement, &self.view) {
            (Some(arrangement), Some(view)) => {
                let selection = view
                    .selection_label(arrangement.sample_rate)
                    .map(|label| format!("   sel {label}"))
                    .unwrap_or_default();
                let end = (view.start + view.visible(self.panel.ruler.width.max(1) as u64))
                    .min(arrangement.frames);
                let rate = arrangement.sample_rate.max(1) as f64;
                format!(
                    "   track {}   view {:.3}–{:.3} s of {:.3} s   {:.3} ms/col{selection}",
                    arrangement
                        .lanes
                        .get(self.active_track)
                        .map(|lane| lane.id.as_str())
                        .unwrap_or("—"),
                    view.start as f64 / rate,
                    end as f64 / rate,
                    arrangement.seconds(),
                    view.ms_per_cell(arrangement.sample_rate),
                )
            }
            _ => String::new(),
        };

        // The second line is the status — or, while the prompt is open, the command
        // line itself with a block cursor, where a `:` belongs.
        let second = match &self.prompt {
            Some(text) => ratatui::text::Line::from(vec![
                Span::styled(
                    ":",
                    Style::new()
                        .fg(Color::LightMagenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(text.clone()),
                Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
            ]),
            None => self.status.clone().into(),
        };

        let message = Paragraph::new(vec![
            ratatui::text::Line::from(vec![
                mode,
                Span::raw(format!("  {focus}mouse {}   {latency}", on_off(self.mouse))),
                Span::styled(timeline, Style::new().fg(Color::Gray)),
            ]),
            second,
        ])
        .wrap(Wrap { trim: true });

        // The **take** is its own row, not a third line of the wrapping paragraph: a
        // status long enough to wrap would otherwise push it out of the block's inner
        // height, and the one line that must stay visible while recording is the one
        // that says a take is running. The message above is the part allowed to clip.
        let take = recording_line(&self.snap);
        let block = Block::bordered().title(" state ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [message_area, take_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(u16::from(take.is_some())),
        ])
        .areas(inner);
        frame.render_widget(message, message_area);
        if let Some(take) = take {
            frame.render_widget(
                Paragraph::new(ratatui::text::Line::from(Span::styled(
                    take,
                    Style::new()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                ))),
                take_area,
            );
        }
    }

    /// The `?` overlay: the keymap table, rendered (so it cannot drift from the
    /// behaviour) and **scrollable**, because a terminal that shows 24 rows cannot
    /// show a 22-entry keymap plus its wrapped meanings. The title says when there
    /// is more below, so a clipped help never reads as the whole help.
    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let rows: Vec<ratatui::text::Line> = workflow::help()
            .map(|(keys, meaning)| {
                ratatui::text::Line::from(vec![
                    Span::styled(
                        format!("{keys:<18}"),
                        Style::new()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(meaning),
                ])
            })
            .collect();

        let popup = centered_rect(64, (workflow::help_len() as u16 + 4).min(area.height), area);
        frame.render_widget(Clear, popup);

        let more = workflow::help_len().saturating_sub(self.help_scroll);
        let title = if more + 4 > popup.height as usize {
            format!(" keys — ? or Esc closes · j/k scrolls ({more} below) ")
        } else {
            " keys — ? or Esc closes ".to_string()
        };

        frame.render_widget(
            Paragraph::new(rows)
                .block(Block::bordered().title(title))
                .scroll((self.help_scroll as u16, 0))
                .wrap(Wrap { trim: true }),
            popup,
        );
    }
}

// ---------------------------------------------------------------------------
// The take line
// ---------------------------------------------------------------------------

/// The live take line, drawn from the **snapshot** the event loop refreshes every
/// frame — so a running take's counters move with the host, and a status message (an
/// export report, a source error) cannot hide the recording.
///
/// `None` when no take is running. The finished take stays a one-shot announcement in
/// the status line: it has an action to offer (`arrange add_clip`), and it does not
/// belong on screen forever.
fn recording_line(snap: &Snapshot) -> Option<String> {
    snap.recording.as_ref().map(|rec| {
        // Monitor drops are best-effort and zero unless something monitors the take,
        // so they are named and shown only when they happen — "N dropped" beside a
        // take read as lost audio.
        let monitor = if rec.monitor_dropped > 0 {
            format!(", {} monitor drops", rec.monitor_dropped)
        } else {
            String::new()
        };
        format!(
            "● REC {} — {} ch, {} frames{monitor} (`:record stop` ends it)",
            rec.take_id, rec.channels, rec.frames
        )
    })
}

// ---------------------------------------------------------------------------
// The terminal runtime
// ---------------------------------------------------------------------------

fn run(keys: bool, wave: Option<PathBuf>, script: Option<PathBuf>) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture);

    let mut app = App::boot(wave, script);
    app.help = keys;
    let outcome = event_loop(&mut terminal, &mut app);

    if app.mouse {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    drop_pool();

    outcome
}

/// Remove this process's session pool (its imports are copies; the tool that
/// brought the material in still has the original).
fn drop_pool() {
    let _ = std::fs::remove_dir_all(session_pool_dir());
}

/// The runtime: poll the host, redraw, wait for input (up to one frame). Mouse
/// and key events both land in `App`, exactly as messages do in the iced spike.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.quit {
        app.snap = app.host.snapshot();
        terminal.draw(|frame| app.draw(frame))?;

        if event::poll(FRAME)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                _ => {}
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Headless modes
// ---------------------------------------------------------------------------

/// `--dump`: render one deterministic frame and print it as plain text. This is
/// the TUI's version of "look at the window" — what CI, the note, and a
/// screenshot-less review can actually read. `--dump --keys` renders the keymap
/// overlay instead; `--dump --wave <file.wav>` or `--dump --script <script>`
/// renders an arrangement (still deterministic: the viewport, the envelope and
/// the synthetic playhead do not depend on timing).
fn dump(keys: bool, wave: Option<PathBuf>, script: Option<PathBuf>) -> io::Result<()> {
    let mut app = App::boot(wave, script);
    // The synthetic transport/meter state keeps the dump deterministic and
    // readable; the arrangement (if any) is real.
    app.snap = App::demo_snapshot();
    app.help = keys;
    app.focus = if app.arrangement.is_some() {
        Panel::Timeline
    } else {
        Panel::Mixer
    };

    let backend = ratatui::backend::TestBackend::new(100, 26);
    // The test backend cannot fail, so these are expects rather than a Result
    // shaped to satisfy it.
    let mut terminal = ratatui::Terminal::new(backend).expect("the test backend is infallible");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the test backend is infallible");

    print!("{}", buffer_text(terminal.backend().buffer()));
    drop_pool();
    Ok(())
}

/// The spike's **session pool**: one directory per process under the temp dir.
/// A pool is session-owned working material (see `media::pool`), so `--wave`
/// imports a copy here rather than pointing the pool at the user's own directory
/// — which would make the pool pass rewrite their file.
fn session_pool_dir() -> PathBuf {
    std::env::temp_dir().join(format!("tui-shell-pool-{}", std::process::id()))
}

/// Import `path` into the spike's session pool at the session rate and build the
/// one-clip host script that places it. Returns `(script, what to tell the user)`.
fn wave_script(path: &std::path::Path) -> Result<(String, String), String> {
    let dir = session_pool_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("pool dir {}: {e}", dir.display()))?;

    // The text format is whitespace-separated, so a path with a space cannot be
    // named in it. Renaming is the honest fix; quoting soup is not.
    if dir.to_string_lossy().split_whitespace().count() != 1 {
        return Err(format!(
            "the temp dir '{}' contains whitespace — set TMPDIR to a simple path",
            dir.display()
        ));
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "the file name is not valid UTF-8".to_string())?;
    if stem.split_whitespace().count() != 1 {
        return Err(format!(
            "'{stem}' contains whitespace — copy the file to a simple name first"
        ));
    }

    let pool = media::Pool::open(&dir)?;
    let imported = pool.import(path, SAMPLE_RATE as u32)?;
    if imported.frames_out() == 0 {
        return Err("the file has no audio frames".to_string());
    }

    // A multi-channel file arrives as one mono source per channel (import splits
    // it). Place each on its own track — and for a stereo file pan the pair hard
    // left/right, so it plays as stereo instead of losing its right channel. A
    // wider file gets one centred track per channel; more channels than the mixer
    // has (`MIXER_CHANNELS_MAX`) is refused by the host's own mount check, with
    // every channel still split and in the pool.
    let sources = &imported.sources;
    let mut script = String::from("host v1\n");
    script.push_str(&format!(
        "mount mixer channels={} @0\n",
        sources.len().max(2)
    ));
    script.push_str(&format!("pool {}\n", dir.display()));
    if sources.len() == 2 {
        script.push_str("set_param mixer ch0.pan -1 @0\n");
        script.push_str("set_param mixer ch1.pan 1 @0\n");
    }
    for (i, source) in sources.iter().enumerate() {
        script.push_str(&format!("arrange add_track t{i}\n"));
        script.push_str(&format!(
            "arrange add_clip t{i} c{i} {} 0 {} 0 0 0 1.0\n",
            source.id, source.frames_out
        ));
    }

    let note = if imported.converted() {
        let first = &sources[0];
        format!(
            "imported {} — {} channel(s) resampled {} → {} Hz, {} frames at the session rate{}",
            imported.id,
            imported.channels,
            first.from_rate,
            first.to_rate,
            imported.frames_out(),
            first
                .preserved
                .as_ref()
                .map(|p| format!(", original kept as {}", p.display()))
                .unwrap_or_default(),
        )
    } else if sources.len() > 1 {
        format!(
            "imported {} — {} channels split into {}, {} frames each at {} Hz",
            imported.id,
            imported.channels,
            imported.ids().join(", "),
            imported.frames_out(),
            SAMPLE_RATE,
        )
    } else {
        format!(
            "imported {} — {} frames at {} Hz",
            imported.id,
            imported.frames_out(),
            SAMPLE_RATE,
        )
    };

    Ok((script, note))
}

/// `--probe`: the non-TUI half of the proof — host thread, transport, and live
/// meters, with no terminal at all. With `--wave` or `--script` it loads *that*
/// material instead of the built-in demo, which is how "the loaded file actually
/// plays" is asserted without speakers: a silent host's meters only move if the
/// arrangement rendered through the mixer.
fn probe(wave: Option<PathBuf>, script: Option<PathBuf>) -> i32 {
    let (what, source) = match (&script, &wave) {
        (Some(path), _) => match std::fs::read_to_string(path) {
            Ok(text) => (format!("script {}", path.display()), text),
            Err(e) => {
                eprintln!("probe: cannot read {}: {e}", path.display());
                return 2;
            }
        },
        (None, Some(path)) => match wave_script(path) {
            Ok((text, note)) => (format!("wave {} ({note})", path.display()), text),
            Err(e) => {
                eprintln!("probe: {e}");
                return 2;
            }
        },
        (None, None) => (
            "the built-in demo profile".to_string(),
            DEMO_SCRIPT.to_string(),
        ),
    };
    println!("probe: probing {what}");

    let script = match host::parse_script(&source) {
        Ok(script) => script,
        Err(e) => {
            eprintln!("probe: the script does not parse: {e}");
            return 2;
        }
    };

    let host = HostHandle::spawn();

    match host.load(&script) {
        Ok(outcome) => println!(
            "probe: loaded — mixer {:?} ch, {} log events, {} underruns",
            outcome.mixer_channels, outcome.event_count, outcome.underruns,
        ),
        Err(e) => {
            eprintln!("probe: load failed: {e}");
            return 1;
        }
    }

    if let Err(e) = host.execute(HostCommand::TransportPlay) {
        eprintln!("probe: transport play refused: {e}");
        return 1;
    }

    let mut peak: f32 = 0.0;
    for _ in 0..24 {
        std::thread::sleep(Duration::from_millis(25));
        let snap = host.snapshot();
        // Every mounted channel, not just the first: a stereo import routes one
        // channel per track, so `ch1` moving is the proof the right channel is
        // wired (a silent one would look identical if only `ch0` were printed).
        let channels = &snap.channels[..snap.channel_count.min(snap.channels.len())];
        peak = peak.max(channels.iter().copied().fold(0.0, f32::max));
        peak = peak.max(snap.master);
        let meters: Vec<String> = channels
            .iter()
            .enumerate()
            .map(|(i, v)| format!("ch{i}={v:.4}"))
            .collect();
        println!(
            "probe: frame={:>7} t={:>6.3}s beat={:>6.2} playing={:<5} {} master={:.4}",
            snap.frame,
            snap.seconds,
            snap.beat,
            snap.playing,
            meters.join(" "),
            snap.master,
        );
    }

    let last = host.snapshot();
    let _ = host.execute(HostCommand::TransportStop);
    host.shutdown();

    println!("probe: peak channels/master = {peak:.4}");

    drop_pool();
    if let Some(e) = &last.last_error {
        eprintln!("probe: pump error: {e}");
        return 1;
    }

    if peak <= 0.0 {
        eprintln!("probe: the transport ran but no meter signal was observed");
        return 1;
    }

    println!("probe: OK — the host thread, transport, and live meters all work");
    0
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Flatten a rendered buffer into text, one line per terminal row. This is how
/// the TUI is inspected without a terminal.
fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    let area = buffer.area();
    let mut text = String::with_capacity(buffer.content().len() + area.height as usize);

    for row in buffer.content().chunks(area.width as usize) {
        let start = text.len();
        for cell in row {
            text.push_str(cell.symbol());
        }
        // Trim the padding ratatui fills the frame with, so the dump is readable.
        while text.len() > start && text.ends_with(' ') {
            text.pop();
        }
        text.push('\n');
    }

    text
}

/// The stretch ratio that makes `source_bpm` material play at `session_bpm`, as a
/// **reduced rational**: the tempos are scaled by 1000 (so a decimal tempo is exact)
/// and divided by their gcd. A rational is what the log carries — an `f32` ratio could
/// print differently on another platform, and the render must be reproducible.
fn tempo_ratio(source_bpm: f64, session_bpm: f64) -> Option<(u32, u32)> {
    if !source_bpm.is_finite()
        || source_bpm <= 0.0
        || !session_bpm.is_finite()
        || session_bpm <= 0.0
    {
        return None;
    }
    let num = (source_bpm * 1000.0).round() as u64;
    let den = (session_bpm * 1000.0).round() as u64;
    if num == 0 || den == 0 {
        return None;
    }
    let g = gcd(num, den);
    let (num, den) = (num / g, den / g);
    (num <= u32::MAX as u64 && den <= u32::MAX as u64).then_some((num as u32, den as u32))
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// A status suffix that says the grid moved the frame (an edit that landed
/// somewhere other than the playhead must say so, or the grid looks like a bug).
fn snapped_note(raw: u64, snapped: u64, grid: &str) -> String {
    if raw == snapped {
        String::new()
    } else {
        format!(" (snapped from {raw} on the {grid} grid)")
    }
}

fn audio_label(snap: &Snapshot) -> String {
    let base = match &snap.audio {
        None => "audio: silent host (no device requested)".to_string(),
        Some(audio) => match &audio.error {
            Some(e) => format!("audio: unavailable — {e}"),
            None => {
                let mismatch = if audio.rate_mismatch {
                    format!(" (requested {} — MISMATCH)", audio.requested_rate)
                } else {
                    String::new()
                };

                format!(
                    "audio: {} Hz · {} ch{mismatch} · underruns {} · drops {}",
                    audio.sample_rate, audio.channels, audio.underruns, audio.drops,
                )
            }
        },
    };

    match &snap.last_error {
        Some(e) => format!("{base} — pump error: {e}"),
        None => base,
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);

    let [row] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);

    popup
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn ms(duration: &Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Render the app once, off-screen, and return the frame as text.
    fn rendered(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 26)).expect("test terminal");
        terminal
            .draw(|frame| app.draw(frame))
            .expect("draw does not fail on a test backend");

        buffer_text(terminal.backend().buffer())
    }

    /// `A` arms the declared rig for the next take, and the record prompt is prefilled
    /// with the armed source — the shell half of `record <take> source=<name>`, so what
    /// runs is what is on screen.
    #[test]
    fn the_source_key_arms_the_declared_rig_for_the_next_take() {
        let mut app = App::demo();
        app.command(
            "source add vcv kind=pulse match=sink.monitor channels=2 clock=free",
            HostCommand::SourceAdd {
                name: "vcv".into(),
                kind: "pulse",
                matcher: "sink.monitor".into(),
                channels: 2,
                clock: host::rig::ClockRole::Free,
            },
        );
        // The actor publishes after it replies: wait for the rig rather than race it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while app.host.snapshot().sources.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        app.on_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
        assert_eq!(
            app.record_source.as_deref(),
            Some("vcv"),
            "the declared source is armed: {}",
            app.status
        );
        assert!(app.status.contains("vcv"), "{}", app.status);

        // `o` prefills the line with the armed source: the typed text is what runs.
        app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        let prompt = app.prompt.clone().expect("the record prompt opens");
        assert!(prompt.starts_with("record take-"), "{prompt}");
        assert!(prompt.contains("source=vcv"), "{prompt}");

        // Cycling past the last source returns to the default input, and the prefill
        // names no source — the pre-binding behaviour.
        app.prompt = None;
        app.on_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
        assert_eq!(app.record_source, None, "{}", app.status);
        app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        let prompt = app.prompt.clone().expect("the record prompt opens");
        assert!(!prompt.contains("source="), "{prompt}");
    }

    /// `o`'s decision is the **host's** state, not the status line.
    ///
    /// The status is a message channel: an export report or a source error overwrites
    /// it, and an earlier `command refused` sits in it until the next command. Deciding
    /// from it — the first version did — acted on state that was not the host's: it
    /// offered to start a take while one ran, and stopped one that had already ended.
    #[test]
    fn the_record_key_reads_the_snapshot_not_the_status() {
        let mut app = App::demo();

        // A take is running while the status says something else entirely: stop it.
        app.snap.recording = Some(host::RecordingStatus {
            take_id: "take-3".to_string(),
            frames: 4_800,
            monitor_dropped: 0,
            channels: 2,
            source: None,
        });
        app.status =
            "exported 96000 frames (wav, +512 tail) — peak -1.0 dBFS, rms -3.0 dBFS".to_string();
        assert_eq!(app.record_intent(), RecordIntent::Stop);

        // No take runs, but the status still says one does: ask for a name rather than
        // send a stop the host would refuse.
        app.snap.recording = None;
        app.status = "● recording take-3 — 2 ch, 4800 frames, 0 dropped (`:record stop` ends it)"
            .to_string();
        assert_eq!(app.record_intent(), RecordIntent::Name);
    }

    /// The take line is drawn from the snapshot, so it is live — the counters are the
    /// host's, not the last thing a command wrote — and absent when no take runs.
    #[test]
    fn the_take_line_is_drawn_from_the_snapshot() {
        let mut app = App::demo();
        app.snap.recording = None;
        assert_eq!(recording_line(&app.snap), None);

        app.snap.recording = Some(host::RecordingStatus {
            take_id: "take-3".to_string(),
            frames: 4_800,
            monitor_dropped: 0,
            channels: 2,
            source: None,
        });
        let line = recording_line(&app.snap).expect("a running take draws a line");
        assert!(line.contains("REC"), "{line}");
        assert!(line.contains("take-3"), "{line}");
        assert!(line.contains("4800"), "{line}");
        assert!(
            !line.contains("drops"),
            "an unmonitored take reports no drops: {line}"
        );

        // The footer renders it, from the snapshot, below the status line.
        assert!(rendered(&mut app).contains("● REC take-3"));
    }

    /// The session pool is **one directory per process** (`session_pool_dir`), so
    /// tests that import into it — or drop it — must not run concurrently: they
    /// share the pool the running shell uses, by design.
    static POOL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn pool_guard() -> std::sync::MutexGuard<'static, ()> {
        POOL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A three-second WAV on disk (a loud second and a half, then silence), so
    /// the timeline has a shape to look for and the synthetic 2.000 s playhead
    /// lands inside the file.
    fn wav_fixture(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tui-shell-render-{name}-{}.wav",
            std::process::id()
        ));

        let rate = 48_000u32;
        let frames = rate as usize * 3;
        let mut writer =
            media::wav::WavWriter::create_float(&path, rate, 1).expect("create the fixture");
        let samples: Vec<f32> = (0..frames)
            .map(|i| {
                let amplitude = if i < frames / 2 { 0.8f32 } else { 0.0 };
                amplitude * (i as f32 * 0.05).sin()
            })
            .collect();
        writer.write(&samples).expect("write");
        writer.finalize().expect("finalize");
        path
    }

    #[test]
    fn the_readout_meters_and_transport_are_rendered() {
        let screen = rendered(&mut App::demo());

        assert!(screen.contains("tui spike"), "title missing:\n{screen}");
        assert!(
            screen.contains("playing"),
            "transport state missing:\n{screen}"
        );
        assert!(screen.contains("2.000 s"), "position missing:\n{screen}");
        assert!(
            screen.contains("audio: 48000 Hz"),
            "audio line missing:\n{screen}"
        );
        assert!(screen.contains("ch0"), "channel 0 row missing:\n{screen}");
        assert!(screen.contains("master"), "master row missing:\n{screen}");
        assert!(screen.contains("0.88"), "live level missing:\n{screen}");
        assert!(screen.contains("0.50"), "master level missing:\n{screen}");
        assert!(
            screen.contains("Play"),
            "transport buttons missing:\n{screen}"
        );
    }

    #[test]
    fn the_help_overlay_lists_every_key() {
        let mut app = App::demo();
        app.help = true;

        // The overlay is scrollable (the keymap outgrew a 24-row terminal) and ratatui
        // scrolls *wrapped lines*, so a row index is not a scroll offset. "Every key is
        // listed" is therefore checked as **reachability**: at some scroll position the
        // entry's key token starts a line of the overlay. Collected across every scroll
        // stop, so adding a binding cannot shift a row out of the check.
        let help: Vec<(&str, &str)> = workflow::help().collect();
        let mut key_lines: Vec<String> = Vec::new();
        // Every scroll stop the keys can reach (`j` clamps at three wrapped lines per
        // entry), so a row cannot hide behind a count change.
        for scroll in 0..=workflow::help_len() * 3 {
            app.help_scroll = scroll;
            let screen = rendered(&mut app);
            for line in screen.lines() {
                key_lines.push(line.trim_start_matches(['│', ' ', '┌', '└']).to_string());
            }
        }
        for (index, (keys, _)) in help.iter().enumerate() {
            let token = keys.split_whitespace().next().expect("a key");
            // The entry's key column sits just inside the overlay's border, and the
            // overlay is drawn over other panels — so the token is matched **after a
            // border**, not at the start of the screen line.
            let after_border = |l: &String| {
                l.contains(&format!("│{token}")) || l.contains(&format!("│ {token}"))
            };
            assert!(
                key_lines.iter().any(after_border),
                "key `{token}` (row {index}) is not reachable in the overlay"
            );
        }

        // Scrolling is bounded and `j`/`k` move it while the overlay is modal.
        app.help_scroll = 0;
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        assert_eq!(app.help_scroll, 1, "j scrolls the overlay");
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()));
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()));
        assert_eq!(app.help_scroll, 0, "k stops at the top");
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert!(
            app.help,
            "keys other than scroll/close do nothing while help is open"
        );
        assert!(
            app.last_command.is_none(),
            "no command leaked through the overlay"
        );
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(!app.help, "Esc closes the overlay");
        assert!(!app.quit, "and still never quits");
    }

    #[test]
    fn a_click_on_a_transport_button_maps_to_its_action() {
        let mut app = App::demo();
        let _ = rendered(&mut app);

        let (play, _) = app
            .buttons
            .iter()
            .find(|(_, action)| *action == Button::Play)
            .expect("the play button was laid out");

        let centre = Position::new(play.x + play.width / 2, play.y);
        assert_eq!(app.action_at(centre), Some(Button::Play));

        // Outside every button nothing happens — no accidental transport hits.
        assert_eq!(app.action_at(Position::new(0, 25)), None);
    }

    #[test]
    fn a_click_on_a_meter_row_selects_that_channel() {
        let mut app = App::demo();
        let _ = rendered(&mut app);
        assert_eq!(app.selected, 1);

        let (_, row) = app.meter_rows.last().copied().expect("meter rows recorded");
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: row.x + 2,
            row: row.y,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.selected, 3, "the last channel row selects channel 3");
    }

    #[test]
    fn keys_drive_the_transport_through_the_host() {
        let mut app = App::demo();
        app.snap.playing = false;

        // The idle host answers the command; a refusal would land in `status`
        // and no command would be recorded.
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()));
        assert!(
            !app.status.contains("refused"),
            "play was refused: {}",
            app.status
        );
        assert!(
            matches!(app.last_command, Some(("play", _))),
            "space did not run the play command: {:?}",
            app.last_command
        );

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty()));
        assert!(app.quit, "q quits");
    }

    #[test]
    fn the_timeline_panel_draws_a_real_file() {
        let _pool = pool_guard();
        let path = wav_fixture("panel");
        let mut app = App::demo();
        app.open_wave(&path);
        let screen = rendered(&mut app);

        assert!(
            screen.contains("timeline —"),
            "no timeline panel:\n{screen}"
        );
        assert!(
            screen.contains(path.file_name().unwrap().to_str().unwrap()),
            "the file name is not shown:\n{screen}"
        );
        assert!(screen.contains("ms/col"), "no density readout:\n{screen}");
        assert!(screen.contains("NORMAL"), "no mode indicator:\n{screen}");
        assert!(
            screen.contains('⣿') || screen.contains('⣤') || screen.contains('⠿'),
            "no braille envelope was drawn:\n{screen}"
        );
        assert!(
            screen.contains('┃'),
            "no playhead (the synthetic transport sits at 2.000 s):\n{screen}"
        );

        // A **named** clip and a marker are drawn on the panel: the label on the clip and
        // the marker's glyph + name on the ruler. (The demo arrangement's clip is
        // `wave`, drawn from its id until it is named.)
        let playhead = app.snap.frame;
        app.prompt = Some("arrange rename_clip t0 c0 take-1".to_string());
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        app.prompt = Some(format!("arrange set_marker {} intro", playhead));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        let screen = rendered(&mut app);
        assert!(
            screen.contains("take-1"),
            "the clip's name is drawn on its lane:\n{screen}"
        );
        assert!(
            screen.contains('▼') && screen.contains("intro"),
            "the marker's glyph and name are drawn on the ruler:\n{screen}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn visual_mode_selects_from_the_playhead_and_escape_leaves_it() {
        let _pool = pool_guard();
        let path = wav_fixture("visual");
        let mut app = App::demo();
        app.open_wave(&path);
        let _ = rendered(&mut app);

        assert_eq!(app.mode, Mode::Normal);

        // `v` anchors the selection at the playhead (the synthetic 2.000 s).
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::empty()));
        assert_eq!(app.mode, Mode::Visual);
        let view = app.view.as_ref().expect("a view");
        assert_eq!(view.selection, Some((app.snap.frame, app.snap.frame)));

        // `l` extends the head, leaving the anchor alone.
        let before = view.selection.expect("selection");
        app.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()));
        let after = app
            .view
            .as_ref()
            .expect("a view")
            .selection
            .expect("selection");
        assert_eq!(after.0, before.0, "the anchor does not move");
        assert!(after.1 > before.1, "the head extends forward: {after:?}");

        // The mode and the span are on screen while it is active.
        let screen = rendered(&mut app);
        assert!(screen.contains("VISUAL"), "mode not shown:\n{screen}");
        assert!(screen.contains("sel "), "selection not shown:\n{screen}");

        // Esc leaves visual mode and drops the selection — and never quits, no
        // matter how often it is pressed (the safety key must not be fatal).
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.view.as_ref().expect("a view").selection.is_none());
        assert!(!app.quit, "Esc must not quit");

        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(!app.quit, "Esc never quits, however often it is pressed");

        // `q` is the way out.
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty()));
        assert!(app.quit, "q quits");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn zoom_keys_change_the_density_without_touching_the_host() {
        let _pool = pool_guard();
        let path = wav_fixture("zoom-keys");
        let mut app = App::demo();
        app.open_wave(&path);
        let _ = rendered(&mut app);

        let fitted = app.view.as_ref().expect("a view").frames_per_cell;
        app.on_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::empty()));
        assert!(
            app.view.as_ref().expect("a view").frames_per_cell < fitted,
            "zoom in narrows the column"
        );
        assert!(
            app.last_command.is_none(),
            "the viewport is free to move — no host command"
        );

        app.on_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::empty()));
        assert_eq!(
            app.view.as_ref().expect("a view").frames_per_cell,
            fitted,
            "0 refits the file"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tab_cycles_the_focus_and_scopes_the_timeline_keys() {
        let _pool = pool_guard();
        let path = wav_fixture("focus");
        let mut app = App::demo();
        assert_eq!(app.focus, Panel::Mixer, "no arrangement → the mixer");

        app.open_wave(&path);
        let _ = rendered(&mut app);
        assert_eq!(app.focus, Panel::Timeline, "loading adopts the timeline");

        // The ring is every panel that *exists*: mixer → timeline → pool → mixer,
        // because a loaded `--wave` session has both an arrangement and a pool.
        assert!(!app.pool.is_empty(), "the wave import filled the pool");
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(app.focus, Panel::Pool, "the pool is in the ring");
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(app.focus, Panel::Mixer);

        // With the mixer focused, timeline keys are inert.
        let before = app.view.as_ref().expect("a view").start;
        app.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()));
        assert_eq!(
            app.view.as_ref().expect("a view").start,
            before,
            "the timeline does not scroll while the mixer has the keys"
        );

        // Shift-Tab (BackTab) goes back through the pool, and then the keys act.
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty()));
        assert_eq!(app.focus, Panel::Pool);
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty()));
        assert_eq!(app.focus, Panel::Timeline);
        let before = app.view.as_ref().expect("a view").frames_per_cell;
        app.on_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::empty()));
        assert!(
            app.view.as_ref().expect("a view").frames_per_cell < before,
            "zoom is live again"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A pool with one three-second source, and a script that places it as one
    /// clip on one track — plus whatever `extra` lines the test needs.
    fn pool_script(name: &str, extra: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let pool =
            std::env::temp_dir().join(format!("tui-shell-pool-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&pool).expect("pool dir");

        let source = pool.join("s1.wav");
        let mut writer =
            media::wav::WavWriter::create_float(&source, 48_000, 1).expect("fixture source");
        let samples: Vec<f32> = (0..144_000)
            .map(|i| (i as f32 * 0.05).sin() * 0.7)
            .collect();
        writer.write(&samples).expect("write");
        writer.finalize().expect("finalize");

        let script_path = pool.join("arrangement.script");
        let script = format!(
            "host v1\nmount mixer channels=2 @0\npool {}\narrange add_track t0\narrange add_clip t0 c0 s1 0 144000 0 0 0 1.0\n{extra}",
            pool.display()
        );
        std::fs::write(&script_path, script).expect("write the script");

        (pool, script_path)
    }

    #[test]
    fn a_loaded_file_is_held_by_the_host_so_it_can_play() {
        let _pool = pool_guard();
        let path = wav_fixture("audible");
        let mut app = App::idle();
        app.open_wave(&path);

        // The arrangement is the **host's**, not the shell's picture of a file the
        // host has never seen — which is what makes it audible: the pump renders
        // this value through the mixer.
        let outcome = app.host.outcome().expect("outcome");
        assert_eq!(outcome.mixer_channels, Some(2), "{}", app.status);
        let timeline = outcome.arrangement.expect("the host holds an arrangement");
        assert_eq!(timeline.tracks.len(), 1);
        assert_eq!(timeline.tracks[0].clips.len(), 1);
        assert_eq!(
            timeline.tracks[0].clips[0].source,
            path.file_stem().unwrap().to_str().unwrap()
        );
        assert_eq!(timeline.tracks[0].clips[0].src_len, 144_000);
        assert_eq!(app.arrangement.as_ref().expect("the panel").clip_count(), 1);

        let _ = std::fs::remove_file(&path);
    }

    /// The reported bug: `--wave` on a 44.1 kHz file used to stop the transport
    /// with a rate-mismatch error. The shell now imports external material into
    /// its session pool at the session rate, so the clip is a straight read.
    #[test]
    fn a_forty_four_one_khz_file_is_imported_at_the_session_rate() {
        let _pool = pool_guard();
        let path = std::env::temp_dir().join(format!("tui-shell-44100-{}.wav", std::process::id()));
        let rate = 44_100u32;
        let frames = rate as usize;
        let mut writer =
            media::wav::WavWriter::create_float(&path, rate, 1).expect("create the fixture");
        let samples: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        writer.write(&samples).expect("write");
        writer.finalize().expect("finalize");

        let mut app = App::idle();
        app.open_wave(&path);

        assert!(
            app.status.contains("44100 → 48000"),
            "the import is not reported: {}",
            app.status
        );
        assert!(
            !app.status.contains("rate-mismatched") && !app.status.contains("refused"),
            "the file was refused instead of converted: {}",
            app.status
        );

        // The host holds the imported source at the session rate: one second of
        // 44.1 kHz material is 48 000 frames of the session's timeline.
        let timeline = app
            .host
            .outcome()
            .expect("outcome")
            .arrangement
            .expect("the host holds an arrangement");
        assert_eq!(timeline.tracks[0].clips[0].src_len, 48_000);
        assert_eq!(app.arrangement.as_ref().expect("the panel").frames, 48_000);

        // …and the panel draws the imported source (its waveform, not a hole).
        let screen = rendered(&mut app);
        assert!(screen.contains("ms/col"), "no timeline:\n{screen}");
        assert!(
            screen.contains('⣿') || screen.contains('⣤') || screen.contains('⠿'),
            "no envelope for the imported source:\n{screen}"
        );

        let _ = std::fs::remove_file(&path);
        drop_pool();
    }

    /// `b` arms the snap grid, and the state line says which division is live —
    /// an invisible grid is a trap (an edit lands somewhere unexplained), so the
    /// mode, like the mode indicator, is always on screen.
    #[test]
    fn the_grid_cycles_from_the_keyboard_and_is_shown() {
        let mut app = App::demo();
        let press =
            |app: &mut App| app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));

        assert!(!app.grid.is_on(), "off until asked");
        assert!(rendered(&mut app).contains("grid off"));

        for label in ["grid bar", "grid beat", "grid 1/2", "grid 1/4"] {
            press(&mut app);
            let screen = rendered(&mut app);
            assert!(
                screen.contains(label),
                "the state line must show `{label}`:\n{screen}"
            );
            assert!(app.status.contains("snap grid"), "{}", app.status);
        }
        press(&mut app);
        assert!(!app.grid.is_on(), "the cycle returns to off");
        assert!(app.status.contains("snap grid off"), "{}", app.status);
    }

    /// The grid quantizes in the **beat** domain through the session's tempo map:
    /// at the demo's 120 bpm and 48 kHz a beat is 24 000 frames and a 4/4 bar is
    /// 96 000, so an edit either lands on a musical line or is left alone.
    #[test]
    fn the_grid_quantizes_frames_in_the_beat_domain() {
        let mut app = App::demo();
        assert_eq!(app.snap_frame(96_123), 96_123, "no grid, no snapping");

        let press =
            |app: &mut App| app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
        press(&mut app); // bar
        assert_eq!(app.snap_frame(96_000), 96_000, "a bar line stays put");
        assert_eq!(app.snap_frame(150_000), 192_000, "150 000 is nearest bar 2");
        assert_eq!(
            app.snap_frame(47_000),
            0,
            "the first bar wins below the half"
        );
        assert_eq!(app.snap_frame(49_000), 96_000);

        press(&mut app); // beat
        assert_eq!(app.snap_frame(96_123), 96_000, "the beat grid is finer");
        assert_eq!(app.snap_frame(108_100), 120_000);
        assert_eq!(app.grid_step_frames(96_000), 24_000, "one beat at 120 bpm");

        press(&mut app); // 1/2
        assert_eq!(app.grid_step_frames(96_000), 12_000);
        press(&mut app); // 1/4
        assert_eq!(app.grid_step_frames(96_000), 6_000);
    }

    /// The grid is quantized in the **beat** domain through the session's tempo
    /// map, so a tempo change moves the lines with the music instead of leaving
    /// them at fixed frames. (Snapping in the frame domain would drift the moment
    /// the tempo changed — the reason the plan puts the math in beats.)
    #[test]
    fn the_grid_follows_a_tempo_change() {
        let mut app = App::demo();
        // A bar grid, then a tempo change at 96 000 (bar 2 start): 120 → 60 bpm,
        // so beats are twice as long from there on.
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
        assert_eq!(app.grid.label(), "bar");
        app.snap.tempo_map.push(96_000, 60.0, 4);
        // At 60 bpm a beat is 48 000 frames and a bar 192 000.
        assert_eq!(app.grid_step_frames(96_000), 192_000);
        assert_eq!(app.snap_frame(96_000), 96_000, "the boundary is a bar line");
        assert_eq!(
            app.snap_frame(150_000),
            96_000,
            "nearest bar is still bar 2"
        );
        assert_eq!(app.snap_frame(250_000), 288_000, "and then bar 3");
        assert_eq!(app.snap_frame(300_000), 288_000);

        // The meter comes from the map too: in 3/4 a "bar" is three beats, so
        // the lines are 72 000 frames apart at 120 bpm.
        let mut app = App::demo();
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
        app.snap.tempo_map = host::TempoMap::new(48_000, 120.0, 3);
        assert_eq!(app.grid_step_frames(0), 72_000);
        assert_eq!(app.snap_frame(0), 0);
        assert_eq!(app.snap_frame(100_000), 72_000, "nearest 3/4 bar");
        assert_eq!(app.snap_frame(200_000), 216_000);
    }

    /// `[`/`]` step the playhead to the previous/next grid line, so a seek is a
    /// musical motion, not a scroll nudge. With the grid off it is one beat.
    #[test]
    fn the_brackets_step_the_playhead_a_grid_line() {
        let mut app = App::demo();
        // `[`/`]` are timeline keys: the focus is what scopes a key to a panel.
        app.focus = Panel::Timeline;
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // Unarmed, the step is one beat from where the playhead is (100 000 +
        // 24 000): the keys always move it somewhere musical.
        app.snap.frame = 100_000;
        press(&mut app, ']');
        assert!(app.status.contains("124000"), "{}", app.status);
        press(&mut app, '[');
        assert!(app.status.contains("76000"), "{}", app.status);

        // Armed at a beat, they land on the grid lines either side.
        press(&mut app, 'b'); // bar
        press(&mut app, 'b'); // beat
        assert_eq!(app.grid.label(), "beat");
        app.snap.frame = 100_000;
        press(&mut app, ']');
        assert!(
            app.status.contains("120000"),
            "`]` takes the next grid line: {}",
            app.status
        );
        // The transport follows the seek (the host publishes the new frame).
        app.snap.frame = 120_000;
        press(&mut app, '[');
        assert!(
            app.status.contains("96000"),
            "`[` takes the previous grid line: {}",
            app.status
        );

        // A coarse grid moves further: a bar is 96 000 frames at 120 bpm in 4/4.
        press(&mut app, 'b'); // 1/2
        press(&mut app, 'b'); // 1/4
        press(&mut app, 'b'); // off
        press(&mut app, 'b'); // bar
        assert_eq!(app.grid.label(), "bar");
        app.snap.frame = 100_000;
        press(&mut app, ']');
        assert!(app.status.contains("192000"), "{}", app.status);
    }

    /// The ruler is the grid made visible: armed, it draws bar lines with their
    /// number and beat ticks; off, it stays the seconds ruler.
    #[test]
    fn the_ruler_draws_bars_and_beats_when_the_grid_is_armed() {
        /// The ruler row: the line above the first lane (the lane gutter marks it).
        fn ruler_row(screen: &str) -> String {
            screen
                .lines()
                .zip(screen.lines().skip(1))
                .find(|(_, next)| next.contains('▸'))
                .map(|(row, _)| row.to_string())
                .unwrap_or_default()
        }

        let _pool = pool_guard();
        let path = wav_fixture("ruler");
        let mut app = App::idle();
        app.open_wave(&path);

        let seconds = ruler_row(&rendered(&mut app));
        assert!(
            seconds.contains("|0.50"),
            "the ungridded ruler is the seconds ruler:\n{seconds}"
        );
        assert!(!seconds.contains('·'), "no beat ticks without a grid");

        for _ in 0..2 {
            app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
        }
        assert_eq!(app.grid.label(), "beat");
        let musical = ruler_row(&rendered(&mut app));
        assert!(
            musical.contains('·'),
            "the beat grid draws beat ticks:\n{musical}"
        );
        // Bar 1 sits under the playhead marker (`┃`), so check bar 2 — the
        // ruler is labelled with bar numbers, not seconds.
        assert!(musical.contains("|2"), "bar 2 is labelled:\n{musical}");
        assert!(
            !musical.contains("|0.50"),
            "the musical ruler replaces the seconds one:\n{musical}"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A **stereo** file keeps both of its channels: `--wave` splits it into one
    /// pool source per channel, places each on its own track, and pans them hard
    /// left/right through the host's own script — so the right channel is played
    /// instead of silently dropped.
    #[test]
    fn a_stereo_file_is_split_across_two_panned_tracks() {
        let _pool = pool_guard();
        let stem = format!("tui-shell-stereo-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("{stem}.wav"));
        let rate = 48_000u32;
        let frames = rate as usize;
        let mut writer =
            media::wav::WavWriter::create_float(&path, rate, 2).expect("create the fixture");
        let mut interleaved = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (i as f32 * 0.05).sin() * 0.5;
            interleaved.push(s); // left
            interleaved.push(-s); // right: the same tone, inverted
        }
        writer.write(&interleaved).expect("write");
        writer.finalize().expect("finalize");

        let mut app = App::idle();
        app.open_wave(&path);

        let outcome = app.host.outcome().expect("outcome");
        let timeline = outcome.arrangement.expect("the host holds an arrangement");
        assert_eq!(
            timeline.tracks.len(),
            2,
            "one track per channel: {}",
            app.status
        );
        assert_eq!(timeline.tracks[0].clips[0].source, format!("{stem}.ch0"));
        assert_eq!(timeline.tracks[1].clips[0].source, format!("{stem}.ch1"));
        assert_eq!(timeline.tracks[0].clips[0].src_len, rate as u64);
        assert!(
            app.status.contains("2 channels split into"),
            "the split is reported: {}",
            app.status
        );

        // The pan is a **logged** parameter, not a shell-side guess.
        let pan = |param: &str| {
            outcome
                .params
                .iter()
                .find(|(plugin, name, _)| *plugin == "mixer" && *name == param)
                .map(|(_, _, value)| *value)
        };
        assert_eq!(pan("ch0.pan"), Some(-1.0), "left channel hard left");
        assert_eq!(pan("ch1.pan"), Some(1.0), "right channel hard right");

        // The panel resolves both sources from the pool listing (no read errors).
        let arrangement = app.arrangement.as_ref().expect("the panel");
        assert_eq!(arrangement.lanes.len(), 2);
        assert_eq!(arrangement.frames, rate as u64);

        // A mono file still lands on one track — the split is channel-count driven.
        let mono = wav_fixture("still-mono");
        app.open_wave(&mono);
        let outcome = app.host.outcome().expect("outcome");
        assert_eq!(
            outcome.arrangement.expect("arrangement").tracks.len(),
            1,
            "a mono file is one track: {}",
            app.status
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&mono);
        drop_pool();
    }

    #[test]
    fn faders_are_read_from_the_log_and_survive_an_undo() {
        let (pool, script) = pool_script(
            "params",
            "set_param mixer ch0.gain 0.3 @0\nset_param mixer master.gain 0.6 @0\nset_param mixer ch1.mute 1 @0\n",
        );
        let mut app = App::idle();
        app.snap = App::demo_snapshot(); // the playhead lands inside the clip
        app.open_script(&script);

        // Reading: the console reflects the **log**, not a default.
        assert_eq!(app.mixer.value(0), 0.3, "ch0 gain: {}", app.status);
        assert_eq!(app.mixer.value(2), 0.6, "strip 2 is the master");
        assert!(app.mixer.muted(1), "ch1 mute");

        // A split edits the arrangement…
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert_eq!(
            app.arrangement
                .as_ref()
                .expect("an arrangement")
                .clip_count(),
            2,
            "{}",
            app.status
        );

        // …and undo replays the log: the clip returns and the faders stay where
        // the log says they are (a fader ride is not an arrangement edit).
        app.snap.can_undo = true; // the live host publishes this; the demo snapshot does not
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        assert_eq!(
            app.arrangement
                .as_ref()
                .expect("an arrangement")
                .clip_count(),
            1,
            "undo restores the clip: {}",
            app.status
        );
        assert_eq!(app.mixer.value(0), 0.3, "the fader survives the replay");
        assert!(app.mixer.muted(1), "the mute survives the replay");

        let _ = std::fs::remove_dir_all(&pool);
    }

    #[test]
    fn edits_go_through_the_hosts_arrange_language() {
        let (pool, script_path) = pool_script("edits", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot(); // playhead at 2.000 s: inside the clip
        app.open_script(&script_path);

        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        assert_eq!(arrangement.lanes.len(), 1, "{}", app.status);
        assert_eq!(arrangement.clip_count(), 1, "{}", app.status);
        assert_eq!(arrangement.frames, 144_000);
        assert_eq!(app.active_track, 0);

        // `x` splits through the host and re-reads the host's value.
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert!(
            matches!(app.last_command, Some(("arrange", _))),
            "split did not dispatch an arrange command: {:?} / {}",
            app.last_command,
            app.status
        );
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").clip_count(),
            2,
            "the host split the clip: {}",
            app.status
        );

        // `d` deletes the clip under the playhead.
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty()));
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").clip_count(),
            1,
            "the host deleted a clip: {}",
            app.status
        );

        // With nothing under the playhead, the key reports instead of panicking.
        app.snap.frame = 1_000_000;
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert!(app.status.contains("no clip"), "status: {}", app.status);

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// The gestures that were missing: move (in time, across tracks) and trim.
    /// Every one of them goes through the host's `arrange` language, and the
    /// host's value — not the shell's guess — is what the assertions read.
    #[test]
    fn clips_move_and_trim_through_the_arrange_language() {
        let (pool, script_path) = pool_script("moves", "arrange add_track t1\n");
        let mut app = App::idle();
        app.snap = App::demo_snapshot(); // the playhead sits at 2.000 s (96 000)
        app.open_script(&script_path);

        let clip = |app: &App| {
            app.arrangement.as_ref().expect("an arrangement").lanes[0]
                .clips
                .first()
                .cloned()
        };

        // `>` trims the clip's end to the playhead: 3 s of source becomes 2 s.
        app.on_key(KeyEvent::new(KeyCode::Char('>'), KeyModifiers::empty()));
        assert!(
            matches!(app.last_command, Some(("arrange", _))),
            "{}",
            app.status
        );
        assert_eq!(
            clip(&app).expect("a clip").src_len,
            96_000,
            "{}",
            app.status
        );

        // …and `u` unwinds it: a trim is a logged edit like any other.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        assert_eq!(
            clip(&app).expect("a clip").src_len,
            144_000,
            "{}",
            app.status
        );

        // `<` trims the *start* to the playhead, carrying the audio with it.
        app.on_key(KeyEvent::new(KeyCode::Char('<'), KeyModifiers::empty()));
        let c = clip(&app).expect("a clip");
        assert_eq!(
            (c.at_frame, c.src_start, c.src_len),
            (96_000, 96_000, 48_000),
            "{}",
            app.status
        );

        // `t` in visual mode trims to the **selection** — select, then act.
        app.view.as_mut().expect("a view").selection = Some((100_000, 120_000));
        app.mode = Mode::Visual;
        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::empty()));
        let c = clip(&app).expect("a clip");
        assert_eq!(
            (c.at_frame, c.src_start, c.src_len),
            (100_000, 100_000, 20_000),
            "the selection became the clip: {}",
            app.status
        );
        assert_eq!(app.mode, Mode::Normal, "the action consumed the selection");
        assert!(
            app.view.as_ref().expect("a view").selection.is_none(),
            "the selection is dropped"
        );

        // **One gesture, one undo**: the two trims are a single history entry.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        let restored = clip(&app).expect("a clip");
        assert_eq!(
            (restored.at_frame, restored.src_start, restored.src_len),
            (96_000, 96_000, 48_000),
            "one undo restores the clip whole (not half-trimmed): {}",
            app.status
        );
        // …and one redo re-applies the whole gesture. The shell's redo guard reads
        // the *published* snapshot (the event loop refreshes it every frame, and the
        // tests' snapshot is a synthetic one), so refresh it the same way first.
        app.snap = app.host.snapshot();
        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let again = clip(&app).expect("a clip");
        assert_eq!(
            (again.at_frame, again.src_start, again.src_len),
            (100_000, 100_000, 20_000),
            "one redo re-applies the gesture: {}",
            app.status
        );

        // `H` / `L` nudge by one beat — 24 000 frames at 120 bpm — and clamp at 0.
        // The playhead follows the clip's start so each hop has a clip under it.
        let hop = |app: &mut App, key: char| {
            let at = clip(app).expect("a clip").at_frame;
            app.snap.frame = at;
            app.on_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::empty()));
        };
        hop(&mut app, 'H');
        assert_eq!(
            clip(&app).expect("a clip").at_frame,
            76_000,
            "{}",
            app.status
        );
        hop(&mut app, 'L');
        assert_eq!(
            clip(&app).expect("a clip").at_frame,
            100_000,
            "{}",
            app.status
        );
        for _ in 0..8 {
            hop(&mut app, 'H');
        }
        assert_eq!(
            clip(&app).expect("a clip").at_frame,
            0,
            "a nudge left clamps at frame 0: {}",
            app.status
        );

        // Put it back where the J/K assertions expect it, then move it down a
        // track: `J` keeps the time and the cursor follows the clip; `K` returns.
        app.snap.frame = 0;
        app.arrange("move_clip t0 c0 100000");
        app.snap.frame = 100_000;
        app.on_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::empty()));
        {
            let arrangement = app.arrangement.as_ref().expect("an arrangement");
            assert_eq!(arrangement.lanes[0].clips.len(), 0, "{}", app.status);
            assert_eq!(arrangement.lanes[1].clips.len(), 1, "{}", app.status);
            assert_eq!(
                arrangement.lanes[1].clips[0].at_frame, 100_000,
                "a track move keeps the clip's time"
            );
        }
        assert_eq!(app.active_track, 1, "the cursor follows the clip");
        app.on_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::empty()));
        assert_eq!(app.active_track, 0);
        assert_eq!(
            app.arrangement.as_ref().expect("an arrangement").lanes[1]
                .clips
                .len(),
            0,
            "{}",
            app.status
        );

        // At the top of the stack `K` reports instead of panicking.
        app.on_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::empty()));
        assert!(
            app.status.contains("no track above"),
            "status: {}",
            app.status
        );
        assert_eq!(app.active_track, 0);

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// `g`/`G` step the clip's gain in dB and `f`/`F` put its fades at the
    /// playhead — both are `arrange` lines like every other edit, and the model's own
    /// limits (gain range, `fade_in + fade_out <= src_len`) are respected with the
    /// clamp reported rather than refused silently.
    #[test]
    fn clip_gain_and_fades_are_keys() {
        let (pool, script_path) = pool_script("gainfade", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot(); // the playhead sits at 2.000 s (96 000)
        app.open_script(&script_path);

        let clip = |app: &App| {
            app.arrangement.as_ref().expect("an arrangement").lanes[0]
                .clips
                .first()
                .cloned()
                .expect("a clip")
        };

        // Gain: -1 dB then +1 dB is back to unity (within the log's rounding).
        app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty()));
        let gain = clip(&app).gain;
        assert!(
            (gain - 10f32.powf(-1.0 / 20.0)).abs() < 1e-3,
            "-1 dB: {gain} ({})",
            app.status
        );
        app.on_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::empty()));
        assert!((clip(&app).gain - 1.0).abs() < 1e-3, "{}", app.status);

        // Fades: `f` puts the fade-in at the playhead, `F` the fade-out, and the
        // two together exactly fill the clip (96 000 + 48 000 = 144 000).
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::empty()));
        assert_eq!(clip(&app).fade_in, 96_000, "{}", app.status);
        app.on_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::empty()));
        let c = clip(&app);
        assert_eq!((c.fade_in, c.fade_out), (96_000, 48_000), "{}", app.status);

        // The model's sum rule caps a fade instead of letting the host refuse it:
        // at 2.5 s the fade-in wants 120 000, but only 96 000 is left.
        app.snap.frame = 120_000;
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::empty()));
        assert_eq!(clip(&app).fade_in, 96_000, "{}", app.status);
        assert!(
            app.status.contains("capped"),
            "the cap is reported: {}",
            app.status
        );

        // With the grid armed, a fade point that moved says where it came from —
        // every playhead-based edit reports the snap, or a grid looks like a bug.
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty())); // bar
        app.snap.frame = 100_000; // off the bar grid (96 000 / 192 000)
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::empty()));
        assert!(
            app.status.contains("snapped from 100000"),
            "a snapped fade is reported: {}",
            app.status
        );
        assert_eq!(clip(&app).fade_in, 96_000, "{}", app.status);

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **Copy and paste are one gesture each.** `y` takes the clip under the
    /// playhead (or the selection) into the shell's clipboard — a value, so nothing
    /// is logged — and `p` puts it on the active track at the playhead as one
    /// `group`, so the whole paste is one undo step. New boundaries get the click
    /// guard (a copied fade would be kept).
    #[test]
    fn copy_and_paste_is_one_logged_gesture() {
        let (pool, script_path) = pool_script("paste", "arrange add_track t1\n");
        let mut app = App::idle();
        app.snap = App::demo_snapshot(); // the playhead sits at 2.000 s (96 000)
        app.open_script(&script_path);

        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // Copy: the clip under the playhead (no selection is open).
        press(&mut app, 'y');
        assert!(app.status.contains("copied 1 clip"), "{}", app.status);
        assert_eq!(app.clipboard.len(), 1, "the clipboard is a shell value");
        assert!(
            app.clipboard[0].1.source_id == "s1",
            "the source id travels"
        );

        // Paste onto the second track, at the (ungridded) playhead.
        app.active_track = 1;
        press(&mut app, 'p');
        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        let pasted = arrangement.lanes[1].clips.first().expect("a pasted clip");
        assert_eq!(
            pasted.id, "paste.1",
            "ids are minted by the shell: {}",
            app.status
        );
        assert_eq!(pasted.source_id, "s1");
        assert_eq!(pasted.at_frame, 96_000);
        assert_eq!(pasted.src_len, 144_000);
        assert_eq!(
            (pasted.fade_in, pasted.fade_out),
            (MICRO_FADE, MICRO_FADE),
            "new boundaries get the click guard"
        );
        assert_eq!(pasted.gain, 1.0);
        assert!(arrangement.lanes[0].clips.len() == 1, "the original stayed");

        // The paste is **one** undo step: `u` removes it whole.
        press(&mut app, 'u');
        assert!(
            app.arrangement.as_ref().expect("arrangement").lanes[1]
                .clips
                .is_empty(),
            "one undo removes the pasted clip: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// `c` copies **and** deletes, as one gesture, and `P` pastes *appended* after
    /// the active track's last clip — the owner's "append", which needs no op of
    /// its own (the target frame is computed shell-side).
    #[test]
    fn cut_and_append_are_gestures_over_existing_ops() {
        let (pool, script_path) = pool_script(
            "cutappend",
            "arrange add_clip t0 c1 s1 0 48000 144000 0 0 1.0\n",
        );
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // Put the playhead inside the first clip and append it after the last one.
        app.snap.frame = 10_000;
        press(&mut app, 'y');
        press(&mut app, 'P');
        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        let ids: Vec<&str> = arrangement.lanes[0]
            .clips
            .iter()
            .map(|clip| clip.id.as_str())
            .collect();
        assert_eq!(ids, vec!["c0", "c1", "paste.1"], "{}", app.status);
        assert_eq!(
            arrangement.lanes[0].clips[2].at_frame, 192_000,
            "appended after the track's last clip (144 000 + 48 000)"
        );

        // Cut the first clip: one gesture, one undo step, and the clipboard keeps
        // it so `p` can put it back.
        press(&mut app, 'u'); // undo the paste first
        app.snap.frame = 10_000;
        press(&mut app, 'c');
        assert!(
            app.status.contains("cut 1 clip"),
            "the cut is reported: {}",
            app.status
        );
        assert!(
            !app.arrangement.as_ref().expect("arrangement").lanes[0]
                .clips
                .iter()
                .any(|clip| clip.id == "c0"),
            "the cut removed it: {}",
            app.status
        );
        press(&mut app, 'u');
        assert!(
            app.arrangement.as_ref().expect("arrangement").lanes[0]
                .clips
                .iter()
                .any(|clip| clip.id == "c0"),
            "one undo restores the cut: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// A grid-armed paste lands on the grid and says so, and the clipboard survives
    /// the snap (the paste target is quantized once; the copied clips keep their
    /// relative spacing).
    #[test]
    fn a_paste_lands_on_the_armed_grid() {
        let (pool, script_path) = pool_script("pastegrid", "arrange add_track t1\n");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        press(&mut app, 'b'); // bar grid (96 000 frames at 120 bpm, 4/4)
        assert_eq!(app.grid.label(), "bar");
        press(&mut app, 'y');
        app.active_track = 1;
        app.snap.frame = 150_000; // between bars: nearest is 192 000
        press(&mut app, 'p');
        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        assert_eq!(arrangement.lanes[1].clips[0].at_frame, 192_000);
        assert!(
            app.status.contains("snapped from 150000"),
            "the snap is reported: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **A copied fade pair can be tight and the paste must still land.** The model
    /// requires `fade_in + fade_out <= src_len`; a clip with `fade_in = src_len` (or
    /// a fade that leaves less room than the guard) is legal, so the *guard* is what
    /// gives way — otherwise the paste mints an op the host refuses, the gesture
    /// fails, and the status would have claimed success.
    #[test]
    fn a_tight_copied_fade_pair_pastes_legally() {
        let (pool, script_path) = pool_script(
            "fadepair",
            // On t1 so the playhead can point at it (c0 covers t0's first 144 000).
            "arrange add_track t1\narrange add_clip t1 c9 s1 0 100 0 60 0 1.0\narrange set_clip_fade t0 c0 144000 0\n",
        );
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // A 100-frame clip with a 60-frame fade-in: 40 frames are left, less than
        // the 64-frame guard, so the pasted fade-out is 40.
        app.active_track = 1;
        app.snap.frame = 50;
        press(&mut app, 'y');
        app.snap.frame = 300;
        press(&mut app, 'p');
        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        let pasted = arrangement.lanes[1]
            .clips
            .iter()
            .find(|clip| clip.id.starts_with("paste."))
            .expect("the tight pair pasted");
        assert_eq!(pasted.src_len, 100);
        assert_eq!((pasted.fade_in, pasted.fade_out), (60, 40));
        assert!(
            pasted.fade_in + pasted.fade_out <= pasted.src_len,
            "the model's sum rule holds"
        );
        assert!(app.status.contains("pasted 1 clip"), "{}", app.status);

        // The extreme: a fade-in the whole clip long. The pasted clip gets no guard
        // at all (there is no room), and the paste still lands.
        app.active_track = 0;
        app.snap.frame = 10_000; // inside c0 (0..144 000)
        press(&mut app, 'y');
        app.active_track = 1;
        app.snap.frame = 1_000;
        press(&mut app, 'p');
        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        let pasted = arrangement.lanes[1]
            .clips
            .iter()
            .rev()
            .find(|clip| clip.id.starts_with("paste."))
            .expect("the full-length fade pasted");
        assert_eq!((pasted.fade_in, pasted.fade_out), (144_000, 0));
        assert!(app.status.contains("pasted 1 clip"), "{}", app.status);

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **A refused gesture must not announce success**, and a refusal must leave the
    /// session alone. `command` returns whether the host applied it; every gesture
    /// with its own success message checks it, and the clipboard's minted ids are
    /// seeded above anything the session names — so a paste collision cannot happen
    /// in the first place.
    #[test]
    fn a_refused_gesture_never_claims_success() {
        let (pool, script_path) = pool_script("refused", "arrange add_track t1\n");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // The mechanism every gesture uses: a refused command returns false and
        // leaves the refusal in the status line.
        assert!(
            !app.command(
                "test",
                HostCommand::Arrange {
                    op: media::ArrangeOp::Delete {
                        track: "t0".into(),
                        clip: "nope".into(),
                    },
                    at_frame: None,
                },
            ),
            "a refused command reports it"
        );
        assert!(app.status.contains("refused"), "{}", app.status);
        assert!(
            !app.arrange_group(&["delete t0 nope".to_string()]),
            "a refused group reports it"
        );

        // The ids are seeded, so pasting twice mints two ids and never collides —
        // even after an undo (the counter only moves forward).
        press(&mut app, 'y');
        app.active_track = 1;
        press(&mut app, 'p');
        press(&mut app, 'u');
        press(&mut app, 'p');
        let ids: Vec<String> = app.arrangement.as_ref().expect("an arrangement").lanes[1]
            .clips
            .iter()
            .map(|clip| clip.id.clone())
            .collect();
        assert_eq!(
            ids,
            vec!["paste.2"],
            "the id counter never rewinds: {}",
            app.status
        );

        // A clipboard from another session is refused **before** anything is minted,
        // and says which source is missing.
        app.clipboard = vec![(
            0,
            app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0].clone(),
        )];
        app.pool.clear();
        let before = app.arrangement.as_ref().expect("arrangement").lanes[1]
            .clips
            .len();
        press(&mut app, 'p');
        assert!(
            app.status.contains("no source"),
            "the missing source is named: {}",
            app.status
        );
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").lanes[1]
                .clips
                .len(),
            before,
            "nothing was added"
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// A **multi-clip** selection pastes as one gesture: both clips land, their
    /// relative spacing is preserved, and **one** undo removes both.
    #[test]
    fn a_multi_clip_paste_keeps_spacing_and_is_one_undo() {
        let (pool, script_path) = pool_script(
            "multipaste",
            "arrange add_track t1\narrange add_clip t0 c1 s1 0 24000 48000 0 0 1.0\n",
        );
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // c0 is 0..144 000 and c1 is 48 000..72 000. The selection motion has its
        // own tests; this one is about the paste, so the range is set directly.
        app.view.as_mut().expect("a view").selection = Some((0, 72_000));
        press(&mut app, 'y');
        assert!(
            app.status.contains("copied 2 clips"),
            "the selection took both: {}",
            app.status
        );
        assert_eq!(app.clipboard.len(), 2);

        app.active_track = 1;
        app.snap.frame = 0;
        press(&mut app, 'p');
        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        let ids: Vec<&str> = arrangement.lanes[1]
            .clips
            .iter()
            .map(|clip| clip.id.as_str())
            .collect();
        assert_eq!(ids, vec!["paste.1", "paste.2"], "{}", app.status);
        assert_eq!(
            arrangement.lanes[1]
                .clips
                .iter()
                .map(|clip| clip.at_frame)
                .collect::<Vec<_>>(),
            vec![0, 48_000],
            "the spacing between the copied clips is kept"
        );
        assert_eq!(app.clipboard[1].1.at_frame, 48_000, "and so is the origin");

        press(&mut app, 'u');
        assert!(
            app.arrangement.as_ref().expect("arrangement").lanes[1]
                .clips
                .is_empty(),
            "one undo removes the whole multi-clip paste: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **Tracks are part of the arrangement value, not a side table**: `a` adds one
    /// with a minted id and makes it active, `R` renames through the host's own
    /// format (the prompt is prefilled), `D` deletes the track *and its clips* as one
    /// gesture, and `{`/`}` reorder — the mixer channel follows the position, so the
    /// console reorders with the timeline.
    #[test]
    fn tracks_add_rename_delete_and_reorder() {
        let (pool, script_path) = pool_script(
            "tracks",
            "arrange add_track t1\narrange add_clip t0 c1 s1 0 24000 48000 0 0 1.0\n",
        );
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };
        let ids = |app: &App| -> Vec<String> {
            app.arrangement
                .as_ref()
                .expect("an arrangement")
                .lanes
                .iter()
                .map(|lane| lane.id.clone())
                .collect()
        };

        // Add: the first free id, and it becomes the track the keys act on. (The
        // demo mixer mounts 4 channels, so track index 2 exists and the status
        // names its channel.)
        press(&mut app, 'a');
        assert_eq!(ids(&app), vec!["t0", "t1", "t2"], "{}", app.status);
        assert_eq!(app.active_track, 2, "the new track is active");
        assert!(app.status.contains("ch2"), "{}", app.status);
        press(&mut app, 'u');
        assert_eq!(ids(&app), vec!["t0", "t1"]);

        // Rename: `R` opens the command line prefilled with the host's own line.
        app.active_track = 0;
        press(&mut app, 'R');
        let prefill = app.prompt.clone().expect("the prompt is open");
        assert_eq!(prefill, "arrange rename_track t0 ");
        for c in "lead".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(ids(&app), vec!["lead", "t1"], "{}", app.status);
        assert!(app.prompt.is_none(), "Enter closes the prompt");

        // Reorder: `}` moves the active track down and the view follows it; the
        // edges say so instead of sending a command.
        press(&mut app, '}');
        assert_eq!(ids(&app), vec!["t1", "lead"], "{}", app.status);
        assert_eq!(app.active_track, 1, "the moved track stays active");
        press(&mut app, '}');
        assert!(app.status.contains("already last"), "{}", app.status);
        press(&mut app, '{');
        assert_eq!(ids(&app), vec!["lead", "t1"], "{}", app.status);
        assert_eq!(app.active_track, 0);
        press(&mut app, '{');
        assert!(app.status.contains("already first"), "{}", app.status);

        // Delete: the track *and its two clips* go in one gesture, and one undo
        // brings the whole lot back.
        app.active_track = 0;
        press(&mut app, 'D');
        assert_eq!(ids(&app), vec!["t1"], "{}", app.status);
        assert!(app.status.contains("2 clips"), "{}", app.status);
        press(&mut app, 'u');
        assert_eq!(ids(&app), vec!["lead", "t1"], "the track is back");
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").lanes[0]
                .clips
                .len(),
            2,
            "and so are its clips"
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **The mixer's width is the limit, and the status says so.** Track `ti` feeds
    /// `ch{ti}`, so a track past the mounted channel count renders nothing — the
    /// value op is permissive, and the *shell* is where the limit is visible.
    #[test]
    fn a_track_past_the_mixers_width_is_reported() {
        let (pool, script_path) = pool_script(
            "narrow",
            // t0 from the fixture, plus two more: three tracks, so the next `a`
            // fills the mixer's fourth channel and the one after that does not.
            "arrange add_track t1\narrange add_track t2\n",
        );
        let mut app = App::idle();
        app.snap = App::demo_snapshot(); // the demo mixer mounts 4 channels
        app.open_script(&script_path);
        assert_eq!(app.snap.channel_count, 4);

        // The fourth track fits…
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").lanes.len(),
            4
        );
        assert!(app.status.contains("ch3"), "{}", app.status);

        // …the fifth cannot play, and the status says why and what to mount.
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").lanes.len(),
            5,
            "the value op stays permissive"
        );
        assert!(
            app.status.contains("mixer has 4 channels")
                && app.status.contains("mount mixer channels=5"),
            "the limit and its fix are named: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **The pool panel is where "load clips from the pool" happens.** `Tab` reaches
    /// it, `j`/`k` walk it, and `Enter` places the selected source on the active track
    /// at the playhead — a logged `add_clip` with the click guard, one undo away.
    #[test]
    fn the_pool_panel_places_a_source_on_the_timeline() {
        let _pool = pool_guard();
        let path = wav_fixture("poolplace");
        let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
        let mut app = App::idle();
        // The synthetic snapshot is the tests' transport/meter state; `undo` gates on
        // its `can_undo`, which the live pump would keep fresh between frames.
        app.snap = App::demo_snapshot();
        app.open_wave(&path);
        assert_eq!(app.focus, Panel::Timeline, "loading adopts the timeline");

        // The session pool is one directory per process, so other tests may have left
        // sources in it: select *this* one rather than assuming it is alone.
        let index = app
            .pool
            .iter()
            .position(|source| source.id == stem)
            .expect("the import listed the source");
        assert_eq!(app.pool[index].channels, 1);

        // Reach the pool and place the source at the playhead on the active track.
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(app.focus, Panel::Pool);
        app.pool_selected = index;
        app.snap.frame = 96_000;
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

        let arrangement = app.arrangement.as_ref().expect("an arrangement");
        let placed = arrangement.lanes[0]
            .clips
            .iter()
            .find(|clip| clip.at_frame == 96_000)
            .expect("the source landed at the playhead");
        assert_eq!(placed.id, "pool.1");
        assert_eq!(placed.source_id, stem);
        assert_eq!(
            placed.fade_in, MICRO_FADE,
            "new boundaries get the click guard"
        );
        assert!(app.status.contains("placed"), "{}", app.status);

        // One undo removes it, and the panel's row survives (it is pool material).
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        assert!(
            !app.arrangement.as_ref().expect("arrangement").lanes[0]
                .clips
                .iter()
                .any(|clip| clip.id == "pool.1"),
            "{}",
            app.status
        );
        assert!(app.pool.iter().any(|source| source.id == stem));

        // The panel draws the source it lists.
        let screen = rendered(&mut app);
        assert!(screen.contains("pool"), "no pool panel:\n{screen}");
        assert!(screen.contains(&stem), "the source is listed:\n{screen}");

        let _ = std::fs::remove_file(&path);
        drop_pool();
    }

    /// Placing into a session with **no tracks** makes the track first: the pool is
    /// usable before there is an arrangement, which is the order a piece is built in.
    #[test]
    fn placing_from_the_pool_creates_the_first_track() {
        // A pool-only session: a pool with one source and no arrangement at all.
        let dir = std::env::temp_dir().join(format!("tui-shell-poolonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pool dir");
        let source = dir.join("jam.wav");
        let mut writer =
            media::wav::WavWriter::create_float(&source, 48_000, 1).expect("fixture source");
        writer.write(&vec![0.5f32; 4_800]).expect("write");
        writer.finalize().expect("finalize");
        let script_path = dir.join("pool-only.script");
        std::fs::write(
            &script_path,
            format!(
                "host v1\nmount mixer channels=2 @0\npool {}\n",
                dir.display()
            ),
        )
        .expect("write the script");

        let mut app = App::idle();
        app.open_script(&script_path);
        let arrangement = app.arrangement.as_ref().expect("an empty arrangement");
        assert!(arrangement.lanes.is_empty(), "no tracks yet");
        assert_eq!(app.pool.len(), 1, "{}", app.status);

        app.focus = Panel::Pool;
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        let arrangement = app.arrangement.as_ref().expect("an arrangement now exists");
        assert_eq!(
            arrangement.lanes.len(),
            1,
            "a track was made: {}",
            app.status
        );
        assert_eq!(arrangement.lanes[0].clips.len(), 1);
        assert_eq!(arrangement.lanes[0].clips[0].id, "pool.1");
        assert_eq!(arrangement.lanes[0].clips[0].source_id, "jam");
        assert!(app.status.contains("new track"), "{}", app.status);
        assert_eq!(app.focus, Panel::Timeline, "and the keys follow it");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A minted id never repeats**, even after an undo: the id is out of the
    /// arrangement but still in the journal, and re-minting it would make journal
    /// recovery drop the second `add_clip` (the clip silently vanishing on a crash).
    /// A source the text format cannot name is refused *by name* instead of producing
    /// a parse error, and a source with no frames is refused too.
    #[test]
    fn pool_placement_mints_forward_and_refuses_unusable_rows() {
        // A pool with a normal source, one whose name has a space, and one with no
        // frames at all.
        let dir = std::env::temp_dir().join(format!("tui-shell-poolrows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pool dir");
        for (name, frames) in [
            ("jam.wav", 4_800usize),
            ("my jam.wav", 4_800),
            ("empty.wav", 0),
        ] {
            let mut w = media::wav::WavWriter::create_float(&dir.join(name), 48_000, 1)
                .expect("fixture source");
            w.write(&vec![0.5f32; frames]).expect("write");
            w.finalize().expect("finalize");
        }
        let script_path = dir.join("pool.script");
        std::fs::write(
            &script_path,
            format!(
                "host v1\nmount mixer channels=2 @0\npool {}\n",
                dir.display()
            ),
        )
        .expect("script");

        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        assert_eq!(app.pool.len(), 3, "{}", app.status);
        app.focus = Panel::Pool;
        let index = |app: &App, id: &str| {
            app.pool
                .iter()
                .position(|source| source.id == id)
                .expect("a row")
        };

        // A normal source places, and an undo does not let the next one reuse the id.
        app.pool_selected = index(&app, "jam");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(app.status.contains("placed jam"), "{}", app.status);
        let first = app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0]
            .id
            .clone();
        assert_eq!(first, "pool.1");

        // The first placement made a track; undo it (both the id and the clip leave
        // the arrangement) and place again.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        app.focus = Panel::Pool;
        app.pool_selected = index(&app, "jam");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        let ids: Vec<String> = app
            .arrangement
            .as_ref()
            .expect("arrangement")
            .lanes
            .iter()
            .flat_map(|lane| lane.clips.iter().map(|clip| clip.id.clone()))
            .collect();
        assert!(
            ids.contains(&"pool.2".to_string()),
            "the minted id moved forward past the journal's: {ids:?} ({})",
            app.status
        );
        assert!(!ids.contains(&"pool.1".to_string()), "and never repeated");

        // A row the `host v1` format cannot name is refused with the reason. (A
        // placement that made a track moved the focus to the timeline, so come back.)
        app.focus = Panel::Pool;
        app.pool_selected = index(&app, "my jam");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            app.status.contains("cannot be named"),
            "the unnameable row explains itself: {}",
            app.status
        );
        let screen = rendered(&mut app);
        assert!(
            screen.contains("unplaceable"),
            "and the panel marks it:\n{screen}"
        );

        // A source with no frames is refused too (it could never be a clip).
        app.focus = Panel::Pool;
        app.pool_selected = index(&app, "empty");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(app.status.contains("no audio frames"), "{}", app.status);

        // A focus whose panel vanished falls back rather than leaving the keys dead.
        app.focus = Panel::Pool;
        app.pool.clear();
        assert_ne!(app.focus_that_exists(), Panel::Pool);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The utility gestures are one log line each, over vocabulary that already
    /// exists** (except `reverse`, which is one new clip property): `V` flips the
    /// read direction, `i` the polarity, `E` silences, `U` normalizes from the peak
    /// pyramid, and `T` trims to the audible content.
    #[test]
    fn the_utility_gestures_are_one_line_each() {
        let _pool = pool_guard();
        let dir = std::env::temp_dir().join(format!("tui-shell-utility-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pool dir");
        // 1 000 frames of silence, 2 000 of a 0.25 tone, 3 000 of silence — an
        // asymmetric shape, so a mirrored trim is distinguishable from a forward one.
        let path = dir.join("shape.wav");
        let mut w = media::wav::WavWriter::create_float(&path, 48_000, 1).expect("fixture");
        let samples: Vec<f32> = (0..6_000)
            .map(|i| {
                if (1_000..3_000).contains(&i) {
                    0.25 * (i as f32 * 0.05).sin()
                } else {
                    0.0
                }
            })
            .collect();
        w.write(&samples).expect("write");
        w.finalize().expect("finalize");

        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_wave(&path);
        let clip = |app: &App| -> Placed {
            app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0].clone()
        };
        // The gestures act on the clip **under the playhead**, so park it inside the
        // clip before each one (a trim moves the clip, and the demo snapshot's
        // playhead is 2.000 s — past this 0.125 s fixture).
        let press = |app: &mut App, c: char| {
            app.snap.frame = app
                .arrangement
                .as_ref()
                .and_then(|arrangement| arrangement.lanes[0].clips.first())
                .map(|clip| clip.at_frame + 1)
                .unwrap_or(0);
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // `V` reverses (and says which way), `V` again restores.
        press(&mut app, 'V');
        assert!(clip(&app).reversed, "{}", app.status);
        assert!(app.status.contains("backwards"), "{}", app.status);
        press(&mut app, 'V');
        assert!(!clip(&app).reversed, "{}", app.status);
        assert!(app.status.contains("forwards"), "{}", app.status);

        // `i` inverts the polarity, `i` again restores it.
        press(&mut app, 'i');
        assert!((clip(&app).gain + 1.0).abs() < 1e-6, "{}", app.status);
        press(&mut app, 'i');
        assert!((clip(&app).gain - 1.0).abs() < 1e-6, "{}", app.status);

        // `E` silences (a gain of zero — the span and the material are untouched).
        press(&mut app, 'E');
        assert_eq!(clip(&app).gain, 0.0, "{}", app.status);
        assert!(clip(&app).src_len > 0, "silence is a gain, not a delete");

        // `U` normalizes from the **source's** peak (0.25 → a gain of 4), which also
        // un-silences the clip: silence was a gain, not a change to the material.
        press(&mut app, 'U');
        let gain = clip(&app).gain;
        assert!(
            (gain - 4.0).abs() < 0.05,
            "normalize to the 0.25 peak: gain {gain} ({})",
            app.status
        );
        assert!(app.status.contains("normalized"), "{}", app.status);

        // `T` trims to the audible content: ~1 000 in and ~3 000 out (the scan works
        // in whole peak bins, so it lands within one bin of the edge).
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        press(&mut app, 'T');
        let c = clip(&app);
        assert!(
            (c.src_start as i64 - 1_000).abs() <= media::PEAK_BASE_BIN as i64,
            "the head silence went: src_start {} ({})",
            c.src_start,
            app.status
        );
        assert!(
            (c.src_len as i64 - 2_000).abs() <= 2 * media::PEAK_BASE_BIN as i64,
            "and the tail: src_len {} ({})",
            c.src_len,
            app.status
        );
        assert!(c.at_frame > 0, "the clip moved to where the audio is");

        // The **mirror**: on a reversed clip the clip's start is the region's top, so
        // the same scan trims the *other* edge first — the two removals swap exactly.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        let before = clip(&app);
        let forward = (before.at_frame, before.end_frame());
        press(&mut app, 'T');
        let trimmed = clip(&app);
        let lead = trimmed.at_frame - forward.0;
        let trail = forward.1 - trimmed.end_frame();
        assert!(
            lead > 0 && trail > 0,
            "both edges had silence: {lead} / {trail}"
        );

        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        let before = clip(&app);
        let forward = (before.at_frame, before.end_frame());
        press(&mut app, 'V');
        press(&mut app, 'T');
        let trimmed = clip(&app);
        assert!(trimmed.reversed, "the clip stayed reversed");
        assert_eq!(
            (
                trimmed.at_frame - forward.0,
                forward.1 - trimmed.end_frame()
            ),
            (trail, lead),
            "a reversed clip trims the other edge first ({} frames became {} then {})",
            lead + trail,
            lead,
            trail
        );

        // A clip whose fades fill it (reachable with `f` at the end, and what paste
        // preserves) is trimmed to content with its fades **capped to the new
        // length**, in the same gesture — refusing instead would make `T` unusable.
        app.open_wave(&path);
        {
            let c = clip(&app);
            app.snap.frame = c.at_frame + 1;
        }
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in format!("arrange set_clip_fade t0 c0 {} 0", clip(&app).src_len).chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(
            clip(&app).fade_in,
            6_000,
            "a full-length fade: {}",
            app.status
        );
        app.snap.frame = clip(&app).at_frame + 1;
        app.on_key(KeyEvent::new(KeyCode::Char('T'), KeyModifiers::empty()));
        let capped = clip(&app);
        assert!(
            capped.fade_in + capped.fade_out <= capped.src_len,
            "the fades fit the trimmed length: {}/{} of {} ({})",
            capped.fade_in,
            capped.fade_out,
            capped.src_len,
            app.status
        );
        assert!(
            app.status.contains("fades capped"),
            "and the cap is reported: {}",
            app.status
        );

        // A clip whose **source** is silent has nothing to normalize to: refused
        // rather than amplified into noise.
        let silent = dir.join("silent.wav");
        let mut w = media::wav::WavWriter::create_float(&silent, 48_000, 1).expect("fixture");
        w.write(&vec![0.0f32; 4_800]).expect("write");
        w.finalize().expect("finalize");
        app.open_wave(&silent);
        press(&mut app, 'U');
        assert!(
            app.status.contains("silent"),
            "a silent source is refused: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&dir);
        drop_pool();
    }

    /// **The peak scans use the true peak**, both halves: a signal that swings mostly
    /// negative (or a polarity-inverted clip) has its loudest excursion in `min`, so
    /// reading only `max` would normalize a −0.9 peak as if it were silent and would
    /// let trim-to-content delete a bin that is loud but negative. (The gate found the
    /// first draft doing exactly that — the panel's own envelope draws both halves.)
    #[test]
    fn the_peak_scans_use_both_halves() {
        let _pool = pool_guard();
        let dir = std::env::temp_dir().join(format!("tui-shell-negative-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pool dir");
        // Silence, then a *constant negative* 0.9 region, then silence.
        let path = dir.join("negative.wav");
        let mut w = media::wav::WavWriter::create_float(&path, 48_000, 1).expect("fixture");
        let samples: Vec<f32> = (0..6_000)
            .map(|i| {
                if (1_000..3_000).contains(&i) {
                    -0.9
                } else {
                    0.0
                }
            })
            .collect();
        w.write(&samples).expect("write");
        w.finalize().expect("finalize");

        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_wave(&path);
        let at_clip = |app: &mut App| {
            app.snap.frame = app
                .arrangement
                .as_ref()
                .and_then(|arrangement| arrangement.lanes[0].clips.first())
                .map(|clip| clip.at_frame + 1)
                .unwrap_or(0);
        };
        let clip = |app: &App| -> Placed {
            app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0].clone()
        };

        // Normalize: peak 0.9 → gain 1/0.9 (the first draft saw max = 0 and refused
        // the clip as silent).
        at_clip(&mut app);
        app.on_key(KeyEvent::new(KeyCode::Char('U'), KeyModifiers::empty()));
        let gain = clip(&app).gain;
        assert!(
            (gain - 1.0 / 0.9).abs() < 0.02,
            "normalize from the negative peak: gain {gain} ({})",
            app.status
        );
        assert!(app.status.contains("peak 0.9000"), "{}", app.status);

        // Trim-to-content: the negative region is content, not silence.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        at_clip(&mut app);
        let before = clip(&app);
        app.on_key(KeyEvent::new(KeyCode::Char('T'), KeyModifiers::empty()));
        let after = clip(&app);
        assert!(
            after.src_len < before.src_len,
            "the silent edges went: {} ({})",
            after.src_len,
            app.status
        );
        assert!(
            after.src_len as i64 > 1_000,
            "but the negative region survived: src_len {}",
            after.src_len
        );

        let _ = std::fs::remove_dir_all(&dir);
        drop_pool();
    }

    /// A paste carries a **reversed** clip's direction: `add_clip` has no direction
    /// operand (a clip is added forward and `reverse` flips it), so the paste emits
    /// its own `reverse` in the same gesture — otherwise a paste would silently play
    /// it forwards (the gate caught that).
    #[test]
    fn a_paste_keeps_a_reversed_clip_reversed() {
        let (pool, script_path) = pool_script("pasteflip", "arrange add_track t1\n");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };

        // Reverse the clip under the playhead, then copy and paste it.
        press(&mut app, 'V');
        assert!(
            app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0].reversed,
            "{}",
            app.status
        );
        press(&mut app, 'y');
        app.active_track = 1;
        press(&mut app, 'p');
        let pasted = app.arrangement.as_ref().expect("arrangement").lanes[1].clips[0].clone();
        assert!(
            pasted.reversed,
            "the paste plays it backwards too: {}",
            app.status
        );

        // One undo removes the whole paste (the `add_clip` and its `reverse`).
        press(&mut app, 'u');
        assert!(
            app.arrangement.as_ref().expect("arrangement").lanes[1]
                .clips
                .is_empty(),
            "{}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **Warp is tempo match.** With a source tempo recorded (`: source_tempo`), `W`
    /// renders the clip's material through the host's stretch at `source/session` and
    /// points the clip at it — one command, one undoable op, and the status says what
    /// it did. Without a tempo it says how to record one instead of guessing.
    #[test]
    fn warp_stretches_a_clip_to_the_session_tempo() {
        let (pool, script_path) = pool_script("warp", "source_tempo s1 90\n");
        let mut app = App::idle();
        app.snap = App::demo_snapshot(); // 120 bpm, the playhead at 96 000
        app.open_script(&script_path);
        assert_eq!(app.source_tempos.get("s1"), Some(&90.0), "{}", app.status);

        let before = app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0].clone();
        app.on_key(KeyEvent::new(KeyCode::Char('W'), KeyModifiers::empty()));
        assert!(app.status.contains("warped"), "{}", app.status);
        assert!(app.status.contains("90.0 → 120.0"), "{}", app.status);

        let after = app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0].clone();
        assert_eq!(
            after.source_id, "s1.stretch.0_144000.3_4",
            "the material was rendered"
        );
        assert!(
            after.src_len < before.src_len,
            "a 90 bpm take is shortened into 120 bpm: {} → {}",
            before.src_len,
            after.src_len
        );
        let expect = before.src_len as f64 * 0.75;
        assert!(
            (after.src_len as f64 - expect).abs() < 2_048.0,
            "the length follows the ratio: {} vs {expect:.0}",
            after.src_len
        );
        // The panel resolves the new source (it is in the outcome's listing).
        assert!(
            app.sources.errors().is_empty(),
            "the rendered source must resolve in the panel"
        );

        // One undo restores the original reference.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()));
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0].source_id,
            "s1"
        );

        let _ = std::fs::remove_dir_all(&pool);

        // Without a tempo, the shell explains the missing fact — and the fix it names
        // is the fix: typing the line at the command line updates the cache (the
        // outcome carries the state), and `W` then warps.
        let (pool, script_path) = pool_script("warpnotempo", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        app.on_key(KeyEvent::new(KeyCode::Char('W'), KeyModifiers::empty()));
        assert!(
            app.status.contains("no tempo recorded") && app.status.contains("source_tempo"),
            "the fix is named: {}",
            app.status
        );
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in "source_tempo s1 90".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(
            app.source_tempos.get("s1"),
            Some(&90.0),
            "the recorded tempo reaches the shell: {}",
            app.status
        );
        app.on_key(KeyEvent::new(KeyCode::Char('W'), KeyModifiers::empty()));
        assert!(
            app.status.contains("warped"),
            "and the gesture it was told to make now works: {}",
            app.status
        );
        let _ = std::fs::remove_dir_all(&pool);
    }

    /// `X` pre-fills a **complete** export line (path and format), so Enter runs it as
    /// it stands and writes the file the status then reports; the `R` pre-fill is the
    /// other shape (a value still to come), and the command line tells them apart by
    /// the trailing space.
    #[test]
    fn the_export_gesture_prefills_a_runnable_line_and_reports() {
        let (pool, script_path) = pool_script("exportgesture", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);

        app.on_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::empty()));
        let prefill = app.prompt.clone().expect("the command line opens");
        assert!(prefill.starts_with("export "), "{prefill}");
        assert!(prefill.ends_with(" f32"), "{prefill}");

        // Enter with the pre-filled line unchanged writes the default file (beside the
        // script the session was opened from) and reports what it wrote.
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            app.status.contains("exported") && app.status.contains("peak"),
            "the report: {}",
            app.status
        );
        let default_path = std::path::Path::new(&script_path)
            .parent()
            .expect("a script dir")
            .join("mix.wav");
        assert!(
            default_path.is_file(),
            "the default export is beside the script ({})",
            default_path.display()
        );

        // The `s16` spelling asks for the dithered 16-bit file instead.
        app.prompt = Some(format!("export {} s16", pool.join("mix16.wav").display()));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            pool.join("mix16.wav").is_file(),
            "and a 16-bit export is one word away"
        );
        assert!(app.status.contains("s16"), "{}", app.status);

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **Markers are a keyboard vocabulary**: `'` names one at the playhead through a
    /// prefilled prompt, `;` / `"` walk them (landing *exactly* on the marker, not on a
    /// snapped frame), the ruler draws the glyph and the name, and `C` names the clip
    /// under the playhead. All of it goes through `arrange` lines, so every gesture is
    /// one log line and one undo.
    #[test]
    fn markers_and_clip_names_are_keyboard_gestures() {
        let (pool, script_path) = pool_script("markers", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        app.grid = workflow::Grid::new(media::Division::Beat); // so a snap would move a frame

        let press = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
        };
        let type_line = |app: &mut App, line: &str| {
            for c in line.chars() {
                app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
            }
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        };

        // `'` prefills the marker line with the playhead's frame and no name yet.
        press(&mut app, '\'');
        let prefill = app.prompt.clone().expect("the command line opens");
        assert!(prefill.starts_with("arrange set_marker "), "{prefill}");
        assert!(
            prefill.ends_with(' '),
            "the name is the missing word: {prefill}"
        );
        // Answering the prefill unchanged gets the hint (the trailing-space rule).
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(app.status.contains("needs the new value"), "{}", app.status);

        // Name two markers (typed through the same prompt) at frames the playhead is at.
        // Both markers mid-bar, so the ruler's name has cells to occupy (a bar number
        // always wins the cells it numbers — the drawing rule).
        let playhead = app.snap.frame;
        let intro = playhead + 12_000;
        press(&mut app, ':');
        type_line(&mut app, &format!("arrange set_marker {intro} intro"));
        assert!(
            app.arrangement
                .as_ref()
                .expect("arrangement")
                .marker_at(intro)
                .is_some(),
            "the marker landed: {}",
            app.status
        );
        let later = playhead + 36_000;
        press(&mut app, ':');
        type_line(&mut app, &format!("arrange set_marker {later} chorus"));
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").markers.len(),
            2
        );

        // Jump: land exactly on the marker, and report its name.
        app.snap.frame = playhead;
        press(&mut app, ';');
        assert_eq!(app.snap.frame, intro, "`;` lands on the marker exactly");
        assert!(app.status.contains("intro"), "{}", app.status);
        press(&mut app, ';');
        assert_eq!(app.snap.frame, later, "and again for the next");
        press(&mut app, '"');
        assert_eq!(app.snap.frame, intro, "the previous-marker key goes back");
        assert!(app.status.contains("intro"), "{}", app.status);

        // The ruler draws the glyph and the name at the marker's cell.
        let markers = app
            .arrangement
            .as_ref()
            .expect("arrangement")
            .markers
            .clone();
        let row = timeline::ruler_row_for_test(&markers, playhead);
        assert!(
            row.contains('▼') && row.contains("intro"),
            "the ruler shows the marker: {row:?}"
        );

        // `C` names the clip under the playhead (the demo clip starts at frame 0).
        app.snap.frame = 0;
        press(&mut app, 'C');
        let prefill = app.prompt.clone().expect("the command line opens");
        assert!(prefill.starts_with("arrange rename_clip t0 "), "{prefill}");
        // The prompt is already open with the prefill: type the missing word and Enter.
        for c in "take-1".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").lanes[0].clips[0]
                .name
                .as_deref(),
            Some("take-1"),
            "the clip carries the label: {}",
            app.status
        );

        // Removing a marker is one op, and it is refused when there is nothing there.
        press(&mut app, ':');
        type_line(&mut app, &format!("arrange remove_marker {later}"));
        assert_eq!(
            app.arrangement.as_ref().expect("arrangement").markers.len(),
            1
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// The stretch ratio is a **reduced rational**, and a degenerate tempo yields none
    /// (never a division by zero or a nonsense render).
    #[test]
    fn the_tempo_ratio_is_a_reduced_rational() {
        assert_eq!(tempo_ratio(90.0, 120.0), Some((3, 4)));
        assert_eq!(tempo_ratio(120.0, 120.0), Some((1, 1)));
        assert_eq!(tempo_ratio(100.0, 120.0), Some((5, 6)));
        assert_eq!(tempo_ratio(128.0, 120.0), Some((16, 15)));
        assert_eq!(tempo_ratio(90.5, 120.0), Some((181, 240)));
        assert_eq!(tempo_ratio(0.0, 120.0), None);
        assert_eq!(tempo_ratio(120.0, 0.0), None);
        assert_eq!(tempo_ratio(f64::NAN, 120.0), None);
        assert_eq!(tempo_ratio(f64::INFINITY, 120.0), None);
    }

    /// The `:` command line types the same `host v1` format the keys dispatch, so
    /// the whole vocabulary is reachable without a key: Enter runs it, Esc cancels,
    /// a parse error is shown and changes nothing, and `↑` walks the history.
    #[test]
    fn the_command_line_runs_host_lines() {
        let (pool, script_path) = pool_script("prompt", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);

        fn type_line(app: &mut App, line: &str) {
            app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
            assert!(app.prompt.is_some(), "the command line opens");
            for c in line.chars() {
                app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
            }
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        }

        // A line the keys have no key for: set the clip's gain.
        type_line(&mut app, "arrange set_clip_gain t0 c0 0.25");
        assert!(app.prompt.is_none(), "Enter closes the prompt");
        let gain = app.arrangement.as_ref().expect("an arrangement").lanes[0].clips[0].gain;
        assert!((gain - 0.25).abs() < 1e-6, "gain {gain} ({})", app.status);
        assert!(app.status.starts_with(": arrange"), "{}", app.status);
        assert_eq!(app.last_command.map(|(label, _)| label), Some("prompt"));

        // Every script line works, not just `arrange`: a parameter, and the
        // transport, through the same parser.
        type_line(&mut app, "set_param mixer ch0.gain 0.5");
        assert!(
            (app.mixer.value(0) - 0.5).abs() < 1e-6,
            "the console reads the log back: {}",
            app.status
        );
        type_line(&mut app, "transport seek 0");
        assert_eq!(
            app.host.snapshot().frame,
            0,
            "the host seeks (the shell's own copy of the snapshot is the event loop's job)"
        );

        // A bad line reports and changes nothing.
        let before = app.arrangement.as_ref().expect("an arrangement").lanes[0].clips[0].gain;
        type_line(&mut app, "arrange nonsense");
        assert!(
            app.status.contains(':'),
            "the error is shown: {}",
            app.status
        );
        assert_eq!(
            app.arrangement.as_ref().expect("an arrangement").lanes[0].clips[0].gain,
            before,
            "a refused line is not logged"
        );

        // Esc cancels without running, and backspace edits.
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()));
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert_eq!(app.prompt.as_deref(), Some("x"), "backspace edits the line");
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(app.prompt.is_none(), "Esc closes the prompt");
        assert!(app.status.contains("cancelled"), "{}", app.status);
        assert_eq!(
            app.arrangement.as_ref().expect("an arrangement").lanes[0]
                .clips
                .len(),
            1,
            "a cancelled line runs nothing"
        );

        // The mode is on screen while the line is open — a modal UI that hides its
        // mode is a trap — and the line replaces the status where a `:` belongs.
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        let screen = rendered(&mut app);
        assert!(
            screen.contains("COMMAND"),
            "the command line's mode is shown:\n{screen}"
        );
        assert!(screen.contains(":x"), "the typed line is shown:\n{screen}");
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));

        // `↑` recalls the last line; `↓` returns to the live (empty) line.
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
        assert_eq!(
            app.prompt.as_deref(),
            Some("arrange nonsense"),
            "history recalls the last line attempted — including one worth fixing"
        );
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
        assert_eq!(
            app.prompt.as_deref(),
            Some(""),
            "down returns to the live line"
        );
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// The one place drift can now enter: the shell's translation of a toolkit event
    /// into the workflow's neutral key. (Which key means *what* is the workflow's
    /// business; that crossterm's `BackTab` is the workflow's `BackTab` is ours.)
    #[test]
    fn toolkit_keys_translate_into_the_workflow() {
        let press = |code: KeyCode, modifiers: KeyModifiers| {
            workflow::action(App::workflow_key(&KeyEvent::new(code, modifiers)).expect("a key"))
        };

        assert_eq!(
            press(KeyCode::Char(' '), KeyModifiers::empty()),
            Some(Action::PlayToggle)
        );
        assert_eq!(
            press(KeyCode::Char('j'), KeyModifiers::empty()),
            Some(Action::Vertical(1))
        );
        assert_eq!(
            press(KeyCode::Char('k'), KeyModifiers::empty()),
            Some(Action::Vertical(-1))
        );
        assert_eq!(
            press(KeyCode::Char('K'), KeyModifiers::SHIFT),
            Some(Action::MoveTrack(-1))
        );
        assert_eq!(
            press(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(Action::CycleFocus(-1))
        );
        assert_eq!(
            press(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Some(Action::Redo)
        );
        assert_eq!(
            press(KeyCode::Char(':'), KeyModifiers::empty()),
            Some(Action::Prompt)
        );
        assert_eq!(
            press(KeyCode::Esc, KeyModifiers::empty()),
            Some(Action::Cancel)
        );
        // A key the workflow does not use is not an action (and not a panic).
        assert_eq!(
            App::workflow_key(&KeyEvent::new(KeyCode::F(5), KeyModifiers::empty())),
            None
        );
    }

    /// The command line is how a session is saved and opened in this shell today:
    /// `: save <dir>` writes the session directory (the log plus its pool) and
    /// `: load <dir>` reads it back — the whole round trip through the same parser
    /// the keys use, with no shell-side session format.
    #[test]
    fn the_command_line_saves_and_loads_a_session() {
        let (pool, script_path) = pool_script("session", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);
        assert_eq!(
            app.arrangement.as_ref().expect("an arrangement").frames,
            144_000,
            "{}",
            app.status
        );

        // A first edit, then save: the directory holds the script and the pool.
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        let dir = std::env::temp_dir().join(format!("tui-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let line = format!("save {}", dir.display());
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

        assert!(dir.join("session.txt").is_file(), "saved: {}", app.status);
        assert!(
            dir.join("pool/s1.wav").is_file(),
            "the pool travelled with it"
        );
        assert!(dir.join("journal.txt").exists(), "the journal exists");

        // A later edit is autosaved into the journal …
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty()));
        assert_eq!(
            app.arrangement
                .as_ref()
                .expect("an arrangement")
                .clip_count(),
            1,
            "{}",
            app.status
        );
        let journal = std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
        assert!(journal.contains("arrange delete"), "autosaved: {journal}");

        // … and `: load` brings the session back (both edits: the split, then the
        // delete left one clip).
        let line = format!("load {}", dir.display());
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(
            app.arrangement
                .as_ref()
                .expect("an arrangement")
                .clip_count(),
            1,
            "the journal's edits came back: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `load` says what the journal's recovery did: the counts beside the line
    /// that triggered them. A torn autosave line (a crash mid-write) is reported,
    /// the intact edits are counted as applied, and a clean load stays quiet —
    /// the report is the crash story, not a ritual.
    #[test]
    fn a_load_reports_the_journals_recovery() {
        let (pool, script_path) = pool_script("torn-load", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);

        let dir = std::env::temp_dir().join(format!("tui-torn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let line = format!("save {}", dir.display());
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(dir.join("session.txt").is_file(), "saved: {}", app.status);

        // A clean load has no story: the journal is empty, so the line stands alone.
        let line = format!("load {}", dir.display());
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(
            app.status,
            format!(": {line}"),
            "nothing to report: {}",
            app.status
        );

        // A crashed journal: one intact edit, one torn final line (no newline).
        std::fs::write(
            dir.join("journal.txt"),
            "arrange trim t0 c0 start 1200\narrange trim t0 c0 sta",
        )
        .expect("torn journal");

        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            app.status.contains("journal: 1 applied, 1 torn, 0 refused"),
            "the crash story is on the status line: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Recording is reachable from the command line like everything else, and its
    /// refusals are the host's own words (here: no pool, so the take has nowhere to
    /// land — no device is touched, so the test runs anywhere).
    #[test]
    fn the_command_line_records_and_reports_refusals() {
        let (pool, script_path) = pool_script("record", "");
        let mut app = App::idle();
        app.snap = App::demo_snapshot();
        app.open_script(&script_path);

        // `record stop` with nothing recording is refused by the host — and touches no
        // device, so this test runs anywhere.
        let line = "record stop".to_string();
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            app.status.starts_with(": record stop") && app.status.contains("no take"),
            "the refusal is reported in the status line: {}",
            app.status
        );

        // An extra word is a typo, not a take id (`record bad id!` must not start a
        // take called "bad").
        let line = "record bad id!".to_string();
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            app.status.contains("source=<name>"),
            "a stray word is a parse error that names the modifier: {}",
            app.status
        );

        // …and the command line still works afterwards.
        let line = "arrange set_clip_gain t0 c0 0.4".to_string();
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        for c in line.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(
            app.arrangement.as_ref().expect("an arrangement").lanes[0].clips[0].gain,
            0.4,
            "{}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&pool);
    }
}
