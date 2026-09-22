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
//!
//! **The pool is session-owned material at the session rate.** Material enters
//! through [`Pool::import`] (which converts a foreign rate once, at the boundary)
//! and [`Pool::conform`] brings a pool that predates the rule — or one that was
//! filled by hand — to the session rate, preserving the original as
//! `{id}.wav.pre{rate}` (its extension is not `.wav`, so the pool never indexes
//! the backup as a source). That is what lets the arrangement keep a single frame
//! domain: a clip is a straight read, never a rate conversion on the audio path.
//! It also means a pool directory is a *working* directory: pointing `pool` at a
//! library of originals makes the pool rewrite (and preserve) copies of them.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::peaks::{PeakBuilder, PeakFile};
use crate::resample::Resampler;
use crate::wav::{WavReader, WavWriter};

/// A pool source: a float-WAV + its `.peaks` sidecar (or a note it's missing).
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// One source brought to the session rate: imported, or conformed in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conform {
    /// The pool source id (the file stem).
    pub id: String,
    pub from_rate: u32,
    pub to_rate: u32,
    /// Frames in the source file.
    pub frames_in: u64,
    /// Frames written at the session rate (`frames_in` when no conversion ran).
    pub frames_out: u64,
    /// Whether the samples were resampled (false = a byte-identical copy).
    pub converted: bool,
    /// The pre-conversion file, preserved beside the pool copy as
    /// `{id}.wav.pre{rate}` (`None` when the material was copied in from
    /// elsewhere — the original is still where it was).
    pub preserved: Option<PathBuf>,
}

/// The conform report: what was converted, and what could not be.
#[derive(Debug, Clone, Default)]
pub struct ConformReport {
    pub converted: Vec<Conform>,
    /// Per-source failures (one unreadable file must not stop the rest).
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
        let mut report = Recovery {
            errors: index.errors.clone(),
            ..Recovery::default()
        };

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
        let mut reader = WavReader::open(wav)
            .map_err(|e| format!("rebuild peaks for {}: {e}", wav.display()))?;
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

    /// Bring `src` (a WAV at **any** rate) into the pool under its file stem, at
    /// the session rate. A source already at the session rate is **copied byte
    /// for byte** (no re-quantization); a foreign rate is resampled once, here,
    /// and gets a `.peaks` sidecar. Importing over an existing id replaces it.
    ///
    /// A source that already lives in the pool is conformed **in place** (its
    /// original preserved as `{id}.wav.pre{rate}`) instead of copied onto itself.
    pub fn import(&self, src: &Path, session_rate: u32) -> Result<Conform, String> {
        if session_rate == 0 {
            return Err("session rate must be non-zero".to_string());
        }
        if !src.is_file() {
            return Err(format!("{} is not a file", src.display()));
        }
        let id = src
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("{} has no usable file stem", src.display()))?
            .to_string();
        // A stem is also a clip id, so it must stay a plain name inside the pool.
        if !valid_id(&id) {
            return Err(format!("'{id}' is not usable as a pool id"));
        }
        let dest = self.dir.join(format!("{id}.wav"));
        let same_file = dest == src
            || (dest.exists() && fs::canonicalize(&dest).ok() == fs::canonicalize(src).ok());
        if same_file {
            return self.conform_one(&dest, session_rate, &id);
        }

        let reader = WavReader::open(src).map_err(|e| format!("import {}: {e}", src.display()))?;
        let from_rate = reader.sample_rate();
        let frames_in = reader.total_frames();
        drop(reader);

        let converted = from_rate != session_rate;
        let frames_out = if converted {
            let (_, _, frames) = Self::convert_file(src, &dest, session_rate)?;
            frames
        } else {
            fs::copy(src, &dest).map_err(|e| format!("copy to {}: {e}", dest.display()))?;
            frames_in
        };
        Self::rebuild_peaks(&dest, &dest.with_extension("peaks"))
            .map_err(|e| format!("import {}: {e}", src.display()))?;

