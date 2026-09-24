//! The timeline panel — an **arrangement** of clips on tracks, drawn as braille
//! envelopes, with a viewport, a playhead, an active track and a selection.
//!
//! Two shapes feed it and they share one drawing path:
//!
//! - `--wave <file.wav>` — one lane, one clip covering the whole file (the
//!   "audiofile view");
//! - `--script <host script>` — the real thing: the host loads the script
//!   (`pool`/`arrange` lines), and the panel draws the arrangement value the host
//!   hands back. The clips are the engine's own clips, not a copy.
//!
//! Design constraints (measured, see
//! `research/architecture/2026-09-21-tui-audio-prior-art.md` §2 and §5):
//!
//! - The data is a **min/max envelope per column from the peak pyramid**, never
//!   raw samples — `PeakBuilder::range_minmax` is exactly that query.
//! - **Braille gives 2×4 sub-cells per terminal cell**, so a 100-cell panel has
//!   200 time positions and 4× the vertical resolution of a block row.
//! - Braille cells carry **one foreground colour per cell**, so a clip's colour
//!   and the selection highlight are cell-granular; colour *is* the clip
//!   boundary signal that survives at this resolution.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use media::peaks::{PEAK_BASE_BIN, PeakBuilder};
use media::wav::WavReader;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::braille::BRAILLE;
use ratatui::widgets::{Block, Paragraph};

/// Braille sub-cells per terminal cell: 2 across, 4 down.
const SUB_COLS: u64 = 2;
const SUB_ROWS: u64 = 4;

/// Terminal columns given to the track gutter (id + active marker).
pub const GUTTER: u16 = 7;

/// Lane colours, so clips read as objects: one colour per track.
const LANE_COLORS: [Color; 6] = [
    Color::LightGreen,
    Color::LightBlue,
    Color::LightYellow,
    Color::LightMagenta,
    Color::LightCyan,
    Color::LightRed,
];

/// A loaded audio file: its peak pyramid and what the view needs to label it.
pub struct Source {
    pub frames: u64,
    peaks: PeakBuilder,
}

impl Source {
    /// Read the file once, feeding a peak pyramid. `WavReader::read_into` yields
    /// mono frames (channel 0 of a stereo file), which is what the pyramid wants.
    pub fn load(path: &Path) -> Result<Source, String> {
        let mut reader = WavReader::open(path)?;
        let mut peaks = PeakBuilder::new();
        let mut buffer = vec![0.0f32; 8192];

        loop {
            let read = reader.read_into(&mut buffer);
            if read == 0 {
                break;
            }
            peaks.push(&buffer[..read]);
        }
        peaks.finalize();

        if peaks.frames() == 0 {
            return Err(format!("{} has no audio frames", path.display()));
        }

        Ok(Source {
            frames: peaks.frames(),
            peaks,
        })
    }

    /// Min/max over a frame range — one envelope column. A silent column is
    /// `(0.0, 0.0)` (real data); an out-of-range query is flat.
    pub fn minmax(&self, start: u64, end: u64) -> (f32, f32) {
        self.peaks.range_minmax(start, end).unwrap_or((0.0, 0.0))
    }
}

/// The sources an arrangement's clips refer to, loaded once each.
#[derive(Default)]
pub struct Sources {
    loaded: HashMap<String, Arc<Source>>,
    errors: Vec<(String, String)>,
}

impl Sources {
    /// Load a source by id from a pool path, or return the cached one. A failure
    /// is recorded (and surfaced in the state panel), never a panic — one broken
    /// file must not take the panel down.
    pub fn get(&mut self, id: &str, path: &Path) -> Option<Arc<Source>> {
        if let Some(source) = self.loaded.get(id) {
            return Some(Arc::clone(source));
        }
        match Source::load(path) {
            Ok(source) => {
                let source = Arc::new(source);
                self.loaded.insert(id.to_string(), Arc::clone(&source));
                Some(source)
            }
            Err(e) => {
                self.errors.push((id.to_string(), e));
                None
            }
        }
    }

    pub fn errors(&self) -> &[(String, String)] {
        &self.errors
    }
}

/// A clip placed on a lane — the engine's clip fields, plus the source it reads.
#[derive(Clone)]
pub struct Placed {
    pub id: String,
    /// The **pool source id** the clip reads (`media::Clip::source`) — what an
    /// `add_clip` line names, so a copied clip can be pasted as a new clip.
    pub source_id: String,
    pub source: Arc<Source>,
    /// First source frame of the region.
    pub src_start: u64,
    /// Region length in frames (the clip's timeline length).
    pub src_len: u64,
    /// Position of the clip's start on the track (timeline frames).
    pub at_frame: u64,
    pub gain: f32,
    pub fade_in: u64,
    pub fade_out: u64,
    /// A baked loop, when the source read wraps every `loop_len` frames.
    pub loop_len: Option<u64>,
}

impl Placed {
    pub fn end_frame(&self) -> u64 {
        self.at_frame + self.src_len
    }

