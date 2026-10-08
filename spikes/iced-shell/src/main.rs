//! The iced spike — the minimal proof that **iced can be a sound-arranger shell**
//! (in-process Rust, replacing the retired Tauri + Vue transport).
//!
//! The evaluation this serves is written down in
//! `.agents/notes/proposed/architecture/2026-09-21-iced-shell-evaluation.md`.
//! What is deliberately in scope here (and nothing more):
//!
//! 1. **The window opens** on this host (niri/Wayland, Mesa) — iced's winit +
//!    wgpu path, the thing the parked Nix shell died on.
//! 2. **The host thread is unchanged.** The shell owns a
//!    [`host::live::HostHandle`] — the exact actor the retired Tauri bridge used — and
//!    never touches the render path. No IPC, no serde wire, no webview.
//! 3. **Transport** (play / stop / rewind + spacebar) drives it.
//! 4. **Meters and the playhead follow the audio** — the published `Snapshot` is
//!    polled on every window frame.
//!
//! 5. **It speaks the shared workflow.** Modes, keys, the `:` command line and the
//!    keymap help come from `workflow` — the same table the ratatui shell reads — so
//!    the two shells cannot grow two workflows. Actions iced cannot express *yet*
//!    (splitting a clip, trimming it, moving it) report themselves in the status line
//!    instead of doing nothing: the gap is visible on purpose, because the rule is
//!    "no shell-only features".
//!
//! 6. **The mixer is real** — channel and master faders are `iced_audio`
//!    `VSlider`s on a dB range (`DBRange`), and their positions come from the
//!    host's **parameter fold**, not from the widget's own memory: a script's
//!    `set_param mixer master.gain 0.8` shows up as a fader at 0.8 (-1.9 dB), and
//!    dragging a fader sends a logged `set_param` back. That is the mixer path a
//!    shell needs, with stock widgets.
//!
//! 7. **The timeline is a real canvas** — one lane per track from the arrangement the
//!    snapshot publishes (`Snapshot::timeline`), each clip a rectangle whose `at_frame`
//!    and `src_len` map onto the canvas width, with the playhead at the transport's
//!    frame. The span is derived (the arrangement's end, or the furthest frame played),
//!    never a shell-side constant.
//!
//! It is *not* a UI yet: there is no waveform, no clip editing, no text-heavy
//! layout. Those are the parts that decide iced against ratatui
//! ([the two candidate shells](../../.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md)),
//! and the evaluations record them as the next step.
//!
//! ```sh
//! cd spikes/iced-shell
//! cargo run              # the window (needs a display + an audio device)
//! cargo run -- --keys    # …with the shared keymap overlay open
//! cargo run -- --probe   # headless: host thread + snapshot polling, no window
//! cargo test             # the key translation, the command line, the console keys
//! ```

use std::time::Duration;

use host::HostCommand;
use host::live::{HostHandle, Snapshot, TimelineStatus};

use iced::keyboard;
use iced::widget::canvas;
use iced::widget::{column, container, progress_bar, row, text, vertical_slider};
use iced::window;
use iced::{
    Center, Color, Element, Fill, Length, Point, Rectangle, Renderer, Size, Subscription, Theme,
};
use iced_audio::{DBRange, Normal, NormalParam};
use workflow::{Action, Key as WorkflowKey, Mode};

/// The demo profile — the `docs/FIRST_SESSION.md` chain, so the meters have
/// signal on launch: a euclidean generator → scale → tone → mixer channel 0.
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

/// The session rate, for turning a seek in seconds into a frame when no device is
/// open (the snapshot carries the device's rate only when audio is).
const SAMPLE_RATE: i64 = 48_000;

/// The console fader's geometry: the height a fader is drawn at, and therefore
/// the distance a drag has to cover to cross its whole range.
const FADER_HEIGHT: f32 = 150.0;

/// The fader's floor: -60 dB reads as silence, and the linear gain at the
/// bottom of the declared range is 0 (true silence, so the floor is a display
/// choice, not a mapping constraint).
const FADER_FLOOR_DB: f32 = -60.0;

/// Where unity sits on the throw. Not a free choice: it follows from the
/// declared gain bounds ([0, 2] -> -inf..+6.02 dB), which puts 0 dB at ~91 %.
fn fader_unity() -> Normal {
    Normal::new((0.0 - FADER_FLOOR_DB) / (fader_max_db() - FADER_FLOOR_DB))
}

/// The fader range, derived from the **mixer's own declared bounds**.
///
/// The ceiling is `20·log10` of the largest gain the engine declares, so the top
/// of every fader is a value the host accepts. Hard-coding +12 dB was the bug:
/// the mixer declares gain `max: 2.0` (+6.02 dB), so a fader at its top asked
/// for gain 3.98 and the host refused it — "parameter 'ch0.gain' out of range
/// [0, 2]" — while the fader snapped back to the unchanged real value.
fn fader_range() -> DBRange {
    DBRange::new(
        FADER_FLOOR_DB,
        fader_max_db(),
        fader_unity(),
        DBRange::DEFAULT_SKEW_FACTOR,
    )
}

/// The largest gain the mixer declares, in dB.
///
/// Panics at boot if the mixer's gain bounds are missing or inconsistent: a
/// silent fallback would put the fader and the host out of step again, which is
/// exactly the defect this replaced.
fn fader_max_db() -> f32 {
    let gains: Vec<f32> = engine::plugins::MIXER_PARAMS
        .iter()
        .filter(|def| def.name.ends_with(".gain"))
        .map(|def| def.max)
        .collect();
    let first = *gains.first().expect("the mixer declares gain parameters");
    assert!(
        gains.iter().all(|max| (*max - first).abs() < f32::EPSILON),
        "the mixer's gain bounds disagree: {gains:?}"
    );
    assert!(first > 0.0, "a gain ceiling of {first} has no dB range");
    20.0 * first.log10()
}

/// The fader's step over the unit range.
///
/// iced's `VerticalSlider` defaults `step` to `T::from(1)` — for `f32` a step of
/// **1.0**. Over a `0.0..=1.0` range its drag maths
/// (`(percent * (end - start) / step).round()`) then yields only 0 or 1, so the
/// fader had exactly two positions: `-60.0 dB` and `+6.0 dB`. A fader needs a
/// step smaller than anything a hand can express.
const FADER_STEP: f32 = 0.001;

/// Build one fader. Every fader goes through here so the step cannot be
/// forgotten at a call site — the bug above was a default nobody set.
fn fader_widget(index: usize, value: f32) -> Element<'static, Message> {
    vertical_slider(0.0..=1.0, value, move |normal| {
        Message::Fader(index, normal)
    })
    .step(FADER_STEP)
    .width(18.0)
    .height(FADER_HEIGHT)
    .into()
}

/// `set_param` targets as `&'static str` (the command carries static names):
/// `ch0.gain` … `ch7.gain`, then the master. `MIXER_CHANNELS_MAX` is 8.
const GAIN_PARAMS: [&str; 8] = [
    "ch0.gain", "ch1.gain", "ch2.gain", "ch3.gain", "ch4.gain", "ch5.gain", "ch6.gain", "ch7.gain",
];
/// The master's fader is the strip *after* the channels.
const MASTER_GAIN: &str = "master.gain";

/// The strip a `set_param` gain name belongs to (channels first, the master
/// last) — the one index mapping the fold and the widgets must agree on.
fn strip_of(param: &str, channels: usize) -> Option<usize> {
    if param == MASTER_GAIN {
        return Some(channels);
    }
    let channel = param
        .strip_prefix("ch")?
        .strip_suffix(".gain")?
        .parse::<usize>()
        .ok()?;
    (channel < channels).then_some(channel)
}

fn main() -> iced::Result {
    if std::env::args().any(|arg| arg == "--probe") {
        std::process::exit(probe());
    }

    // `--sweep` drives a fader the way the mouse does, in-process, and reports
    // what it costs the audio device. A GUI drag cannot be piped in, and the
    // question ("does a drag starve the device?") is a measurement, not a claim.
    if std::env::args().any(|arg| arg == "--sweep") {
        std::process::exit(sweep());
    }

    // `--record-check` drives a whole take through the real host: start, capture,
    // stop, and the pool source it left behind. Recording is the one MVP verb whose
    // proof needs a real input device, so it cannot be a unit test.
    if std::env::args().any(|arg| arg == "--record-check") {
        std::process::exit(record_check());
    }

    // `--record-underrun` samples the **output device's** underrun counter across a
    // play+record cycle. Play alone is clean (see `--sweep`), so the question the
    // owner's report raises is whether starting a take bursts the counter once or
    // starves it for the whole take — a burst is cosmetic, sustained starvation is
    // real dropout. Additive probe; it records one take like `--record-check`.
    if std::env::args().any(|arg| arg == "--record-underrun") {
        std::process::exit(record_underrun());
    }

    // `--keys` opens the keymap overlay at boot: the same affordance the TUI spike has,
    // and what makes the help screenshotable (a GUI's keys cannot be piped in).
    let keys = std::env::args().any(|arg| arg == "--keys");

    iced::application(move || Spike::boot(keys), Spike::update, Spike::view)
        .title(Spike::title)
        .subscription(Spike::subscription)
        .theme(Spike::theme)
        .window_size((920.0, 520.0))
        .run()
}

/// The whole shell state: the host actor (a `Send + Sync` handle — the session
/// itself stays on its own thread, as in the Tauri bridge) plus the last
/// published snapshot and a status line.
struct Spike {
    host: HostHandle,
    snap: Snapshot,
    status: String,
    /// One fader per mixer channel plus the master, as `iced_audio` normalized
    /// parameters. The *values* live in the host's log fold; these are the
    /// widget's view of it.
    faders: Vec<NormalParam>,
    channels: usize,
    /// Which strip `j`/`k` and `+`/`-`/`0` act on (channels first, master last).
    selected: usize,
    /// The editing mode, from the shared workflow. Always on screen.
    mode: Mode,
    /// The `:` command line, `Some(text)` while it is open; the same escape hatch the
    /// TUI has, over the same `host v1` lines.
    prompt: Option<String>,
    history: Vec<String>,
    history_at: usize,
    help: bool,
    /// The furthest frame playback has reached **in this pass**, so the timeline keeps a
    /// span while stopped. A shell-side memory, not engine state: the span is the **max**
    /// of this and the arrangement's own end (`timeline_span`), so a loaded piece shows
    /// its whole length before it is played. It **resets when the transport moves
    /// backwards** — a `:load` starts a new session at frame 0, and a seek or rewind
    /// starts a new pass; keeping the old maximum would compress a freshly loaded piece
    /// into the left of the canvas (see `advance_span`).
    span_frames: u64,
    /// The shared workflow's snap grid. The iced timeline draws the arrangement, but
    /// it has no editing gestures yet, so the grid cannot quantize an edit here — the
    /// *state* and its cycling order are the workflow's, and the shell says which
    /// division is armed rather than pretending the key does nothing.
    grid: workflow::Grid,
}

/// A pool directory for this run, **created**, so the shell can record.
///
/// `set_pool` refuses a directory that does not exist (load-bearing: the rebuild
/// path relies on that refusal, see `HostSession::set_pool`), so making the
/// directory is the shell's errand. The path sits under the workspace's ignored
/// `target/`, derived from the crate's own manifest directory — deterministic no
/// matter where the binary is invoked, unlike walking up from the cwd. The spike
/// lives at `spikes/iced-shell`, so the workspace root is two levels up.
fn demo_pool_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("iced-pool")
}

