//! Core piece 2 — the audio graph interpreter, now a **patch bay** (Spike A.5,
//! patch-bay note 2026-08-17): nodes declare named, typed ports; patch cords
//! connect them; the interpreter routes by signal kind.
//!
//! Signal kinds:
//! - **Audio** — sample buffers;
//! - **Control** — one f32 per block;
//! - **Trigger** / **Note** — sample-timestamped event streams (offsets within
//!   the block), carried in fixed-capacity buffers.
//!
//! Phase 1 port rule: **many audio `In` ports per node** (the mixer's
//! channels, fan-in summed per port), **at most one audio `Out`**, and at most
//! one control/trigger/note port per kind (fan-in sums audio, merges events,
//! single-driver for control). The Phase 1 mixer generalizes the bus.
//!
//! Per-node latency (PDC) is part of the value from day one; the interpreter
//! delays earlier stages so audio paths align at the output.
//!
//! Render path invariant: **no allocation** — every buffer is preallocated or
//! fixed-capacity (enforced by a counting-allocator test).

use std::f64::consts::TAU;

use crate::clock::TempoMap;

/// Render block size. The hot path is fixed-size; no allocation per block.
pub const BLOCK: usize = 512;

/// Event capacity per output port per block.
pub const CAP_EVENTS: usize = 32;
/// Event capacity for merged fan-in per block.
pub const MERGE_CAP: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// A sample-accurate trigger: the offset of the event within the block.
pub type Trigger = u32;

/// A pitched event (musical-event model note): pitch in semitones from A4
/// (440 Hz), offset within the block, velocity 0..1, duration in samples.
/// The representation is continuous (f32); 12-TET is a quantization.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct NoteEvent {
    pub offset: u32,
    pub pitch: f32,
    pub velocity: f32,
    pub duration: u32,
}

/// Fixed-capacity event buffer — the no-allocation carrier for trigger/note
/// streams on the render path. Pushes beyond capacity are dropped (documented).
pub struct EventBuf<T: Copy + Default, const CAP: usize> {
    buf: [T; CAP],
    count: usize,
}

impl<T: Copy + Default, const CAP: usize> EventBuf<T, CAP> {
    pub fn new() -> Self {
        Self {
            buf: std::array::from_fn(|_| T::default()),
            count: 0,
        }
    }

    pub fn clear(&mut self) {
        self.count = 0;
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn is_full(&self) -> bool {
        self.count == CAP
    }

    /// Push; returns false (and drops) when the buffer is full.
    pub fn push(&mut self, value: T) -> bool {
        if self.count == CAP {
            return false;
        }
        self.buf[self.count] = value;
        self.count += 1;
        true
    }

    /// Insert keeping the buffer sorted by offset — the merge point for event
    /// fan-in (multiple producers, each ascending; the merged stream must stay
    /// sorted for consumers like [`ToneGen`] that drain by offset). O(n) shift
    /// on a fixed array; returns false (and drops) when full.
    pub fn insert_sorted(&mut self, value: T) -> bool
    where
        T: PartialOrd,
    {
        if self.count == CAP {
            return false;
        }
        let pos = self.buf[..self.count].partition_point(|v| *v <= value);
        if pos < self.count {
            self.buf.copy_within(pos..self.count, pos + 1);
        }
        self.buf[pos] = value;
        self.count += 1;
        true
    }

    pub fn as_slice(&self) -> &[T] {
        &self.buf[..self.count]
    }
}

impl<T: Copy + Default, const CAP: usize> Default for EventBuf<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

/// The kind of signal a port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalKind {
    Audio,
    Control,
    Trigger,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    In,
    Out,
}

/// A named, typed port on a node — the patch-bay surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Port {
    pub name: &'static str,
    pub direction: Direction,
    pub kind: SignalKind,
}

#[derive(Debug, Clone, Copy)]
pub struct RenderBlock<'a> {
    pub frame: u64,
    pub sample_rate: u32,
    pub tempo: &'a TempoMap,
}