    pub fn contains(&self, frame: u64) -> bool {
        frame >= self.at_frame && frame < self.end_frame()
    }

    /// The source frame for an arrangement frame, honouring a baked loop.
    pub fn source_frame(&self, frame: u64) -> Option<u64> {
        if !self.contains(frame) {
            return None;
        }
        let offset = frame - self.at_frame;
        let offset = match self.loop_len {
            Some(loop_len) if loop_len > 0 => offset % loop_len,
            _ => offset,
        };
        Some((self.src_start + offset).min(self.source.frames.saturating_sub(1)))
    }
}

pub struct Lane {
    pub id: String,
    pub clips: Vec<Placed>,
}

/// The arrangement: lanes of clips, and how long it is.
#[derive(Default)]
pub struct Arrangement {
    pub lanes: Vec<Lane>,
    /// The end of the last clip — the arrangement's length in frames.
    pub frames: u64,
    pub sample_rate: u32,
    /// Where the clips came from (a file, or a script), for the panel title.
    pub origin: String,
}

impl Arrangement {
    /// The host's arrangement, with each clip's source loaded from the pool.
    ///
    /// `pool` is the pool's own listing (`HostOutcome::pool_sources`), which is
    /// what maps a clip's `source` id to a readable path.
    pub fn from_host(
        timeline: &media::Timeline,
        pool: Option<&[media::PoolSource]>,
        sources: &mut Sources,
        sample_rate: u32,
        origin: String,
    ) -> Arrangement {
        let pool_dir = pool
            .and_then(|list| list.first())
            .and_then(|source| source.wav.parent().map(Path::to_path_buf));

        let mut lanes = Vec::with_capacity(timeline.tracks.len());
        let mut frames = 0u64;

        for track in &timeline.tracks {
            let mut clips = Vec::with_capacity(track.clips.len());

            for clip in &track.clips {
                // Resolve the clip's source: the pool lists it by id; a clip whose
                // source is not in the pool falls back to `<pool>/<id>.wav`.
                let path = pool
                    .and_then(|list| list.iter().find(|s| s.id == clip.source))
                    .map(|s| s.wav.clone())
                    .or_else(|| {
                        pool_dir
                            .as_ref()
                            .map(|dir| dir.join(format!("{}.wav", clip.source)))
                    });

                let Some(path) = path else {
                    sources.errors.push((
                        clip.source.clone(),
                        "no pool source with this id".to_string(),
                    ));
                    continue;
                };
                let Some(source) = sources.get(&clip.source, &path) else {
                    continue;
                };

                frames = frames.max(clip.at_frame + clip.src_len);
                clips.push(Placed {
                    id: clip.id.clone(),
                    source_id: clip.source.clone(),
                    source,
                    src_start: clip.src_start,
                    src_len: clip.src_len,
                    at_frame: clip.at_frame,
                    gain: clip.gain,
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                    loop_len: clip.loop_len,
                });
            }

            // **Every** track is a lane, including an empty one: a track is a place
            // a clip can be moved *to*, and a lane that vanishes when its last clip
            // leaves makes `J`/`K` and the active-track cursor disagree with the
            // host's own track list.
            lanes.push(Lane {
                id: track.id.clone(),
                clips,
            });
        }

        Arrangement {
            lanes,
            frames,
            sample_rate,
            origin,
        }
    }

    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate.max(1) as f64
    }

    pub fn clip_count(&self) -> usize {
        self.lanes.iter().map(|lane| lane.clips.len()).sum()
    }

    /// The clip covering `frame` on `lane`.
    pub fn clip_at(&self, lane: usize, frame: u64) -> Option<&Placed> {
        self.lanes
            .get(lane)?
            .clips
            .iter()
            .find(|clip| clip.contains(frame))
    }

    /// The next clip on `lane` starting at or after `frame` — for "next clip".
    pub fn clip_from(&self, lane: usize, frame: u64) -> Option<&Placed> {
        self.lanes
            .get(lane)?
            .clips
            .iter()
            .filter(|clip| clip.at_frame > frame)
            .min_by_key(|clip| clip.at_frame)
    }

    /// The previous clip on `lane` starting before `frame` — for "previous clip".
    pub fn clip_before(&self, lane: usize, frame: u64) -> Option<&Placed> {
        self.lanes
            .get(lane)?
            .clips
            .iter()
            .filter(|clip| clip.at_frame < frame)
            .max_by_key(|clip| clip.at_frame)
    }
}

/// The interaction model's modes come from the **shared workflow** (`workflow::Mode`),
/// not from this view: the model is the product, the panel is a rendering of it.
pub use workflow::Mode;

/// The viewport: which frames are visible, how wide a column is, and the
/// selection (in frames) when there is one.
pub struct View {
    /// The first visible frame (snapped to the column grid so zooming is stable).
    pub start: u64,
    /// Frames per terminal cell — always even, so it divides into 2 sub-columns.
    pub frames_per_cell: u64,
    /// Anchor and head of the selection, in frames (visual mode).
    pub selection: Option<(u64, u64)>,
    /// The panel width the view was fitted to; the fit is redone on the first
    /// draw at the real width (the terminal size is not known at load time).
    fitted_width: u64,
}