#[derive(Debug, Clone)]
enum Message {
    /// A repaint tick — poll the host's published snapshot.
    Tick,
    Play,
    Stop,
    Rewind,
    /// A key press, already translated into the shared workflow's vocabulary.
    Key(WorkflowKey),
    /// A fader gesture: `strip` is the channel (or the master, last).
    Fader(usize, f32),
}

impl Spike {
    /// Boot: open the live host (with audio when a device exists) and load the
    /// demo profile. A failure is surfaced in `status`, never a panic — a shell
    /// with no signal is a state, not a crash.
    fn boot(keys: bool) -> Self {
        let mut spike = Self::from_host(HostHandle::spawn_with_audio());
        spike.help = keys;
        spike
    }

    /// A silent host for the tests (no device, so they run anywhere) — the same
    /// construction path, so the state cannot differ between the app and its tests.
    #[cfg(test)]
    fn headless() -> Self {
        Self::from_host(HostHandle::spawn())
    }

    fn from_host(host: HostHandle) -> Self {
        let demo = match host::parse_script(DEMO_SCRIPT) {
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
        // The pool is set **after** the load, because `load` replaces the session
        // wholesale — setting it first was silently discarded, and `record` then
        // refused for want of a pool that had been chosen and thrown away. Created
        // here because `set_pool` refuses a directory that does not exist (a
        // load-bearing refusal, see `HostSession::set_pool`). The path is resolved
        // once: it is the same directory for the create and the bind.
        let pool_dir = demo_pool_dir();
        // The pool is named only when it failed: a status line is for what went
        // wrong, not for the bookkeeping that worked.
        let pool_status = match std::fs::create_dir_all(&pool_dir) {
            Ok(()) => match host.execute(HostCommand::Pool { dir: pool_dir }) {
                Ok(()) => String::new(),
                Err(e) => format!("pool refused: {e}"),
            },
            Err(e) => format!("cannot create the pool: {e}"),
        };
        let status = if pool_status.is_empty() {
            demo
        } else {
            format!("{pool_status} — {demo}")
        };

        let mut spike = Spike {
            snap: host.snapshot(),
            host,
            status,
            faders: vec![fader_range().default_param(); 1],
            channels: 0,
            selected: 0,
            mode: Mode::Normal,
            prompt: None,
            history: Vec::new(),
            history_at: 0,
            help: false,
            span_frames: 0,
            grid: workflow::Grid::default(),
        };
        spike.adopt();
        spike
    }

    /// Take the mixer's fader positions from the host's **parameter fold** — the
    /// log is the source of truth, exactly as `HostOutcome::params` reports it,
    /// never the widget's own memory. Called at boot and when a gesture ends, so
    /// a fader the log moved (a script, a replay, an undo) is visible here.
    fn adopt(&mut self) {
        let outcome = match self.host.outcome() {
            Ok(outcome) => outcome,
            Err(e) => {
                self.status = format!("cannot read the host: {e}");
                return;
            }
        };

        let channels = outcome
            .params
            .iter()
            .find(|(plugin, param, _)| *plugin == "mixer" && *param == "channels")
            .map(|(_, _, value)| (*value).max(0.0) as usize)
            .unwrap_or(self.channels)
            .min(GAIN_PARAMS.len() - 1);
        if channels + 1 != self.faders.len() {
            self.channels = channels;
            self.faders = vec![fader_range().default_param(); channels + 1];
        }

        for (plugin, param, value) in &outcome.params {
            if *plugin != "mixer" {
                continue;
            }
            if let Some(index) = strip_of(param, channels) {
                self.set_fader(index, *value);
            }
        }
    }

    /// Show `gain` (linear) on fader `index`, through the fader's dB range.
    fn set_fader(&mut self, index: usize, gain: f32) {
        if let Some(fader) = self.faders.get_mut(index) {
            let db = if gain > 0.0 {
                20.0 * gain.log10()
            } else {
                f32::NEG_INFINITY
            };
            fader.set(fader_range().map_db(db));
        }
    }

    /// The `set_param` target for a strip: channels first, the **master last**
    /// (its fader index is the channel count, so the two index spaces differ).
    fn gain_param(index: usize, channels: usize) -> &'static str {
        if index >= channels {
            MASTER_GAIN
        } else {
            GAIN_PARAMS[index.min(GAIN_PARAMS.len() - 1)]
        }
    }

    /// A fader gesture: move the widget, then send the host a logged `set_param`
    /// with the gain that position means. The engine smooths the change, so a
    /// drag is a stream of parameter events — which is what automation is.
    fn gesture(&mut self, index: usize, normal: f32) {
        let normal = Normal::new(normal.clamp(0.0, 1.0));
        if let Some(fader) = self.faders.get_mut(index) {
            fader.set(normal);
        }
        let param = Spike::gain_param(index, self.channels);
        let db = fader_range().unmap_to_db(normal);
        let gain = 10f32.powf(db / 20.0);
        self.status = format!("{param} = {db:+.1} dB ({gain:.4})");
        let _ = self.command(HostCommand::SetParam {
            plugin: "mixer",
            param,
            value: gain,
            at_frame: None,
        });
    }

    fn title(&self) -> String {
        "sound-arranger · iced spike".to_string()
    }

