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
