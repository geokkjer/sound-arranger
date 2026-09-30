use super::*;

/// A band-limited sine at `freq` Hz, `frames` long at `rate`.
fn tone(freq: f64, rate: u32, frames: usize, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| (std::f64::consts::TAU * freq * i as f64 / rate as f64).sin() as f32 * amplitude)
        .collect()
}

/// The energy of everything in `out` that is *not* a sinusoid at `freq`,
/// in dB relative to the total. The fit absorbs amplitude and phase, so this
/// measures droop-free distortion, not level.
fn residual_db(out: &[f32], rate: u32, freq: f64) -> f64 {
    let n = out.len() as f64;
    let (mut ss, mut sc, mut cc, mut ys, mut yc, mut total) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &y) in out.iter().enumerate() {
        let t = std::f64::consts::TAU * freq * i as f64 / rate as f64;
        let (s, c) = (t.sin(), t.cos());
        ss += s * s;
        sc += s * c;
        cc += c * c;
        ys += y as f64 * s;
        yc += y as f64 * c;
        total += (y as f64) * (y as f64);
    }
    // Solve the 2×2 normal equations for a·sin + b·cos.
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    let mut err = 0.0;
    for (i, &y) in out.iter().enumerate() {
        let t = std::f64::consts::TAU * freq * i as f64 / rate as f64;
        let fit = a * t.sin() + b * t.cos();
        err += (y as f64 - fit) * (y as f64 - fit);
    }
    assert!(total > 0.0, "no output energy");
    let _ = n;
    10.0 * (err / total).log10()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64).sqrt()
}

#[test]
fn equal_rates_are_a_bit_exact_passthrough() {
    let input = tone(440.0, 48_000, 1000, 0.7);
    let mut rs = Resampler::new(48_000, 48_000).expect("rates");
    assert!(rs.is_passthrough());
    let mut out = Vec::new();
    rs.process(&input[..400], &mut out);
    rs.process(&input[400..], &mut out);
    rs.flush(&mut out);
    assert_eq!(out, input, "equal rates must not touch the samples");
}

#[test]
fn forty_four_one_to_forty_eight_is_exactly_the_right_length() {
    let mut rs = Resampler::new(44_100, 48_000).expect("rates");
    assert_eq!(rs.frames_out(44_100), 48_000);
    assert_eq!(rs.frames_out(1), 1); // round(1.088)
    let input = tone(440.0, 44_100, 44_100, 0.5);
    let mut out = Vec::new();
    rs.process(&input, &mut out);
    assert!(
        out.len() < 48_000,
        "the tail needs the flush: {}",
        out.len()
    );
    rs.flush(&mut out);
    assert_eq!(out.len(), 48_000, "one second in is one second out");
    // Flush is idempotent and never produces extra frames.
    rs.flush(&mut out);
    assert_eq!(out.len(), 48_000);
}

#[test]
fn empty_input_produces_nothing() {
    let out = resample_mono(&[], 44_100, 48_000).expect("rates");
    assert!(out.is_empty());
}

#[test]
fn a_zero_rate_is_refused() {
    assert!(Resampler::new(0, 48_000).is_err());
    assert!(Resampler::new(48_000, 0).is_err());
}

#[test]
fn pitch_survives_the_conversion() {
    // 5 s of 440 Hz: the zero-crossing count is the cheapest pitch probe and
    // is exactly the assertion the drift compensator uses.
    let seconds = 5u32;
    let input = tone(440.0, 44_100, 44_100 * seconds as usize, 0.5);
    let out = resample_mono(&input, 44_100, 48_000).expect("rates");
    let crossings = out
        .windows(2)
        .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
        .count();
    let expected = 440.0 * seconds as f32 * 2.0;
    assert!(
        (crossings as f32 - expected).abs() < expected * 0.01,
        "pitch moved: {crossings} crossings vs ~{expected}"
    );
}

#[test]
fn a_constant_stays_constant() {
    // Unity DC gain per phase: this is what makes gain/DC survive. The very
    // first and last samples ring in/out of the centred kernel (silence
    // behind t = 0 and past the end), so the plateau is what must be exact.
    let input = vec![0.6f32; 44_100];
    let out = resample_mono(&input, 44_100, 48_000).expect("rates");
    for (i, v) in out.iter().enumerate().skip(TAPS).take(out.len() - 2 * TAPS) {
        assert!((v - 0.6).abs() < 1e-4, "sample {i} drifted: {v}");
    }
}

