//! Phase 1 acceptance tests (foundation + soft mixer — phase-1 note):
//! - `Engine::set_param` is logged, sample-accurate, replayable, fail-loud;
//! - the mixer owns the master bus (the tone no longer claims the output);
//! - per-channel gain/mute/solo and the master fader are verified numerically;
//! - meters report the expected peaks (render-path writes, no blocking);
//! - the mixer render path allocates nothing (counting allocator).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use engine::*;

const SR: u32 = 48_000;

fn engine() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory("euclidean", plugins::euclidean_factory, plugins::euclidean::EUCLIDEAN_PORTS, &[]);
    e.register_factory("scale", plugins::scale_factory, plugins::scale::SCALE_PORTS, &[]);
    e.register_factory("tone", plugins::tone_factory, plugins::tone::TONE_PORTS, plugins::tone::TONE_PARAMS);
    e.register_factory("mixer", plugins::mixer_factory, plugins::mixer::MIXER_PORTS, plugins::mixer::MIXER_PARAMS);
    e
}

/// The Phase-1 canonical chain: euclidean triggers → scale → tone → mixer.ch0.
/// Pure setup — the mounts/patches are scheduled at frame 0 and applied by the
/// test's own first render.
fn chain(e: &mut Engine) {
    e.mount(
        "euclidean",
        &[("steps", 8.0), ("pulses", 1.0), ("rotation", 1.0), ("pulses_per_beat", 4.0)],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0)]).unwrap();
    e.mount("tone", &[("gain", 0.25), ("blip_len", 1200.0)]).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger")).unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
}

/// A node-level rig: two sines into mixer channels 0 and 1, the mixer as the
/// master out, and the shared meter bank. (Node-level — the logged/plugin path
/// is exercised by the `set_param` and bus tests below.)
fn node_rig() -> (Graph, NodeId, NodeId, NodeId, Arc<MeterBank>) {
    let mut g = Graph::new();
    let s0 = g.add_node(
        NodeKind::Sine(Sine::new(440.0)),
        vec![Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio }],
    );
    let s1 = g.add_node(
        NodeKind::Sine(Sine::new(880.0)),
        vec![Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio }],
    );
    let mixer_node = MixerNode::new();
    let meters = mixer_node.meters();
    let mixer = g.add_node(NodeKind::Opaque(Box::new(mixer_node)), MIXER_PORTS.to_vec());
    g.set_out(mixer);
    g.connect(s0, "audio", mixer, "ch0").unwrap();
    g.connect(s1, "audio", mixer, "ch1").unwrap();
    (g, s0, s1, mixer, meters)
}

fn sine_sample(freq: f32, phase: &mut f64) -> f32 {
    let s = (TAU * *phase).sin() as f32;
    *phase += freq as f64 / SR as f64;
    s
}

fn render_node(g: &mut Graph, frames: usize) -> Vec<f32> {
    let tempo = TempoMap::new(SR, 120.0, 4);
    let mut out = vec![0.0f32; frames];
    let mut pos = 0u64;
    for chunk in out.chunks_mut(BLOCK) {
        g.render(chunk, RenderBlock { frame: pos, sample_rate: SR, tempo: &tempo });
        pos += chunk.len() as u64;
    }
    out
}

// ---------------------------------------------------------- set_param path

/// A logged gain change mid-session is sample-accurate and replayable
/// byte-identically.
#[test]
fn set_param_is_logged_and_replayable() {
    let run = |e: &mut Engine| -> Vec<f32> {
        chain(e);
        let a = e.render(7000); // blip begins at 6000 (gain 0.25)
        e.set_param("mixer", "ch0.gain", 0.0).unwrap(); // mute ch0 at 7000
        let b = e.render(3000);
        let mut all = a;
        all.extend(b);
        all
    };

    let mut e1 = engine();
    let out1 = run(&mut e1);

    assert!(out1[6001..7000].iter().any(|s| *s != 0.0), "blip before the gain change");
    assert!(out1[7000..].iter().all(|s| *s == 0.0), "gain 0 mutes from frame 7000");

    // Replay the log on a fresh engine: byte-identical.
    let mut e2 = engine();
    e2.replay_from(&e1.log).unwrap();
    let out2 = e2.render(10_000);
    assert_eq!(out1, out2, "set_param must be replayable byte-identically");
}

/// A gain change at an exact frame applies sample-accurately (block-split).
#[test]
fn set_param_is_sample_accurate() {
    let mut e = engine();
    chain(&mut e);
    let a = e.render(6500); // mid-blip
    e.set_param("mixer", "ch0.gain", 0.0).unwrap();
    let b = e.render(3000);
    let mut out = a;
    out.extend(b);

    assert!(out[6499] != 0.0, "blip still sounding before the frame");
    assert_eq!(out[6500], 0.0, "gain 0 applies exactly at frame 6500");
    assert!(out[6500..].iter().all(|s| *s == 0.0));
}

