//! The **master bus** plugin: a compressor and a lookahead brickwall limiter on
//! the mix out, mounted after the mixer.
//!
//! This is the profile's mastering stage, and it is a graph node like any other:
//! the mixer's stereo `audio` out patches into its stereo `audio` in, the plugin
//! claims the graph's output, and the live pump and the bounce both flow through
//! it with no new code path (determinism inherited — the same input renders the
//! same bytes).
//!
//! **One documented truth:** the master *fader* is the mixer's `master.gain`;
//! `makeup` belongs to the compressor. Two gains on the same bus would fight.
//!
//! - **Compressor** — stereo-linked, feed-forward, peak-detecting: `threshold`
//!   (dBFS), `ratio`, `attack_ms`, `release_ms`, `makeup` (dB). The detector
//!   follows `max(|L|, |R|)` so a loud channel cannot duck only itself (which
//!   would move the stereo image), and the gain is applied to both channels.
//!   Its defaults are gentle glue (`-12 dB`, 2:1) — `ratio 1` is exactly
//!   transparent.
//! - **Lookahead brickwall limiter** — `ceiling` (dBFS, default −0.4). A sliding
//!   minimum over a `lookahead` window of the *required* gain makes the gain
//!   already reduced when a transient arrives, so nothing crosses the ceiling by
//!   construction rather than by hoping the release is fast enough. The input is
//!   delayed by the lookahead, and that delay is **declared through
//!   `AudioNode::latency`** so the interpreter's PDC compensates it — the
//!   limiter is sample-aligned with every other path.
//! - **Metering** — peak (post-limiter, per side) and gain reduction (dB, the
//!   most the compressor or limiter pulled this block) are written to shared
//!   atomics under the `master.meters` context key: the render path never
//!   blocks and never allocates.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{Disposer, ParamDef, Plugin, PluginApi};
use crate::graph::{
    AudioNode, CAP_EVENTS, Direction, EventBuf, NodeIO, NodeId, NodeKind, NoteEvent, Port,
    RenderBlock, SignalKind, Trigger,
};

/// The limiter's lookahead, in milliseconds. 5 ms is enough to catch a transient
/// before it is heard while keeping the added latency musically invisible (and
/// well inside the graph's [`crate::graph::MAX_PDC`]).
pub const MASTER_LOOKAHEAD_MS: f32 = 5.0;
/// Default ceiling: −0.4 dBFS — the plan's number, leaving a little headroom for
/// a later lossy encode without an audible level drop.
pub const MASTER_CEILING_DB: f32 = -0.4;
/// The master plugin's port surface: a stereo bus in, a stereo bus out.
pub const MASTER_PORTS: &[Port] = &[
    Port {
        name: "audio",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 2,
    },
    Port {
        name: "audio",
        direction: Direction::Out,
        kind: SignalKind::Audio,
        channels: 2,
    },
];
/// The declared parameter surface — the logged `SetParam` namespace.
pub const MASTER_PARAMS: &[ParamDef] = &[
    ParamDef {
        name: "threshold",
        min: -60.0,
        max: 0.0,
    },
    ParamDef {
        name: "ratio",
        min: 1.0,
        max: 20.0,
    },
    ParamDef {
        name: "attack_ms",
        min: 0.1,
        max: 200.0,
    },
    ParamDef {
        name: "release_ms",
        min: 5.0,
        max: 2_000.0,
    },
    ParamDef {
        name: "makeup",
        min: -12.0,
        max: 24.0,
    },
    ParamDef {
        name: "ceiling",
        min: -12.0,
        max: 0.0,
    },
];

/// Default compressor threshold (dBFS): gentle glue, not a brickwall — the
/// limiter is what guarantees the ceiling.
pub const MASTER_THRESHOLD_DB: f32 = -12.0;
/// Default compressor ratio (2:1 — audible as glue, not as pumping).
pub const MASTER_RATIO: f32 = 2.0;
/// Default attack (ms): slow enough to let transients through to the limiter.
pub const MASTER_ATTACK_MS: f32 = 10.0;
/// Default release (ms).
pub const MASTER_RELEASE_MS: f32 = 120.0;

/// Per-block master meters: peak per side (post-limiter) and the gain reduction
/// applied this block, all as `f32` bits in atomics — the render path writes,
/// the control side reads. `PartialEq`-free by nature (atomics).
pub struct MasterMeters {
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    /// Gain reduction in dB (≥ 0): how much the compressor + limiter pulled the
    /// block down. `0` is "transparent".
    reduction_db: AtomicU32,
}

impl MasterMeters {
    /// Peak of the left output for the last rendered block.
    pub fn peak_l(&self) -> f32 {
        f32::from_bits(self.peak_l.load(Ordering::Relaxed))
    }