/// The finest zoom the **peak pyramid** can honestly serve: one base bin per
/// terminal cell (`PEAK_BASE_BIN` = 256 frames ≈ 5.3 ms/column at 48 kHz).
///
/// This is a real limit, not a UI choice: `range_minmax` walks whole base bins, so
/// a column narrower than a bin would draw the *same* bin repeatedly and claim
/// detail that is not in the data. Zooming below it needs a raw-sample read path
/// (which is how `tui-wave` reaches single samples). The human's read of the
/// braille envelope is that this resolution is already enough, so it is not
/// urgent — the *out* direction is the one that had to open up.
const MIN_FRAMES_PER_CELL: u64 = PEAK_BASE_BIN as u64;

/// How far *out* of the content the view may zoom: zooming out past the whole
/// arrangement is allowed (a DAW shows margin around the piece), up to this
/// multiple of the fitted density.
const ZOOM_OUT_MARGIN: u64 = 4;

impl View {
    pub fn new(total_frames: u64) -> Self {
        let mut view = View {
            start: 0,
            frames_per_cell: SUB_COLS,
            selection: None,
            fitted_width: 0,
        };
        view.fit(total_frames, 100);
        view
    }

    /// Frames visible across `width_cells` terminal cells.
    pub fn visible(&self, width_cells: u64) -> u64 {
        self.frames_per_cell * width_cells.max(1)
    }

    /// The density that shows the whole arrangement.
    pub fn fitted_density(&self, total_frames: u64, width_cells: u64) -> u64 {
        let per_cell = (total_frames / width_cells.max(1)).max(MIN_FRAMES_PER_CELL);
        // Even, so a cell is exactly two sub-columns.
        per_cell + (per_cell % SUB_COLS)
    }

    /// Show the whole arrangement.
    pub fn fit(&mut self, total_frames: u64, width_cells: u64) {
        self.frames_per_cell = self.fitted_density(total_frames, width_cells);
        self.start = 0;
        self.fitted_width = width_cells.max(1);
    }

    /// The first draw knows the real panel width; fit once when it differs from
    /// the assumed one, so "fit" (and the density readout) start out honest.
    pub fn fit_if_needed(&mut self, total_frames: u64, width_cells: u64) {
        if self.fitted_width != width_cells.max(1) {
            let selection = self.selection;
            self.fit(total_frames, width_cells);
            self.selection = selection;
        }
    }

    /// The coarsest zoom: the fitted density times [`ZOOM_OUT_MARGIN`].
    pub fn max_frames_per_cell(&self, total_frames: u64, width_cells: u64) -> u64 {
        self.fitted_density(total_frames, width_cells)
            .saturating_mul(ZOOM_OUT_MARGIN)
    }

    /// Zoom by a factor of two, keeping `anchor` (a frame) roughly in place.
    pub fn zoom(&mut self, total_frames: u64, width_cells: u64, anchor: u64, in_: bool) {
        let before = self.frames_per_cell;
        let next = if in_ {
            before / 2
        } else {
            before.saturating_mul(2)
        };

        let per_cell = next.clamp(
            MIN_FRAMES_PER_CELL,
            self.max_frames_per_cell(total_frames, width_cells),
        );
        if per_cell == before {
            return;
        }
        self.frames_per_cell = per_cell;

        // Keep the anchor at the same fraction of the screen.
        let visible = self.visible(width_cells);
        let anchor = anchor.min(total_frames);
        let offset = (anchor - self.start.min(anchor)) * per_cell / before;
        self.start = anchor.saturating_sub(offset.min(visible));
        self.clamp(total_frames, width_cells);
    }

    /// Scroll by a fraction of the screen.
    pub fn scroll(&mut self, total_frames: u64, width_cells: u64, cells: i64) {
        let delta = (self.frames_per_cell as i64) * cells;
        self.start = (self.start as i64 + delta).max(0) as u64;
        self.clamp(total_frames, width_cells);
    }

    /// Keep the viewport inside the content (a short piece still starts at 0).
    pub fn clamp(&mut self, total_frames: u64, width_cells: u64) {
        let visible = self.visible(width_cells);
        let limit = total_frames.saturating_sub(visible.min(total_frames));
        self.start = self.start.min(limit);
        self.start -= self.start % self.frames_per_cell;
    }

    /// Frame under terminal cell `cell`, in the current viewport.
    pub fn frame_at(&self, cell: u64) -> u64 {
        self.start + cell * self.frames_per_cell
    }

    pub fn selection_label(&self, sample_rate: u32) -> Option<String> {
        let (a, b) = self.selection?;
        let (lo, hi) = (a.min(b), a.max(b));
        let rate = sample_rate.max(1) as f64;
        Some(format!(
            "{:.3} s → {:.3} s  ({:.3} s)",
            lo as f64 / rate,
            hi as f64 / rate,
            (hi - lo) as f64 / rate,
        ))
    }