    /// A fixed dark theme — iced's built-in theme enum is the whole theming story
    /// here; the retired Vue shell's token contract has no iced equivalent yet.
    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Tick => {
                self.snap = self.host.snapshot();
                // The played-span memory the timeline's width derives from: the furthest
                // the transport has been **this pass**, so the view is stable while
                // stopped instead of rescaling every frame. A backward move (a `:load`,
                // a seek, a rewind) starts a new pass and forgets the old maximum.
                self.span_frames = advance_span(self.span_frames, self.snap.frame);
            }
            Message::Play => self.play(),
            Message::Stop => {
                let _ = self.command(HostCommand::TransportStop);
            }
            Message::Rewind => self.rewind(),
            Message::Key(key) => self.on_key(key),
            Message::Fader(index, gesture) => self.gesture(index, gesture),
        }
    }

    /// Translate iced's key event into the shared workflow's neutral key — the mirror
    /// of the TUI's translation, and the only place this shell knows iced's key types.
    fn workflow_key(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<WorkflowKey> {
        use keyboard::key::Named;

        Some(match key.as_ref() {
            keyboard::Key::Named(Named::Space) => WorkflowKey::Space,
            keyboard::Key::Named(Named::Enter) => WorkflowKey::Enter,
            keyboard::Key::Named(Named::Escape) => WorkflowKey::Esc,
            keyboard::Key::Named(Named::Backspace) => WorkflowKey::Backspace,
            keyboard::Key::Named(Named::Tab) if modifiers.shift() => WorkflowKey::BackTab,
            keyboard::Key::Named(Named::Tab) => WorkflowKey::Tab,
            keyboard::Key::Named(Named::ArrowUp) => WorkflowKey::Up,
            keyboard::Key::Named(Named::ArrowDown) => WorkflowKey::Down,
            keyboard::Key::Named(Named::ArrowLeft) => WorkflowKey::Left,
            keyboard::Key::Named(Named::ArrowRight) => WorkflowKey::Right,
            keyboard::Key::Named(Named::Home) => WorkflowKey::Home,
            keyboard::Key::Character(s) => {
                let c = s.chars().next()?;
                if modifiers.control() {
                    WorkflowKey::Ctrl(c.to_ascii_lowercase())
                } else {
                    // iced reports the *logical* key, so Shift+j arrives as "J".
                    WorkflowKey::Char(c)
                }
            }
            _ => return None,
        })
    }

    /// The modal key path, exactly as the TUI's: the command line and the help take
    /// every key while they are open, and everything else goes through the **shared
    /// workflow** — the same table, the same actions.
    fn on_key(&mut self, key: WorkflowKey) {
        if self.prompt.is_some() {
            self.prompt_key(key);
            return;
        }
        if self.help {
            if matches!(
                workflow::action(key),
                Some(Action::Help | Action::Cancel | Action::Quit)
            ) {
                self.help = false;
            }
            return;
        }
        // An unbound key does nothing — the same as the TUI. A shell that reports
        // every stray key teaches the user to ignore its status line.
        if let Some(action) = workflow::action(key) {
            self.dispatch(action);
        }
    }

    /// The command line's keys — text, so this is the shell's own business.
    fn prompt_key(&mut self, key: WorkflowKey) {
        match key {
            WorkflowKey::Enter => self.run_prompt(),
            WorkflowKey::Esc | WorkflowKey::Ctrl('c') => {
                self.prompt = None;
                self.mode = Mode::Normal;
                self.status = "command line cancelled".to_string();
            }
            WorkflowKey::Backspace => {
                if let Some(text) = self.prompt.as_mut() {
                    text.pop();
                }
            }
            WorkflowKey::Up => self.history_step(-1),
            WorkflowKey::Down => self.history_step(1),
            WorkflowKey::Char(c) => {
                if let Some(text) = self.prompt.as_mut() {
                    text.push(c);
                }
            }
            _ => {}
        }
    }

    /// One workflow action. Every arm is shared vocabulary; an action iced cannot
    /// express yet **says so** rather than doing nothing — the parity debt is visible
    /// by design.
    fn dispatch(&mut self, action: Action) {
        match action {
            Action::PlayToggle => self.toggle(),
            Action::RecordToggle => self.toggle_record(),
            Action::Stop => {
                let _ = self.command(HostCommand::TransportStop);
            }
            Action::Rewind => self.rewind(),
            Action::SeekSeconds(seconds) => self.seek_seconds(seconds),
            Action::Vertical(direction) => self.select_strip(direction),
            Action::Zoom(direction) => self.ride(direction),
            Action::Fit => self.set_selected_fader(1.0),
            Action::Undo => {
                let _ = self.command(HostCommand::Undo);
            }
            Action::Redo => {
                let _ = self.command(HostCommand::Redo);
            }
            Action::Prompt => {
                self.prompt = Some(String::new());
                self.history_at = self.history.len();
                self.mode = Mode::Command;
            }
            Action::Help => {
                self.help = !self.help;
                self.status = "the keymap — `?` or Esc closes it".to_string();
            }
            Action::Quit => {
                // iced has no "exit" message; the window close is the way out.
                self.status =
                    "close the window to quit (the TUI's `q` has no iced twin yet)".to_string();
            }
            Action::ToggleMouseCapture => {
                self.status =
                    "mouse capture is a terminal concern — nothing to toggle here".to_string();
            }
            Action::Cancel => {
                self.status.clear();
            }
            Action::ExportMix => {
                // The workflow's export gesture, prefilled like the TUI's: the iced shell
                // has the same command line, so the render-out path is shared rather than
                // a gap (only the *editing gestures* are missing, not the gesture).
                self.prompt = Some("export mix.wav f32".to_string());
                self.status =
                    "export the whole arrangement: edit the path/format, then Enter".to_string();
            }
            Action::GridCycle => {
                self.grid.cycle();
                self.status = if self.grid.is_on() {
                    format!(
                        "snap grid: {} — the grid is workflow state, but the iced timeline's \
                         editing gestures are not built yet, so nothing snaps here",
                        self.grid.label()
                    )
                } else {
                    "snap grid off".to_string()
                };
            }
            other => {
                self.status = format!(
                    "`{}` needs the timeline — the workflow's keys are shared, but the iced \
                     shell's timeline editing is not built yet",
                    other.name()
                );
            }
        }
    }

    /// Run a typed `host v1` line through the host's own parser — the same path the
    /// TUI's command line takes, one line = one command.
    fn run_prompt(&mut self) {
        let Some(raw) = self.prompt.take() else {
            return;
        };
        self.mode = Mode::Normal;
        let line = raw.trim().to_string();
        if line.is_empty() {
            return;
        }
        self.history.push(line.clone());
        self.history_at = self.history.len();

        let script = format!("host v1\n{line}\n");
        let commands = match host::parse_script(&script) {
            Ok(commands) => commands,
            Err(e) => {
                self.status = format!(": {line} — {e}");
                return;
            }
        };
        for command in commands {
            if let Err(e) = self.host.execute(command) {
                self.status = format!(": {line} — {e}");
                return;
            }
        }
        self.status = format!(": {line}");
        self.adopt();
    }

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
        self.prompt = Some(self.history.get(next).cloned().unwrap_or_default());
    }

    /// Spacebar: start or stop, whichever the transport is not.
    fn toggle(&mut self) {
        if self.snap.playing {
            let _ = self.command(HostCommand::TransportStop);
        } else {
            self.play();
        }
    }

    fn play(&mut self) {
        let _ = self.command(HostCommand::TransportPlay);
    }

    fn rewind(&mut self) {
        // Seek is "rebuild + render to target", so it is intended for a stopped
        // transport: stop first, then move the playhead home.
        let _ = self.command(HostCommand::TransportStop);
        let _ = self.command(HostCommand::TransportSeek { frame: 0 });
    }

    /// `,`/`.`: ±1 s, stopping first (a seek is rebuilt to the target frame).
    fn seek_seconds(&mut self, seconds: i64) {
        let rate = self
            .snap
            .audio
            .as_ref()
            .map_or(SAMPLE_RATE, |a| a.sample_rate as i64);
        let target = (self.snap.frame as i64 + seconds * rate).max(0) as u64;
        let _ = self.command(HostCommand::TransportStop);
        let _ = self.command(HostCommand::TransportSeek { frame: target });
    }

    /// `j`/`k`: move the console's selected strip (channels first, master last).
    fn select_strip(&mut self, direction: i32) {
        let count = self.faders.len() as i32;
        if count == 0 {
            return;
        }
        self.selected = (self.selected as i32 + direction).clamp(0, count - 1) as usize;
        let label = if self.selected >= self.channels {
            "master".to_string()
        } else {
            format!("ch{}", self.selected)
        };
        self.status = format!("selected {label}");
    }

    /// `+`/`-`: ride the selected fader (the workflow's `Zoom`, which the focused
    /// panel reads as zoom or as a fader move — iced has a console, so it is a fader).
    fn ride(&mut self, direction: i32) {
        let index = self.selected;
        if index >= self.faders.len() {
            return;
        }
        let current = fader_range().unmap_to_db(self.faders[index].normal);
        self.set_selected_fader(10f32.powf((current + 0.5 * direction as f32) / 20.0));
    }

    /// Move the selected fader to `gain` and log it — the same `set_param` the TUI
    /// sends, through the same fold.
    fn set_selected_fader(&mut self, gain: f32) {
        let index = self.selected;
        if index >= self.faders.len() {
            return;
        }
        let normal = fader_range().map_db(if gain > 0.0 {
            20.0 * gain.log10()
        } else {
            f32::NEG_INFINITY
        });
        self.gesture(index, normal.as_f32());
    }

    fn subscription(&self) -> Subscription<Message> {
        // `window::frames()` fires on every redraw, and each redraw is itself a
        // message, so the loop is self-sustaining: poll the host, repaint, repeat
        // — i.e. the playhead follows the audio at the display's rate. (iced's
        // `time::every` needs the tokio/smol time backend, which the default
        // feature set does not enable; frames need no timer backend at all.)
        Subscription::batch([
            window::frames().map(|_| Message::Tick),
            keyboard::listen().filter_map(|event| {
                // The workflow decides what a key means; this only translates the
                // toolkit's event into its vocabulary (and drops what it cannot name).
                let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
                    return None;
                };
                Spike::workflow_key(&key, modifiers).map(Message::Key)
            }),
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let reading = row![
            stat("position", format!("{:.3} s", self.snap.seconds)),
            stat("beat", format!("{:.2}", self.snap.beat)),
            stat("tempo", format!("{:.1} bpm", self.snap.bpm)),
            stat("frame", format!("{}", self.snap.frame)),
            stat("transport", transport_label(&self.snap).to_string()),
        ]
        .spacing(24);

        let controls = row![
            control("Play", Message::Play),
            control("Stop", Message::Stop),
            control("Rewind", Message::Rewind),
            text("space = play/stop · r = rewind · o = record/stop").size(12),
        ]
        .spacing(12)
        .align_y(iced::Center);

        // The mode is always on screen — a modal UI that hides its mode is a trap.
        let mode = container(text(format!(" {} ", self.mode.label())).size(12))
            .padding(2)
            .style(|_theme| container::Style {
                background: Some(
                    if self.mode == Mode::Normal {
                        iced::Color::from_rgb(0.25, 0.25, 0.28)
                    } else {
                        iced::Color::from_rgb(0.45, 0.25, 0.55)
                    }
                    .into(),
                ),
                ..container::Style::default()
            });

        // The second line is the status — or the command line, where a `:` belongs.
        let line = match &self.prompt {
            Some(typed) => row![
                text(":").size(14),
                text(typed.clone()).size(14),
                text("▏").size(14)
            ]
            .spacing(2),
            None => row![text(&self.status).size(12)].spacing(0),
        };

        let mut body = column![
            row![
                text("iced spike").size(22),
                mode,
                text("the shared workflow: one keymap, two shells").size(12)
            ]
            .spacing(12)
            .align_y(iced::Center),
            reading,
            controls,
            self.console(),
            self.timeline(),
            text(take_label(&self.snap)).size(12),
            text(audio_label(&self.snap)).size(12),
            line,
        ]
        .spacing(14)
        .padding(20);

        if self.help {
            body = body.push(self.keymap());
        }

        container(body).width(Fill).height(Fill).padding(12).into()
    }

    /// The `?` overlay: the keymap, rendered from the **shared table** — the same rows
    /// the TUI shows, so the two shells cannot document different keys.
    fn keymap(&self) -> Element<'_, Message> {
        let mut rows = column![].spacing(3);
        for (keys, meaning) in workflow::help() {
            rows = rows.push(
                row![
                    text(format!("{keys:<18}"))
                        .size(12)
                        .width(Length::Fixed(170.0)),
                    text(meaning).size(12),
                ]
                .spacing(6),
            );
        }

        container(column![text("keys — ? or Esc closes").size(13), rows].spacing(8))
            .padding(10)
            .width(Fill)
            .style(|_theme| container::Style {
                background: Some(iced::Color::from_rgb(0.10, 0.10, 0.12).into()),
                ..container::Style::default()
            })
            .into()
    }

    /// The mixer: per channel and master, a live meter beside a real **fader**
    /// (`iced_audio::VSlider` on the dB range), with the value read out in dB.
    /// The meter is a plain `progress_bar` (the value that matters here is the
    /// fader, and a peak meter with hold is a widget we would style ourselves).
    fn console(&self) -> Element<'_, Message> {
        if self.channels == 0 {
            return text("no mixer mounted").size(12).into();
        }

        let mut strips = row![].spacing(14).height(Length::Fixed(210.0));
        for index in 0..=self.channels {
            let Some(fader) = self.faders.get(index) else {
                continue;
            };
            // Two counts that used to be assumed equal: `self.channels` is the
            // **fold's** mixer width, `snap.channels` is the **snapshot's** meter
            // levels, and they disagree whenever no device is open. Indexing the
            // meter vector by the fold's count panicked at startup ("len is 0 but
            // the index is 0"); a strip we cannot meter simply reads 0.
            let (label, meter) = if index == self.channels {
                ("master".to_string(), self.snap.master)
            } else {
                (
                    format!("ch{index}"),
                    self.snap.channels.get(index).copied().unwrap_or(0.0),
                )
            };
            let db = fader_range().unmap_to_db(fader.normal);

            strips = strips.push(
                column![
                    row![
                        container(progress_bar(0.0..=1.0, meter).vertical())
                            .width(Length::Fixed(16.0))
                            .height(FADER_HEIGHT),
                        fader_widget(index, fader.normal.as_f32()),
                    ]
                    .spacing(6),
                    text(label).size(11),
                    text(format!("{db:+.1} dB")).size(10),
                ]
                .spacing(4)
                .align_x(Center),
            );
        }

        strips.into()
    }

    /// `o`: stop the take in progress, or start one.
    ///
    /// The host needs the take's id *before* the recording starts (a take is
    /// declared state, so the session can replay without the device), which is why
    /// the shell names it. The name is chosen against the ids the host publishes —
    /// `pool_ids` — so a second recording cannot quietly overwrite the first
    /// source, and the shape follows the capture convention the pool already uses:
    /// a take `take-1` writes `take-1.ch0`, `take-1.ch1`, …
    fn toggle_record(&mut self) {
        // Decide from the **host's** current state, not the last repaint: the
        // decision is start-vs-stop, and a stale snapshot can only get that wrong —
        // it either refuses a start it should make or stops a take that already
        // ended. A repaint is at most a frame away, but a key press is not a frame.
        self.snap = self.host.snapshot();
        if let Some(rec) = self.snap.recording.clone() {
            // A stop the host refused is its own answer, already in the status line:
            // overwriting it here would report a take that did not end.
            if self.command(HostCommand::RecordStop).is_err() {
                return;
            }
            // The report is the authoritative one. The stop drains the device, so the
            // pre-stop snapshot's frame count is short by the tail — read what landed
            // from the host (the same `last_take` the take line draws) rather than
            // repainting a count that is already stale.
            match self.host.outcome() {
                Ok(outcome) => {
                    self.snap.recording = outcome.recording;
                    self.snap.last_take = outcome.last_take;
                    self.status = match &self.snap.last_take {
                        Some(t) => format!(
                            "finished {} — {} frames, {} ch (its sources are pool material now)",
                            t.take_id, t.frames, t.channels
                        ),
                        None => format!(
                            "{} stopped — its sources are pool material now",
                            rec.take_id
                        ),
                    };
                }
                Err(e) => {
                    self.status = format!(
                        "{} stopped, but the report could not be read: {e}",
                        rec.take_id
                    );
                }
            }
            return;
        }
        let take_id = self.next_take_id();
        // A successful start says which take is running. A refusal (no pool, or a
        // device that would not open) keeps the host's own words; the `Result` is the
        // answer, not the status line, which an earlier refusal is still sitting in.
        // A take from the machine's default input: naming a declared source is the
        // command line's job (`record <take> source=<name>`), and a shell affordance
        // for it follows the binding slice rather than guessing a source here.
        if self
            .command(HostCommand::Record {
                take_id: take_id.clone(),
                source: None,
            })
            .is_ok()
        {
            self.status = format!("● recording {take_id} — `o` stops it");
        }
    }

    /// The next `take-N` this session does not already hold.
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

    /// The arrangement timeline: a caption and the canvas that draws it.
    ///
    /// The arrangement comes from `Snapshot::timeline` — the host's **cached**
    /// reconstruction, behind an `Arc`, so the per-frame poll clones a pointer rather
    /// than the whole piece. The canvas is a real one: one lane per track, each clip a
    /// rectangle mapped from `at_frame`/`src_len`, and the playhead at `snap.frame`. The
    /// span is the arrangement's end or the furthest frame played, whichever is greater,
    /// so a loaded piece is visible before it is played and the view never rescales
    /// under a stopped transport.
    ///
    /// An `error` on the status is drawn **as the error**, never as an empty timeline:
    /// the host can fail to reconstruct a poisoned arrangement, and a shell that drew
    /// nothing would be saying "the piece is empty" about a piece it could not read.
    fn timeline(&self) -> Element<'_, Message> {
        let status = &self.snap.timeline;
        let span = timeline_span(status, self.span_frames);
        let clips = status
            .timeline
            .as_ref()
            .map_or(0, |t| t.tracks.iter().map(|track| track.clips.len()).sum());
        let caption = format!(
            "{:.2} s / {:.2} s · {} clip(s)",
            self.snap.seconds,
            span as f64 / f64::from(self.snap.sample_rate.max(1)),
            clips
        );
        let tracks = status.timeline.as_ref().map_or(0, |t| t.tracks.len());
        let canvas: Element<'_, Message> = canvas::Canvas::new(TimelineView {
            status: status.clone(),
            frame: self.snap.frame,
            span,
        })
        .width(Fill)
        .height(Length::Fixed(timeline_height(tracks)))
        .into();
        column![
            row![text("timeline").size(11), text(caption).size(11)].spacing(12),
            canvas,
        ]
        .spacing(4)
        .into()
    }

    /// Apply a command and return the host's answer.
    ///
    /// The `Result` is the authority: `execute` is a synchronous round trip through
    /// the actor, so it already knows whether the command was applied. The status
    /// line is only where a refusal is **said** — it is never cleared on success, so
    /// reading it back for `command refused` turns an earlier refusal into a
    /// misreading of this one. A caller that needs the answer matches on this.
    fn command(&mut self, command: HostCommand) -> Result<(), String> {
        match self.host.execute(command) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.status = format!("command refused: {e}");
                Err(e)
            }
        }
    }
}