        Ok(Conform {
            id,
            from_rate,
            to_rate: session_rate,
            frames_in,
            frames_out,
            converted,
            preserved: None,
        })
    }

    /// Bring every source in the pool to `session_rate` — the maintenance pass
    /// that makes an existing (or hand-filled) pool playable, the sibling of
    /// [`Pool::recover`]. Each mismatched source is converted in place and its
    /// pre-conversion file is kept as `{id}.wav.pre{rate}` — not a `.wav`, so
    /// the pool never indexes it. Per-source failures are reported, not fatal: a
    /// source that cannot be converted stays visible, at its own rate.
    pub fn conform(&self, session_rate: u32) -> Result<ConformReport, String> {
        if session_rate == 0 {
            return Err("session rate must be non-zero".to_string());
        }
        let index = self.list()?;
        let mut report = ConformReport {
            errors: index.errors.clone(),
            ..ConformReport::default()
        };
        for src in index
            .sources
            .iter()
            .filter(|s| s.sample_rate != session_rate)
        {
            match self.conform_one(&src.wav, session_rate, &src.id) {
                Ok(conform) => report.converted.push(conform),
                Err(e) => report.errors.push((src.wav.clone(), e)),
            }
        }
        Ok(report)
    }

    /// Convert one pool source in place, preserving the original. The converted
    /// audio is written to a side file first and then **renamed over** the
    /// source, so a crash never leaves the pool without its source (the
    /// pre-conversion copy is a hard link when the filesystem allows it).
    fn conform_one(&self, path: &Path, session_rate: u32, id: &str) -> Result<Conform, String> {
        let reader =
            WavReader::open(path).map_err(|e| format!("conform {}: {e}", path.display()))?;
        let from_rate = reader.sample_rate();
        let frames_in = reader.total_frames();
        drop(reader);
        if from_rate == session_rate {
            return Ok(Conform {
                id: id.to_string(),
                from_rate,
                to_rate: session_rate,
                frames_in,
                frames_out: frames_in,
                converted: false,
                preserved: None,
            });
        }

        let tmp = path.with_extension("converting");
        let (_, _, frames_out) = match Self::convert_file(path, &tmp, session_rate) {
            Ok(done) => done,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(format!("conform {}: {e}", path.display()));
            }
        };

        let backup = path.with_extension(format!("wav.pre{from_rate}"));
        let linked = fs::hard_link(path, &backup).is_ok();
        if !linked && let Err(e) = fs::copy(path, &backup) {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "conform {}: preserving the original failed: {e}",
                path.display()
            ));
        }
        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            if linked {
                let _ = fs::remove_file(&backup);
            }
            return Err(format!(
                "conform {}: replacing the source failed: {e}",
                path.display()
            ));
        }
        Self::rebuild_peaks(path, &path.with_extension("peaks"))?;

        Ok(Conform {
            id: id.to_string(),
            from_rate,
            to_rate: session_rate,
            frames_in,
            frames_out,
            converted: true,
            preserved: Some(backup),
        })
    }

    /// Stream `src` through the resampler into a mono float WAV at `out_rate`.
    /// Returns `(from_rate, frames_in, frames_out)`. The reader yields mono
    /// frames (channel 0 of a stereo file, like every pool reader).
    fn convert_file(src: &Path, dst: &Path, out_rate: u32) -> Result<(u32, u64, u64), String> {
        let mut reader =
            WavReader::open(src).map_err(|e| format!("convert {}: {e}", src.display()))?;
        let from_rate = reader.sample_rate();
        let frames_in = reader.total_frames();
        let mut resampler = Resampler::new(from_rate, out_rate)?;
        let mut writer = WavWriter::create_float(dst, out_rate, 1)?;

        let mut input = vec![0.0f32; 16_384];
        let mut output: Vec<f32> = Vec::with_capacity(32_768);
        loop {
            let n = reader.read_into(&mut input);
            if n == 0 {
                break;
            }
            resampler.process(&input[..n], &mut output);
            if !output.is_empty() {
                writer.write(&output)?;
                output.clear();
            }
        }
        resampler.flush(&mut output);
        if !output.is_empty() {
            writer.write(&output)?;
        }
        let frames_out = writer.frames_written();
        writer.finalize()?;
        Ok((from_rate, frames_in, frames_out))
    }
}
