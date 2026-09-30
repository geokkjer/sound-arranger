use super::*;
use crate::clock::TempoMap;
use crate::graph::{Graph, RenderMode};

fn block(_frames: usize) -> RenderBlock<'static> {
    // A leaked tempo map keeps the test's borrow simple; tests are short-lived.
    let tempo: &'static TempoMap = Box::leak(Box::new(TempoMap::new(48_000, 120.0, 4)));
    RenderBlock {
        frame: 0,
        sample_rate: 48_000,
        tempo,
        mode: RenderMode::Timeline,
    }
}

/// Run `frames` of a mono signal through a master node and return the stereo
/// output, one block at a time (so the ballistics see a realistic block size).
fn run(node: &mut MasterNode, input: &[f32], frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; input.len() * 2];
    let mut done = 0;
    while done < input.len() {
        let n = frames.min(input.len() - done);
        let mut interleaved = vec![0.0f32; n * 2];
        for i in 0..n {
            interleaved[i * 2] = input[done + i];
            interleaved[i * 2 + 1] = input[done + i];
        }
        let io = NodeIO {
            audio_in: &interleaved,
            audio_ins: crate::AudioInputs::none(),
            audio_in_count: 1,
            audio_out_channels: 2,
            frames: n,
            control_in: 0.0,
            triggers_in: &[],
            notes_in: &[],
        };
        let mut block_out = vec![0.0f32; n * 2];
        let mut control = 0.0f32;
        let mut triggers = EventBuf::new();
        let mut notes = EventBuf::new();
        node.render(
            &io,
            &mut block_out,
            &mut control,
            &mut triggers,
            &mut notes,
            block(n),
        );
        out[done * 2..(done + n) * 2].copy_from_slice(&block_out);
        done += n;
    }
    out
}

