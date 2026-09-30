use super::*;

/// 120 bpm at 48 kHz: one beat is 24 000 frames, a 4/4 bar is 96 000.
const RATE: u32 = 48_000;
const BPM: f64 = 120.0;

#[test]
fn a_beat_is_half_a_second_at_120_bpm() {
    assert_eq!(beats_to_frames(1.0, BPM, RATE), 24_000);
    assert_eq!(beats_to_frames(4.0, BPM, RATE), 96_000);
    assert_eq!(frames_to_beats(24_000, BPM, RATE), 1.0);
    // The inverse pair round-trips on a beat boundary, and the guards do not
    // divide by zero (a malformed tempo or a 0 Hz session).
    assert_eq!(
        beats_to_frames(frames_to_beats(48_000, BPM, RATE), BPM, RATE),
        48_000
    );
    assert_eq!(beats_to_frames(1.0, 0.0, RATE), 0);
    assert_eq!(beats_to_frames(1.0, BPM, 0), 0);
    assert_eq!(frames_to_beats(100, 0.0, RATE), 0.0);
    assert_eq!(
        beats_to_frames(-4.0, BPM, RATE),
        0,
        "a negative beat clamps"
    );
}

#[test]
fn divisions_are_musical_steps() {
    assert_eq!(Division::Bar.beats(4), 4.0);
    assert_eq!(Division::Beat.beats(4), 1.0);
    assert_eq!(Division::Half.beats(4), 0.5);
    assert_eq!(Division::Quarter.beats(4), 0.25);
    // A 3/4 bar is three beats; a degenerate meter does not divide by zero.
    assert_eq!(Division::Bar.beats(3), 3.0);
    assert_eq!(Division::Bar.beats(0), 1.0);
    assert_eq!(Division::parse("1/2"), Some(Division::Half));
    assert_eq!(Division::parse("bar"), Some(Division::Bar));
    assert_eq!(Division::parse("nope"), None);
    // The cycle visits every division and returns.
    let mut d = Division::Bar;
    for _ in 0..Division::ALL.len() {
        d = d.next();
    }
    assert_eq!(d, Division::Bar);
    assert_eq!(Division::Bar.next(), Division::Beat);
    assert_eq!(Division::Bar.previous(), Division::Quarter);
}

#[test]
fn a_bar_grid_snaps_to_the_bar() {
    let grid = Grid::new(Division::Bar, 4);
    assert_eq!(grid.step_beats(), 4.0);
    assert_eq!(grid.nearest(0.4), 0.0);
    assert_eq!(grid.nearest(2.0), 4.0, "exactly between bars rounds up");
    assert_eq!(grid.nearest(5.9), 4.0);
    assert_eq!(grid.floor(5.9), 4.0);
    assert_eq!(grid.ceil(5.9), 8.0);
    assert_eq!(grid.ceil(4.0), 4.0, "on a line, that line");
    // In frames: a bar at 120 bpm / 48 kHz.
    assert_eq!(grid.nearest_frame(0, BPM, RATE), 0);
    assert_eq!(grid.nearest_frame(95_000, BPM, RATE), 96_000);
    assert_eq!(grid.nearest_frame(47_000, BPM, RATE), 0);
    assert_eq!(grid.nearest_frame(49_000, BPM, RATE), 96_000);
}

#[test]
fn to_grid_is_the_constant_tempo_convenience() {
    // The plan's example: a beat grid is 24 000 frames at 120 bpm, and the
    // halves/quarters subdivide it.
    assert_eq!(to_grid(24_000, Division::Beat, 4, BPM, RATE), 24_000);
    assert_eq!(to_grid(30_000, Division::Beat, 4, BPM, RATE), 24_000);
    assert_eq!(to_grid(36_000, Division::Beat, 4, BPM, RATE), 48_000);
    assert_eq!(to_grid(30_000, Division::Half, 4, BPM, RATE), 36_000);
    assert_eq!(to_grid(30_000, Division::Quarter, 4, BPM, RATE), 30_000);
    assert_eq!(to_grid(0, Division::Bar, 4, BPM, RATE), 0);
}

#[test]
fn quantize_frames_rounds_to_the_nearest_step() {
    // The parser's `snap=480` (a 10 ms grid at 48 kHz).
    assert_eq!(quantize_frames(48_213, 480), 48_000);
    assert_eq!(quantize_frames(48_213, 24_000), 48_000);
    assert_eq!(quantize_frames(36_000, 24_000), 48_000);
    assert_eq!(quantize_frames(35_999, 24_000), 24_000);
    assert_eq!(quantize_frames(0, 480), 0);
    assert_eq!(quantize_frames(487, 480), 480);
    // Half a step rounds up (never a silent no-op on the midpoint).
    assert_eq!(quantize_frames(240, 480), 480);
    // A frame at the top of the range saturates instead of wrapping to a
    // small one (which would seek to the start).
    assert_eq!(quantize_frames(u64::MAX, 2), u64::MAX - 1);
    assert_eq!(quantize_frames(u64::MAX, 1), u64::MAX);
}
