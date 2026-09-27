//! Phase 1 acceptance tests (foundation + soft mixer — phase-1 note):
//! - `Engine::set_param` is logged, sample-accurate, replayable, fail-loud;
//! - the mixer owns the master bus (the tone no longer claims the output);
//! - per-channel gain/mute/solo and the master fader are verified numerically;
//! - meters report the expected peaks (render-path writes, no blocking);
//! - the mixer render path allocates nothing (counting allocator).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine::*;

const SR: u32 = 48_000;

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

/// The Phase-1 canonical chain: euclidean triggers → scale → tone → mixer.ch0.
/// Pure setup — the mounts/patches are scheduled at frame 0 and applied by the
/// test's own first render.
fn chain(e: &mut Engine) {
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
    e.mount("mixer", &[]).unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap();
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
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
    );
    let s1 = g.add_node(
        NodeKind::Sine(Sine::new(880.0)),
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
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

/// The equal-power center-pan gain each channel sees at `pan = 0` (√2/2).
const CENTER: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// De-interleave a stereo master to its L channel (frames), the mono reference
/// these channel-behaviour tests were written against.
fn l(buf: &[f32]) -> Vec<f32> {
    buf.as_chunks::<2>().0.iter().map(|p| p[0]).collect()
}

/// Render `frames` of the mixer (the stereo master) and return the L channel
/// as a frame-aligned mono slice — so the existing per-frame assertions stay
/// meaningful (a center-panned mono source appears on both channels, scaled by
/// [`CENTER`]). A mono master (`ch == 1`) returns the buffer unchanged.
fn render_node(g: &mut Graph, frames: usize) -> Vec<f32> {
    let tempo = TempoMap::new(SR, 120.0, 4);
    let ch = g.out_channels().max(1);
    let mut out = vec![0.0f32; frames * ch];
    let mut pos = 0u64;
    for chunk in out.chunks_mut(BLOCK * ch) {
        g.render(
            chunk,
            RenderBlock {
                frame: pos,
                sample_rate: SR,
                tempo: &tempo,
                mode: RenderMode::Timeline,
            },
        );
        pos += (chunk.len() / ch) as u64;
    }
    if ch >= 2 { l(&out) } else { out }
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
        l(&all) // the master is now stereo; compare the L channel
    };

    let mut e1 = engine();
    let out1 = run(&mut e1);

    assert!(
        out1[6001..7000].iter().any(|s| *s != 0.0),
        "blip before the gain change"
    );
    assert!(
        out1[7000..].iter().all(|s| *s == 0.0),
        "gain 0 mutes from frame 7000"
    );

    // Replay the log on a fresh engine: byte-identical (on the L channel).
    let mut e2 = engine();
    e2.replay_from(&e1.log).unwrap();
    let out2 = l(&e2.render(10_000));
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
    let out = l(&out); // master is stereo; compare the L channel

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
        e.log
            .events()
            .iter()
            .all(|ev| !matches!(ev, Event::SetParam { .. })),
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
    assert_eq!(out.len(), 512 * 2, "the stereo master renders L+R");
    assert!(
        e.log
            .events()
            .iter()
            .any(|ev| matches!(ev, Event::SetParam { .. }))
    );
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
        let expected =
            (sine_sample(440.0, &mut p0) * 0.5 + sine_sample(880.0, &mut p1) * 1.0) * CENTER;
        assert!(
            (*s - expected).abs() < 1e-6,
            "sample {i}: {s} vs {expected}"
        );
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
        let expected = sine_sample(880.0, &mut p1) * CENTER;
        assert!(
            (*s - expected).abs() < 1e-6,
            "muted ch0 must be silent: {s} vs {expected}"
        );
    }

    // Solo ch1: ch0 (unmuted, louder) must be excluded.
    let (mut g, _s0, _s1, mixer, _meters) = node_rig();
    g.set_param(mixer, "ch0.mute", 0.0);
    g.set_param(mixer, "ch1.solo", 1.0);
    let out = render_node(&mut g, BLOCK);
    let mut p1 = 0.0f64;
    for s in out.iter() {
        let expected = sine_sample(880.0, &mut p1) * CENTER;
        assert!(
            (*s - expected).abs() < 1e-6,
            "solo must exclude ch0: {s} vs {expected}"
        );
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
        let expected = (sine_sample(440.0, &mut p0) + sine_sample(880.0, &mut p1)) * 0.5 * CENTER;
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
    let ch0_peak: f32 = (0..BLOCK)
        .map(|_i| sine_sample(440.0, &mut p0).abs() * 0.5)
        .fold(0.0, f32::max);
    assert!(
        (meters.channel_peak(0) - ch0_peak).abs() < 1e-6,
        "ch0 meter {:.6} vs {ch0_peak:.6}",
        meters.channel_peak(0)
    );
    assert!(
        (meters.master_peak() - max_out).abs() < 1e-6,
        "master meter {:.6} vs {max_out:.6}",
        meters.master_peak()
    );
}

