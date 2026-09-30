use super::*;

/// Render `input` through a stretcher in one shot (the offline shape).
fn render(input: &[f32], num: u32, den: u32) -> Vec<f32> {
    let mut s = Stretch::for_len(num, den, input.len() as u64).expect("stretcher");
    let mut out = Vec::new();
    s.process(input, &mut out);
    s.flush(&mut out);
    out
}

fn tone(frames: usize, rate: u32, hz: f64) -> Vec<f32> {
    (0..frames)
        .map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.5)
        .collect()
}

fn crossings(audio: &[f32]) -> usize {
    audio
        .windows(2)
        .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
        .count()
}

/// The ratio decides the length: a 2× stretch is twice as long (within one
/// synthesis hop, which is the grain of an overlap-add transform), and a 3/4
/// stretch is three quarters.
#[test]
fn the_ratio_decides_the_length() {
    let input = tone(48_000, 48_000, 440.0);
    let hop = Stretch::DEFAULT_HOP as i64;
    // The output is window-aligned: `blocks × hop + hop`, where `blocks =
    // ceil(input / hop_in)`. That is the honest grain of an overlap-add transform —
    // the host logs the frames it actually wrote, so nothing downstream assumes
    // `input × num / den` to the frame.
    for (num, den) in [(2u32, 1u32), (3, 4), (1, 2), (5, 4)] {
        let out = render(&input, num, den);
        let expect = input.len() as f64 * num as f64 / den as f64;
        assert!(
            (out.len() as f64 - expect).abs() <= 2.0 * hop as f64,
            "{num}/{den} should be ~{expect:.0}, got {}",
            out.len()
        );
    }
}

/// **Pitch is preserved**: that is the whole point of a time-stretch, and a
/// zero-crossing count is the countable property (the resampler's tests use it).
#[test]
fn a_tone_keeps_its_pitch_while_the_length_changes() {
    let rate = 48_000u32;
    let input = tone(rate as usize, rate, 440.0);
    for (num, den) in [(2u32, 1u32), (1, 2), (3, 4), (5, 4)] {
        let out = render(&input, num, den);
        let seconds = out.len() as f64 / rate as f64;
        let expect = 2.0 * 440.0 * seconds;
        let got = crossings(&out) as f64;
        assert!(
            (got - expect).abs() <= expect * 0.05 + 4.0,
            "{num}/{den}: {got} crossings over {seconds:.3} s, expected ~{expect:.0}"
        );
    }
}

/// A constant stays constant: the periodic Hann at 50 % overlap sums to exactly 1,
/// so a stretch must not introduce ripple (and a DC offset must not beat against
/// the hop).
#[test]
fn a_constant_stays_constant() {
    let input = vec![0.25f32; 24_000];
    let out = render(&input, 3, 2);
    let middle = &out[2_048..out.len() - 2_048];
    let worst = middle
        .iter()
        .map(|s| (s - 0.25).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-3, "ripple {worst} in a stretched constant");
}

/// **A transient stays where it was** (within a hop): WSOLA aligns by correlation,
/// so a click does not smear across the output the way a phase vocoder would.
#[test]
fn a_transient_stays_put() {
    let at = 20_000usize;
    let mut input = vec![0.0f32; 48_000];
    input[at] = 1.0;
    input[at + 1] = -1.0;
    let ratio = 2.0;
    let out = render(&input, 2, 1);
    let expect = (at as f64 * ratio) as usize;
    // Within the block grid (one hop) plus the search radius: the click is read by
    // whichever block's window covers it, at an offset the correlation chose.
    let hop = Stretch::DEFAULT_HOP + 2 * (Stretch::DEFAULT_HOP / 4);
    let peak = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).expect("finite"))
        .map(|(i, _)| i)
        .expect("a peak");
    assert!(
        (peak as i64 - expect as i64).abs() <= hop as i64,
        "the click landed at {peak}, expected ~{expect} (±{hop})"
    );
}

/// The transform is **byte-reproducible**: the same input and ratio produce the
/// same bytes (the property the log's `u32/u32` ratio exists for).
#[test]
fn the_same_input_and_ratio_reproduce_the_same_bytes() {
    let input = tone(9_600, 48_000, 220.0);
    let a = render(&input, 7, 5);
    let b = render(&input, 7, 5);
    assert_eq!(a.len(), b.len());
    assert!(
        a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
        "a stretch must be deterministic"
    );
    // …and the ratio is a rational, so it is *logged* exactly rather than as an
    // f32 that might print differently on another platform.
    assert_eq!((7u32, 5u32), (7, 5));
}

