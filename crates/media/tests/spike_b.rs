//! Spike B acceptance tests (media-engine note):
//! - a long file streams from disk through the engine graph glitch-free and
//!   sample-identical (no underruns, EOF → silence);
//! - splicing a clip *during playback* lands sample-accurately with an
//!   equal-power crossfade of exactly the requested length;
//! - recording through the graph round-trips the master to WAV; a take that
//!   dies mid-write is recovered by `WavWriter::recover`;
//! - device-clock drift keeps the recorded timeline in session frames at the
//!   right pitch, with bounded buffering;
//! - the same media command sequence on fresh engines bounces byte-identically
//!   (media-level determinism);
//! - the media render path (steady state) allocates nothing;
//! - the 25-minute soak (play + record + splice + bounce) stays glitch-free;
//! - the cpal device path opens and runs on real hardware.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use engine::*;
use media::*;

const SR: u32 = 48_000;

// ---------------------------------------------------------------- helpers

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("spike-b-{name}-{}.wav", std::process::id()))
}

fn write_tone(path: &Path, freq: f32, frames: u64) {
    let mut w = WavWriter::create(path, SR, 1).unwrap();
    let mut buf = vec![0.0f32; 4096];
    let mut phase = 0.0f64;
    let mut left = frames;
    while left > 0 {
        let n = buf.len().min(left as usize);
        for s in &mut buf[..n] {
            *s = (TAU * phase).sin() as f32 * 0.5;
            phase += freq as f64 / SR as f64;
        }
        w.write(&buf[..n]).unwrap();
        left -= n as u64;
    }
    w.finalize().unwrap();
}

fn read_all(path: &Path) -> Vec<f32> {
    let mut r = WavReader::open(path).unwrap();
    let mut out = vec![0.0f32; r.total_frames() as usize];
    assert_eq!(r.read_into(&mut out), out.len(), "read the whole file");
    out
}

fn read_at(path: &Path, frame: u64, n: usize) -> Vec<f32> {
    let mut r = WavReader::open(path).unwrap();
    r.seek_frames(frame).unwrap();
    let mut out = vec![0.0f32; n];
    r.read_into(&mut out);
    out
}

/// An engine with a playback node mounted as the master out; the mailbox and
/// the shared underrun counter are kept so tests can drive and assert.
struct Rig {
    e: Engine,
    player_id: NodeId,
    mbox: Mailbox,
    underruns: Arc<AtomicU64>,
    deferred: Arc<AtomicU64>,
    recorder: Option<Arc<WavRecorder>>,
}

fn rig_with_player(clip: ClipRef) -> Rig {
    let mut e = Engine::new(SR, 120.0, 4);
    let mbox = mailbox();
    let player = FilePlayer::start(clip, DEFAULT_RING_CAPACITY).unwrap();
    warm_player(&player); // deterministic: fill the ring before rendering starts
    let node = PlaybackNode::new(Some(player), mbox.clone());
    let underruns = node.underrun_counter();
    let deferred = node.deferred_counter();
    let player_id = e.graph.add_node(
        NodeKind::Opaque(Box::new(node)),
        vec![Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio, channels: 1 }],
    );
    e.graph.set_out(player_id);
    Rig { e, player_id, mbox, underruns, deferred, recorder: None }
}

