//! Grid snapping (alpha slice C): musical divisions in the **beat domain**, plus
//! the frame-domain helpers the parser and the shell share.
//!
//! The grid is *UI state*: a snapped edit is an edit whose frame was quantized
//! **before** the command was issued, so the log still records absolute frames and
//! determinism is untouched. Nothing here is logged, and nothing here changes a
//! sample — which is why the whole module is pure functions over numbers.
//!
//! Two domains, deliberately:
//!
//! - **Beats** ([`Grid`]) are what a musician means, and the only domain that
//!   survives a tempo change. The shell quantizes a *beat* position through the
//!   session's [`TempoMap`](../../engine/clock/struct.TempoMap.html) — which owns
//!   the segment math — rather than converting frames itself.
//! - **Frames** ([`quantize_frames`]) are what the `host v1` language speaks:
//!   a script writes `snap=<frames>` and gets exactly the logged frame the shell
//!   would have produced, with no tempo knowledge in the parser.
//!
//! Zero-crossing snap is deferred (a second quantizer behind the same seam);
//! click-freedom comes from default micro-fades on new boundaries.

/// A musical division of the grid — the four the alpha offers, coarser to finer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Division {
    /// A whole bar (the meter's beats-per-bar).
    Bar,
    /// One beat.
    Beat,
    /// Half a beat (the "and" of a beat).
    Half,
    /// A quarter of a beat (the sixteenth-note grid at 4/4).
    Quarter,
}

impl Division {
    /// Coarsest to finest, which is also the cycle order.
    pub const ALL: [Division; 4] = [
        Division::Bar,
        Division::Beat,
        Division::Half,
        Division::Quarter,
    ];

    /// Beats per grid step under a meter of `beats_per_bar` beats.
    pub fn beats(self, beats_per_bar: u32) -> f64 {
        // A meter of zero (a malformed tempo map) must not divide by zero: treat
        // it as the common 4/4 rather than producing an infinite step.
        let per_bar = beats_per_bar.max(1) as f64;
        match self {
            Division::Bar => per_bar,
            Division::Beat => 1.0,
            Division::Half => 0.5,
            Division::Quarter => 0.25,
        }
    }

    /// The next division in the cycle (`Bar → Beat → Half → Quarter → Bar`).
    pub fn next(self) -> Division {
        match self {
            Division::Bar => Division::Beat,
            Division::Beat => Division::Half,
            Division::Half => Division::Quarter,
            Division::Quarter => Division::Bar,
        }
    }

    /// The previous division in the cycle.
    pub fn previous(self) -> Division {
        match self {
            Division::Bar => Division::Quarter,
            Division::Beat => Division::Bar,
            Division::Half => Division::Beat,
            Division::Quarter => Division::Half,
        }
    }

    /// A short label for the status line.
    pub fn label(self) -> &'static str {
        match self {
            Division::Bar => "bar",
            Division::Beat => "beat",
            Division::Half => "1/2",
            Division::Quarter => "1/4",
        }
    }

    /// Parse a division name (the spelling a `host v1` script or a config uses).
    pub fn parse(name: &str) -> Option<Division> {
        match name {
            "bar" => Some(Division::Bar),
            "beat" => Some(Division::Beat),
            "1/2" | "half" => Some(Division::Half),
            "1/4" | "quarter" => Some(Division::Quarter),
            _ => None,
        }
    }
}

/// A grid: a step in beats, and the beat-domain quantization built on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    step_beats: f64,
}

impl Grid {
    /// The grid for `division` under a meter of `beats_per_bar` beats.
    pub fn new(division: Division, beats_per_bar: u32) -> Self {
        Grid {
            step_beats: division.beats(beats_per_bar),
        }
    }

    /// The step, in beats (never zero — see [`Division::beats`]).
    pub fn step_beats(&self) -> f64 {
        self.step_beats
    }

    /// The nearest grid line to `beat`. A position exactly between two lines
    /// rounds up, so a nudge is never a no-op on the line itself.
    pub fn nearest(&self, beat: f64) -> f64 {
        (beat / self.step_beats).round() * self.step_beats
    }

    /// The grid line at or before `beat`.
    pub fn floor(&self, beat: f64) -> f64 {
        (beat / self.step_beats).floor() * self.step_beats
    }

    /// The grid line at or after `beat`. Exactly on a line, that line.
    pub fn ceil(&self, beat: f64) -> f64 {
        (beat / self.step_beats).ceil() * self.step_beats
    }

    /// The frame nearest `frame` on the grid, at a **constant** tempo. The shell
    /// uses the session's tempo map instead (segments included); this is the
    /// single-tempo convenience `host v1`'s `snap=` and the tests share.
    pub fn nearest_frame(&self, frame: u64, tempo_bpm: f64, sample_rate: u32) -> u64 {
        let beat = frames_to_beats(frame, tempo_bpm, sample_rate);
        beats_to_frames(self.nearest(beat), tempo_bpm, sample_rate)
    }
}

/// Frames for `beat` at a constant `tempo_bpm` and `sample_rate`.
pub fn beats_to_frames(beat: f64, tempo_bpm: f64, sample_rate: u32) -> u64 {
    if tempo_bpm <= 0.0 || !tempo_bpm.is_finite() || sample_rate == 0 {
        return 0;
    }
    (beat.max(0.0) * 60.0 / tempo_bpm * sample_rate as f64).round() as u64
}

/// Beats for `frame` at a constant `tempo_bpm` and `sample_rate`.
pub fn frames_to_beats(frame: u64, tempo_bpm: f64, sample_rate: u32) -> f64 {
    if tempo_bpm <= 0.0 || !tempo_bpm.is_finite() || sample_rate == 0 {
        return 0.0;
    }
    frame as f64 * tempo_bpm / 60.0 / sample_rate as f64
}

/// The frame nearest `frame` on `division`'s grid at a constant tempo — the
/// plan's `snap::to_grid(frame, division, tempo, rate)`.
pub fn to_grid(
    frame: u64,
    division: Division,
    beats_per_bar: u32,
    tempo_bpm: f64,
    sample_rate: u32,
) -> u64 {
    Grid::new(division, beats_per_bar).nearest_frame(frame, tempo_bpm, sample_rate)
}

/// Round `frame` to the nearest multiple of `step` **frames** — the domain
/// `host v1`'s additive `snap=<frames>` modifier operates in, so a script snaps
/// exactly like the shell with no tempo knowledge in the parser. `step` must be
/// non-zero; a caller that accepts user input validates it (an "off" grid is the
/// absence of the modifier, not a step of zero).
///
/// The midpoint rounds **up** for an even step, and up for an odd step too when
/// the frame is past the integer half (`step = 5`: 2 → 0, 3 → 5). A frame near
/// `u64::MAX` saturates instead of wrapping.
pub fn quantize_frames(frame: u64, step: u64) -> u64 {
    debug_assert!(step > 0, "a zero grid step is not a grid");
    if step == 0 {
        return frame;
    }
    let half = step / 2;
    // `saturating_add`: a frame near `u64::MAX` (a nonsense seek, but a *parsed*
    // one) must not wrap to a small frame and move the playhead to the start.
    (frame.saturating_add(half) / step) * step
}

#[cfg(test)]
#[path = "tests/snap.rs"]
mod tests;