/// **A short region is sized to itself** (`for_len`) and then stretched exactly: a
/// constant comes back constant, and the length is the ratio's. The gate reproduced
/// the first draft emitting material followed by silence here — the default hop's
/// window was longer than the region, so there was nothing to overlap. That case is
/// now an explicit contract (`min_region_frames`): a caller that hands a default
/// stretcher a shorter region gets a *placement*, and the host refuses instead.
#[test]
fn a_short_region_is_stretched_when_the_window_fits_it() {
    let input = vec![0.5f32; 20];
    for (num, den) in [(2u32, 1u32), (1, 2), (1, 4), (3, 2)] {
        let mut s = Stretch::for_len(num, den, 20).expect("stretcher");
        assert!(
            s.min_region_frames() <= input.len() as u64,
            "{num}/{den}: the window must fit the region"
        );
        let mut out = Vec::new();
        s.process(&input, &mut out);
        s.flush(&mut out);
        let expect = input.len() as f64 * num as f64 / den as f64;
        assert!(
            (out.len() as f64 - expect).abs() <= 8.0,
            "{num}/{den}: expected ~{expect:.0} frames, got {}",
            out.len()
        );
        assert!(
            out.iter().all(|v| (*v - 0.5).abs() < 1e-3),
            "{num}/{den} must not trail into silence or fade: {:?}",
            &out[out.len().saturating_sub(6)..]
        );
    }

    // A default stretcher's window is what it says it is (the contract the host
    // checks before rendering a very short clip).
    let big = Stretch::new(2, 1).expect("stretcher");
    assert_eq!(big.min_region_frames(), 2 * Stretch::DEFAULT_HOP as u64);
    assert!(big.min_region_frames() > input.len() as u64);
}

/// **Streaming equals one shot, byte for byte**: the caller may feed a region in
/// whatever chunks it reads, and the output cannot depend on the chunking.
#[test]
fn streaming_in_chunks_matches_one_shot() {
    let input = tone(20_000, 48_000, 330.0);
    let one = render(&input, 5, 3);

    let mut s = Stretch::for_len(5, 3, input.len() as u64).expect("stretcher");
    let mut chunked = Vec::new();
    for chunk in input.chunks(37) {
        s.process(chunk, &mut chunked);
    }
    s.flush(&mut chunked);

    assert_eq!(one.len(), chunked.len(), "the lengths must agree");
    assert!(
        one.iter()
            .zip(&chunked)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "the bytes must agree whatever the chunking"
    );
}

/// A ratio that only *rounds* to 1:1 is not an identity: the caller must stretch,
/// not copy (the gate found `hop_in == hop_out` reporting `2049/2048` as identity —
/// a silent copy under a logged op that claims a stretch).
#[test]
fn a_near_identity_ratio_is_not_an_identity() {
    assert!(Stretch::new(1, 1).expect("1:1").is_identity());
    assert!(Stretch::new(4, 4).expect("4:4").is_identity());
    assert!(!Stretch::new(2049, 2048).expect("fine").is_identity());
    assert!(!Stretch::new(129, 128).expect("fine").is_identity());
    assert!(!Stretch::new(2, 1).expect("double").is_identity());
    assert!(!Stretch::new(1, 2).expect("half").is_identity());
    // The ratio as the log will carry it: verbatim, unreduced (its value *is* the
    // record of what was asked for).
    assert_eq!(Stretch::new(90, 120).expect("tempo").rational(), (90, 120));
}

/// Guards: a zero ratio is refused, a one-frame input still yields output, and an
/// identity ratio reports itself so the caller can copy bit-exactly.
#[test]
fn guards_and_edge_inputs() {
    assert!(Stretch::new(0, 1).is_err());
    assert!(Stretch::new(1, 0).is_err());
    assert!(Stretch::new(1, 1).expect("1:1").is_identity());

    let tiny = render(&[0.5], 2, 1);
    assert!(!tiny.is_empty(), "a one-frame input still stretches");
    let silent = render(&vec![0.0f32; 100], 2, 1);
    assert!(
        silent.iter().all(|s| *s == 0.0),
        "silence stretches to silence (no NaN from the search's energy guard)"
    );
}
