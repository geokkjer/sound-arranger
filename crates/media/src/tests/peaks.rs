use super::*;

fn sine_block(freq: f32, sr: u32, n: usize, phase: &mut f64) -> Vec<f32> {
    (0..n)
        .map(|_| {
            let s = (std::f64::consts::TAU * *phase).sin() as f32 * 0.5;
            *phase += freq as f64 / sr as f64;
            s
        })
        .collect()
}

#[test]
fn base_bins_match_independent_computation() {
    let mut b = PeakBuilder::new();
    let mut phase = 0.0f64;
    let samples = sine_block(440.0, 48_000, 10 * PEAK_BASE_BIN + 100, &mut phase);
    b.push(&samples);
    assert_eq!(b.base_bins(), 11); // 10 full + 1 partial
    for bin in 0..10 {
        let window = &samples[bin * PEAK_BASE_BIN..(bin + 1) * PEAK_BASE_BIN];
        let expect_min = window.iter().copied().fold(f32::INFINITY, f32::min);
        let expect_max = window.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let Some((lo, hi)) = b.base_minmax(bin) else {
            panic!("bin {bin} should exist")
        };
        assert!(
            (lo - expect_min).abs() < 1e-7,
            "bin {bin} min {lo} vs {expect_min}"
        );
        assert!(
            (hi - expect_max).abs() < 1e-7,
            "bin {bin} max {hi} vs {expect_max}"
        );
    }
}

#[test]
fn range_query_covers_the_window() {
    let mut b = PeakBuilder::new();
    let mut phase = 0.0f64;
    b.push(&sine_block(440.0, 48_000, 4096, &mut phase));
    let (lo, hi) = b.range_minmax(1000, 2000).unwrap();
    assert!(
        lo < 0.0 && hi > 0.0,
        "a sine crosses zero in any window: {lo}..{hi}"
    );
}

#[test]
fn upper_levels_are_monotone_coarser() {
    let mut b = PeakBuilder::new();
    let mut phase = 0.0f64;
    b.push(&sine_block(440.0, 48_000, 4096, &mut phase));
    let levels = b.levels();
    // level k+1 min <= level k min over the same footprint (never tighter)
    for (k, (mn, mx)) in levels.iter().enumerate().take(PEAK_LEVELS - 1) {
        for i in 0..levels[k + 1].0.len() {
            assert!(
                levels[k + 1].0[i] <= mn[i * 2],
                "level {} min must be <= level {k}",
                k + 1
            );
            assert!(
                levels[k + 1].1[i] >= mx[i * 2],
                "level {} max must be >= level {k}",
                k + 1
            );
        }
    }
}

#[test]
fn sidecar_roundtrips() {
    let path = std::env::temp_dir().join(format!("media-peaks-{}.spk", std::process::id()));
    let mut b = PeakBuilder::new();
    let mut phase = 0.0f64;
    b.push(&sine_block(440.0, 48_000, 5120, &mut phase));
    PeakFile::write(&path, &mut b, 48_000).unwrap();
    let (base_bin, levels, frames, sr, data) = PeakFile::read(&path).unwrap();
    assert_eq!(base_bin, PEAK_BASE_BIN as u32);
    assert_eq!(levels, PEAK_LEVELS);
    assert_eq!(frames, 5120);
    assert_eq!(sr, 48_000);
    assert_eq!(data[0].0.len(), 20); // 5120/256 = 20 base bins
    assert_eq!(data[1].0.len(), 10);
    // base level min/max match the builder
    assert_eq!(data[0].0[0], b.base_minmax(0).unwrap().0);
    assert_eq!(data[0].1[0], b.base_minmax(0).unwrap().1);
    // levels() indexing matches the sidecar: level k is identical data
    // (kimi review finding 2 — regression guard).
    let api = b.levels();
    assert_eq!(api.len(), data.len());
    for k in 0..data.len() {
        assert_eq!(api[k].0, data[k].0, "level {k} min");
        assert_eq!(api[k].1, data[k].1, "level {k} max");
    }
    let _ = std::fs::remove_file(&path);
}
