//! The timeline panel — a real audio file drawn as a braille envelope, with a
//! viewport, a playhead and a selection.
//!
//! This is the "audiofile view" the TUI evaluation needed and the thing a text
//! editor has no analogue for. It is also where **visual mode gets its first real
//! object**: a selection over a frame span.
//!
//! Design constraints (measured, see
//! `research/architecture/2026-09-21-tui-audio-prior-art.md` §2 and §5):
//!
//! - The data is a **min/max envelope per column from the peak pyramid**, never
//!   raw samples — `PeakBuilder::range_minmax` is exactly that query, so the
//!   timeline costs one pass over the file at load and O(columns) per frame.
//! - **Braille gives 2×4 sub-cells per terminal cell**, so a 200-cell panel has
//!   400 time positions and 4× the vertical resolution of a block row. One view
//!   column is one *sub*-column.
//! - Braille cells carry **one foreground colour per cell**, so clip colours and
//!   the selection highlight are cell-granular; the envelope itself is a single
//!   colour.

use std::path::{Path, PathBuf};

use media::peaks::PeakBuilder;
use media::wav::WavReader;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::braille::BRAILLE;
use ratatui::widgets::{Block, Paragraph};

/// Braille sub-cells per terminal cell: 2 across, 4 down.
const SUB_COLS: u64 = 2;
const SUB_ROWS: u64 = 4;

/// A loaded audio file: its peak pyramid and what the view needs to label it.
pub struct Wave {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub frames: u64,
    peaks: PeakBuilder,
}

impl Wave {
    /// Read the file once, feeding a peak pyramid. `WavReader::read_into` yields
    /// mono frames (channel 0 of a stereo file), which is what the pyramid wants.
    pub fn load(path: &Path) -> Result<Wave, String> {
        let mut reader = WavReader::open(path)?;
        let sample_rate = reader.sample_rate();
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

        Ok(Wave {
            path: path.to_path_buf(),
            sample_rate,
            frames: peaks.frames(),
            peaks,
        })
    }

    /// Min/max over a frame range — one envelope column. A silent column is
    /// `(0.0, 0.0)` (real data), an out-of-range query is also flat, which the
    /// view clamps anyway.
    fn minmax(&self, start: u64, end: u64) -> (f32, f32) {
        self.peaks.range_minmax(start, end).unwrap_or((0.0, 0.0))
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Seconds, for the readouts.
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate as f64
    }
}

/// Which of the interaction model's modes the panel is in. Only the two that the
/// timeline needs so far — the full model (Normal / Visual / Insert / Command) is
/// the [modal editing note](../../../.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual => "VISUAL",
        }
    }
}

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
/// detail that is not in the data. Zooming below this needs a raw-sample read
/// path (which is how `tui-wave` reaches single samples) — recorded as the next
/// step in the evaluation note.
const MIN_FRAMES_PER_CELL: u64 = media::peaks::PEAK_BASE_BIN as u64;

impl View {
    pub fn new(wave: &Wave) -> Self {
        let mut view = View {
            start: 0,
            frames_per_cell: SUB_COLS,
            selection: None,
            fitted_width: 0,
        };
        view.fit(wave, 100);
        view
    }

    /// Frames visible across `width_cells` terminal cells.
    pub fn visible(&self, width_cells: u64) -> u64 {
        self.frames_per_cell * width_cells.max(1)
    }

    /// Show the whole file.
    pub fn fit(&mut self, wave: &Wave, width_cells: u64) {
        let width = width_cells.max(1);
        let per_cell = (wave.frames / width).max(MIN_FRAMES_PER_CELL);
        // Even, so a cell is exactly two sub-columns.
        self.frames_per_cell = per_cell + (per_cell % SUB_COLS);
        self.start = 0;
        self.fitted_width = width;
    }

    /// The first draw knows the real panel width; fit once when it differs from
    /// the assumed one, so "fit" (and the density readout) start out honest.
    pub fn fit_if_needed(&mut self, wave: &Wave, width_cells: u64) {
        if self.fitted_width != width_cells.max(1) {
            let selection = self.selection;
            self.fit(wave, width_cells);
            self.selection = selection;
        }
    }