    /// Peak of the right output for the last rendered block.
    pub fn peak_r(&self) -> f32 {
        f32::from_bits(self.peak_r.load(Ordering::Relaxed))
    }

    /// Gain reduction of the last rendered block, in dB (≥ 0).
    pub fn reduction_db(&self) -> f32 {
        f32::from_bits(self.reduction_db.load(Ordering::Relaxed))
    }
}

impl Default for MasterMeters {
    fn default() -> Self {
        MasterMeters {
            peak_l: AtomicU32::new(0.0f32.to_bits()),
            peak_r: AtomicU32::new(0.0f32.to_bits()),
            reduction_db: AtomicU32::new(0.0f32.to_bits()),
        }
    }
}

/// A sliding-window **maximum** over the last `cap` values: the limiter's
/// lookahead, held over *peaks* so the gain can be derived at read time from the
/// newest `ceiling`. Monotonic deque over two preallocated rings — amortized O(1),
/// no allocation on the render path, no per-block scan.
struct MaxWindow {
    cap: usize,
    vals: Vec<f32>,
    idx: Vec<u64>,
    head: usize,
    len: usize,
}

impl MaxWindow {
    fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        MaxWindow {
            cap,
            // One spare slot separates "full" from "empty" in the ring.
            vals: vec![0.0; cap + 1],
            idx: vec![0; cap + 1],
            head: 0,
            len: 0,
        }
    }

    fn at(&self, i: usize) -> usize {
        (self.head + i) % self.vals.len()
    }

    fn push(&mut self, index: u64, value: f32) {
        // Drop everything the new value dominates (it is ≤ as large and newer).
        while self.len > 0 {
            let back = self.at(self.len - 1);
            if self.vals[back] <= value {
                self.len -= 1;
            } else {
                break;
            }
        }
        let slot = self.at(self.len);
        self.vals[slot] = value;
        self.idx[slot] = index;
        self.len += 1;
        // Drop the front once it falls out of the window `[index - cap + 1, index]`.
        while self.len > 0 {
            let front = self.at(0);
            if self.idx[front] + self.cap as u64 <= index {
                self.head = (self.head + 1) % self.vals.len();
                self.len -= 1;
            } else {
                break;
            }
        }
    }

    /// The largest value in the window (0 when empty — no peak yet, so nothing to
    /// limit).
    fn max(&self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        self.vals[self.at(0)]
    }
}

/// `x`, or `0.0` when it is not finite (NaN, ±inf) — the bus's input policy.
fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

