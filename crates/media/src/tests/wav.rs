use super::*;
use std::mem;

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

/// A **genuinely crashed take** — the header declares more audio than the file
/// holds — is still the case `recover` exists for, and it recovers exactly as
/// before: every frame that landed is kept, the torn tail is cut at a frame
/// boundary, and the file gets *shorter*, never longer.
///
/// The fixture is a real writer's take (placeholder header, no `finalize`, so
/// the declared size is `0xFFFF_FFFF` — far longer than the file) with the
/// last flush torn off, leaving a partial frame.
#[test]
fn a_crashed_take_that_is_short_of_its_own_declaration_recovers() {
    let path = tmp("short-of-declaration");
    let n = 1234u64;
    {
        let mut w = WavWriter::create(&path, 48_000, 1).unwrap();
        w.write(&vec![0.5; n as usize]).unwrap();
        w.flush().unwrap();
        mem::forget(w); // suppress Drop's best-effort finalize — the crash
    }
    // A torn tail: the last 3 bytes are a partial frame the flush never
    // completed, so the file holds 1232 whole frames and half of the next.
    let written = std::fs::metadata(&path).unwrap().len();
    assert_eq!(written, 44 + n * 2);
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(written - 3).unwrap();
    drop(file);
    let torn = std::fs::metadata(&path).unwrap().len();
    let kept = (torn - 44) / 2;
    assert_eq!(kept, n - 2, "the fixture lost a partial frame");

    assert!(
        !WavWriter::is_finalized(&path).unwrap(),
        "a take short of its own declaration is not finalized"
    );
    let recovered = WavWriter::recover(&path).unwrap();
    assert_eq!(
        recovered, kept,
        "every whole frame that landed is recovered"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        44 + kept * 2,
        "the torn partial frame is cut, and the file is no longer than it was"
    );
    assert!(
        torn < written,
        "and recovery never grew what was on disk ({torn} < {written})"
    );
    assert!(WavWriter::is_finalized(&path).unwrap());
    let mut r = WavReader::open(&path).unwrap();
    assert_eq!(r.total_frames(), kept);
    let mut back = vec![0.0f32; kept as usize];
    assert_eq!(r.read_into(&mut back), kept as usize);
    assert!(back.iter().all(|s| (*s - 0.5).abs() < 1e-3));
    let _ = std::fs::remove_file(&path);
}

/// A **foreign** WAV carrying a trailing `LIST`/`INFO` chunk is not a crashed
/// take, and `recover` must not turn its metadata into audio. Every other tool
/// that writes a WAV (a DAW export, `ffmpeg`, Audacity) may append `LIST`/
/// `INFO`/`fact`/`cue` chunks *after* the data chunk; the file then holds more
/// than its data chunk declares.
///
/// The pre-fix `is_finalized` was `declared_end == file_len`, so this file read
/// as unfinalized, and `recover`'s rescan counted the trailing bytes as audio:
/// the header was rewritten to declare them, `set_len` could not shrink the
/// file (there was nothing to cut), and `is_finalized` then reported the
/// inflated file as good. The metadata became part of the user's take.
#[test]
fn a_trailing_chunk_is_neither_crashed_nor_audio() {
    let path = tmp("trailing-chunk");
    let n = 1_000u64;
    // A DAW-shaped 16-bit mono file: 2000 audio bytes at 44, then a 28-byte
    // `LIST` chunk at 2044. (The writer is dropped before the read: its
    // `finalize` patch is still in the `BufWriter` until then.)
    {
        let mut w = WavWriter::create(&path, 48_000, 1).unwrap();
        w.write(&vec![0.5; n as usize]).unwrap();
        w.finalize().unwrap();
    }
    let mut bytes = std::fs::read(&path).unwrap();
    let info = b"INFOISFTthisisatool\x00";
    let mut list = Vec::new();
    list.extend_from_slice(b"LIST");
    list.extend_from_slice(&(info.len() as u32).to_le_bytes());
    list.extend_from_slice(info);
    bytes.extend_from_slice(&list);
    // And the RIFF size the foreign tool wrote counts the chunk too.
    let riff = ((bytes.len() - 8) as u32).to_le_bytes();
    bytes[4..8].copy_from_slice(&riff);
    std::fs::write(&path, &bytes).unwrap();
    let file_len = std::fs::metadata(&path).unwrap().len();
    assert_eq!(file_len, 2072, "2000 audio bytes + a 28-byte LIST chunk");

    assert!(
        WavWriter::is_finalized(&path).unwrap(),
        "holding a trailing chunk is legal RIFF, not an unfinalized take"
    );
    let before = std::fs::read(&path).unwrap();
    let refused = WavWriter::recover(&path)
        .expect_err("recovery must refuse a file whose data chunk is not the file's tail");
    assert!(
        refused.contains("trailing") || refused.contains("left alone"),
        "the refusal must say why: {refused}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused recovery must not touch a single byte"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        file_len,
        "and must never grow the file"
    );

    // The audio the reader sees is the take and nothing else: 1000 frames, not
    // the 13 the `LIST` chunk's bytes would have become.
    let mut r = WavReader::open(&path).unwrap();
    assert_eq!(r.total_frames(), n);
    let mut back = vec![0.0f32; n as usize];
    assert_eq!(r.read_into(&mut back), n as usize);
    assert!(back.iter().all(|s| (*s - 0.5).abs() < 1e-3));
    let _ = std::fs::remove_file(&path);
}

