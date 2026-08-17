//! Core piece 2 — the audio graph interpreter (minimal-core note §2).
//!
//! The graph is a *value*: nodes, single-input edges, and params. Plugins edit
//! the value; the interpreter renders it, allocation-free, block by block.
//! Two node tiers:
//! - **declarative** ([`Sine`], [`Gain`]) — serializable, diffable, IPC-safe;
//! - **opaque** ([`AudioNode`] trait objects, e.g. [`BlipSynth`] today, fundsp
//!   composites and CLAP later) — trusted in-process code under RT discipline.
//!
//! Per-node latency is part of the value from day one: every node reports its
//! latency and the interpreter compensates across the chain (PDC), delaying
//! earlier stages so parallel paths align.

use std::f64::consts::TAU;

/// Render block size. The hot path is fixed-size; no allocation per block.
pub const BLOCK: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Copy)]
pub struct RenderBlock {
    pub frame: u64,
    pub sample_rate: u32,
}

/// The **opaque tier**: nodes contributed as code (fundsp composites today,
/// CLAP tomorrow). Must not allocate or block on the render path.
pub trait AudioNode: Send {
    /// Samples of latency this node introduces (PDC; 0 for most nodes).
    fn latency(&self) -> u32;
    fn render(&mut self, inputs: &[&[f32]], out: &mut [f32], block: RenderBlock);
    fn set_param(&mut self, _name: &str, _value: f32) {}
    /// Sample-accurate trigger within the current block (offset from block start).
    fn on_trigger(&mut self, _offset: u32) {}
}

/// A node in the graph: the value model's unit.
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
}

impl Node {
    pub fn latency(&self) -> u32 {
        match &self.kind {
            NodeKind::Sine(_) | NodeKind::Gain(_) => 0,
            NodeKind::Opaque(node) => node.latency(),
        }
    }
}

pub enum NodeKind {
    Sine(Sine),
    Gain(Gain),
    Opaque(Box<dyn AudioNode>),
}

impl NodeKind {
    fn render(&mut self, inputs: &[&[f32]], out: &mut [f32], block: RenderBlock) {
        match self {
            NodeKind::Sine(node) => node.render(inputs, out, block),
            NodeKind::Gain(node) => node.render(inputs, out, block),
            NodeKind::Opaque(node) => node.render(inputs, out, block),
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        match self {
            NodeKind::Sine(node) => node.set_param(name, value),
            NodeKind::Gain(node) => node.set_param(name, value),
            NodeKind::Opaque(node) => node.set_param(name, value),
        }
    }

    fn trigger(&mut self, offset: u32) {
        if let NodeKind::Opaque(node) = self {
            node.on_trigger(offset);
        }
    }
}

/// Declarative tier: a sine oscillator (source).
#[derive(Debug, Clone)]
pub struct Sine {
    pub freq: f32,
    phase: f64,
}

impl Sine {
    pub fn new(freq: f32) -> Self {
        Sine { freq, phase: 0.0 }
    }
}

impl AudioNode for Sine {
    fn latency(&self) -> u32 {
        0
    }

    fn render(&mut self, _inputs: &[&[f32]], out: &mut [f32], block: RenderBlock) {
        let inc = TAU * self.freq as f64 / block.sample_rate as f64;
        for sample in out.iter_mut() {
            *sample = self.phase.sin() as f32;
            self.phase += inc;
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        if name == "freq" {
            self.freq = value;
        }
    }
}

/// Declarative tier: a gain stage.
#[derive(Debug, Clone)]
pub struct Gain {
    pub gain: f32,
}

impl Gain {
    pub fn new(gain: f32) -> Self {
        Gain { gain }
    }
}

impl AudioNode for Gain {
    fn latency(&self) -> u32 {
        0
    }

    fn render(&mut self, inputs: &[&[f32]], out: &mut [f32], _block: RenderBlock) {
        let src = inputs.first().copied().unwrap_or(&[]);
        for (i, sample) in out.iter_mut().enumerate() {
            *sample = src.get(i).copied().unwrap_or(0.0) * self.gain;
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        if name == "gain" {
            self.gain = value;
        }
    }
}

/// Opaque tier: a monophonic blip voice with sample-accurate onsets and a fixed
/// quadratic-decay envelope. Fixed-size arrays only — no allocation on the
/// render path. Voice *management* (allocation/stealing) is deliberately
/// deferred (RESEARCH §14 risk 8); one voice with a small fixed pool is enough
/// for the spike.
pub struct BlipSynth {
    pub freq: f32,
    pub gain: f32,
    pub blip_len: u32,
    blips: [Option<Blip>; MAX_BLIPS],
    /// (offset, len) pending onsets, in ascending offset order (generator contract).
    pending: [(u32, u32); MAX_PENDING],
    pending_head: usize,
    pending_count: usize,
}

const MAX_BLIPS: usize = 16;
const MAX_PENDING: usize = 8;

#[derive(Clone, Copy)]
struct Blip {
    phase: f64,
    remaining: u32,
    len: u32,
}

impl BlipSynth {
    pub fn new(freq: f32, gain: f32, blip_len: u32) -> Self {
        BlipSynth {
            freq,
            gain,
            blip_len: blip_len.max(1),
            blips: [None; MAX_BLIPS],
            pending: [(0, 0); MAX_PENDING],
            pending_head: 0,
            pending_count: 0,
        }
    }
}

impl AudioNode for BlipSynth {
    fn latency(&self) -> u32 {
        0
    }

