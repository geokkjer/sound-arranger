//! The one test that exercises the **real input device** through `open_input` — the path
//! that recorded silently corrupt multi-channel takes (channel 0 duplicated into a
//! "mono" ring, then demuxed as `device_channels` interleaved frames: half the duration,
//! wrong channels). It is `#[ignore]`d because it needs a device and a signal; run it
//! with `cargo test -p media --test hardware_input -- --ignored`.
//!
//! What it asserts is the *shape* of the take, which is what the bug broke: the device's
//! own channel count and the full frame duration (a mono-ring bug halves it).

use std::sync::Arc;
use std::time::Duration;

use media::wav::WavReader;
use media::{Capture, Spsc};

#[test]
#[ignore = "needs a real input device"]
fn open_input_captures_every_channel_at_full_duration() {
    let ring = Arc::new(Spsc::new(1 << 18));
    let Ok(handle) = media::devices::open_input(ring.clone()) else {
        eprintln!("SKIP: no default input device");
        return;
    };
    let rate = handle.sample_rate;
    let channels = handle.channels as usize;

    let dir = std::env::temp_dir().join(format!("media-hw-input-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cap = Capture::start(&dir, "hw", channels, rate, rate, ring.clone()).expect("capture");

    let seconds = 0.7f64;
    std::thread::sleep(Duration::from_secs_f64(seconds));
    drop(handle);
    cap.stop().expect("the take finalizes");

    // Full duration: the device delivered ~`seconds` of frames, not half of them.
    let frames = cap.frames();
    let expected = (seconds * rate as f64) as u64;
    assert!(
        frames > expected * 8 / 10 && frames < expected * 12 / 10,
        "captured {frames} frames in {seconds} s at {rate} Hz (want ~{expected}) — \
         a mono ring demuxed as interleaved would halve this"
    );

    // …and one real source per device channel, each with those frames and peaks.
    assert_eq!(cap.channels(), channels);
    for k in 0..channels {
        let path = cap.path_for(k);
        let r = WavReader::open(&path).expect("the take source opens");
        assert_eq!(r.sample_rate(), rate);
        assert!(
            r.total_frames() > expected * 8 / 10,
            "channel {k} holds {} frames (want ~{expected})",
            r.total_frames()
        );
        assert!(
            dir.join(format!("hw.ch{k}.peaks")).exists(),
            "channel {k} has a peaks sidecar"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
