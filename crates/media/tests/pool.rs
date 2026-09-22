//! P1.3.3 — the media pool: enumeration + crash recovery (finalize un-finalized
//! takes, rebuild missing `.peaks`). The pool is take-id-addressed.

use std::mem;
use std::path::PathBuf;

use media::peaks::PeakFile;
use media::wav::{WavReader, WavWriter};
use media::Pool;

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("media-pool-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_finalized(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, value: f32) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), sample_rate, 1).unwrap();
    let mut buf = vec![0.0f32; 4096];
    let mut i = 0u64;
    while i < frames {
        let n = buf.len().min((frames - i) as usize);
        buf[..n].fill(value);
        w.write(&buf[..n]).unwrap();
        i += n as u64;
    }
    w.finalize().unwrap();
}

/// Write a "crashed" take: header with placeholder sizes + audio, but *no*
/// finalize (Drop is suppressed so best-effort finalize doesn't run).
fn write_crashed(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, value: f32) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), sample_rate, 1).unwrap();
    let mut buf = vec![0.0f32; 4096];
    let mut i = 0u64;
    while i < frames {
        let n = buf.len().min((frames - i) as usize);
        buf[..n].fill(value);
        w.write(&buf[..n]).unwrap();
        i += n as u64;
    }
    w.flush().unwrap();
    mem::forget(w); // suppress Drop's best-effort finalize — the "crash"
}

#[test]
fn list_returns_sources_with_frames_and_peaks_status() {
    let dir = tmp_dir("list");
    write_finalized(&dir, "take-1.ch0", 5000, 48_000, 0.5);
    write_finalized(&dir, "take-1.ch1", 3000, 48_000, -0.25); // no peaks written

    let pool = Pool::open(&dir).unwrap();
    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 2);

    let c0 = index.sources.iter().find(|s| s.id == "take-1.ch0").unwrap();
    assert_eq!(c0.frames, 5000);
    assert_eq!(c0.sample_rate, 48_000);
    assert!(c0.peaks_missing, "ch0 has no sidecar in this test");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recover_finalizes_a_crashed_take() {
    let dir = tmp_dir("crash");
    let frames = 1234u64;
    write_crashed(&dir, "take-2.ch0", frames, 48_000, 0.7);

    let pool = Pool::open(&dir).unwrap();
    // before recover, the take is not finalized
    assert!(!media::wav::WavWriter::is_finalized(&dir.join("take-2.ch0.wav")).unwrap());

    let report = pool.recover().unwrap();
    assert_eq!(report.finalized.len(), 1, "one crashed take recovered");
    assert_eq!(report.finalized[0].0, "take-2.ch0");
    assert_eq!(report.finalized[0].1, frames);

    // recovered file is now well-formed and readable to its full length
    assert!(media::wav::WavWriter::is_finalized(&dir.join("take-2.ch0.wav")).unwrap());
    let mut r = WavReader::open(&dir.join("take-2.ch0.wav")).unwrap();
    assert_eq!(r.total_frames(), frames);
    let mut back = vec![0.0f32; frames as usize];
    assert_eq!(r.read_into(&mut back), frames as usize);
    assert!(back.iter().all(|s| (*s - 0.7).abs() < 1e-4));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recover_rebuilds_missing_peaks() {
    let dir = tmp_dir("peaks");
    write_finalized(&dir, "take-3.ch0", 4000, 44_100, 0.3);
    let peaks_path = dir.join("take-3.ch0.peaks");
    assert!(!peaks_path.exists());

    let pool = Pool::open(&dir).unwrap();
    let report = pool.recover().unwrap();
    assert_eq!(report.rebuilt_peaks, vec!["take-3.ch0".to_string()]);
    assert!(peaks_path.exists(), "peaks sidecar rebuilt");

    // the rebuilt sidecar is readable (PeakData = (base_bin, levels, frames, sample_rate, per-level))
    let (_, _, frames, sample_rate, _) = PeakFile::read(&peaks_path).unwrap();
    assert_eq!(frames, 4000);
    assert_eq!(sample_rate, 44_100);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn path_for_maps_stem_to_wav() {
    let dir = tmp_dir("path");
    write_finalized(&dir, "take-4.ch1", 1000, 48_000, 0.1);
    let pool = Pool::open(&dir).unwrap();
    assert_eq!(
        pool.path_for("take-4.ch1"),
        Some(dir.join("take-4.ch1.wav"))
    );
    assert!(pool.path_for("nope").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 440 Hz sine, so pitch survives as a countable property.
fn write_tone(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, freq: f64) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), sample_rate, 1).unwrap();
    let samples: Vec<f32> = (0..frames)
        .map(|i| (std::f64::consts::TAU * freq * i as f64 / sample_rate as f64).sin() as f32 * 0.5)
        .collect();
    w.write(&samples).unwrap();
    w.finalize().unwrap();
}

/// Read a mono WAV back in full, with its header facts.
fn read_all(path: &std::path::Path) -> (Vec<f32>, u32, u64) {
    let mut r = WavReader::open(path).unwrap();
    let rate = r.sample_rate();
    let frames = r.total_frames();
    let mut buf = vec![0.0f32; frames as usize];
    assert_eq!(r.read_into(&mut buf), frames as usize);
    (buf, rate, frames)
}

#[test]
fn import_converts_a_foreign_rate_once() {
    // The reported bug: a 44.1 kHz file in a 48 kHz session was refused outright.
    let src_dir = tmp_dir("import-src");
    let pool_dir = tmp_dir("import-pool");
    write_tone(&src_dir, "jam", 44_100, 44_100, 440.0);

    let pool = Pool::open(&pool_dir).unwrap();
    let done = pool.import(&src_dir.join("jam.wav"), 48_000).unwrap();

    assert!(done.converted, "44.1 kHz must be resampled");
    assert_eq!((done.from_rate, done.to_rate), (44_100, 48_000));
    assert_eq!((done.frames_in, done.frames_out), (44_100, 48_000));
    assert_eq!(done.preserved, None, "imported material stays where it was");

    // The pool now holds a session-rate source…
    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 1);
    assert_eq!(index.sources[0].id, "jam");
    assert_eq!(index.sources[0].sample_rate, 48_000);
    assert_eq!(index.sources[0].frames, 48_000);
    assert!(!index.sources[0].peaks_missing, "import derives the peaks");

    // …whose audio is the same tone at full level, not a pitch-shifted copy.
    let (audio, rate, frames) = read_all(&pool_dir.join("jam.wav"));
    assert_eq!((rate, frames), (48_000, 48_000));
    let crossings = audio
        .windows(2)
        .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
        .count();
    assert!(
        (crossings as i64 - 880).abs() <= 2,
        "440 Hz over 1 s should cross zero ~880 times, got {crossings}"
    );
    let middle = &audio[256..audio.len() - 256];
    let rms = (middle.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / middle.len() as f64).sqrt();
    assert!(
        (rms - 0.5 / std::f64::consts::SQRT_2).abs() < 0.01,
        "level changed on import: rms {rms:.4}"
    );

    let _ = std::fs::remove_dir_all(&src_dir);
    let _ = std::fs::remove_dir_all(&pool_dir);
}