    /// Zoom by a factor of two, keeping `anchor` (a frame) roughly in place.
    pub fn zoom(&mut self, wave: &Wave, width_cells: u64, anchor: u64, in_: bool) {
        let before = self.frames_per_cell;
        let next = if in_ {
            before / 2
        } else {
            before.saturating_mul(2)
        };

        // Fit is the coarsest useful zoom: no point scrolling empty space.
        let mut per_cell = next.clamp(
            MIN_FRAMES_PER_CELL,
            (wave.frames / width_cells.max(1)).max(MIN_FRAMES_PER_CELL),
        );
        per_cell += per_cell % SUB_COLS;
        // Keep an even value in range.
        per_cell = per_cell.max(MIN_FRAMES_PER_CELL);
        if per_cell == before {
            return;
        }

        self.frames_per_cell = per_cell;

        // Keep the anchor at the same fraction of the screen.
        let visible = self.visible(width_cells);
        let anchor = anchor.min(wave.frames);
        let offset = (anchor - self.start.min(anchor)) * per_cell / before;
        self.start = anchor.saturating_sub(offset.min(visible));
        self.clamp(wave, width_cells);
    }

    /// Scroll by a fraction of the screen.
    pub fn scroll(&mut self, wave: &Wave, width_cells: u64, cells: i64) {
        let delta = (self.frames_per_cell as i64) * cells;
        self.start = (self.start as i64 + delta).max(0) as u64;
        self.clamp(wave, width_cells);
    }

    /// Keep the viewport inside the file (a short file still starts at 0).
    pub fn clamp(&mut self, wave: &Wave, width_cells: u64) {
        let visible = self.visible(width_cells);
        self.start = self
            .start
            .min(wave.frames.saturating_sub(visible.min(wave.frames)));
        self.start -= self.start % self.frames_per_cell;
    }

    /// Frame under terminal cell `cell`, in the current viewport.
    pub fn frame_at(&self, cell: u64) -> u64 {
        self.start + cell * self.frames_per_cell
    }

    pub fn selection_label(&self, wave: &Wave) -> Option<String> {
        let (a, b) = self.selection?;
        let (lo, hi) = (a.min(b), a.max(b));
        let rate = wave.sample_rate as f64;
        Some(format!(
            "{:.3} s → {:.3} s  ({:.3} s)",
            lo as f64 / rate,
            hi as f64 / rate,
            (hi - lo) as f64 / rate,
        ))
    }

    /// Milliseconds per terminal cell — the density number the evaluation asks
    /// for, shown live rather than argued about.
    pub fn ms_per_cell(&self, wave: &Wave) -> f64 {
        self.frames_per_cell as f64 / wave.sample_rate as f64 * 1000.0
    }
}

/// What the panel drew, so the mouse can hit-test it (the terminal has no widget
/// tree — the view publishes what is clickable).
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelRects {
    /// The envelope + ruler area, for click-to-seek.
    pub timeline: Rect,
    /// The ruler row, for click-to-seek (kept separate so a click can be told
    /// apart from a drag later).
    pub ruler: Rect,
}

/// Draw the panel: file name, ruler, braille envelope, selection, playhead.
pub fn draw(
    frame: &mut ratatui::Frame,
    area: Rect,
    wave: &Wave,
    view: &View,
    playhead_frame: u64,
    mode: Mode,
) -> PanelRects {
    let title = format!(
        " timeline — {} · {:.2} s · {} Hz · {:.3} ms/cell ",
        wave.file_name(),
        wave.seconds(),
        wave.sample_rate,
        view.ms_per_cell(wave),
    );
    let block =
        Block::bordered()
            .title(title)
            .border_style(Style::new().fg(if mode == Mode::Visual {
                Color::LightMagenta
            } else {
                Color::DarkGray
            }));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 || inner.width == 0 {
        return PanelRects {
            timeline: area,
            ruler: area,
        };
    }

    draw_ruler(frame, inner, wave, view);

    let envelope = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    draw_envelope(frame.buffer_mut(), envelope, wave, view, playhead_frame);

    PanelRects {
        timeline: envelope,
        ruler: Rect::new(inner.x, inner.y, inner.width, 1),
    }
}

