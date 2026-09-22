//! Minimal RIFF/WAVE reader and writer, std only.
//!
//! Writer: 16-bit PCM mono/stereo, placeholder sizes written *before* any
//! audio and patched on [`WavWriter::finalize`] — so a take is
//! crash-recoverable from the first byte: [`WavWriter::recover`] rescans the
//! data chunk and patches the header of a file that died mid-write. Reader:
//! 16-bit PCM and 32-bit float, mono or stereo (stereo → channel 0; the
//! Phase-0 graph is mono). `hound` stays the Phase-1 upgrade if format edge
//! cases (WAVEFORMATEXTENSIBLE, 24-bit, …) bite (Spike B note, Alternatives).

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const RIFF_TAG: &[u8; 4] = b"RIFF";
const WAVE_TAG: &[u8; 4] = b"WAVE";
const FMT_TAG: &[u8; 4] = b"fmt ";
const DATA_TAG: &[u8; 4] = b"data";

const FMT_PCM: u16 = 1;
const FMT_FLOAT: u16 = 3;
/// Header layout positions: RIFF size at 4, data size at 40 (12-byte RIFF +
/// 8-byte fmt header + 16-byte fmt body + 4-byte "data" tag).
const RIFF_SIZE_POS: u64 = 4;
const DATA_SIZE_POS: u64 = 40;

/// Parsed WAV header; `data_offset` is the file position of the first audio
/// byte, `data_bytes` the declared data-chunk size.
struct Header {
    channels: u16,
    sample_rate: u32,
    bits: u16,
    data_offset: u64,
    data_bytes: u32,
}

impl Header {
    fn block_align(&self) -> u64 {
        self.channels as u64 * (self.bits / 8) as u64
    }
}

fn read_tag(reader: &mut impl Read, tag: &[u8; 4], what: &str) -> Result<(), String> {
    let mut buf = [0u8; 4];
    reader
        .read_exact(&mut buf)
        .map_err(|e| format!("{what}: {e}"))?;
    if &buf != tag {
        return Err(format!(
            "{what}: expected '{}', found {:?}",
            String::from_utf8_lossy(tag),
            buf
        ));
    }
    Ok(())
}

fn parse_header(reader: &mut (impl Read + Seek)) -> Result<Header, String> {
    read_tag(reader, RIFF_TAG, "not a RIFF file")?;
    let mut size = [0u8; 4];
    reader.read_exact(&mut size).map_err(|e| e.to_string())?;
    read_tag(reader, WAVE_TAG, "not a WAVE file")?;

    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, rate, bits)
    let mut data: Option<(u64, u32)> = None; // (offset, bytes)

    loop {
        let mut chunk = [0u8; 8];
        if reader.read_exact(&mut chunk).is_err() {
            break; // EOF before data → reported below
        }
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        if &chunk[..4] == FMT_TAG {
            let mut body = [0u8; 40];
            let n = size.min(40) as usize;
            reader
                .read_exact(&mut body[..n])
                .map_err(|e| format!("fmt chunk: {e}"))?;
            if size > 40 {
                reader
                    .seek(SeekFrom::Current(size as i64 - n as i64))
                    .map_err(|e| e.to_string())?;
            }
            let format = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            let bits = u16::from_le_bytes([body[14], body[15]]);
            if format != FMT_PCM && format != FMT_FLOAT {
                return Err(format!(
                    "unsupported WAV audio format {format} (PCM or float only)"
                ));
            }
            if channels != 1 && channels != 2 {
                return Err(format!(
                    "unsupported channel count {channels} (mono/stereo only)"
                ));
            }
            if bits != 16 && bits != 32 {
                return Err(format!(
                    "unsupported bit depth {bits} (16-bit PCM or 32-bit float only)"
                ));
            }
            fmt = Some((format, channels, rate, bits));
        } else if &chunk[..4] == DATA_TAG {
            data = Some((
                reader.stream_position().map_err(|e| e.to_string())?,
                size as u32,
            ));
            break; // data is the last chunk for files we write; a reader may re-seek
        } else {
            reader
                .seek(SeekFrom::Current(size as i64))
                .map_err(|e| e.to_string())?;
        }
    }

    let (_format, channels, sample_rate, bits) = fmt.ok_or("missing fmt chunk")?;
    let (data_offset, data_bytes) = data.ok_or("missing data chunk")?;
    Ok(Header {
        channels,
        sample_rate,
        bits,
        data_offset,
        data_bytes,
    })
}