    fn on_trigger(&mut self, offset: u32) {
        if self.pending_count < MAX_PENDING {
            let at = self.pending_head + self.pending_count;
            self.pending[at] = (offset, self.blip_len);
            self.pending_count += 1;
        }
        // beyond the pool: drop (documented; voice management is deferred)
    }

    fn render(&mut self, _inputs: &[&[f32]], out: &mut [f32], block: RenderBlock) {
        let inc = TAU * self.freq as f64 / block.sample_rate as f64;
        for (i, sample) in out.iter_mut().enumerate() {
            // start blips whose onset lands on this exact sample
            while self.pending_count > 0 && self.pending[self.pending_head].0 as usize == i {
                let (_, len) = self.pending[self.pending_head];
                if let Some(slot) = self.blips.iter_mut().find(|b| b.is_none()) {
                    *slot = Some(Blip {
                        phase: 0.0,
                        remaining: len,
                        len,
                    });
                }
                self.pending_head += 1;
                self.pending_count -= 1;
            }
            let mut s = 0.0f32;
            for slot in self.blips.iter_mut() {
                let finished = if let Some(blip) = slot {
                    let env = (blip.remaining as f32 / blip.len as f32).powi(2);
                    s += blip.phase.sin() as f32 * self.gain * env;
                    blip.phase += inc;
                    blip.remaining -= 1;
                    blip.remaining == 0
                } else {
                    false
                };
                if finished {
                    *slot = None;
                }
            }
            *sample = s;
        }
        // All pending onsets for this block were consumed; reset the cursor so
        // the fixed array is reused from the start (never grows past MAX_PENDING).
        if self.pending_count == 0 {
            self.pending_head = 0;
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        match name {
            "freq" => self.freq = value,
            "gain" => self.gain = value,
            "blip_len" => self.blip_len = (value as u32).max(1),
            _ => {}
        }
    }
}

/// A per-stage delay line for PDC (delay of `len` samples).
#[derive(Clone)]
pub struct RingDelay {
    buf: Vec<f32>,
    pos: usize,
    len: usize,
}

impl RingDelay {
    pub fn new(len: usize) -> Self {
        RingDelay {
            buf: vec![0.0; len],
            pos: 0,
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Push `x`, return the value delayed by `len` samples.
    pub fn process(&mut self, x: f32) -> f32 {
        if self.len == 0 {
            return x;
        }
        let delayed = self.buf[self.pos];
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.len;
        delayed
    }
}

/// The graph value + interpreter. Spike A: a single-input feed-forward chain
/// (fan-out/sends arrive with the mixer in Phase 1); edges must go forward.
pub struct Graph {
    nodes: Vec<Node>,
    /// in-edge: for node `i`, the index of its single input node.
    input_of: Vec<Option<usize>>,
    /// per-node output scratch (BLOCK-sized, preallocated).
    scratch: Vec<Vec<f32>>,
    /// per-node PDC delay lines.
    delays: Vec<RingDelay>,
    /// per-node cumulative latency, preallocated (no allocation in render).
    cum: Vec<u32>,
    next_id: u64,
    pub out_node: Option<NodeId>,
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            nodes: Vec::new(),
            input_of: Vec::new(),
            scratch: Vec::new(),
            delays: Vec::new(),
            cum: Vec::new(),
            next_id: 0,
            out_node: None,
        }
    }

    pub fn add_node(&mut self, kind: NodeKind) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.push(Node { id, kind });
        self.input_of.push(None);
        self.scratch.push(vec![0.0; BLOCK]);
        self.delays.push(RingDelay::new(0));
        self.cum.push(0);
        if self.out_node.is_none() {
            self.out_node = Some(id);
        }
        id
    }

    pub fn connect(&mut self, from: NodeId, to: NodeId) -> Result<(), String> {
        let fi = self.index_of(from).ok_or("connect: unknown 'from' node")?;
        let ti = self.index_of(to).ok_or("connect: unknown 'to' node")?;
        if fi >= ti {
            return Err("connect: edges must go forward (topological order)".into());
        }
        if self.input_of[ti].is_some() {
            return Err("connect: spike A is a single-input chain".into());
        }
        self.input_of[ti] = Some(fi);
        Ok(())
    }

