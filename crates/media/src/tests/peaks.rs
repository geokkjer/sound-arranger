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

/// The read side answers what the write side holds: a `Peaks` loaded from a sidecar
/// returns exactly the builder's base-level query over the same range, and a coarser
/// level's answer **contains** it (a bin is a superset of the frames it covers).
///
/// This is the equivalence that lets a shell draw from the persisted pyramid instead of
/// re-reading the WAV — if the two disagreed, a shell would be drawing a different piece
/// from the one that plays.
#[test]
fn loaded_peaks_answer_like_the_builder() {
    let path = std::env::temp_dir().join(format!("media-peaks-load-{}.spk", std::process::id()));
    let mut b = PeakBuilder::new();
    let mut phase = 0.0f64;
    // A signal whose amplitude walks, so different windows have different extremes.
    let mut samples = Vec::new();
    for block in 0..40 {
        let amp = 0.1 + 0.02 * block as f32;
        samples.extend(
            sine_block(220.0, 48_000, 1024, &mut phase)
                .into_iter()
                .map(|s| s * amp * 2.0),
        );
    }
    b.push(&samples);
    PeakFile::write(&path, &mut b, 48_000).unwrap();
    let peaks = PeakFile::load(&path).unwrap();

    assert_eq!(peaks.frames(), samples.len() as u64);
    assert_eq!(peaks.sample_rate(), 48_000);
    assert_eq!(peaks.base_bin(), PEAK_BASE_BIN as u64);

    for (start, end) in [(0, 4096), (1000, 2000), (256, 257), (40_000, 40_960)] {
        let builder = b.range_minmax(start, end);
        let loaded = peaks.range_minmax(start, end);
        assert_eq!(
            builder.map(|(lo, hi)| (lo.to_bits(), hi.to_bits())),
            loaded.map(|(lo, hi)| (lo.to_bits(), hi.to_bits())),
            "base-level query {start}..{end} must match the builder"
        );
        // A coarser level aggregates whole bins: never narrower than the truth.
        let coarse = peaks.range_minmax_at(peaks.levels() - 1, start, end);
        let (lo, hi) = coarse.expect("a coarse level covers the whole source");
        let (blo, bhi) = builder.expect("the range is inside the data");
        assert!(
            lo <= blo && hi >= bhi,
            "level coarse must contain {blo}..{bhi}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

/// The level a column draws from is chosen so a column costs one or two bins, never
/// `frames_per_column` frames — the property that keeps the canvas bounded by pixels.
#[test]
fn level_selection_gives_at_most_a_couple_of_bins_per_column() {
    let path = std::env::temp_dir().join(format!("media-peaks-levels-{}.spk", std::process::id()));
    let mut b = PeakBuilder::new();
    let mut phase = 0.0f64;
    b.push(&sine_block(440.0, 48_000, 48_000 * 30, &mut phase)); // 30 s
    PeakFile::write(&path, &mut b, 48_000).unwrap();
    let peaks = PeakFile::load(&path).unwrap();

    for per_column in [256u64, 512, 4096, 48_000, 480_000] {
        let level = peaks.level_for(per_column);
        let bin = peaks.bin_frames(level);
        assert!(
            bin <= per_column,
            "level {level} ({bin} frames/bin) must not be finer than a {per_column}-frame column"
        );
        let bins = (48_000u64.div_ceil(bin)).max(1);
        assert!(
            bins <= 256,
            "a 1 s query at {per_column} frames/column read {bins} bins"
        );
        // …and the chosen level is as coarse as the pyramid can be for that column.
        if level + 1 < peaks.levels() && !peaks.levels[level + 1].0.is_empty() {
            assert!(
                peaks.bin_frames(level + 1) > per_column,
                "level {} would still be no finer than the column",
                level + 1
            );
        }
    }
}

/// A query a drawing can make at the edges of the data, or through a missing sidecar,
/// is `None` — never a panic and never a fabricated sample.
#[test]
fn out_of_range_and_missing_sidecars_do_not_panic() {
    let missing = std::env::temp_dir().join("media-peaks-does-not-exist.spk");
    let _ = std::fs::remove_file(&missing);
    assert!(PeakFile::load(&missing).is_err());

    let path = std::env::temp_dir().join(format!("media-peaks-edges-{}.spk", std::process::id()));
    let mut b = PeakBuilder::new();
    let mut phase = 0.0f64;
    b.push(&sine_block(440.0, 48_000, 300, &mut phase)); // shorter than one base bin
    PeakFile::write(&path, &mut b, 48_000).unwrap();
    let peaks = PeakFile::load(&path).unwrap();

    assert!(peaks.range_minmax(0, 300).is_some());
    assert!(peaks.range_minmax(0, 301).is_none(), "past the data");
    assert!(peaks.range_minmax(10, 10).is_none(), "empty range");
    assert!(peaks.range_minmax(20, 10).is_none(), "reversed range");
    // Every level is answerable, including the coarse ones of a source shorter than
    // one of their bins: the query clamps to the bins the level actually has.
    for level in 0..peaks.levels() {
        assert!(
            peaks.range_minmax_at(level, 0, 300).is_some(),
            "level {level} of a 300-frame source"
        );
    }
    let _ = std::fs::remove_file(&path);
}
