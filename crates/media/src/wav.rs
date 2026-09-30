//! Minimal RIFF/WAVE reader and writer, std only.
//!
//! Writer: 16-bit PCM mono/stereo, placeholder sizes written *before* any
//! audio and patched on [`WavWriter::finalize`] — so a take is
//! crash-recoverable from the first byte: [`WavWriter::recover`] rescans the
//! data chunk and patches the header of a file that died mid-write. A crashed
//! take is *short of its own declaration*, and recovery can only shorten a file:
//! bytes after a data chunk (a trailing `LIST`/`INFO` chunk, the RIFF pad byte
//! after odd-length 24-bit data) are another tool's business, not audio to
//! declare. Reader: 16-bit PCM and 32-bit float, mono or stereo (stereo →
//! channel 0; the Phase-0 graph is mono). `hound` stays the Phase-1 upgrade if
//! format edge cases (WAVEFORMATEXTENSIBLE, 24-bit, …) bite (Spike B note,
//! Alternatives).

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
                // The overshoot skips a body like any other chunk, so it carries
                // the same pad byte.
                let pad = size & 1;
                reader
                    .seek(SeekFrom::Current(size as i64 - n as i64 + pad as i64))
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
            // RIFF pads an odd-sized chunk body to a word boundary. Skipping only
            // the body leaves the reader on the pad byte, reads `[pad, 'd','a']`
            // as the next tag, and loses every chunk from there on — so a valid
            // foreign WAV with a `LIST`/`cue `/`bext` before `data` was refused
            // with "missing data chunk".
            let pad = size & 1;
            reader
                .seek(SeekFrom::Current(size as i64 + pad as i64))
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
    ///
    /// **Recovery shortens a file; it never lengthens one.** What it salvages is a
    /// take that is *short of its own declaration* — the header claims more audio
    /// than the file holds, which is the evidence a write died (the `0xFFFF_FFFF`
    /// placeholder [`write_header`] puts up front, or a size patched for the full
    /// take before the last flush landed).
    ///
    /// Bytes *after* the declared data are a **legal** shape, not a crash: a
    /// trailing `LIST`/`INFO`/`fact`/`cue` chunk, or the RIFF pad byte after
    /// odd-length 24-bit data. No take this crate wrote ever declares less than it
    /// wrote, so such a file is one another tool owns, and counting its metadata
    /// as frames would re-declare that metadata as a user's take. It is therefore
    /// **refused** and left byte for byte as it was — and [`Self::is_finalized`]
    /// does not offer it to the pool's crash pass in the first place.
    pub fn recover(path: &Path) -> Result<u64, String> {
        let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let h = parse_header(&mut file)?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();
        let declared_end = h.data_offset + h.data_bytes as u64;
        if declared_end < file_len {
            return Err(format!(
                "{}: the data chunk ends at byte {declared_end} but the file is {file_len} \
                 bytes — the {extra} trailing bytes are a chunk or a pad byte, not a crashed \
                 take, so it is left alone",
                path.display(),
                extra = file_len - declared_end
            ));
        }
        // What a salvage keeps: the whole frames the file *holds*, capped by what
        // it declares and floored to a frame boundary. That cap is the structural
        // half of "recovery never grows a file" — the size computed below is at
        // most what is on disk, so `set_len` can only drop a torn tail.
        let held_bytes = file_len.saturating_sub(h.data_offset);
        let frames = held_bytes.min(h.data_bytes as u64) / h.block_align();
        // Patch a *frame-aligned* size: a torn tail from a crash mid-flush is
        // truncated away so the recovered file is formally well-formed
        // (kimi review finding 7). Refuse a take that would overflow the u32
        // size field rather than writing a corrupt small header (>4 GiB). The
        // sample width is the header's own (`bits / 8`) — the same term
        // `block_align` uses — so a 24-bit take is described at three bytes a
        // sample and survives recovery whole.
        let data_bytes = data_bytes(frames, h.channels, h.bytes_per_sample())?;
        debug_assert!(
            h.data_offset + data_bytes <= file_len,
            "a salvage is bounded by the bytes the file holds"
        );
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

    /// Whether a file needs no crash recovery: **the file holds at least every
    /// frame it declares** (`data_offset + data_bytes <= file_len`). A take that
    /// is *short of its own declaration* reports `false` and is what the pool's
    /// crash pass recovers — the `0xFFFF_FFFF` placeholder [`write_header`] writes
    /// before any audio, or a header patched for more audio than the write reached.
    ///
    /// A file that holds *more* than it declares is `true`, and that is the whole
    /// point: a trailing `LIST`/`INFO`/`fact`/`cue` chunk and the RIFF pad byte
    /// after odd-length 24-bit data are legal RIFF, so "the declared size is not
    /// the file length" is the *absence* of a shape, never the evidence of a
    /// crash — and it is not evidence in that direction, because the trailing bytes
    /// are not audio. No take this crate wrote ever declares less than it wrote, so
    /// a file in that state belongs to another tool and is left alone;
    /// [`Self::recover`] refuses it outright if it is called directly.
    pub fn is_finalized(path: &Path) -> Result<bool, String> {
        let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let h = parse_header(&mut file)?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();
        Ok(h.data_offset + h.data_bytes as u64 <= file_len)
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
#[path = "tests/wav.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/float_tests.rs"]
mod float_tests;