#[test]
fn the_passband_is_flat_and_distortion_free() {
    // 1 kHz: far from both Nyquists — the converter must be transparent.
    let input = tone(1000.0, 44_100, 22_050, 0.5); // 0.5 s = 500 whole cycles
    let out = resample_mono(&input, 44_100, 48_000).expect("rates");
    let db = residual_db(&out, 48_000, 1000.0);
    assert!(db < -60.0, "1 kHz residual {db:.1} dB is not transparent");

    // 20 kHz: the top of the passband, where linear interpolation would be
    // ≈ -6 dB and any imaging would show up as residual. Measure the plateau
    // (the ramps at the file's edges are not part of the passband claim).
    let input = tone(20_000.0, 44_100, 22_050, 0.5); // 10000 whole cycles
    let out = resample_mono(&input, 44_100, 48_000).expect("rates");
    let middle = &out[TAPS..out.len() - TAPS];
    let db = residual_db(middle, 48_000, 20_000.0);
    assert!(db < -40.0, "20 kHz residual {db:.1} dB — images or droop");
    // …and it is not attenuated into nothing.
    let amplitude = rms(middle) * std::f64::consts::SQRT_2;
    assert!(
        (amplitude - 0.5).abs() < 0.05,
        "20 kHz lost level: amplitude {amplitude:.3}"
    );
}

#[test]
fn downsampling_removes_content_above_the_new_nyquist() {
    // 30 kHz cannot survive 48 kHz (24 kHz Nyquist) — it must be filtered
    // out, not folded back as an alias. (48 → 44.1 kHz has no such room: its
    // stopband is 22.05–24 kHz and the input band ends at 24 kHz, so the
    // transition band covers all of it.)
    let input = tone(30_000.0, 96_000, 48_000, 0.5);
    let out = resample_mono(&input, 96_000, 48_000).expect("rates");
    assert_eq!(out.len(), 24_000);
    let middle = &out[TAPS..out.len() - TAPS];
    let ratio = rms(middle) / rms(&input);
    assert!(
        ratio < 0.05,
        "alias not filtered: output is {ratio:.4} of the input"
    );
}

#[test]
fn a_downsample_keeps_what_fits() {
    // 1 kHz survives 48 → 44.1 kHz at full level.
    let input = tone(1000.0, 48_000, 24_000, 0.5);
    let out = resample_mono(&input, 48_000, 44_100).expect("rates");
    let middle = &out[TAPS..out.len() - TAPS];
    let amplitude = rms(middle) * std::f64::consts::SQRT_2;
    assert!(
        (amplitude - 0.5).abs() < 0.02,
        "level changed: {amplitude:.3}"
    );
    assert!(residual_db(middle, 44_100, 1000.0) < -60.0);
}

#[test]
fn the_block_size_does_not_change_the_output() {
    let input = tone(3000.0, 44_100, 30_000, 0.8);

    let mut one = Vec::new();
    let mut rs = Resampler::new(44_100, 48_000).expect("rates");
    rs.process(&input, &mut one);
    rs.flush(&mut one);

    let mut many = Vec::new();
    let mut rs = Resampler::new(44_100, 48_000).expect("rates");
    for chunk in input.chunks(7) {
        rs.process(chunk, &mut many);
    }
    rs.flush(&mut many);

    assert_eq!(one.len(), many.len());
    assert_eq!(
        one, many,
        "chunking changed the conversion (not deterministic)"
    );
}

#[test]
fn rates_other_than_the_common_pair_work() {
    // 48 kHz → 96 kHz (an integer ratio) and 22.05 → 48 kHz.
    let out = resample_mono(&tone(1000.0, 48_000, 4800, 0.5), 48_000, 96_000).expect("rates");
    assert_eq!(out.len(), 9600);
    assert!(residual_db(&out, 96_000, 1000.0) < -60.0);

    let out = resample_mono(&tone(1000.0, 22_050, 22_050, 0.5), 22_050, 48_000).expect("rates");
    assert_eq!(out.len(), 48_000);
    assert!(residual_db(&out, 48_000, 1000.0) < -60.0);
}

/// Not a correctness test — the measured rate, which is what decides that
/// importing a long take is a pause and not a problem:
/// `cargo test -p media --release -- --ignored --nocapture throughput`.
#[test]
#[ignore]
fn throughput() {
    let seconds = 60usize;
    let input = tone(440.0, 44_100, 44_100 * seconds, 0.5);
    let start = std::time::Instant::now();
    let out = resample_mono(&input, 44_100, 48_000).expect("rates");
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(out.len(), 48_000 * seconds);
    println!(
        "resample: {seconds} s of 44.1→48 kHz mono in {elapsed:.2} s ({:.0}× realtime)",
        seconds as f64 / elapsed
    );
}
