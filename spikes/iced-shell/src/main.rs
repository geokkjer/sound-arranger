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
//!    [`host::live::HostHandle`] — the exact actor the Tauri bridge uses — and
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
//! It is *not* a UI: there is no timeline canvas, no waveform, no clip editing,
//! no text-heavy layout. Those are the parts that decide iced against ratatui
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
use host::live::{HostHandle, Snapshot};

use iced::keyboard;
use iced::widget::{column, container, progress_bar, row, text};
use iced::window;
use iced::{Center, Element, Fill, Length, Subscription, Theme};
use iced_audio::{DBRange, Gesture, Normal, NormalParam, VSlider};
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

/// The console fader range: -60 dB (silence) to +12 dB, with unity at ~83 % of
/// the travel — the console convention. `DBRange` is logarithmic and skewed
/// towards 0 dB, so the useful part of the throw is not squeezed into the top
/// millimetre, and `unmap_to_db` is the exact inverse of `map_db`.
const FADER: DBRange = DBRange::new(
    -60.0,
    12.0,
    Normal::new(0.833),
    DBRange::DEFAULT_SKEW_FACTOR,
);

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
    Fader(usize, Gesture),
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

        let mut spike = Spike {
            snap: host.snapshot(),
            host,
            status,
            faders: vec![FADER.default_param(); 1],
            channels: 0,
            selected: 0,
            mode: Mode::Normal,
            prompt: None,
            history: Vec::new(),
            history_at: 0,
            help: false,
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
            self.faders = vec![FADER.default_param(); channels + 1];
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
            fader.set(FADER.map_db(db));
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
    fn gesture(&mut self, index: usize, gesture: Gesture) {
        let Gesture::Gesturing(normal) = gesture else {
            // Start/end carry no value; re-read the host so a value the log
            // disagrees with (a clamp, say) wins.
            if matches!(gesture, Gesture::GestureEnd) {
                self.adopt();
            }
            return;
        };

        if let Some(fader) = self.faders.get_mut(index) {
            fader.set(normal);
        }
        let param = Spike::gain_param(index, self.channels);
        let db = FADER.unmap_to_db(normal);
        let gain = 10f32.powf(db / 20.0);
        self.status = format!("{param} = {db:+.1} dB ({gain:.4})");
        self.command(HostCommand::SetParam {
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
    /// here; the Vue side's token contract has no iced equivalent yet.
    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Tick => self.snap = self.host.snapshot(),
            Message::Play => self.play(),
            Message::Stop => self.command(HostCommand::TransportStop),
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
            Action::Stop => self.command(HostCommand::TransportStop),
            Action::Rewind => self.rewind(),
            Action::SeekSeconds(seconds) => self.seek_seconds(seconds),
            Action::Vertical(direction) => self.select_strip(direction),
            Action::Zoom(direction) => self.ride(direction),
            Action::Fit => self.set_selected_fader(1.0),
            Action::Undo => self.command(HostCommand::Undo),
            Action::Redo => self.command(HostCommand::Redo),
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
            other => {
                self.status = format!(
                    "`{}` needs the timeline — the workflow's keys are shared, but the iced \
                     timeline canvas is not built yet",
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
            self.command(HostCommand::TransportStop);
        } else {
            self.play();
        }
    }

    fn play(&mut self) {
        self.command(HostCommand::TransportPlay);
    }

    fn rewind(&mut self) {
        // Seek is "rebuild + render to target", so it is intended for a stopped
        // transport: stop first, then move the playhead home.
        self.command(HostCommand::TransportStop);
        self.command(HostCommand::TransportSeek { frame: 0 });
    }

    /// `,`/`.`: ±1 s, stopping first (a seek is rebuilt to the target frame).
    fn seek_seconds(&mut self, seconds: i64) {
        let rate = self
            .snap
            .audio
            .as_ref()
            .map_or(SAMPLE_RATE, |a| a.sample_rate as i64);
        let target = (self.snap.frame as i64 + seconds * rate).max(0) as u64;
        self.command(HostCommand::TransportStop);
        self.command(HostCommand::TransportSeek { frame: target });
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
        let current = FADER.unmap_to_db(self.faders[index].normal);
        self.set_selected_fader(10f32.powf((current + 0.5 * direction as f32) / 20.0));
    }

    /// Move the selected fader to `gain` and log it — the same `set_param` the TUI
    /// sends, through the same fold.
    fn set_selected_fader(&mut self, gain: f32) {
        let index = self.selected;
        if index >= self.faders.len() {
            return;
        }
        self.gesture(
            index,
            Gesture::Gesturing(FADER.map_db(if gain > 0.0 {
                20.0 * gain.log10()
            } else {
                f32::NEG_INFINITY
            })),
        );
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
            text("space = play/stop · r = rewind").size(12),
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
            let (label, meter) = if index == self.channels {
                ("master".to_string(), self.snap.master)
            } else {
                (format!("ch{index}"), self.snap.channels[index])
            };
            let db = FADER.unmap_to_db(fader.normal);

            strips = strips.push(
                column![
                    row![
                        container(progress_bar(0.0..=1.0, meter).vertical())
                            .width(Length::Fixed(16.0))
                            .height(Length::Fixed(150.0)),
                        VSlider::new(*fader)
                            .on_gesture(move |gesture| Message::Fader(index, gesture))
                            .width(Length::Fixed(18.0))
                            .height(Length::Fixed(150.0)),
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

    fn command(&mut self, command: HostCommand) {
        if let Err(e) = self.host.execute(command) {
            self.status = format!("command refused: {e}");
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

    /// The dB fader range is the inverse of itself, unity is 0 dB, and silence
    /// floors at the range minimum instead of going to -inf/NaN.
    #[test]
    fn the_fader_range_round_trips_in_db() {
        assert!(FADER.unmap_to_db(FADER.map_db(0.0)).abs() < 1e-4);

        for gain in [1.0f32, 0.5, 0.25, 2.0] {
            let db = 20.0 * gain.log10();
            let back = 10f32.powf(FADER.unmap_to_db(FADER.map_db(db)) / 20.0);
            assert!((back - gain).abs() < 0.01 * gain, "{gain} -> {back}");
        }

        assert_eq!(FADER.unmap_to_db(FADER.map_db(-120.0)), -60.0);
        assert_eq!(FADER.unmap_to_db(FADER.map_db(f32::NEG_INFINITY)), -60.0);
        assert_eq!(FADER.unmap_to_db(FADER.map_db(24.0)), 12.0);
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
        let db = FADER.unmap_to_db(app.faders[master].normal);
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

        // Transport: space starts it (the actor publishes before the reply).
        app.on_key(WorkflowKey::Space);
        assert!(app.host.snapshot().playing, "space plays");

        // The console: `j` selects, `+` rides the selected fader, `0` returns it.
        assert_eq!(app.selected, 0);
        app.on_key(WorkflowKey::Char('j'));
        assert_eq!(app.selected, 1, "{}", app.status);
        let before = FADER.unmap_to_db(app.faders[1].normal);
        app.on_key(WorkflowKey::Char('+'));
        let after = FADER.unmap_to_db(app.faders[1].normal);
        assert!((after - (before + 0.5)).abs() < 0.01, "{before} -> {after}");
        app.on_key(WorkflowKey::Char('0'));
        assert!(
            FADER.unmap_to_db(app.faders[1].normal).abs() < 0.01,
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