/// Wait until the player's ring is full (or the clip is done) instead of a
/// fixed sleep — the render tests assert zero underruns, so the warm-up must
/// be deterministic, not timing-lucky.
fn warm_player(player: &FilePlayer) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while player.produced() < DEFAULT_RING_CAPACITY as u64 && !player.eof() {
        assert!(std::time::Instant::now() < deadline, "reader did not warm up in 10 s");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Mount a recorder fed by the player's audio (the master bus in miniature —
/// the Phase 1 mixer owns the real one).
fn add_recorder(rig: &mut Rig, path: &Path) -> Arc<WavRecorder> {
    let rec = Arc::new(WavRecorder::start(path, SR).unwrap());
    let id = rig.e.graph.add_node(
        NodeKind::Opaque(Box::new(RecordNode::new(rec.clone()))),
        vec![Port { name: "audio", direction: Direction::In, kind: SignalKind::Audio, channels: 1 }],
    );
    rig.e.graph.connect(rig.player_id, "audio", id, "audio").unwrap();
    rig.recorder = Some(rec.clone());
    rec
}

fn schedule_splice(rig: &Rig, at_frame: u64, clip: ClipRef, crossfade: u32) {
    let incoming = FilePlayer::start(clip, DEFAULT_RING_CAPACITY).unwrap();
    warm_player(&incoming); // the fade window must be fed the moment it opens
    rig.mbox.lock().unwrap().push_back(SpliceCmd { at_frame, incoming, crossfade });
}

/// Render `frames` in blocks, sleeping `per_block` between blocks. The offline
/// render loop is a *fast-forward* consumer (hundreds of × real time); the
/// reader/writer threads sustain a fixed rate, so an unpaced test would starve
/// them the way real playback never does (the device callback consumes at
/// exactly real-time speed). Pacing keeps the consumer below the sustained
/// rate — the honest operating regime — while staying fast (0.5 ms per
/// 512-frame block ≈ 21× real time).
fn render_paced(e: &mut Engine, frames: usize, per_block: Duration) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    let mut block = [0.0f32; engine::BLOCK];
    for chunk in out.chunks_mut(engine::BLOCK) {
        let n = chunk.len();
        e.render_into(&mut block[..n]);
        chunk.copy_from_slice(&block[..n]);
        std::thread::sleep(per_block);
    }
    out
}

// ------------------------------------------------------------------ tests

/// A 60-second file streams from disk through the engine graph: every sample
/// arrives exactly once, nothing underruns, and the tail is silence.
#[test]
fn playback_streams_a_long_file_glitch_free() {
    let path = tmp("stream");
    let frames = SR as u64 * 60;
    write_tone(&path, 440.0, frames);

    let mut rig = rig_with_player(ClipRef::whole(&path).unwrap());
    let out = render_paced(&mut rig.e, frames as usize + engine::BLOCK, Duration::from_millis(1));

    assert_eq!(rig.underruns.load(Ordering::Relaxed), 0, "no underruns on a warm cached file");
    let expected = read_all(&path);
    for (a, b) in expected.iter().zip(&out[..frames as usize]) {
        assert_eq!(*a, *b, "streamed content must equal the file sample-for-sample");
    }
    assert!(out[frames as usize..].iter().all(|s| *s == 0.0), "tail must be silence");
    let _ = std::fs::remove_file(&path);
}

/// Splicing a clip in mid-playback is sample-accurate: pure A before the
/// frame, an equal-power crossfade of exactly the requested length, pure B
/// after it — with no underruns anywhere.
#[test]
fn splice_during_playback_is_sample_accurate() {
    let a = tmp("splice-a");
    let b = tmp("splice-b");
    let frames = SR as u64 * 2;
    write_tone(&a, 440.0, frames);
    write_tone(&b, 880.0, frames);

    let f = 60_000u64; // splice at 1.25 s
    let xf = 512u32;
    let mut rig = rig_with_player(ClipRef::whole(&a).unwrap());
    schedule_splice(&rig, f, ClipRef::whole(&b).unwrap(), xf);

    let total = (f + xf as u64 + 10_000) as usize;
    let out = render_paced(&mut rig.e, total, Duration::from_millis(1));
    assert_eq!(rig.underruns.load(Ordering::Relaxed), 0, "no underruns across the splice");
    assert_eq!(rig.deferred.load(Ordering::Relaxed), 0, "the splice applies at its exact frame");

    let fa = read_all(&a);
    let fb = read_all(&b);
    // pre-splice is pure A, sample-exact
    for (x, s) in fa[..f as usize].iter().zip(&out[..f as usize]) {
        assert_eq!(*x, *s, "pre-splice must equal clip A");
    }
    // the fade window: both sources must be audible (moved away from pure A
    // and already bringing B in)
    let win = &out[f as usize..(f + xf as u64) as usize];
    let diff_a: f32 = win
        .iter()
        .zip(&fa[f as usize..f as usize + xf as usize])
        .map(|(x, a)| (x - a).abs())
        .sum();
    let diff_b: f32 = win.iter().zip(&fb[..xf as usize]).map(|(x, b)| (x - b).abs()).sum();
    assert!(diff_a > 1.0, "the fade must move away from pure A: {diff_a}");
    assert!(diff_b > 1.0, "the fade must bring B in: {diff_b}");
    // post-fade is pure B, aligned to the splice frame, sample-exact
    for (x, s) in fb[xf as usize..].iter().zip(&out[(f + xf as u64) as usize..]) {
        assert_eq!(*x, *s, "post-fade must equal clip B");
    }

    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
}