fn control<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    iced::widget::button(text(label)).on_press(message).into()
}

fn stat<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    column![text(label).size(11), text(value).size(18)]
        .spacing(2)
        .into()
}

/// The take line: a live recording indicator, or the last finished take.
///
/// Read from the **snapshot** (the host publishes the take state every pump tick),
/// so it is live rather than a one-shot message that scrolls away — the same split
/// the TUI's footer makes, from the same fields.
fn take_label(snap: &Snapshot) -> String {
    if let Some(rec) = &snap.recording {
        // Monitor drops are best-effort and zero unless something is monitoring the
        // take, so they are named and shown only when they happen: "N dropped" beside
        // a take read as lost audio, which it is not.
        let monitor = if rec.monitor_dropped > 0 {
            format!(", {} monitor drops", rec.monitor_dropped)
        } else {
            String::new()
        };
        return format!(
            "● REC {} — {} frames, {} ch{monitor}  (`o` stops it)",
            rec.take_id, rec.frames, rec.channels
        );
    }
    match &snap.last_take {
        Some(t) => format!(
            "last take {} — {} frames, {} ch → {} pool source(s)",
            t.take_id,
            t.frames,
            t.channels,
            t.sources.len()
        ),
        None => "no take yet — `o` records one".to_string(),
    }
}

/// The played-span memory after seeing `frame`: the running maximum, **reset** when the
/// transport moves backwards.
///
/// Backwards means a new pass: a `:load` starts a new session at frame 0, and a seek or
/// rewind returns to a frame already played. Keeping the old maximum through that would
/// let the played span dominate the arrangement's own end in [`timeline_span`], drawing
/// a freshly loaded piece compressed into the left of the canvas — the one flow the
/// timeline exists for.
fn advance_span(span: u64, frame: u64) -> u64 {
    if frame < span { frame } else { span.max(frame) }
}

/// The span the timeline maps onto: the arrangement's own end, or the furthest frame
/// the transport has played, whichever is greater.
///
/// The **max** is what makes the view honest in both directions: a loaded piece shows
/// its whole length before it is played (the arrangement end), and a take recorded past
/// the last clip still has somewhere to put the playhead (the played span). Neither
/// alone works: the arrangement end is 0 in an empty session, and the played span is 0
/// while stopped.
fn timeline_span(status: &TimelineStatus, played: u64) -> u64 {
    let end = status
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.end_frame().ok())
        .unwrap_or(0);
    end.max(played)
}

/// A timeline frame's x within a plot `width`, for a `span` of frames.
///
/// The one mapping the timeline draws with, kept pure so the geometry can be tested
/// without a renderer. A zero span (nothing placed, nothing played) draws at the left
/// edge rather than dividing by zero, and a frame past the span clamps to the right edge
/// so a stale span never draws outside the box.
fn frame_x(frame: u64, span: u64, width: f32) -> f32 {
    if span == 0 || width <= 0.0 {
        return 0.0;
    }
    (frame as f64 / span as f64).clamp(0.0, 1.0) as f32 * width
}

/// A clip's `(x, width)` in plot points, from its placement and its span.
///
/// The clip's own width is the mapped distance between `at_frame` and
/// `at_frame + src_len`, so a clip that runs past the span clamps to a zero-width (or
/// truncated) bar at the right edge instead of overflowing the canvas.
fn clip_span(at_frame: u64, src_len: u64, span: u64, width: f32) -> (f32, f32) {
    let x = frame_x(at_frame, span, width);
    let end = frame_x(at_frame.saturating_add(src_len), span, width);
    (x, (end - x).max(0.0))
}

/// The canvas's height for `tracks` lanes — one lane per track, a minimum of one so an
/// empty arrangement still has a bed to draw the message on.
fn timeline_height(tracks: usize) -> f32 {
    let lanes = tracks.max(1) as f32;
    (TIMELINE_PAD * 2.0 + lanes * TIMELINE_LANE_HEIGHT + (lanes - 1.0) * TIMELINE_LANE_GAP)
        .min(TIMELINE_MAX_HEIGHT)
}

/// The timeline canvas's geometry, in points.
const TIMELINE_PAD: f32 = 8.0;
/// The gutter the track ids are drawn in; clips map onto the width **after** it, so a
/// label never sits under a clip.
const TIMELINE_LABEL_WIDTH: f32 = 64.0;
const TIMELINE_LANE_HEIGHT: f32 = 22.0;
const TIMELINE_LANE_GAP: f32 = 6.0;
/// The tallest the timeline grows before lanes start shrinking; past this the shell's
/// other rows would be pushed off the window.
const TIMELINE_MAX_HEIGHT: f32 = 260.0;
const TIMELINE_PLAYHEAD_WIDTH: f32 = 2.0;

const TIMELINE_BG: Color = Color::from_rgb(0.06, 0.06, 0.08);
const TIMELINE_LANE: Color = Color::from_rgb(0.12, 0.12, 0.16);
const TIMELINE_CLIP: Color = Color::from_rgb(0.30, 0.55, 0.85);
const TIMELINE_CLIP_EDGE: Color = Color::from_rgb(0.55, 0.78, 1.0);
const TIMELINE_PLAYHEAD: Color = Color::from_rgb(1.0, 0.55, 0.35);
const TIMELINE_LABEL: Color = Color::from_rgb(0.75, 0.78, 0.85);
/// The muted colour of "there is nothing to draw, and why".
const TIMELINE_MUTED: Color = Color::from_rgb(0.62, 0.48, 0.48);

/// The timeline canvas program.
///
/// It owns its data — a cloned [`TimelineStatus`] (an `Arc` clone, not the piece) plus
/// the playhead frame and the span — so the canvas needs no borrow of the shell and is
/// rebuilt from each snapshot. It is deliberately **not** cached: an edit, an undo or a
/// load can change the arrangement under it, and a cached geometry would keep drawing
/// the piece the shell no longer holds.
struct TimelineView {
    status: TimelineStatus,
    /// The transport's frame, for the playhead.
    frame: u64,
    /// The span every x maps against (see [`timeline_span`]).
    span: u64,
}

impl canvas::Program<Message> for TimelineView {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut canvas_frame = canvas::Frame::new(renderer, bounds.size());
        canvas_frame.fill_rectangle(Point::ORIGIN, bounds.size(), TIMELINE_BG);
        let text_width = (bounds.width - TIMELINE_PAD * 2.0).max(1.0);

        // A failed reconstruction is drawn **as itself**. An empty lane bed here would
        // say "the arrangement is empty" about a piece the host could not read.
        if let Some(error) = &self.status.error {
            canvas_frame.fill_text(canvas::Text {
                content: format!("arrangement unavailable — {error}"),
                position: Point::new(TIMELINE_PAD, TIMELINE_PAD),
                max_width: text_width,
                color: TIMELINE_MUTED,
                size: 12.0.into(),
                ..canvas::Text::default()
            });
            return vec![canvas_frame.into_geometry()];
        }

        let Some(timeline) = &self.status.timeline else {
            canvas_frame.fill_text(canvas::Text {
                content: "waiting for the arrangement on the snapshot".to_string(),
                position: Point::new(TIMELINE_PAD, TIMELINE_PAD),
                max_width: text_width,
                color: TIMELINE_MUTED,
                size: 12.0.into(),
                ..canvas::Text::default()
            });
            return vec![canvas_frame.into_geometry()];
        };

        let plot_x = TIMELINE_LABEL_WIDTH;
        let plot_width = (bounds.width - plot_x - TIMELINE_PAD).max(1.0);
        let lanes = timeline.tracks.len().max(1) as f32;
        let lane_height =
            ((bounds.height - TIMELINE_PAD * 2.0 - TIMELINE_LANE_GAP * (lanes - 1.0)) / lanes)
                .max(4.0);

        if timeline.tracks.is_empty() {
            canvas_frame.fill_text(canvas::Text {
                content: "the arrangement has no tracks".to_string(),
                position: Point::new(plot_x, TIMELINE_PAD),
                max_width: plot_width,
                color: TIMELINE_MUTED,
                size: 11.0.into(),
                ..canvas::Text::default()
            });
        }