/// A chunked WAV reader (16-bit PCM + 32-bit float; mono, or stereo → channel 0).
pub struct WavReader {
    reader: BufReader<File>,
    channels: u16,
    bits: u16,
    sample_rate: u32,
    data_offset: u64,
    total_frames: u64,
    frames_left: u64,
}

impl WavReader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let h = parse_header(&mut file)?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();
        // A crashed-take header may declare a placeholder size; trust the file
        // length instead so the reader still returns every written sample.
        let mut data_bytes = h.data_bytes as u64;
        if h.data_offset + data_bytes > file_len {
            data_bytes = file_len - h.data_offset;
        }
        file.seek(SeekFrom::Start(h.data_offset))
            .map_err(|e| e.to_string())?;
        let total_frames = data_bytes / h.block_align();
        Ok(WavReader {
            reader: BufReader::new(file),
            channels: h.channels,
            bits: h.bits,
            sample_rate: h.sample_rate,
            data_offset: h.data_offset,
            total_frames,
            frames_left: total_frames,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Position the reader at frame `n` within the file's data chunk.
    pub fn seek_frames(&mut self, n: u64) -> Result<(), String> {
        if n > self.total_frames {
            return Err(format!("seek_frames({n}) past end ({})", self.total_frames));
        }
        self.reader
            .seek(SeekFrom::Start(self.data_offset + n * self.block_align()))
            .map_err(|e| e.to_string())?;
        self.frames_left = self.total_frames - n;
        Ok(())
    }

    fn block_align(&self) -> u64 {
        self.channels as u64 * (self.bits / 8) as u64
    }

    /// Read up to `out.len()` mono frames (stereo files contribute channel 0).
    /// Returns the number of frames written (fewer at EOF). Never blocks on
    /// the audio path — called from reader threads, not from render.
    pub fn read_into(&mut self, out: &mut [f32]) -> usize {
        const MAX_FRAMES: usize = 256;
        let mut written = 0usize;
        while written < out.len() && self.frames_left > 0 {
            let want = (out.len() - written)
                .min(MAX_FRAMES)
                .min(self.frames_left as usize);
            let bytes = want * self.block_align() as usize;
            let mut raw = [0u8; MAX_FRAMES * 8]; // 256 frames × 2 ch × 4 bytes
            let Ok(got) = self.reader.read(&mut raw[..bytes]) else {
                break;
            };
            if got == 0 {
                break;
            }
            let frames = got / self.block_align() as usize;
            let ba = self.block_align() as usize;
            for f in 0..frames {
                let base = f * ba;
                let s = if self.bits == 16 {
                    let v = i16::from_le_bytes([raw[base], raw[base + 1]]);
                    v as f32 / 32768.0
                } else {
                    f32::from_le_bytes([raw[base], raw[base + 1], raw[base + 2], raw[base + 3]])
                };
                out[written] = s;
                written += 1;
            }
            self.frames_left -= frames as u64;
        }
        written
    }
}

/// A WAV writer whose header sizes are patched on `finalize` (and best-effort
/// on drop), and whose crashed takes are recoverable. 16-bit PCM by default;
/// [`WavWriter::create_float`] writes 32-bit float — the media *pool*'s format
/// (prior-art research: float on disk, 16-bit at bounce/export).
pub struct WavWriter {
    writer: BufWriter<File>,
    pub sample_rate: u32,
    pub channels: u16,
    float: bool,
    frames: u64,
    finalized: bool,
    path: PathBuf,
}

impl WavWriter {
    /// Create (truncate) the file and write a complete header with placeholder
    /// sizes — the take is recoverable from the first byte.
    pub fn create(path: &Path, sample_rate: u32, channels: u16) -> Result<Self, String> {
        Self::create_with(path, sample_rate, channels, false)
    }

    /// Create a 32-bit float WAV (format 3) — the media pool's source format.
    pub fn create_float(path: &Path, sample_rate: u32, channels: u16) -> Result<Self, String> {
        Self::create_with(path, sample_rate, channels, true)
    }

    fn create_with(
        path: &Path,
        sample_rate: u32,
        channels: u16,
        float: bool,
    ) -> Result<Self, String> {
        if channels != 1 && channels != 2 {
            return Err(format!(
                "WAV writer supports mono/stereo, got {channels} channels"
            ));
        }
        let file = File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
        let mut writer = BufWriter::new(file);
        write_header(&mut writer, sample_rate, channels, float)?;
        Ok(WavWriter {
            writer,
            sample_rate,
            channels,
            float,
            frames: 0,
            finalized: false,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append interleaved frames (mono: one sample per frame; stereo: pairs).
    /// The last partial frame is dropped. 16-bit clamps to [-1, 1] and
    /// quantizes; float writes the raw samples (the pool keeps the signal).
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let frames = samples.len() / self.channels as usize;
        let bytes_per_sample = if self.float { 4 } else { 2 };
        let mut bytes = Vec::with_capacity(frames * self.channels as usize * bytes_per_sample);
        for f in 0..frames {
            for ch in 0..self.channels as usize {
                let s = samples[f * self.channels as usize + ch];
                if self.float {
                    bytes.extend_from_slice(&s.to_le_bytes());
                } else {
                    let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        self.writer
            .write_all(&bytes)
            .map_err(|e| format!("write {}: {e}", self.path.display()))?;
        self.frames += frames as u64;
        Ok(())
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
    }

    /// Flush the underlying buffer to the OS. The recorder thread calls this
    /// after every chunk so a crashed take loses nothing the writer already
    /// drained (crash recovery then finds every written frame).
    pub fn flush(&mut self) -> Result<(), String> {
        self.writer
            .flush()
            .map_err(|e| format!("flush {}: {e}", self.path.display()))
    }

    /// Patch the placeholder sizes and flush. Idempotent.
    pub fn finalize(&mut self) -> Result<(), String> {
        if self.finalized {
            return Ok(());
        }
        self.writer.flush().map_err(|e| format!("flush: {e}"))?;
        patch_sizes(&mut self.writer, self.frames, self.channels, self.float)?;
        self.finalized = true;
        Ok(())
    }

    /// Recover a take that died mid-write (no `finalize` ran): rescan the data
    /// chunk, patch both size fields, and return the frame count recovered.
    /// Works because the header with placeholders is written before any audio.
    pub fn recover(path: &Path) -> Result<u64, String> {
        let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let h = parse_header(&mut file)?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();
        let actual_bytes = file_len.saturating_sub(h.data_offset);
        let frames = actual_bytes / h.block_align();
        // Patch a *frame-aligned* size: a torn tail from a crash mid-flush is
        // truncated away so the recovered file is formally well-formed
        // (kimi review finding 7). Refuse a take that would overflow the u32
        // size field rather than writing a corrupt small header (>4 GiB).
        let data_bytes = data_bytes(frames, h.channels, h.bits == 32)?;
        let mut f = File::options()
            .write(true)
            .open(path)
            .map_err(|e| format!("open rw {}: {e}", path.display()))?;
        f.seek(SeekFrom::Start(RIFF_SIZE_POS))
            .map_err(|e| e.to_string())?;
        f.write_all(&(36u32 + data_bytes as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(DATA_SIZE_POS))
            .map_err(|e| e.to_string())?;
        f.write_all(&(data_bytes as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.set_len(h.data_offset + data_bytes)
            .map_err(|e| e.to_string())?;
        f.flush().map_err(|e| e.to_string())?;
        Ok(frames)
    }

    /// Whether a file is formally well-formed (its declared data size exactly
    /// matches the bytes after the data header) — i.e. a crashed take whose
    /// placeholder size was never patched returns `false` (the pool recovers it).
    pub fn is_finalized(path: &Path) -> Result<bool, String> {
        let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let h = parse_header(&mut file)?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();
        Ok(h.data_offset + h.data_bytes as u64 == file_len)
    }
}

impl Drop for WavWriter {
    /// Best-effort header patch on drop, so a forgotten `finalize` still
    /// yields a readable file (the crash case — no drop at all — is
    /// `recover`'s job).
    fn drop(&mut self) {
        if !self.finalized {
            let _ = self.finalize();
        }
    }
}

fn write_header(
    w: &mut impl Write,
    sample_rate: u32,
    channels: u16,
    float: bool,
) -> Result<(), String> {
    w.write_all(RIFF_TAG).map_err(|e| e.to_string())?;
    w.write_all(&0xFFFF_FFFFu32.to_le_bytes())
        .map_err(|e| e.to_string())?; // riff size placeholder
    w.write_all(WAVE_TAG).map_err(|e| e.to_string())?;
    w.write_all(FMT_TAG).map_err(|e| e.to_string())?;
    w.write_all(&16u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    w.write_all(&(if float { FMT_FLOAT } else { FMT_PCM }).to_le_bytes())
        .map_err(|e| e.to_string())?;
    w.write_all(&channels.to_le_bytes())
        .map_err(|e| e.to_string())?;
    w.write_all(&sample_rate.to_le_bytes())
        .map_err(|e| e.to_string())?;
    let bytes_per_sample: u16 = if float { 4 } else { 2 };
    w.write_all(&(sample_rate * channels as u32 * bytes_per_sample as u32).to_le_bytes())
        .map_err(|e| e.to_string())?; // byte rate
    w.write_all(&(channels * bytes_per_sample).to_le_bytes())
        .map_err(|e| e.to_string())?; // block align
    w.write_all(&(bytes_per_sample * 8).to_le_bytes())
        .map_err(|e| e.to_string())?; // bits
    w.write_all(DATA_TAG).map_err(|e| e.to_string())?;
    w.write_all(&0xFFFF_FFFFu32.to_le_bytes())
        .map_err(|e| e.to_string())?; // data size placeholder
    Ok(())
}

fn patch_sizes(
    w: &mut (impl Write + Seek),
    frames: u64,
    channels: u16,
    float: bool,
) -> Result<(), String> {
    let data_bytes = data_bytes(frames, channels, float)?;
    let data_bytes = data_bytes as u32;
    w.seek(SeekFrom::Start(RIFF_SIZE_POS))
        .map_err(|e| e.to_string())?;
    w.write_all(&(36u32 + data_bytes).to_le_bytes())
        .map_err(|e| e.to_string())?;
    w.seek(SeekFrom::Start(DATA_SIZE_POS))
        .map_err(|e| e.to_string())?;
    w.write_all(&data_bytes.to_le_bytes())
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The RIFF data-chunk size, or `Err` if it would exceed the u32 field — a take
/// that big must not silently truncate to a corrupt small header (the pool
/// records long live jams; ~6.2 h mono float @48 kHz crosses 4 GiB). RF64 is the
/// longer-term answer; for now a >4 GiB take fails loud.
fn data_bytes(frames: u64, channels: u16, float: bool) -> Result<u64, String> {
    let bytes_per_sample: u64 = if float { 4 } else { 2 };
    let data_bytes = frames
        .checked_mul(channels as u64)
        .and_then(|b| b.checked_mul(bytes_per_sample))
        .ok_or("take length overflows the WAV size arithmetic")?;
    // The RIFF size field is `36 + data_bytes (+ 1 pad byte if data_bytes is odd,
    // since WAV chunks are word-aligned)` — both must fit u32. `-37` (not `-36`)
    // covers the odd-data pad case; an odd data_bytes at the exact `-36` boundary
    // would still overflow the patched header.
    if data_bytes > u32::MAX as u64 - 37 {
        return Err(format!(
            "take of {data_bytes} audio bytes exceeds the WAV u32 size field (max {}) — an RF64/segmented take is not yet supported",
            u32::MAX as u64 - 37
        ));
    }
    Ok(data_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("media-wav-{name}-{}.wav", std::process::id()))
    }

    #[test]
    fn roundtrip_mono_16() {
        let path = tmp("mono");
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) * 2.0 - 1.0).collect();
        {
            let mut w = WavWriter::create(&path, 48_000, 1).unwrap();
            w.write(&samples).unwrap();
            w.finalize().unwrap();
        }
        let mut r = WavReader::open(&path).unwrap();
        assert_eq!(r.sample_rate(), 48_000);
        assert_eq!(r.channels(), 1);
        assert_eq!(r.total_frames(), 1000);
        let mut back = vec![0.0f32; 1000];
        assert_eq!(r.read_into(&mut back), 1000);
        for (a, b) in samples.iter().zip(&back) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn roundtrip_stereo_reads_channel0() {
        let path = tmp("stereo");
        let frames: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let mut interleaved = Vec::with_capacity(frames.len() * 2);
        for f in &frames {
            interleaved.push(*f);
            interleaved.push(-*f); // channel 1 must be ignored
        }
        {
            let mut w = WavWriter::create(&path, 44_100, 2).unwrap();
            w.write(&interleaved).unwrap();
            w.finalize().unwrap();
        }
        let mut r = WavReader::open(&path).unwrap();
        assert_eq!(r.channels(), 2);
        let mut back = vec![0.0f32; 100];
        assert_eq!(r.read_into(&mut back), 100);
        for (a, b) in frames.iter().zip(&back) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn finalize_patches_sizes() {
        let path = tmp("sizes");
        let n = 5000u64;
        {
            let mut w = WavWriter::create(&path, 48_000, 1).unwrap();
            w.write(&vec![0.25; n as usize]).unwrap();
            w.finalize().unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            36 + (n * 2) as u32
        );
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            (n * 2) as u32
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A take that dies mid-write (drop without finalize, header still
    /// placeholder) is recovered to its full length by `recover`.
    #[test]
    fn crash_recovery_recovers_the_take() {
        let path = tmp("crash");
        let n = 1234u64;
        {
            let mut w = WavWriter::create(&path, 48_000, 1).unwrap();
            w.write(&vec![0.5; n as usize]).unwrap();
            // no finalize — the "crash"
        }
        assert_eq!(WavWriter::recover(&path).unwrap(), n);
        let mut r = WavReader::open(&path).unwrap();
        assert_eq!(r.total_frames(), n);
        let mut back = vec![0.0f32; n as usize];
        assert_eq!(r.read_into(&mut back), n as usize);
        assert!(back.iter().all(|s| (*s - 0.5).abs() < 1e-3));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_garbage() {
        let path = tmp("garbage");
        std::fs::write(&path, b"not a wave file at all").unwrap();
        assert!(WavReader::open(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod float_tests {
    use super::*;

    #[test]
    fn float_roundtrip_is_lossless() {
        let path = std::env::temp_dir().join(format!("media-wav-float-{}.wav", std::process::id()));
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) * 2.0 - 1.0).collect();
        {
            let mut w = WavWriter::create_float(&path, 48_000, 1).unwrap();
            w.write(&samples).unwrap();
            w.finalize().unwrap();
        }
        let mut r = WavReader::open(&path).unwrap();
        assert_eq!(r.sample_rate(), 48_000);
        assert_eq!(r.total_frames(), 1000);
        let mut back = vec![0.0f32; 1000];
        assert_eq!(r.read_into(&mut back), 1000);
        for (a, b) in samples.iter().zip(&back) {
            assert_eq!(*a, *b, "float WAV must round-trip exactly (bit-identical)");
        }
        let _ = std::fs::remove_file(&path);
    }

    /// A crashed float take recovers to its full length like the 16-bit one.
    #[test]
    fn float_take_crash_recovers() {
        let path =
            std::env::temp_dir().join(format!("media-wav-float-crash-{}.wav", std::process::id()));
        {
            let mut w = WavWriter::create_float(&path, 48_000, 1).unwrap();
            w.write(&vec![0.5; 777]).unwrap();
            // no finalize — the "crash"
        }
        assert_eq!(WavWriter::recover(&path).unwrap(), 777);
        let _ = std::fs::remove_file(&path);
    }

    /// A take crossing the u32 WAV size field must fail loud, never silently
    /// truncate to a corrupt small header (>4 GiB — the pool records long jams).
    #[test]
    fn data_bytes_guard_refuses_oversized_take() {
        // mono float: data_bytes = frames * 4. The RIFF field is `36 + data_bytes
        // (+1 pad if odd)`; the guard is `> u32::MAX - 37` (conservative, covers
        // the odd-data pad even though our writers always produce even data_bytes).
        let ok_frames = (u32::MAX as u64 - 37) / 4; // data_bytes just under the bound
        assert!(
            data_bytes(ok_frames, 1, true).is_ok(),
            "just under the boundary is ok"
        );
        let err_frames = ok_frames + 1; // data_bytes just over
        assert!(
            data_bytes(err_frames, 1, true).is_err(),
            "just over the boundary must refuse"
        );
    }
}
