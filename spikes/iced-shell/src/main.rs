//! The iced spike — the minimal proof that **iced can be a second sound-arranger
//! shell**, replacing the Tauri + Vue transport with an in-process Rust one.
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
//! It is *not* a UI: there is no timeline canvas, no waveform, no clip editing,
//! no text-heavy layout. Those are the parts that decide iced vs Tauri+Vue, and
//! the note records them as the next step if this proof holds.
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
use iced::{Element, Fill, Length, Subscription, Theme};

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

        Spike {
            snap: host.snapshot(),
            host,
            status,
        }
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
            self.meters(),
            text(audio_label(&self.snap)).size(12),
            text(&self.status).size(12),
        ]
        .spacing(16)
        .padding(20);

        container(body).width(Fill).height(Fill).padding(12).into()
    }

    /// The channel meters plus the master, as vertical gauges. Cheap widgets on
    /// purpose: a bare `progress_bar` per channel — the spike is testing that
    /// live values reach the widgets at all, not the final mixer look.
    fn meters(&self) -> Element<'_, Message> {
        let count = self.snap.channel_count;

        let mut bars = row![].spacing(10).height(Length::Fixed(150.0));

        if count == 0 {
            bars = bars.push(text("no mixer mounted").size(12));
        }

        for channel in 0..count {
            bars = bars.push(
                column![
                    container(progress_bar(0.0..=1.0, self.snap.channels[channel]).vertical())
                        .width(Length::Fixed(28.0))
                        .height(Length::Fixed(130.0)),
                    text(format!("ch{channel}")).size(11),
                ]
                .spacing(4)
                .align_x(iced::Center),
            );
        }

        bars = bars.push(
            column![
                container(progress_bar(0.0..=1.0, self.snap.master).vertical())
                    .width(Length::Fixed(28.0))
                    .height(Length::Fixed(130.0)),
                text("master").size(11),
            ]
            .spacing(4)
            .align_x(iced::Center),
        );

        bars.into()
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