        for (index, track) in timeline.tracks.iter().enumerate() {
            let y = TIMELINE_PAD + index as f32 * (lane_height + TIMELINE_LANE_GAP);
            // The lane's bed, so a track with no clip in view still reads as a lane.
            canvas_frame.fill_rectangle(
                Point::new(plot_x, y),
                Size::new(plot_width, lane_height),
                TIMELINE_LANE,
            );
            // The track id, in the gutter the clip mapping leaves free. Text is drawn on
            // top of the geometry by iced, which is exactly where a lane label belongs.
            canvas_frame.fill_text(canvas::Text {
                content: track.id.clone(),
                position: Point::new(4.0, y + lane_height / 2.0),
                max_width: TIMELINE_LABEL_WIDTH - 8.0,
                color: TIMELINE_LABEL,
                size: 11.0.into(),
                align_y: iced::alignment::Vertical::Center,
                ..canvas::Text::default()
            });
            for clip in &track.clips {
                let (x, width) = clip_span(clip.at_frame, clip.src_len, self.span, plot_width);
                if width <= 0.0 {
                    continue;
                }
                let bar = Rectangle {
                    x: plot_x + x,
                    y: y + 1.0,
                    width: width.max(1.0),
                    height: (lane_height - 2.0).max(1.0),
                };
                canvas_frame.fill_rectangle(bar.position(), bar.size(), TIMELINE_CLIP);
                canvas_frame.stroke_rectangle(
                    bar.position(),
                    bar.size(),
                    canvas::Stroke::default()
                        .with_color(TIMELINE_CLIP_EDGE)
                        .with_width(1.0),
                );
            }
        }

        // The playhead, over the lanes and clamped to the plot.
        let x = plot_x + frame_x(self.frame, self.span, plot_width);
        canvas_frame.fill_rectangle(
            Point::new(x - TIMELINE_PLAYHEAD_WIDTH / 2.0, TIMELINE_PAD),
            Size::new(
                TIMELINE_PLAYHEAD_WIDTH,
                (bounds.height - TIMELINE_PAD * 2.0).max(1.0),
            ),
            TIMELINE_PLAYHEAD,
        );

        vec![canvas_frame.into_geometry()]
    }
}

fn transport_label(snap: &Snapshot) -> &'static str {
    if snap.playing { "playing" } else { "stopped" }
}

/// The audio state line: the negotiated device, or the reason there is none. The
/// pump's last wiring error is appended when set, so a moving-but-silent playhead
/// is never mistaken for success.
fn audio_label(snap: &Snapshot) -> String {
    let base = match &snap.audio {
        None => "audio: silent host (no device requested)".to_string(),
        Some(a) => match &a.error {
            Some(e) => format!("audio: unavailable — {e}"),
            None => {
                let mismatch = if a.rate_mismatch {
                    format!(" (requested {} — MISMATCH)", a.requested_rate)
                } else {
                    String::new()
                };

                format!(
                    "audio: {} Hz · {} ch{mismatch} · underruns {} · drops {}",
                    a.sample_rate, a.channels, a.underruns, a.drops,
                )
            }
        },
    };

    match &snap.last_error {
        Some(e) => format!("{base} — pump error: {e}"),
        None => base,
    }
}

/// Record a short take through the host and report what landed.
fn record_check() -> i32 {
    let mut spike = Spike::from_host(HostHandle::spawn_with_audio());
    println!("record-check: pool {}", demo_pool_dir().display());
    println!(
        "record-check: pool sources before {:?}",
        spike.snap.pool_ids
    );

    // Start: the same key the window sends.
    spike.update(Message::Key(WorkflowKey::Char('o')));
    println!("record-check: start status {:?}", spike.status);
    // The marker, not the substring: the host refuses a second take with "a take
    // is already *recording*", so `contains("recording")` would read a refusal as
    // a start — the smell this slice removed from the toggle itself.
    if !spike.status.starts_with("● recording") {
        eprintln!("record-check: could not start a take (no input device?)");
        return 1;
    }

    // The capture is pumped while the transport runs, so a take is recorded during
    // playback — press play, as a person would.
    let _ = spike.host.execute(HostCommand::TransportPlay);
    std::thread::sleep(Duration::from_millis(200));

    // Capture for a beat, sampling the live indicator the window draws.
    for i in 1..=4 {
        std::thread::sleep(Duration::from_millis(250));
        spike.snap = spike.host.snapshot();
        println!("record-check: t+{}ms  {}", i * 250, take_label(&spike.snap));
    }

    // Stop: the same key again.
    spike.update(Message::Key(WorkflowKey::Char('o')));
    std::thread::sleep(Duration::from_millis(400));
    spike.snap = spike.host.snapshot();
    println!("record-check: stop status {:?}", spike.status);
    println!(
        "record-check: pool sources after  {:?}",
        spike.snap.pool_ids
    );
    match &spike.snap.last_take {
        Some(t) => {
            println!(
                "record-check: take {} — {} frames, {} ch, {} monitor drops, sources {:?}",
                t.take_id, t.frames, t.channels, t.monitor_dropped, t.sources
            );
            if t.frames == 0 {
                eprintln!("record-check: the take captured no frames");
                return 1;
            }
            // The **after-state**, not just the take: the list the next take is named
            // against must include this one's sources, or the auto-namer would reuse
            // the id it just recorded. This is the assertion the check was missing.
            let missing: Vec<&String> = t
                .sources
                .iter()
                .filter(|source| !spike.snap.pool_ids.contains(source))
                .collect();
            if !missing.is_empty() {
                eprintln!(
                    "record-check: the take's sources {missing:?} did not reach the published pool: {:?}",
                    spike.snap.pool_ids
                );
                return 1;
            }
            println!("record-check: OK — a take reached the pool");
            0
        }
        None => {
            eprintln!("record-check: no finished take was reported");
            1
        }
    }
}

/// Sample the output device's underrun counter across a play+record cycle.
///
/// The counter is in **device frames** (one per starved frame), so a single starved
/// callback adds its whole buffer length and a two-second burst reads as ~96 000.
/// The question is therefore the *shape*: a one-time jump at the stream transition
/// (cosmetic, and the known 2026-09-21 finding) versus a counter that keeps climbing
/// while the take runs (real starvation, and a bug in the record path). Each line
/// prints the counter and the delta from the previous sample, so the two are
/// distinguishable at a glance.
fn record_underrun() -> i32 {
    let mut spike = Spike::from_host(HostHandle::spawn_with_audio());
    let mut last: Option<u64> = None;

    sample_underruns(&mut spike, &mut last, "at open");
    std::thread::sleep(Duration::from_millis(700));
    sample_underruns(&mut spike, &mut last, "idle");

    let _ = spike.host.execute(HostCommand::TransportPlay);
    for i in 1..=4 {
        std::thread::sleep(Duration::from_millis(150));
        sample_underruns(&mut spike, &mut last, &format!("playing +{}ms", i * 150));
    }

    // Start the take the way the window does: the same key, through `update`.
    spike.update(Message::Key(WorkflowKey::Char('o')));
    println!("record-underrun: start status {:?}", spike.status);
    std::thread::sleep(Duration::from_millis(50));
    sample_underruns(&mut spike, &mut last, "record start");

    for i in 1..=8 {
        std::thread::sleep(Duration::from_millis(250));
        spike.snap = spike.host.snapshot();
        let rec = spike
            .snap
            .recording
            .as_ref()
            .map(|r| r.monitor_dropped)
            .unwrap_or(0);
        let u = spike.snap.audio.as_ref().map(|a| a.underruns);
        let delta = match (u, last) {
            (Some(b), Some(a)) => b.saturating_sub(a),
            _ => 0,
        };
        last = u;
        println!(
            "record-underrun: {:<28} underruns {u:?} (+{delta}) monitor-dropped {rec}",
            format!("rec +{:>4}ms", i * 250)
        );
    }

    spike.update(Message::Key(WorkflowKey::Char('o')));
    std::thread::sleep(Duration::from_millis(400));
    sample_underruns(&mut spike, &mut last, "after stop");
    println!("record-underrun: stop status {:?}", spike.status);
    0
}

/// One sample of the output device's underruns/drops, with the delta since the
/// previous sample, so a one-time burst and a climbing counter are told apart.
fn sample_underruns(spike: &mut Spike, last: &mut Option<u64>, label: &str) {
    spike.snap = spike.host.snapshot();
    let u = spike.snap.audio.as_ref().map(|a| a.underruns);
    let drops = spike.snap.audio.as_ref().map(|a| a.drops);
    let delta = match (u, *last) {
        (Some(b), Some(a)) => b.saturating_sub(a),
        _ => 0,
    };
    *last = u;
    println!("record-underrun: {label:<28} underruns {u:?} (+{delta}) drops {drops:?}");
}