    pub fn set_out(&mut self, id: NodeId) {
        self.out_node = Some(id);
    }

    pub fn remove_node(&mut self, id: NodeId) -> Option<Node> {
        let idx = self.index_of(id)?;
        if self.out_node == Some(id) {
            self.out_node = None;
        }
        self.input_of.remove(idx);
        self.scratch.remove(idx);
        self.delays.remove(idx);
        self.cum.remove(idx);
        for edge in self.input_of.iter_mut() {
            if let Some(j) = edge
                && *j > idx {
                    *j -= 1;
                }
        }
        Some(self.nodes.remove(idx))
    }

    pub fn set_param(&mut self, id: NodeId, name: &str, value: f32) {
        if let Some(i) = self.index_of(id) {
            self.nodes[i].kind.set_param(name, value);
        }
    }

    /// Sample-accurate trigger delivery to a node within the current block.
    pub fn trigger(&mut self, id: NodeId, offset: u32) {
        if let Some(i) = self.index_of(id) {
            self.nodes[i].kind.trigger(offset);
        }
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    fn index_of(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }

    /// Render one block into `out`. Allocation-free: scratch, delay lines and
    /// the cumulative-latency vector are preallocated; per-node latency drives
    /// PDC (earlier stages delayed so all paths align at the output). Node
    /// latency is fixed at mount in Spike A; a runtime latency change would
    /// rebuild a delay line (a control-side event with a click — real dynamic
    /// PDC arrives with Phase 1).
    pub fn render(&mut self, out: &mut [f32], block: RenderBlock) {
        let len = out.len();
        let mut max_cum = 0u32;
        for i in 0..self.nodes.len() {
            let base = self.input_of[i].map(|j| self.cum[j]).unwrap_or(0);
            self.cum[i] = base + self.nodes[i].latency();
            max_cum = max_cum.max(self.cum[i]);
        }

        for i in 0..self.nodes.len() {
            let input = self.input_of[i];
            let (before, after) = self.scratch.split_at_mut(i);
            let out_buf = &mut after[0][..len];
            let inputs: [&[f32]; 1] = match input {
                Some(j) => [&before[j][..len]],
                None => [&[]],
            };
            self.nodes[i].kind.render(&inputs, out_buf, block);

            // PDC: delay this stage so it aligns with the longest path.
            let d = (max_cum - self.cum[i]) as usize;
            if self.delays[i].len() != d {
                self.delays[i] = RingDelay::new(d);
            }
            if d > 0 {
                for sample in out_buf.iter_mut() {
                    *sample = self.delays[i].process(*sample);
                }
            }
        }

        match self.out_node.and_then(|id| self.index_of(id)) {
            Some(i) => out.copy_from_slice(&self.scratch[i][..len]),
            None => out.fill(0.0),
        }
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3-sample-latency opaque node: passes its input through unchanged, but
    /// *reports* latency so the interpreter must compensate.
    struct TestDelay {
        len: u32,
    }

    impl AudioNode for TestDelay {
        fn latency(&self) -> u32 {
            self.len
        }

        fn render(&mut self, inputs: &[&[f32]], out: &mut [f32], _block: RenderBlock) {
            let src = inputs.first().copied().unwrap_or(&[]);
            out.copy_from_slice(&src[..out.len().min(src.len())]);
        }
    }

    #[test]
    fn pdc_delays_the_fast_path() {
        let mut g = Graph::new();
        let sine = g.add_node(NodeKind::Sine(Sine::new(440.0)));
        let delay = g.add_node(NodeKind::Opaque(Box::new(TestDelay { len: 3 })));
        g.connect(sine, delay).unwrap();
        g.set_out(delay);

        let mut out = [0.0f32; 64];
        let block = RenderBlock { frame: 0, sample_rate: 48_000 };
        g.render(&mut out, block);

        // The sine is delayed by 3 samples (PDC), so the first 3 samples are
        // silence and sine sample k lands at out[3+k].
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.0);
        let inc = (TAU * 440.0 / 48_000.0).sin() as f32;
        assert!((out[4] - inc).abs() < 1e-6, "out[4] = {}", out[4]);
        assert!((out[5] - (TAU * 440.0 * 2.0 / 48_000.0).sin() as f32).abs() < 1e-6);
    }

    #[test]
    fn remove_node_fixes_edges() {
        let mut g = Graph::new();
        let a = g.add_node(NodeKind::Gain(Gain::new(1.0)));
        let b = g.add_node(NodeKind::Gain(Gain::new(1.0)));
        let c = g.add_node(NodeKind::Gain(Gain::new(1.0)));
        g.connect(a, b).unwrap();
        g.connect(b, c).unwrap();
        g.remove_node(a);
        // b's input edge was removed with a; c's input now points at b (index 0).
        assert_eq!(g.nodes().len(), 2);
    }
}
