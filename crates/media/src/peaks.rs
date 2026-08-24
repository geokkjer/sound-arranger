//! The live peak pyramid (P1.2; Audacity's structure per the prior-art
//! research): min/max per **256-sample base bin**, accumulated incrementally
//! as samples arrive (the "live" part — the recorder's write side feeds it),
//! upper levels (min/max over 2× bins per level) computed at finalize, and a
//! persisted per-source sidecar (`.peaks`). The UI reads the sidecar for
//! waveform drawing at any zoom; the base level alone is the live view.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Samples per base bin (the Audacity "reduction" granularity).
pub const PEAK_BASE_BIN: usize = 256;
/// Number of pyramid levels: level k covers 2^k base bins (level 9 ≈ 2.7 s of
/// audio at 48 kHz — one bin per wide zoom).
pub const PEAK_LEVELS: usize = 10;

const PEAK_MAGIC: &[u8; 4] = b"SPK1";

/// Accumulates the pyramid. The recorder's write path calls [`PeakBuilder::push`]
/// per written chunk; the base level is always current, upper levels are
/// derived at [`PeakBuilder::finalize`] (or on demand).
pub struct PeakBuilder {
    base_min: Vec<f32>,
    base_max: Vec<f32>,
    cur_min: f32,
    cur_max: f32,
    cur_count: usize,
    frames: u64,
    finalized: bool,
}

impl PeakBuilder {
    pub fn new() -> Self {
        PeakBuilder {
            base_min: Vec::new(),
            base_max: Vec::new(),
            cur_min: 0.0,
            cur_max: 0.0,
            cur_count: 0,
            frames: 0,
            finalized: false,
        }
    }

    /// Feed samples (called off the audio path — the recorder's write side).
    /// Base bins complete as 256 samples accumulate.
    pub fn push(&mut self, samples: &[f32]) {
        assert!(!self.finalized, "peaks: push after finalize");
        for &s in samples {
            if self.cur_count == 0 {
                self.cur_min = s;
                self.cur_max = s;
            } else {
                self.cur_min = self.cur_min.min(s);
                self.cur_max = self.cur_max.max(s);
            }
            self.cur_count += 1;
            if self.cur_count == PEAK_BASE_BIN {
                self.base_min.push(self.cur_min);
                self.base_max.push(self.cur_max);
                self.cur_count = 0;
            }
        }
        self.frames += samples.len() as u64;
    }

    /// Total frames fed.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Completed base bins (the final partial bin is kept in the accumulator
    /// and included if [`PeakBuilder::finalize`] is called).
    pub fn base_bins(&self) -> usize {
        self.base_min.len() + usize::from(self.cur_count > 0)
    }

    /// Min/max of base bin `bin`.
    /// Min/max of a base bin; `None` for an out-of-range bin (a UI zoom query can
    /// hit one). A genuine bin of silence returns `Some((0.0, 0.0))` — silence is
    /// data; an error sentinel must not be representable as data (kimi must-fix).
    pub fn base_minmax(&self, bin: usize) -> Option<(f32, f32)> {
        if bin < self.base_min.len() {
            Some((self.base_min[bin], self.base_max[bin]))
        } else if bin == self.base_min.len() && self.cur_count > 0 {
            Some((self.cur_min, self.cur_max))
        } else {
            None
        }
    }

    /// Flush the partial bin and compute the upper levels. Idempotent.
    pub fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        if self.cur_count > 0 {
            self.base_min.push(self.cur_min);
            self.base_max.push(self.cur_max);
            self.cur_count = 0;
        }
        self.finalized = true;
    }

    /// Upper levels: level k holds min/max over 2^k base bins — **level 0 is
    /// the base itself**, matching the sidecar format (kimi review finding 2).
    pub fn levels(&self) -> Vec<PeakLevel> {
        let mut out = Vec::with_capacity(PEAK_LEVELS);
        out.push((self.base_min.clone(), self.base_max.clone()));
        let mut level_min = self.base_min.clone();
        let mut level_max = self.base_max.clone();
        for _ in 1..PEAK_LEVELS {
            let (mut next_min, mut next_max) = (Vec::new(), Vec::new());
            for pair in level_min.chunks(2) {
                next_min.push(pair.iter().copied().fold(f32::INFINITY, f32::min));
            }
            for pair in level_max.chunks(2) {
                next_max.push(pair.iter().copied().fold(f32::NEG_INFINITY, f32::max));
            }
            out.push((next_min.clone(), next_max.clone()));
            level_min = next_min;
            level_max = next_max;
        }
        out
    }

    /// Min/max over the frame range `[start, end)`, walking base bins
    /// (correct for any range; the zoom-out fast path via the levels is a UI
    /// refinement).
    pub fn range_minmax(&self, start: u64, end: u64) -> Option<(f32, f32)> {
        // bounds-safe: None for an empty/invalid range or no data (never panic).
        if start >= end || end > self.frames || self.base_bins() == 0 {
            return None;
        }
        let b0 = (start / PEAK_BASE_BIN as u64) as usize;
        let b1 = ((end - 1) / PEAK_BASE_BIN as u64) as usize;
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for b in b0..=b1.min(self.base_bins() - 1) {
            if let Some((lo, hi)) = self.base_minmax(b) {
                mn = mn.min(lo);
                mx = mx.max(hi);
            }
        }
        Some((mn, mx))
    }
}

impl Default for PeakBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// One pyramid level's min/max arrays.
pub type PeakLevel = (Vec<f32>, Vec<f32>);
/// The parsed sidecar: (base_bin, levels, frames, sample_rate, per-level data).
pub type PeakData = (u32, usize, u64, u32, Vec<PeakLevel>);

/// The `.peaks` sidecar: magic + header + per-level min/max pairs.
pub struct PeakFile;