/// Drive a fader through the **real** message path — the same `Message::Fader`
/// the slider emits on every frame it moves — and report the device's underruns
/// before and after.
///
/// This exists because a GUI drag cannot be scripted from outside and cannot be
/// piped in: the only way to answer "does dragging starve the audio device?" is
/// to make the shell do the dragging itself.
fn sweep() -> i32 {
    // With audio: the underrun counter is the device's, so a silent host measures nothing.
    let mut spike = Spike::from_host(HostHandle::spawn_with_audio());

    let underruns = |spike: &Spike| spike.snap.audio.as_ref().map(|a| a.underruns);
    let index = spike.channels.min(spike.faders.len().saturating_sub(1));

    // Phase 0 — STOPPED, and untouched: does the device starve on its own?
    let read = |spike: &mut Spike| {
        spike.snap = spike.host.snapshot();
        underruns(spike)
    };

    // Phase 0a — measure the idle RATE, not just the delta: one underrun per
    // source frame means the callback is running at 100 % starvation.
    {
        let a = read(&mut spike);
        let t = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(1000));
        let b = read(&mut spike);
        let secs = t.elapsed().as_secs_f64();
        if let (Some(a), Some(b)) = (a, b) {
            println!(
                "sweep: idle rate {:.0} underruns/s (48 kHz = total starvation; 0 = device paused)",
                (b - a) as f64 / secs
            );
        }
    }
    // The device opens during `spawn_with_audio`, so this is the counter at the
    // instant we can first see it.
    let t0 = std::time::Instant::now();
    let at_open = read(&mut spike);
    println!(
        "sweep: t+{}ms at first sight — underruns {at_open:?}",
        t0.elapsed().as_millis()
    );
    std::thread::sleep(Duration::from_millis(700));
    let stopped_a = read(&mut spike);
    println!(
        "sweep: t+{}ms STOPPED — underruns {stopped_a:?}",
        t0.elapsed().as_millis()
    );
    std::thread::sleep(Duration::from_millis(700));
    let stopped_b = read(&mut spike);
    println!(
        "sweep: t+{}ms STOPPED — underruns {stopped_b:?}",
        t0.elapsed().as_millis()
    );

    // The owner's order: the app is open and idle, a fader moves, then play.
    //
    // This **varies the pre-play delay** on purpose. Sampling a fixed moment twice
    // is what hid the idle-starve bug for several attempts: a reading taken before
    // the device thread opens the stream shows `0 -> 0`, which reads as a quiet
    // device when it is an unopened one. Scaling the delay makes the accrual
    // arithmetic. Before the pump kept an idle ring fed these three lines read
    // 15,360 / 59,392 / 116,736 — 48,000 per second of idle, one per frame; they
    // must read 0 now, and a non-zero value here is the regression.
    //
    // This block was dropped by the record slice's first commit and **restored**
    // after the merge gate caught it: the note that owns the idle-starve fix still
    // names this probe as the regression instrument, and a guard deleted in
    // silence is the shape that note exists to prevent.
    for delay_ms in [300u64, 1200, 2400] {
        let mut probe = Spike::from_host(HostHandle::spawn_with_audio());
        let read = |s: &mut Spike| {
            s.snap = s.host.snapshot();
            s.snap.audio.as_ref().map(|a| a.underruns)
        };
        let at_open = read(&mut probe);
        std::thread::sleep(Duration::from_millis(delay_ms));
        // "move the sliders"
        probe.update(Message::Fader(0, 0.42));
        probe.update(Message::Fader(1, 0.17));
        let before_play = read(&mut probe);
        let _ = probe.host.execute(HostCommand::TransportPlay);
        std::thread::sleep(Duration::from_millis(250));
        let after_play = read(&mut probe);
        println!(
            "sweep: idle {delay_ms:>4}ms -> play: open {at_open:?}, before play {before_play:?}, after play {after_play:?}"
        );
    }

    if let Err(e) = spike.host.execute(HostCommand::TransportPlay) {
        eprintln!("sweep: transport play refused: {e}");
        return 1;
    }

    // The shape of the burst: sample as fast as we can right after play.
    for i in 1..=10 {
        std::thread::sleep(Duration::from_millis(20));
        let u = read(&mut spike);
        println!("sweep: playing +{}ms — underruns {u:?}", i * 20);
    }
    // Settle: let the ring fill and the playing clock run.
    spike.snap = spike.host.snapshot();
    let before = underruns(&spike);
    println!(
        "sweep: settled — underruns {before:?}, playing {}",
        spike.snap.playing
    );

    // A drag, at about the rate a 60 Hz pointer produces: 300 messages over a
    // second, sweeping the fader's range and back.
    const STEPS: u32 = 300;
    let started = std::time::Instant::now();
    let mut sent = 0u32;
    for step in 0..STEPS {
        let phase = step as f32 / STEPS as f32;
        let normal = if step % 2 == 0 { phase } else { 1.0 - phase };
        spike.update(Message::Fader(index, normal));
        sent += 1;
        if step % 20 == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let elapsed = started.elapsed();

    std::thread::sleep(Duration::from_millis(300));
    spike.snap = spike.host.snapshot();
    let after = underruns(&spike);

    // What does the HOST settle on for each position? The endpoints alone would
    // mean the intermediate values never survive the round trip.
    println!("sweep: position -> the gain the host reports back");
    for i in 0..=10 {
        let normal = i as f32 / 10.0;
        spike.update(Message::Fader(index, normal));
        let outcome = spike.host.outcome().ok();
        let reported = outcome.as_ref().and_then(|o| {
            o.params
                .iter()
                .find(|(_, param, _)| *param == "master.gain")
                .map(|(_, _, v)| *v)
        });
        println!("  normal {normal:.1} -> host master.gain {reported:?}");
    }
    println!(
        "sweep: {sent} fader messages in {:?} ({:.0}/s) — underruns {after:?}",
        elapsed,
        sent as f32 / elapsed.as_secs_f32().max(1e-6),
    );
    match (before, after) {
        (Some(b), Some(a)) => {
            println!("sweep: delta {} underruns", a.saturating_sub(b));
            println!(
                "sweep: OK — audio: {} Hz, {} ch, drops {}",
                spike
                    .snap
                    .audio
                    .as_ref()
                    .map(|x| x.sample_rate)
                    .unwrap_or(0),
                spike.snap.audio.as_ref().map(|x| x.channels).unwrap_or(0),
                spike.snap.audio.as_ref().map(|x| x.drops).unwrap_or(0)
            );
            0
        }
        _ => {
            eprintln!("sweep: no audio device was opened — nothing to measure");
            1
        }
    }
}