/// The mixer owns the master bus: unmounting it silences the master even
/// though the tone is still mounted and sounding.
#[test]
fn mixer_owns_the_master_bus() {
    let mut e = engine();
    chain(&mut e);
    let out = l(&e.render(7000));
    assert!(
        out[6001..7000].iter().any(|s| *s != 0.0),
        "blip routes through the mixer"
    );

    e.unmount("mixer").unwrap();
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
    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "the mixer render path must not allocate"
    );
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
    let ch0_level: f32 = (0..BLOCK)
        .map(|_i| sine_sample(440.0, &mut p0).abs())
        .fold(0.0, f32::max);
    assert!(
        (meters.channel_peak(0) - ch0_level).abs() < 1e-6,
        "muted ch0 must still meter (pre-mute): {:.6} vs {ch0_level:.6}",
        meters.channel_peak(0)
    );
    let mut p1 = 0.0f64;
    for s in out.iter() {
        let expected = sine_sample(880.0, &mut p1) * CENTER;
        assert!(
            (*s - expected).abs() < 1e-6,
            "only ch1 sounds: {s} vs {expected}"
        );
    }

    // mute wins over solo: mute the only solo'd channel → the mix is silent
    let (mut g, _s0, _s1, mixer, _meters) = node_rig();
    g.set_param(mixer, "ch1.solo", 1.0);
    g.set_param(mixer, "ch1.mute", 1.0);
    let out = render_node(&mut g, BLOCK);
    assert!(
        out.iter().all(|s| *s == 0.0),
        "mute is absolute — wins over solo"
    );
}

/// Unmounting a *middle* node rewires surviving cords' indices coherently
/// (kimi finding 6): after removing `scale`, the tone stops sounding and the
/// mixer master goes silent.
#[test]
fn remove_middle_node_rewires_cords() {
    let mut e = engine();
    chain(&mut e);
    let out = l(&e.render(7000));
    assert!(out[6001..7000].iter().any(|s| *s != 0.0), "chain sounds");

    e.unmount("scale").unwrap();
    let out = l(&e.render(2000)); // 7000..9000: the first blip's tail rings to 7200
    assert!(
        out[200..].iter().all(|s| *s == 0.0),
        "after the tail (7200) no triggers → no notes → silence through the mixer"
    );
}

/// Patches beyond the mounted channel count are accepted but ignored
/// (documented): with `channels = 2`, a source into ch3 is silent.
#[test]
fn mixer_ignores_channels_beyond_the_mounted_count() {
    let mut g = Graph::new();
    let s0 = g.add_node(
        NodeKind::Sine(Sine::new(440.0)),
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
    );
    let s3 = g.add_node(
        NodeKind::Sine(Sine::new(880.0)),
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
    );
    let mixer_node = MixerNode::with_channels(2, Arc::new(MeterBank::new(2)));
    let mixer = g.add_node(NodeKind::Opaque(Box::new(mixer_node)), MIXER_PORTS.to_vec());
    g.set_out(mixer);
    g.connect(s0, "audio", mixer, "ch0").unwrap();
    g.connect(s3, "audio", mixer, "ch3").unwrap(); // beyond channels=2: ignored
    let out = render_node(&mut g, BLOCK);
    let mut p0 = 0.0f64;
    for s in out.iter() {
        let expected = sine_sample(440.0, &mut p0) * CENTER;
        assert!(
            (*s - expected).abs() < 1e-6,
            "ch3 must be ignored: {s} vs {expected}"
        );
    }
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
    e.unmount("mixer").unwrap();
    e.render(512); // the scheduled unmount applies (disposer runs)
    assert!(
        e.ctx.get::<Arc<MeterBank>>("mixer.meters").is_none(),
        "the disposer withdraws the service"
    );
}

/// A probe: records how many audio inputs the render handed it.
struct WidthProbe(Arc<AtomicUsize>);

impl AudioNode for WidthProbe {
    fn latency(&self) -> u32 {
        0
    }

    fn render(
        &mut self,
        io: &NodeIO,
        out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _block: RenderBlock,
    ) {
        self.0.store(io.audio_in_count, Ordering::Relaxed);
        out.fill(0.0);
    }
}

