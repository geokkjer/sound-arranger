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
use std::time::{Duration, Instant};

use host::HostCommand;
use host::live::{AudioStatus, HostHandle, Snapshot};

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
    ("j  k  ↑  ↓", "select the next / previous channel"),
    ("m", "toggle mouse capture"),
    ("?", "this keymap"),
    ("q  Esc  Ctrl+c", "quit"),
    (
        "mouse",
        "click Play/Stop/Rewind · click a meter row to select · wheel = seek",
    ),
];

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keys = args.iter().any(|arg| arg == "--keys");

    if args.iter().any(|arg| arg == "--probe") {
        std::process::exit(probe());
    }

    if args.iter().any(|arg| arg == "--dump") {
        return dump(keys);
    }

    run(keys)
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
    /// The selected mixer channel (the target for future edit keys).
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

        App {
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
        }
    }

    /// The deterministic frame `--dump` and the tests render: a synthetic
    /// snapshot with signal in it, so the output is reproducible and does not
    /// depend on an audio device or on timing.
    fn demo() -> Self {
        let mut app = App::idle();

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

        app.snap = snap;
        app.status = "demo loaded — mixer 4 ch, 8 log events, 0 underruns".to_string();
        app.selected = 1;
        app
    }

    /// A silent, idle host for the dump/tests: the rendered state comes from the
    /// synthetic snapshot, so the host only exists to satisfy the type (and to
    /// answer the transport commands the keymap tests exercise).
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
        let count = self.snap.channel_count as i64;
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
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('s') => self.stop(),
            KeyCode::Char('r') | KeyCode::Home => self.rewind(),
            KeyCode::Char(',') => self.nudge(-1),
            KeyCode::Char('.') => self.nudge(1),
            KeyCode::Char('j') | KeyCode::Down => self.select(1),
            KeyCode::Char('k') | KeyCode::Up => self.select(-1),
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Char('m') => self.toggle_mouse(),
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            _ => {}
        }
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

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self.action_at(position) {
                    self.apply(action);
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

        if self.help {
            self.draw_help(frame, area);
        }
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
        let block = Block::bordered().title(" meters ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        self.meter_rows.clear();

        let count = self.snap.channel_count;
        if count == 0 {
            frame.render_widget(
                Paragraph::new("no mixer mounted").style(Style::new().fg(Color::Gray)),
                inner,
            );
            return;
        }

        // The rows that fit: one per channel plus the master.
        let rows = (inner.height as usize).min(count + 1);
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
        if count + 1 > rows && inner.height > 0 {
            let hidden = count + 1 - rows;
            frame.render_widget(
                Paragraph::new(format!("… {hidden} more")).style(Style::new().fg(Color::DarkGray)),
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

        let paragraph = Paragraph::new(vec![
            format!("mouse: {}   {}", on_off(self.mouse), latency).into(),
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

fn run(keys: bool) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture);

    let mut app = App::boot();
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
/// overlay instead of the main view.
fn dump(keys: bool) -> io::Result<()> {
    let mut app = App::demo();
    app.help = keys;

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

/// `--probe`: the non-TUI half of the proof — host thread, transport, and live
/// meters, with no terminal at all.
fn probe() -> i32 {
    let script = match host::parse_script(DEMO_SCRIPT) {
        Ok(script) => script,
        Err(e) => {
            eprintln!("probe: demo script does not parse: {e}");
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
}
