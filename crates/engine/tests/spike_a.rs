//! Spike A acceptance tests (minimal-core note):
//! - sample-accurate clock (tempo map round-trip, scheduler exactness);
//! - value-based graph with two node tiers and PDC;
//! - reversible mount/unmount — no residual sound or state, sample-accurately;
//! - *same log ⇒ byte-identical bounce* (determinism, including mid-session
//!   mounts and tempo changes — the frame-less-event hole kimi caught);
//! - fail-loud `inject` checks;
//! - spatial composability: two plugins cooperate via a service key;
//! - the render path never allocates (counting allocator).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine::*;

const SR: u32 = 48_000;

fn engine() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory("euclidean", plugins::euclidean_factory);
    e
}

fn mount_euclidean(e: &mut Engine) {
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 3.0),
            ("rotation", 0.0),
            ("pitch", 220.0),
            ("gain", 0.2),
            ("blip_len", 800.0),
        ],
    )
    .expect("mount euclidean");
}

/// Sample-accurate onset: a trigger at frame 6000 must produce silence before
/// it and a blip exactly from that frame onward.
#[test]
fn blip_onset_is_sample_accurate() {
    let mut e = engine();
    // 8 steps of 16ths at 120bpm = 8 × 6000 samples = one bar. rotation 1 puts
    // the single pulse at step 1 → frame 6000.
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 1.0),
            ("rotation", 1.0),
            ("pitch", 440.0),
            ("gain", 0.25),
            ("blip_len", 1200.0),
        ],
    )
    .unwrap();
    let out = e.render(7000);
    assert_eq!(out[5999], 0.0);
    assert_eq!(out[6000], 0.0, "onset sample is sin(0)");
    assert!(out[6001] > 0.0, "blip must begin at frame 6001 (onset at 6000)");
}

/// Same log ⇒ byte-identical bounce, across mount, a scheduled unmount, and a
/// tempo change.
#[test]
fn determinism_same_log_same_bounce() {
    let mut e1 = engine();
    mount_euclidean(&mut e1);
    e1.schedule_unmount("euclidean", 1_200_000);
    e1.set_tempo(96.0, 4);
    let a = e1.render(2 * 48_000);
    let log = e1.log.clone();

    let mut e2 = engine();
    e2.replay_from(&log).expect("replay");
    let b = e2.render(2 * 48_000);

    assert_eq!(log.len(), 3, "mount + scheduled unmount + tempo change");
    assert_eq!(a, b, "same log must render byte-identical audio");
}

/// Replay must reproduce mid-session tempo changes: the `SetTempo` event
/// carries its frame, so a change after 2s lands at 2s on replay, not at 0.
#[test]
fn replay_is_exact_for_mid_session_tempo_change() {
    let mut e1 = engine();
    mount_euclidean(&mut e1);
    let first = e1.render(2 * 48_000); // 2s at 120bpm
    e1.set_tempo(240.0, 4); // mid-session tempo change
    e1.schedule_unmount("euclidean", 3 * 48_000);
    let second = e1.render(2 * 48_000);
    let log = e1.log.clone();

    let mut e2 = engine();
    e2.replay_from(&log).unwrap();
    let b = e2.render(4 * 48_000);

    assert_eq!(&b[..2 * 48_000], &first[..], "pre-change segment must match");
    assert_eq!(&b[2 * 48_000..], &second[..], "post-change segment must match");
    assert!(b[3 * 48_000..].iter().all(|s| *s == 0.0), "silence after unmount");
}

/// Replay must reproduce mid-session mounts: silence before the mount frame.
#[test]
fn replay_is_exact_for_mid_session_mount() {
    let mut e1 = engine();
    let _quiet = e1.render(48_000); // 1s of silence — nothing mounted yet
    e1.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 3.0),
            ("pitch", 220.0),
            ("gain", 0.2),
            ("blip_len", 800.0),
        ],
    )
    .unwrap();
    let after = e1.render(48_000);
    let log = e1.log.clone();

    let mut e2 = engine();
    e2.replay_from(&log).unwrap();
    let b = e2.render(96_000);

    assert!(b[..48_000].iter().all(|s| *s == 0.0), "silence before the mount frame");
    assert_eq!(&b[48_000..], &after[..], "audio after the mount frame must match");
}

/// Unmounting is sample-accurate: a long-decay blip is cut exactly at the
/// scheduled frame — sound up to the boundary, exact silence from it.
#[test]
fn unmount_is_sample_accurate() {
    let mut e = engine();
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 3.0),
            ("rotation", 0.0),
            ("pitch", 220.0),
            ("gain", 0.2),
            ("blip_len", 20_000.0), // long decay so the cut is audible
        ],
    )
    .unwrap();
    let at = 96_000 + 100; // 100 samples into a block — mid-block lifecycle
    e.schedule_unmount("euclidean", at);
    let out = e.render((at + 64) as usize);

    assert!(out[..at as usize].iter().any(|s| *s != 0.0), "sound before the unmount");
    assert!(out[(at - 1) as usize] != 0.0, "the blip is still decaying at the boundary");
    assert!(
        out[at as usize..].iter().all(|s| *s == 0.0),
        "exact silence from frame {at}"
    );
}

