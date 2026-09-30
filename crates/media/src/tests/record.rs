use super::*;

#[test]
fn recorder_roundtrips_through_the_writer_thread() {
    let path = std::env::temp_dir().join(format!("media-record-rt-{}.wav", std::process::id()));
    let rec: Arc<dyn Recorder> = Arc::new(WavRecorder::start(&path, 48_000).unwrap());
    // 50k samples: below the 64k ring capacity, so no overrun is possible
    // regardless of writer-thread timing; the roundtrip proves the thread.
    let n = 50_000u64;
    let samples: Vec<f32> = (0..n).map(|i| (i % 1000) as f32 / 1000.0 - 0.5).collect();
    for s in &samples {
        rec.push(*s);
    }
    rec.stop().unwrap();
    drop(rec);

    assert_eq!(
        crate::wav::WavReader::open(&path).unwrap().total_frames(),
        n
    );
    let mut r = crate::wav::WavReader::open(&path).unwrap();
    let mut back = vec![0.0f32; n as usize];
    assert_eq!(r.read_into(&mut back), n as usize);
    for (a, b) in samples.iter().zip(&back) {
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn recorder_ignores_pushes_after_stop() {
    let path = std::env::temp_dir().join(format!("media-record-stop-{}.wav", std::process::id()));
    let rec: Arc<dyn Recorder> = Arc::new(WavRecorder::start(&path, 48_000).unwrap());
    for _ in 0..100 {
        rec.push(0.25);
    }
    rec.stop().unwrap();
    rec.push(0.5); // must be ignored
    drop(rec);
    assert_eq!(
        crate::wav::WavReader::open(&path).unwrap().total_frames(),
        100
    );
    let _ = std::fs::remove_file(&path);
}
