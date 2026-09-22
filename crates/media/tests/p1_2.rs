//! P1.2 acceptance tests (recorder + adaptable mixer — p1-2 note):
//! - a virtual 4-channel device captured to the pool lands as 4 float-WAV
//!   sources + peaks sidecars (exact round-trip);
//! - the per-channel monitoring nodes route into the adaptable mixer
//!   (`channels = 4`): the summed master is correct and glitch-free while the
//!   virtual device and the arranger run concurrently at a paced rate;
//! - the mixer's `channels` mount param is replayable;
//! - the real-hardware device path captures the default input's channels.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use engine::*;
use media::*;

const SR: u32 = 48_000;
const CHANNELS: usize = 4;
const FREQS: [f32; 4] = [440.0, 660.0, 880.0, 220.0];
/// Equal-power center-pan gain each mono mixer channel sees (`pan = 0`).
const CENTER: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn engine() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory(
        "euclidean",
        plugins::euclidean_factory,
        plugins::euclidean::EUCLIDEAN_PORTS,
        &[],
    );
    e.register_factory(
        "scale",
        plugins::scale_factory,
        plugins::scale::SCALE_PORTS,
        &[],
    );
    e.register_factory(
        "tone",
        plugins::tone_factory,
        plugins::tone::TONE_PORTS,
        plugins::tone::TONE_PARAMS,
    );
    e.register_factory(
        "mixer",
        plugins::mixer_factory,
        plugins::mixer::MIXER_PORTS,
        plugins::mixer::MIXER_PARAMS,
    );
    e
}

/// A virtual multichannel device: one sine per channel, interleaved, fed in
/// paced blocks so the demux and the monitoring consumer keep up (the real
/// device runs at wall-clock pace too).
struct Device {
    phase: [f64; CHANNELS],
}

impl Device {
    fn new() -> Self {
        Device {
            phase: [0.0; CHANNELS],
        }
    }

    /// Push one interleaved frame (blocks until the source accepts).
    fn push_frame(&mut self, source: &Spsc<f32>) {
        for (k, freq) in FREQS.iter().enumerate() {
            let v = (TAU * self.phase[k]).sin() as f32 * 0.5;
            while !source.try_push(v) {
                std::thread::sleep(Duration::from_micros(10));
            }
            self.phase[k] += *freq as f64 / SR as f64;
        }
    }
}