/// A node's input width is **its own**. The graph used to refuse more than
/// `MAX_AUDIO_INS` (8) audio inputs, and that constant was the array dimension in
/// the node-facing `NodeIO` — a structural ceiling, not a validation bound. Now the
/// width is whatever the node declared and the view is sized from it.
#[test]
fn a_node_may_declare_more_inputs_than_the_old_ceiling() {
    const WIDE: usize = 12;
    let tempo = TempoMap::new(SR, 120.0, 4);
    let mut g = Graph::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let mut ports: Vec<Port> = (0..WIDE)
        .map(|i| Port {
            name: Box::leak(format!("ch{i}").into_boxed_str()),
            direction: Direction::In,
            kind: SignalKind::Audio,
            channels: 1,
        })
        .collect();
    ports.push(Port {
        name: "audio",
        direction: Direction::Out,
        kind: SignalKind::Audio,
        channels: 1,
    });
    g.add_node(
        NodeKind::Opaque(Box::new(WidthProbe(Arc::clone(&seen)))),
        ports,
    );
    let mut out = vec![0.0f32; BLOCK];
    g.render(
        &mut out,
        RenderBlock {
            frame: 0,
            sample_rate: SR,
            tempo: &tempo,
            mode: RenderMode::Timeline,
        },
    );
    assert_eq!(
        seen.load(Ordering::Relaxed),
        WIDE,
        "every declared audio input reaches the node"
    );
}

/// A patch to a channel the mixer did **not** mount is refused: the mounted surface is
/// what validation reads, so the old "accepted but ignored" path is gone.
#[test]
fn a_patch_to_an_unmounted_channel_is_refused() {
    let mut e = engine();
    e.mount("tone", &[]).unwrap();
    e.mount("mixer", &[("channels", 2.0)]).unwrap();
    // The mounts apply during a render, and the mounted surface is what patches read.
    let _ = e.render(64);
    e.patch(("tone", "audio"), ("mixer", "ch1"))
        .expect("a mounted channel accepts the patch");
    let refused = e.patch(("tone", "audio"), ("mixer", "ch2"));
    let err = refused.expect_err("an unmounted channel is refused");
    assert!(err.contains("no port 'ch2'"), "got: {err}");
}

/// The ceiling is gone on **both** rungs of surface resolution: a queued mount's factory
/// answers for the width it will have, and once applied the instance answers for the
/// width it has. Twenty channels used to be impossible.
#[test]
fn a_wider_mixer_offers_its_extra_channels_before_and_after_it_applies() {
    let mut e = engine();
    e.mount("tone", &[]).unwrap();
    e.mount("mixer", &[("channels", 20.0)]).unwrap();
    // Queued: no instance yet, so the surface comes from the factory with these params.
    e.patch(("tone", "audio"), ("mixer", "ch19"))
        .expect("a queued mount answers for its own width");
    let _ = e.render(64);
    // Applied: the instance's mounted surface answers, and it is just as wide.
    e.patch(("tone", "audio"), ("mixer", "ch18"))
        .expect("the applied instance answers for its width too");
}

/// …and the audio actually reaches the wide channel: twenty sources into twenty mounted
/// channels, with the last one metered.
#[test]
fn a_mixer_wider_than_the_old_ceiling_carries_every_channel() {
    const WIDE: usize = 20;
    let mut g = Graph::new();
    let bank = Arc::new(MeterBank::new(WIDE));
    let plugin = mixer_factory(&[("channels", WIDE as f32)]).expect("a wide mixer mounts");
    let ports = plugin.mounted_ports();
    // Sources first: a cord must run forward in node order.
    let sources: Vec<NodeId> = (0..WIDE)
        .map(|i| {
            g.add_node(
                NodeKind::Sine(Sine::new(220.0 + i as f32)),
                vec![Port {
                    name: "audio",
                    direction: Direction::Out,
                    kind: SignalKind::Audio,
                    channels: 1,
                }],
            )
        })
        .collect();
    let mixer = g.add_node(
        NodeKind::Opaque(Box::new(MixerNode::with_channels(WIDE, Arc::clone(&bank)))),
        ports,
    );
    g.set_out(mixer);
    for (i, src) in sources.into_iter().enumerate() {
        g.connect(src, "audio", mixer, &format!("ch{i}"))
            .expect("every mounted channel takes its source");
    }
    let _ = render_node(&mut g, BLOCK);
    assert!(
        bank.channel_peak(WIDE - 1) > 0.0,
        "the twentieth channel carried audio"
    );
}