    /// Milliseconds per terminal cell — the density number the evaluation asks
    /// for, shown live rather than argued about.
    pub fn ms_per_cell(&self, sample_rate: u32) -> f64 {
        self.frames_per_cell as f64 / sample_rate.max(1) as f64 * 1000.0
    }
}

/// What the panel drew, so the mouse can hit-test it (the terminal has no widget
/// tree — the view publishes what is clickable).
#[derive(Debug, Clone, Default)]
pub struct PanelRects {
    /// The ruler row: clicking it seeks.
    pub ruler: Rect,
    /// One rect per visible lane, in lane order: clicking sets the active track
    /// and seeks.
    pub lanes: Vec<(usize, Rect)>,
    /// The whole panel, for the focus ring and click-to-focus.
    pub outer: Rect,
}

impl PanelRects {
    /// The lane under a position, if any.
    pub fn lane_at(&self, position: ratatui::layout::Position) -> Option<usize> {
        self.lanes
            .iter()
            .find(|(_, rect)| rect.contains(position))
            .map(|(lane, _)| *lane)
    }

    /// Whether a position is inside the panel (ruler or a lane).
    pub fn contains(&self, position: ratatui::layout::Position) -> bool {
        self.ruler.contains(position) || self.lane_at(position).is_some()
    }
}

/// Draw the panel: origin, ruler, one envelope lane per track, clip boundaries,
/// the selection, the playhead, and the active track's marker.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut ratatui::Frame,
    area: Rect,
    arrangement: &Arrangement,
    view: &View,
    playhead_frame: u64,
    mode: Mode,
    active_track: usize,
    focused: bool,
    grid: Option<media::Grid>,
    tempo: &host::TempoMap,
) -> PanelRects {
    let lanes = arrangement.lanes.len();
    let title = format!(
        " timeline — {} · {} track{}, {} clip{} · {:.2} s · {:.3} ms/cell ",
        arrangement.origin,
        lanes,
        if lanes == 1 { "" } else { "s" },
        arrangement.clip_count(),
        if arrangement.clip_count() == 1 {
            ""
        } else {
            "s"
        },
        arrangement.seconds(),
        view.ms_per_cell(arrangement.sample_rate),
    );

    // Focus is shown on the border; visual mode overrides it (the mode must be
    // visible even when the panel is not focused).
    let border = if mode == Mode::Visual {
        Color::LightMagenta
    } else if focused {
        Color::LightBlue
    } else {
        Color::DarkGray
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut rects = PanelRects {
        outer: area,
        ..PanelRects::default()
    };

    if inner.height < 2 || inner.width <= GUTTER {
        return rects;
    }

    let width_cells = inner.width.saturating_sub(GUTTER) as u64;

    // The ruler spans the envelope area, right of the gutter.
    let ruler_area = Rect::new(inner.x + GUTTER, inner.y, inner.width - GUTTER, 1);
    draw_ruler(frame, ruler_area, arrangement, view, grid, tempo);
    rects.ruler = ruler_area;

    let body = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);

    if arrangement.lanes.is_empty() {
        frame.render_widget(
            Paragraph::new("no clips — run with --wave <file.wav> or --script <host script>")
                .style(Style::new().fg(Color::Gray)),
            body,
        );
        return rects;
    }

    draw_lanes(frame, body, arrangement, view, active_track, &mut rects);

    // The playhead, over everything, gutter included.
    if playhead_frame >= view.start {
        let column = (playhead_frame - view.start) / view.frames_per_cell;
        if column < width_cells {
            let x = inner.x + GUTTER + column as u16;
            for y in inner.y..inner.y + inner.height {
                let cell = &mut frame.buffer_mut()[(x, y)];
                cell.set_char('┃');
                cell.set_style(Style::new().fg(Color::LightRed));
            }
        }
    }

    rects
}

/// The ruler: a **bar/beat** ruler when a snap grid is armed, and the seconds
/// ruler otherwise.
///
/// The musical ruler is what makes the grid legible — you can see the lines an
/// edit will land on. Bar lines are labelled with the bar number (from the
/// session's tempo map, so a tempo or meter change moves them with the music) and
/// beat lines get a light tick; when bars are too dense to label, it degrades to
/// beat ticks alone rather than printing a smear.
fn draw_ruler(
    frame: &mut ratatui::Frame,
    area: Rect,
    arrangement: &Arrangement,
    view: &View,
    grid: Option<media::Grid>,
    tempo: &host::TempoMap,
) {
    if let Some(grid) = grid {
        draw_musical_ruler(frame, area, arrangement, view, grid, tempo);
        return;
    }
    draw_seconds_ruler(frame, area, arrangement, view);
}