/// The same media command sequence on fresh engines renders identically and
/// bounces byte-identically (media-level determinism).
#[test]
fn bounce_is_byte_identical_on_replay() {
    let a = tmp("bounce-a");
    let b = tmp("bounce-b");
    let frames = SR as u64 * 2;
    write_tone(&a, 440.0, frames);
    write_tone(&b, 220.0, frames / 2); // 1 s — plays out before the session ends
    let f = 48_000u64; // splice at 1 s
    let total = SR as usize * 3; // 3 s session
    let rec1 = tmp("bounce-rec1");
    let rec2 = tmp("bounce-rec2");

    let run = |rec_path: &Path| -> Vec<f32> {
        let mut rig = rig_with_player(ClipRef::whole(&a).unwrap());
        add_recorder(&mut rig, rec_path);
        schedule_splice(&rig, f, ClipRef::whole(&b).unwrap(), 512);
        let out = render_paced(&mut rig.e, total, Duration::from_millis(1));
        rig.recorder.as_ref().unwrap().stop().unwrap();
        assert_eq!(rig.underruns.load(Ordering::Relaxed), 0);
        out
    };

    let out1 = run(&rec1);
    let out2 = run(&rec2);
    assert_eq!(out1, out2, "the same media script must render identically");
    let bytes1 = std::fs::read(&rec1).unwrap();
    let bytes2 = std::fs::read(&rec2).unwrap();
    assert_eq!(bytes1, bytes2, "the same media script must bounce byte-identically");

    for p in [&a, &b, &rec1, &rec2] {
        let _ = std::fs::remove_file(p);
    }
}

/// Recording through the graph: the take must be a faithful (16-bit) copy of
/// the rendered master, with no overruns.
#[test]
fn record_roundtrip_matches_the_master() {
    let src = tmp("rec-src");
    let rec_path = tmp("rec-take");
    let frames = SR as u64 * 30;
    write_tone(&src, 330.0, frames);

    let mut rig = rig_with_player(ClipRef::whole(&src).unwrap());
    add_recorder(&mut rig, &rec_path);
    let out = render_paced(&mut rig.e, frames as usize, Duration::from_millis(1));
    rig.recorder.as_ref().unwrap().stop().unwrap();

    assert_eq!(rig.underruns.load(Ordering::Relaxed), 0);
    assert_eq!(rig.recorder.as_ref().unwrap().overruns(), 0);
    let recorded = read_all(&rec_path);
    assert_eq!(recorded.len(), frames as usize, "the take holds every session frame");
    for (a, b) in out.iter().zip(&recorded) {
        assert!((a - b).abs() < 2e-4, "master {a} vs take {b}");
    }

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&rec_path);
}

/// A take that dies mid-write (header never patched) is recovered to its full
/// length by `WavWriter::recover`.
#[test]
fn crashed_take_is_recovered() {
    let src = tmp("crash-src");
    let rec_path = tmp("crash-take");
    let frames = SR as u64 * 20;
    write_tone(&src, 440.0, frames);

    let mut rig = rig_with_player(ClipRef::whole(&src).unwrap());
    let rec = add_recorder(&mut rig, &rec_path);
    let out = render_paced(&mut rig.e, frames as usize, Duration::from_millis(1));
    // simulate a crash: leak the recorder so neither stop() nor Drop patches
    // the header (the writer thread stays alive but idle — the take is as the
    // disk saw it)
    std::mem::forget(rec);
    assert_eq!(rig.underruns.load(Ordering::Relaxed), 0);

    // wait for the writer to finish draining (it is a separate thread)
    let expected_bytes = 44 + frames as usize * 2;
    for _ in 0..500 {
        if std::fs::metadata(&rec_path).map(|m| m.len() as usize).unwrap_or(0) >= expected_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let recovered = WavWriter::recover(&rec_path).unwrap();
    assert_eq!(recovered, frames, "recovery must find every written frame");
    let recorded = read_all(&rec_path);
    assert_eq!(recorded.len(), frames as usize);
    for (a, b) in out.iter().zip(&recorded) {
        assert!((a - b).abs() < 2e-4, "master {a} vs recovered {b}");
    }

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&rec_path);
}

