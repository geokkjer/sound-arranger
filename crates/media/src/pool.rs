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
    /// Channels in the source file. A pool source is mono (1) by convention —
    /// capture writes `{take}.ch{k}`, import splits a multi-channel file into one
    /// source per channel — so a value above 1 means the file is not yet split
    /// (`Pool::conform` expands it).
    pub channels: u16,
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
    /// Channels in the source file this entry came from. Above 1 means that file
    /// was multi-channel and was split: the file's own stem keeps channel 0 and
    /// the rest live beside it as `{stem}.ch{k}` — this entry is one of those
    /// sources (the file's own, or a channel).
    pub channels: u16,
    /// Sibling sources written from the file's other channels, in channel order
    /// (empty for mono material, or when the siblings already existed).
    pub extracted: Vec<String>,
    /// The pre-conversion/pre-split file, preserved beside the pool copy when the
    /// pool's own file had to be rewritten. The extension is not `.wav`, so the
    /// pool never indexes the backup as a source.
    pub preserved: Option<PathBuf>,
}

/// The result of importing one file: one [`Conform`] per pool source written.
/// A mono file yields `{id}`; a multi-channel file is **split at the boundary**
/// into `{id}.ch0`, `{id}.ch1`, … — the same per-channel naming the capture path
/// uses — so every pool source is mono and a clip is a straight read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Import {
    /// The imported file's stem.
    pub id: String,
    /// Channels found in the file — always `sources.len()`: one pool source per
    /// channel (a mono file is one source under `id`; a multi-channel file is
    /// `{id}.ch0`, `{id}.ch1`, …). Read before the split for a file already in
    /// the pool, so it is the file's own count in every case.
    pub channels: u16,
    /// One entry per pool source written, in channel order.
    pub sources: Vec<Conform>,
}

impl Import {
    /// The pool source ids the file produced, in channel order.
    pub fn ids(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.id.as_str()).collect()
    }

    /// Frames of the longest source written.
    pub fn frames_out(&self) -> u64 {
        self.sources.iter().map(|s| s.frames_out).max().unwrap_or(0)
    }

    /// Whether any channel had to be resampled to reach the session rate.
    pub fn converted(&self) -> bool {
        self.sources.iter().any(|s| s.converted)
    }
}

