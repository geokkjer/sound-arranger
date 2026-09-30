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
/// what *other* tools write, which is why the reader must accept it). No pad
/// byte: see [`write_pcm24_padded`] for the conforming odd-length shape.
fn write_pcm24(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
    let data = pcm24_bytes(samples);
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

/// 24-bit samples as little-endian three-byte integers, `i24` the width a 24-bit
/// file is described at.
fn pcm24_bytes(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|s| {
            let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            [v as u8, (v >> 8) as u8, (v >> 16) as u8]
        })
        .collect()
}

/// A 24-bit PCM WAV **with the RIFF pad byte** — what a conforming writer emits
/// when the data chunk has an odd length (5 frames of `i24` is 15 bytes), and
/// what this crate's writer does not emit. The file is one byte longer than its
/// data chunk declares, and the pad byte is not audio.
fn write_pcm24_padded(path: &Path, rate: u32, samples: &[f32]) {
    let data = pcm24_bytes(samples);
    assert_eq!(data.len() % 2, 1, "an odd length is what needs the pad");
    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((36 + data.len() + 1) as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes()); // PCM
    file.extend_from_slice(&1u16.to_le_bytes()); // mono
    file.extend_from_slice(&rate.to_le_bytes());
    file.extend_from_slice(&(rate * 3).to_le_bytes());
    file.extend_from_slice(&3u16.to_le_bytes());
    file.extend_from_slice(&24u16.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(&data);
    file.push(0); // the pad byte, so the next chunk would start word-aligned
    std::fs::write(path, file).expect("write padded pcm24");
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

/// **The RIFF pad byte after odd-length data is not a crashed take.** A
/// conforming writer pads an odd-length data chunk to a word boundary, so a
/// 24-bit file with an odd frame count is *one byte longer* than its data chunk
/// declares — a legal shape, not an unfinished write. The pre-fix
/// `is_finalized` was `declared_end == file_len`, so it read `false`, and the
/// pool's crash pass then ran `recover` on a file this crate never wrote: the
/// rescan counted the pad byte as audio, `set_len` cut it off, and the header
/// was rewritten. This must be refused, byte for byte, and the pad byte must
/// survive.
#[test]
fn a_riff_pad_byte_after_odd_data_is_neither_crashed_nor_audio() {
    let dir = std::env::temp_dir().join(format!("wav24-pad-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("t24-pad.wav");
    let samples: [f32; 5] = [0.0, 0.5, -0.5, 1.0, -1.0]; // 5 × 3 = 15 bytes, odd
    write_pcm24_padded(&path, 48_000, &samples);
    let file_len = std::fs::metadata(&path).unwrap().len();
    assert_eq!(file_len, 60, "44 header + 15 audio + 1 pad byte");

    assert!(
        WavWriter::is_finalized(&path).unwrap(),
        "the pad byte is legal RIFF, not an unfinalized take"
    );
    let before = std::fs::read(&path).unwrap();
    WavWriter::recover(&path)
        .expect_err("recovery must refuse a file whose only extra byte is a pad byte");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused recovery must not touch a single byte"
    );

    // The reader still sees the five frames, and not a sixth cut in half by
    // the pad byte.
    let mut r = WavReader::open(&path).unwrap();
    assert_eq!(r.total_frames(), samples.len() as u64);
    let mut back = vec![0.0f32; samples.len()];
    assert_eq!(r.read_into(&mut back), samples.len());
    for (got, want) in back.iter().zip(samples) {
        assert!((got - want).abs() < 1e-5, "24-bit sample {got} != {want}");
    }
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