/// A virtual input device at 48001 Hz recorded through the drift compensator:
/// the timeline is in session frames (bounded buffering) and the pitch is
/// preserved (zero-crossing check).
#[test]
fn drift_keeps_the_recorded_timeline_in_session_frames() {
    struct DriftSource {
        rate: u32,
        phase: f64,
        acc: f64,
        fed: u64,
    }
    impl DriftSource {
        fn new(rate: u32) -> Self {
            DriftSource { rate, phase: 0.0, acc: 0.0, fed: 0 }
        }
        /// Feed the compensator the frames the device delivered for one
        /// 512-frame output block.
        fn feed(&mut self, c: &mut DriftCompensator) {
            self.acc += self.rate as f64 / SR as f64 * 512.0;
            let want = self.acc.floor() as u64 - self.fed;
            if want > 0 {
                let block: Vec<f32> = (0..want)
                    .map(|_| {
                        let s = (TAU * self.phase).sin() as f32 * 0.5;
                        self.phase += 440.0 / self.rate as f64;
                        s
                    })
                    .collect();
                c.push_input(&block);
                self.fed += want;
            }
        }
    }

    let rec_path = tmp("drift-take");
    let rec: Arc<WavRecorder> = Arc::new(WavRecorder::start(&rec_path, SR).unwrap());
    let mut c = DriftCompensator::new(48_001, SR);
    let mut dev = DriftSource::new(48_001);

    // ceil(session_frames / 512) blocks: the take spans the whole 10 s window,
    // including its fractional final block.
    let seconds = 10u32;
    let session_frames = SR as usize * seconds as usize;
    let blocks = session_frames.div_ceil(512);
    let mut out = vec![0.0f32; 512];
    for _ in 0..blocks {
        dev.feed(&mut c);
        let n = c.pull_output(&mut out);
        for s in &out[..n] {
            rec.push(*s);
        }
        std::thread::sleep(Duration::from_micros(500)); // pace the fast-forward consumer
    }
    rec.stop().unwrap();
    assert_eq!(rec.overruns(), 0, "the recorder must not drop drift-fed frames");
    drop(rec);

    let recorded = read_all(&rec_path);
    assert!(
        recorded.len() >= session_frames - 2,
        "the take must hold ~10 s of session frames, got {}",
        recorded.len()
    );
    let crossings = recorded
        .windows(2)
        .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
        .count();
    let expected = 440.0 * seconds as f32 * 2.0;
    assert!(
        (crossings as f32 - expected).abs() < expected * 0.01,
        "pitch drifted: {crossings} crossings vs ~{expected}"
    );
    let _ = std::fs::remove_file(&rec_path);
}

// ------------------------------------------------- counting-allocator check