impl PeakFile {
    /// Write the sidecar (16-bit-quantized min/max would suffice, but the
    /// pool keeps float — store f32 pairs). Finalizes the builder. **Atomic:** writes
    /// to `${path}.tmp` then renames, so "exists ⇒ valid" (kimi pool must-fix 2).
    pub fn write(path: &Path, builder: &mut PeakBuilder, sample_rate: u32) -> Result<(), String> {
        builder.finalize();
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        let write = || -> Result<(), String> {
            let file = File::create(&tmp).map_err(|e| format!("peaks create {}: {e}", tmp.display()))?;
            let mut w = BufWriter::new(file);
            w.write_all(PEAK_MAGIC).map_err(|e| e.to_string())?;
            w.write_all(&(PEAK_BASE_BIN as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            w.write_all(&(PEAK_LEVELS as u8).to_le_bytes()).map_err(|e| e.to_string())?;
            w.write_all(&builder.frames().to_le_bytes()).map_err(|e| e.to_string())?;
            w.write_all(&sample_rate.to_le_bytes()).map_err(|e| e.to_string())?;
            let mut level_min = builder.base_min.clone();
            let mut level_max = builder.base_max.clone();
            for _ in 0..PEAK_LEVELS {
                write_level(&mut w, &level_min, &level_max)?;
                level_min = pair_min(&level_min);
                level_max = pair_max(&level_max);
            }
            w.flush().map_err(|e| e.to_string())?;
            Ok(())
        };
        let r = write();
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return r;
        }
        std::fs::rename(&tmp, path).map_err(|e| format!("peaks rename {}: {e}", path.display()))
    }

    /// Read the sidecar: (base_bin, levels, frames, sample_rate, per-level
    /// min/max arrays).
    pub fn read(path: &Path) -> Result<PeakData, String> {
        let mut f = File::open(path).map_err(|e| format!("peaks open {}: {e}", path.display()))?;
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic).map_err(|e| e.to_string())?;
        if &magic != PEAK_MAGIC {
            return Err("not a peaks file".into());
        }
        let mut u32b = [0u8; 4];
        let mut u64b = [0u8; 8];
        f.read_exact(&mut u32b).map_err(|e| e.to_string())?;
        let base_bin = u32::from_le_bytes(u32b);
        let mut level_byte = [0u8; 1];
        f.read_exact(&mut level_byte).map_err(|e| e.to_string())?;
        f.read_exact(&mut u64b).map_err(|e| e.to_string())?;
        let frames = u64::from_le_bytes(u64b);
        f.read_exact(&mut u32b).map_err(|e| e.to_string())?;
        let sample_rate = u32::from_le_bytes(u32b);
        // Bound the per-level allocation: a valid sidecar's largest level (base) has
        // ~frames/base_bin bins, and higher levels keep halving. A corrupt/hostile
        // sidecar claiming billions of bins must fail loud, not allocate gigabytes
        // (kimi pool should-fix 4).
        let max_bins = (frames as usize).div_ceil(PEAK_BASE_BIN).max(1);
        let levels = (level_byte[0] as usize).min(PEAK_LEVELS);
        let mut out = Vec::with_capacity(levels);
        for _ in 0..levels {
            f.read_exact(&mut u32b).map_err(|e| e.to_string())?;
            let n = u32::from_le_bytes(u32b) as usize;
            if n > max_bins {
                return Err(format!("peaks level length {n} exceeds the source's frame count"));
            }
            let mut mn = vec![0.0f32; n];
            let mut mx = vec![0.0f32; n];
            for v in mn.iter_mut() {
                f.read_exact(&mut u32b).map_err(|e| e.to_string())?;
                *v = f32::from_bits(u32::from_le_bytes(u32b));
            }
            for v in mx.iter_mut() {
                f.read_exact(&mut u32b).map_err(|e| e.to_string())?;
                *v = f32::from_bits(u32::from_le_bytes(u32b));
            }
            out.push((mn, mx));
        }
        Ok((base_bin, levels, frames, sample_rate, out))
    }
}

fn write_level(w: &mut impl Write, mn: &[f32], mx: &[f32]) -> Result<(), String> {
    w.write_all(&(mn.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    for v in mn {
        w.write_all(&v.to_bits().to_le_bytes()).map_err(|e| e.to_string())?;
    }
    for v in mx {
        w.write_all(&v.to_bits().to_le_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pair_min(v: &[f32]) -> Vec<f32> {
    v.chunks(2).map(|p| p.iter().copied().fold(f32::INFINITY, f32::min)).collect()
}

fn pair_max(v: &[f32]) -> Vec<f32> {
    v.chunks(2).map(|p| p.iter().copied().fold(f32::NEG_INFINITY, f32::max)).collect()
}

#[cfg(test)]
mod tests {
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
            let Some((lo, hi)) = b.base_minmax(bin) else { panic!("bin {bin} should exist") };
            assert!((lo - expect_min).abs() < 1e-7, "bin {bin} min {lo} vs {expect_min}");
            assert!((hi - expect_max).abs() < 1e-7, "bin {bin} max {hi} vs {expect_max}");
        }
    }

    #[test]
    fn range_query_covers_the_window() {
        let mut b = PeakBuilder::new();
        let mut phase = 0.0f64;
        b.push(&sine_block(440.0, 48_000, 4096, &mut phase));
        let (lo, hi) = b.range_minmax(1000, 2000).unwrap();
        assert!(lo < 0.0 && hi > 0.0, "a sine crosses zero in any window: {lo}..{hi}");
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
                assert!(levels[k + 1].0[i] <= mn[i * 2], "level {} min must be <= level {k}", k + 1);
                assert!(levels[k + 1].1[i] >= mx[i * 2], "level {} max must be >= level {k}", k + 1);
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
}