/// set_param is fail-loud: unknown plugin, registered-but-not-mounted plugin,
/// undeclared parameter name, non-finite value, and out-of-range values are
/// all refused — and a refusal is never logged.
#[test]
fn set_param_refused_for_unmounted_plugin() {
    let mut e = engine();
    let err = e.set_param("ghost", "gain", 0.0).unwrap_err();
    assert!(err.contains("unknown plugin"), "got: {err}");

    let err = e.set_param("mixer", "ch0.gain", 0.5).unwrap_err();
    assert!(err.contains("neither scheduled nor mounted"), "got: {err}");

    chain(&mut e);
    let err = e.set_param("mixer", "ch0.gian", 0.5).unwrap_err(); // typo
    assert!(err.contains("no parameter"), "got: {err}");
    let err = e.set_param("mixer", "ch0.gain", f32::NAN).unwrap_err();
    assert!(err.contains("finite"), "got: {err}");
    let err = e.set_param("mixer", "ch0.gain", 99.0).unwrap_err();
    assert!(err.contains("out of range"), "got: {err}");

    assert!(
        e.log.events().iter().all(|ev| !matches!(ev, Event::SetParam { .. })),
        "a refused set_param must never be logged"
    );
}

/// set_param accepts a plugin whose mount is scheduled (applies at the same
/// frame, before the parameter — consistent with `patch`'s
/// scheduled-or-mounted rule).
#[test]
fn set_param_accepts_scheduled_mount() {
    let mut e = engine();
    e.mount("mixer", &[]).unwrap(); // scheduled, not yet applied
    e.set_param("mixer", "master.gain", 0.5).unwrap();
    let out = e.render(512);
    assert_eq!(out.len(), 512);
    assert!(e.log.events().iter().any(|ev| matches!(ev, Event::SetParam { .. })));
}

// ----------------------------------------------------------- mixer behaviour

/// Two sines into ch0/ch1 with per-channel gains produce the weighted sum.
#[test]
fn mixer_routes_channels_with_gain() {
    let (mut g, _s0, _s1, mixer, _meters) = node_rig();
    g.set_param(mixer, "ch0.gain", 0.5);
    g.set_param(mixer, "ch1.gain", 1.0);
    g.set_param(mixer, "master.gain", 1.0);
    let out = render_node(&mut g, BLOCK);

    let (mut p0, mut p1) = (0.0f64, 0.0f64);
    for (i, s) in out.iter().enumerate() {
        let expected = sine_sample(440.0, &mut p0) * 0.5 + sine_sample(880.0, &mut p1) * 1.0;
        assert!((*s - expected).abs() < 1e-6, "sample {i}: {s} vs {expected}");
    }
}

/// Mute silences its channel; solo makes only solo'd channels sound.
#[test]
fn mixer_mute_and_solo() {
    let (mut g, _s0, _s1, mixer, _meters) = node_rig();
    g.set_param(mixer, "ch0.mute", 1.0);
    let out = render_node(&mut g, BLOCK);
    let mut p1 = 0.0f64;
    for s in out.iter() {
        let expected = sine_sample(880.0, &mut p1);
        assert!((*s - expected).abs() < 1e-6, "muted ch0 must be silent: {s} vs {expected}");
    }

    // Solo ch1: ch0 (unmuted, louder) must be excluded.
    let (mut g, _s0, _s1, mixer, _meters) = node_rig();
    g.set_param(mixer, "ch0.mute", 0.0);
    g.set_param(mixer, "ch1.solo", 1.0);
    let out = render_node(&mut g, BLOCK);
    let mut p1 = 0.0f64;
    for s in out.iter() {
        let expected = sine_sample(880.0, &mut p1);
        assert!((*s - expected).abs() < 1e-6, "solo must exclude ch0: {s} vs {expected}");
    }
}

/// The master fader scales the summed mix.
#[test]
fn mixer_master_fader() {
    let (mut g, _s0, _s1, mixer, _meters) = node_rig();
    g.set_param(mixer, "master.gain", 0.5);
    let out = render_node(&mut g, BLOCK);
    let mut p0 = 0.0f64;
    let mut p1 = 0.0f64;
    for s in out.iter() {
        let expected = (sine_sample(440.0, &mut p0) + sine_sample(880.0, &mut p1)) * 0.5;
        assert!((*s - expected).abs() < 1e-6, "fader 0.5: {s} vs {expected}");
    }
}

/// Meters report the post-fader channel peaks and the master peak, exactly as
/// computed during render.
#[test]
fn mixer_meters() {
    let (mut g, _s0, _s1, mixer, meters) = node_rig();
    g.set_param(mixer, "ch0.gain", 0.5);
    let out = render_node(&mut g, BLOCK);
    let max_out = out.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

    let mut p0 = 0.0f64;
    let ch0_peak: f32 = (0..BLOCK).map(|_i| sine_sample(440.0, &mut p0).abs() * 0.5).fold(0.0, f32::max);
    assert!((meters.channel_peak(0) - ch0_peak).abs() < 1e-6, "ch0 meter {:.6} vs {ch0_peak:.6}", meters.channel_peak(0));
    assert!((meters.master_peak() - max_out).abs() < 1e-6, "master meter {:.6} vs {max_out:.6}", meters.master_peak());
}