/// Capture + monitoring → adaptable mixer, end to end: the summed master must
/// equal the four channel sines (glitch-free, paced concurrent device), while
/// the pool receives the float sources + peaks.
#[test]
fn capture_into_adaptable_mixer_and_pool() {
    let dir = std::env::temp_dir().join(format!("p12-e2e-{}", std::process::id()));
    let frames = SR as usize * 2; // 2 s of virtual device time

    let source = Arc::new(Spsc::new(1 << 18));
    let cap = Capture::start(&dir, "jam1", CHANNELS, SR, SR, source.clone()).unwrap();

    let mut e = engine();
    // Monitoring nodes first (indices 0..3) — the mixer's node lands after
    // them when its scheduled mount applies, so cords stay forward.
    let mut capture_ids = Vec::new();
    let mut underruns = Vec::new();
    for k in 0..CHANNELS {
        let node = CaptureNode::new(cap.channel_ring(k));
        underruns.push(node.underrun_counter());
        capture_ids.push(e.graph.add_node(
            NodeKind::Opaque(Box::new(node)),
            vec![Port {
                name: "audio",
                direction: Direction::Out,
                kind: SignalKind::Audio,
                channels: 1,
            }],
        ));
    }
    // The adaptable mixer (channels = the device's 4) claims the bus.
    e.mount("mixer", &[("channels", CHANNELS as f32)]).unwrap();
    e.render(BLOCK); // applies the mixer mount
    let mixer_id = e.graph.out_node.expect("the mixer claims the master bus");
    for (k, id) in capture_ids.iter().enumerate() {
        e.graph
            .connect(*id, "audio", mixer_id, &format!("ch{k}"))
            .unwrap();
    }
    // The setup render above popped the still-empty rings; the glitch-free
    // claim covers the paced run only.
    for c in &underruns {
        c.store(0, Ordering::Relaxed);
    }

    // The virtual device and the arranger run concurrently, both paced.
    let mut dev = Device::new();
    let mut out = Vec::with_capacity(frames);
    // Warm: fill ~0.5 s into the monitoring rings so the demux is well ahead
    // before the paced loop starts (the warm frames are part of the take).
    let warm_blocks = SR as usize / 2 / BLOCK;
    let blocks = frames / BLOCK;
    let fed_frames = (warm_blocks + blocks) * BLOCK;
    for _ in 0..warm_blocks {
        for _ in 0..BLOCK {
            dev.push_frame(&source);
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    std::thread::sleep(Duration::from_millis(10));
    for _ in 0..blocks {
        for _ in 0..BLOCK {
            dev.push_frame(&source);
        }
        // let the demux deliver the batch before the consumer pops it
        std::thread::sleep(Duration::from_micros(200));
        // The master is now the stereo mixer bus; render a stereo block.
        let mut block = [0.0f32; BLOCK * 2];
        e.render_into(&mut block);
        out.extend_from_slice(&block);
        std::thread::sleep(Duration::from_micros(300)); // ~10× real time, below the sustained rate
    }

    // Glitch-freedom is proven by the *exact* master content below (a dropped
    // monitoring sample would shift the stream and fail it); the counters are
    // diagnostic (kimi review finding 18 — no timing-dependent assertion).
    for (k, c) in underruns.iter().enumerate() {
        let n = c.load(Ordering::Relaxed);
        if n > 0 {
            eprintln!("note: monitoring channel {k} underran {n} samples (diagnostic)");
        }
    }

    // The master equals the center-panned sum of the four channel sines (fresh
    // device for the expected signal). Each channel is center-panned, so the L
    // channel carries the sum scaled by √2/2; the stereo master interleaves L,R.
    let mut expect_phase = [0.0f64; CHANNELS];
    for (i, s) in out.as_chunks::<2>().0.iter().map(|p| p[0]).enumerate() {
        let mut expected = 0.0f32;
        for k in 0..CHANNELS {
            expected += (TAU * expect_phase[k]).sin() as f32 * 0.5;
            expect_phase[k] += FREQS[k] as f64 / SR as f64;
        }
        let expected = expected * CENTER;
        assert!((s - expected).abs() < 1e-5, "sample {i}: {s} vs {expected}");
    }

    cap.stop().unwrap();
    for (k, freq) in FREQS.iter().enumerate() {
        let wav_path = cap.path_for(k);
        let peaks_path = dir.join(format!("jam1.ch{k}.peaks"));
        assert!(wav_path.exists(), "pool source ch{k}");
        assert!(peaks_path.exists(), "peaks sidecar ch{k}");
        let mut r = WavReader::open(&wav_path).unwrap();
        assert_eq!(
            r.total_frames(),
            fed_frames as u64,
            "pool source ch{k} holds every fed frame"
        );
        let mut back = vec![0.0f32; fed_frames];
        r.read_into(&mut back);
        let mut phase = 0.0f64;
        for (i, s) in back.iter().enumerate() {
            let expected = (TAU * phase).sin() as f32 * 0.5;
            phase += *freq as f64 / SR as f64;
            assert_eq!(
                *s, expected,
                "pool ch{k} sample {i} round-trips exactly (float)"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The mixer's `channels` mount param is part of the log: replaying a log
/// that mounts a 2-channel mixer renders byte-identically.
#[test]
fn mixer_channel_param_is_replayable() {
    let chain = |e: &mut Engine| {
        e.mount(
            "euclidean",
            &[
                ("steps", 8.0),
                ("pulses", 1.0),
                ("rotation", 1.0),
                ("pulses_per_beat", 4.0),
            ],
        )
        .unwrap();
        e.mount("scale", &[("root", 0.0)]).unwrap();
        e.mount("tone", &[("gain", 0.25), ("blip_len", 1200.0)])
            .unwrap();
        e.mount("mixer", &[("channels", 2.0)]).unwrap();
        e.patch(("euclidean", "triggers"), ("scale", "trigger"))
            .unwrap();
        e.patch(("scale", "note"), ("tone", "note")).unwrap();
        e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
    };

    let mut e1 = engine();
    chain(&mut e1);
    let out1 = e1.render(10_000);
    assert!(
        out1[6001..].iter().any(|s| *s != 0.0),
        "the chain sounds through the 2-ch mixer"
    );

    let mut e2 = engine();
    e2.replay_from(&e1.log).unwrap();
    let out2 = e2.render(10_000);
    assert_eq!(
        out1, out2,
        "the channels param must replay byte-identically"
    );
}

/// Real hardware: capture the default input's channels for a moment and land
/// the float sources + peaks in the pool. Skips politely without a device.
#[test]
#[ignore = "needs real audio hardware; run: cargo test -p media -- --ignored devices"]
fn hardware_capture_lands_pool_sources() {
    let (rate, ch) = match media::devices::default_input_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("SKIP: no default input device ({e})");
            return;
        }
    };
    let dir = std::env::temp_dir().join(format!("p12-hw-{}", std::process::id()));
    let source = Arc::new(Spsc::new(1 << 18));
    let cap = Capture::start(&dir, "hw", ch as usize, rate, rate, source.clone()).unwrap();
    let handle = media::devices::open_input(source.clone()).expect("open input");
    std::thread::sleep(Duration::from_millis(800));
    drop(handle);
    cap.stop().unwrap();
    for k in 0..ch as usize {
        let wav_path = cap.path_for(k);
        assert!(wav_path.exists(), "hardware capture ch{k} source WAV");
        let mut r = WavReader::open(&wav_path).unwrap();
        assert!(r.total_frames() > 0, "hardware capture ch{k} holds samples");
        let mut back = vec![0.0f32; r.total_frames() as usize];
        assert!(
            r.read_into(&mut back) > 0,
            "hardware capture ch{k} reads back"
        );
        let peaks_path = dir.join(format!("hw.ch{k}.peaks"));
        assert!(peaks_path.exists(), "hardware capture ch{k} peaks");
        let (base_bin, levels, frames, sr, _) = PeakFile::read(&peaks_path).unwrap();
        assert_eq!(
            (base_bin, sr),
            (256, rate),
            "peaks header matches the device"
        );
        assert_eq!(levels, PEAK_LEVELS);
        assert!(frames > 0, "peaks sidecar carries frames");
    }
    eprintln!("captured {ch} channels @ {rate} Hz into the pool (0 source overruns expected)");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pool's peaks sidecar matches an independent min/max reduction of the
/// pool WAV (acceptance criterion — kimi finding 17).
#[test]
fn pool_peaks_match_independent_reduction() {
    let dir = std::env::temp_dir().join(format!("p12-peaks-{}", std::process::id()));
    let frames = 5120usize;
    let source = Arc::new(Spsc::new(1 << 18));
    let cap = Capture::start(&dir, "peaktake", CHANNELS, SR, SR, source.clone()).unwrap();
    let mut dev = Device::new();
    for _ in 0..frames {
        dev.push_frame(&source);
    }
    std::thread::sleep(Duration::from_millis(100));
    cap.stop().unwrap();

    let wav_path = cap.path_for(0);
    let mut r = WavReader::open(&wav_path).unwrap();
    let mut back = vec![0.0f32; frames];
    assert_eq!(r.read_into(&mut back), frames);
    let (_, _, _, _, data) = PeakFile::read(&dir.join("peaktake.ch0.peaks")).unwrap();
    for bin in 0..(frames / 256) {
        let window = &back[bin * 256..(bin + 1) * 256];
        let mn = window.iter().copied().fold(f32::INFINITY, f32::min);
        let mx = window.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(data[0].0[bin], mn, "base min bin {bin}");
        assert_eq!(data[0].1[bin], mx, "base max bin {bin}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------- counting-allocator check

struct CountingAllocator;
thread_local! {
    static MEASURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
static ALLOCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if MEASURING.with(|m| m.get()) {
            ALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        if MEASURING.with(|m| m.get()) {
            ALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOC: CountingAllocator = CountingAllocator;

/// The capture→mixer graph (steady state) allocates nothing on the render
/// path (acceptance criterion — kimi finding 17).
#[test]
fn capture_to_mixer_render_path_does_not_allocate() {
    let dir = std::env::temp_dir().join(format!("p12-noalloc-{}", std::process::id()));
    let source = Arc::new(Spsc::new(1 << 18));
    let cap = Capture::start(&dir, "na", CHANNELS, SR, SR, source.clone()).unwrap();

    let mut e = engine();
    let mut capture_ids = Vec::new();
    for k in 0..CHANNELS {
        let id = e.graph.add_node(
            NodeKind::Opaque(Box::new(CaptureNode::new(cap.channel_ring(k)))),
            vec![Port {
                name: "audio",
                direction: Direction::Out,
                kind: SignalKind::Audio,
                channels: 1,
            }],
        );
        capture_ids.push(id);
    }
    e.mount("mixer", &[("channels", CHANNELS as f32)]).unwrap();
    e.render(BLOCK);
    let mixer = e.graph.out_node.unwrap();
    for (k, id) in capture_ids.iter().enumerate() {
        e.graph
            .connect(*id, "audio", mixer, &format!("ch{k}"))
            .unwrap();
    }

    let mut out = vec![0.0f32; 8192];
    e.render_into(&mut out); // prime
    ALLOCS.store(0, Ordering::Relaxed);
    MEASURING.with(|m| m.set(true));
    e.render_into(&mut out);
    MEASURING.with(|m| m.set(false));
    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "capture→mixer render must not allocate"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