/// The mixer adapts to the input layout (P1.2): mounted with `channels = 2`, only the
/// first two inputs are processed. Defense in depth — a mounted node declares only its
/// own channels, so this is the node's own guard rather than a reachable product path.
#[test]
fn mixer_adapts_to_channel_count() {
    let mut g = Graph::new();
    let s0 = g.add_node(
        NodeKind::Sine(Sine::new(440.0)),
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
    );
    let s1 = g.add_node(
        NodeKind::Sine(Sine::new(660.0)),
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
    );
    let s2 = g.add_node(
        NodeKind::Sine(Sine::new(880.0)),
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
    );
    let mixer_node = MixerNode::with_channels(2, Arc::new(MeterBank::new(2)));
    let mixer = g.add_node(NodeKind::Opaque(Box::new(mixer_node)), MIXER_PORTS.to_vec());
    g.set_out(mixer);
    g.connect(s0, "audio", mixer, "ch0").unwrap();
    g.connect(s1, "audio", mixer, "ch1").unwrap();
    g.connect(s2, "audio", mixer, "ch2").unwrap(); // accepted, ignored
    let out = render_node(&mut g, BLOCK);

    let (mut p0, mut p1) = (0.0f64, 0.0f64);
    for s in out.iter() {
        let expected = (sine_sample(440.0, &mut p0) + sine_sample(660.0, &mut p1)) * CENTER;
        assert!(
            (*s - expected).abs() < 1e-6,
            "ch2 must be ignored: {s} vs {expected}"
        );
    }
}

/// The GLM-5.3 review catch (#1): a logged mixer mount at a mid-session frame,
/// replayed and rendered in ONE call, must advance the clock by exactly the
/// requested frames. A mid-call master-width change used to split the frame
/// count (replay clock 1536 vs live 2048), silently breaking byte-identical
/// replay. Master-width changes are now parked at render-call boundaries.
#[test]
fn replay_across_mixer_mount_in_one_call_advances_the_clock_exactly() {
    let mut e1 = engine();
    let _ = e1.render(1024); // mono silence
    e1.mount("mixer", &[]).unwrap(); // scheduled @1024
    let _ = e1.render(1024); // stereo silence
    assert_eq!(e1.clock.frame(), 2048, "live renders exactly 2048 frames");

    let mut e2 = engine();
    e2.replay_from(&e1.log).unwrap();
    let out = e2.render(2048); // ONE call spanning the mount
    assert_eq!(
        e2.clock.frame(),
        2048,
        "replay in one call must advance exactly the requested frames (got {})",
        e2.clock.frame()
    );
    assert_eq!(
        out.len(),
        2048,
        "the master is mono for the whole call (the width change is parked to the next boundary)"
    );
}

/// The GLM-5.3 review test-gap: pan is asserted only at center. Verify the
/// equal-power law at hard left (-1 → L only), hard right (+1 → R only), and
/// that a channel's pan is clamped to [-1, 1].
#[test]
fn pan_law_is_equal_power_and_hard_left_right() {
    let tempo = TempoMap::new(SR, 120.0, 4);
    let render = |g: &mut Graph| -> Vec<f32> {
        let mut out = vec![0.0f32; BLOCK * 2];
        g.render(
            &mut out,
            RenderBlock {
                frame: 0,
                sample_rate: SR,
                tempo: &tempo,
                mode: RenderMode::Timeline,
            },
        );
        out
    };
    let frame_of = |samples: &[f32], i: usize| (samples[2 * i], samples[2 * i + 1]);

    // hard left: only L carries the mono channel. Zero ch1 so it doesn't
    // contribute to R (node_rig routes two sines into ch0/ch1).
    let (mut g, _s0, _s1, mixer, _m) = node_rig();
    g.set_param(mixer, "ch1.gain", 0.0);
    g.set_param(mixer, "ch0.pan", -1.0);
    let out = render(&mut g);
    let (l, r) = frame_of(&out, 100);
    assert!(
        l.abs() > 1e-3,
        "hard-left L must carry the signal (got {l})"
    );
    assert!(r.abs() < 1e-6, "hard-left R must be silent (got {r})");

    // hard right: only R.
    g.set_param(mixer, "ch0.pan", 1.0);
    let out = render(&mut g);
    let (l, r) = frame_of(&out, 100);
    assert!(l.abs() < 1e-6, "hard-right L must be silent (got {l})");
    assert!(
        r.abs() > 1e-3,
        "hard-right R must carry the signal (got {r})"
    );

    // clamp: an out-of-range pan is clamped to the [-1, 1] law, not the raw value.
    g.set_param(mixer, "ch0.pan", 3.0);
    let out = render(&mut g);
    let (l, r) = frame_of(&out, 100);
    assert!(
        l.abs() < 1e-6 && r.abs() > 1e-3,
        "pan 3.0 clamps to hard right"
    );
}