struct CountingAllocator;
thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if MEASURING.with(|m| m.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if MEASURING.with(|m| m.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOC: CountingAllocator = CountingAllocator;

/// The media render path (steady state — no commands firing) allocates
/// nothing: nodes only pop/push SPSC rings. Thread spawns, file opens, and
/// command application happened on the control side.
#[test]
fn render_path_does_not_allocate_with_media_nodes() {
    let src = tmp("noalloc-src");
    let rec_path = tmp("noalloc-rec");
    write_tone(&src, 440.0, SR as u64);

    let mut rig = rig_with_player(ClipRef::whole(&src).unwrap());
    add_recorder(&mut rig, &rec_path);
    // a splice that completes *during* the measured render: command issue
    // (thread spawn, warm ring) is control-side; application (fade mix, player
    // retire → detach) must not allocate or block on the render path.
    schedule_splice(&rig, 0, ClipRef::whole(&src).unwrap(), 512);

    let mut out = vec![0.0f32; 8192];
    rig.e.render_into(&mut out); // prime: mounts and warm-up happen here
    ALLOCS.store(0, Ordering::Relaxed);
    MEASURING.with(|m| m.set(true));
    rig.e.render_into(&mut out);
    MEASURING.with(|m| m.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "the media render path must not allocate");

    rig.recorder.as_ref().unwrap().stop().unwrap();
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&rec_path);
}

// ------------------------------------------------------------- soak (long)

/// The Spike B headline: 25 minutes of a long file streamed from disk while
/// recording the master, a splice at minute 15, glitch-free to the end.
/// Ignored for routine runs (writes ~350 MB to /tmp); run in release with:
/// `cargo test -p media --release -- --ignored soak`
#[test]
#[ignore = "25-minute soak; run in release: cargo test -p media --release -- --ignored soak"]
fn soak_25_minutes_stream_record_splice_bounce() {
    let a = tmp("soak-a");
    let b = tmp("soak-b");
    let rec_path = tmp("soak-rec");
    let sr = SR as u64;
    let total = sr * 25 * 60; // 25 min
    let splice_at = sr * 15 * 60; // minute 15
    write_tone(&a, 440.0, total);
    write_tone(&b, 220.0, sr * 11 * 60); // enough for the remaining 10 min

    let mut rig = rig_with_player(ClipRef::whole(&a).unwrap());
    let rec = add_recorder(&mut rig, &rec_path);
    schedule_splice(&rig, splice_at, ClipRef::whole(&b).unwrap(), 512);
    std::thread::sleep(Duration::from_millis(200));

    let _out = render_paced(&mut rig.e, total as usize, Duration::from_micros(200));
    rig.recorder.as_ref().unwrap().stop().unwrap();

    assert_eq!(rig.underruns.load(Ordering::Relaxed), 0, "no underruns over 25 minutes");
    assert_eq!(rec.overruns(), 0, "no recorder overruns over 25 minutes");
    let recorded = read_all(&rec_path);
    assert_eq!(recorded.len(), total as usize, "the take holds every session frame");

    // spot checks: pre-splice is A, post-splice (after the fade) is B, and
    // the take is still audio at the very end.
    let pre = read_at(&a, splice_at - 1, 1);
    let post = read_at(&b, 512, 1);
    assert_eq!(recorded[splice_at as usize - 1], pre[0], "pre-splice must be clip A");
    assert_eq!(recorded[splice_at as usize + 512], post[0], "post-fade must be clip B");
    let tail_max = recorded[total as usize - SR as usize..]
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    assert!(tail_max > 0.1, "audio still playing at the end (max {tail_max})");

    for p in [&a, &b, &rec_path] {
        let _ = std::fs::remove_file(p);
    }
}

// ------------------------------------------------------------ devices (hw)

/// The cpal device path on real hardware: open the default output, play a
/// 1 s tone (the device must drain it), and open the default input for a
/// moment. Skips politely when no device exists. Run with:
/// `cargo test -p media -- --ignored devices`
#[test]
#[ignore = "needs real audio hardware; run: cargo test -p media -- --ignored devices"]
fn devices_open_and_run() {
    let out_name = media::devices::default_output_name();
    let in_name = media::devices::default_input_name();
    if out_name.is_none() && in_name.is_none() {
        eprintln!("SKIP: no default audio device on this machine");
        return;
    }
    eprintln!(
        "devices: out={out_name:?} in={in_name:?}\n  all: {:?}",
        media::devices::list_devices()
    );

    if let Some(name) = out_name {
        let ring = Arc::new(Spsc::new(1 << 16));
        let handle = media::devices::open_output(ring.clone(), 1, 48_000).expect("open output");
        assert!(
            !handle.rate_mismatch,
            "the test device should provide 48 kHz (got {})",
            handle.sample_rate
        );
        let n = handle.sample_rate as usize;
        let mut phase = 0.0f64;
        for _ in 0..n {
            let s = (TAU * phase).sin() as f32 * 0.2;
            phase += 440.0 / handle.sample_rate as f64;
            while !ring.try_push(s) {
                std::thread::sleep(Duration::from_micros(10));
            }
        }
        std::thread::sleep(Duration::from_millis(1200));
        let remaining = ring.len();
        assert!(remaining < n / 2, "{name}: device consumed the tone (left {remaining} of {n})");
        drop(handle);
    }

    if let Some(name) = in_name {
        let ring = Arc::new(Spsc::new(1 << 16));
        let handle = media::devices::open_input(ring.clone()).expect("open input");
        std::thread::sleep(Duration::from_millis(500));
        eprintln!("{name}: captured {} samples in 0.5 s", ring.len());
        drop(handle);
    }
}
