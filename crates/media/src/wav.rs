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

/// Frames read per interleaved chunk (and the size of the reader's stack buffer).
const MAX_CHUNK_FRAMES: usize = 256;
/// The reader's interleaved chunk, in bytes: [`MAX_CHUNK_FRAMES`] frames of the
/// widest sample it decodes (4 bytes). A frame must fit, so a file is refused
/// past the channel count this allows rather than read without progress.
const MAX_CHUNK_BYTES: usize = MAX_CHUNK_FRAMES * 4;
/// The channel ceiling: how many 4-byte samples fit one chunk. 5.1 is 6.
const MAX_CHANNELS: u16 = (MAX_CHUNK_BYTES / 4) as u16;
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
    /// Bytes one sample occupies, from the bit depth the header declares. Never
    /// inferred from an "is this float?" test: 24-bit PCM is *three* bytes a
    /// sample, and describing a recovered 24-bit take at two truncated it to
    /// two thirds while returning the frame count the file no longer held.
    fn bytes_per_sample(&self) -> u16 {
        self.bits / 8
    }

    fn block_align(&self) -> u64 {
        self.channels as u64 * self.bytes_per_sample() as u64
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
            // Any channel count a recorder wrote is readable — `read_into` yields
            // one channel and `with_channel` picks which, so a 5.1 file imports
            // as six mono pool sources. The ceiling is what one interleaved chunk
            // can hold ([`MAX_CHANNELS`] 4-byte samples in [`MAX_CHUNK_BYTES`]): a
            // file whose *frame* does not fit could never make progress, so it is
            // refused here rather than spun on. Real material is far below it, and
            // the declared block-align field is ignored either way — the reader
            // re-derives frames from the channel count and bit depth.
            if channels == 0 || channels > MAX_CHANNELS {
                return Err(format!(
                    "unsupported channel count {channels} (max {MAX_CHANNELS})"
                ));
            }
            // 24-bit PCM is what most other tools write; it decodes to f32 on the
            // control side like the rest (the pool is float). A 32-bit *PCM* file is
            // still refused — only 32-bit float is a WAV we write, and guessing
            // between int and float by format tag would be a silent corruption.
            if bits != 16 && bits != 24 && bits != 32 {
                return Err(format!(
                    "unsupported bit depth {bits} (16/24-bit PCM or 32-bit float)"
                ));
            }
            if bits == 32 && format != FMT_FLOAT {
                return Err("32-bit WAV must be float (format 3), not PCM".to_string());
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
    /// Which channel `read_into` yields (0 by default; see [`Self::with_channel`]).
    pick: u16,
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
            pick: 0,
        })
    }

    /// Read channel `k` instead of channel 0 — how a multi-channel file is split
    /// into the pool's mono sources. Errors past the file's channel count.
    pub fn with_channel(mut self, k: u16) -> Result<Self, String> {
        if k >= self.channels {
            return Err(format!(
                "channel {k} does not exist in a {}-channel file",
                self.channels
            ));
        }
        self.pick = k;
        Ok(self)
    }

    /// Which channel `read_into` yields.
    pub fn channel(&self) -> u16 {
        self.pick
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

    /// Read up to `out.len()` mono frames of the selected channel (channel 0
    /// unless [`Self::with_channel`] said otherwise). Returns the number of
    /// frames written (fewer at EOF). Never blocks on the audio path — called
    /// from reader threads, not from render.
    pub fn read_into(&mut self, out: &mut [f32]) -> usize {
        // One interleaved chunk at a time, always on the stack: cap the frame
        // count by the buffer so a wide file (5.1, 7.1) cannot overrun it. The
        // channel guard in `parse_header` makes a zero-width chunk unreachable;
        // returning 0 rather than looping keeps that a guarantee, not a hope.
        let mut raw = [0u8; MAX_CHUNK_BYTES];
        let ba = self.block_align() as usize;
        let chunk_frames = MAX_CHUNK_FRAMES.min(raw.len() / ba.max(1));
        if chunk_frames == 0 {
            return 0;
        }
        let mut written = 0usize;
        while written < out.len() && self.frames_left > 0 {
            let want = (out.len() - written)
                .min(chunk_frames)
                .min(self.frames_left as usize);
            // Whole frames only: a short read would leave the stream pointing
            // mid-frame, so every later sample would come from the wrong channel.
            if self.reader.read_exact(&mut raw[..want * ba]).is_err() {
                break;
            }
            let frames = want;
            let pick = self.pick as usize * (self.bits / 8) as usize;
            for f in 0..frames {
                let at = f * ba + pick;
                let s = match self.bits {
                    16 => {
                        let v = i16::from_le_bytes([raw[at], raw[at + 1]]);
                        v as f32 / 32768.0
                    }
                    // Sign-extend three little-endian bytes and scale by 2^23.
                    24 => {
                        let v = i32::from_le_bytes([
                            raw[at],
                            raw[at + 1],
                            raw[at + 2],
                            if raw[at + 2] & 0x80 != 0 { 0xff } else { 0x00 },
                        ]);
                        v as f32 / 8_388_608.0
                    }
                    _ => f32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]),
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
        // size field rather than writing a corrupt small header (>4 GiB). The
        // sample width is the header's own (`bits / 8`) — the same term
        // `block_align` uses — so a 24-bit take is described at three bytes a
        // sample and survives recovery whole.
        let data_bytes = data_bytes(frames, h.channels, h.bytes_per_sample())?;
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
    let bytes_per_sample: u16 = if float { 4 } else { 2 };
    let data_bytes = data_bytes(frames, channels, bytes_per_sample)?;
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

/// The RIFF data-chunk size for a take of `frames` at a sample width of
/// `bytes_per_sample` (a *width*, not a format flag — a 24-bit take is three
/// bytes a sample), or `Err` if it would exceed the u32 field — a take
/// that big must not silently truncate to a corrupt small header (the pool
/// records long live jams; ~6.2 h mono float @48 kHz crosses 4 GiB). RF64 is the
/// longer-term answer; for now a >4 GiB take fails loud.
fn data_bytes(frames: u64, channels: u16, bytes_per_sample: u16) -> Result<u64, String> {
    let data_bytes = frames
        .checked_mul(channels as u64)
        .and_then(|b| b.checked_mul(bytes_per_sample as u64))
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

    /// Channel 1 of a stereo file, read directly — how the pool splits material.
    #[test]
    fn with_channel_reads_that_channel() {
        let path = tmp("pick");
        let left: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let mut interleaved = Vec::with_capacity(left.len() * 2);
        for f in &left {
            interleaved.push(*f);
            interleaved.push(-*f);
        }
        {
            let mut w = WavWriter::create(&path, 44_100, 2).unwrap();
            w.write(&interleaved).unwrap();
            w.finalize().unwrap();
        }
        let mut r = WavReader::open(&path).unwrap().with_channel(1).unwrap();
        assert_eq!(r.channel(), 1);
        let mut back = vec![0.0f32; 100];
        assert_eq!(r.read_into(&mut back), 100);
        for (a, b) in left.iter().zip(&back) {
            assert!((a + b).abs() < 1e-3, "{a} vs {b}");
        }
        assert!(WavReader::open(&path).unwrap().with_channel(2).is_err());
        let _ = std::fs::remove_file(&path);
    }

    /// Hand-write a float WAV with more than two channels — the *writer* is
    /// mono/stereo on purpose, but a reader must survive anything a recorder or
    /// another tool produced.
    fn write_wide_float_wav(path: &Path, rate: u32, channels: u16, interleaved: &[f32]) {
        let ba = channels * 4;
        let data_bytes = (interleaved.len() * 4) as u32;
        let mut v = Vec::with_capacity(44 + data_bytes as usize);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * ba as u32).to_le_bytes());
        v.extend_from_slice(&ba.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_bytes.to_le_bytes());
        for s in interleaved {
            v.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, v).unwrap();
    }

    /// A wide file (more than two channels) reads channel *k* without overrunning
    /// the reader's stack chunk — the interleaved frame is 4 bytes per channel.
    #[test]
    fn a_six_channel_file_reads_every_channel() {
        let path = tmp("six");
        let want = 700usize; // more than one 256-frame chunk
        let mut interleaved = Vec::with_capacity(want * 6);
        for i in 0..want {
            for ch in 0..6 {
                interleaved.push((i as f32 + ch as f32) / 1000.0);
            }
        }
        write_wide_float_wav(&path, 48_000, 6, &interleaved);
        for ch in 0..6u16 {
            let mut r = WavReader::open(&path).unwrap().with_channel(ch).unwrap();
            let mut back = vec![0.0f32; want];
            assert_eq!(r.read_into(&mut back), want);
            for (i, s) in back.iter().enumerate() {
                let expect = (i as f32 + ch as f32) / 1000.0;
                assert!(
                    (s - expect).abs() < 1e-6,
                    "ch{ch} frame {i}: {s} vs {expect}"
                );
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    /// A file too wide for one chunk is refused at open, never read without
    /// progress (a frame wider than the chunk buffer used to mean `want == 0` and
    /// an infinite loop). The widest the reader accepts still progresses.
    #[test]
    fn a_frame_wider_than_the_chunk_is_refused() {
        let path = tmp("toowide");
        write_wide_float_wav(&path, 48_000, MAX_CHANNELS + 1, &[]);
        assert!(
            WavReader::open(&path).is_err(),
            "a frame the reader cannot chunk must be refused"
        );

        // At the ceiling a one-frame file still reads (progress, no spin).
        write_wide_float_wav(
            &path,
            48_000,
            MAX_CHANNELS,
            &vec![0.5f32; MAX_CHANNELS as usize],
        );
        let mut r = WavReader::open(&path).unwrap().with_channel(3).unwrap();
        let mut back = [0.0f32; 4];
        assert_eq!(r.read_into(&mut back), 1, "the single frame is read");
        assert_eq!(back[0], 0.5);
        assert_eq!(r.read_into(&mut back), 0, "and then EOF");
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
        // mono float: data_bytes = frames * 4, i.e. 4 bytes per sample. The RIFF
        // field is `36 + data_bytes (+1 pad if odd)`; the guard is
        // `> u32::MAX - 37` (conservative, covers the odd-data pad even though our
        // writers always produce even data_bytes).
        let ok_frames = (u32::MAX as u64 - 37) / 4; // data_bytes just under the bound
        assert!(
            data_bytes(ok_frames, 1, 4).is_ok(),
            "just under the boundary is ok"
        );
        let err_frames = ok_frames + 1; // data_bytes just over
        assert!(
            data_bytes(err_frames, 1, 4).is_err(),
            "just over the boundary must refuse"
        );
    }

    /// Write a 24-bit PCM WAV by hand (the writer only does 16-bit/float — 24-bit is
    /// what *other* tools write, which is why the reader must accept it).
    fn write_pcm24(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|s| {
                let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                [v as u8, (v >> 8) as u8, (v >> 16) as u8]
            })
            .collect();
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        file.extend_from_slice(b"WAVEfmt ");
        file.extend_from_slice(&16u32.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes()); // PCM
        file.extend_from_slice(&channels.to_le_bytes());
        file.extend_from_slice(&rate.to_le_bytes());
        file.extend_from_slice(&(rate * channels as u32 * 3).to_le_bytes());
        file.extend_from_slice(&(channels * 3).to_le_bytes());
        file.extend_from_slice(&24u16.to_le_bytes());
        file.extend_from_slice(b"data");
        file.extend_from_slice(&(data.len() as u32).to_le_bytes());
        file.extend_from_slice(&data);
        std::fs::write(path, file).expect("write pcm24");
    }

    /// 24-bit PCM is what other tools write: it must read back exactly (sign extension
    /// and the 2^23 scale), and a 32-bit **PCM** file is still refused — only 32-bit
    /// float is a WAV this pool reads.
    #[test]
    fn a_twenty_four_bit_wav_reads_back() {
        let dir = std::env::temp_dir().join(format!("wav24-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t24.wav");
        write_pcm24(&path, 48_000, 1, &[0.0, 0.5, -0.5, 1.0, -1.0]);

        let mut r = WavReader::open(&path).expect("24-bit opens");
        assert_eq!(r.sample_rate(), 48_000);
        assert_eq!(r.channels(), 1);
        assert_eq!(r.total_frames(), 5);
        let mut back = vec![0.0f32; 5];
        assert_eq!(r.read_into(&mut back), 5);
        for (got, want) in back.iter().zip([0.0, 0.5, -0.5, 1.0, -1.0]) {
            assert!(
                (got - want).abs() < 1e-5,
                "24-bit sample {got} != {want} (sign extension or scale wrong)"
            );
        }

        // A 32-bit PCM file (format 1) is not a float WAV: refused, not guessed at.
        let bad = dir.join("bad.wav");
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(b"WAVEfmt ");
        file.extend_from_slice(&16u32.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes()); // PCM, not float
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&48_000u32.to_le_bytes());
        file.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        file.extend_from_slice(&4u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(b"data");
        file.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&bad, file).expect("write bad");
        assert!(WavReader::open(&bad).is_err(), "32-bit PCM is refused");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A crashed **24-bit** take recovers to its full length: `recover` describes
    /// the file at the width the header declares (three bytes a sample), not at a
    /// float-or-16-bit guess. The first draft passed `h.bits == 32` as a "float"
    /// flag, so a 24-bit take was described at 2 bytes a sample — `set_len` kept
    /// two thirds of the audio, `recover` still returned the *pre*-truncation
    /// frame count (so `Pool::recover` reported frames the file no longer held),
    /// and `is_finalized` then hid the loss forever.
    #[test]
    fn a_twenty_four_bit_take_recovers_to_its_full_length() {
        let dir = std::env::temp_dir().join(format!("wav24-recover-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t24-crash.wav");
        let samples: [f32; 6] = [0.0, 0.5, -0.5, 1.0, -1.0, 0.25];
        write_pcm24(&path, 48_000, 1, &samples);

        // The header a crashed take is left with: the placeholder size `write_header`
        // writes before any audio, so `is_finalized` is false and the pool's crash
        // pass calls `recover`.
        let mut bytes = std::fs::read(&path).expect("read the fixture");
        bytes[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, &bytes).expect("write the crashed fixture");
        assert!(
            !WavWriter::is_finalized(&path).unwrap(),
            "the fixture must look like a crashed take"
        );
        let audio = bytes[44..].to_vec();
        assert_eq!(audio.len(), samples.len() * 3, "3 bytes a 24-bit sample");

        let n = samples.len() as u64;
        assert_eq!(
            WavWriter::recover(&path).unwrap(),
            n,
            "the recovered count is the file's own frames"
        );

        // The take survived whole: the audio bytes are untouched, and the header now
        // declares them at their real width.
        let bytes = std::fs::read(&path).expect("read the recovered file");
        assert_eq!(&bytes[44..], &audio[..], "no audio byte was truncated away");
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            (n * 3) as u32,
            "the data size is 3 bytes a sample, not 2"
        );
        assert!(WavWriter::is_finalized(&path).unwrap());

        // …and the reader agrees with the report: every frame, at its own value.
        let mut r = WavReader::open(&path).unwrap();
        assert_eq!(r.total_frames(), n);
        let mut back = vec![0.0f32; n as usize];
        assert_eq!(r.read_into(&mut back), n as usize);
        for (got, want) in back.iter().zip(samples) {
            assert!(
                (got - want).abs() < 1e-5,
                "recovered 24-bit sample {got} != {want}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