/// A **foreign WAV with an odd-sized chunk before `data`** is valid RIFF: the
/// pad byte is what puts the next chunk on a word boundary, and a conforming
/// writer emits it. The chunk walk skipped a body *without* its pad, so the
/// reader landed on the pad byte, read `[pad, 'd','a']` as the next tag, and
/// `parse_header` reported "missing data chunk" — a valid file refused with no
/// diagnostic about alignment, so `Pool::list` filed it in `errors` and
/// `import` failed. The writer never emits such a chunk, which is why no
/// round-trip test could see it — this file is hand-spliced.
#[test]
fn an_odd_sized_chunk_before_data_is_skipped_with_its_pad_byte() {
    let path = tmp("odd-chunk-before-data");
    let n = 1_000u64;
    {
        let mut w = WavWriter::create(&path, 48_000, 1).unwrap();
        w.write(&vec![0.5; n as usize]).unwrap();
        w.finalize().unwrap();
    }
    // Splice an 11-byte `LIST`/`INFO` chunk — the odd-length tag other tools
    // write routinely — and its pad byte in front of the `data` tag at 36.
    let body = b"INFOISFTav\x00";
    assert_eq!(body.len() % 2, 1, "an odd length is what needs the pad");
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"LIST");
    chunk.extend_from_slice(&(body.len() as u32).to_le_bytes());
    chunk.extend_from_slice(body);
    chunk.push(0); // the RIFF pad byte, so `data` starts word-aligned
    let mut bytes = std::fs::read(&path).unwrap();
    let tail = bytes.split_off(36); // the `data` tag, its size, the audio
    bytes.extend_from_slice(&chunk);
    bytes.extend_from_slice(&tail);
    // And the RIFF size the foreign tool wrote counts the chunk and its pad.
    let riff = ((bytes.len() - 8) as u32).to_le_bytes();
    bytes[4..8].copy_from_slice(&riff);
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        44 + chunk.len() as u64 + n * 2,
        "2000 audio bytes + the LIST chunk and its pad"
    );

    let mut r = WavReader::open(&path)
        .expect("an odd-sized chunk before `data` is not a reason to refuse a file");
    assert_eq!(r.sample_rate(), 48_000);
    assert_eq!(r.total_frames(), n);
    let mut back = vec![0.0f32; n as usize];
    assert_eq!(r.read_into(&mut back), n as usize);
    assert!(back.iter().all(|s| (*s - 0.5).abs() < 1e-3));

    // The pool's crash pass asks the same question of this file, through the
    // same walk — it is a well-formed take that happens to carry metadata.
    assert!(
        WavWriter::is_finalized(&path).unwrap(),
        "the chunk walk must reach the data chunk for the recovery predicate too"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn rejects_garbage() {
    let path = tmp("garbage");
    std::fs::write(&path, b"not a wave file at all").unwrap();
    assert!(WavReader::open(&path).is_err());
    let _ = std::fs::remove_file(&path);
}
