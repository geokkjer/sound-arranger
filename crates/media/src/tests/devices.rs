use super::*;

/// A mono source is duplicated across the device's channels, one source
/// sample per FRAME (not per output sample — that would halve the rate).
#[test]
fn output_duplicates_mono_source_across_channels() {
    let ring = Spsc::new(16);
    for v in [0.1f32, 0.2, 0.3, 0.4] {
        assert!(ring.try_push(v), "ring must accept the source");
    }
    let mut out = [0.0f32; 8]; // 4 stereo frames
    let u = AtomicU64::new(0);
    fill_output(&mut out, &ring, 1, 2, &u);
    assert_eq!(out, [0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4]);
    assert_eq!(u.load(Ordering::Relaxed), 0, "no starve");
}

/// A stereo source passes through a stereo device frame-for-frame.
#[test]
fn output_passes_stereo_source_through() {
    let ring = Spsc::new(16);
    for v in [0.1f32, 0.9, 0.2, 0.8] {
        assert!(ring.try_push(v));
    }
    let mut out = [0.0f32; 4]; // 2 stereo frames
    let u = AtomicU64::new(0);
    fill_output(&mut out, &ring, 2, 2, &u);
    assert_eq!(out, [0.1, 0.9, 0.2, 0.8], "L/R stay paired");
    assert_eq!(u.load(Ordering::Relaxed), 0);
}

/// A mono device averages a stereo source instead of dropping a channel.
#[test]
fn output_downmixes_stereo_to_a_mono_device() {
    let ring = Spsc::new(16);
    for v in [0.4f32, 0.2] {
        assert!(ring.try_push(v));
    }
    let mut out = [0.0f32; 1];
    let u = AtomicU64::new(0);
    fill_output(&mut out, &ring, 2, 1, &u);
    assert!(
        (out[0] - 0.3).abs() < 1e-6,
        "mono device averages L/R, got {}",
        out[0]
    );
}

#[test]
fn output_silence_when_empty_counts_underruns() {
    let ring = Spsc::new(16);
    let mut out = [1.0f32; 4]; // 2 stereo frames
    let u = AtomicU64::new(0);
    fill_output(&mut out, &ring, 2, 2, &u);
    assert_eq!(out, [0.0; 4], "empty ring must play silence");
    assert_eq!(
        u.load(Ordering::Relaxed),
        2,
        "each starved frame is counted"
    );
}

/// A partial source frame is **not** consumed: both frames starve and the
/// lone sample stays put, so L/R never swap across the gap.
#[test]
fn output_partial_frame_starves_without_misaligning() {
    let ring = Spsc::new(16);
    assert!(ring.try_push(0.1)); // half of an expected stereo pair
    let mut out = [9.0f32; 4];
    let u = AtomicU64::new(0);
    fill_output(&mut out, &ring, 2, 2, &u);
    assert_eq!(out, [0.0; 4], "a partial frame plays silence");
    assert_eq!(u.load(Ordering::Relaxed), 2);
    assert_eq!(
        ring.len(),
        1,
        "the lone sample is left for a complete frame"
    );
}

#[test]
fn output_zero_channels_degrades_to_mono_not_crash() {
    // documents why `fill_output` guards device_channels.max(1): chunks_mut(0)
    // panics. A zero-channel device degrades to mono, never crashes.
    let ring = Spsc::new(16);
    for _ in 0..4 {
        assert!(ring.try_push(0.5));
    }
    let mut out = [1.0f32; 4];
    let u = AtomicU64::new(0);
    fill_output(&mut out, &ring, 1, 0, &u);
    assert_eq!(out, [0.5; 4]);
}

/// Prints what the default output device actually offers — the diagnostic for
/// "which config should negotiation pick". Ignored by default (real hardware).
#[test]
#[ignore = "needs real audio hardware; run: cargo test -p media -- --ignored output_device_report -- --nocapture"]
fn output_device_report() {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        eprintln!("no default output device");
        return;
    };
    let default = device
        .default_output_config()
        .expect("default output config");
    eprintln!(
        "default: {} ch @ {} Hz {:?}",
        default.channels(),
        default.sample_rate(),
        default.sample_format()
    );
    match device.supported_output_configs() {
        Ok(configs) => {
            for r in configs {
                eprintln!(
                    "  range: {} ch, {}..{} Hz, {:?}",
                    r.channels(),
                    r.min_sample_rate(),
                    r.max_sample_rate(),
                    r.sample_format()
                );
            }
        }
        Err(e) => eprintln!("supported_output_configs: {e}"),
    }
}

/// Negotiation prefers the device's own channel count — **not** the most
/// channels. A pro-audio node offers 1..=64; picking the widest put the stereo
/// mix on channels that were not the monitor pair (silent, uncounted).
#[test]
fn output_range_prefers_the_default_channel_count_over_more_channels() {
    let ranges = [
        (2u16, 1u32, 384_000u32, true),
        (64, 1, 384_000, true),
        (2, 1, 384_000, false),
    ];
    assert_eq!(
        best_output_range(&ranges, 48_000, 2, true),
        Some(0),
        "2-ch F32 beats 64-ch"
    );
}

/// …and requires the requested rate, preferring the default's format, and
/// reports "no range" when nothing covers it (so the caller flags the mismatch
/// rather than lying about the clock).
#[test]
fn output_range_requires_the_rate_and_prefers_the_default_format() {
    let ranges = [
        (2u16, 44_100u32, 44_100u32, true), // wrong rate
        (2, 44_100, 192_000, false),        // right rate, integer
        (2, 44_100, 192_000, true),         // right rate, f32  <- pick
    ];
    assert_eq!(
        best_output_range(&ranges, 48_000, 2, true),
        Some(2),
        "f32 over integer"
    );
    assert_eq!(
        best_output_range(&ranges, 48_000, 2, false),
        Some(1),
        "integer when preferred"
    );
    assert_eq!(
        best_output_range(&ranges, 22_050, 2, true),
        None,
        "below every min -> fall back"
    );
}

#[test]
fn input_pushes_every_channel_interleaved() {
    // A stereo input buffer [L0,R0,L1,R1,...] must reach the ring **whole**: that
    // is what `Capture` demuxes (`channels` interleaved samples per frame). Pushing
    // channel 0 alone made every multi-channel take silently corrupt — the bug this
    // test exists to prevent (the device path had no coverage before it).
    let ring = Spsc::new(16);
    let data = [0.1f32, 0.9, 0.2, 0.8, 0.3, 0.7];
    let overruns = AtomicU64::new(0);
    fill_input(&data, &ring, 2, &overruns);
    for expected in data {
        assert_eq!(
            ring.try_pop(),
            Some(expected),
            "interleaved order is preserved"
        );
    }
    assert_eq!(ring.try_pop(), None, "every sample, in order");
    assert_eq!(overruns.load(Ordering::Relaxed), 0);

    // A full ring drops samples and counts each drop, channel-interleaving intact.
    let small = Spsc::new(4);
    let overruns = AtomicU64::new(0);
    fill_input(&[1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0], &small, 2, &overruns);
    assert_eq!(small.try_pop(), Some(1.0));
    assert_eq!(small.try_pop(), Some(2.0));
    assert_eq!(small.try_pop(), Some(3.0));
    assert_eq!(small.try_pop(), Some(4.0));
    assert_eq!(overruns.load(Ordering::Relaxed), 2, "the rest were counted");
}
