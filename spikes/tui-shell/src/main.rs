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
//!    Tauri bridge and the iced spike use.
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
use timeline::{Arrangement, Mode, Sources, View};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Clear, Gauge, Paragraph, Wrap};
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

/// The keymap — rendered verbatim by the `?` overlay, so the help and the
/// behaviour cannot drift apart.
const KEYMAP: &[(&str, &str)] = &[
    ("space", "play / stop"),
    ("s", "stop"),
    ("r  Home", "rewind to 0"),
    (",  .", "seek -1 s / +1 s (stops first)"),
    ("Tab  Shift-Tab", "move between panels (mixer ⇄ timeline)"),
    (
        "j  k  ↑  ↓",
        "the focused panel: mixer channel / active track",
    ),
    ("v", "visual mode: select from the playhead"),
    (
        "h  l",
        "timeline: scroll (visual mode: extend the selection)",
    ),
    (
        "+  -",
        "timeline: zoom in / out · mixer: ride the selected fader",
    ),
    ("0", "timeline: fit · mixer: fader to unity"),
    ("M  S", "mixer: mute / solo the selected channel"),
    ("x", "timeline: split the clip under the playhead"),
    ("d", "timeline: delete the clip under the playhead"),
    ("n  N", "timeline: jump to the next / previous clip"),
    (
        "u  Ctrl+r",
        "undo / redo the last arrangement edit (a log replay)",
    ),
    (
        "Esc",
        "leave visual mode / close this overlay (never quits)",
    ),
    ("m", "toggle mouse capture"),
    ("?", "this keymap"),
    ("q  Ctrl+c", "quit"),
    (
        "mouse",
        "click a panel to focus · click a lane = that track + seek · click the ruler = seek · wheel = seek",
    ),
];

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
    mouse: bool,
    quit: bool,
    /// The last command's label and how long the UI thread was blocked in it.
    last_command: Option<(&'static str, Duration)>,
    /// Rects recorded by the last draw, for mouse hit-testing: the terminal has
    /// no widget tree to query, so the view publishes what is clickable.
    buttons: Vec<(Rect, Action)>,
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
    view: Option<View>,
    mode: Mode,
    focus: Panel,
    active_track: usize,
    /// Where the timeline came from, for the panel title.
    origin: String,
    /// A counter for ids the shell generates (split halves) — ids are logged, so
    /// they must be unique and deterministic per session.
    next_id: u64,
    panel: timeline::PanelRects,
}

/// Which panel the keys act on. `Tab`/`Shift-Tab` cycle; a click focuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Mixer,
    Timeline,
}

impl Panel {
    fn label(self) -> &'static str {
        match self {
            Panel::Mixer => "mixer",
            Panel::Timeline => "timeline",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Play,
    Stop,
    Rewind,
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
            view: None,
            mode: Mode::Normal,
            focus: Panel::Mixer,
            active_track: 0,
            origin: String::new(),
            next_id: 0,
            panel: timeline::PanelRects::default(),
        };

        if let Some(path) = script {
            app.open_script(&path);
        } else if let Some(path) = wave {
            app.open_wave(&path);
        }

