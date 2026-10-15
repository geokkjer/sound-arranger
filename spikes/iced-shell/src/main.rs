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
//! 8. **It draws the material, and it has a viewport.** Each clip carries the source's
//!    real envelope, read from the pool's persisted `.peaks` pyramid
//!    ([`PeakFile::load`]) through the pool directory the snapshot publishes — a
//!    level-aware query, so a column costs one or two bins at any zoom rather than
//!    walking the source. The viewport zooms around the playhead, scrolls, fits, and
//!    seeks by clicking the ruler (`+ - 0 h l`, `Tab` to focus it; the same keys and the
//!    same meanings as the TUI's timeline). `--wave <file>` imports a piece into the
//!    spike's own session pool and places it, so there is something real to draw;
//!    `--perf` measures the frame budget while it plays.
//!
//! It is *not* a UI yet: clip editing (split, trim, move), the pool panel, and
//! text-heavy layout are still missing. Those are the parts that decide iced against
//! ratatui
//! ([the two candidate shells](../../.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md)),
//! and the evaluations record them as the next step.
//!
//! ```sh
//! cd spikes/iced-shell
//! cargo run              # the window (needs a display + an audio device)
//! cargo run -- --keys    # …with the shared keymap overlay open
//! cargo run -- --probe   # headless: host thread + snapshot polling, no window
//! cargo run -- --wave <file.wav>   # import a piece into the session pool and draw it
//! cargo run -- --wave <file.wav> --perf   # …and measure the frame budget while it plays
//! cargo test             # the key translation, the command line, the console keys
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host::HostCommand;
use host::live::{HostHandle, Snapshot, TimelineStatus};

use iced::keyboard;
use iced::mouse;
use iced::widget::canvas;
use iced::widget::{column, container, progress_bar, row, text, vertical_slider};
use iced::window;
use iced::{
    Center, Color, Element, Fill, Length, Point, Rectangle, Renderer, Size, Subscription, Theme,
};
use iced_audio::{DBRange, Normal, NormalParam};
use media::peaks::{PEAK_BASE_BIN, PeakFile, Peaks};
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keys = args.iter().any(|arg| arg == "--keys");

    // `--wave <file>` imports a copy into the spike's own session pool at the session
    // rate and places it — the piece the timeline draws. Without it the demo profile
    // has no clips, so the canvas is a lane bed and there is nothing to measure.
    let wave = args
        .windows(2)
        .find(|pair| pair[0] == "--wave")
        .map(|pair| PathBuf::from(&pair[1]));
    if args.iter().any(|arg| arg == "--wave") && wave.is_none() {
        eprintln!("--wave needs a path: --wave <file.wav>");
        std::process::exit(2);
    }
    // `--perf` measures the redraw cadence while the transport plays and exits with the
    // numbers. A GUI cannot be scripted from outside, so the shell measures itself —
    // the same shape as `--sweep` and `--record-check`, and why it needs a display.
    let perf = args.iter().any(|arg| arg == "--perf");

    let boot_wave = wave.clone();
    iced::application(
        move || Spike::boot(keys, boot_wave.clone(), perf),
        Spike::update,
        Spike::view,
    )
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
    /// The **declared source** the next take will capture, when one is armed (`A`
    /// cycles `default → the rig's sources → default`). `None` is the default input.
    /// Shell state, like the take id: the host binds the name at `record` time and the
    /// take report carries it back.
    record_source: Option<String>,
    /// Which panel `+ - 0 h l j k` act on. The workflow's own split: the key is shared,
    /// the meaning is the focus's ([`Action::Zoom`]/[`Action::Fit`] are "zoom / fit" or
    /// "ride / unity"), so the timeline's viewport is reachable without growing a key
    /// the table does not have.
    focus: Focus,
    /// The timeline's viewport, **shared with the canvas program** (an `Arc` clone) so
    /// the draw can fit it to the width only the draw knows, and the keys read back the
    /// width the last draw measured. Behind a `Mutex` because the canvas program is
    /// rebuilt each redraw and cannot borrow the shell.
    viewport: Arc<Mutex<Viewport>>,
    /// The active track the timeline's focus moves (`j`/`k` when it has focus), so the
    /// lane under the playhead has a home in the shell rather than in the draw.
    active_track: usize,
    /// The sources' persisted pyramids, loaded lazily from the pool's `.peaks` sidecars
    /// and cached — a drawing must not read a file per frame.
    peaks: HashMap<String, SourcePeaks>,
    /// The pool directory `peaks` was loaded against. A rebind changes it, and the old
    /// pool's material is not this pool's.
    peaks_pool: Option<PathBuf>,
    /// `--perf`: sample the redraw cadence while the transport plays, then print and exit.
    /// `None` for a normal run.
    perf: Option<PerfRun>,
    /// Envelope columns drawn in the last redraw, shared with the canvas program (the
    /// draw is the only place that knows). `--perf` reports it beside the frame interval,
    /// so the shell's own work is attributed rather than assumed.
    perf_columns: Arc<AtomicUsize>,
}

/// The focused panel — the shell's half of the workflow's per-panel keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Console,
    Timeline,
}

/// One source's pyramid, or why there is none: a clip whose material cannot be drawn is
/// a state the shell shows, not a hole it draws silence through.
enum SourcePeaks {
    Loaded(Arc<Peaks>),
    Missing(String),
}

/// `--perf` state: when the measurement started, and the redraw intervals seen.
struct PerfRun {
    started: std::time::Instant,
    intervals: Vec<f32>,
    last: Option<std::time::Instant>,
    /// Envelope columns drawn in the last frame — the shell's own per-frame work, which
    /// is the half of the budget this change controls.
    columns: usize,
}

/// How long `--perf` measures before it prints and exits.
const PERF_WINDOW: Duration = Duration::from_secs(6);
/// Redraws `--perf` does not count as steady state. The first frames carry the window and
/// shader (or software-renderer) warm-up, and a one-second stall there is a startup cost,
/// not a frame-budget failure — it is reported separately rather than averaged away or
/// hidden.
const PERF_WARMUP_FRAMES: usize = 30;
/// An interval longer than this is a **stall**, not a slow frame, and is reported by value:
/// a percentile would hide it, and a stall is what decides whether a loop is usable.
const PERF_STALL_MS: f32 = 100.0;

