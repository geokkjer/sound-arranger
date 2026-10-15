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

/// The **read side** of the pyramid: one source's persisted `.peaks`, with the
/// envelope query a UI draws with.
///
/// [`PeakBuilder`] is the write side — it accumulates min/max as samples arrive, which
/// is what the recorder feeds. A shell that draws a source which already exists reads
/// the sidecar instead of re-reading the WAV: the pyramid is persisted exactly so the
/// UI does not have to. (The TUI spike rebuilds a builder from the WAV; that works, and
/// reads every sample. This is the cheaper route the sidecar exists for.)
///
/// The query that matters is **level-aware**. Base-level [`Peaks::range_minmax`] walks
/// one bin per [`PEAK_BASE_BIN`] frames, so one whole-piece column would walk the whole
/// source. [`Peaks::level_for`] picks the pyramid level whose bins are no finer than a
/// column, so a column costs one or two bins at any zoom — the difference between a
/// canvas that scales with pixels and one that scales with samples.
pub struct Peaks {
    /// Frames per **base** bin, from the sidecar header — carried in the format, not
    /// assumed to equal [`PEAK_BASE_BIN`], so a future writer can change it.
    base_bin: u64,
    frames: u64,
    sample_rate: u32,
    /// Level k's bins each cover `base_bin << k` frames; level 0 is the base.
    levels: Vec<PeakLevel>,
}

impl Peaks {
    /// Total frames the pyramid covers.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The rate the sidecar was written at.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frames per base bin.
    pub fn base_bin(&self) -> u64 {
        self.base_bin.max(1)
    }

    /// How many pyramid levels the sidecar carries.
    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    /// Frames each bin of `level` covers, with `level` pulled back to a level that
    /// carries data (a short source's coarse levels are empty).
    pub fn bin_frames(&self, level: usize) -> u64 {
        self.raw_bin_frames(self.clamp_level(level))
    }

    /// The finest level whose bins are **no finer than** `frames_per_column` — i.e. the
    /// coarsest level that still gives at least one bin per column, so a column reads
    /// one or two bins rather than `frames_per_column` frames.
    ///
    /// Below the base bin this is level 0: the pyramid cannot honestly serve a column
    /// narrower than a bin (it would repeat the same bin and claim detail the data does
    /// not have), so the caller's viewport must not ask — see the iced shell's
    /// `MIN_FRAMES_PER_POINT`.
    pub fn level_for(&self, frames_per_column: u64) -> usize {
        let per = frames_per_column.max(1);
        let mut level = 0;
        while level + 1 < self.levels() && self.raw_bin_frames(level + 1) <= per {
            level += 1;
        }
        self.clamp_level(level)
    }

    /// Min/max over the frame range `[start, end)`, read from `level`'s bins.
    ///
    /// `None` for an empty or out-of-data range, and for a level with no data: a UI
    /// query can hit both, and neither is a panic. The bins a range touches are a
    /// superset of it, so the answer is **at least** as wide as the true extremes of the
    /// range and never narrower — the honest direction for a drawing.
    pub fn range_minmax_at(&self, level: usize, start: u64, end: u64) -> Option<(f32, f32)> {
        if start >= end || end > self.frames {
            return None;
        }
        let (mins, maxs) = self.levels.get(self.clamp_level(level))?;
        if mins.is_empty() {
            return None;
        }
        let bin = self.bin_frames(level);
        // A well-formed sidecar covers its whole frame count at every level it carries,
        // so `b0` lands in range; the clamp is for a truncated or hand-edited one.
        let last = mins.len() - 1;
        let b0 = ((start / bin) as usize).min(last);
        let b1 = (((end - 1) / bin) as usize).min(last);
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for bin in b0..=b1 {
            mn = mn.min(mins[bin]);
            mx = mx.max(maxs[bin]);
        }
        Some((mn, mx))
    }

    /// Min/max over `[start, end)` at the **base** level — the same query, and the same
    /// answer, as a finalized [`PeakBuilder::range_minmax`] over the same range.
    pub fn range_minmax(&self, start: u64, end: u64) -> Option<(f32, f32)> {
        self.range_minmax_at(0, start, end)
    }

    /// Frames per bin at `level` **without** the empty-level fallback, for probing which
    /// levels the pyramid actually carries.
    fn raw_bin_frames(&self, level: usize) -> u64 {
        self.base_bin()
            .checked_shl(level.min(63) as u32)
            .unwrap_or(u64::MAX)
    }

    /// `level`, pulled back to the deepest level that carries data — the coarse levels
    /// of a short source halve away to nothing.
    fn clamp_level(&self, level: usize) -> usize {
        let mut level = level.min(self.levels.len().saturating_sub(1));
        while level > 0 && self.levels[level].0.is_empty() {
            level -= 1;
        }
        level
    }
}

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
            let file =
                File::create(&tmp).map_err(|e| format!("peaks create {}: {e}", tmp.display()))?;
            let mut w = BufWriter::new(file);
            w.write_all(PEAK_MAGIC).map_err(|e| e.to_string())?;
            w.write_all(&(PEAK_BASE_BIN as u32).to_le_bytes())
                .map_err(|e| e.to_string())?;
            w.write_all(&(PEAK_LEVELS as u8).to_le_bytes())
                .map_err(|e| e.to_string())?;
            w.write_all(&builder.frames().to_le_bytes())
                .map_err(|e| e.to_string())?;
            w.write_all(&sample_rate.to_le_bytes())
                .map_err(|e| e.to_string())?;
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

    /// The `.peaks` sidecar that belongs to a source's `.wav` — **the one place the
    /// naming lives**, so the writers (`Pool::import`, `media::Capture`), the pool index
    /// and a shell that draws the source cannot disagree about it.
    pub fn sidecar_path(wav: &Path) -> PathBuf {
        wav.with_extension("peaks")
    }

    /// Read the sidecar into the query-able [`Peaks`] — the read path a shell draws an
    /// envelope with. The tuple [`PeakFile::read`] stays the raw parser (diagnostics and
    /// tests); this is the same data in the shape a drawing wants.
    pub fn load(path: &Path) -> Result<Peaks, String> {
        let (base_bin, _, frames, sample_rate, levels) = Self::read(path)?;
        Ok(Peaks {
            base_bin: u64::from(base_bin),
            frames,
            sample_rate,
            levels,
        })
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
                return Err(format!(
                    "peaks level length {n} exceeds the source's frame count"
                ));
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
    w.write_all(&(mn.len() as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    for v in mn {
        w.write_all(&v.to_bits().to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    for v in mx {
        w.write_all(&v.to_bits().to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pair_min(v: &[f32]) -> Vec<f32> {
    v.chunks(2)
        .map(|p| p.iter().copied().fold(f32::INFINITY, f32::min))
        .collect()
}

fn pair_max(v: &[f32]) -> Vec<f32> {
    v.chunks(2)
        .map(|p| p.iter().copied().fold(f32::NEG_INFINITY, f32::max))
        .collect()
}

#[cfg(test)]
#[path = "tests/peaks.rs"]
mod tests;
