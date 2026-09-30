use super::*;

#[test]
fn the_same_seed_gives_the_same_bytes() {
    let mut a = TpdfDither::new(DITHER_SEED);
    let mut b = TpdfDither::new(DITHER_SEED);
    let mut x: Vec<f32> = (0..1_000).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
    let mut y = x.clone();
    a.quantize_s16(&mut x);
    b.quantize_s16(&mut y);
    assert_eq!(x, y, "a fixed seed is a reproducible export");
    // …and a different seed dithers differently (the stream is real).
    let mut c = TpdfDither::new(DITHER_SEED ^ 1);
    let mut z: Vec<f32> = (0..1_000).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
    c.quantize_s16(&mut z);
    assert_ne!(x, z);
}

#[test]
fn the_output_is_on_the_16_bit_grid() {
    let mut d = TpdfDither::new(DITHER_SEED);
    let mut x: Vec<f32> = (0..500).map(|i| (i as f32 * 0.07).sin() * 0.9).collect();
    d.quantize_s16(&mut x);
    for s in &x {
        let q = s * 32767.0;
        assert!(
            (q - q.round()).abs() < 1e-2,
            "{s} is not on the 16-bit grid ({q})"
        );
        assert!((-32767.0..=32767.0).contains(&q));
    }
}

/// The dither really is **triangular**: zero mean and a variance of `1/6` LSB²
/// (rectangular would be `1/12`, and no dither would be a spike at zero).
#[test]
fn the_dither_is_triangular() {
    let mut d = TpdfDither::new(DITHER_SEED);
    let n = 200_000;
    let mut sum = 0.0f64;
    let mut sumsq = 0.0f64;
    for _ in 0..n {
        let o = d.next_offset() as f64;
        sum += o;
        sumsq += o * o;
    }
    let mean = sum / n as f64;
    let var = sumsq / n as f64 - mean * mean;
    assert!(mean.abs() < 0.01, "zero mean, got {mean}");
    assert!(
        (var - 1.0 / 6.0).abs() < 0.01,
        "TPDF variance is 1/6, got {var}"
    );
    // The support is (-1, 1): no offset ever leaves it.
    assert!(((-1.0)..1.0).contains(&(d.next_offset() as f64)));
}

/// Quantising a dithered signal is **unbiased**: the error's mean over a long
/// run of a quiet constant is ~0 (a truncating quantiser would show a fixed
/// offset — the correlated distortion dither exists to remove).
#[test]
fn the_quantisation_error_has_no_offset() {
    let mut d = TpdfDither::new(DITHER_SEED);
    let value = 0.000_04f32; // far below one LSB (1/32767 ≈ 0.000_0305)
    let mut x = vec![value; 100_000];
    d.quantize_s16(&mut x);
    let mean: f64 = x.iter().map(|s| (*s - value) as f64).sum::<f64>() / x.len() as f64;
    assert!(
        mean.abs() < 1e-6,
        "the error averages out, got {mean} (a whole LSB is {})",
        1.0f64 / 32767.0
    );
    // …and the quantised values actually vary (it is dithering, not dropping).
    let distinct = x.iter().filter(|s| **s != x[0]).count();
    assert!(distinct > 0, "the dither moves samples across the grid");
}

/// **Full scale is not a hard clip.** A constant 1.0 must dither between the top two
/// codes (32766 and 32767), never sit clamped on 32767: a clamp is signal-correlated
/// error, which is the one thing dither exists to remove.
#[test]
fn full_scale_dithers_without_clamping() {
    let mut d = TpdfDither::new(DITHER_SEED);
    let mut x = vec![1.0f32; 40_000];
    d.quantize_s16(&mut x);
    assert!(
        x.iter().all(|s| *s <= 1.0),
        "nothing is written past full scale"
    );
    let top = x
        .iter()
        .filter(|s| (**s * 32767.0).round() == 32767.0)
        .count();
    let next = x
        .iter()
        .filter(|s| (**s * 32767.0).round() == 32766.0)
        .count();
    assert!(
        top > 0 && next > 0,
        "the top two codes both appear ({top} / {next}) — full scale dithers, it does not clamp"
    );
    // `round(32766 + d)` for `d` in (-1, 1) can also reach 32765; nothing else, and
    // never a clamp (the peak check already refused anything above 1.0).
    assert!(
        x.iter().all(|s| {
            let q = (*s * 32767.0).round();
            (32765.0..=32767.0).contains(&q)
        }),
        "no sample is pushed past the rails"
    );
}

/// Non-finite input is silenced rather than written as a NaN sample.
#[test]
fn non_finite_input_is_silenced() {
    let mut d = TpdfDither::new(DITHER_SEED);
    let mut x = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
    d.quantize_s16(&mut x);
    assert!(x.iter().all(|s| s.is_finite()));
    // 0.5 is not on the 16-bit grid (16384/32767 = 0.50001526); it must land on
    // the nearest grid point, within one LSB.
    assert!(
        (x[3] - 0.5).abs() <= 1.0 / 32767.0,
        "a finite value is quantised, not dropped: {}",
        x[3]
    );
}