fn db_to_lin(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

fn lin_to_db(lin: f32) -> f32 {
    if lin <= 0.0 {
        -120.0
    } else {
        20.0 * lin.log10()
    }
}

/// The master node: compressor, then lookahead limiter, on an interleaved stereo
/// bus. One frame of state per stage — no per-block allocation.
pub struct MasterNode {
    // Compressor
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_db: f32,
    /// Attack/release coefficients, recomputed when the times or the rate change.
    attack_coeff: f32,
    release_coeff: f32,
    sample_rate: u32,
    /// Peak envelope (linear), stereo-linked.
    env: f32,
    // Limiter
    ceiling_db: f32,
    lookahead: usize,
    /// The delayed input, interleaved: `2 * lookahead` frames.
    delay: Vec<f32>,
    /// The delayed **dry** input (pre-compressor), so the reduction meter compares
    /// the output frame against the frame that actually produced it.
    dry_delay: Vec<f32>,
    delay_pos: usize,
    /// Sliding maximum of the peak over the lookahead window.
    window: MaxWindow,
    /// The gain currently applied (linear), released upward toward the target the
    /// lookahead window's peak implies.
    gain: f32,
    frame_index: u64,
    meters: Arc<MasterMeters>,
}

impl MasterNode {
    /// A master chain at the documented defaults, for `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_meters(sample_rate, Arc::new(MasterMeters::default()))
    }

    /// The plugin's constructor: the meter bank is shared so the profile can read
    /// it through the `master.meters` context key.
    pub fn with_meters(sample_rate: u32, meters: Arc<MasterMeters>) -> Self {
        let lookahead = ((MASTER_LOOKAHEAD_MS / 1000.0) * sample_rate as f32).round() as usize;
        // Never zero: a limiter without lookahead cannot honour a ceiling before
        // the fact, and the PDC path is what makes the delay inaudible.
        let lookahead = lookahead.clamp(1, crate::graph::MAX_PDC / 2);
        let mut node = MasterNode {
            threshold_db: MASTER_THRESHOLD_DB,
            ratio: MASTER_RATIO,
            attack_ms: MASTER_ATTACK_MS,
            release_ms: MASTER_RELEASE_MS,
            makeup_db: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            sample_rate,
            env: 0.0,
            ceiling_db: MASTER_CEILING_DB,
            lookahead,
            delay: vec![0.0; lookahead * 2],
            dry_delay: vec![0.0; lookahead * 2],
            delay_pos: 0,
            window: MaxWindow::new(lookahead + 1),
            gain: 1.0,
            frame_index: 0,
            meters,
        };
        node.update_ballistics();
        node
    }

    /// The lookahead in frames — also the node's declared PDC latency.
    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    pub fn meters(&self) -> Arc<MasterMeters> {
        self.meters.clone()
    }

    fn update_ballistics(&mut self) {
        let sr = self.sample_rate.max(1) as f32;
        let coeff = |ms: f32| (-1.0 / (ms.max(0.01) / 1000.0 * sr)).exp();
        self.attack_coeff = coeff(self.attack_ms);
        self.release_coeff = coeff(self.release_ms);
    }

    fn set_param(&mut self, name: &str, value: f32) {
        // The loud path validates against `MASTER_PARAMS`; direct calls (tests,
        // `Graph::set_param`) are clamped here so a node can never be steered out
        // of its declared range.
        match name {
            "threshold" => self.threshold_db = value.clamp(-60.0, 0.0),
            "ratio" => self.ratio = value.clamp(1.0, 20.0),
            "attack_ms" => {
                self.attack_ms = value.clamp(0.1, 200.0);
                self.update_ballistics();
            }
            "release_ms" => {
                self.release_ms = value.clamp(5.0, 2_000.0);
                self.update_ballistics();
            }
            "makeup" => self.makeup_db = value.clamp(-12.0, 24.0),
            "ceiling" => self.ceiling_db = value.clamp(-12.0, 0.0),
            other => debug_assert!(false, "master: unknown param '{other}'"),
        }
    }

    /// Compressor gain for a peak level (linear), in dB: `0` at or below
    /// threshold, then `(threshold - level) * (1 - 1/ratio)`.
    fn compressor_gain_db(&self, peak: f32) -> f32 {
        if peak <= 0.0 {
            return 0.0;
        }
        let over = lin_to_db(peak) - self.threshold_db;
        if over <= 0.0 {
            0.0
        } else {
            -over * (1.0 - 1.0 / self.ratio)
        }
    }
}

impl Default for MasterNode {
    fn default() -> Self {
        Self::new(48_000)
    }
}

impl AudioNode for MasterNode {
    fn latency(&self) -> u32 {
        // The limiter's delay is real latency: the graph compensates it (PDC) so
        // the master stays aligned with every parallel path.
        self.lookahead as u32
    }