        app
    }

    /// Load a single audio file as a **one-clip arrangement in the host**
    /// (`--wave`): the shell synthesises the same kind of script a user would
    /// write and hands it over, so the file is *audible* and the arrangement the
    /// panel draws is the engine's own value — not a shell-side picture of a file
    /// the host has never seen.
    fn open_wave(&mut self, path: &std::path::Path) {
        let script = match wave_script(path) {
            Ok(script) => script,
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
                        "loaded {} — {} clip(s), {} log events (press space to hear it)",
                        path.display(),
                        clips,
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
        App {
            snap: host.snapshot(),
            host,
            status: String::new(),
            selected: 0,
            help: false,
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
            view: None,
            mode: Mode::Normal,
            focus: Panel::Mixer,
            active_track: 0,
            origin: String::new(),
            next_id: 0,
            panel: timeline::PanelRects::default(),
        }
    }

    // -- update ------------------------------------------------------------

    /// Run a host command and record how long it blocked the UI thread. A shell
    /// that binds a command to a key owns this number: `TransportSeek` is
    /// O(target) in the host, so it is the one that will be felt.
    fn command(&mut self, label: &'static str, command: HostCommand) {
        let started = Instant::now();
        let outcome = self.host.execute(command);
        let elapsed = started.elapsed();

        match outcome {
            Ok(()) => self.last_command = Some((label, elapsed)),
            Err(e) => self.status = format!("{label} refused: {e}"),
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

    /// Seek by whole seconds, clamped at zero. Stops first (see `rewind`).
    fn nudge(&mut self, seconds: i64) {
        let target = (self.snap.frame as i64 + seconds * SAMPLE_RATE as i64).max(0) as u64;

        self.stop();
        self.command("seek", HostCommand::TransportSeek { frame: target });
    }

    fn select(&mut self, delta: i64) {
        // Strips include the master, so the console's last strip is reachable.
        let count = self.mixer.strips() as i64;
        if count == 0 {
            return;
        }

        self.selected = (self.selected as i64 + delta).rem_euclid(count) as usize;
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Play => self.play(),
            Action::Stop => self.stop(),
            Action::Rewind => self.rewind(),
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            // Global transport: available in either panel and in any mode.
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('s') => self.stop(),
            KeyCode::Char('r') if ctrl => self.redo(),
            KeyCode::Char('r') | KeyCode::Home => self.rewind(),
            KeyCode::Char(',') => self.nudge(-1),
            KeyCode::Char('.') => self.nudge(1),
            // Undo/redo are *log replays*: the host drops the last arrangement
            // edit and rebuilds by replaying the log, and the shell re-reads the
            // arrangement **and** the parameters — which is why a fader ride
            // survives undoing a clip edit.
            KeyCode::Char('u') => self.undo(),

            // Panel navigation. Shift-Tab arrives as BackTab on most terminals.
            KeyCode::Tab => self.cycle_focus(1),
            KeyCode::BackTab => self.cycle_focus(-1),

            // Panel-scoped vertical movement: the mixer's channel, or the
            // timeline's active track.
            KeyCode::Char('j') | KeyCode::Down => self.vertical(1),
            KeyCode::Char('k') | KeyCode::Up => self.vertical(-1),

            // Timeline only (the viewport is free to move; only seeking and
            // editing talk to the host).
            KeyCode::Char('h') | KeyCode::Left => self.timeline_key(|app| app.timeline_move(-1)),
            KeyCode::Char('l') | KeyCode::Right => self.timeline_key(|app| app.timeline_move(1)),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('Z') => self.plus(),
            KeyCode::Char('-') | KeyCode::Char('z') => self.minus(),
            KeyCode::Char('0') => self.zero(),
            KeyCode::Char('M') => self.mixer_key(|app| app.mute()),
            KeyCode::Char('S') => self.mixer_key(|app| app.solo()),
            KeyCode::Char('v') => self.timeline_key(|app| app.visual()),
            KeyCode::Char('x') => self.timeline_key(|app| app.split_at_playhead()),
            KeyCode::Char('d') => self.timeline_key(|app| app.delete_at_playhead()),
            // Clip-to-clip motion: the "next word" analogue for an arrangement.
            KeyCode::Char('n') => self.timeline_key(|app| app.seek_clip(true)),
            KeyCode::Char('N') => self.timeline_key(|app| app.seek_clip(false)),

            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Char('m') => self.toggle_mouse(),
            KeyCode::Char('q') => self.quit = true,
            // Esc leaves *state* (the help overlay, a visual selection) and never
            // quits: `q` and Ctrl+c are the only ways out. A double-Esc quit was a
            // design mistake — it makes the safety key destructive.
            KeyCode::Esc => {
                if self.help {
                    self.help = false;
                } else if self.mode == Mode::Visual {
                    self.mode = Mode::Normal;
                    if let Some(view) = self.view.as_mut() {
                        view.selection = None;
                    }
                }
            }
            KeyCode::Char('c') if ctrl => self.quit = true,
            _ => {}
        }
    }

    /// Run `action` only when the timeline is the focused panel.
    fn timeline_key(&mut self, action: impl FnOnce(&mut App)) {
        if self.focus == Panel::Timeline {
            action(self);
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
    fn plus(&mut self) {
        match self.focus {
            Panel::Timeline => self.timeline_zoom(true),
            Panel::Mixer => self.ride(0.05),
        }
    }

    fn minus(&mut self) {
        match self.focus {
            Panel::Timeline => self.timeline_zoom(false),
            Panel::Mixer => self.ride(-0.05),
        }
    }

    fn zero(&mut self) {
        match self.focus {
            Panel::Timeline => self.timeline_fit(),
            Panel::Mixer => self.set_fader(self.selected, 1.0),
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
                Some(command) => self.command("set_param", command),
                None => self.status = format!("set_param {param}: nothing parsed"),
            },
            Err(e) => self.status = format!("set_param {param}: {e}"),
        }
    }

    /// `j`/`k`: the focused panel's vertical movement — mixer channel, or the
    /// timeline's active track.
    fn vertical(&mut self, direction: i64) {
        match self.focus {
            Panel::Mixer => self.select(direction),
            Panel::Timeline => {
                let lanes = self.arrangement.as_ref().map_or(0, |a| a.lanes.len());
                if lanes == 0 {
                    return;
                }
                self.active_track =
                    (self.active_track as i64 + direction).clamp(0, lanes as i64 - 1) as usize;
            }
        }
    }

    /// `Tab`/`Shift-Tab`: move the focus ring.
    fn cycle_focus(&mut self, _direction: i64) {
        if self.arrangement.is_none() {
            self.focus = Panel::Mixer; // only one panel to focus
            return;
        }
        self.focus = match self.focus {
            Panel::Mixer => Panel::Timeline,
            Panel::Timeline => Panel::Mixer,
        };
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

    /// Dispatch an `arrange` line **through the host's parser**, so the shell and
    /// the CLI share one vocabulary: the ops a key runs are the ops a script
    /// writes, and the host logs them like any other command.
    fn arrange(&mut self, line: &str) {
        match host::parse_arrange_line(line) {
            Ok((op, at_frame)) => {
                self.command("arrange", HostCommand::Arrange { op, at_frame });
                self.refresh_arrangement();
            }
            Err(e) => self.status = format!("arrange: {e}"),
        }
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
        let at = self.snap.frame;

        self.status = format!("split {clip_id} at {at} → {left} + {right}");
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
        let arrangement = self.arrangement.as_ref()?;
        let lane = arrangement.lanes.get(self.active_track)?;
        let clip = arrangement.clip_at(self.active_track, self.snap.frame)?;
        Some((lane.id.clone(), clip.id.clone()))
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
    /// dispatching exactly the command a key would, so there is one log.
    fn timeline_seek(&mut self, position: Position) {
        let (Some(arrangement), Some(view)) = (&self.arrangement, &self.view) else {
            return;
        };
        let cell = position.x.saturating_sub(self.panel.ruler.x) as u64;
        let frame = view.frame_at(cell).min(arrangement.frames);
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
    fn action_at(&self, position: Position) -> Option<Action> {
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
            self.draw_timeline(frame, timeline_area);
            self.draw_mixer(frame, mixer_area);
            self.draw_foot(frame, foot);
        } else {
            let [head, controls, meters, foot] = Layout::vertical([
                Constraint::Length(5),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(4),
            ])
            .areas(area);

            self.draw_head(frame, head);
            self.draw_controls(frame, controls);
            self.draw_meters(frame, meters);
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
        );
        self.panel = panel;
    }

    fn draw_head(&self, frame: &mut Frame, area: Rect) {
        let transport = if self.snap.playing {
            "playing"
        } else {
            "stopped"
        };

        let line = format!(
            "position {:.3} s   beat {:.2}   tempo {:.1} bpm   frame {}   {}",
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
            ("Play", Action::Play, playing),
            ("Stop", Action::Stop, !playing),
            ("Rewind", Action::Rewind, false),
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
        let mode_label = format!(" {} ", self.mode.label());
        let mode = if self.mode == Mode::Visual {
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

        // Which panel the keys act on, the active track, the viewport, and the
        // selection — all of it on one line so the state is never guessed.
        let focus = format!("focus {}   ", self.focus.label());
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

        let paragraph = Paragraph::new(vec![
            ratatui::text::Line::from(vec![
                mode,
                Span::raw(format!("  {focus}mouse {}   {latency}", on_off(self.mouse))),
                Span::styled(timeline, Style::new().fg(Color::Gray)),
            ]),
            self.status.clone().into(),
        ])
        .block(Block::bordered().title(" state "))
        .wrap(Wrap { trim: true });

        frame.render_widget(paragraph, area);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(64, KEYMAP.len() as u16 + 4, area);
        frame.render_widget(Clear, popup);

        let rows: Vec<ratatui::text::Line> = KEYMAP
            .iter()
            .map(|(keys, meaning)| {
                ratatui::text::Line::from(vec![
                    Span::styled(
                        format!("{keys:<18}"),
                        Style::new()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(*meaning),
                ])
            })
            .collect();

        frame.render_widget(
            Paragraph::new(rows)
                .block(Block::bordered().title(" keys — ? or Esc closes "))
                .wrap(Wrap { trim: true }),
            popup,
        );
    }
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

    outcome
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
    Ok(())
}

/// The one-clip host script the shell synthesises for `--wave`. The host owns the
/// result, so a loaded file is **audible**, editable and logged like any other
/// arrangement — and the panel draws the engine's value, not the shell's picture
/// of a file the host has never seen.
fn wave_script(path: &std::path::Path) -> Result<String, String> {
    let reader = media::wav::WavReader::open(path).map_err(|e| e.to_string())?;
    let frames = reader.total_frames();
    if frames == 0 {
        return Err("the file has no audio frames".to_string());
    }

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "the file name is not valid UTF-8".to_string())?;
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));

    // The text format is whitespace-separated, so a path with a space cannot be
    // named in it. Renaming is the honest fix; quoting soup is not.
    for part in [stem, dir.to_str().unwrap_or("")] {
        if part.split_whitespace().count() != 1 {
            return Err(format!(
                "'{part}' contains whitespace — copy the file to a simple path first"
            ));
        }
    }

    Ok(format!(
        "host v1\nmount mixer channels=2 @0\npool {}\narrange add_track t0\narrange add_clip t0 c0 {stem} 0 {frames} 0 0 0 1.0\n",
        dir.display()
    ))
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
            Ok(text) => (format!("wave {}", path.display()), text),
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
        peak = peak.max(snap.channels[0]).max(snap.master);
        println!(
            "probe: frame={:>7} t={:>6.3}s beat={:>6.2} playing={:<5} ch0={:.4} master={:.4}",
            snap.frame, snap.seconds, snap.beat, snap.playing, snap.channels[0], snap.master,
        );
    }

    let last = host.snapshot();
    let _ = host.execute(HostCommand::TransportStop);
    host.shutdown();

    println!("probe: peak ch0/master = {peak:.4}");

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
        let screen = rendered(&mut app);

        for (keys, _) in KEYMAP {
            // The overlay pads the key column, so match on the first token.
            let token = keys.split_whitespace().next().expect("a key");
            assert!(
                screen.contains(token),
                "key `{token}` missing from the overlay:\n{screen}"
            );
        }
    }

    #[test]
    fn a_click_on_a_transport_button_maps_to_its_action() {
        let mut app = App::demo();
        let _ = rendered(&mut app);

        let (play, _) = app
            .buttons
            .iter()
            .find(|(_, action)| *action == Action::Play)
            .expect("the play button was laid out");

        let centre = Position::new(play.x + play.width / 2, play.y);
        assert_eq!(app.action_at(centre), Some(Action::Play));

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

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn visual_mode_selects_from_the_playhead_and_escape_leaves_it() {
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
        let path = wav_fixture("focus");
        let mut app = App::demo();
        assert_eq!(app.focus, Panel::Mixer, "no arrangement → the mixer");

        app.open_wave(&path);
        let _ = rendered(&mut app);
        assert_eq!(app.focus, Panel::Timeline, "loading adopts the timeline");

        // With the mixer focused, timeline keys are inert.
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(app.focus, Panel::Mixer);
        let before = app.view.as_ref().expect("a view").start;
        app.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()));
        assert_eq!(
            app.view.as_ref().expect("a view").start,
            before,
            "the timeline does not scroll while the mixer has the keys"
        );

        // Shift-Tab (BackTab) goes back, and then the keys act again.
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
}
