//! The media pool (P1.3.3, `pool`): immutable float-WAV sources + `.peaks`
//! sidecars, take-id-addressed (`{take_id}.ch{N}.wav`). This is the value a
//! timeline UI reads (sources, frame counts, peaks) and the crash-recovery pass
//! that finalises un-finalized takes and rebuilds missing peaks.
//!
//! Sources are immutable *post-finalize*; `Pool::recover` only finalizes crashed
//! takes (patches the header + truncates a torn tail) and *derives* missing
//! `.peaks` — it never mutates a well-formed source. A clip references a source by
//! its stem id (`take_id.ch{N}`); the arranger's `PoolResolver`/`Pool::path_for`
//! maps that id to its `.wav` path.

use std::fs;
use std::path::{Path, PathBuf};

use crate::peaks::{PeakBuilder, PeakFile};
use crate::wav::{WavReader, WavWriter};

/// A pool source: a float-WAV + its `.peaks` sidecar (or a note it's missing).
#[derive(Debug, Clone)]
pub struct PoolSource {
    /// The id clips reference (the `.wav` file stem, e.g. `take-1.ch0`).
    pub id: String,
    pub wav: PathBuf,
    pub peaks: PathBuf,
    pub frames: u64,
    pub sample_rate: u32,
    pub peaks_missing: bool,
    /// Whether the take is formally well-formed (a crashed, un-finalized take is
    /// `false` — it shows its recoverable length until `Pool::recover` runs).
    pub finalized: bool,
}

/// The pool index — a value a timeline UI reads (sources, frames, peaks).
#[derive(Debug, Clone, Default)]
pub struct PoolIndex {
    pub sources: Vec<PoolSource>,
    /// Sources that could not be read, reported rather than aborting the index —
    /// a crash day is exactly when a malformed file is likely.
    pub errors: Vec<(PathBuf, String)>,
}

/// The crash-recovery report: which takes were finalized and which peaks rebuilt.
#[derive(Debug, Clone, Default)]
pub struct Recovery {
    /// (source id, frames recovered) for takes recovered mid-write.
    pub finalized: Vec<(String, u64)>,
    /// source ids whose `.peaks` were (re)built.
    pub rebuilt_peaks: Vec<String>,
    /// Per-source failures (a broken source must not stop the rest).
    pub errors: Vec<(PathBuf, String)>,
}

/// The media pool: a directory of float-WAV sources + `.peaks` sidecars.
#[derive(Debug, Clone)]
pub struct Pool {
    dir: PathBuf,
}

/// Whether `id` is a plain file stem (no path separators, no `..`, non-empty) —
/// a clip id is user-craftable, so it must not escape the pool dir (kimi pool
/// should-fix 5).
fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.contains('\\') && id != "." && id != ".."
}

impl Pool {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        if !dir.is_dir() {
            return Err(format!("pool dir '{}' is not a directory", dir.display()));
        }
        Ok(Pool { dir })
    }

    /// The `.wav` path for a source id (stem), if it exists. Rejects a non-plain
    /// stem so a crafted id cannot escape the pool directory.
    pub fn path_for(&self, id: &str) -> Option<PathBuf> {
        if !valid_id(id) {
            return None;
        }
        let p = self.dir.join(format!("{id}.wav"));
        p.is_file().then_some(p)
    }

    /// Enumerate the pool sources (`.wav` files, sorted by filename for a
    /// deterministic index) with their frame counts, sample rate, and whether the
    /// `.peaks` sidecar is present. A source that cannot be read is reported in
    /// `errors` and skipped — not fatal to the whole index.
    pub fn list(&self) -> Result<PoolIndex, String> {
        let mut wavs: Vec<PathBuf> = fs::read_dir(&self.dir)
            .map_err(|e| format!("pool dir: {e}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "wav"))
            .collect();
        wavs.sort();

        let mut index = PoolIndex::default();
        for wav in wavs {
            let reader = match WavReader::open(&wav) {
                Ok(r) => r,
                Err(e) => {
                    index.errors.push((wav.clone(), e));
                    continue;
                }
            };
            let peaks = wav.with_extension("peaks");
            // id = the file stem (take_id.ch{N})
            let id = wav
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.strip_suffix(".wav").unwrap_or(n).to_string())
                .unwrap_or_default();
            let finalized = WavWriter::is_finalized(&wav).unwrap_or(false);
            index.sources.push(PoolSource {
                id,
                peaks_missing: !peaks.is_file(),
                wav,
                peaks,
                frames: reader.total_frames(),
                sample_rate: reader.sample_rate(),
                finalized,
            });
        }
        Ok(index)
    }

    /// Recover crashed takes (finalize un-finalized `.wav`s) and rebuild missing
    /// or corrupt `.peaks` sidecars. Never mutates a well-formed source (other
    /// than deriving its missing peaks). Per-source failures are reported, not
    /// fatal. **Ordering is load-bearing:** finalize a source's take *before*
    /// rebuilding its peaks, so the peak frame count matches the recovered take.
    pub fn recover(&self) -> Result<Recovery, String> {
        let index = self.list()?;
        // carry list()'s unreadable sources through as recovery failures
        let mut report = Recovery { errors: index.errors.clone(), ..Recovery::default() };

        for src in &index.sources {
            // Finalize a crashed take: the header still declares placeholder sizes.
            if !src.finalized {
                match WavWriter::recover(&src.wav) {
                    Ok(frames) => report.finalized.push((src.id.clone(), frames)),
                    Err(e) => {
                        report.errors.push((src.wav.clone(), e));
                        continue;
                    }
                }
            }
            // Rebuild a missing *or corrupt* `.peaks` (validated by read, not
            // is_file — a truncated sidecar exists but is unusable). Derives a
            // sidecar; never rewrites the source.
            if !src.peaks_missing && PeakFile::read(&src.peaks).is_ok() {
                continue;
            }
            match Self::rebuild_peaks(&src.wav, &src.peaks) {
                Ok(()) => report.rebuilt_peaks.push(src.id.clone()),
                Err(e) => report.errors.push((src.wav.clone(), e)),
            }
        }
        Ok(report)
    }

    /// Rebuild a `.peaks` sidecar from a float-WAV, streaming it in chunks.
    fn rebuild_peaks(wav: &Path, peaks: &Path) -> Result<(), String> {
        let mut reader = WavReader::open(wav).map_err(|e| format!("rebuild peaks for {}: {e}", wav.display()))?;
        let sample_rate = reader.sample_rate();
        let mut builder = PeakBuilder::new();
        let mut buf = vec![0.0f32; 4096];
        loop {
            let n = reader.read_into(&mut buf);
            if n == 0 {
                break;
            }
            builder.push(&buf[..n]);
        }
        PeakFile::write(peaks, &mut builder, sample_rate)
    }
}
