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
#[path = "tests/master.rs"]
mod tests;