/// Maximum audio inputs a node may declare (the mixer's channels, Phase 1).
pub const MAX_AUDIO_INS: usize = 8;

/// What a node may read this block: its inputs, merged from connected
/// producers by the interpreter. Audio inputs are per *port* (a mixer's
/// channels are separate inputs, `audio_ins[..audio_in_count]`); control,
/// trigger, and note stay single-port per node in Phase 1.
pub struct NodeIO<'a> {
    /// the first audio input (convenience for single-input nodes; `&[]` when
    /// the node declares none)
    pub audio_in: &'a [f32],
    /// per audio-In port, in port order; meaningful up to `audio_in_count`
    pub audio_ins: [&'a [f32]; MAX_AUDIO_INS],
    pub audio_in_count: usize,
    pub control_in: f32,
    pub triggers_in: &'a [Trigger],
    pub notes_in: &'a [NoteEvent],
}

/// The **opaque tier**: nodes contributed as code (fundsp composites later,
/// CLAP after that). Must not allocate or block on the render path.
pub trait AudioNode: Send {
    /// Samples of latency this node introduces on its audio path (PDC).
    fn latency(&self) -> u32;
    fn render(
        &mut self,
        io: &NodeIO,
        out_audio: &mut [f32],
        out_control: &mut f32,
        out_triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        out_notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    );
    fn set_param(&mut self, _name: &str, _value: f32) {}
}

/// A node in the graph: the value model's unit, with its declared ports.
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub ports: Vec<Port>,
}

impl Node {
    pub fn latency(&self) -> u32 {
        match &self.kind {
            NodeKind::Sine(_) | NodeKind::Gain(_) => 0,
            NodeKind::Opaque(node) => node.latency(),
        }
    }

    pub fn port(&self, name: &str) -> Option<Port> {
        self.ports.iter().copied().find(|p| p.name == name)
    }

    /// The position of a port within this node's ports vec — patch cords
    /// carry port indices, which are stable for the node's lifetime.
    pub fn port_index(&self, name: &str) -> Option<usize> {
        self.ports.iter().position(|p| p.name == name)
    }
}

pub enum NodeKind {
    Sine(Sine),
    Gain(Gain),
    Opaque(Box<dyn AudioNode>),
}

impl NodeKind {
    fn render(
        &mut self,
        io: &NodeIO,
        out_audio: &mut [f32],
        out_control: &mut f32,
        out_triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        out_notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    ) {
        match self {
            NodeKind::Sine(node) => node.render(io, out_audio, out_control, out_triggers, out_notes, block),
            NodeKind::Gain(node) => node.render(io, out_audio, out_control, out_triggers, out_notes, block),
            NodeKind::Opaque(node) => node.render(io, out_audio, out_control, out_triggers, out_notes, block),
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        match self {
            NodeKind::Sine(node) => node.set_param(name, value),
            NodeKind::Gain(node) => node.set_param(name, value),
            NodeKind::Opaque(node) => node.set_param(name, value),
        }
    }
}

/// Declarative tier: a sine oscillator (source, `out("audio")`).
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

