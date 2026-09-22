//! Drain/EOF (the implemented `drain-eof-phase` note): stateful nodes' buffered
//! tails are rendered out after the timeline instead of being cut at the frame
//! budget.
//!
//! What these prove:
//! - a sounding blip's tail is emitted under `DrainPolicy::Tails` and dropped
//!   under `HardCut`;
//! - a drain is bounded and reports `capped` instead of truncating silently;
//! - a drained render is byte-identical on replay (a pure function of state);
//! - a drained render does not continue a *source* (the graph-level gate).

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

/// euclidean → scale → tone → mixer.ch0 (the blip chain).
fn mount_blip_chain(e: &mut Engine, blip_len: f32) {
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 1.0),
            ("rotation", 0.0),
            ("pulses_per_beat", 4.0),
        ],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0), ("note_len", 2_400.0)])
        .unwrap();
    e.mount("tone", &[("gain", 0.9), ("blip_len", blip_len)])
        .unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
}

/// An engine whose blip chain has a **live** tail right now (advanced, bounded,
/// until the first blip is scheduled) — so a drain has something to emit.
fn engine_with_a_live_blip(blip_len: f32) -> Engine {
    let mut e = engine();
    mount_blip_chain(&mut e, blip_len);
    let mut rendered = 0u64;
    while !e.graph.has_tail() && rendered < 4 * SR as u64 {
        e.render(256);
        rendered += 256;
    }
    assert!(
        e.graph.has_tail(),
        "the blip chain must start a blip within 4 s"
    );
    e
}

#[test]
fn drain_emits_the_blip_tail_that_a_hard_cut_drops() {
    let mut e = engine_with_a_live_blip(20_000.0);
    let ch = e.graph.out_channels().max(1);

    let (out, outcome) = e.render_with_drain(1_000, DrainPolicy::Tails, MAX_DRAIN_FRAMES);
    assert!(
        outcome.tail_frames > 0,
        "a sounding blip must produce a tail"
    );
    assert!(!outcome.capped, "a 20k blip drains well inside the bound");
    assert_eq!(out.len(), (1_000 + outcome.tail_frames as usize) * ch);

    // The drained region carries audio: the blip's remainder, not silence.
    let drained = &out[1_000 * ch..];
    assert!(
        drained.iter().any(|s| s.abs() > 1e-4),
        "the tail must carry the blip out"
    );
}

#[test]
fn hard_cut_keeps_the_pre_drain_behaviour() {
    let mut e = engine_with_a_live_blip(20_000.0);
    let ch = e.graph.out_channels().max(1);

    let (out, outcome) = e.render_with_drain(1_000, DrainPolicy::HardCut, MAX_DRAIN_FRAMES);
    assert_eq!(
        outcome,
        DrainOutcome::default(),
        "a hard cut drains nothing"
    );
    assert_eq!(out.len(), 1_000 * ch, "the buffer is exactly the timeline");
}

#[test]
fn drain_is_bounded_and_reports_the_cap() {
    // A blip far longer than the drain bound: the cap must be reported, not
    // silently truncate.
    let mut e = engine_with_a_live_blip(1_000_000.0);
    let ch = e.graph.out_channels().max(1);

    let (out, outcome) = e.render_with_drain(100, DrainPolicy::Tails, 4_096);
    assert!(outcome.capped, "a tail beyond the bound must report capped");
    assert_eq!(
        outcome.tail_frames, 4_096,
        "the drain stops exactly at the bound"
    );
    assert_eq!(out.len(), (100 + 4_096) * ch);
}

#[test]
fn a_drained_render_is_deterministic() {
    // Drain is a pure function of graph state (no wall clock, no randomness):
    // two identical sessions must produce identical bytes, tail included. (This
    // proves drain purity; log replay of a drained session awaits media-command
    // logging — the drain is not yet a logged event.)
    let run = || {
        let mut e = engine_with_a_live_blip(20_000.0);
        e.render_with_drain(1_000, DrainPolicy::Tails, MAX_DRAIN_FRAMES)
            .0
    };
    assert_eq!(run(), run(), "a drained render must be deterministic");
}

#[test]
fn a_graph_with_no_tail_drains_nothing() {
    // No tail *and* no latency (the mixer is a plain processor): nothing is in
    // flight, so the drain emits nothing. (In-flight PDC samples are a separate
    // reason to drain — see `drain_flushes_in_flight_pdc_samples`.)
    let mut e = engine();
    e.mount("mixer", &[]).unwrap();
    e.render(256);
    assert!(!e.graph.has_tail());
    assert_eq!(e.graph.flush_frames(), 0, "the mixer introduces no latency");
    let (tail, outcome) = e.drain(MAX_DRAIN_FRAMES);
    assert!(tail.is_empty());
    assert_eq!(outcome, DrainOutcome::default());
}

/// A pass-through node that *reports* 3 samples of latency (so PDC delays the
/// fast path by 3) and holds no tail.
struct LatencyOnly;

impl AudioNode for LatencyOnly {
    fn latency(&self) -> u32 {
        3
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
        let n = out.len().min(io.audio_in.len());
        out[..n].copy_from_slice(&io.audio_in[..n]);
    }
}

#[test]
fn drain_flushes_in_flight_pdc_samples() {
    // A latency graph with **no tail**: the last samples are held in the PDC
    // delay line, so a drain must flush them. In-flight samples are not "audio
    // presence" — the tail gate alone would drop them.
    let mut e = engine();
    e.mount("mixer", &[]).unwrap();
    e.flush_scheduled(); // materialize the mixer (the bus owner)
    let mixer = e.graph.out_node.expect("the mixer claimed the bus");
    let lat = e
        .graph
        .insert_before(
            mixer,
            NodeKind::Opaque(Box::new(LatencyOnly)),
            vec![
                Port::audio("in", Direction::In),
                Port::audio("audio", Direction::Out),
            ],
        )
        .unwrap();
    let src = e
        .graph
        .insert_before(
            lat,
            NodeKind::Sine(Sine::new(440.0)),
            vec![Port::audio("audio", Direction::Out)],
        )
        .unwrap();
    e.graph.connect(src, "audio", lat, "in").unwrap();
    e.graph.connect(lat, "audio", mixer, "ch0").unwrap();

    e.render(64); // prime: the PDC delay line now holds in-flight samples
    assert!(!e.graph.has_tail(), "a sine holds no tail");
    assert!(e.graph.flush_frames() > 0, "the latency node owes a flush");

    let ch = e.graph.out_channels().max(1);
    let (tail, outcome) = e.drain(MAX_DRAIN_FRAMES);
    assert!(
        outcome.tail_frames > 0,
        "in-flight samples must not be dropped"
    );
    assert!(!outcome.capped);
    assert_eq!(tail.len(), outcome.tail_frames as usize * ch);
    assert!(
        tail.iter().any(|s| s.abs() > 1e-6),
        "the flushed in-flight sine samples must reach the master"
    );
}