#[test]
fn import_copies_a_matching_rate_bit_for_bit() {
    let src_dir = tmp_dir("import-copy-src");
    let pool_dir = tmp_dir("import-copy-pool");
    // 16-bit PCM: a re-quantizing import would show up as different bytes.
    let mut w = WavWriter::create(&src_dir.join("take.wav"), 48_000, 1).unwrap();
    w.write(&vec![0.25f32; 4800]).unwrap();
    w.finalize().unwrap();

    let pool = Pool::open(&pool_dir).unwrap();
    let done = pool.import(&src_dir.join("take.wav"), 48_000).unwrap();
    assert!(!done.converted, "a matching rate must not be resampled");
    assert_eq!((done.frames_in, done.frames_out), (4800, 4800));
    assert_eq!(
        std::fs::read(src_dir.join("take.wav")).unwrap(),
        std::fs::read(pool_dir.join("take.wav")).unwrap(),
        "the copy must be byte-identical"
    );

    let _ = std::fs::remove_dir_all(&src_dir);
    let _ = std::fs::remove_dir_all(&pool_dir);
}

#[test]
fn conform_brings_a_hand_filled_pool_to_the_session_rate() {
    let dir = tmp_dir("conform");
    write_tone(&dir, "old.ch0", 44_100, 44_100, 440.0); // foreign rate
    write_finalized(&dir, "old.ch1", 48_000, 48_000, 0.1); // already fine

    let pool = Pool::open(&dir).unwrap();
    let report = pool.conform(48_000).unwrap();

    assert_eq!(report.converted.len(), 1, "only the foreign source is touched");
    let done = &report.converted[0];
    assert_eq!(done.id, "old.ch0");
    assert_eq!((done.from_rate, done.to_rate), (44_100, 48_000));
    assert_eq!((done.frames_in, done.frames_out), (44_100, 48_000));

    // The original is preserved under a name the pool does not index.
    let preserved = done.preserved.clone().expect("the original is kept");
    assert_eq!(preserved, dir.join("old.ch0.wav.pre44100"));
    let (_, rate, frames) = read_all(&preserved);
    assert_eq!((rate, frames), (44_100, 44_100));

    // The pool now reads as one rate, with peaks rebuilt for the new length.
    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 2, "the preserved original is not a source");
    assert!(index.sources.iter().all(|s| s.sample_rate == 48_000));
    let (_, _, frames, rate, _) = PeakFile::read(&dir.join("old.ch0.peaks")).unwrap();
    assert_eq!((frames, rate), (48_000, 48_000));

    // Idempotent: a second pass has nothing to do.
    let again = pool.conform(48_000).unwrap();
    assert!(again.converted.is_empty());
    assert!(again.errors.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_and_conform_refuse_what_they_cannot_do() {
    let dir = tmp_dir("conform-guards");
    write_finalized(&dir, "s1", 1000, 44_100, 0.1); // foreign, so conform has work
    let pool = Pool::open(&dir).unwrap();

    assert!(pool.import(&dir.join("s1.wav"), 0).is_err(), "zero session rate");
    assert!(pool.import(&dir.join("missing.wav"), 48_000).is_err(), "missing source");
    assert!(pool.conform(0).is_err(), "zero session rate");

    // An unreadable file is reported, not fatal: the readable one still conforms.
    std::fs::write(dir.join("broken.wav"), b"not a wav").unwrap();
    let report = pool.conform(48_000).unwrap();
    assert_eq!(report.errors.len(), 1, "the broken file is reported");
    assert!(report.errors[0].0.ends_with("broken.wav"));
    assert!(
        report.converted.iter().any(|c| c.id == "s1"),
        "the readable source is still imported"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
