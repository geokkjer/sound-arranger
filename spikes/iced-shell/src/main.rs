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
//! 5. **The mixer is real** — channel and master faders are `iced_audio`
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
//! cargo run -- --probe   # headless: host thread + snapshot polling, no window
//! ```

use std::time::Duration;

use host::HostCommand;
use host::live::{HostHandle, Snapshot};

use iced::keyboard;
use iced::widget::{column, container, progress_bar, row, text};
use iced::window;
use iced::{Center, Element, Fill, Length, Subscription, Theme};
use iced_audio::{DBRange, Gesture, Normal, NormalParam, VSlider};

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

    iced::application(Spike::boot, Spike::update, Spike::view)
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
}

#[derive(Debug, Clone)]
enum Message {
    /// A repaint tick — poll the host's published snapshot.
    Tick,
    Play,
    Stop,
    Rewind,
    /// Spacebar: start or stop, whichever the transport is not.
    Toggle,
    /// A fader gesture: `strip` is the channel (or the master, last).
    Fader(usize, Gesture),
}

impl Spike {
    /// Boot: open the live host (with audio when a device exists) and load the
    /// demo profile. A failure is surfaced in `status`, never a panic — a shell
    /// with no signal is a state, not a crash.
    fn boot() -> Self {
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

        let mut spike = Spike {
            snap: host.snapshot(),
            host,
            status,
            faders: vec![FADER.default_param(); 1],
            channels: 0,
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
            Message::Play => self.command(HostCommand::TransportPlay),
            Message::Stop => self.command(HostCommand::TransportStop),
            Message::Rewind => {
                // Seek is "rebuild + render to target", so it is intended for a
                // stopped transport: stop first, then move the playhead home.
                self.command(HostCommand::TransportStop);
                self.command(HostCommand::TransportSeek { frame: 0 });
            }
            Message::Toggle => {
                if self.snap.playing {
                    self.command(HostCommand::TransportStop);
                } else {
                    self.command(HostCommand::TransportPlay);
                }
            }
            Message::Fader(index, gesture) => self.gesture(index, gesture),
        }
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
                let keyboard::Event::KeyPressed { modified_key, .. } = event else {
                    return None;
                };

                match modified_key.as_ref() {
                    keyboard::Key::Named(keyboard::key::Named::Space) => Some(Message::Toggle),
                    keyboard::Key::Character("r") => Some(Message::Rewind),
                    _ => None,
                }
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

        let body = column![
            text("iced spike").size(22),
            text("a second shell over the same Host API — in-process, no webview").size(13),
            reading,
            controls,
            self.console(),
            text(audio_label(&self.snap)).size(12),
            text(&self.status).size(12),
        ]
        .spacing(16)
        .padding(20);

        container(body).width(Fill).height(Fill).padding(12).into()
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
}