impl PerfRun {
    /// One redraw arrived: note the interval since the previous one, and stop when the
    /// window is up. A GUI cannot be scripted from outside, so the shell prints its own
    /// numbers — the same reason `--sweep` drives its own fader.
    fn sample(&mut self, columns: usize, summary: &str) {
        let now = std::time::Instant::now();
        if let Some(last) = self.last {
            self.intervals.push((now - last).as_secs_f32() * 1000.0);
        }
        self.last = Some(now);
        self.columns = columns;
        if now.duration_since(self.started) < PERF_WINDOW {
            return;
        }
        let elapsed = now.duration_since(self.started).as_secs_f32();
        let steady: Vec<f32> = self
            .intervals
            .iter()
            .skip(PERF_WARMUP_FRAMES)
            .copied()
            .collect();
        let worst_start = self
            .intervals
            .iter()
            .take(PERF_WARMUP_FRAMES)
            .copied()
            .fold(0.0f32, f32::max);
        println!(
            "perf: {} redraws in {elapsed:.2} s (the transport ran throughout)",
            self.intervals.len(),
        );
        if !steady.is_empty() {
            let mut sorted = steady.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let pct = |p: f64| -> f32 {
                let index = ((sorted.len() - 1) as f64 * p).round() as usize;
                sorted[index]
            };
            println!(
                "perf: steady p50 {:.2} ms ({:.1} fps), p95 {:.2} ms, max {:.2} ms over {} \
                 frames (skipping the first {PERF_WARMUP_FRAMES})",
                pct(0.50),
                1000.0 / pct(0.50).max(0.001),
                pct(0.95),
                pct(1.0),
                sorted.len(),
            );
            // A percentile hides a stall, and a stall is the thing that decides whether a
            // loop is usable — so the outliers are named, with their values.
            let stalls: Vec<f32> = self
                .intervals
                .iter()
                .skip(PERF_WARMUP_FRAMES)
                .copied()
                .filter(|ms| *ms > PERF_STALL_MS)
                .collect();
            println!(
                "perf: stalls over {PERF_STALL_MS:.0} ms after warm-up: {} {}",
                stalls.len(),
                stalls
                    .iter()
                    .map(|ms| format!("{ms:.0} ms"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        println!(
            "perf: startup — longest of the first {PERF_WARMUP_FRAMES} intervals {worst_start:.2} ms \
             (window + shader warm-up, not a frame-budget failure)"
        );
        println!(
            "perf: envelope columns in the last redraw: {} (one per point per visible clip)",
            self.columns
        );
        println!("perf: what the shell held — {summary}");
        println!(
            "perf: a frame is a snapshot poll + host publish + this canvas; the shell's own \
             work is the envelope query"
        );
        println!(
            "perf: a gap between redraws is not always the shell — a window the compositor is \
             not showing receives no frame callbacks, and that reads as a stall here. Compare a \
             run with 0 envelope columns (the demo profile) to tell the two apart."
        );
        std::process::exit(0);
    }
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

/// How many consecutive clips `--wave` cuts each imported source into: enough clips,
/// tracks and non-zero `src_start` offsets that the canvas draws several real envelopes
/// out of one pyramid — the shape the peaks mapping and the viewport are exercised by.
const WAVE_CLIPS_PER_SOURCE: u64 = 3;

/// Import `path` into the spike's pool at the session rate, and build the `host v1`
/// script that places it: one track per channel, each channel cut into
/// [`WAVE_CLIPS_PER_SOURCE`] **consecutive** clips.
///
/// Consecutive matters: the clips play exactly like the file, while giving the canvas
/// several clips whose `src_start` is not zero — which is the case a peaks mapping gets
/// wrong if it ignores the region offset. (The TUI's `--wave` places one clip per
/// channel; a spike that measures a *timeline* needs more than one clip to measure.)
fn wave_script(path: &Path) -> Result<(String, String), String> {
    let dir = demo_pool_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("pool dir {}: {e}", dir.display()))?;
    // `host v1` is whitespace-separated, so a path with whitespace cannot be named in it.
    // Renaming is the honest fix; quoting soup is not (the same guard the TUI makes).
    if dir.to_string_lossy().split_whitespace().count() != 1 {
        return Err(format!(
            "the pool path '{}' contains whitespace — set the workspace somewhere simple",
            dir.display()
        ));
    }

    let pool = media::Pool::open(&dir)?;
    let imported = pool.import(path, SAMPLE_RATE as u32)?;
    if imported.frames_out() == 0 {
        return Err(format!("{} has no audio frames", path.display()));
    }
    let sources = &imported.sources;
    if sources.is_empty() {
        return Err(format!("{} imported no sources", path.display()));
    }

    let mut script = String::from("host v1\n");
    // The load carries the pool, so the piece can resolve its own sources — and
    // `load` replaces the session wholesale, so a pool bound before it would be gone.
    script.push_str(&format!("pool {}\n", dir.display()));
    script.push_str(&format!(
        "mount mixer channels={} @0\n",
        sources.len().max(2)
    ));
    if sources.len() == 2 {
        // One source per channel, hard-panned: the pair plays as stereo instead of
        // collapsing the right channel into the centre.
        script.push_str("set_param mixer ch0.pan -1 @0\n");
        script.push_str("set_param mixer ch1.pan 1 @0\n");
    }

    let mut clips = 0u64;
    for (track, source) in sources.iter().enumerate() {
        script.push_str(&format!("arrange add_track t{track}\n"));
        for part in 0..WAVE_CLIPS_PER_SOURCE {
            let start = source.frames_out * part / WAVE_CLIPS_PER_SOURCE;
            let end = source.frames_out * (part + 1) / WAVE_CLIPS_PER_SOURCE;
            if end <= start {
                continue;
            }
            // Clip ids are global to the arrangement, not per track: a counter, so the
            // second track's first clip is not `c0` again (the host refuses a duplicate).
            script.push_str(&format!(
                "arrange add_clip t{track} c{clips} {} {start} {} {start} 0 0 1.0\n",
                source.id,
                end - start,
            ));
            clips += 1;
        }
    }

    let note = format!(
        "wave — {} imported, {clips} clip(s) on {} track(s){}",
        imported.id,
        sources.len(),
        if imported.converted() {
            " (resampled to the session rate)"
        } else {
            ""
        }
    );
    Ok((script, note))
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
    /// A click on the timeline: seek to this frame, through the same command a key uses.
    SeekTo(u64),
}

impl Spike {
    /// Boot: open the live host (with audio when a device exists) and load the
    /// demo profile. A failure is surfaced in `status`, never a panic — a shell
    /// with no signal is a state, not a crash.
    fn boot(keys: bool, wave: Option<PathBuf>, perf: bool) -> Self {
        let mut spike = Self::from_host(HostHandle::spawn_with_audio());
        spike.help = keys;
        if let Some(path) = wave {
            spike.open_wave(&path);
        }
        if perf {
            spike.perf = Some(PerfRun {
                started: std::time::Instant::now(),
                intervals: Vec::new(),
                last: None,
                columns: 0,
            });
            // The measurement is a play-through: start here so the owner sees the piece
            // move while it is measured, and so the frame budget includes the transport
            // running (the criterion is "holds frame rate while the transport runs").
            spike.play();
        }
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
            record_source: None,
            focus: Focus::Console,
            viewport: Arc::new(Mutex::new(Viewport::default())),
            active_track: 0,
            peaks: HashMap::new(),
            peaks_pool: None,
            perf: None,
            perf_columns: Arc::new(AtomicUsize::new(0)),
        };
        spike.adopt();
        spike
    }

    /// `--wave <file>`: import a copy into the spike's **session pool** at the session
    /// rate and load the piece it makes, so the timeline draws real pool material.
    ///
    /// The import is a copy on purpose: a pool is session-owned working material, and
    /// pointing the pool at the user's directory would let the pool pass rewrite their
    /// file ([`media::Pool::import`]). A failure is a status line, never a panic — a
    /// spike with no piece still has a window.
    fn open_wave(&mut self, path: &Path) {
        let (script, note) = match wave_script(path) {
            Ok(built) => built,
            Err(e) => {
                self.status = format!("--wave: {e}");
                return;
            }
        };
        let script = match host::parse_script(&script) {
            Ok(script) => script,
            Err(e) => {
                self.status = format!("--wave: the script it built does not parse: {e}");
                return;
            }
        };
        match self.host.load(&script) {
            Ok(_) => {
                self.snap = self.host.snapshot();
                // The pool the script bound is the one the peaks come from; a new pool
                // drops the old material's pyramids.
                self.peaks.clear();
                self.peaks_pool = None;
                // A load starts at frame 0: the viewport fits the new piece rather than
                // keeping the previous one's zoom.
                self.span_frames = 0;
                if let Ok(mut view) = self.viewport.lock() {
                    *view = Viewport::default();
                }
                self.adopt();
                self.status = note;
            }
            Err(e) => self.status = format!("--wave: load refused: {e}"),
        }
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
                // The pyramids the arrangement now needs, read once each (the map is the
                // cache) — a poll tick is the only place a file is opened.
                self.adopt_peaks();
                if self.perf.is_some() {
                    // The canvas counts what it drew; the shell reports it *and* resets, so
                    // the number is this frame's work rather than every frame's sum.
                    let columns = self.perf_columns.swap(0, Ordering::Relaxed);
                    let (clips, tracks) = self
                        .snap
                        .timeline
                        .timeline
                        .as_ref()
                        .map(|timeline| {
                            (
                                timeline
                                    .tracks
                                    .iter()
                                    .map(|track| track.clips.len())
                                    .sum::<usize>(),
                                timeline.tracks.len(),
                            )
                        })
                        .unwrap_or((0, 0));
                    let loaded = self
                        .peaks
                        .values()
                        .filter(|entry| matches!(entry, SourcePeaks::Loaded(_)))
                        .count();
                    let missing = self.peaks.len() - loaded;
                    let summary = format!(
                        "{tracks} track(s), {clips} clip(s), {loaded} source(s) drawn, \
                         {missing} without peaks; last status: {}",
                        self.status
                    );
                    if let Some(perf) = self.perf.as_mut() {
                        perf.sample(columns, &summary);
                    }
                }
            }
            Message::Play => self.play(),
            Message::Stop => {
                let _ = self.command(HostCommand::TransportStop);
            }
            Message::Rewind => self.rewind(),
            Message::Key(key) => self.on_key(key),
            Message::Fader(index, gesture) => self.gesture(index, gesture),
            Message::SeekTo(frame) => {
                // A click is a decision about the timeline, so it takes the focus the way
                // clicking a panel does in the TUI.
                self.focus = Focus::Timeline;
                self.seek_frame(frame);
            }
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
            Action::CycleRecordSource => self.cycle_record_source(),
            Action::Stop => {
                let _ = self.command(HostCommand::TransportStop);
            }
            Action::Rewind => self.rewind(),
            Action::SeekSeconds(seconds) => self.seek_seconds(seconds),
            Action::Vertical(direction) => self.vertical(direction),
            Action::Timeline(direction) => self.timeline_scroll(direction),
            Action::Zoom(direction) => self.zoom_or_ride(direction),
            Action::Fit => self.fit_or_unity(),
            Action::CycleFocus(direction) => self.cycle_focus(direction),
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

    /// `j`/`k`: the **focused panel** moves — the console's selected strip (channels
    /// first, master last), or the timeline's active track. The key is the workflow's; the
    /// meaning is the focus's, exactly as [`Action::Zoom`] and [`Action::Fit`] are.
    fn vertical(&mut self, direction: i32) {
        match self.focus {
            Focus::Console => self.select_strip(direction),
            Focus::Timeline => self.select_track(direction),
        }
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

    /// `j`/`k` with the timeline focused: move the active track.
    fn select_track(&mut self, direction: i32) {
        let tracks = self
            .snap
            .timeline
            .timeline
            .as_ref()
            .map_or(0, |timeline| timeline.tracks.len());
        if tracks == 0 {
            self.status = "the arrangement has no tracks".to_string();
            return;
        }
        self.active_track =
            (self.active_track as i32 + direction).clamp(0, tracks as i32 - 1) as usize;
        let id = self
            .snap
            .timeline
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.tracks.get(self.active_track))
            .map(|track| track.id.clone())
            .unwrap_or_default();
        self.status = format!("active track {id}");
    }

    /// `Tab`/`Shift-Tab`: the focused panel. Two panels, so a direction wraps the same
    /// either way — the workflow's [`Action::CycleFocus`] carries a direction for shells
    /// with more.
    fn cycle_focus(&mut self, _direction: i32) {
        self.focus = match self.focus {
            Focus::Console => Focus::Timeline,
            Focus::Timeline => Focus::Console,
        };
        self.status = match self.focus {
            Focus::Console => "focus: console (`+ -` ride the selected fader)".to_string(),
            Focus::Timeline => {
                "focus: timeline (`+ -` zoom, `h l` scroll, `0` fit, click to seek)".to_string()
            }
        };
    }

    /// The content length every timeline mapping clamps to: the arrangement's end, or the
    /// furthest frame played (see [`timeline_span`]).
    fn content_frames(&self) -> u64 {
        timeline_span(&self.snap.timeline, self.span_frames)
    }

    /// The plot width the last draw measured, for a key that has to zoom before the next
    /// draw happens. Before the first draw there is no measurement, and
    /// [`DEFAULT_PLOT_WIDTH`] stands in until the draw fits the view properly.
    fn plot_width(&self) -> f32 {
        match self.viewport.lock() {
            Ok(view) => view.width.unwrap_or(DEFAULT_PLOT_WIDTH),
            Err(_) => DEFAULT_PLOT_WIDTH,
        }
    }

    /// `+`/`-`: the focused panel's "more / less" — zoom the timeline, or ride the
    /// selected fader. The same split the TUI makes; the workflow's own doc says so.
    fn zoom_or_ride(&mut self, direction: i32) {
        match self.focus {
            Focus::Console => self.ride(direction),
            Focus::Timeline => {
                let (total, width) = (self.content_frames(), self.plot_width());
                if total == 0 {
                    self.status = "nothing to zoom — the arrangement is empty".to_string();
                    return;
                }
                // Anchor on the playhead, kept inside the content: zooming holds what you
                // are looking at rather than jumping to the start.
                let anchor = self.snap.frame.min(total);
                if let Ok(mut view) = self.viewport.lock() {
                    view.zoom(total, width, anchor, direction > 0);
                    self.status = format!(
                        "timeline zoom: {:.1} ms/point, view {:.2}–{:.2} s",
                        view.ms_per_point(self.snap.sample_rate),
                        view.start as f64 / f64::from(self.snap.sample_rate.max(1)),
                        (view.start + view.visible(width)) as f64
                            / f64::from(self.snap.sample_rate.max(1)),
                    );
                }
            }
        }
    }

    /// `0`: the focused panel's neutral default — fit the timeline, or the selected fader
    /// to unity.
    fn fit_or_unity(&mut self) {
        match self.focus {
            Focus::Console => self.set_selected_fader(1.0),
            Focus::Timeline => {
                let (total, width) = (self.content_frames(), self.plot_width());
                if let Ok(mut view) = self.viewport.lock() {
                    view.fit(total, width);
                }
                self.status = "timeline: fit to the piece".to_string();
            }
        }
    }

    /// `h`/`l`: scroll the timeline by an eighth of the screen. With the console focused
    /// there is nothing to scroll, and the shell says so rather than doing nothing.
    fn timeline_scroll(&mut self, direction: i64) {
        if self.focus != Focus::Timeline {
            self.status = "`h`/`l` scroll the timeline — `Tab` focuses it".to_string();
            return;
        }
        let (total, width) = (self.content_frames(), self.plot_width());
        if let Ok(mut view) = self.viewport.lock() {
            view.scroll(total, width, direction);
            self.status = format!(
                "timeline: view {:.2}–{:.2} s",
                view.start as f64 / f64::from(self.snap.sample_rate.max(1)),
                (view.start + view.visible(width)) as f64 / f64::from(self.snap.sample_rate.max(1)),
            );
        }
    }

    /// Seek to `frame` — the same `TransportSeek` a key sends, snapped to the workflow's
    /// grid when one is armed (a click that lands 3 ms off the beat is a click that lands
    /// 3 ms off the beat, and the grid is what the shell already has to fix it).
    fn seek_frame(&mut self, frame: u64) {
        let total = self.content_frames();
        let raw = frame.min(total);
        let target = self.snap_frame(raw);
        let _ = self.command(HostCommand::TransportStop);
        let _ = self.command(HostCommand::TransportSeek { frame: target });
        self.status = if target == raw {
            format!("seek to frame {target}")
        } else {
            format!("seek to frame {target} (snapped from {raw})")
        };
    }

    /// Round `frame` to the armed grid division, in the **beat** domain the snapshot's
    /// tempo map owns — the TUI's `snap_frame`, from the same published map.
    fn snap_frame(&self, frame: u64) -> u64 {
        let map = &self.snap.tempo_map;
        let Some(grid) = self.grid.grid(map.meter_at(frame)) else {
            return frame;
        };
        map.frame_at(grid.nearest(map.beat_at(frame)))
    }

    /// Load any pyramid the arrangement needs and the cache does not hold.
    ///
    /// Called on the poll tick, so a source is read **once** — the map is the cache, and a
    /// failure is cached too (a missing sidecar must not become a file read per frame). A
    /// **rebind** (`set_pool` through the command line, or a script's `pool` line) changes
    /// the directory the whole cache belongs to, so it is dropped rather than mixed.
    fn adopt_peaks(&mut self) {
        if self.snap.pool_dir != self.peaks_pool {
            self.peaks.clear();
            self.peaks_pool = self.snap.pool_dir.clone();
        }
        let wanted: Vec<String> = self
            .snap
            .timeline
            .timeline
            .as_ref()
            .map(|timeline| {
                timeline
                    .tracks
                    .iter()
                    .flat_map(|track| track.clips.iter().map(|clip| clip.source.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // Nothing new to read on almost every frame — and then this is a poll tick that
        // opens no file and stats no directory at all.
        if wanted.iter().all(|id| self.peaks.contains_key(id)) {
            return;
        }
        let Some(dir) = self.snap.pool_dir.clone() else {
            return;
        };
        let Ok(pool) = media::Pool::open(&dir) else {
            return;
        };
        for id in wanted {
            if self.peaks.contains_key(&id) {
                continue;
            }
            // The pool's own guarded lookup, never a join of the id onto the directory: a
            // clip's source is a token in the log, so it can carry `../`.
            let entry = match pool.path_for(&id) {
                Some(wav) => match PeakFile::load(&PeakFile::sidecar_path(&wav)) {
                    Ok(peaks) => SourcePeaks::Loaded(Arc::new(peaks)),
                    Err(e) => SourcePeaks::Missing(e),
                },
                None => SourcePeaks::Missing(format!("no source '{id}' in the pool")),
            };
            // A source that cannot be drawn says so **once** (the cache is the guard: the
            // next frame finds the entry) instead of silently drawing a plain bar that
            // reads as silence.
            if let SourcePeaks::Missing(why) = &entry {
                self.status = format!("timeline: no peaks for '{id}' — {why}");
            }
            self.peaks.insert(id, entry);
        }
    }

    /// `+`/`-`: ride the selected fader (the workflow's `Zoom`, which the focused
    /// panel reads as zoom or as a fader move).
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
            text(format!(
                "space = play/stop · r = rewind · o = record/stop · A = source: {}",
                record_source_label(self.record_source.as_deref())
            ))
            .size(12),
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
        // The take captures the **armed** source (`A` cycles it); `None` is the default
        // input, exactly as before the binding slice.
        let source = self.record_source.clone();
        if self
            .command(HostCommand::Record {
                take_id: take_id.clone(),
                source: source.clone(),
            })
            .is_ok()
        {
            self.status = match &source {
                Some(name) => format!("● recording {take_id} from '{name}' — `o` stops it"),
                None => format!("● recording {take_id} — `o` stops it"),
            };
        }
    }

    /// `A`: arm the source the next take captures — `default input → the rig's first
    /// declared source → … → default`.
    ///
    /// The armed name is **shell state** (like the take id): the host publishes the
    /// declarations (`snap.sources`) and binds the name only at `record` time. A rig
    /// with nothing declared cycles to `default` and says so, rather than arming a
    /// name that could never bind.
    fn cycle_record_source(&mut self) {
        self.snap = self.host.snapshot();
        let names = &self.snap.sources;
        self.record_source = match &self.record_source {
            None => names.first().cloned(),
            Some(current) => {
                let idx = names.iter().position(|n| n == current);
                match idx {
                    Some(i) if i + 1 < names.len() => Some(names[i + 1].clone()),
                    // The last declared source (or a name no longer declared) cycles
                    // back to the default input.
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
    /// rectangle with the source's **real envelope** inside it (read from the pool's
    /// `.peaks` pyramid through the directory the snapshot publishes), and the playhead at
    /// `snap.frame`. The content length is the arrangement's end or the furthest frame
    /// played, whichever is greater, so a loaded piece is visible before it is played and
    /// the view never rescales under a stopped transport; the **viewport** maps it.
    ///
    /// An `error` on the status is drawn **as the error**, never as an empty timeline:
    /// the host can fail to reconstruct a poisoned arrangement, and a shell that drew
    /// nothing would be saying "the piece is empty" about a piece it could not read.
    fn timeline(&self) -> Element<'_, Message> {
        let status = &self.snap.timeline;
        let total = timeline_span(status, self.span_frames);
        let clips = status
            .timeline
            .as_ref()
            .map_or(0, |t| t.tracks.iter().map(|track| track.clips.len()).sum());
        let (view_start, view_end, density) = match self.viewport.lock() {
            Ok(view) => {
                let width = view.width.unwrap_or(DEFAULT_PLOT_WIDTH);
                (
                    view.start,
                    (view.start + view.visible(width)).min(total),
                    view.ms_per_point(self.snap.sample_rate),
                )
            }
            Err(_) => (0, total, 0.0),
        };
        let missing = self
            .peaks
            .values()
            .filter(|entry| matches!(entry, SourcePeaks::Missing(_)))
            .count();
        let mut caption = format!(
            "{:.2} s / {:.2} s · {} clip(s) · {:.1} ms/pt · view {:.2}–{:.2} s",
            self.snap.seconds,
            total as f64 / f64::from(self.snap.sample_rate.max(1)),
            clips,
            density,
            view_start as f64 / f64::from(self.snap.sample_rate.max(1)),
            view_end as f64 / f64::from(self.snap.sample_rate.max(1)),
        );
        if missing > 0 {
            caption.push_str(&format!(" · {missing} source(s) without peaks"));
        }
        if self.focus == Focus::Timeline {
            caption.push_str("  [focused: + - zoom, h l scroll, 0 fit]");
        } else {
            caption.push_str("  [Tab focuses the timeline]");
        }
        let tracks = status.timeline.as_ref().map_or(0, |t| t.tracks.len());
        let canvas: Element<'_, Message> = canvas::Canvas::new(TimelineView {
            status: status.clone(),
            frame: self.snap.frame,
            total,
            active_track: self.active_track,
            focused: self.focus == Focus::Timeline,
            viewport: Arc::clone(&self.viewport),
            peaks: self
                .peaks
                .iter()
                .filter_map(|(id, entry)| match entry {
                    SourcePeaks::Loaded(peaks) => Some((id.clone(), Arc::clone(peaks))),
                    SourcePeaks::Missing(_) => None,
                })
                .collect(),
            columns: Arc::clone(&self.perf_columns),
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

/// What the footer says the next take will capture: the armed declared source, or
/// the machine's default input.
fn record_source_label(armed: Option<&str>) -> String {
    match armed {
        Some(name) => name.to_string(),
        None => "default".to_string(),
    }
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

/// The timeline's **viewport**, in points — the iced shell's unit.
///
/// The TUI's timeline has the same laws in terminal cells
/// ([`spikes/tui-shell/src/timeline.rs`](../../tui-shell/src/timeline.rs)): zoom by two
/// around an anchor, scroll by a fraction of the screen, clamp inside the content, and
/// never finer than the peak pyramid can honestly serve. The maths is per-shell **by
/// design**, not by accident: the workflow crate leaves geometry to the shell ("a shell
/// owns the playhead, the active track, and the text it builds"), and a point is not a
/// cell. What the two shells share is the *key*, and what it means — `+ - 0 h l` on the
/// focused panel.
///
/// The pyramid's floor is the real limit: below one base bin per point the envelope would
/// repeat a bin and claim detail the data does not have, so zooming in stops there
/// ([`MIN_FRAMES_PER_POINT`]).
#[derive(Debug, Clone, Copy)]
struct Viewport {
    /// The first visible frame.
    start: u64,
    /// Frames per point — `f64` because zoom is a ratio, and a fit rarely lands on one.
    frames_per_point: f64,
    /// The plot width the view was last fitted to, if it has been drawn. The draw knows
    /// the width; the keys read it back, which is why the shell and the canvas share one
    /// viewport.
    width: Option<f32>,
}

/// The finest zoom the peak pyramid can honestly serve: one base bin per point
/// ([`PEAK_BASE_BIN`] = 256 frames ≈ 5.3 ms/point at 48 kHz). Below it a column would
/// draw the *same* bin repeatedly and claim detail that is not in the sidecar.
const MIN_FRAMES_PER_POINT: f64 = PEAK_BASE_BIN as f64;
/// How far *out* of the content the view may zoom: zooming out past the whole piece is
/// allowed (a DAW shows margin around it), up to this multiple of the fitted density.
const ZOOM_OUT_MARGIN: f64 = 4.0;
/// A scroll key moves an eighth of the screen — small enough to be a nudge, large enough
/// to be a move.
const SCROLL_FRACTION: f64 = 8.0;

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            start: 0,
            frames_per_point: MIN_FRAMES_PER_POINT,
            width: None,
        }
    }
}

impl Viewport {
    /// Frames visible across `width` points — zero for a plot with no width, so a
    /// degenerate draw cannot claim to show content.
    fn visible(&self, width: f32) -> u64 {
        if width <= 0.0 {
            return 0;
        }
        (self.frames_per_point * f64::from(width)).round() as u64
    }

    /// The density that shows `total` frames across `width` points.
    fn fitted_density(total: u64, width: f32) -> f64 {
        (total as f64 / f64::from(width.max(1.0))).max(MIN_FRAMES_PER_POINT)
    }

    /// Show the whole piece.
    fn fit(&mut self, total: u64, width: f32) {
        self.frames_per_point = Self::fitted_density(total, width);
        self.start = 0;
        self.width = Some(width.max(1.0));
    }

    /// The first draw knows the real plot width, and so does every draw after a resize; fit
    /// once whenever the width differs from the one the view was fitted to, so "fit" (and the
    /// density readout) are honest rather than measured against an assumed width. A resize
    /// therefore refits — the TUI's rule, and the alternative is holding a zoom against a
    /// width it was not chosen for.
    fn fit_if_needed(&mut self, total: u64, width: f32) {
        let width = width.max(1.0);
        if self.width != Some(width) {
            self.fit(total, width);
        }
    }

    /// The coarsest zoom: the fitted density times [`ZOOM_OUT_MARGIN`].
    fn max_frames_per_point(total: u64, width: f32) -> f64 {
        Self::fitted_density(total, width) * ZOOM_OUT_MARGIN
    }

    /// Zoom by a factor of two around `anchor` (a frame), which stays at the same
    /// fraction of the screen — the TUI's rule, and what makes zooming feel anchored
    /// rather than jumpy.
    fn zoom(&mut self, total: u64, width: f32, anchor: u64, in_: bool) {
        let width = width.max(1.0);
        let before = self.frames_per_point;
        let next = if in_ { before / 2.0 } else { before * 2.0 };
        let per = next.clamp(
            MIN_FRAMES_PER_POINT,
            Self::max_frames_per_point(total, width),
        );
        if (per - before).abs() < f64::EPSILON {
            return;
        }
        self.frames_per_point = per;
        let anchor = anchor.min(total);
        let offset = ((anchor - self.start.min(anchor)) as f64) * per / before;
        self.start = anchor.saturating_sub(offset.min(self.visible(width) as f64) as u64);
        self.width = Some(width);
        self.clamp(total, width);
    }

    /// Scroll by a fraction of the screen.
    fn scroll(&mut self, total: u64, width: f32, direction: i64) {
        let width = width.max(1.0);
        let delta = (self.frames_per_point * f64::from(width) / SCROLL_FRACTION) * direction as f64;
        self.start = (self.start as f64 + delta).max(0.0) as u64;
        self.width = Some(width);
        self.clamp(total, width);
    }

    /// Keep the viewport inside the content: a short piece still starts at 0, and the
    /// view never runs off the right end (the fitted view is exactly the whole piece).
    fn clamp(&mut self, total: u64, width: f32) {
        let visible = self.visible(width);
        let limit = total.saturating_sub(visible.min(total));
        self.start = self.start.min(limit);
    }

    /// The frame under plot point `x` (x measured **inside** the plot, after the label
    /// gutter). Clamped to `total`, so a click past the end lands on the end.
    fn frame_at(&self, x: f32, total: u64) -> u64 {
        let frame = self.start as f64 + f64::from(x.max(0.0)) * self.frames_per_point;
        (frame.max(0.0) as u64).min(total)
    }

    /// The x of `frame` inside the plot — [`frame_at`](Self::frame_at)'s inverse, and the
    /// only mapping a clip or the playhead is drawn with.
    fn x_at(&self, frame: u64) -> f32 {
        ((frame.saturating_sub(self.start)) as f64 / self.frames_per_point) as f32
    }

    /// Milliseconds per point — the density number the evaluation asks for, shown live
    /// rather than argued about.
    fn ms_per_point(&self, sample_rate: u32) -> f64 {
        self.frames_per_point / f64::from(sample_rate.max(1)) * 1000.0
    }
}

/// The plot width assumed before the first draw has measured the real one — only used to
/// render a caption (the draw itself always fits to the width it is given).
const DEFAULT_PLOT_WIDTH: f32 = 600.0;

/// Up to two contiguous source-frame ranges: what one column of one clip reads.
///
/// Two, because a column shorter than a loop can cross exactly one loop boundary; a column
/// covering a whole loop or more reads the loop once. A fixed array keeps the mapping
/// allocation-free in a draw path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceRanges {
    ranges: [(u64, u64); 2],
    count: usize,
}

impl SourceRanges {
    fn empty() -> Self {
        SourceRanges {
            ranges: [(0, 0); 2],
            count: 0,
        }
    }

    fn one(range: (u64, u64)) -> Self {
        SourceRanges {
            ranges: [range, (0, 0)],
            count: 1,
        }
    }

    fn push(&mut self, range: (u64, u64)) {
        if self.count < 2 && range.0 < range.1 {
            self.ranges[self.count] = range;
            self.count += 1;
        }
    }

    fn iter(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.ranges[..self.count].iter().copied()
    }

    fn count(&self) -> usize {
        self.count
    }
}

/// The **source** frame ranges a clip's timeline range `[from, to)` reads, honouring the
/// clip's region offset, a baked loop and a reversed direction.
///
/// This is the peaks mapping's whole subtlety, and the place a waveform is drawn wrong if
/// it is not: a clip whose region does not start at 0 must draw the pyramid from
/// `src_start`, not from 0, or every clip of a cut-up take draws the same first seconds.
/// A reversed region reads its frames backwards — the min/max envelope of a set does not
/// care about order, so the reversal is a *range* mirror, not a mirrored drawing. (The
/// model refuses a clip that is both looped and reversed — "loop phase is not
/// representable" — so the two never have to compose.)
fn clip_source_ranges(clip: &media::Clip, from: u64, to: u64) -> SourceRanges {
    let start = from.max(clip.at_frame);
    let end = to.min(clip.at_frame.saturating_add(clip.src_len));
    if clip.src_len == 0 || end <= start {
        return SourceRanges::empty();
    }
    let loop_len = match clip.loop_len {
        Some(len) if len > 0 => len,
        _ => clip.src_len,
    };
    let (o0, o1) = (start - clip.at_frame, end - clip.at_frame);
    // Offsets map to source frames; `src_len - 1 - offset` is the reversed read.
    let map = |s: u64, e: u64| -> (u64, u64) {
        if clip.reversed {
            (
                clip.src_start + clip.src_len.saturating_sub(e),
                clip.src_start + clip.src_len.saturating_sub(s),
            )
        } else {
            (clip.src_start + s, clip.src_start + e)
        }
    };
    // A column covering a whole loop or more reads the loop once: the repeats are the same
    // material, and normalising over them would measure what a listener hears once.
    if o1 - o0 >= loop_len {
        return SourceRanges::one(map(0, loop_len));
    }
    let mut out = SourceRanges::empty();
    let mut offset = o0;
    // Fewer than `loop_len` offsets span at most two loop segments, so two ranges suffice.
    while offset < o1 && out.count() < 2 {
        let segment_end = ((offset / loop_len) + 1) * loop_len;
        let segment_end = segment_end.min(o1);
        out.push(map(offset % loop_len, (segment_end - 1) % loop_len + 1));
        offset = segment_end;
    }
    out
}

/// The envelope columns a clip's visible span maps to: for each one-point column, the
/// `(x, min, max)` the pyramid holds for the **source frames under that column**.
///
/// The frame range comes from the viewport, not from the clip's width, which is what makes
/// the envelope the same shape at every zoom — and the pyramid level is chosen from the
/// same number, so a column costs one or two bins rather than `frames_per_point` frames.
/// A column the data does not cover is skipped, never faked.
fn envelope_columns(
    clip: &media::Clip,
    peaks: &Peaks,
    view: &Viewport,
    total: u64,
    from_x: f32,
    to_x: f32,
) -> Vec<(f32, f32, f32)> {
    let level = peaks.level_for(view.frames_per_point.round().max(1.0) as u64);
    let mut columns = Vec::new();
    let mut x = from_x.max(0.0).floor();
    while x < to_x {
        let f0 = view.frame_at(x, total);
        let f1 = view.frame_at(x + 1.0, total).max(f0 + 1);
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for (start, end) in clip_source_ranges(clip, f0, f1).iter() {
            if let Some((min, max)) = peaks.range_minmax_at(level, start, end) {
                lo = lo.min(min);
                hi = hi.max(max);
            }
        }
        if lo <= hi {
            columns.push((x, lo, hi));
        }
        x += 1.0;
    }
    columns
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
/// A clip whose source has no readable peaks: a *different* colour, so "not drawn" never
/// reads as "silence".
const TIMELINE_CLIP_MUTED: Color = Color::from_rgb(0.24, 0.28, 0.34);
/// The source envelope inside a clip — lighter than the body so the shape reads.
const TIMELINE_ENVELOPE: Color = Color::from_rgb(0.82, 0.93, 1.0);
const TIMELINE_CLIP_EDGE: Color = Color::from_rgb(0.55, 0.78, 1.0);
const TIMELINE_PLAYHEAD: Color = Color::from_rgb(1.0, 0.55, 0.35);
const TIMELINE_LABEL: Color = Color::from_rgb(0.75, 0.78, 0.85);
/// The active track's id, when the timeline has the focus.
const TIMELINE_ACTIVE_LABEL: Color = Color::from_rgb(1.0, 0.85, 0.55);
/// The muted colour of "there is nothing to draw, and why".
const TIMELINE_MUTED: Color = Color::from_rgb(0.62, 0.48, 0.48);

/// The timeline canvas program.
///
/// It owns its data — a cloned [`TimelineStatus`] (an `Arc` clone, not the piece), the
/// playhead frame, the content length, a pyramid per source (an `Arc` clone each) and a
/// handle on the shell's viewport — so the canvas needs no borrow of the shell and is
/// rebuilt from each snapshot. It is deliberately **not** cached: an edit, an undo or a
/// load can change the arrangement under it, and a cached geometry would keep drawing the
/// piece the shell no longer holds. (The *pyramids* are cached, in the shell, because
/// reading a sidecar per frame is I/O; the geometry is not, because it is cheap and the
/// arrangement can change on any frame.)
struct TimelineView {
    status: TimelineStatus,
    /// The transport's frame, for the playhead.
    frame: u64,
    /// The content length every mapping clamps to (see [`timeline_span`]).
    total: u64,
    /// The track the timeline's focus is on (`j`/`k`), highlighted in the gutter.
    active_track: usize,
    /// Whether the timeline is the focused panel. The canvas is told rather than left to
    /// infer it: a gutter label must not look active on its own.
    focused: bool,
    /// The shell's viewport, shared so the draw can fit it to the width only the draw
    /// knows, and the keys can read that width back.
    viewport: Arc<Mutex<Viewport>>,
    /// The loaded pyramids, by pool source id. A clip whose source is absent is drawn as a
    /// muted bar — never as silence.
    peaks: HashMap<String, Arc<Peaks>>,
    /// Envelope columns drawn this redraw, for `--perf`.
    columns: Arc<AtomicUsize>,
}

impl TimelineView {
    /// The plot inside `bounds`: `(x, width)`, right of the track-id gutter.
    fn plot(bounds: Rectangle) -> (f32, f32) {
        let x = TIMELINE_LABEL_WIDTH;
        (x, (bounds.width - x - TIMELINE_PAD).max(1.0))
    }

    /// The viewport as this draw sees it, fitted to the plot width the first time (and
    /// after a resize). A poisoned lock is not a reason to lose the drawing: it falls back
    /// to a fitted view that just is not shared.
    fn view(&self, plot_width: f32) -> Viewport {
        match self.viewport.lock() {
            Ok(mut view) => {
                view.fit_if_needed(self.total, plot_width);
                *view
            }
            Err(_) => {
                let mut view = Viewport::default();
                view.fit(self.total, plot_width);
                view
            }
        }
    }

    /// Whether the timeline is the focused panel — the canvas is told, because a gutter
    /// label must not pretend to be active on its own.
    fn active(&self) -> bool {
        self.focused
    }
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

        let (plot_x, plot_width) = Self::plot(bounds);
        // The viewport is fitted here, where the real width is known, and read back by the
        // keys afterwards — the one place the shell and the canvas share state.
        let view = self.view(plot_width);
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
                color: if index == self.active_track && self.active() {
                    TIMELINE_ACTIVE_LABEL
                } else {
                    TIMELINE_LABEL
                },
                size: 11.0.into(),
                align_y: iced::alignment::Vertical::Center,
                ..canvas::Text::default()
            });
            for clip in &track.clips {
                let x0 = (plot_x + view.x_at(clip.at_frame)).max(plot_x);
                let x1 = (plot_x + view.x_at(clip.at_frame.saturating_add(clip.src_len)))
                    .min(plot_x + plot_width);
                if x1 <= x0 {
                    continue;
                }
                let bar = Rectangle {
                    x: x0,
                    y: y + 1.0,
                    width: (x1 - x0).max(1.0),
                    height: (lane_height - 2.0).max(1.0),
                };
                let pyramid = self.peaks.get(&clip.source);
                // The clip's body: the source's envelope when it is loaded, a muted fill
                // when it is not, so "no peaks" is a different *colour* rather than a
                // silence drawn as if it were material.
                canvas_frame.fill_rectangle(
                    bar.position(),
                    bar.size(),
                    if pyramid.is_some() {
                        TIMELINE_CLIP
                    } else {
                        TIMELINE_CLIP_MUTED
                    },
                );
                if let Some(peaks) = pyramid {
                    let mid = bar.y + bar.height / 2.0;
                    let half = (bar.height / 2.0) - 1.0;
                    let columns =
                        envelope_columns(clip, peaks, &view, self.total, x0 - plot_x, x1 - plot_x);
                    for (column, lo, hi) in &columns {
                        let top = mid - hi.clamp(-1.0, 1.0) * half;
                        let bottom = mid - lo.clamp(-1.0, 1.0) * half;
                        canvas_frame.fill_rectangle(
                            Point::new(plot_x + column, top),
                            Size::new(1.0, (bottom - top).max(1.0)),
                            TIMELINE_ENVELOPE,
                        );
                    }
                    self.columns.fetch_add(columns.len(), Ordering::Relaxed);
                }
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
        let x = plot_x + view.x_at(self.frame);
        if x >= plot_x && x <= plot_x + plot_width {
            canvas_frame.fill_rectangle(
                Point::new(x - TIMELINE_PLAYHEAD_WIDTH / 2.0, TIMELINE_PAD),
                Size::new(
                    TIMELINE_PLAYHEAD_WIDTH,
                    (bounds.height - TIMELINE_PAD * 2.0).max(1.0),
                ),
                TIMELINE_PLAYHEAD,
            );
        }

        vec![canvas_frame.into_geometry()]
    }

    /// A click anywhere in the plot seeks — the same command a key would send, so there is
    /// one path to the host and one log. The canvas is the only place that knows the plot's
    /// geometry, which is why the hit-test lives here and the message carries the frame.
    fn update(
        &self,
        _state: &mut (),
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else {
            return None;
        };
        let position = cursor.position_in(bounds)?;
        let (plot_x, plot_width) = Self::plot(bounds);
        let x = position.x - plot_x;
        if !(0.0..=plot_width).contains(&x) {
            return None;
        }
        let view = self.view(plot_width);
        Some(iced::widget::Action::publish(Message::SeekTo(
            view.frame_at(x, self.total),
        )))
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

    /// `A` arms the declared rig in order and wraps back to the default input, and the
    /// footer names what is armed — the shell half of `record <take> source=<name>`.
    #[test]
    fn the_source_key_cycles_the_declared_rig() {
        let mut spike = Spike::headless();

        // Declare through the host, not the snapshot: the cycle reads the *published*
        // rig, so a `source add` typed on the command line is what it arms.
        for (name, matcher) in [("vcv", "sink.monitor"), ("mon", "loopback")] {
            spike
                .command(host::HostCommand::SourceAdd {
                    name: name.into(),
                    kind: "pulse",
                    matcher: matcher.into(),
                    channels: 2,
                    clock: host::rig::ClockRole::Free,
                })
                .expect("source add");
        }
        // The actor publishes after it replies: wait for the rig rather than race the pump.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while spike.host.snapshot().sources.len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        spike.update(Message::Key(WorkflowKey::Char('A')));
        assert_eq!(
            spike.record_source.as_deref(),
            Some("vcv"),
            "the first declared source: {}",
            spike.status
        );
        assert!(spike.status.contains("vcv"), "{}", spike.status);

        spike.update(Message::Key(WorkflowKey::Char('A')));
        assert_eq!(spike.record_source.as_deref(), Some("mon"), "then the next");

        spike.update(Message::Key(WorkflowKey::Char('A')));
        assert_eq!(spike.record_source, None, "and back to the default input");
        assert!(spike.status.contains("default"), "{}", spike.status);

        // The footer names it: the default reads "default", an armed name reads itself.
        assert_eq!(record_source_label(None), "default");
        assert_eq!(record_source_label(Some("vcv")), "vcv");
    }

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

    /// The viewport is the timeline's one piece of arithmetic. A fitted view shows the
    /// whole piece across the plot; a zoom halves the density and holds its anchor; a
    /// scroll moves by a fraction of the screen; and everything stays inside the content.
    #[test]
    fn the_viewport_maps_frames_to_points_and_back() {
        // A 10 s piece across 200 points fits at 2400 frames/point, whole.
        let mut view = Viewport::default();
        view.fit(480_000, 200.0);
        assert_eq!(view.start, 0);
        assert!((view.frames_per_point - 2400.0).abs() < 1e-9);
        assert_eq!(view.visible(200.0), 480_000);

        // The mapping is the identity in both directions, and clamps at the ends.
        assert_eq!(view.frame_at(0.0, 480_000), 0);
        assert_eq!(view.frame_at(100.0, 480_000), 240_000);
        assert!((view.x_at(240_000) - 100.0).abs() < 1e-4);
        assert_eq!(view.frame_at(1e9, 480_000), 480_000, "past the end clamps");
        assert_eq!(view.frame_at(-5.0, 480_000), 0, "before the start clamps");

        // Zoom in around the playhead: the anchor stays where it was on screen.
        view.zoom(480_000, 200.0, 240_000, true);
        assert!((view.frames_per_point - 1200.0).abs() < 1e-9);
        assert_eq!(view.x_at(240_000).round(), 100.0, "the anchor holds");

        // Scroll one eighth of the screen (200 points × 1200 frames ÷ 8 = 30,000).
        let before = view.start;
        view.scroll(480_000, 200.0, 1);
        assert_eq!(view.start, before + 30_000);
        // …and the far end still stops at the content.
        view.scroll(480_000, 200.0, 1_000);
        assert_eq!(view.start, 480_000 - view.visible(200.0));
    }

    /// Zoom stops at the pyramid's floor: below one base bin per point the envelope would
    /// repeat a bin and claim detail the sidecar does not hold.
    #[test]
    fn zoom_never_goes_below_the_pyramid_floor() {
        let mut view = Viewport::default();
        view.fit(48_000, 400.0);
        for _ in 0..40 {
            view.zoom(48_000, 400.0, 24_000, true);
        }
        assert_eq!(
            view.frames_per_point, MIN_FRAMES_PER_POINT,
            "one base bin per point is the floor"
        );
        // Zooming out is bounded by the fitted density times the margin.
        for _ in 0..40 {
            view.zoom(48_000, 400.0, 24_000, false);
        }
        let max = Viewport::max_frames_per_point(48_000, 400.0);
        assert!(
            (view.frames_per_point - max).abs() < 1e-9,
            "{}",
            view.frames_per_point
        );
    }

    /// A zero-length piece does not divide by zero, and neither does a zero-width plot.
    #[test]
    fn a_degenerate_view_is_not_a_panic() {
        let mut view = Viewport::default();
        view.fit(0, 0.0);
        assert_eq!(view.visible(0.0), 0);
        assert_eq!(view.frame_at(10.0, 0), 0);
        assert_eq!(view.x_at(0), 0.0);
        view.zoom(0, 0.0, 0, true);
        view.scroll(0, 0.0, 1);
        view.clamp(0, 0.0);
        // A piece shorter than the floor still shows its whole length across the plot.
        let mut tiny = Viewport::default();
        tiny.fit(100, 200.0);
        assert_eq!(tiny.frames_per_point, MIN_FRAMES_PER_POINT);
        assert_eq!(tiny.visible(200.0), MIN_FRAMES_PER_POINT as u64 * 200);
    }

    /// A clip's source ranges honour the **region offset**: a clip reading the middle of a
    /// take must draw the middle of the pyramid, not the first seconds again.
    #[test]
    fn a_clip_reads_its_region_not_the_start_of_the_source() {
        let mut clip = clip("c0", 0, 1000);
        clip.src_start = 5000;
        clip.src_len = 1000;

        // Timeline frames 0..100 read source 5000..5100.
        assert_eq!(
            clip_source_ranges(&clip, 0, 100),
            SourceRanges::one((5000, 5100))
        );
        // Frames outside the clip are not this clip's material.
        assert_eq!(clip_source_ranges(&clip, 1000, 1100), SourceRanges::empty());
        // A range crossing the clip's edge is clamped to it.
        assert_eq!(
            clip_source_ranges(&clip, 900, 1500),
            SourceRanges::one((5900, 6000))
        );
    }

    /// A reversed clip reads its region backwards — the range mirrors, so the same set of
    /// samples is queried (a min/max envelope does not care about order).
    #[test]
    fn a_reversed_clip_mirrors_its_source_range() {
        let mut clip = clip("c0", 0, 1000);
        clip.src_start = 200;
        clip.reversed = true;
        // The clip's first frame is the region's top: frames 0..100 read 1100..1200.
        assert_eq!(
            clip_source_ranges(&clip, 0, 100),
            SourceRanges::one((1100, 1200))
        );
        assert_eq!(
            clip_source_ranges(&clip, 900, 1000),
            SourceRanges::one((200, 300))
        );
    }

    /// A looped clip repeats its loop; a column covering more than one loop reads the loop
    /// once, and a column crossing a boundary reads two ranges.
    #[test]
    fn a_looped_clip_wraps_its_source_range() {
        let mut clip = clip("c0", 0, 3000); // three loops of 1000
        clip.src_start = 0;
        clip.loop_len = Some(1000);

        // Inside one loop: a plain range.
        assert_eq!(
            clip_source_ranges(&clip, 100, 200),
            SourceRanges::one((100, 200))
        );
        // Crossing the boundary at 1000: two contiguous ranges, in source order.
        let crossing = clip_source_ranges(&clip, 950, 1050);
        assert_eq!(crossing.count(), 2);
        let ranges: Vec<(u64, u64)> = crossing.iter().collect();
        assert_eq!(ranges, vec![(950, 1000), (0, 50)]);
        // A whole loop or more is the loop, read once.
        assert_eq!(
            clip_source_ranges(&clip, 500, 1600),
            SourceRanges::one((0, 1000))
        );
    }

    /// The envelope a clip draws is read from the pyramid at the level the zoom implies,
    /// one column per point, and a column only exists where the data does.
    #[test]
    fn the_envelope_columns_follow_the_viewport() {
        let path = std::env::temp_dir().join(format!("iced-peaks-{}.spk", std::process::id()));
        let frames = 480_000u64; // 10 s: long enough that the fit is above the floor
        let mut builder = media::PeakBuilder::new();
        builder.push(&vec![0.5f32; frames as usize]);
        media::PeakFile::write(&path, &mut builder, 48_000).unwrap();
        let peaks = PeakFile::load(&path).unwrap();

        let clip = clip("c0", 0, frames);
        let mut view = Viewport::default();
        view.fit(frames, 400.0);
        assert!(
            view.frames_per_point > MIN_FRAMES_PER_POINT,
            "the fixture is fitted, not floored"
        );

        let columns = envelope_columns(&clip, &peaks, &view, frames, 0.0, 400.0);
        assert_eq!(columns.len(), 400, "one column per point of the piece");
        for (_, lo, hi) in &columns {
            assert!(*lo <= *hi, "a column's min never exceeds its max");
            assert!(
                (*hi - 0.5).abs() < 1e-6 && (*lo - 0.5).abs() < 1e-6,
                "the constant signal reads back as itself: {lo}..{hi}"
            );
        }

        // The level is chosen from the zoom: no finer than a column, and no coarser than
        // it has to be — a column costs one or two bins at any density.
        let per_column = view.frames_per_point.round() as u64;
        let level = peaks.level_for(per_column);
        assert!(peaks.bin_frames(level) <= per_column);
        if level + 1 < peaks.levels() {
            assert!(peaks.bin_frames(level + 1) > per_column);
        }

        // A clip whose region is past the pyramid's frames draws nothing, rather than
        // repeating the last bin as if it were material.
        let mut past = clip.clone();
        past.src_start = frames;
        assert!(
            envelope_columns(&past, &peaks, &view, frames, 0.0, 400.0).is_empty(),
            "no data, no faked columns"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The clip mapping clamps at both ends: frame 0 sits at the left, a fractional
    /// placement lands where the ratio says, and a frame past the content runs off the plot
    /// (the draw clips it) without wrapping or going negative.
    #[test]
    fn the_clip_mapping_is_bounded() {
        let mut view = Viewport::default();
        view.fit(0, 200.0);
        assert_eq!(view.frame_at(0.0, 0), 0, "an empty piece has no frame");

        // 200,000 frames across 200 points: 1000 frames per point.
        view.fit(200_000, 200.0);
        assert_eq!(view.x_at(0), 0.0);
        assert!((view.x_at(500) - 0.5).abs() < 1e-4);
        assert!((view.x_at(250) - 0.25).abs() < 1e-4);
        assert!(
            (view.x_at(200_000) - 200.0).abs() < 1e-4,
            "the end is the plot's edge"
        );
        assert!((view.x_at(5_000) - 5.0).abs() < 1e-4);
        // A frame before the viewport start clamps to the left edge (saturating), never a
        // wrapped or negative x.
        view.scroll(200_000, 200.0, 1);
        assert_eq!(view.x_at(0), 0.0);
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
mod wave {
    use super::*;

    /// A stereo float-WAV fixture at the session rate: a left channel and its inverse, so
    /// a channel mix-up is visible in the peaks.
    fn fixture(name: &str, frames: usize) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{name}-{}.wav", std::process::id()));
        let mut writer =
            media::wav::WavWriter::create_float(&path, SAMPLE_RATE as u32, 2).expect("create");
        let mut interleaved = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let sample = (i as f32 / 64.0).sin() * 0.4;
            interleaved.push(sample);
            interleaved.push(-sample);
        }
        writer.write(&interleaved).expect("samples");
        writer.finalize().expect("finalize");
        path
    }

    /// `--wave` end to end, headless: the file is imported into the session pool, the
    /// script it builds places **consecutive** clips (so the region offset is exercised),
    /// the host publishes the arrangement, and the shell loads each source's persisted
    /// pyramid — which is what the canvas draws an envelope from.
    ///
    /// This is the one test that covers the whole slice without a window: import → pool →
    /// sidecar → level-aware envelope query.
    #[test]
    fn a_wave_becomes_a_piece_the_timeline_can_draw() {
        let frames = 48_000usize; // 1 s per channel
        let path = fixture("iced-wave-fixture", frames);

        let (script, note) = wave_script(&path).expect("the import builds a script");
        assert!(script.starts_with("host v1"), "{script}");
        assert!(
            script.contains("pool "),
            "the script binds the pool: {script}"
        );
        // One clip per channel per cut — the import splits a stereo file into two mono
        // sources, and each is cut into [`WAVE_CLIPS_PER_SOURCE`] consecutive clips.
        let expected_clips = WAVE_CLIPS_PER_SOURCE as usize * 2;
        assert_eq!(script.matches("add_clip").count(), expected_clips);
        assert!(note.contains("2 track(s)"), "{note}");

        let mut spike = Spike::headless();
        spike.open_wave(&path);
        assert!(
            spike.status.contains("clip(s)"),
            "the shell reports what it loaded: {}",
            spike.status
        );
        // The poll tick is where the pyramids are read — the same path the window takes.
        spike.update(Message::Tick);

        let timeline = spike
            .snap
            .timeline
            .timeline
            .clone()
            .expect("the arrangement is published");
        assert_eq!(timeline.tracks.len(), 2, "one track per channel");
        let clips: usize = timeline.tracks.iter().map(|track| track.clips.len()).sum();
        assert_eq!(clips, expected_clips, "the piece has real clips to draw");

        // The second clip of the first track reads a region that does not start at 0 —
        // the case a peaks mapping gets wrong by drawing the pyramid from frame 0.
        let second = timeline.tracks[0].clips[1].clone();
        assert!(second.src_start > 0, "a cut-up clip has a region offset");
        let peaks = match spike.peaks.get(&second.source) {
            Some(SourcePeaks::Loaded(peaks)) => Arc::clone(peaks),
            Some(SourcePeaks::Missing(why)) => panic!("no peaks for {}: {why}", second.source),
            None => panic!("{} was never loaded", second.source),
        };
        assert_eq!(peaks.frames(), frames as u64);

        // And a draw of that clip's region, at the shell's fitted view, finds an envelope
        // there — the last link between the sidecar and the canvas.
        let mut view = Viewport::default();
        view.fit(timeline.end_frame().expect("an end"), 400.0);
        let columns = envelope_columns(
            &second,
            &peaks,
            &view,
            timeline.end_frame().unwrap(),
            0.0,
            400.0,
        );
        assert!(
            !columns.is_empty(),
            "the imported source's pyramid answers an envelope query"
        );
        // The fixture swings ±0.4, so the drawn extremes are inside [-0.4, 0.4] and at
        // least one column is non-silent.
        assert!(columns.iter().any(|(_, lo, hi)| *hi > 0.1 || *lo < -0.1));
        assert!(columns.iter().all(|(_, lo, hi)| *lo >= -0.5 && *hi <= 0.5));

        // The import wrote sources into the spike's **real** session pool, so the test
        // takes its own material back out: the pool is shared with `--record-check`, and a
        // test that left fixtures there would grow it on every run.
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Ok(entries) = std::fs::read_dir(demo_pool_dir())
        {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(stem) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::remove_file(&path);
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
