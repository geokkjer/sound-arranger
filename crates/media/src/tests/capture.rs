use super::*;

/// A virtual 4-channel device: interleaved frames, one sine per channel.
fn feed_interleaved(source: &Spsc<f32>, channels: usize, frames: usize, sr: u32) {
    let freqs = [440.0f32, 660.0, 880.0, 220.0];
    let mut phase = [0.0f64; 4];
    let mut i = 0usize;
    while i < frames * channels {
        for (k, s) in phase.iter_mut().enumerate() {
            let v = (std::f64::consts::TAU * *s).sin() as f32 * 0.5;
            while !source.try_push(v) {
                std::thread::sleep(std::time::Duration::from_micros(10));
            }
            *s += freqs[k] as f64 / sr as f64;
            i += 1;
            if i >= frames * channels {
                break;
            }
        }
    }
}

#[test]
fn capture_writes_per_channel_pool_sources() {
    let dir = std::env::temp_dir().join(format!("p12-pool-{}", std::process::id()));
    let channels = 4;
    let sr = 48_000u32;
    let frames = 5120usize;
    let source = Arc::new(Spsc::new(1 << 18));
    let cap = Capture::start(&dir, "take1", channels, sr, sr, source.clone()).unwrap();
    feed_interleaved(&source, channels, frames, sr);
    // wait for the demux to drain
    std::thread::sleep(std::time::Duration::from_millis(100));
    cap.stop().unwrap();

    for k in 0..channels {
        let wav_path = cap.path_for(k);
        let peaks_path = dir.join(format!("take1.ch{k}.peaks"));
        assert!(wav_path.exists(), "channel {k} source WAV");
        assert!(peaks_path.exists(), "channel {k} peaks sidecar");

        let mut r = crate::wav::WavReader::open(&wav_path).unwrap();
        assert_eq!(r.sample_rate(), sr);
        assert_eq!(r.total_frames(), frames as u64);
        let mut back = vec![0.0f32; frames];
        assert_eq!(r.read_into(&mut back), frames);

        // float round-trip: the pool source is bit-identical to the input
        let freqs = [440.0f32, 660.0, 880.0, 220.0];
        let mut phase = 0.0f64;
        for (i, s) in back.iter().enumerate() {
            let expected = (std::f64::consts::TAU * phase).sin() as f32 * 0.5;
            phase += freqs[k] as f64 / sr as f64;
            assert_eq!(*s, expected, "ch{k} sample {i} must round-trip exactly");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A zero clock is refused by `start`, while the message can still name the
/// parameter. Downstream it is a division: the drift ratio is
/// `input_rate / sample_rate` and the demux sizes a session buffer from its
/// reciprocal, so `input_rate == 0` made `session_cap` an overflowing `usize` —
/// a demux-thread panic in debug that `stop` reports as the wrong cause, and in
/// release a take that records one DC sample per batch while `pending` grows
/// without bound.
#[test]
fn a_zero_rate_is_refused_by_start() {
    let dir = std::env::temp_dir().join(format!("p12-rate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sr = 48_000u32;

    let Err(e) = Capture::start(&dir, "zerod", 2, sr, 0, Arc::new(Spsc::new(4096))) else {
        panic!("a zero device rate must be refused, not divided by");
    };
    assert!(e.contains("input_rate"), "the message names the rate: {e}");
    let Err(e) = Capture::start(&dir, "zerod", 2, 0, sr, Arc::new(Spsc::new(4096))) else {
        panic!("a zero session rate must be refused, not divided by");
    };
    assert!(e.contains("sample_rate"), "the message names the rate: {e}");

    // Refused before anything was created: no pool dir, no take, no thread.
    assert!(
        !dir.exists(),
        "a refused take leaves no pool material behind: {}",
        dir.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A device that stops mid-frame **keeps** what it delivered: the tail is padded
/// with silence, never dropped. The samples the other channels did deliver are real
/// material, silence is the honest value for a channel that never delivered, and
/// padding keeps the per-channel stems the same length (dropping the tail would
/// leave them unequal).
///
/// This is also why the host's capture test flaked: whether a stray sample happened
/// to be sitting in the buffer at stop was pure timing, and the old code turned that
/// into an error.
#[test]
fn a_tail_that_never_completed_a_frame_is_padded_not_dropped() {
    let dir = std::env::temp_dir().join(format!("p12-tail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (sr, channels) = (48_000u32, 2usize);
    let source = Arc::new(Spsc::new(4096));
    for i in 0..5 {
        assert!(source.try_push(0.1 * (i as f32 + 1.0)), "ch0 {i}");
        assert!(source.try_push(-0.2), "ch1 {i}");
    }
    // ch0 of a frame whose ch1 never arrived: an odd sample count for a stereo device.
    assert!(source.try_push(0.75), "the stray ch0 sample");

    let cap = Capture::start(&dir, "tail", channels, sr, sr, source).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    cap.stop()
        .expect("an incomplete tail is kept, never an error");
    assert_eq!(cap.frames(), 6, "five whole frames plus the padded one");

    let read = |k: usize| {
        let mut r = crate::wav::WavReader::open(&cap.path_for(k)).unwrap();
        let mut buf = vec![0.0f32; r.total_frames() as usize];
        let n = r.read_into(&mut buf);
        buf.truncate(n);
        buf
    };
    let (ch0, ch1) = (read(0), read(1));
    assert_eq!(
        ch0.len(),
        ch1.len(),
        "the per-channel stems stay equal length"
    );
    assert_eq!(
        ch0.last().copied(),
        Some(0.75),
        "the delivered sample is kept"
    );
    assert_eq!(
        ch1.last().copied(),
        Some(0.0),
        "the channel that never delivered is silence, not a shorter file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A device whose clock drifts from the session must be recorded into SESSION
/// frames: the take's length is device_frames / ratio, and the pitch is
/// preserved (a rate-mismatched take would play shifted).
fn assert_drift_corrected(input_sr: u32, session_sr: u32) {
    let dir = std::env::temp_dir().join(format!("p12-drift-{}-{input_sr}", std::process::id()));
    let channels = 1u32;
    let dev_frames = session_sr as u64; // 1 s of device frames
    let source = Arc::new(Spsc::new(1 << 18));
    let cap = Capture::start(
        &dir,
        "drift1",
        channels as usize,
        session_sr,
        input_sr,
        source.clone(),
    )
    .unwrap();

    // feed a 440 Hz tone at the DEVICE clock
    let mut phase = 0.0f64;
    let mut pushed = 0u64;
    while pushed < dev_frames {
        let v = (std::f64::consts::TAU * phase).sin() as f32 * 0.5;
        while !source.try_push(v) {
            std::thread::sleep(std::time::Duration::from_micros(10));
        }
        phase += 440.0 / input_sr as f64;
        pushed += 1;
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    cap.stop().unwrap();

    let wav_path = cap.path_for(0);
    let mut r = crate::wav::WavReader::open(&wav_path).unwrap();
    // device_frames / ratio = session_frames (the drift absorbs the fractional
    // remainder; ± a couple for the batch-pull tail).
    let expected = (dev_frames as f64 * session_sr as f64 / input_sr as f64).round() as i64;
    let got = r.total_frames() as i64;
    assert!(
        (got - expected).abs() <= 4,
        "drifted take must land in session frames, got {got} (expected ~{expected})"
    );
    // pitch preserved: zero-crossings over the recorded span ≈ 440 * seconds * 2
    let mut back = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut back);
    let crossings = back[..n]
        .windows(2)
        .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
        .count();
    let expect_cross = 440.0 * (got as f64 / session_sr as f64) * 2.0;
    assert!(
        (crossings as f64 - expect_cross).abs() < expect_cross * 0.02,
        "pitch drifted: {crossings} vs ~{expect_cross}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn faster_drift_records_into_session_frames() {
    assert_drift_corrected(48_001, 48_000); // device clock faster than session
}

#[test]
fn slower_drift_records_into_session_frames() {
    assert_drift_corrected(47_999, 48_000); // device clock slower (the ratio < 1 case)
}