/// The time ruler: a labelled tick every round number of seconds, chosen so ticks
/// are at least 10 cells apart.
fn draw_seconds_ruler(
    frame: &mut ratatui::Frame,
    area: Rect,
    arrangement: &Arrangement,
    view: &View,
) {
    const STEPS: [f64; 10] = [0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];
    let rate = arrangement.sample_rate.max(1);
    let ms_per_cell = view.ms_per_cell(rate).max(0.0001);
    let cells_per_second = 1000.0 / ms_per_cell;
    let step = STEPS
        .iter()
        .copied()
        .find(|s| s * cells_per_second >= 10.0)
        .unwrap_or(300.0);

    let width = area.width as usize;
    let mut row = vec![b' '; width];
    let start_seconds = view.start as f64 / rate as f64;
    let end_seconds = (view.start + view.visible(area.width as u64)) as f64 / rate as f64;

    let mut tick = (start_seconds / step).ceil() * step;
    while tick <= end_seconds {
        let cell = ((tick - start_seconds) * cells_per_second).round() as usize;
        if cell < width {
            row[cell] = b'|';
            let label = format_label(tick, step);
            for (i, ch) in label.bytes().enumerate() {
                if cell + 1 + i < width {
                    row[cell + 1 + i] = ch;
                }
            }
        }
        tick += step;
    }

    let text = String::from_utf8_lossy(&row).to_string();
    frame.render_widget(
        Paragraph::new(text).style(Style::new().fg(Color::Gray)),
        Rect::new(area.x, area.y, area.width, 1),
    );
}

/// The bar/beat ruler: `|` on a bar (labelled with its number), `·` on a beat.
///
/// Driven **by cell**, not by grid line: each column asks "does a bar (or, failing
/// that, a grid line) fall in me?", which is what makes the ruler honest at every
/// zoom. Walking grid lines instead would either iterate hundreds of thousands of
/// steps in a 30-minute view or cap the walk and leave the far end of the row
/// blank. Bars win a cell over beat ticks, so a number is never lost to a tick.
fn draw_musical_ruler(
    frame: &mut ratatui::Frame,
    area: Rect,
    arrangement: &Arrangement,
    view: &View,
    grid: media::Grid,
    tempo: &host::TempoMap,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let rate = arrangement.sample_rate.max(1);
    // Frames per column, from the viewport's own zoom.
    let frames_per_cell = view.ms_per_cell(rate) * rate as f64 / 1000.0;
    if frames_per_cell <= 0.0 || !frames_per_cell.is_finite() {
        return;
    }
    let step = grid.step_beats();
    if step <= 0.0 {
        return;
    }

    let end_frame = view.start + view.visible(area.width as u64);
    // The meter in force at the viewport's start sets the bar length and the bar
    // numbering. (A meter change *inside* the view keeps this phase: continuing
    // the count across a segment boundary needs a bar-phase notion the tempo map
    // does not have yet — recorded in the slice note as still open.)
    let bar_beats = tempo.meter_at(view.start).max(1) as f64;
    let row = ruler_marks(
        width,
        view,
        frames_per_cell,
        end_frame,
        grid,
        tempo,
        bar_beats,
    );
    frame.render_widget(
        Paragraph::new(String::from_iter(row)).style(Style::new().fg(Color::Gray)),
        Rect::new(area.x, area.y, area.width, 1),
    );
}

/// The ruler's row, in characters — the draw above is only a render of this, which
/// keeps the geometry testable without a terminal.
///
/// `numbered` is decided by the caller's cell budget (a bar number needs ~4 cells).
fn ruler_marks(
    width: usize,
    view: &View,
    frames_per_cell: f64,
    end_frame: u64,
    grid: media::Grid,
    tempo: &host::TempoMap,
    bar_beats: f64,
) -> Vec<char> {
    let step = grid.step_beats();
    let cells_per_beat = (tempo.frame_at(tempo.beat_at(view.start).ceil() + 1.0)
        - tempo.frame_at(tempo.beat_at(view.start).ceil())) as f64
        / frames_per_cell;
    let numbered = cells_per_beat * bar_beats >= 5.0;
    // The cell a frame falls in, if it is visible at all.
    let cell_of = |at: u64| -> Option<usize> {
        (at >= view.start && at <= end_frame)
            .then(|| ((at - view.start) as f64 / frames_per_cell).floor() as usize)
            .filter(|cell| *cell < width)
    };

    let mut row = vec![' '; width];
    for cell in 0..width {
        // The beat at the *left edge* of this cell (a cell is ~10 ms wide at the
        // zoom floor, so one sample of the map per cell is the right resolution).
        let at = view.start + (cell as f64 * frames_per_cell).round() as u64;
        let beat = tempo.beat_at(at);

        // A bar in this cell wins: it carries the number.
        let bar_frame = tempo.frame_at((beat / bar_beats).round() * bar_beats);
        if cell_of(bar_frame) == Some(cell) {
            row[cell] = '|';
            if numbered {
                let number = ((beat / bar_beats).round() as u64) + 1;
                for (k, ch) in number.to_string().chars().enumerate() {
                    if cell + 1 + k < width {
                        row[cell + 1 + k] = ch;
                    }
                }
            }
            continue;
        }
        // Otherwise, a grid line finer than a bar.
        if step < bar_beats {
            let line = tempo.frame_at((beat / step).round() * step);
            if cell_of(line) == Some(cell) {
                row[cell] = '·';
            }
        }
    }
    row
}