/// A time ruler: a labelled tick every round number of seconds, chosen so ticks
/// are at least 10 cells apart.
fn draw_ruler(frame: &mut ratatui::Frame, inner: Rect, wave: &Wave, view: &View) {
    const STEPS: [f64; 10] = [0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];
    let ms_per_cell = view.ms_per_cell(wave).max(0.0001);
    let cells_per_second = 1000.0 / ms_per_cell;
    let step = STEPS
        .iter()
        .copied()
        .find(|s| s * cells_per_second >= 10.0)
        .unwrap_or(300.0);

    let width = inner.width as usize;
    let mut row = vec![b' '; width];
    let start_seconds = view.start as f64 / wave.sample_rate as f64;
    let end_seconds =
        (view.start + view.visible(inner.width as u64)) as f64 / wave.sample_rate as f64;

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
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
}

fn format_label(seconds: f64, step: f64) -> String {
    if step >= 1.0 {
        let total = seconds.round() as u64;
        format!("{}:{:02}", total / 60, total % 60)
    } else {
        format!("{seconds:.2}")
    }
}

/// The envelope: one min/max span per braille sub-column, written straight into
/// the buffer (2 sub-columns and 4 sub-rows per cell).
fn draw_envelope(buffer: &mut Buffer, area: Rect, wave: &Wave, view: &View, playhead_frame: u64) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let sub_cols = area.width as u64 * SUB_COLS;
    let sub_rows = area.height as u64 * SUB_ROWS;
    let frames_per_sub_col = (view.frames_per_cell / SUB_COLS).max(1);
    let half = sub_rows as f32 / 2.0;

    // Which cells the selection covers, in sub-columns.
    let selection = view.selection.map(|(a, b)| (a.min(b), a.max(b)));

    let mut bits = vec![0u8; area.width as usize * area.height as usize];

    for col in 0..sub_cols {
        let start = view.start + col * frames_per_sub_col;
        if start >= wave.frames {
            break;
        }
        let end = (start + frames_per_sub_col).min(wave.frames);
        let (min, max) = wave.minmax(start, end);

        // Amplitude → sub-row (terminal y grows downward, so max is the smaller
        // y). A silent column lands on the centre line — silence reads as data.
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
            if cell_y < area.height {
                let index = cell_y as usize * area.width as usize + cell_x as usize;
                bits[index] |= 1 << (dy * 2 + dx);
            }
        }
    }

    // Paint the accumulated braille, styling for the selection.
    for cell_y in 0..area.height {
        for cell_x in 0..area.width {
            let index = cell_y as usize * area.width as usize + cell_x as usize;
            let pattern = bits[index];
            if pattern == 0 {
                continue;
            }

            let frame_at = view.frame_at(cell_x as u64);
            let selected = selection
                .map(|(lo, hi)| frame_at >= lo && frame_at < hi)
                .unwrap_or(false);

            let style = if selected {
                Style::new().fg(Color::Black).bg(Color::LightMagenta)
            } else {
                Style::new().fg(Color::LightGreen)
            };

            let cell = &mut buffer[(area.x + cell_x, area.y + cell_y)];
            cell.set_char(BRAILLE[pattern as usize]);
            cell.set_style(style);
        }
    }

    // The playhead: one column, overriding the envelope there.
    if playhead_frame >= view.start {
        let column = (playhead_frame - view.start) / view.frames_per_cell;
        if column < area.width as u64 {
            for cell_y in 0..area.height {
                let cell = &mut buffer[(area.x + column as u16, area.y + cell_y)];
                cell.set_char('┃');
                cell.set_style(Style::new().fg(Color::LightRed));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_loaded_wave_reports_its_shape_and_an_envelope() {
        let path = temp_path("load");
        fixture(&path, 48_000, 48_000);
        let wave = Wave::load(&path).expect("load");

        assert_eq!(wave.sample_rate, 48_000);
        assert_eq!(wave.frames, 48_000);
        assert!((wave.seconds() - 1.0).abs() < 1e-9);

        // The first section is loud, the middle is silent, the last is quieter.
        // (Queried away from the section boundaries: `range_minmax` walks whole
        // 256-sample base bins, so a query that straddles a boundary inherits the
        // neighbouring bin's peak — a real property of the pyramid, and why the
        // view's zoom floor is one bin per column.)
        let (min_a, max_a) = wave.minmax(1_000, 9_000);
        let (min_b, max_b) = wave.minmax(17_000, 23_000);
        let (min_c, max_c) = wave.minmax(33_000, 41_000);
        assert!(max_a > 0.5, "loud section: {max_a}");
        assert_eq!((min_b, max_b), (0.0, 0.0), "silent section");
        assert!(max_c > 0.1 && max_c < 0.5, "quiet section: {max_c}");
        assert!(min_a.min(min_c) < 0.0, "the envelope has a negative side");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fit_shows_the_whole_file_and_zoom_narrows_it() {
        let path = temp_path("zoom");
        fixture(&path, 48_000, 480_000); // 10 s
        let wave = Wave::load(&path).expect("load");

        let mut view = View::new(&wave);
        let width = 100;
        view.fit(&wave, width);
        assert_eq!(view.start, 0);
        assert!(
            view.visible(width) >= wave.frames,
            "fit must show everything: {} < {}",
            view.visible(width),
            wave.frames
        );

        let fitted = view.frames_per_cell;
        view.zoom(&wave, width, 240_000, true);
        assert!(view.frames_per_cell < fitted, "zoom in narrows the view");
        assert_eq!(view.frames_per_cell % SUB_COLS, 0, "always even");

        // Zoom out never exceeds the fit.
        for _ in 0..40 {
            view.zoom(&wave, width, 240_000, false);
        }
        assert!(view.frames_per_cell <= fitted.max(MIN_FRAMES_PER_CELL));
        assert!(view.visible(width) >= wave.frames / 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn zooming_to_the_floor_stops_at_the_peak_bins() {
        let path = temp_path("floor");
        fixture(&path, 48_000, 96_000);
        let wave = Wave::load(&path).expect("load");

        let mut view = View::new(&wave);
        for _ in 0..60 {
            view.zoom(&wave, 100, 0, true);
        }
        // The pyramid's base bin is 256 frames: below that the same bin would be
        // drawn twice, so the view must stop here (≈5.3 ms/cell at 48 kHz) — a
        // raw-sample path is what would go finer.
        assert_eq!(view.frames_per_cell, MIN_FRAMES_PER_CELL);
        assert!(
            (view.ms_per_cell(&wave) - 5.333).abs() < 0.05,
            "≈5.33 ms/cell at the floor, got {}",
            view.ms_per_cell(&wave)
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scrolling_stays_inside_the_file() {
        let path = temp_path("scroll");
        fixture(&path, 48_000, 480_000);
        let wave = Wave::load(&path).expect("load");
        let width = 100;

        let mut view = View::new(&wave);
        view.scroll(&wave, width, -50);
        assert_eq!(view.start, 0, "cannot scroll before the file");

        for _ in 0..500 {
            view.scroll(&wave, width, 20);
        }
        assert!(
            view.start + view.visible(width) <= wave.frames + view.frames_per_cell,
            "cannot scroll far past the end"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_selection_reports_its_span() {
        let path = temp_path("selection");
        fixture(&path, 48_000, 480_000);
        let wave = Wave::load(&path).expect("load");

        let mut view = View::new(&wave);
        assert!(view.selection_label(&wave).is_none());

        view.selection = Some((48_000, 96_000));
        let label = view.selection_label(&wave).expect("label");
        assert!(label.contains("1.000 s → 2.000 s"), "{label}");
        assert!(label.contains("(1.000 s)"), "{label}");

        let _ = std::fs::remove_file(&path);
    }
}