    fn render(
        &mut self,
        _io: &NodeIO,
        out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    ) {
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

/// Declarative tier: a gain stage (`in("audio")`, `out("audio")`).
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

    fn render(
        &mut self,
        io: &NodeIO,
        out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _block: RenderBlock,
    ) {
        for (i, sample) in out.iter_mut().enumerate() {
            *sample = io.audio_in.get(i).copied().unwrap_or(0.0) * self.gain;
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        if name == "gain" {
            self.gain = value;
        }
    }
}

/// Opaque tier: the euclidean generator as a node. Pure function of the block
/// and the tempo map: emits `out("triggers")` sample-accurately. No voice —
/// patch the triggers into whatever you like.
pub struct EuclideanGen {
    pub steps: u32,
    pub pulses_per_beat: u32,
    pub pattern: Vec<bool>,
}

impl AudioNode for EuclideanGen {
    fn latency(&self) -> u32 {
        0
    }

    fn render(
        &mut self,
        _io: &NodeIO,
        out_audio: &mut [f32],
        _control: &mut f32,
        out_triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    ) {
        let len = out_audio.len() as u64;
        let step_beats = 1.0 / self.pulses_per_beat.max(1) as f64;
        let b0 = block.tempo.beat_at(block.frame);
        let b1 = block.tempo.beat_at(block.frame + len.saturating_sub(1)) + step_beats;
        let s0 = (b0 / step_beats).floor() as i64;
        let s1 = (b1 / step_beats).ceil() as i64;
        for step in s0..s1 {
            if step < 0 {
                continue;
            }
            let step = step as u64;
            if !self.pattern[(step % self.steps.max(1) as u64) as usize] {
                continue;
            }
            let frame = block.tempo.frame_at(step as f64 * step_beats);
            if frame >= block.frame && frame < block.frame + len {
                out_triggers.push((frame - block.frame) as u32);
            }
        }
    }
}

/// Opaque tier: a scale/degree pitch generator. `in("trigger")` → `out("note")`:
/// each trigger advances through the configured degrees from the root.
pub struct ScaleGen {
    pub root: f32,
    pub degrees: Vec<i32>,
    pub note_len: u32,
    counter: usize,
}

impl ScaleGen {
    pub fn new(root: f32, degrees: Vec<i32>, note_len: u32) -> Self {
        ScaleGen {
            root,
            degrees,
            note_len,
            counter: 0,
        }
    }
}

impl AudioNode for ScaleGen {
    fn latency(&self) -> u32 {
        0
    }

    fn render(
        &mut self,
        io: &NodeIO,
        _audio: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        out_notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _block: RenderBlock,
    ) {
        if self.degrees.is_empty() {
            return;
        }
        for trigger in io.triggers_in {
            let degree = self.degrees[self.counter % self.degrees.len()];
            self.counter += 1;
            out_notes.push(NoteEvent {
                offset: *trigger,
                pitch: self.root + degree as f32,
                velocity: 1.0,
                duration: self.note_len,
            });
        }
    }
}

/// Opaque tier: a tone voice. `in("note")` → `out("audio")`: each note starts
/// a fixed-decay blip at the note's pitch, sample-accurately. Fixed-size
/// arrays only; voice *management* stays deferred (RESEARCH §14 risk 8).
pub struct ToneGen {
    pub gain: f32,
    pub blip_len: u32,
    blips: [Option<Blip>; MAX_BLIPS],
    /// (offset, len, freq) pending onsets, ascending offset order.
    pending: [(u32, u32, f32); MAX_PENDING],
    pending_head: usize,
    pending_count: usize,
}

const MAX_BLIPS: usize = 16;
const MAX_PENDING: usize = 8;

#[derive(Clone, Copy)]
struct Blip {
    phase: f64,
    inc: f64,
    remaining: u32,
    len: u32,
}

impl ToneGen {
    pub fn new(gain: f32, blip_len: u32) -> Self {
        ToneGen {
            gain,
            blip_len: blip_len.max(1),
            blips: [None; MAX_BLIPS],
            pending: [(0, 0, 0.0); MAX_PENDING],
            pending_head: 0,
            pending_count: 0,
        }
    }

    fn schedule(&mut self, offset: u32, len: u32, freq: f32) {
        if self.pending_count < MAX_PENDING {
            let at = self.pending_head + self.pending_count;
            self.pending[at] = (offset, len.max(1), freq);
            self.pending_count += 1;
        }
        // beyond the pool: drop (documented; voice management is deferred)
    }
}

impl AudioNode for ToneGen {
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
        block: RenderBlock,
    ) {
        for note in io.notes_in {
            // pitch = semitones from A4 (440 Hz); the representation is
            // continuous (musical-event model note).
            let freq = 440.0 * 2f32.powf(note.pitch / 12.0);
            self.schedule(note.offset, self.blip_len, freq);
        }
        for (i, sample) in out.iter_mut().enumerate() {
            // Drain pending onsets at or before this sample. The merged note
            // stream is sorted by offset (insert_sorted), so `== i` would
            // suffice; `<=` is defensive against stale entries (drop them).
            while self.pending_count > 0 && self.pending[self.pending_head].0 as usize <= i {
                let (offset, len, freq) = self.pending[self.pending_head];
                if offset as usize == i
                    && let Some(slot) = self.blips.iter_mut().find(|b| b.is_none()) {
                        *slot = Some(Blip {
                            phase: 0.0,
                            inc: TAU * freq as f64 / block.sample_rate as f64,
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
                    blip.phase += blip.inc;
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
        if self.pending_count == 0 {
            self.pending_head = 0;
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        match name {
            "gain" => self.gain = value,
            "blip_len" => self.blip_len = (value as u32).max(1),
            _ => {}
        }
    }
}

/// A per-stage delay line for PDC. Preallocated to [`MAX_PDC`] samples at node
/// creation (control side); the render path only changes the read offset — no
/// allocation, no buffer rebuild.
pub struct RingDelay {
    buf: Vec<f32>,
    pos: usize,
    delay: usize,
}

/// Maximum per-node latency the interpreter compensates (PDC) in Spike A.5.
pub const MAX_PDC: usize = 64;

impl RingDelay {
    pub fn with_capacity(cap: usize) -> Self {
        RingDelay {
            buf: vec![0.0; cap],
            pos: 0,
            delay: 0,
        }
    }

    pub fn set_delay(&mut self, delay: usize) {
        debug_assert!(delay <= self.buf.len(), "PDC delay exceeds MAX_PDC");
        self.delay = delay.min(self.buf.len());
    }

    pub fn delay(&self) -> usize {
        self.delay
    }

    /// Push `x`, return the value delayed by `delay` samples.
    pub fn process(&mut self, x: f32) -> f32 {
        if self.delay == 0 {
            return x;
        }
        let cap = self.buf.len();
        let read = (self.pos + cap - self.delay) % cap;
        let delayed = self.buf[read];
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % cap;
        delayed
    }
}

/// A patch cord: a typed connection between two ports, identified by
/// (node index, port index within that node's ports vec). For audio, `to.1`
/// is the *audio-in scratch index* (the port's position among the node's
/// audio-In ports) so the interpreter can fan in per channel; control/trigger/
/// note merge per kind into the node's single per-kind buffer (`to.1` unused).
#[derive(Clone, Copy)]
struct PatchCord {
    from: (usize, usize),
    to: (usize, usize),
    kind: SignalKind,
}

/// The graph value + interpreter: a patch bay over typed ports.
pub struct Graph {
    nodes: Vec<Node>,
    cords: Vec<PatchCord>,
    /// per-node audio output scratch (PDC-delayed).
    audio_out: Vec<Vec<f32>>,
    /// per-node control output (one f32 per block).
    control_out: Vec<f32>,
    /// per-node trigger / note outputs (fixed capacity).
    triggers_out: Vec<EventBuf<Trigger, CAP_EVENTS>>,
    notes_out: Vec<EventBuf<NoteEvent, CAP_EVENTS>>,
    /// per-node merged fan-in buffers.
    triggers_in: Vec<EventBuf<Trigger, MERGE_CAP>>,
    notes_in: Vec<EventBuf<NoteEvent, MERGE_CAP>>,
    /// per-node per-audio-In-port fan-in scratch, preallocated (a mixer's
    /// channels are separate inputs; single-input nodes have one buffer).
    audio_ins: Vec<Vec<Vec<f32>>>,
    /// per-node: indices of its audio-In ports (into the node's ports vec).
    audio_in_ports: Vec<Vec<usize>>,
    /// per-node cumulative audio latency + PDC delay lines.
    cum: Vec<u32>,
    delays: Vec<RingDelay>,
    next_id: u64,
    pub out_node: Option<NodeId>,
    /// render-loop scratch for control fan-in.
    control_scratch: f32,
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            nodes: Vec::new(),
            cords: Vec::new(),
            audio_out: Vec::new(),
            control_out: Vec::new(),
            triggers_out: Vec::new(),
            notes_out: Vec::new(),
            triggers_in: Vec::new(),
            notes_in: Vec::new(),
            audio_ins: Vec::new(),
            audio_in_ports: Vec::new(),
            cum: Vec::new(),
            delays: Vec::new(),
            next_id: 0,
            out_node: None,
            control_scratch: 0.0,
        }
    }

    pub fn add_node(&mut self, kind: NodeKind, ports: Vec<Port>) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        let audio_in_ports: Vec<usize> = ports
            .iter()
            .enumerate()
            .filter(|(_, p)| p.direction == Direction::In && p.kind == SignalKind::Audio)
            .map(|(i, _)| i)
            .collect();
        let audio_out_ports = ports
            .iter()
            .filter(|p| p.direction == Direction::Out && p.kind == SignalKind::Audio)
            .count();
        // Phase-1 shape (kimi review findings 2 + 8): many audio Ins (the
        // mixer's channels), at most one audio Out — enforced loudly.
        assert!(audio_in_ports.len() <= MAX_AUDIO_INS, "node declares {} audio inputs (max {MAX_AUDIO_INS})", audio_in_ports.len());
        assert!(audio_out_ports <= 1, "node declares {audio_out_ports} audio outputs (max 1 in Phase 1)");
        self.nodes.push(Node { id, kind, ports });
        self.audio_out.push(vec![0.0; BLOCK]);
        self.control_out.push(0.0);
        self.triggers_out.push(EventBuf::new());
        self.notes_out.push(EventBuf::new());
        self.triggers_in.push(EventBuf::new());
        self.notes_in.push(EventBuf::new());
        self.audio_ins.push(audio_in_ports.iter().map(|_| vec![0.0; BLOCK]).collect());
        self.audio_in_ports.push(audio_in_ports);
        self.cum.push(0);
        self.delays.push(RingDelay::with_capacity(MAX_PDC));
        // The master-bus fallback: only a node with an audio output may claim
        // it (the mixer claims it explicitly on mount; a trigger-only first
        // node must not become the bus — kimi review finding 9).
        if self.out_node.is_none() && audio_out_ports == 1 {
            self.out_node = Some(id);
        }
        id
    }

    pub fn set_out(&mut self, id: NodeId) {
        self.out_node = Some(id);
    }

    /// Connect two ports (type-checked, forward order). Multiple producers are
    /// allowed for audio (summed per input port) and events (merged); control
    /// inputs are single-driver in Phase 1. Audio `In` ports may be many per
    /// node (the mixer's channels); other kinds stay one-per-node.
    pub fn connect(&mut self, from: NodeId, from_port: &str, to: NodeId, to_port: &str) -> Result<(), String> {
        let fi = self.index_of(from).ok_or("connect: unknown 'from' node")?;
        let ti = self.index_of(to).ok_or("connect: unknown 'to' node")?;
        if fi >= ti {
            return Err("connect: patch cords must go forward (topological order)".into());
        }
        let from_port_idx = self.nodes[fi]
            .port_index(from_port)
            .ok_or_else(|| format!("no port '{from_port}' on node {fi}"))?;
        let to_port_idx = self.nodes[ti]
            .port_index(to_port)
            .ok_or_else(|| format!("no port '{to_port}' on node {ti}"))?;
        let out_port = self.nodes[fi].ports[from_port_idx];
        let in_port = self.nodes[ti].ports[to_port_idx];
        if out_port.direction != Direction::Out || in_port.direction != Direction::In {
            return Err(format!(
                "connect: '{from_port}' must be an Out port and '{to_port}' an In port"
            ));
        }
        if out_port.kind != in_port.kind {
            return Err(format!(
                "connect: signal kind mismatch — '{from_port}' is {:?}, '{to_port}' is {:?}",
                out_port.kind, in_port.kind
            ));
        }
        if in_port.kind == SignalKind::Control
            && self.cords.iter().any(|c| c.to.0 == ti && c.kind == SignalKind::Control)
        {
            return Err("connect: control inputs are single-driver in phase 1".into());
        }
        let to_scratch = if in_port.kind == SignalKind::Audio {
            self.audio_in_ports[ti]
                .iter()
                .position(|&p| p == to_port_idx)
                .expect("audio-in port must be registered")
        } else {
            0
        };
        self.cords.push(PatchCord {
            from: (fi, from_port_idx),
            to: (ti, to_scratch),
            kind: in_port.kind,
        });
        Ok(())
    }

    pub fn remove_node(&mut self, id: NodeId) -> Option<Node> {
        let idx = self.index_of(id)?;
        if self.out_node == Some(id) {
            self.out_node = None;
        }
        self.cords.retain(|c| c.from.0 != idx && c.to.0 != idx);
        for cord in self.cords.iter_mut() {
            if cord.from.0 > idx {
                cord.from.0 -= 1;
            }
            if cord.to.0 > idx {
                cord.to.0 -= 1;
            }
        }
        self.audio_out.remove(idx);
        self.control_out.remove(idx);
        self.triggers_out.remove(idx);
        self.notes_out.remove(idx);
        self.triggers_in.remove(idx);
        self.notes_in.remove(idx);
        self.audio_ins.remove(idx);
        self.audio_in_ports.remove(idx);
        self.cum.remove(idx);
        self.delays.remove(idx);
        Some(self.nodes.remove(idx))
    }

    /// Set a node parameter directly. **Unlogged** — the logged, replayable
    /// path is `Engine::set_param`; direct use bypasses the log (kimi review
    /// nit 11). Needed by the engine's apply path and node-level tests.
    pub fn set_param(&mut self, id: NodeId, name: &str, value: f32) {
        if let Some(i) = self.index_of(id) {
            self.nodes[i].kind.set_param(name, value);
        }
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    fn index_of(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }

    /// Render one block into `out`. Allocation-free: every buffer is
    /// preallocated or fixed-capacity; audio paths are PDC-aligned.
    pub fn render(&mut self, out: &mut [f32], block: RenderBlock) {
        let len = out.len();
        for i in 0..self.nodes.len() {
            self.triggers_out[i].clear();
            self.notes_out[i].clear();
            self.triggers_in[i].clear();
            self.notes_in[i].clear();
            for port in self.audio_ins[i].iter_mut() {
                port[..len].fill(0.0);
            }
        }

        // Cumulative audio latency per node (longest audio path) + PDC max.
        let mut max_cum = 0u32;
        for i in 0..self.nodes.len() {
            let base = self
                .cords
                .iter()
                .filter(|c| c.to.0 == i && c.kind == SignalKind::Audio)
                .map(|c| self.cum[c.from.0])
                .max()
                .unwrap_or(0);
            self.cum[i] = base + self.nodes[i].latency();
            max_cum = max_cum.max(self.cum[i]);
        }

        for i in 0..self.nodes.len() {
            // Gather inputs from producers (all earlier nodes). Audio fans in
            // per input port (a mixer's channels stay separate); control/
            // trigger/note merge into the node's single per-kind buffer.
            for cord in self.cords.iter().filter(|c| c.to.0 == i) {
                match cord.kind {
                    SignalKind::Audio => {
                        let src = &self.audio_out[cord.from.0][..len];
                        let dst = &mut self.audio_ins[i][cord.to.1][..len];
                        for (acc, s) in dst.iter_mut().zip(src) {
                            *acc += *s;
                        }
                    }
                    SignalKind::Control => {
                        self.control_scratch += self.control_out[cord.from.0];
                    }
                    SignalKind::Trigger => {
                        let src = &self.triggers_out[cord.from.0];
                        let dst = &mut self.triggers_in[i];
                        for t in src.as_slice() {
                            // keep the merged stream sorted by offset (fan-in)
                            dst.insert_sorted(*t);
                        }
                    }
                    SignalKind::Note => {
                        let src = &self.notes_out[cord.from.0];
                        let dst = &mut self.notes_in[i];
                        for n in src.as_slice() {
                            dst.insert_sorted(*n);
                        }
                    }
                }
            }

            let mut io_ins = [&[][..]; MAX_AUDIO_INS];
            let count = self.audio_in_ports[i].len().min(MAX_AUDIO_INS);
            for (k, _) in self.audio_in_ports[i].iter().enumerate().take(count) {
                io_ins[k] = &self.audio_ins[i][k][..len];
            }
            let io = NodeIO {
                audio_in: io_ins[0],
                audio_ins: io_ins,
                audio_in_count: count,
                control_in: self.control_scratch,
                triggers_in: self.triggers_in[i].as_slice(),
                notes_in: self.notes_in[i].as_slice(),
            };

            let (audio_before, audio_after) = self.audio_out.split_at_mut(i);
            let out_audio = &mut audio_after[0][..len];
            self.nodes[i].kind.render(
                &io,
                out_audio,
                &mut self.control_out[i],
                &mut self.triggers_out[i],
                &mut self.notes_out[i],
                block,
            );

            // PDC: delay this stage so audio aligns with the longest path.
            // Delay lines are preallocated (MAX_PDC); only the read offset
            // changes — no allocation on the render path.
            self.delays[i].set_delay((max_cum - self.cum[i]) as usize);
            let d = self.delays[i].delay();
            if d > 0 {
                for sample in out_audio.iter_mut() {
                    *sample = self.delays[i].process(*sample);
                }
            }
            let _ = audio_before;
            self.control_scratch = 0.0;
        }

        match self.out_node.and_then(|id| self.index_of(id)) {
            Some(i) => out.copy_from_slice(&self.audio_out[i][..len]),
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

    /// A 3-sample-latency opaque node: passes audio through, but *reports*
    /// latency so the interpreter must compensate.
    struct TestDelay {
        len: u32,
    }

    impl AudioNode for TestDelay {
        fn latency(&self) -> u32 {
            self.len
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

    fn tempo() -> TempoMap {
        TempoMap::new(48_000, 120.0, 4)
    }

    #[test]
    fn pdc_delays_the_fast_path() {
        let mut g = Graph::new();
        let sine = g.add_node(
            NodeKind::Sine(Sine::new(440.0)),
            vec![Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio }],
        );
        let delay = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 3 })),
            vec![
                Port { name: "audio", direction: Direction::In, kind: SignalKind::Audio },
                Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio },
            ],
        );
        g.connect(sine, "audio", delay, "audio").unwrap();
        g.set_out(delay);

        let mut out = [0.0f32; 64];
        let block = RenderBlock { frame: 0, sample_rate: 48_000, tempo: &tempo() };
        g.render(&mut out, block);

        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.0);
        let inc = (TAU * 440.0 / 48_000.0).sin() as f32;
        assert!((out[4] - inc).abs() < 1e-6, "out[4] = {}", out[4]);
    }

    #[test]
    fn connect_rejects_type_mismatch() {
        let mut g = Graph::new();
        let sine = g.add_node(
            NodeKind::Sine(Sine::new(440.0)),
            vec![Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio }],
        );
        let delay = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 0 })),
            vec![Port { name: "audio", direction: Direction::In, kind: SignalKind::Audio }],
        );
        let err = g.connect(sine, "audio", delay, "nope").unwrap_err();
        assert!(err.contains("no port 'nope'"), "got: {err}");
        // forward-order enforced
        let err = g.connect(delay, "audio", sine, "audio").unwrap_err();
        assert!(err.contains("forward"), "got: {err}");
    }
}