/// The conform report: every pool source the pass rewrote, split or extracted
/// (one entry per source, so a split file contributes its `{id}` and each
/// `{id}.ch{k}`), and what could not be touched.
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
                channels: reader.channels(),
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

    /// Bring `src` (a WAV at **any** rate, with **any** channel count) into the
    /// pool under its file stem, at the session rate. A mono source already at
    /// the session rate is **copied byte for byte** (no re-quantization); a
    /// foreign rate is resampled once, here, and gets a `.peaks` sidecar.
    /// Importing over an existing id replaces it.
    ///
    /// A **multi-channel** file is split at the boundary: one mono source per
    /// channel, `{id}.ch0`, `{id}.ch1`, … — the naming the capture path already
    /// writes — so a clip is always a straight mono read and the mixer (not the
    /// reader) decides where a channel goes.
    ///
    /// A source that already lives in the pool is expanded **in place** (its
    /// original preserved as `{id}.wav.pre{rate}`) instead of copied onto itself.
    pub fn import(&self, src: &Path, session_rate: u32) -> Result<Import, String> {
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
            return Ok(Import {
                id: id.clone(),
                channels: WavReader::open(&dest)?.channels(),
                sources: self.expand_one(&dest, session_rate, &id)?,
            });
        }

        let reader = WavReader::open(src).map_err(|e| format!("import {}: {e}", src.display()))?;
        let from_rate = reader.sample_rate();
        let frames_in = reader.total_frames();
        let channels = reader.channels();
        drop(reader);

        if channels > 1 {
            // Every channel is rendered to a **temporary** name first: nothing in
            // the pool changes until all of them exist, so a failure part-way
            // through (an unreadable channel, a full disk) leaves the previous
            // material untouched and never leaves a torn file where a source is
            // expected. A leftover `.converting` file is not a `.wav`, so the pool
            // never indexes it.
            let mut staged: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
            for ch in 0..channels {
                let ch_dest = self.dir.join(format!("{id}.ch{ch}.wav"));
                match Self::stage_channel(src, &ch_dest, ch, session_rate) {
                    Ok((tmp, frames_out)) => staged.push((tmp, ch_dest, frames_out)),
                    Err(e) => {
                        for (tmp, _, _) in &staged {
                            let _ = fs::remove_file(tmp);
                        }
                        return Err(format!("import {}: {e}", src.display()));
                    }
                }
            }

            // Commit: the id now means *this* file, so material an earlier import
            // left under it — the whole-file source and any channel beyond the new
            // count — is replaced (a clip on `jam` must not keep playing an old
            // take, or a channel that no longer exists).
            self.replace_sources(&id, channels);
            let mut sources = Vec::with_capacity(channels as usize);
            for (tmp, ch_dest, frames_out) in &staged {
                if let Err(e) = fs::rename(tmp, ch_dest) {
                    let _ = fs::remove_file(tmp);
                    for (t, _, _) in &staged {
                        let _ = fs::remove_file(t);
                    }
                    return Err(format!(
                        "import {}: commit {}: {e}",
                        src.display(),
                        ch_dest.display()
                    ));
                }
                // Peaks last: a crash in this window leaves a source whose
                // sidecar is missing or stale, which `list` reports
                // (`peaks_missing`) and `Pool::recover` rebuilds.
                Self::rebuild_peaks(ch_dest, &ch_dest.with_extension("peaks"))
                    .map_err(|e| format!("import {}: {e}", src.display()))?;
                sources.push(Conform {
                    id: ch_dest
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    from_rate,
                    to_rate: session_rate,
                    frames_in,
                    frames_out: *frames_out,
                    converted: from_rate != session_rate,
                    channels,
                    extracted: Vec::new(),
                    preserved: None,
                });
            }
            return Ok(Import {
                id,
                channels,
                sources,
            });
        }

        // A mono file: resampled, or copied byte for byte when it already fits
        // (no re-quantization) — both through a temporary name, then committed,
        // so a failure cannot leave a torn source. The id means this file, so any
        // channel sources an earlier multi-channel import left under it go.
        let converted = from_rate != session_rate;
        let tmp = dest.with_extension("converting");
        let frames_out = if converted {
            match Self::write_channel(src, &tmp, 0, session_rate) {
                Ok((_, _, frames)) => frames,
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    return Err(format!("import {}: {e}", src.display()));
                }
            }
        } else {
            if let Err(e) = fs::copy(src, &tmp) {
                let _ = fs::remove_file(&tmp);
                return Err(format!("copy to {}: {e}", tmp.display()));
            }
            frames_in
        };
        self.replace_sources(&id, 1);
        if let Err(e) = fs::rename(&tmp, &dest) {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "import {}: commit {}: {e}",
                src.display(),
                dest.display()
            ));
        }
        Self::rebuild_peaks(&dest, &dest.with_extension("peaks"))
            .map_err(|e| format!("import {}: {e}", src.display()))?;

        Ok(Import {
            id: id.clone(),
            channels: 1,
            sources: vec![Conform {
                id,
                from_rate,
                to_rate: session_rate,
                frames_in,
                frames_out,
                converted,
                channels: 1,
                extracted: Vec::new(),
                preserved: None,
            }],
        })
    }

    /// Bring every source in the pool to the session rate and split any
    /// multi-channel file into mono siblings — the maintenance pass that makes an
    /// existing (or hand-filled) pool playable, the sibling of [`Pool::recover`].
    /// A converted source's pre-conversion file is kept as `{id}.wav.pre{rate}`
    /// (or `{id}.wav.pre{channels}ch` when only the channel split rewrote it) —
    /// not a `.wav`, so the pool never indexes the backup as a source.
    /// Per-source failures are reported, not fatal: a source that cannot be
    /// converted stays visible, at its own rate.
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
            .filter(|s| s.sample_rate != session_rate || s.channels > 1)
        {
            match self.expand_one(&src.wav, session_rate, &src.id) {
                Ok(conforms) => report.converted.extend(conforms),
                Err(e) => report.errors.push((src.wav.clone(), e)),
            }
        }
        Ok(report)
    }

    /// Render one channel of `src` into the pool as a **temporary** file beside
    /// its destination (`{dest}.converting`), returning `(temp path, frames)`.
    /// Nothing is renamed into place until every channel of the file is written,
    /// so an interrupted split never leaves a torn or half-updated source — and a
    /// leftover `.converting` is not a `.wav`, so the pool never indexes it.
    fn stage_channel(
        src: &Path,
        dest: &Path,
        channel: u16,
        out_rate: u32,
    ) -> Result<(PathBuf, u64), String> {
        let tmp = dest.with_extension("converting");
        match Self::write_channel(src, &tmp, channel, out_rate) {
            Ok((_, _, frames_out)) => Ok((tmp, frames_out)),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// Remove the sources an earlier import left under `id`: the whole-file
    /// source itself, and every `{id}.ch{k}` channel at or beyond
    /// `keep_channels`. **Importing replaces the id** — without this, a stereo
    /// file imported over a stem that held a 5.1 take (or a mono source) would
    /// leave the old material addressable, and a clip on that id would keep
    /// playing audio the user just replaced.
    ///
    /// Called only once the new material is safely written, so a failed import
    /// changes nothing. A filesystem error here is ignored: `list` reports what
    /// remains, and the next import retries.
    fn replace_sources(&self, id: &str, keep_channels: u16) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "wav") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // `{id}` itself, or `{id}.ch{k}` with k beyond what this import keeps.
            let stale = match stem.strip_prefix(id) {
                Some("") => true,
                Some(rest) => rest
                    .strip_prefix(".ch")
                    .and_then(|k| k.parse::<u16>().ok())
                    .is_some_and(|k| k >= keep_channels),
                None => false,
            };
            if stale {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("peaks"));
            }
        }
    }

    /// A backup name for `path` (`{stem}.wav.{tag}`) that does not exist yet, so
    /// preserving an original can never overwrite an earlier preservation of the
    /// same file. Returns the plain tagged name when it is free.
    fn free_backup(path: &Path, tag: &str) -> PathBuf {
        let base = path.with_extension(format!("wav.{tag}"));
        if !base.exists() {
            return base;
        }
        for n in 2..1000 {
            let candidate = path.with_extension(format!("wav.{tag}.{n}"));
            if !candidate.exists() {
                return candidate;
            }
        }
        path.with_extension(format!("wav.{tag}.{}", std::process::id()))
    }

    /// Write one channel of `src` as a mono float WAV at `out_rate`, resampling
    /// when the file's rate differs. Returns `(from_rate, frames_in, frames_out)`.
    fn write_channel(
        src: &Path,
        dst: &Path,
        channel: u16,
        out_rate: u32,
    ) -> Result<(u32, u64, u64), String> {
        let mut reader = WavReader::open(src)?.with_channel(channel)?;
        let from_rate = reader.sample_rate();
        let frames_in = reader.total_frames();
        let mut resampler = (from_rate != out_rate)
            .then(|| Resampler::new(from_rate, out_rate))
            .transpose()?;
        let mut writer = WavWriter::create_float(dst, out_rate, 1)?;
        let mut input = vec![0.0f32; 16_384];
        let mut output: Vec<f32> = Vec::with_capacity(32_768);
        loop {
            let n = reader.read_into(&mut input);
            if n == 0 {
                break;
            }
            match resampler.as_mut() {
                Some(r) => {
                    r.process(&input[..n], &mut output);
                    if !output.is_empty() {
                        writer.write(&output)?;
                        output.clear();
                    }
                }
                None => writer.write(&input[..n])?,
            }
        }
        if let Some(r) = resampler.as_mut() {
            r.flush(&mut output);
            if !output.is_empty() {
                writer.write(&output)?;
            }
        }
        let frames_out = writer.frames_written();
        writer.finalize()?;
        Ok((from_rate, frames_in, frames_out))
    }

    /// Bring one pool source in place to the session rate, splitting a
    /// multi-channel file: the extra channels are written beside it as
    /// `{id}.ch{k}` — **re-derived every pass**, through a temporary name, so a
    /// torn sibling left by an interrupted run is healed rather than trusted (the
    /// naming makes `{id}.ch{k}` mean "channel k of `{id}`", so writing it is the
    /// correct reading of the name, not a clobber) — and `{id}` itself is
    /// rewritten as its channel 0. Both go through a side file and a **rename**,
    /// so a crash never leaves the pool without its source, and the preserved
    /// original keeps a hard link when the filesystem allows it.
    ///
    /// `{id}` keeps meaning "channel 0" — a clip that referenced the file before
    /// the split still plays what it always played, and the other channel becomes
    /// addressable instead of silently dropped.
    ///
    /// Returns one entry per pool source this pass wrote, the file's own `{id}`
    /// entry first (carrying the sibling ids in [`Conform::extracted`]) so a
    /// caller can place every channel of the material from the list alone.
    fn expand_one(&self, path: &Path, session_rate: u32, id: &str) -> Result<Vec<Conform>, String> {
        let reader =
            WavReader::open(path).map_err(|e| format!("conform {}: {e}", path.display()))?;
        let from_rate = reader.sample_rate();
        let frames_in = reader.total_frames();
        let channels = reader.channels();
        drop(reader);

        if from_rate == session_rate && channels == 1 {
            // Nothing to do: the source is already a mono session-rate file. The
            // entry still describes it, because `Pool::import` of a pool source
            // over itself has no other way to report what it found (the `conform`
            // filter never sends a source here — this is the idempotent no-op).
            return Ok(vec![Conform {
                id: id.to_string(),
                from_rate,
                to_rate: session_rate,
                frames_in,
                frames_out: frames_in,
                converted: false,
                channels,
                extracted: Vec::new(),
                preserved: None,
            }]);
        }

        // Stage the siblings and channel 0 from the *original* file, then commit
        // them together (channel 0 is renamed over `{id}` last, so every read
        // happened against the original).
        let mut staged: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
        for ch in 1..channels {
            let sibling = path.with_file_name(format!("{id}.ch{ch}.wav"));
            match Self::stage_channel(path, &sibling, ch, session_rate) {
                Ok(staged_channel) => staged.push((staged_channel.0, sibling, staged_channel.1)),
                Err(e) => {
                    for (tmp, _, _) in &staged {
                        let _ = fs::remove_file(tmp);
                    }
                    return Err(format!("conform {}: {e}", path.display()));
                }
            }
        }
        let primary_tmp = path.with_extension("converting");
        let primary_frames = match Self::write_channel(path, &primary_tmp, 0, session_rate) {
            Ok((_, _, frames)) => frames,
            Err(e) => {
                let _ = fs::remove_file(&primary_tmp);
                for (tmp, _, _) in &staged {
                    let _ = fs::remove_file(tmp);
                }
                return Err(format!("conform {}: {e}", path.display()));
            }
        };

        // The original is kept under a name the pool never indexes; a name already
        // taken belongs to an earlier preservation, so this one is numbered.
        let tag = if from_rate == session_rate {
            format!("pre{channels}ch")
        } else {
            format!("pre{from_rate}")
        };
        let backup = Self::free_backup(path, &tag);
        let linked = fs::hard_link(path, &backup).is_ok();
        if !linked && let Err(e) = fs::copy(path, &backup) {
            let _ = fs::remove_file(&primary_tmp);
            for (tmp, _, _) in &staged {
                let _ = fs::remove_file(tmp);
            }
            return Err(format!(
                "conform {}: preserving the original failed: {e}",
                path.display()
            ));
        }

        let mut written = Vec::new();
        for (tmp, dest, frames_out) in &staged {
            if let Err(e) = fs::rename(tmp, dest) {
                let _ = fs::remove_file(tmp);
                return Err(format!(
                    "conform {}: commit {}: {e}",
                    path.display(),
                    dest.display()
                ));
            }
            // Peaks after the wav: a crash in this window leaves a source whose
            // sidecar is missing or stale, which `list` reports (`peaks_missing`)
            // and `Pool::recover` rebuilds.
            Self::rebuild_peaks(dest, &dest.with_extension("peaks"))?;
            written.push(Conform {
                id: dest
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string(),
                from_rate,
                to_rate: session_rate,
                frames_in,
                frames_out: *frames_out,
                converted: from_rate != session_rate,
                channels,
                extracted: Vec::new(),
                preserved: None,
            });
        }

        if let Err(e) = fs::rename(&primary_tmp, path) {
            let _ = fs::remove_file(&primary_tmp);
            if linked {
                let _ = fs::remove_file(&backup);
            }
            return Err(format!(
                "conform {}: replacing the source failed: {e}",
                path.display()
            ));
        }
        Self::rebuild_peaks(path, &path.with_extension("peaks"))?;

        let extracted: Vec<String> = written.iter().map(|c| c.id.clone()).collect();
        let primary = Conform {
            id: id.to_string(),
            from_rate,
            to_rate: session_rate,
            frames_in,
            frames_out: primary_frames,
            converted: from_rate != session_rate,
            channels,
            extracted,
            preserved: Some(backup),
        };

        let mut all = vec![primary];
        all.extend(written);
        Ok(all)
    }
}