fn format_label(seconds: f64, step: f64) -> String {
    if step >= 1.0 {
        let total = seconds.round() as u64;
        format!("{}:{:02}", total / 60, total % 60)
    } else {
        format!("{seconds:.2}")
    }
}

/// The lanes: a gutter per track, then a braille envelope per clip, coloured by
/// track, with clip boundaries marked by half-block glyphs at the edges.
fn draw_lanes(
    frame: &mut ratatui::Frame,
    body: Rect,
    arrangement: &Arrangement,
    view: &View,
    active_track: usize,
    rects: &mut PanelRects,
) {
    let lanes = arrangement.lanes.len();
    let rows_per_lane = (body.height as usize / lanes.max(1)).clamp(1, 4) as u16;
    let visible_lanes = ((body.height as usize) / rows_per_lane.max(1) as usize).min(lanes);

    let width_cells = body.width.saturating_sub(GUTTER) as u64;
    if width_cells == 0 {
        return;
    }
    let sub_cols = width_cells * SUB_COLS;
    let frames_per_sub_col = (view.frames_per_cell / SUB_COLS).max(1);
    let selection = view.selection.map(|(a, b)| (a.min(b), a.max(b)));

    for (lane_index, lane) in arrangement.lanes.iter().take(visible_lanes).enumerate() {
        let y = body.y + lane_index as u16 * rows_per_lane;
        let lane_area = Rect::new(body.x + GUTTER, y, body.width - GUTTER, rows_per_lane);
        rects.lanes.push((lane_index, lane_area));

        // Gutter: the track id, marked when it is the active track.
        let color = LANE_COLORS[lane_index % LANE_COLORS.len()];
        let active = lane_index == active_track;
        let gutter_style = if active {
            Style::new()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(color)
        };
        frame.render_widget(
            Paragraph::new(format!("{}{:<4}", if active { "▸" } else { " " }, lane.id))
                .style(gutter_style),
            Rect::new(body.x, y, GUTTER, rows_per_lane.max(1)),
        );

        let sub_rows = lane_area.height as u64 * SUB_ROWS;
        let half = sub_rows as f32 / 2.0;
        let mut bits = vec![0u8; lane_area.width as usize * rows_per_lane as usize];

        for col in 0..sub_cols {
            let frame_start = view.start + col * frames_per_sub_col;
            let frame_end = frame_start + frames_per_sub_col;

            // The clip covering this column (the first one that does).
            let Some(clip) = lane
                .clips
                .iter()
                .find(|clip| frame_end > clip.at_frame && frame_start < clip.end_frame())
            else {
                continue;
            };
            let Some(source_frame) = clip.source_frame(frame_start.max(clip.at_frame)) else {
                continue;
            };

            let end = (source_frame + frames_per_sub_col).min(clip.source.frames);
            let (min, max) = clip.source.minmax(source_frame, end.max(source_frame + 1));

            // Fades and gain are per-clip and authoritative, so the envelope
            // shows them: a column inside a fade is scaled by its linear ramp.
            let offset = frame_start.max(clip.at_frame) - clip.at_frame;
            let mut fade = 1.0f32;
            if clip.fade_in > 0 && offset < clip.fade_in {
                fade = offset as f32 / clip.fade_in as f32;
            }
            let tail = clip.src_len.saturating_sub(offset);
            if clip.fade_out > 0 && tail < clip.fade_out {
                fade = fade.min(tail as f32 / clip.fade_out as f32);
            }
            let scale = clip.gain * fade;
            let (min, max) = (min * scale, max * scale);

            let to_row = |value: f32| -> u64 {
                let y = half - value.clamp(-1.0, 1.0) * half;
                (y.round().max(0.0) as u64).min(sub_rows.saturating_sub(1))
            };
            let (top, bottom) = (to_row(max), to_row(min));

            let cell_x = (col / SUB_COLS) as u16;
            let dx = (col % SUB_COLS) as u8;
            for sub_row in top..=bottom {
                let cell_y = (sub_row / SUB_ROWS) as u16;
                let dy = (sub_row % SUB_ROWS) as u8;
                if cell_y < lane_area.height {
                    let index = cell_y as usize * lane_area.width as usize + cell_x as usize;
                    bits[index] |= 1 << (dy * 2 + dx);
                }
            }
        }

        // Paint the braille, styling for the selection.
        for cell_y in 0..lane_area.height {
            for cell_x in 0..lane_area.width {
                let index = cell_y as usize * lane_area.width as usize + cell_x as usize;
                let pattern = bits[index];
                if pattern == 0 {
                    continue;
                }

                let frame_at = view.frame_at(cell_x as u64);
                let selected = selection
                    .map(|(lo, hi)| frame_at >= lo && frame_at < hi)
                    .unwrap_or(false);

                let style = if selected {
                    Style::new().fg(Color::Black).bg(color)
                } else {
                    Style::new().fg(color)
                };

                let cell = &mut frame.buffer_mut()[(lane_area.x + cell_x, lane_area.y + cell_y)];
                cell.set_char(BRAILLE[pattern as usize]);
                cell.set_style(style);
            }
        }

        // Clip boundaries: the first and last column of each visible clip, drawn
        // over the envelope (a cell is the finest boundary this resolution has).
        for clip in &lane.clips {
            if clip.end_frame() <= view.start {
                continue;
            }
            for (edge, glyph) in [(clip.at_frame, '▏'), (clip.end_frame() - 1, '▕')] {
                if edge < view.start {
                    continue;
                }
                let column = (edge - view.start) / view.frames_per_cell;
                if column >= width_cells {
                    continue;
                }
                let x = lane_area.x + column as u16;
                for y in lane_area.y..lane_area.y + lane_area.height {
                    let cell = &mut frame.buffer_mut()[(x, y)];
                    cell.set_char(glyph);
                    cell.set_style(Style::new().fg(Color::White));
                }
            }
        }
    }

    if visible_lanes < lanes {
        let hidden = lanes - visible_lanes;
        frame.render_widget(
            Paragraph::new(format!(
                "… {hidden} more track{}",
                if hidden == 1 { "" } else { "s" }
            ))
            .style(Style::new().fg(Color::DarkGray)),
            Rect::new(body.x, body.y + body.height - 1, body.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A tiny WAV on disk: a burst, silence, then a quieter burst — enough for
    /// the envelope to have shape to check.
    fn fixture(path: &Path, sample_rate: u32, frames: usize) {
        let mut writer = media::wav::WavWriter::create_float(path, sample_rate, 1).expect("create");
        let samples: Vec<f32> = (0..frames)
            .map(|i| {
                let section = i * 3 / frames.max(1); // 0, 1, 2
                let amplitude = [0.9f32, 0.0, 0.3][section.min(2)];
                amplitude * (i as f32 * 0.05).sin()
            })
            .collect();
        writer.write(&samples).expect("write");
        writer.finalize().expect("finalize");
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("tui-shell-test-{name}-{}.wav", std::process::id()));
        path
    }

    /// A fixture WAV plus the **host-shaped** value the panel receives — a
    /// `Timeline` and the pool listing that resolves its clip's source — so the
    /// tests exercise the same path the running shell does (`from_host`).
    fn a_wave(name: &str, frames: usize) -> (PathBuf, Arrangement) {
        let path = temp_path(name);
        fixture(&path, 48_000, frames);

        let pool = vec![media::PoolSource {
            id: "s1".to_string(),
            wav: path.clone(),
            peaks: path.with_extension("peaks"),
            frames: frames as u64,
            sample_rate: 48_000,
            channels: 1,
            peaks_missing: true,
            finalized: true,
        }];
        let timeline = media::Timeline {
            tracks: vec![media::Track {
                id: "t0".to_string(),
                clips: vec![media::Clip {
                    id: "c0".to_string(),
                    source: "s1".to_string(),
                    src_start: 0,
                    src_len: frames as u64,
                    at_frame: 0,
                    fade_in: 0,
                    fade_out: 0,
                    gain: 1.0,
                    loop_len: None,
                }],
            }],
        };

        let mut sources = Sources::default();
        let arrangement = Arrangement::from_host(
            &timeline,
            Some(&pool),
            &mut sources,
            48_000,
            name.to_string(),
        );

        (path, arrangement)
    }

    #[test]
    fn a_loaded_wave_reports_its_shape_and_an_envelope() {
        let (path, arrangement) = a_wave("load", 48_000);
        let source = &arrangement.lanes[0].clips[0].source;

        assert_eq!(arrangement.sample_rate, 48_000);
        assert_eq!(arrangement.frames, 48_000);
        assert!((arrangement.seconds() - 1.0).abs() < 1e-9);
        assert_eq!(arrangement.clip_count(), 1);

        // The first section is loud, the middle is silent, the last is quieter.
        // (Queried away from the section boundaries: `range_minmax` walks whole
        // 256-sample base bins, so a query that straddles a boundary inherits the
        // neighbouring bin's peak — a real property of the pyramid, and why the
        // view's zoom floor is one bin per column.)
        let (_, max_a) = source.minmax(1_000, 9_000);
        let (min_b, max_b) = source.minmax(17_000, 23_000);
        let (min_c, max_c) = source.minmax(33_000, 41_000);
        assert!(max_a > 0.5, "loud section: {max_a}");
        assert_eq!((min_b, max_b), (0.0, 0.0), "silent section");
        assert!(max_c > 0.1 && max_c < 0.5, "quiet section: {max_c}");
        assert!(min_c < 0.0, "the envelope has a negative side");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fit_shows_the_whole_arrangement_and_zoom_narrows_it() {
        let (path, arrangement) = a_wave("zoom", 480_000); // 10 s
        let width = 100;

        let mut view = View::new(arrangement.frames);
        view.fit(arrangement.frames, width);
        assert_eq!(view.start, 0);
        assert!(
            view.visible(width) >= arrangement.frames,
            "fit must show everything: {} < {}",
            view.visible(width),
            arrangement.frames
        );

        let fitted = view.frames_per_cell;
        view.zoom(arrangement.frames, width, 240_000, true);
        assert!(view.frames_per_cell < fitted, "zoom in narrows the view");
        assert_eq!(view.frames_per_cell % SUB_COLS, 0, "always even");

        // Zooming out keeps going past the content, but not without bound.
        for _ in 0..40 {
            view.zoom(arrangement.frames, width, 240_000, false);
        }
        assert_eq!(
            view.frames_per_cell,
            fitted * ZOOM_OUT_MARGIN,
            "zoom out stops at the margin multiple"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn zooming_to_the_floor_stops_at_the_peak_bins() {
        let (path, arrangement) = a_wave("floor", 96_000);

        let mut view = View::new(arrangement.frames);
        for _ in 0..60 {
            view.zoom(arrangement.frames, 100, 0, true);
        }
        // The pyramid's base bin is 256 frames: below that the same bin would be
        // drawn twice, so the view must stop here (≈5.3 ms/cell at 48 kHz).
        assert_eq!(view.frames_per_cell, MIN_FRAMES_PER_CELL);
        assert!(
            (view.ms_per_cell(arrangement.sample_rate) - 5.333).abs() < 0.05,
            "≈5.33 ms/cell at the floor, got {}",
            view.ms_per_cell(arrangement.sample_rate)
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scrolling_stays_inside_the_content() {
        let (path, arrangement) = a_wave("scroll", 480_000);
        let width = 100;

        let mut view = View::new(arrangement.frames);
        view.scroll(arrangement.frames, width, -50);
        assert_eq!(view.start, 0, "cannot scroll before the content");

        for _ in 0..500 {
            view.scroll(arrangement.frames, width, 20);
        }
        assert!(
            view.start + view.visible(width) <= arrangement.frames + view.frames_per_cell,
            "cannot scroll far past the end"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_selection_reports_its_span() {
        let (path, arrangement) = a_wave("selection", 480_000);

        let mut view = View::new(arrangement.frames);
        assert!(view.selection_label(arrangement.sample_rate).is_none());

        view.selection = Some((48_000, 96_000));
        let label = view
            .selection_label(arrangement.sample_rate)
            .expect("label");
        assert!(label.contains("1.000 s → 2.000 s"), "{label}");
        assert!(label.contains("(1.000 s)"), "{label}");

        let _ = std::fs::remove_file(&path);
    }

    /// The ruler is drawn **by cell**, so it survives a view that spans far more
    /// grid steps than there are columns — the case that used to stop after a
    /// fixed number of steps and leave the rest of the row blank (a 30-minute
    /// arrangement on the 1/4 grid needs ~14 000 steps for 100 columns).
    #[test]
    fn the_musical_ruler_fills_the_row_at_any_zoom() {
        let rate = 48_000u32;
        let tempo = host::TempoMap::new(rate, 120.0, 4);
        let grid = media::Grid::new(media::Division::Quarter, 4);
        let width = 100usize;

        for (seconds, label) in [(1.5f64, "zoomed in"), (1800.0, "30 minutes")] {
            let frames = (seconds * rate as f64) as u64;
            let mut view = View::new(frames);
            view.fit(frames, width as u64);
            let frames_per_cell = view.visible(width as u64) as f64 / width as f64;
            let end = view.start + view.visible(width as u64);
            let row = ruler_marks(width, &view, frames_per_cell, end, grid, &tempo, 4.0);

            assert_eq!(row.len(), width);
            assert_eq!(row[0], '|', "{label}: the first bar is marked");
            let marks = row.iter().filter(|c| !c.is_whitespace()).count();
            // Zoomed in, a fine grid is legitimately sparse (one line every ~8
            // cells); zoomed out, a bar lands in most cells — either way the row
            // is marked, never capped.
            assert!(marks >= 5, "{label}: the row is unmarked ({marks} marks)");
            if seconds > 60.0 {
                assert!(
                    marks >= width / 4,
                    "{label}: a zoomed-out grid must mark most of the row ({marks} marks)"
                );
            }
            // The far end of the row is drawn too — the bug this test exists for.
            let tail = &row[width - 10..];
            assert!(
                tail.iter().any(|c| !c.is_whitespace()),
                "{label}: the last columns are blank: {row:?}"
            );
        }
    }

    #[test]
    fn a_looped_clip_wraps_inside_its_own_region() {
        let (path, arrangement) = a_wave("mapping", 480_000);
        let mut clip = arrangement.lanes[0].clips[0].clone();

        assert!(clip.contains(0));
        assert!(clip.contains(clip.end_frame() - 1));
        assert!(!clip.contains(clip.end_frame()), "the end is exclusive");

        clip.src_start = 1_000;
        clip.loop_len = Some(500);
        // 1250 and 1750 are a loop length apart, so they read the same sample.
        assert_eq!(clip.source_frame(1_250), Some(1_250));
        assert_eq!(clip.source_frame(1_750), Some(1_250));

        let _ = std::fs::remove_file(&path);
    }
}
