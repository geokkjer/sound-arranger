//! native-soft-synth spike — a fundsp-backed mono voice mounted as an opaque
//! [`AudioNode`], proving the integration and the engine's invariants:
//! - it sounds, and its pitch follows the scale (fundsp osc/filter are real);
//! - a note onset is sample-accurate (no sound before the trigger frame);
//! - rendering is byte-identical across replay (same log ⇒ same audio);
//! - `set_param` is logged + applied (and undeclared params are refused);
//! - the render path does not allocate (fundsp voice pre-allocated at mount);
//! - the node reports its latency for PDC (0 for a minimum-phase biquad).

#![cfg(feature = "fundsp")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine::*;
use engine::plugins::fundsp_synth::{FundspSynth, FUNDSP_PARAMS, FUNDSP_PORTS, fundsp_synth_factory};

const SR: u32 = 48_000;

fn engine() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory("euclidean", plugins::euclidean_factory, plugins::euclidean::EUCLIDEAN_PORTS, &[]);
    e.register_factory("scale", plugins::scale_factory, plugins::scale::SCALE_PORTS, &[]);
    e.register_factory("fundsp_synth", fundsp_synth_factory, FUNDSP_PORTS, FUNDSP_PARAMS);
    e.register_factory("mixer", plugins::mixer_factory, plugins::mixer::MIXER_PORTS, plugins::mixer::MIXER_PARAMS);
    e
}

/// euclidean triggers → scale → fundsp_synth → audio into the mixer's ch0.
fn mount_chain(e: &mut Engine) {
    e.mount(
        "euclidean",
        &[("steps", 8.0), ("pulses", 3.0), ("rotation", 0.0), ("pulses_per_beat", 4.0)],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0), ("note_len", 400.0)]).unwrap();
    e.mount("fundsp_synth", &[("cutoff", 8_000.0), ("q", 0.7), ("env_len", 20_000.0), ("gain", 0.3)])
        .unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger")).unwrap();
    e.patch(("scale", "note"), ("fundsp_synth", "note")).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("fundsp_synth", "audio"), ("mixer", "ch0")).unwrap();
}

/// The fundsp voice sounds: render the patched chain and require non-zero audio.
#[test]
fn fundsp_voice_sounds() {
    let mut e = engine();
    mount_chain(&mut e);
    let out = e.render(48_000);
    assert!(out.iter().any(|s| s.abs() > 1e-4), "the fundsp voice must produce audio");
    // It is a *pitched* voice — the decayed region actually oscillates.
    let mut crossings = 0usize;
    for w in out[8_000..20_000].windows(2) {
        if (w[0] > 0.0) != (w[1] > 0.0) {
            crossings += 1;
        }
    }
    assert!(crossings > 5, "expected an oscillatory decay, got {crossings} crossings");
}

/// Pitch flows through: an octave-up scale root ≈ twice the zero crossings.
#[test]
fn fundsp_pitch_follows_scale() {
    let crossings = |root: f32| -> usize {
        let mut e = engine();
        e.mount(
            "euclidean",
            &[("steps", 8.0), ("pulses", 1.0), ("rotation", 1.0), ("pulses_per_beat", 4.0)],
        )
        .unwrap();
        e.mount("scale", &[("root", root)]).unwrap();
        e.mount("fundsp_synth", &[("cutoff", 12_000.0), ("q", 0.7), ("env_len", 12_000.0), ("gain", 0.3)])
            .unwrap();
        e.patch(("euclidean", "triggers"), ("scale", "trigger")).unwrap();
        e.patch(("scale", "note"), ("fundsp_synth", "note")).unwrap();
        e.mount("mixer", &[]).unwrap();
        e.patch(("fundsp_synth", "audio"), ("mixer", "ch0")).unwrap();
        let out = e.render(48_000);
        let window = &out[6_300..9_500];
        let mut n = 0usize;
        for w in window.windows(2) {
            if (w[0] > 0.0) != (w[1] > 0.0) {
                n += 1;
            }
        }
        n
    };
    let low = crossings(0.0); // 440 Hz
    let high = crossings(12.0); // 880 Hz
    assert!(low > 0 && high > 0, "both must sound: {low} vs {high}");
    assert!(high > low * 3 / 2 && high < low * 2 + 4, "an octave ≈ twice the crossings: {low} vs {high}");
}