    fn render(
        &mut self,
        io: &NodeIO,
        out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    ) {
        if block.sample_rate != self.sample_rate {
            // The rate is fixed per session (the clock owns it); a mismatch means
            // the node was mounted into a different session than it was built for.
            debug_assert_eq!(
                block.sample_rate, self.sample_rate,
                "master node sample rate mismatch"
            );
            self.sample_rate = block.sample_rate;
            self.update_ballistics();
        }
        let ceiling = db_to_lin(self.ceiling_db);
        let makeup = db_to_lin(self.makeup_db);
        let frames = out.len() / 2;
        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        let mut max_reduction = 0.0f32;
        for i in 0..frames {
            // The bus is where upstream garbage converges, so a non-finite sample is
            // replaced by silence rather than propagated: a NaN would pass straight
            // through, and an `+inf` would make the required gain `ceiling/inf = 0`,
            // turning into `inf * 0 = NaN` one lookahead later — a single bad sample
            // becoming a burst of them.
            let l = finite_or_zero(io.audio_in.get(i * 2).copied().unwrap_or(0.0));
            let r = finite_or_zero(io.audio_in.get(i * 2 + 1).copied().unwrap_or(0.0));

            // --- compressor: stereo-linked peak detector -------------------
            let level = l.abs().max(r.abs());
            let coeff = if level > self.env {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.env = coeff * self.env + (1.0 - coeff) * level;
            let comp_db = self.compressor_gain_db(self.env);
            let comp = db_to_lin(comp_db) * makeup;

            let cl = l * comp;
            let cr = r * comp;

            // --- limiter: a sliding maximum of the PEAK over the lookahead, with
            // the required gain computed at *read* time, applied to the delayed
            // signal ----------------------------------------------------------
            // The window holds the peaks of the frames still inside the delay line
            // plus this one, so for the frame leaving the line it is exactly the
            // peak over that frame's forward window [j, j + lookahead]. Deriving the
            // gain here — not when the frame was pushed — is what makes a `ceiling`
            // change apply to every buffered frame *instantly* instead of ageing out
            // over one lookahead (the gate measured an 18 dB overshoot for one
            // window-length during a mid-stream ceiling drop).
            self.window.push(self.frame_index, cl.abs().max(cr.abs()));
            self.frame_index += 1;
            let window_peak = self.window.max();
            let target = if window_peak > ceiling {
                ceiling / window_peak
            } else {
                1.0
            };
            // Attack is instant (brickwall by construction); the release only ever
            // *rises* toward the target, never past it.
            if target < self.gain {
                self.gain = target;
            } else {
                self.gain =
                    (self.gain + (target - self.gain) * (1.0 - self.release_coeff)).min(target);
            }

            let dl = self.delay[self.delay_pos * 2];
            let dr = self.delay[self.delay_pos * 2 + 1];
            let ddl = self.dry_delay[self.delay_pos * 2];
            let ddr = self.dry_delay[self.delay_pos * 2 + 1];
            self.delay[self.delay_pos * 2] = cl;
            self.delay[self.delay_pos * 2 + 1] = cr;
            self.dry_delay[self.delay_pos * 2] = l;
            self.dry_delay[self.delay_pos * 2 + 1] = r;
            self.delay_pos = (self.delay_pos + 1) % self.lookahead;

            let yl = dl * self.gain;
            let yr = dr * self.gain;
            out[i * 2] = yl;
            out[i * 2 + 1] = yr;
            peak_l = peak_l.max(yl.abs());
            peak_r = peak_r.max(yr.abs());
            // Total reduction (compressor + limiter) against the **same frame's** dry
            // input: the output frame leaves the delay line `lookahead` frames after its
            // dry input arrived, so the dry is delayed alongside it. Comparing against
            // the *current* `l`/`r` instead measures two different moments — it reported
            // a phantom 114 dB of reduction while the limiter was idle, simply counting
            // the delay line's warm-up.
            let dry = ddl.abs().max(ddr.abs());
            let wet = yl.abs().max(yr.abs());
            if dry > 1e-9 {
                max_reduction = max_reduction.max(lin_to_db(dry) - lin_to_db(wet));
            }
        }
        self.meters
            .peak_l
            .store(peak_l.to_bits(), Ordering::Relaxed);
        self.meters
            .peak_r
            .store(peak_r.to_bits(), Ordering::Relaxed);
        self.meters
            .reduction_db
            .store(max_reduction.to_bits(), Ordering::Relaxed);
    }

    fn set_param(&mut self, name: &str, value: f32) {
        self.set_param(name, value);
    }
}

/// The master plugin: mounting it claims the graph's output (so it must be
/// mounted **after** the mixer and patched from it) and provides its meters under
/// the `master.meters` context key. Unmounting restores the previous bus owner,
/// so dropping the mastering stage does not leave the graph silent.
pub struct MasterPlugin;

impl Plugin for MasterPlugin {
    fn id(&self) -> &'static str {
        "master"
    }

    fn inject(&self) -> &'static [&'static str] {
        // The bus it processes comes from a patch, not a service: nothing to inject.
        &[]
    }

    fn ports(&self) -> &'static [Port] {
        MASTER_PORTS
    }

    fn params(&self) -> &'static [ParamDef] {
        MASTER_PARAMS
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let meters = Arc::new(MasterMeters::default());
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(MasterNode::with_meters(
                api.clock.sample_rate,
                meters.clone(),
            ))),
            MASTER_PORTS.to_vec(),
        );
        let previous = api.graph.out_node;
        api.graph.set_out(node);
        api.ctx.provide("master.meters", meters);
        Ok((
            node,
            Box::new(move |dis: &mut super::DisposerCtx| {
                dis.graph.remove_node(node);
                dis.ctx.remove("master.meters");
                // The bus goes back to whoever owned it before: unmounting the
                // mastering stage must not leave the graph with no output.
                if dis.graph.out_node.is_none()
                    && let Some(prev) = previous
                {
                    dis.graph.set_out(prev);
                }
            }),
        ))
    }
}

/// Factory form: the master takes no mount parameters (the chain is configured
/// through `set_param`, which the log carries).
pub fn master_factory(_params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    Ok(Box::new(MasterPlugin))
}

#[cfg(test)]
mod tests {
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
                audio_ins: [&[], &[], &[], &[], &[], &[], &[], &[]],
                audio_in_count: 1,
                audio_out_channels: 2,
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
            audio_ins: [&[], &[], &[], &[], &[], &[], &[], &[]],
            audio_in_count: 1,
            audio_out_channels: 2,
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
}