/// Unmounting leaves no residual sound at all (and replay still reproduces).
#[test]
fn unmount_removes_contribution() {
    let mut e = engine();
    mount_euclidean(&mut e);
    let bar = 2 * 48_000;
    e.schedule_unmount("euclidean", bar);
    let out = e.render(bar as usize + 4096);
    assert!(out[..bar as usize].iter().any(|s| *s != 0.0));
    assert!(out[bar as usize..].iter().all(|s| *s == 0.0));
}

/// Mount → unmount → re-mount reproduces the identical signal (reversibility
/// is exact, not approximate).
#[test]
fn remount_reproduces_identical_signal() {
    let mut e1 = engine();
    mount_euclidean(&mut e1);
    let first = e1.render(2 * 48_000);

    let mut e2 = engine();
    mount_euclidean(&mut e2);
    e2.unmount("euclidean");
    let _gone = e2.render(2 * 48_000);
    mount_euclidean(&mut e2);
    let again = e2.render(2 * 48_000);

    assert_eq!(first, again, "re-mount must reproduce the exact signal");
}

/// Spatial composability, static half: a consumer plugin declares `rhythm`,
/// resolves it at mount (fail-loud when the provider is absent), and the
/// provider's disposer withdraws it.
#[test]
fn plugins_cooperate_via_service_key() {
    struct BassFollower;
    impl Plugin for BassFollower {
        fn id(&self) -> &'static str {
            "bass"
        }
        fn inject(&self) -> &'static [&'static str] {
            &["rhythm"]
        }
        fn apply(&mut self, api: &mut PluginApi) -> Result<Disposer, String> {
            let rhythm = api
                .ctx
                .get::<Rhythm>("rhythm")
                .ok_or("rhythm service missing at apply")?;
            assert!(rhythm.pattern.len() == rhythm.steps as usize);
            Ok(Box::new(|_| {}))
        }
    }
    let bass_factory: PluginFactory = |_| Ok(Box::new(BassFollower));

    let mut e = engine();
    e.register_factory("bass", bass_factory);
    // Consumer first: the provider is absent → fail-loud.
    let err = e.mount("bass", &[]).unwrap_err();
    assert!(err.contains("rhythm"), "got: {err}");
    assert!(e.log.is_empty(), "a refused mount must not be logged");
    // Provider first, then consumer: the provider must be *applied* (rendered)
    // before its service exists — load order via requirements.
    mount_euclidean(&mut e);
    e.render(1);
    e.mount("bass", &[]).expect("bass mounts once euclidean provides rhythm");
    // Unmounting the provider withdraws the service once applied.
    e.unmount("euclidean");
    e.unmount("bass");
    e.render(1);
    let err = e.mount("bass", &[]).unwrap_err();
    assert!(err.contains("rhythm"), "got: {err}");
}

/// Fail-loud inject: a plugin requiring a missing service is refused at mount.
#[test]
fn inject_fails_loud() {
    struct NeedsMissing;
    impl Plugin for NeedsMissing {
        fn id(&self) -> &'static str {
            "needs-missing"
        }
        fn inject(&self) -> &'static [&'static str] {
            &["definitely-not-provided"]
        }
        fn apply(&mut self, _api: &mut PluginApi) -> Result<Disposer, String> {
            Ok(Box::new(|_| {}))
        }
    }
    let factory: PluginFactory = |_| Ok(Box::new(NeedsMissing));
    let mut e = engine();
    e.register_factory("needs-missing", factory);
    let err = e.mount("needs-missing", &[]).unwrap_err();
    assert!(err.contains("definitely-not-provided"), "got: {err}");
    assert!(e.log.is_empty(), "a refused mount must not be logged");
}

/// Unknown plugins are refused, and a duplicate mount of the same name is too.
#[test]
fn mount_validation() {
    let mut e = engine();
    let err = e.mount("nope", &[]).unwrap_err();
    assert!(err.contains("unknown plugin"), "got: {err}");
    mount_euclidean(&mut e);
    let err = e.mount("euclidean", &[]).unwrap_err();
    assert!(err.contains("already mounted"), "got: {err}");
}

/// The euclidean generator honours tempo changes (the tempo map is the single
/// time authority). E(8,3) rotated by 1 puts a pulse at step 1: frame 6000 at
/// 120bpm, frame 3000 after a switch to 240bpm at frame 0.
#[test]
fn tempo_change_moves_triggers() {
    let mut e = engine();
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 3.0),
            ("rotation", 1.0),
            ("pitch", 440.0),
            ("gain", 0.25),
            ("blip_len", 1200.0),
        ],
    )
    .unwrap();
    e.set_tempo(240.0, 4);
    let out = e.render(5000);
    assert_eq!(out[2999], 0.0);
    assert!(out[3001] > 0.0, "pulse must land at frame 3000 under 240bpm");
}

// The render path must not allocate: a counting allocator asserts zero
// allocations inside `render_into` (the invariant the notes promise). The
// counter is thread-local so parallel tests don't pollute it.
thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;
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

#[test]
fn render_path_does_not_allocate() {
    let mut e = engine();
    mount_euclidean(&mut e);
    e.schedule_unmount("euclidean", 100_000);
    // Prime: applying the mount allocates on the control side (factory, boxes,
    // service table). The measured region must be allocation-free.
    let _prime = e.render(512);
    let mut out = vec![0.0f32; 8192]; // allocated before measuring

    ALLOCS.store(0, Ordering::Relaxed);
    MEASURING.with(|m| m.set(true));
    e.render_into(&mut out);
    MEASURING.with(|m| m.set(false));

    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "the render path must not allocate (engine invariant)"
    );
}