/// A single note onset is sample-accurate: silence before the trigger frame.
#[test]
fn fundsp_onset_is_sample_accurate() {
    let mut e = engine();
    e.mount(
        "euclidean",
        &[("steps", 8.0), ("pulses", 1.0), ("rotation", 1.0), ("pulses_per_beat", 4.0)],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0)]).unwrap();
    e.mount("fundsp_synth", &[("cutoff", 8_000.0), ("q", 0.7), ("env_len", 20_000.0), ("gain", 0.3)])
        .unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger")).unwrap();
    e.patch(("scale", "note"), ("fundsp_synth", "note")).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("fundsp_synth", "audio"), ("mixer", "ch0")).unwrap();
    let out = e.render(7_000);
    assert!(out[..6_000].iter().all(|s| *s == 0.0), "silence before the trigger frame");
    assert!(out[6_000..].iter().any(|s| *s != 0.0), "the voice must start at frame 6000");
}

/// Patching + mounting is logged; replay reproduces byte-identical audio.
#[test]
fn fundsp_replay_is_byte_identical() {
    let mut e1 = engine();
    mount_chain(&mut e1);
    let a = e1.render(2 * 48_000);
    let log = e1.log.clone();
    assert!(log.events().iter().any(|ev| matches!(ev, Event::Patch { .. })));

    let mut e2 = engine();
    e2.replay_from(&log).unwrap();
    let b = e2.render(2 * 48_000);
    assert_eq!(a, b, "same log must render byte-identical audio");
}

/// Replay reproduces a MID-SESSION `set_param` on the voice (the param path is
/// deterministic, incl. an in-flight envelope rescale).
#[test]
fn fundsp_replay_includes_set_param() {
    let mut e1 = engine();
    e1.mount(
        "euclidean",
        &[("steps", 8.0), ("pulses", 2.0), ("rotation", 0.0), ("pulses_per_beat", 4.0)],
    )
    .unwrap();
    e1.mount("scale", &[("root", 0.0), ("note_len", 400.0)]).unwrap();
    e1.mount("fundsp_synth", &[("cutoff", 8_000.0), ("q", 0.7), ("env_len", 20_000.0), ("gain", 0.3)])
        .unwrap();
    e1.patch(("euclidean", "triggers"), ("scale", "trigger")).unwrap();
    e1.patch(("scale", "note"), ("fundsp_synth", "note")).unwrap();
    e1.mount("mixer", &[]).unwrap();
    e1.patch(("fundsp_synth", "audio"), ("mixer", "ch0")).unwrap();

    let seg1 = e1.render(2 * 48_000);
    e1.set_param("fundsp_synth", "gain", 0.8).unwrap();
    let seg2 = e1.render(2 * 48_000);
    let log = e1.log.clone();

    let mut e2 = engine();
    e2.replay_from(&log).unwrap();
    let b = e2.render(4 * 48_000);
    assert_eq!(&b[..2 * 48_000], &seg1[..], "pre-set_param segment must match");
    assert_eq!(&b[2 * 48_000..], &seg2[..], "post-set_param segment must match");
}

/// `set_param` is logged, sample-accurate, and refused when undeclared.
#[test]
fn fundsp_set_param_is_logged_and_validated() {
    let mut e = engine();
    mount_chain(&mut e);
    let baseline = e.render(4_800);
    e.set_param("fundsp_synth", "gain", 0.9).unwrap();
    assert!(e.log.events().iter().any(|ev| matches!(ev, Event::SetParam { param: "gain", .. })));
    let loud = e.render(4_800);

    let peak = |b: &[f32]| b.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&loud) > peak(&baseline), "raising gain must raise the peak");

    // An undeclared param (cutoff is a construction param there) is refused.
    let err = e.set_param("fundsp_synth", "cutoff", 4_000.0).unwrap_err();
    assert!(err.contains("no parameter 'cutoff'"), "got: {err}");
}

// The render path must not allocate (a fundsp voice pre-allocated at mount).
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
fn fundsp_render_does_not_allocate() {
    let mut e = engine();
    mount_chain(&mut e);
    // Prime: the mounts + patch + mixer apply their allocations on the control
    // side. The measured region must be free.
    let _ = e.render(512);
    let mut out = vec![0.0f32; 8192]; // allocated before measuring

    ALLOCS.store(0, Ordering::Relaxed);
    MEASURING.with(|m| m.set(true));
    e.render_into(&mut out);
    MEASURING.with(|m| m.set(false));

    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "the fundsp voice must not allocate on the render path");
}

/// The node reports its latency for PDC: 0 for a minimum-phase fundsp path.
#[test]
fn fundsp_latency_is_reported_for_pdc() {
    let v = FundspSynth::new(8_000.0, 0.7, 20_000, 0.3, SR);
    assert_eq!(v.latency(), 0, "a minimum-phase fundsp path adds no whole-sample latency");
}