/// The headless probe: the non-GUI half of the proof, runnable in a terminal (or
/// CI) with no display. It exercises exactly what the window exercises —
/// `HostHandle` boot, script load, transport, snapshot polling — and fails loudly
/// if no meter signal was ever observed.
fn probe() -> i32 {
    let script = match host::parse_script(DEMO_SCRIPT) {
        Ok(script) => script,
        Err(e) => {
            eprintln!("probe: demo script does not parse: {e}");
            return 2;
        }
    };

    // Silent host: the wall-clock pump, so the probe needs no audio device.
    let host = HostHandle::spawn();

    match host.load(&script) {
        Ok(outcome) => println!(
            "probe: loaded — mixer {:?} ch, {} log events, {} underruns, {} media commands",
            outcome.mixer_channels, outcome.event_count, outcome.underruns, outcome.media_commands,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A strip we cannot meter must read 0 rather than panicking.
    ///
    /// Regression: the console guarded on the **fold's** channel count but
    /// indexed the **snapshot's** meter vector. At startup the fold reports the
    /// mounted mixer width while `snap.channels` is still empty (no device yet),
    /// which panicked with "index out of bounds: the len is 0 but the index is 0".
    #[test]
    fn an_unmetered_strip_reads_zero_instead_of_panicking() {
        let mut spike = Spike::headless();
        spike.adopt();
        assert!(
            spike.channels > 0,
            "the headless fixture should mount a mixer"
        );

        // The mismatch that panicked: the fold says there are channels, the
        // published snapshot has no meter levels for them yet.
        spike.snap.channels.clear();
        // The assertion is that this returns rather than panicking.
        let _ = spike.console();
    }

    /// The one mapping the host's fold and the widgets must agree on: channels
    /// first, the master last — and nothing outside the mounted channels.
    #[test]
    fn a_gain_name_maps_to_its_strip() {
        assert_eq!(strip_of("ch0.gain", 4), Some(0));
        assert_eq!(strip_of("ch3.gain", 4), Some(3));
        assert_eq!(
            strip_of("master.gain", 4),
            Some(4),
            "the master is the last strip"
        );
        assert_eq!(
            strip_of("ch4.gain", 4),
            None,
            "past the mounted channel count"
        );
        assert_eq!(strip_of("ch0.mute", 4), None, "only gains have faders");
        assert_eq!(strip_of("channels", 4), None);

        assert_eq!(Spike::gain_param(0, 4), "ch0.gain");
        assert_eq!(Spike::gain_param(3, 4), "ch3.gain");
        assert_eq!(Spike::gain_param(4, 4), "master.gain");
    }

    /// The iced shell speaks the shared workflow's grid too: the same cycling
    /// order and the same labels, and it *says* that nothing snaps here yet
    /// (the editing gestures are the part that is missing, not the model or the
    /// canvas that now draws it).
    #[test]
    fn the_grid_cycles_and_says_what_it_cannot_do() {
        let mut spike = Spike::headless();
        assert!(!spike.grid.is_on());
        spike.dispatch(workflow::Action::GridCycle);
        assert_eq!(spike.grid.label(), "bar");
        assert!(spike.status.contains("bar"), "{}", spike.status);
        spike.dispatch(workflow::Action::GridCycle);
        assert_eq!(spike.grid.label(), "beat");
        assert!(
            spike.status.contains("not built yet"),
            "the shell must report the gap, not drop it: {}",
            spike.status
        );
    }

    /// The dB fader range is the inverse of itself, unity is 0 dB, and silence
    /// floors at the range minimum instead of going to -inf/NaN.
    #[test]
    fn the_fader_range_round_trips_in_db() {
        assert!(fader_range().unmap_to_db(fader_range().map_db(0.0)).abs() < 1e-4);

        for gain in [1.0f32, 0.5, 0.25, 2.0] {
            let db = 20.0 * gain.log10();
            let back = 10f32.powf(fader_range().unmap_to_db(fader_range().map_db(db)) / 20.0);
            assert!((back - gain).abs() < 0.01 * gain, "{gain} -> {back}");
        }

        assert_eq!(
            fader_range().unmap_to_db(fader_range().map_db(-120.0)),
            -60.0
        );
        assert_eq!(
            fader_range().unmap_to_db(fader_range().map_db(f32::NEG_INFINITY)),
            -60.0
        );
        // A value above the ceiling clamps to the ceiling (exact float identity
        // is not guaranteed through the skew, so compare with a tolerance).
        let clamped = fader_range().unmap_to_db(fader_range().map_db(24.0));
        assert!(
            (clamped - fader_max_db()).abs() < 1e-4,
            "24 dB clamped to {clamped:+.4}, not the ceiling {:.4}",
            fader_max_db()
        );
    }

    /// The translation layer is the one place this shell can drift from the shared
    /// workflow; the meanings themselves are the workflow's business.
    #[test]
    fn iced_keys_translate_into_the_workflow() {
        use keyboard::key::Named;
        let press = |key: keyboard::Key, modifiers: keyboard::Modifiers| {
            workflow::action(Spike::workflow_key(&key, modifiers).expect("a key"))
        };

        assert_eq!(
            press(
                keyboard::Key::Named(Named::Space),
                keyboard::Modifiers::default()
            ),
            Some(Action::PlayToggle)
        );
        assert_eq!(
            press(
                keyboard::Key::Character("j".into()),
                keyboard::Modifiers::default()
            ),
            Some(Action::Vertical(1))
        );
        assert_eq!(
            press(
                keyboard::Key::Character("K".into()),
                keyboard::Modifiers::SHIFT
            ),
            Some(Action::MoveTrack(-1))
        );
        assert_eq!(
            press(keyboard::Key::Named(Named::Tab), keyboard::Modifiers::SHIFT),
            Some(Action::CycleFocus(-1)),
            "Shift-Tab is the workflow's BackTab, whatever the toolkit calls it"
        );
        assert_eq!(
            press(
                keyboard::Key::Character("r".into()),
                keyboard::Modifiers::CTRL
            ),
            Some(Action::Redo)
        );
        assert_eq!(
            press(
                keyboard::Key::Character(":".into()),
                keyboard::Modifiers::default()
            ),
            Some(Action::Prompt)
        );
        assert_eq!(
            press(
                keyboard::Key::Named(Named::Escape),
                keyboard::Modifiers::default()
            ),
            Some(Action::Cancel)
        );
        assert_eq!(
            Spike::workflow_key(
                &keyboard::Key::Named(Named::F5),
                keyboard::Modifiers::default()
            ),
            None,
            "a key the workflow does not use is not an action"
        );
    }

    /// The same `host v1` line the TUI's command line runs: typed here, logged there.
    #[test]
    fn the_command_line_runs_host_lines() {
        let mut app = Spike::headless();
        let master = app.channels;

        app.on_key(WorkflowKey::Char(':'));
        assert_eq!(app.mode, Mode::Command, "the command line is a mode");
        for c in "set_param mixer master.gain 0.5".chars() {
            app.on_key(WorkflowKey::Char(c));
        }
        app.on_key(WorkflowKey::Enter);

        assert!(app.prompt.is_none(), "Enter closes the line");
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status.starts_with(": set_param"), "{}", app.status);
        let db = fader_range().unmap_to_db(app.faders[master].normal);
        assert!(
            (db - 20.0 * 0.5f32.log10()).abs() < 0.01,
            "the fader follows the log fold: {db} dB ({})",
            app.status
        );

        // Esc cancels, and a bad line reports without changing anything.
        app.on_key(WorkflowKey::Char(':'));
        app.on_key(WorkflowKey::Char('x'));
        app.on_key(WorkflowKey::Esc);
        assert!(app.prompt.is_none());
        assert!(app.status.contains("cancelled"), "{}", app.status);

        let before = app.faders[master].normal;
        app.on_key(WorkflowKey::Char(':'));
        for c in "arrange nonsense".chars() {
            app.on_key(WorkflowKey::Char(c));
        }
        app.on_key(WorkflowKey::Enter);
        assert!(
            app.status.contains("arrange"),
            "the error is shown: {}",
            app.status
        );
        assert_eq!(
            app.faders[master].normal, before,
            "a refused line changes nothing"
        );

        // History: `↑` recalls the last attempted line.
        app.on_key(WorkflowKey::Char(':'));
        app.on_key(WorkflowKey::Up);
        assert_eq!(app.prompt.as_deref(), Some("arrange nonsense"));
        app.on_key(WorkflowKey::Esc);
    }

    /// The workflow's keys work where iced has the surface, and **say so** where it
    /// does not — the parity debt is visible rather than silent.
    #[test]
    fn shared_actions_work_or_report_their_gap() {
        let mut app = Spike::headless();

        // Transport: space starts it. The live actor publishes its state on its own
        // thread, so the first read after the key can still be the previous snapshot
        // under load — wait for the state, do not race it (this test failed once in ~40
        // full-suite runs for exactly that reason).
        app.on_key(WorkflowKey::Space);
        let mut playing = false;
        for _ in 0..200 {
            if app.host.snapshot().playing {
                playing = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(playing, "space plays");

        // The console: `j` selects, `+` rides the selected fader, `0` returns it.
        assert_eq!(app.selected, 0);
        app.on_key(WorkflowKey::Char('j'));
        assert_eq!(app.selected, 1, "{}", app.status);
        let before = fader_range().unmap_to_db(app.faders[1].normal);
        app.on_key(WorkflowKey::Char('+'));
        let after = fader_range().unmap_to_db(app.faders[1].normal);
        assert!((after - (before + 0.5)).abs() < 0.01, "{before} -> {after}");
        app.on_key(WorkflowKey::Char('0'));
        assert!(
            fader_range().unmap_to_db(app.faders[1].normal).abs() < 0.01,
            "0 is unity"
        );

        // The timeline's actions are the workflow's, but not yet this shell's.
        app.on_key(WorkflowKey::Char('x'));
        assert!(
            app.status.contains("needs the timeline"),
            "an unwired action reports: {}",
            app.status
        );

        // The keymap overlay is the shared table, toggled by the shared key.
        app.on_key(WorkflowKey::Char('?'));
        assert!(app.help, "? opens the keymap");
        assert!(workflow::help_len() > 0);
        app.on_key(WorkflowKey::Esc);
        assert!(!app.help, "Esc closes it");
    }
}

#[cfg(test)]
mod fader_travel {
    use super::*;

    /// The fader spans the widget's whole range, so a full-height drag is the
    /// fader's whole travel.
    ///
    /// Regression: this was `iced_audio`'s *virtual* slider, whose drag was
    /// relative (`0.00385` of the range per pixel — a scroll rate) **and**
    /// inverted vertically, so dragging down moved the handle up and the control
    /// never reached its top. The widget is now iced's own absolute
    /// `vertical_slider`, whose range is exactly `0.0..=1.0` — the same space
    /// `Normal` lives in — so the two need no conversion.
    #[test]
    fn the_fader_range_is_the_sliders_whole_range() {
        for normal in [0.0f32, 0.25, 0.5, 0.833, 1.0] {
            let n = Normal::new(normal);
            let round_tripped = fader_range().map_db(fader_range().unmap_to_db(n)).as_f32();
            assert!(
                (round_tripped - normal).abs() < 1e-5,
                "normal {normal} round-trips to {round_tripped}"
            );
        }
        // The slider is fed `fader.normal.as_f32()` directly, so the range the
        // widget is given must be the unit interval.
        assert_eq!(fader_range().unmap_to_db(Normal::new(0.0)), FADER_FLOOR_DB);
        assert!((fader_range().unmap_to_db(Normal::new(1.0)) - fader_max_db()).abs() < 1e-4);
    }

    /// Every part of the throw has to be usable, including the top.
    ///
    /// Regression: a far-above-unity knee (the obvious "+24 dB for headroom")
    /// squeezed the useful top of the range into the last few pixels — the final
    /// 10 % of travel bought ~0.9 dB.
    #[test]
    fn the_top_of_the_throw_is_usable() {
        let knee = fader_range().unmap_to_db(Normal::new(1.0));

        // The top tenth must buy a usable amount, or the fader is useless there.
        let at_90 = fader_range().unmap_to_db(Normal::new(0.9));
        assert!(
            knee - at_90 >= 2.0,
            "the last 10 % of travel buys only {:.2} dB (from {at_90:+.2} to {knee:+.2})",
            knee - at_90
        );

        // Unity is where the *declared bounds* put it, not the 83 % console
        // convention: with a -60 dB floor and a +6.02 dB ceiling, 0 dB lands at
        // about 91 %. The test asserts the relationship, so a change to either
        // bound is a deliberate move rather than an accident in this file.
        let unity = fader_range().map_db(0.0).as_f32();
        assert!(
            (unity - fader_unity().as_f32()).abs() < 1e-6,
            "unity landed at {unity:.3} of the travel"
        );
        assert!(
            at_90 < unity && unity < 1.0,
            "unity must sit inside the top tenth: {at_90:.3} < {unity:.3} < 1.0"
        );
    }
}

#[cfg(test)]
mod fader_matches_the_declared_bounds {
    use super::*;

    /// The fader's top must be a gain the host accepts.
    ///
    /// Regression: the ceiling was hard-coded at +12 dB (gain 3.98) while the
    /// mixer declares gain `max: 2.0`, so the top of every fader asked for a
    /// value the engine refuses — "[0, 2] out of range" — and the fader jumped
    /// back to the unchanged real value.
    #[test]
    fn the_top_of_the_fader_is_a_value_the_host_accepts() {
        let ceiling = 10f32.powf(fader_max_db() / 20.0);
        for def in engine::plugins::MIXER_PARAMS
            .iter()
            .filter(|d| d.name.ends_with(".gain"))
        {
            assert!(
                ceiling <= def.max + 1e-4,
                "the fader's top is gain {ceiling:.4}, but '{}' declares max {}",
                def.name,
                def.max
            );
            assert!(
                ceiling >= def.min,
                "the fader's top is below '{}' min",
                def.name
            );
        }
        // And it is *at* the declared ceiling, not below it — a fader that stops
        // short cannot reach the gain the host is willing to give.
        let declared_max = engine::plugins::MIXER_PARAMS
            .iter()
            .filter(|d| d.name.ends_with(".gain"))
            .map(|d| d.max)
            .fold(f32::MIN, f32::max);
        assert!(
            (ceiling - declared_max).abs() < 1e-4,
            "ceiling {ceiling} vs declared {declared_max}"
        );
    }

    /// Unity sits where the bounds put it: 0 dB between a -60 dB floor and the
    /// declared ceiling.
    #[test]
    fn unity_follows_from_the_bounds() {
        let expected = (0.0 - FADER_FLOOR_DB) / (fader_max_db() - FADER_FLOOR_DB);
        assert!((fader_unity().as_f32() - expected).abs() < 1e-6);
        let db = fader_range().unmap_to_db(fader_unity());
        assert!(db.abs() < 1e-3, "unity reads {db:+.3} dB, not 0");
    }
}

#[cfg(test)]
mod fader_step {
    use super::*;

    /// The fader must have positions *between* its ends.
    ///
    /// Regression: iced's `VerticalSlider` defaults `step` to `T::from(1)`, which
    /// for `f32` is a step of 1.0. Its drag maths is
    /// `(percent * (end - start) / step).round()`, so over a `0.0..=1.0` range a
    /// default-stepped slider can only ever produce 0.0 or 1.0 — the fader jumped
    /// between `-60.0 dB` and `+6.0 dB` and nothing in between existed.
    #[test]
    fn the_step_leaves_usable_positions_between_the_ends() {
        // What the widget would compute, on the same maths it uses.
        let locate = |percent: f64, step: f32| -> f32 {
            let steps = (percent * (1.0 - 0.0) / step as f64).round();
            (steps * step as f64 + 0.0) as f32
        };

        // The crate's default collapses the whole throw to its ends.
        let default_step: f32 = 1.0;
        let distinct_default: std::collections::BTreeSet<u32> = (0..=100)
            .map(|i| (locate(i as f64 / 100.0, default_step) * 1e6) as u32)
            .collect();
        assert_eq!(
            distinct_default.len(),
            2,
            "the default step should collapse to two positions (this is the bug)"
        );

        // Ours must not.
        let distinct: std::collections::BTreeSet<u32> = (0..=100)
            .map(|i| (locate(i as f64 / 100.0, FADER_STEP) * 1e6) as u32)
            .collect();
        assert!(
            distinct.len() >= 100,
            "the fader offers only {} positions across the throw",
            distinct.len()
        );

        // And a mid-throw position must be a genuine intermediate gain.
        let mid = locate(0.5, FADER_STEP);
        let db = fader_range().unmap_to_db(Normal::new(mid));
        assert!(
            db > FADER_FLOOR_DB + 1.0 && db < fader_max_db() - 1.0,
            "mid-throw reads {db:+.1} dB, which is not between the ends"
        );
    }
}

#[cfg(test)]
mod record_mvp {
    use super::*;

    /// A new take must not reuse a name the pool already holds.
    ///
    /// The id is chosen *before* the recording starts (a take is declared state,
    /// so the session replays without the device), which is why it cannot be
    /// discovered afterwards — the shell names it against `pool_ids`.
    #[test]
    fn the_next_take_avoids_the_names_the_pool_holds() {
        let mut spike = Spike::headless();

        // An empty pool starts at one. (`next_take_id` reads the published pool,
        // not the filesystem, so a `--record-check` run's leftovers cannot reach it.)
        spike.snap.pool_ids.clear();
        assert_eq!(spike.next_take_id(), "take-1");

        // The first take written `take-1.ch0`/`.ch1` means the next is `take-2`.
        spike.snap.pool_ids = vec!["take-1.ch0".into(), "take-1.ch1".into()];
        assert_eq!(spike.next_take_id(), "take-2");

        // A gap is not reused either: the highest wins, so a pool holding 1 and 7
        // yields 8 rather than stepping on 1.
        spike.snap.pool_ids = vec!["take-1.ch0".into(), "take-7.ch0".into()];
        assert_eq!(spike.next_take_id(), "take-8");

        // Material that is not a take is ignored, not parsed as one — an imported
        // source must not shift the numbering.
        spike.snap.pool_ids = vec!["drums.ch0".into(), "note.txt".into(), "take-3.ch0".into()];
        assert_eq!(spike.next_take_id(), "take-4");

        // Duplicate ids are safe (the max is taken, not the count).
        spike.snap.pool_ids = vec!["take-2.ch0".into(), "take-2.ch0".into()];
        assert_eq!(spike.next_take_id(), "take-3");
    }

    /// The take line follows the published snapshot, so it is live: a recording in
    /// progress is shown with its counters, and a finished take reports its sources.
    #[test]
    fn the_take_line_reads_the_snapshot() {
        let mut spike = Spike::headless();

        spike.snap.recording = None;
        spike.snap.last_take = None;
        assert!(take_label(&spike.snap).contains("no take yet"));

        spike.snap.recording = Some(host::RecordingStatus {
            take_id: "take-2".into(),
            frames: 4800,
            monitor_dropped: 0,
            channels: 2,
            source: None,
        });
        let live = take_label(&spike.snap);
        assert!(live.contains("REC"), "{live}");
        assert!(live.contains("take-2"), "{live}");
        assert!(live.contains("4800"), "{live}");
        assert!(
            !live.contains("drops"),
            "a take nobody monitors says nothing about drops: {live}"
        );

        // Monitor loss is best-effort and named as such, never shown as "dropped".
        spike.snap.recording = Some(host::RecordingStatus {
            take_id: "take-2".into(),
            frames: 4800,
            monitor_dropped: 512,
            channels: 2,
            source: None,
        });
        let monitored = take_label(&spike.snap);
        assert!(
            monitored.contains("512 monitor drops"),
            "the counter is labelled as monitor loss: {monitored}"
        );

        // A finished take wins once recording is cleared, because the id is free.
        spike.snap.recording = None;
        spike.snap.last_take = Some(host::TakeReport {
            take_id: "take-2".into(),
            frames: 96_000,
            monitor_dropped: 0,
            channels: 2,
            source: None,
            sources: vec!["take-2.ch0".into(), "take-2.ch1".into()],
            sample_rate: 48_000,
            at_frame: 0,
        });
        let done = take_label(&spike.snap);
        assert!(done.contains("last take"), "{done}");
        assert!(done.contains("2 pool source(s)"), "{done}");
    }
}

#[cfg(test)]
mod timeline {
    use super::*;

    /// A clip on `t0`, for the fixtures below.
    fn clip(id: &str, at_frame: u64, src_len: u64) -> media::Clip {
        media::Clip {
            id: id.into(),
            name: None,
            source: "s.ch0".into(),
            src_start: 0,
            src_len,
            at_frame,
            fade_in: 0,
            fade_out: 0,
            gain: 1.0,
            loop_len: None,
            reversed: false,
        }
    }

    /// A snapshot carrying `tracks`, as the host publishes one (the `Arc` is the same
    /// shape `TimelineStatus` builds).
    fn snapshot_with(tracks: Vec<media::Track>) -> Snapshot {
        Snapshot {
            timeline: TimelineStatus {
                timeline: Some(std::sync::Arc::new(media::Timeline {
                    tracks,
                    markers: Vec::new(),
                })),
                error: None,
            },
            frame: 250,
            ..Snapshot::default()
        }
    }

    /// The clip mapping is the timeline's one piece of arithmetic: zero span, frame 0,
    /// a fractional placement, and a clamp past the end.
    #[test]
    fn the_clip_mapping_is_bounded() {
        // A zero/empty span draws at the left rather than dividing by zero, at any
        // canvas width (including a degenerate one).
        assert_eq!(frame_x(500, 0, 200.0), 0.0);
        assert_eq!(clip_span(0, 0, 0, 200.0), (0.0, 0.0));
        assert_eq!(clip_span(500, 500, 1_000, 0.0), (0.0, 0.0));

        // A clip at frame 0 starts at the left and spans its own length.
        assert_eq!(clip_span(0, 500, 1_000, 200.0), (0.0, 100.0));

        // A fractional mapping lands where the ratio says (250/1000 of 200 = 50).
        let (x, width) = clip_span(250, 250, 1_000, 200.0);
        assert!((x - 50.0).abs() < 1e-4, "x = {x}");
        assert!((width - 50.0).abs() < 1e-4, "width = {width}");

        // Past the span, both ends clamp to the right edge: never outside the box.
        assert_eq!(frame_x(5_000, 1_000, 200.0), 200.0);
        assert_eq!(clip_span(1_500, 500, 1_000, 200.0), (200.0, 0.0));
        // A clip that starts before the end and runs past it is truncated there.
        assert_eq!(clip_span(750, 500, 1_000, 200.0), (150.0, 50.0));
    }

    /// The span is the arrangement's end or the furthest frame played, whichever is
    /// greater — so a loaded piece is visible before it is played, and a take past the
    /// last clip still has room for the playhead.
    #[test]
    fn the_span_is_the_arrangement_end_or_the_furthest_frame_played() {
        let empty = Snapshot::default();
        assert_eq!(timeline_span(&empty.timeline, 0), 0);
        assert_eq!(timeline_span(&empty.timeline, 4_800), 4_800);

        let placed = snapshot_with(vec![media::Track {
            id: "t0".into(),
            clips: vec![clip("c0", 0, 96_000)],
        }]);
        assert_eq!(timeline_span(&placed.timeline, 0), 96_000);
        assert_eq!(
            timeline_span(&placed.timeline, 192_000),
            192_000,
            "a played frame past the arrangement still widens the span"
        );

        // A failed reconstruction is not an arrangement: the played span stands alone.
        let failed = TimelineStatus {
            timeline: None,
            error: Some("poisoned".into()),
        };
        assert_eq!(timeline_span(&failed, 1_000), 1_000);
    }

    /// The played-span memory grows forward and **resets** on a backward move, so a
    /// loaded or rewound piece is drawn against its own arrangement end again.
    #[test]
    fn the_span_memory_resets_when_the_transport_moves_backwards() {
        assert_eq!(advance_span(0, 0), 0);
        assert_eq!(advance_span(0, 500), 500);
        assert_eq!(advance_span(500, 900), 900);
        assert_eq!(advance_span(900, 400), 400, "a seek back starts a new pass");
        assert_eq!(advance_span(400, 0), 0, "a load starts at frame 0");
        assert_eq!(advance_span(0, 250), 250, "and grows again from there");
    }

    /// The canvas program is built from a snapshot with **no tracks** and from one with
    /// tracks, at the heights the shell derives. Neither may panic — the placeholder
    /// this replaced is gone, but an empty or unreadable arrangement is still a state
    /// the shell has to draw.
    #[test]
    fn the_canvas_program_builds_from_a_snapshot_with_and_without_tracks() {
        // No arrangement value, an empty arrangement, an arrangement with a track, and
        // a failed reconstruction: every shape the snapshot can publish.
        let shapes = [
            Snapshot::default(),
            snapshot_with(Vec::new()),
            snapshot_with(vec![
                media::Track {
                    id: "t0".into(),
                    clips: vec![clip("c0", 0, 48_000)],
                },
                media::Track {
                    id: "t1".into(),
                    clips: vec![clip("c1", 24_000, 48_000), clip("c2", 72_000, 0)],
                },
            ]),
            Snapshot {
                timeline: TimelineStatus {
                    timeline: None,
                    error: Some("the editor is poisoned".into()),
                },
                ..Snapshot::default()
            },
        ];

        let mut spike = Spike::headless();
        for snap in shapes {
            for tracks in [0usize, 1, 2, 8] {
                assert!(
                    timeline_height(tracks) >= TIMELINE_PAD * 2.0,
                    "the {tracks}-lane height collapses"
                );
            }
            spike.snap = snap;
            // Building the widget is the program construction the canvas does on every
            // redraw; there is no renderer here, so this is where "builds" is pinned.
            let _ = spike.timeline();
        }
    }
}

#[cfg(test)]
mod record_toggle {
    use super::*;

    /// `o` starts a take when none is running and stops the one that is.
    ///
    /// Device-gated rather than mocked: a take needs a real input device, so the
    /// stop half runs only once a start has succeeded. On a machine with no input
    /// (CI) it verifies the honest alternative instead — that the refusal is
    /// surfaced rather than swallowed — and says which branch it took, so a
    /// skipped assertion cannot read as a pass.
    #[test]
    fn the_toggle_starts_then_stops_a_take() {
        let mut spike = Spike::from_host(HostHandle::spawn_with_audio());
        assert_eq!(spike.snap.recording, None, "a fresh shell is not recording");

        spike.update(Message::Key(WorkflowKey::Char('o')));

        // The start is a command round trip, so read the host rather than the
        // repaint the shell has not had yet.
        let mut started = false;
        for _ in 0..2_000 {
            spike.snap = spike.host.snapshot();
            if spike.snap.recording.is_some() {
                started = true;
                break;
            }
            if spike.status.starts_with("command refused") {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        if !started {
            assert!(
                spike.status.starts_with("command refused"),
                "no take started and nothing was refused: {:?}",
                spike.status
            );
            assert!(
                spike.snap.recording.is_none(),
                "the snapshot claims a recording while the start was refused"
            );
            eprintln!(
                "record_toggle: no input device — verified the refusal path instead ({})",
                spike.status
            );
            return;
        }

        // It started: the status carries the marker (not just the substring — the
        // host's refusal says "a take is already recording"), and the same key stops it.
        assert!(
            spike.status.starts_with("● recording"),
            "a started take should say so: {:?}",
            spike.status
        );
        // Play, as a person would: the pump drives the capture, so a take with the
        // transport stopped holds at 0 frames.
        let _ = spike.host.execute(HostCommand::TransportPlay);
        // The expected name comes from the shell's own rule, read *before* the
        // press: pinning `take-1` here would pass once on a clean pool and fail on
        // the next run, which is a test that only works the first time.
        let expected = spike.next_take_id();
        let take_id = spike.snap.recording.clone().expect("started").take_id;
        assert_eq!(take_id, expected, "the take is named by `next_take_id`");

        spike.update(Message::Key(WorkflowKey::Char('o')));
        let mut finished = false;
        for _ in 0..3_000 {
            spike.snap = spike.host.snapshot();
            if spike.snap.recording.is_none() && spike.snap.last_take.is_some() {
                finished = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            finished,
            "the second `o` did not finish the take: {:?}",
            spike.status
        );
        let report = spike.snap.last_take.clone().expect("finished");
        assert_eq!(report.take_id, take_id, "the finished take keeps its id");

        // Scope item 3, asserted where it is drawn: the finished take reports pool
        // sources, and the take's channels are published as pool ids — which is the
        // list the *next* take is named against, so a take that is not published
        // would be overwritten by the next one.
        let line = take_label(&spike.snap);
        assert!(line.contains("last take"), "{line}");
        assert!(line.contains("pool source"), "{line}");
        for source in &report.sources {
            assert!(
                spike.snap.pool_ids.contains(source),
                "the finished take's source {source:?} did not reach the published pool: {:?}",
                spike.snap.pool_ids
            );
        }
        // And the next name steps over it rather than reusing it.
        assert_ne!(
            spike.next_take_id(),
            take_id,
            "the next take would reuse the id just recorded"
        );
        assert!(
            !spike.status.starts_with("command refused"),
            "{:?}",
            spike.status
        );
    }
}

#[cfg(test)]
mod command_result {
    use super::*;

    /// `command`'s `Result` is the answer; the status line is only where a refusal is
    /// **said**, and it is never cleared on success.
    ///
    /// Reading the status back for `command refused` is what made a successful record
    /// start on a lingering refusal read as a refusal. The `Result` cannot be confused
    /// that way, and this pins the contract the toggle now relies on.
    #[test]
    fn the_result_is_the_answer_not_the_status() {
        let mut spike = Spike::headless();

        // A refusal is both returned and said, so a caller may keep the host's words.
        assert!(spike.command(HostCommand::RecordStop).is_err());
        assert!(
            spike.status.starts_with("command refused"),
            "{:?}",
            spike.status
        );

        // A command the host applies returns `Ok` while an earlier refusal is still on
        // the status line: a caller testing the status would read success as refusal.
        let stale = spike.status.clone();
        assert!(spike.command(HostCommand::TransportStop).is_ok());
        assert_eq!(spike.status, stale);
    }
}