/// The mixer owns the master bus: unmounting it silences the master even
/// though the tone is still mounted and sounding.
#[test]
fn mixer_owns_the_master_bus() {
    let mut e = engine();
    chain(&mut e);
    let out = e.render(7000);
    assert!(out[6001..7000].iter().any(|s| *s != 0.0), "blip routes through the mixer");

    e.unmount("mixer");
    let out = e.render(2000);
    assert!(
        out.iter().all(|s| *s == 0.0),
        "without the mixer there is no master (the tone no longer claims the out)"
    );
}

// -------------------------------------------------------- no-alloc render

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

/// The mixer's render path (steady state, including the per-channel fan-in)
/// allocates nothing.
#[test]
fn render_path_does_not_allocate_with_mixer() {
    let mut e = engine();
    chain(&mut e);
    let mut out = vec![0.0f32; 8192];
    e.render_into(&mut out); // prime: mounts/patches apply here
    ALLOCS.store(0, Ordering::Relaxed);
    MEASURING.with(|m| m.set(true));
    e.render_into(&mut out);
    MEASURING.with(|m| m.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "the mixer render path must not allocate");
}

/// Meters are post-gain/pre-mute: a muted channel still shows its level
/// (kimi finding 10), and mute is absolute — it wins over solo.
#[test]
fn mixer_meter_points_and_mute_wins_over_solo() {
    // muted channel still meters, and the mix contains only ch1
    let (mut g, _s0, _s1, mixer, meters) = node_rig();
    g.set_param(mixer, "ch0.mute", 1.0);
    let out = render_node(&mut g, BLOCK);
    let mut p0 = 0.0f64;
    let ch0_level: f32 = (0..BLOCK).map(|_i| sine_sample(440.0, &mut p0).abs()).fold(0.0, f32::max);
    assert!(
        (meters.channel_peak(0) - ch0_level).abs() < 1e-6,
        "muted ch0 must still meter (pre-mute): {:.6} vs {ch0_level:.6}",
        meters.channel_peak(0)
    );
    let mut p1 = 0.0f64;
    for s in out.iter() {
        let expected = sine_sample(880.0, &mut p1);
        assert!((*s - expected).abs() < 1e-6, "only ch1 sounds: {s} vs {expected}");
    }

    // mute wins over solo: mute the only solo'd channel → the mix is silent
    let (mut g, _s0, _s1, mixer, _meters) = node_rig();
    g.set_param(mixer, "ch1.solo", 1.0);
    g.set_param(mixer, "ch1.mute", 1.0);
    let out = render_node(&mut g, BLOCK);
    assert!(out.iter().all(|s| *s == 0.0), "mute is absolute — wins over solo");
}

/// Unmounting a *middle* node rewires surviving cords' indices coherently
/// (kimi finding 6): after removing `scale`, the tone stops sounding and the
/// mixer master goes silent.
#[test]
fn remove_middle_node_rewires_cords() {
    let mut e = engine();
    chain(&mut e);
    let out = e.render(7000);
    assert!(out[6001..7000].iter().any(|s| *s != 0.0), "chain sounds");

    e.unmount("scale");
    let out = e.render(2000); // 7000..9000: the first blip's tail rings to 7200
    assert!(
        out[200..].iter().all(|s| *s == 0.0),
        "after the tail (7200) no triggers → no notes → silence through the mixer"
    );
}

/// The mixer's meters are reachable through the real plugin path (kimi
/// finding 1): mounting the mixer provides `mixer.meters` on the context.
#[test]
fn mixer_meters_are_a_context_service() {
    let mut e = engine();
    e.mount("mixer", &[]).unwrap();
    e.render(512);
    let meters = e
        .ctx
        .get::<Arc<MeterBank>>("mixer.meters")
        .expect("the mixer provides its meters under 'mixer.meters'");
    assert_eq!(meters.master_peak(), 0.0, "no input → master meters 0");
    e.unmount("mixer");
    e.render(512); // the scheduled unmount applies (disposer runs)
    assert!(e.ctx.get::<Arc<MeterBank>>("mixer.meters").is_none(), "the disposer withdraws the service");
}

/// The graph refuses more than MAX_AUDIO_INS audio inputs loudly (kimi
/// finding 2).
#[test]
#[should_panic(expected = "max")]
fn more_than_max_audio_ins_refused() {
    let mut g = Graph::new();
    let mut ports: Vec<Port> = (0..engine::MAX_AUDIO_INS + 1)
        .map(|i| Port {
            name: Box::leak(format!("ch{i}").into_boxed_str()),
            direction: Direction::In,
            kind: SignalKind::Audio,
        })
        .collect();
    ports.push(Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio });
    let _ = g.add_node(NodeKind::Opaque(Box::new(MixerNode::new())), ports);
}