fn peak(out: &[f32]) -> f32 {
    out.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

/// The chain is **transparent** when asked to be: ratio 1, ceiling above the
/// material, no makeup → the output is the input (delayed).
#[test]
fn a_transparent_setting_passes_the_signal() {
    let mut node = MasterNode::new(48_000);
    node.set_param("ratio", 1.0);
    node.set_param("threshold", 0.0);
    node.set_param("makeup", 0.0);
    node.set_param("ceiling", 0.0);
    let input: Vec<f32> = (0..2_048).map(|i| 0.25 * (i as f32 * 0.05).sin()).collect();
    let out = run(&mut node, &input, 256);
    let delay = node.lookahead();
    // The output lags the input by the lookahead, so compare where both exist.
    for i in 1_000..(input.len() - delay) {
        let y = out[(i + delay) * 2];
        assert!(
            (y - input[i]).abs() < 1e-5,
            "sample {i}: {y} vs {}",
            input[i]
        );
    }
}

/// The limiter is a **brickwall**: a signal far above the ceiling comes out at
/// or below it, with no overshoot on the transient.
#[test]
fn the_limiter_never_crosses_the_ceiling() {
    let mut node = MasterNode::new(48_000);
    node.set_param("ratio", 1.0);
    node.set_param("ceiling", -6.0);
    let ceiling = db_to_lin(-6.0);
    // A quiet run into a full-scale burst: the gain must already be down when
    // the burst starts (that is what the lookahead buys).
    let mut input = vec![0.02f32; 4_800];
    input.extend(std::iter::repeat_n(1.0f32, 4_800));
    let out = run(&mut node, &input, 256);
    let worst = peak(&out);
    assert!(
        worst <= ceiling + 1e-4,
        "the brickwall holds: peak {worst} vs ceiling {ceiling}"
    );
    // …and the burst is still loud (not ducked to nothing).
    assert!(worst > ceiling * 0.9, "the burst survives: {worst}");
}

/// The compressor pulls peaks down and releases afterward — and the makeup
/// gain is part of the compressor, not a second fader. The meter is "the last
/// rendered block", so it is read while the loud section is playing (an offline
/// render that ends in quiet reports 0 dB, correctly).
#[test]
fn the_compressor_reduces_and_recovers() {
    let mut node = MasterNode::new(48_000);
    node.set_param("threshold", -20.0);
    node.set_param("ratio", 8.0);
    node.set_param("attack_ms", 1.0);
    node.set_param("ceiling", 0.0);
    let delay = node.lookahead();

    // The loud section: reduced, and the reduction is metered.
    let loud = vec![0.5f32; 9_600];
    let out = run(&mut node, &loud, 256);
    let loud_out = out[(delay + 8_000) * 2].abs();
    assert!(
        loud_out / 0.5 < 0.4,
        "the loud section is compressed by >8 dB: {} of 0.5",
        loud_out
    );
    let reduction = node.meters().reduction_db();
    assert!(
        reduction > 6.0,
        "gain reduction is metered while it is happening: {reduction} dB"
    );

    // The quiet section: the release brings the level back. Measured 9 000 frames
    // in (a 120 ms release from ~13 dB of reduction is still rising), so the
    // assertion is "substantially recovered", not "exactly the input".
    let quiet = vec![0.02f32; 9_600];
    let out = run(&mut node, &quiet, 256);
    let quiet_out = out[(delay + 9_000) * 2].abs();
    assert!(
        quiet_out / 0.02 > 0.8,
        "and the quiet section has recovered: {quiet_out} of 0.02"
    );
    // Still releasing (a 120 ms release from ~13 dB takes a while to fall under
    // 1 dB), so the assertion is "most of the way back", not "at zero".
    let released = node.meters().reduction_db();
    assert!(
        released < reduction * 0.4,
        "the meter is most of the way back: {released} dB vs {reduction} dB during the loud part"
    );
}

/// The lookahead is **declared latency**, so PDC can align it.
#[test]
fn the_lookahead_is_declared_as_latency() {
    let node = MasterNode::new(48_000);
    assert_eq!(
        node.latency(),
        node.lookahead() as u32,
        "the declared latency is the limiter's delay"
    );
    assert!(
        (node.lookahead() as f32 - 240.0).abs() <= 1.0,
        "5 ms at 48 kHz is 240 frames, got {}",
        node.lookahead()
    );
    assert!(node.latency() as usize <= crate::graph::MAX_PDC);
}

/// **Garbage on the bus does not spread.** A NaN passes through as silence; an
/// `+inf` (which would otherwise make the required gain 0 and the *next* block's
/// `inf * 0` a NaN) is silenced at the input, and the node recovers on the
/// following clean samples.
#[test]
fn non_finite_input_is_silenced_not_propagated() {
    let mut node = MasterNode::new(48_000);
    node.set_param("ratio", 1.0);
    let mut input = vec![0.25f32; 4_096];
    input[100] = f32::NAN;
    input[101] = f32::INFINITY;
    input[102] = f32::NEG_INFINITY;
    let out = run(&mut node, &input, 256);
    assert!(out.iter().all(|x| x.is_finite()), "the output stays finite");
    let delay = node.lookahead();
    for i in 1_500..(input.len() - delay) {
        assert!(
            (out[(i + delay) * 2] - 0.25).abs() < 1e-5,
            "and the clean signal is intact at {i}: {}",
            out[(i + delay) * 2]
        );
    }
}

/// The sliding maximum really is a maximum over the window (the property the
/// brickwall rests on), and it is held over *peaks* so a ceiling change applies
/// to buffered frames immediately.
#[test]
fn the_lookahead_window_is_a_sliding_maximum() {
    let mut w = MaxWindow::new(4);
    for (i, v) in [0.2f32, 0.9, 0.4, 0.3].iter().enumerate() {
        w.push(i as u64, *v);
    }
    assert_eq!(w.max(), 0.9, "the window holds 0.2 0.9 0.4 0.3");
    w.push(4, 0.5);
    assert_eq!(w.max(), 0.9, "0.9 is still inside [1..=4]");
    w.push(5, 0.6);
    assert_eq!(w.max(), 0.6, "0.9 and 0.2 have left the window");
    w.push(6, 1.4);
    assert_eq!(w.max(), 1.4);
    for i in 7..40 {
        w.push(i, 0.1);
    }
    assert_eq!(w.max(), 0.1, "and the spike eventually leaves");
}

/// **A ceiling drop mid-stream is honoured immediately**: the frames already
/// inside the delay line were pushed under the old ceiling, and because the
/// window holds *peaks* (not the derived gains) the new ceiling applies to them
/// at read time. The gate measured an 18 dB overshoot for one window-length in
/// the min-of-required-gains version.
#[test]
fn a_lower_ceiling_applies_to_the_frames_already_in_the_delay_line() {
    let mut node = MasterNode::new(48_000);
    node.set_param("ratio", 1.0);
    node.set_param("ceiling", 0.0);
    // Render one lookahead of loud material at ceiling 0 (no reduction at all).
    let mut out_block = vec![0.0f32; 256 * 2];
    let mut control = 0.0;
    let mut triggers = EventBuf::new();
    let mut notes = EventBuf::new();
    let loud = vec![0.5f32; 512];
    let mut interleaved = vec![0.0f32; 256 * 2];
    for i in 0..256 {
        interleaved[i * 2] = loud[i];
        interleaved[i * 2 + 1] = loud[i];
    }
    let io = NodeIO {
        audio_in: &interleaved,
        audio_ins: crate::AudioInputs::none(),
        audio_in_count: 1,
        audio_out_channels: 2,
        frames: 256,
        control_in: 0.0,
        triggers_in: &[],
        notes_in: &[],
    };
    node.render(
        &io,
        &mut out_block,
        &mut control,
        &mut triggers,
        &mut notes,
        block(256),
    );
    // Now drop the ceiling (to the bottom of the declared range, −12 dB — a direct
    // `set_param` clamps to `MASTER_PARAMS`, exactly as the logged path does) and
    // keep rendering the same loud material: every frame that leaves the line from
    // here must respect the *new* ceiling, including the frames that were already
    // inside it when the ceiling changed.
    node.set_param("ceiling", -12.0);
    let ceiling = db_to_lin(-12.0);
    let mut worst = 0.0f32;
    for _ in 0..8 {
        node.render(
            &io,
            &mut out_block,
            &mut control,
            &mut triggers,
            &mut notes,
            block(256),
        );
        for x in out_block.iter() {
            worst = worst.max(x.abs());
        }
    }
    assert!(
        worst <= ceiling + 1e-4,
        "the new ceiling applies to the buffered frames: {worst} vs {ceiling}"
    );
}

/// An impulse source: one frame of silence-then-1.0? No — a **step** at frame 0
/// (every frame of the first block), which the PDC delay shifts wholesale, so the
/// first non-zero output frame *is* the transit.
struct Step {
    sent: bool,
}

impl AudioNode for Step {
    fn latency(&self) -> u32 {
        0
    }
    fn render(
        &mut self,
        _io: &NodeIO,
        out: &mut [f32],
        _c: &mut f32,
        _t: &mut EventBuf<Trigger, CAP_EVENTS>,
        _n: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _b: RenderBlock,
    ) {
        out.fill(if self.sent { 0.0 } else { 1.0 });
        self.sent = true;
    }
}

/// A stereo pass-through (the "no latency" middle of a chain).
struct PassStereo;

impl AudioNode for PassStereo {
    fn latency(&self) -> u32 {
        0
    }
    fn render(
        &mut self,
        io: &NodeIO,
        out: &mut [f32],
        _c: &mut f32,
        _t: &mut EventBuf<Trigger, CAP_EVENTS>,
        _n: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _b: RenderBlock,
    ) {
        out.copy_from_slice(&io.audio_in[..out.len()]);
    }
}

/// **The transit is measured, not assumed.** A stereo node's PDC delay line runs
/// over interleaved samples, so its delay must be declared in *frames* and applied
/// as `frames * channels`: delaying by the frame count alone halved a stereo node's
/// compensation (240 samples is 240 mono frames but 120 stereo frames), which put
/// the master bus's output 120+ frames out of alignment. With the delay applied per
/// frame, the first non-zero output frame equals `flush_frames()` for a chain with
/// and without a middle node — and that equality is what an offline render's head
/// trim rests on ([`crate::Engine::render_with_drain_aligned`]).
#[test]
fn pdc_delays_multichannel_nodes_by_frames_and_flush_matches_the_transit() {
    for middle in [false, true] {
        let mut g = Graph::new();
        let src = g.add_node(
            NodeKind::Opaque(Box::new(Step { sent: false })),
            vec![Port::stereo_audio("audio", Direction::Out)],
        );
        let mut prev = src;
        if middle {
            let mid = g.add_node(
                NodeKind::Opaque(Box::new(PassStereo)),
                vec![
                    Port::stereo_audio("in", Direction::In),
                    Port::stereo_audio("audio", Direction::Out),
                ],
            );
            g.connect(src, "audio", mid, "in").unwrap();
            prev = mid;
        }
        let master = g.add_node(
            NodeKind::Opaque(Box::new(MasterNode::new(48_000))),
            MASTER_PORTS.to_vec(),
        );
        g.connect(prev, "audio", master, "audio").unwrap();
        g.set_out(master);

        // `Graph::render` fills exactly one block (BLOCK frames * channels), so
        // render two: the chained case's transit (720) is longer than one block.
        let tempo: &'static TempoMap = Box::leak(Box::new(TempoMap::new(48_000, 120.0, 4)));
        let mut left: Vec<f32> = Vec::new();
        for _ in 0..2 {
            let mut out = vec![0.0f32; 512 * 2];
            g.render(
                &mut out,
                RenderBlock {
                    frame: 0,
                    sample_rate: 48_000,
                    tempo,
                    mode: RenderMode::Timeline,
                },
            );
            left.extend(out.chunks(2).map(|f| f[0]));
        }
        let first = left
            .iter()
            .position(|x| x.abs() > 0.01)
            .expect("the step reaches the output");
        assert_eq!(
            first as u32,
            g.flush_frames(),
            "middle={middle}: the measured transit must equal flush_frames()                  (the trim an aligned offline render relies on)"
        );
        assert!(
            first >= 240,
            "middle={middle}: the master's lookahead is in the transit ({first})"
        );
    }
}

/// The graph node wraps the same DSP: mounting it as the out node and feeding
/// it stereo audio through a cord works end to end, with the PDC delay
/// compensated.
#[test]
fn the_master_compensates_its_lookahead_through_pdc() {
    let mut g = Graph::new();
    let src = g.add_node(
        NodeKind::Sine(crate::graph::Sine::new(440.0)),
        vec![Port::stereo_audio("audio", Direction::Out)],
    );
    let master = g.add_node(
        NodeKind::Opaque(Box::new(MasterNode::new(48_000))),
        MASTER_PORTS.to_vec(),
    );
    g.connect(src, "audio", master, "audio").unwrap();
    g.set_out(master);
    let mut out = [0.0f32; 128];
    g.render(
        &mut out,
        RenderBlock {
            frame: 0,
            sample_rate: 48_000,
            tempo: &TempoMap::new(48_000, 120.0, 4),
            mode: RenderMode::Timeline,
        },
    );
    assert!(
        g.flush_frames() >= MASTER_LOOKAHEAD_MS as u32 * 48,
        "the drain carries the limiter's delay: {}",
        g.flush_frames()
    );
}
