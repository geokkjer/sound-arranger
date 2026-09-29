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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
///
/// `channels` is meaningful only for `SignalKind::Audio`: 1 = mono (the
/// default and every node today except the mixer's master), 2 = stereo. The
/// graph sizes a node's audio scalars by the channel count so a single typed
/// stereo port carries L/R. Control/trigger/note ports keep `channels = 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Port {
    pub name: &'static str,
    pub direction: Direction,
    pub kind: SignalKind,
    pub channels: u16,
}

impl Port {
    /// A mono audio port (the `channels` field default).
    pub fn audio(name: &'static str, direction: Direction) -> Self {
        Port {
            name,
            direction,
            kind: SignalKind::Audio,
            channels: 1,
        }
    }

    /// A stereo audio port (the master bus once the mixer is the sink).
    pub fn stereo_audio(name: &'static str, direction: Direction) -> Self {
        Port {
            name,
            direction,
            kind: SignalKind::Audio,
            channels: 2,
        }
    }

    /// The number of audio channels this port carries (1 for non-audio).
    pub fn channels(&self) -> usize {
        if self.kind == SignalKind::Audio {
            self.channels.max(1) as usize
        } else {
            1
        }
    }
}

/// What phase a render is in. `Timeline` is the normal pass. `Drain` is the
/// post-timeline flush of buffered tails — the EOF signal (FFmpeg's send-NULL):
/// self-driven sources mute themselves, so only buffered tails, and the
/// processing chain that carries them, reach the master. The mode is explicit so
/// drain participation never depends on a node's port shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Timeline,
    Drain,
}

#[derive(Debug, Clone, Copy)]
pub struct RenderBlock<'a> {
    pub frame: u64,
    pub sample_rate: u32,
    pub tempo: &'a TempoMap,
    /// Which phase this block is; a self-driven source emits silence in `Drain`.
    pub mode: RenderMode,
}

/// The audio inputs a node may read this block, in port order — a view onto the
/// graph's per-port fan-in buffers.
///
/// There is deliberately **no** compile-time maximum: a node's input width is
/// whatever it declared, so a node mounted with many inputs simply has a wider view
/// (the buffers are preallocated at `add_node`, so the render path still allocates
/// nothing).
#[derive(Clone, Copy)]
pub struct AudioInputs<'a> {
    ports: &'a [Vec<f32>],
    channels: &'a [usize],
    frames: usize,
}

impl<'a> AudioInputs<'a> {
    /// No audio inputs at all — a source node, or a render test's stub.
    pub const fn none() -> Self {
        AudioInputs {
            ports: &[],
            channels: &[],
            frames: 0,
        }
    }

    /// How many audio-In ports this node declared.
    pub fn count(&self) -> usize {
        self.ports.len()
    }

    /// The `port`-th audio input, or `&[]` when the node declared fewer ports. Its
    /// length is `port.channels() * frames` — interleaved when the port declares
    /// more than one channel.
    pub fn get(&self, port: usize) -> &'a [f32] {
        match (self.ports.get(port), self.channels.get(port)) {
            (Some(buf), Some(&ch)) => &buf[..ch * self.frames],
            _ => &[],
        }
    }
}

/// What a node may read this block: its inputs, merged from connected
/// producers by the interpreter. Audio inputs are per *port* (a mixer's
/// channels are separate inputs); control, trigger, and note stay single-port per
/// node in Phase 1.
pub struct NodeIO<'a> {
    /// The first audio input (convenience for single-input nodes; `&[]` when the
    /// node declares none). Its length is `channels * frames` — mono for a port that
    /// declares one channel, interleaved `L,R` for the master bus's stereo input.
    pub audio_in: &'a [f32],
    /// Per audio-In port, in port order; meaningful up to `audio_in_count`. Each
    /// slice is `port.channels() * frames` long (interleaved when above 1).
    pub audio_ins: AudioInputs<'a>,
    pub audio_in_count: usize,
    /// the node's audio output channel count (1 mono, 2 stereo) — the length
    /// of the `out_audio` slice a node receives is `channels * frames`. A node
    /// must write its audio for `channels` interleaved channels.
    pub audio_out_channels: usize,
    /// Frames in this block. A node with no audio ports receives an empty
    /// `out_audio` (there is no slice to measure), so portless nodes — a clock
    /// generator, a meter — read the block's width from here.
    pub frames: usize,
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

    /// Whether this node still holds buffered output that belongs to the piece
    /// (a delay/reverb tail, a decaying voice, an encoder's delay). The engine's
    /// drain phase keeps rendering while any mounted node reports a tail;
    /// stateless and pass-through nodes keep the default `false`.
    ///
    /// **Contract:** during a drain this must be *monotone non-increasing* (it is
    /// what makes the drain terminate), and a node reporting `false` must emit no
    /// non-zero audio in a drain block.
    fn has_tail(&self) -> bool {
        false
    }
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

    /// Whether this node holds buffered output (the drain gate's per-node term).
    pub fn has_tail(&self) -> bool {
        match &self.kind {
            NodeKind::Sine(node) => node.has_tail(),
            NodeKind::Gain(node) => node.has_tail(),
            NodeKind::Opaque(node) => node.has_tail(),
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
            NodeKind::Sine(node) => {
                node.render(io, out_audio, out_control, out_triggers, out_notes, block)
            }
            NodeKind::Gain(node) => {
                node.render(io, out_audio, out_control, out_triggers, out_notes, block)
            }
            NodeKind::Opaque(node) => {
                node.render(io, out_audio, out_control, out_triggers, out_notes, block)
            }
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
        if block.mode == RenderMode::Drain {
            out.fill(0.0); // a free-running source does not sound past the timeline
            return;
        }
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

/// The hard bound on **grid steps** one [`EuclideanGen`] block walks — the
/// euclidean node's counterpart to the clock-out node's `CLOCK_OUT_CAP`, and for
/// the same reason. The block's grid is a *pure function of the tempo*: the step
/// index of the block's last frame comes from `beat_at`, and at an absurd tempo
/// the `f64 → i64` cast of that index saturates at `i64::MAX`, so the walk
/// `[s0, s1)` is ~9.2e18 steps long **on the first block at frame 0**. That is a
/// permanent render-thread hang, not a panic, and no bound on the event buffer
/// prevents it: the cost is the iteration, and the body `continue`s on the
/// pattern check long before it would push.
///
/// Eight steps per frame is where the grid becomes finer than the block's own
/// resolution — `frame_at` rounds to whole frames, so from there on several
/// steps share every frame and their offsets are indistinguishable. At the
/// default four pulses per beat and a 512-frame block that is ~1.2e5 bpm
/// (a sixteenth note every 0.125 ms), and a quarter of it at sixteen pulses per
/// beat. Everything below that is walked whole, so the audible output is
/// unchanged for every tempo a person could mean; what is left is **counted**,
/// not walked ([`EuclideanGen::drops`]).
pub const EUCLIDEAN_STEP_CAP: u64 = BLOCK as u64 * 8;

/// Opaque tier: the euclidean generator as a node. Pure function of the block
/// and the tempo map: emits `out("triggers")` sample-accurately. No voice —
/// patch the triggers into whatever you like.
///
/// `pattern` is indexed by `step % steps.max(1)`, so the two are built together
/// and [`EuclideanGen::new`] asserts the length; a node hand-assembled with a
/// shorter pattern would index out of bounds **on the render thread**, which is
/// why the fields that make a node are set through a constructor.
pub struct EuclideanGen {
    pub steps: u32,
    pub pulses_per_beat: u32,
    pub pattern: Vec<bool>,
    /// Grid steps a block did not evaluate because its walk hit
    /// [`EUCLIDEAN_STEP_CAP`] (or because the trigger buffer was already full,
    /// which no further push could have used). Behind a shared atomic so the
    /// plugin can **publish** the live count while the render path only ever
    /// increments — a counter is not worth a lock in a block. The `euclidean`
    /// plugin publishes it under
    /// [`EUCLIDEAN_DROPS_KEY`](crate::plugins::euclidean::EUCLIDEAN_DROPS_KEY).
    drops: Arc<AtomicU64>,
}

impl EuclideanGen {
    /// Build a node with its own drop counter, for a caller that mounts it
    /// directly. Reads the count back with [`Self::drops`].
    pub fn new(steps: u32, pulses_per_beat: u32, pattern: Vec<bool>) -> Self {
        Self::with_drop_counter(steps, pulses_per_beat, pattern, Arc::new(AtomicU64::new(0)))
    }

    /// Build a node against an **existing** counter — the plugin's form, which
    /// publishes the same `Arc` into the context so a reader outside the graph
    /// sees the live count.
    pub fn with_drop_counter(
        steps: u32,
        pulses_per_beat: u32,
        pattern: Vec<bool>,
        drops: Arc<AtomicU64>,
    ) -> Self {
        assert_eq!(
            pattern.len() as u64,
            steps.max(1) as u64,
            "the euclidean pattern is indexed by `step % steps` — it must be exactly \
             `steps.max(1)` long",
        );
        EuclideanGen {
            steps,
            pulses_per_beat,
            pattern,
            drops,
        }
    }

    /// Grid steps this node did not evaluate, over every block it has rendered.
    /// Zero at any tempo a person could mean; nonzero means the block's grid was
    /// denser than [`EUCLIDEAN_STEP_CAP`] and part of it went unwalked — a loud
    /// bound, never a silent truncation.
    pub fn drops(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }
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
        if block.mode == RenderMode::Drain {
            return; // no new onsets past the timeline; only tails drain
        }
        let len = out_audio.len() as u64;
        let step_beats = 1.0 / self.pulses_per_beat.max(1) as f64;
        let b0 = block.tempo.beat_at(block.frame);
        let b1 = block.tempo.beat_at(block.frame + len.saturating_sub(1)) + step_beats;
        let s0 = (b0 / step_beats).floor() as i64;
        let s1 = (b1 / step_beats).ceil() as i64;
        // The block's **whole** grid, in closed form: how many steps a walk of
        // `[s0, s1)` would visit, known without visiting any of them. The
        // `f64 → i64` casts above saturate (they do not wrap), so at an absurd
        // tempo this is honestly "as many as there are" rather than a negative
        // or a wrapped one — which is what makes the count of what the walk
        // *skipped* exact below.
        let grid = s1.saturating_sub(s0).max(0) as u64;
        let stride = self.steps.max(1) as u64;
        let mut step = s0;
        let mut walked = 0u64;
        while step < s1 {
            if step >= 0 {
                let index = step as u64;
                if self.pattern[(index % stride) as usize] {
                    let frame = block.tempo.frame_at(index as f64 * step_beats);
                    if frame >= block.frame && frame < block.frame + len {
                        out_triggers.push((frame - block.frame) as u32);
                    }
                }
            }
            walked += 1;
            // Bounded both ways, and both are sound. The cap is the one that
            // matters: `grid` is the tempo's, and no tempo may hold the render
            // thread in this loop. The full buffer is the cheaper stop — a push
            // into it would be refused, so the emitted triggers are identical
            // either way, and stopping there keeps this node from ever being the
            // source of a silently refused push.
            if walked >= EUCLIDEAN_STEP_CAP || out_triggers.is_full() {
                break;
            }
            // Saturating rather than `+ 1`: `s1` is itself a saturating cast, so
            // the loop condition is what stops the walk, and an unchecked
            // increment past `i64::MAX` must not be how that happens.
            step = step.saturating_add(1);
        }
        // What the walk did not take, counted exactly — `drops` means "grid steps
        // this block did not evaluate", not "steps that would have sounded" (a
        // pattern position that is false emits nothing by design). A block whose
        // grid fits the cap adds nothing, so the count stays zero at any sane
        // tempo.
        let skipped = grid.saturating_sub(walked);
        if skipped > 0 {
            self.drops.fetch_add(skipped, Ordering::Relaxed);
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

    /// A blip still sounding is output the piece owns — the drain phase must
    /// render it out before the bounce ends. A merely *queued* onset is not a
    /// tail (and a malformed `offset >= frames` could never be drained by the
    /// per-sample loop, so counting it would pin the drain open).
    fn has_tail(&self) -> bool {
        self.blips.iter().any(|b| b.is_some())
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
                    && let Some(slot) = self.blips.iter_mut().find(|b| b.is_none())
                {
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

/// Maximum per-node latency the interpreter compensates (PDC), in **frames**.
/// 64 samples was far below real lookahead (~5 ms limiters ≈ 240 samples
/// @48 kHz); 4096 ≈ 85 ms covers limiters/reverb/PFX. A node whose latency
/// exceeds this is clamped silently (release) — raise the cap before an effect
/// with more latency lands.
pub const MAX_PDC: usize = 4096;
/// The widest channel layout a PDC delay line is sized for (the master bus is
/// stereo). A delay line holds `frames * channels` samples, so it is allocated
/// `MAX_PDC * this` once per node.
pub const MAX_PDC_CHANNELS: usize = 2;

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
    /// per-node audio output scratch (PDC-delayed), sized `channels * BLOCK`
    /// (channel-major; a mono node is `BLOCK`, the mixer's stereo master is
    /// `2 * BLOCK`).
    audio_out: Vec<Vec<f32>>,
    /// per-node audio output channel count (1 = mono, 2 = stereo).
    audio_out_ch: Vec<usize>,
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
    /// per-node: the channel count of each audio-In port, in the same order —
    /// precomputed at `add_node` so the render path can size a view without
    /// touching the node's ports.
    audio_in_channels: Vec<Vec<usize>>,
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
            audio_out_ch: Vec::new(),
            control_out: Vec::new(),
            triggers_out: Vec::new(),
            notes_out: Vec::new(),
            triggers_in: Vec::new(),
            notes_in: Vec::new(),
            audio_ins: Vec::new(),
            audio_in_ports: Vec::new(),
            audio_in_channels: Vec::new(),
            cum: Vec::new(),
            delays: Vec::new(),
            next_id: 0,
            out_node: None,
            control_scratch: 0.0,
        }
    }

    /// The audio channel count of a node's (single) audio-out port — 1 mono,
    /// 2 stereo. Non-audio nodes report 1. The count is also the width the PDC delay
    /// line is sized for ([`MAX_PDC_CHANNELS`]), so a wider declared output is a loud
    /// programming error rather than a node whose latency is silently clamped.
    fn node_out_channels(ports: &[Port]) -> usize {
        let channels = ports
            .iter()
            .find(|p| p.direction == Direction::Out && p.kind == SignalKind::Audio)
            .map(|p| p.channels())
            .unwrap_or(1);
        assert!(
            channels <= MAX_PDC_CHANNELS,
            "node declares a {channels}-channel audio output (max {MAX_PDC_CHANNELS}: \
             raise MAX_PDC_CHANNELS before a wider bus, so its PDC delay line fits)"
        );
        channels
    }

    /// One fan-in buffer per audio-In port, sized `channels * BLOCK` (a mono port is
    /// `BLOCK`; a stereo input is `2 * BLOCK`, interleaved). The declared port channel
    /// count is the seam: a cord copies as many interleaved channels as the
    /// **destination port** declares, and `patch` refuses a cord whose two ends disagree.
    fn audio_in_buffers(ports: &[Port], audio_in_ports: &[usize]) -> Vec<Vec<f32>> {
        audio_in_ports
            .iter()
            .map(|&i| vec![0.0; ports[i].channels() * BLOCK])
            .collect()
    }

    /// Audio channels on input port `k` of node `i` (the declared count).
    fn in_channels(&self, i: usize, k: usize) -> usize {
        self.nodes[i].ports[self.audio_in_ports[i][k]].channels()
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
        assert!(
            audio_out_ports <= 1,
            "node declares {audio_out_ports} audio outputs (max 1 in Phase 1)"
        );
        let out_ch = Self::node_out_channels(&ports);
        let ins = Self::audio_in_buffers(&ports, &audio_in_ports);
        let audio_in_channels: Vec<usize> = audio_in_ports
            .iter()
            .map(|&pi| ports[pi].channels())
            .collect();
        self.nodes.push(Node { id, kind, ports });
        self.audio_out.push(vec![0.0; out_ch * BLOCK]);
        self.audio_out_ch.push(out_ch);
        self.control_out.push(0.0);
        self.triggers_out.push(EventBuf::new());
        self.notes_out.push(EventBuf::new());
        self.triggers_in.push(EventBuf::new());
        self.notes_in.push(EventBuf::new());
        self.audio_ins.push(ins);
        self.audio_in_ports.push(audio_in_ports);
        self.audio_in_channels.push(audio_in_channels);
        self.cum.push(0);
        self.delays
            .push(RingDelay::with_capacity(MAX_PDC * MAX_PDC_CHANNELS));
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
    pub fn connect(
        &mut self,
        from: NodeId,
        from_port: &str,
        to: NodeId,
        to_port: &str,
    ) -> Result<(), String> {
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
            && self
                .cords
                .iter()
                .any(|c| c.to.0 == ti && c.kind == SignalKind::Control)
        {
            return Err("connect: control inputs are single-driver in phase 1".into());
        }
        // Audio cords must also match channel count: a cord carries one count, so a
        // stereo→mono (or mono→stereo) cord would silently read the interleaved buffer
        // as mono — refuse loud instead. (Stereo cords themselves are supported since
        // the master-bus slice: equal counts copy interleaved, sample for sample.)
        if in_port.kind == SignalKind::Audio && out_port.channels() != in_port.channels() {
            return Err(format!(
                "connect: audio channel mismatch — '{from_port}' is {}ch, '{to_port}' is {}ch",
                out_port.channels(),
                in_port.channels()
            ));
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

    /// Insert a node immediately before `pivot` in the topological order,
    /// shifting `pivot` and every later node (and their cord indices) up by one.
    /// Like [`add_node`](Self::add_node) but at a chosen position — the one way
    /// to add a node that must precede an existing node (e.g. an arranger source
    /// before the mixer, which is the last node) while satisfying the graph's
    /// forward-order `connect` rule. Does not claim the master bus (mirrors
    /// `add_node`'s fallback: only when `out_node` is still `None`).
    pub fn insert_before(
        &mut self,
        pivot: NodeId,
        kind: NodeKind,
        ports: Vec<Port>,
    ) -> Result<NodeId, String> {
        let idx = self
            .index_of(pivot)
            .ok_or("insert_before: unknown pivot node")?;
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
        assert!(
            audio_out_ports <= 1,
            "node declares {audio_out_ports} audio outputs (max 1 in Phase 1)"
        );
        let out_ch = Self::node_out_channels(&ports);
        let ins = Self::audio_in_buffers(&ports, &audio_in_ports);
        let audio_in_channels: Vec<usize> = audio_in_ports
            .iter()
            .map(|&pi| ports[pi].channels())
            .collect();
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(idx, Node { id, kind, ports });
        self.audio_out.insert(idx, vec![0.0; out_ch * BLOCK]);
        self.audio_out_ch.insert(idx, out_ch);
        self.control_out.insert(idx, 0.0);
        self.triggers_out.insert(idx, EventBuf::new());
        self.notes_out.insert(idx, EventBuf::new());
        self.triggers_in.insert(idx, EventBuf::new());
        self.notes_in.insert(idx, EventBuf::new());
        self.audio_ins.insert(idx, ins);
        self.audio_in_ports.insert(idx, audio_in_ports);
        self.audio_in_channels.insert(idx, audio_in_channels);
        self.cum.insert(idx, 0);
        self.delays
            .insert(idx, RingDelay::with_capacity(MAX_PDC * MAX_PDC_CHANNELS));
        // Nodes at/after the insertion point shifted up by one; re-index cords.
        for cord in self.cords.iter_mut() {
            if cord.from.0 >= idx {
                cord.from.0 += 1;
            }
            if cord.to.0 >= idx {
                cord.to.0 += 1;
            }
        }
        // The master-bus fallback: only claim it if no node has yet (the mixer
        // claims it on mount; an inserted arranger source must not steal it).
        if self.out_node.is_none() && audio_out_ports == 1 {
            self.out_node = Some(id);
        }
        Ok(id)
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
        self.audio_out_ch.remove(idx);
        self.control_out.remove(idx);
        self.triggers_out.remove(idx);
        self.notes_out.remove(idx);
        self.triggers_in.remove(idx);
        self.notes_in.remove(idx);
        self.audio_ins.remove(idx);
        self.audio_in_ports.remove(idx);
        self.audio_in_channels.remove(idx);
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

    /// The master output channel count (from the bus owner), 1 mono.
    pub fn out_channels(&self) -> usize {
        self.out_node
            .and_then(|id| self.index_of(id))
            .map(|i| self.audio_out_ch[i])
            .unwrap_or(1)
    }

    /// Whether any mounted node still holds buffered output — the drain gate.
    pub fn has_tail(&self) -> bool {
        self.nodes.iter().any(Node::has_tail)
    }

    /// The cumulative latency of the longest audio path into each node's output —
    /// the `cum` the PDC delay is derived from (`delay[i] = max_cum - cum[i]`).
    /// **Pure**: a function of the wiring and the nodes' declared latency, so it is
    /// correct *before* the first block. That matters because the drain's flush and
    /// an aligned offline render both need the transit before rendering
    /// ([`flush_frames`](Self::flush_frames)); `render_inner` keeps its own
    /// preallocated copy of the same numbers so the render path never allocates.
    fn cumulative_latency(&self) -> Vec<u32> {
        let mut cum = vec![0u32; self.nodes.len()];
        for i in 0..self.nodes.len() {
            let base = self
                .cords
                .iter()
                .filter(|c| c.to.0 == i && c.kind == SignalKind::Audio)
                .map(|c| cum[c.from.0])
                .max()
                .unwrap_or(0);
            cum[i] = base
                .saturating_add(self.nodes[i].latency())
                .min(MAX_PDC as u32);
        }
        cum
    }

    /// Frames still in flight after the last rendered block — the flush a drain owes
    /// so a rendered sample is never dropped, and the head an **aligned** offline
    /// render discards (the output lags the clock by this transit). The transit from
    /// node `i` is `T[i] = d_i + max over consumers c (latency[c] + T[c])` (`T = 0`
    /// at a node with no consumer), where `d_i` is the PDC delay applied to node
    /// `i`; `max_cum` alone under-counts chained paths. Computed in reverse node
    /// order (cords only go forward).
    pub fn flush_frames(&self) -> u32 {
        let n = self.nodes.len();
        let cum = self.cumulative_latency();
        let max_cum = cum.iter().copied().max().unwrap_or(0);
        let mut transit = vec![0u32; n];
        for i in (0..n).rev() {
            let d = max_cum.saturating_sub(cum[i]);
            let downstream = self
                .cords
                .iter()
                .filter(|c| c.from.0 == i && c.kind == SignalKind::Audio)
                .map(|c| self.nodes[c.to.0].latency().saturating_add(transit[c.to.0]))
                .max()
                .unwrap_or(0);
            transit[i] = d.saturating_add(downstream);
        }
        transit.iter().copied().max().unwrap_or(0)
    }

    /// Render one block into `out` (interleaved `channels * frames` samples).
    /// Allocation-free: every buffer is preallocated or fixed-capacity; audio
    /// paths are PDC-aligned. The block's [`RenderMode`] decides whether
    /// self-driven sources sound or mute.
    pub fn render(&mut self, out: &mut [f32], block: RenderBlock) {
        self.render_inner(out, block);
    }

    /// Render one **drain** block (a [`RenderMode::Drain`] block) and report
    /// whether a tail remains. Every node renders in index order; a self-driven
    /// source mutes itself on the mode, so only buffered tails — and the
    /// processing chain that carries them — reach the master.
    pub fn render_drain(&mut self, out: &mut [f32], block: RenderBlock) -> bool {
        debug_assert_eq!(
            block.mode,
            RenderMode::Drain,
            "render_drain needs a Drain block"
        );
        self.render_inner(out, block);
        self.has_tail()
    }

    fn render_inner(&mut self, out: &mut [f32], block: RenderBlock) {
        let channels = self.out_channels();
        // Frame count in this chunk. The master output is interleaved, so
        // `out.len() == channels * frames`. Node buffers are always mono (or
        // the node's own channel count) and use `frames`, never `out.len()`.
        let frames = out.len() / channels.max(1);
        for i in 0..self.nodes.len() {
            self.triggers_out[i].clear();
            self.notes_out[i].clear();
            self.triggers_in[i].clear();
            self.notes_in[i].clear();
            for k in 0..self.audio_ins[i].len() {
                let n = self.in_channels(i, k) * frames;
                self.audio_ins[i][k][..n].fill(0.0);
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
            // Clamped at `MAX_PDC`: the cap is the delay line's contract, so a chain
            // whose latencies add up beyond it saturates here (loud in debug, silent in
            // release) instead of asking for a delay the preallocated ring cannot hold.
            self.cum[i] = (base + self.nodes[i].latency()).min(MAX_PDC as u32);
            max_cum = max_cum.max(self.cum[i]);
        }

        for i in 0..self.nodes.len() {
            // Gather inputs from producers (all earlier nodes). Audio fans in
            // per input port (a mixer's channels stay separate); control/
            // trigger/note merge into the node's single per-kind buffer.
            for cord in self.cords.iter().filter(|c| c.to.0 == i) {
                match cord.kind {
                    SignalKind::Audio => {
                        // Audio is copied **channel-aware**: both ends of a cord declare
                        // their channel count and `patch` refuses a mismatch, so a stereo
                        // bus (the mixer's interleaved L,R) reaches a stereo consumer
                        // intact while the mono path stays a flat sum. Fan-in adds
                        // like with like, per interleaved sample.
                        let ch = self.in_channels(i, cord.to.1);
                        let src = &self.audio_out[cord.from.0][..ch * frames];
                        let dst = &mut self.audio_ins[i][cord.to.1][..ch * frames];
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

            let ins = AudioInputs {
                ports: &self.audio_ins[i],
                channels: &self.audio_in_channels[i],
                frames,
            };
            let out_ch = self.audio_out_ch[i];
            let io = NodeIO {
                audio_in: ins.get(0),
                audio_ins: ins,
                audio_in_count: ins.count(),
                audio_out_channels: out_ch,
                frames,
                control_in: self.control_scratch,
                triggers_in: self.triggers_in[i].as_slice(),
                notes_in: self.notes_in[i].as_slice(),
            };

            let (audio_before, audio_after) = self.audio_out.split_at_mut(i);
            let out_audio = &mut audio_after[0][..out_ch * frames];
            self.nodes[i].kind.render(
                &io,
                out_audio,
                &mut self.control_out[i],
                &mut self.triggers_out[i],
                &mut self.notes_out[i],
                block,
            );

            // PDC: delay this stage so audio aligns with the longest path.
            // Delay lines are preallocated (MAX_PDC * stereo); only the read
            // offset changes — no allocation on the render path. The delay is
            // declared in **frames** and the line runs over the interleaved buffer,
            // so a multichannel node's line advances by `frames * channels`
            // samples: delaying by the frame count alone halves a stereo node's
            // delay (240 samples is 240 mono frames but only 120 stereo frames).
            let pdc = (max_cum.saturating_sub(self.cum[i])).min(MAX_PDC as u32) as usize;
            let channels = self.audio_out_ch[i].max(1);
            // `pdc <= MAX_PDC` and `channels <= MAX_PDC_CHANNELS` (asserted at node
            // creation), so `pdc * channels` always fits the preallocated ring.
            self.delays[i].set_delay(pdc * channels);
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
            // `out` is interleaved `channels * frames`; the bus owner's buffer
            // holds the same channel-major layout, so a straight copy works.
            Some(i) => out.copy_from_slice(&self.audio_out[i][..out.len()]),
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
            vec![Port {
                name: "audio",
                direction: Direction::Out,
                kind: SignalKind::Audio,
                channels: 1,
            }],
        );
        let delay = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 3 })),
            vec![
                Port {
                    name: "audio",
                    direction: Direction::In,
                    kind: SignalKind::Audio,
                    channels: 1,
                },
                Port {
                    name: "audio",
                    direction: Direction::Out,
                    kind: SignalKind::Audio,
                    channels: 1,
                },
            ],
        );
        g.connect(sine, "audio", delay, "audio").unwrap();
        g.set_out(delay);

        let mut out = [0.0f32; 64];
        let block = RenderBlock {
            frame: 0,
            sample_rate: 48_000,
            tempo: &tempo(),
            mode: RenderMode::Timeline,
        };
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
            vec![Port {
                name: "audio",
                direction: Direction::Out,
                kind: SignalKind::Audio,
                channels: 1,
            }],
        );
        let delay = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 0 })),
            vec![Port {
                name: "audio",
                direction: Direction::In,
                kind: SignalKind::Audio,
                channels: 1,
            }],
        );
        let err = g.connect(sine, "audio", delay, "nope").unwrap_err();
        assert!(err.contains("no port 'nope'"), "got: {err}");
        // forward-order enforced
        let err = g.connect(delay, "audio", sine, "audio").unwrap_err();
        assert!(err.contains("forward"), "got: {err}");
    }

    #[test]
    fn insert_before_lets_a_new_source_precede_the_sink() {
        let mut g = Graph::new();
        // a sink first (the mixer's role in the arranger wiring)
        let sink = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 0 })),
            vec![
                Port {
                    name: "audio",
                    direction: Direction::In,
                    kind: SignalKind::Audio,
                    channels: 1,
                },
                Port {
                    name: "audio",
                    direction: Direction::Out,
                    kind: SignalKind::Audio,
                    channels: 1,
                },
            ],
        );
        g.set_out(sink);
        // a source inserted *before* the sink — must satisfy forward order.
        let src = g
            .insert_before(
                sink,
                NodeKind::Sine(Sine::new(440.0)),
                vec![Port {
                    name: "audio",
                    direction: Direction::Out,
                    kind: SignalKind::Audio,
                    channels: 1,
                }],
            )
            .unwrap();
        assert_eq!(src, NodeId(1), "inserted node is a fresh id");
        // the inserted source precedes the sink in the topological order.
        g.connect(src, "audio", sink, "audio").unwrap();
        // out_node must still be the sink (the mixer may never be stolen).
        assert_eq!(g.out_node, Some(sink));

        let mut out = [0.0f32; 64];
        let block = RenderBlock {
            frame: 0,
            sample_rate: 48_000,
            tempo: &tempo(),
            mode: RenderMode::Timeline,
        };
        g.render(&mut out, block);
        assert!(
            out[..32].iter().any(|s| s.abs() > 1e-6),
            "the inserted source must reach the sink"
        );
    }

    #[test]
    fn insert_before_reindexes_cords_after_the_insertion_point() {
        let mut g = Graph::new();
        let a = g.add_node(
            NodeKind::Sine(Sine::new(440.0)),
            vec![Port {
                name: "audio",
                direction: Direction::Out,
                kind: SignalKind::Audio,
                channels: 1,
            }],
        );
        let b = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 0 })),
            vec![
                Port {
                    name: "audio",
                    direction: Direction::In,
                    kind: SignalKind::Audio,
                    channels: 1,
                },
                Port {
                    name: "audio",
                    direction: Direction::Out,
                    kind: SignalKind::Audio,
                    channels: 1,
                },
            ],
        );
        // a cord a -> b establishes b as the later node.
        g.connect(a, "audio", b, "audio").unwrap();
        // insert a node BEFORE a (index 0); a's index and the a->b cord must
        // both shift up by one without breaking the connect.
        let x = g
            .insert_before(
                a,
                NodeKind::Sine(Sine::new(220.0)),
                vec![Port {
                    name: "audio",
                    direction: Direction::Out,
                    kind: SignalKind::Audio,
                    channels: 1,
                }],
            )
            .unwrap();
        assert_eq!(x, NodeId(2), "inserted node is a fresh id");
        // the pre-existing a->b cord is still forward-ordered after the shift.
        g.connect(a, "audio", b, "audio").unwrap();
    }

    /// A node with an unbounded tail: emits 1.0 forever and declares a tail.
    struct ForeverTail;

    impl AudioNode for ForeverTail {
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
            _block: RenderBlock,
        ) {
            out.fill(1.0);
        }
        fn has_tail(&self) -> bool {
            true
        }
    }

    /// A two-input summer (a processing node, to carry a tail across the gate).
    struct Sum;

    impl AudioNode for Sum {
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
            out.fill(0.0);
            for k in 0..io.audio_in_count {
                for (o, i) in out.iter_mut().zip(io.audio_ins.get(k)) {
                    *o += *i;
                }
            }
        }
    }

    #[test]
    fn render_drain_mutes_a_source_but_keeps_a_tail() {
        let mut g = Graph::new();
        let sine = g.add_node(
            NodeKind::Sine(Sine::new(440.0)),
            vec![Port::audio("audio", Direction::Out)],
        );
        let tail = g.add_node(
            NodeKind::Opaque(Box::new(ForeverTail)),
            vec![Port::audio("audio", Direction::Out)],
        );
        let mix = g.add_node(
            NodeKind::Opaque(Box::new(Sum)),
            vec![
                Port::audio("a", Direction::In),
                Port::audio("b", Direction::In),
                Port::audio("audio", Direction::Out),
            ],
        );
        g.connect(sine, "audio", mix, "a").unwrap();
        g.connect(tail, "audio", mix, "b").unwrap();
        g.set_out(mix);

        // A normal render sums both: the sine is audible alongside the tail.
        let mut normal = [0.0f32; 32];
        g.render(
            &mut normal,
            RenderBlock {
                frame: 0,
                sample_rate: 48_000,
                tempo: &tempo(),
                mode: RenderMode::Timeline,
            },
        );
        assert!(
            normal.iter().any(|s| (*s - 1.0).abs() > 1e-6),
            "the sine contributes in a normal render"
        );

        // A drain render mutes the free-running source: only the tail's 1.0.
        let mut drain = [0.0f32; 32];
        g.render_drain(
            &mut drain,
            RenderBlock {
                frame: 0,
                sample_rate: 48_000,
                tempo: &tempo(),
                mode: RenderMode::Drain,
            },
        );
        assert_eq!(
            drain, [1.0f32; 32],
            "the source is muted; only the tail sounds"
        );
    }

    #[test]
    fn has_tail_is_false_without_buffered_output() {
        // A sine is a source, not a tail: the drain gate must not treat it as
        // something to render out.
        let mut g = Graph::new();
        let sine = g.add_node(
            NodeKind::Sine(Sine::new(440.0)),
            vec![Port::audio("audio", Direction::Out)],
        );
        g.set_out(sine);
        assert!(!g.has_tail());
    }

    #[test]
    fn flush_frames_covers_the_chained_pdc_transit() {
        let mut g = Graph::new();
        let src = g.add_node(
            NodeKind::Opaque(Box::new(ForeverTail)),
            vec![Port::audio("audio", Direction::Out)],
        );
        let sink = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 3 })),
            vec![
                Port::audio("audio", Direction::In),
                Port::audio("audio", Direction::Out),
            ],
        );
        g.connect(src, "audio", sink, "audio").unwrap();
        g.set_out(sink);
        let mut out = [0.0f32; 16];
        g.render(
            &mut out,
            RenderBlock {
                frame: 0,
                sample_rate: 48_000,
                tempo: &tempo(),
                mode: RenderMode::Timeline,
            },
        );
        // The source's PDC delay (3) *plus* the sink's own latency (3): the
        // cumulative latency alone (3) under-flushes the chained path.
        assert_eq!(g.flush_frames(), 6);
    }

    /// A constant stereo source: `L = +0.25`, `R = -0.25` on every frame — distinct
    /// enough that a channel swap, a lost channel or a misread interleave is visible.
    struct StereoSource;

    impl AudioNode for StereoSource {
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
            _block: RenderBlock,
        ) {
            for frame in out.as_chunks_mut::<2>().0 {
                frame[0] = 0.25;
                frame[1] = -0.25;
            }
        }
    }

    /// A stereo pass-through that **marks each channel**, so the test can tell which
    /// interleaved sample arrived where: `L + 1`, `R + 10`.
    struct StereoMark;

    impl AudioNode for StereoMark {
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
            for (i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let l = io.audio_in.get(i * 2).copied().unwrap_or(0.0);
                let r = io.audio_in.get(i * 2 + 1).copied().unwrap_or(0.0);
                frame[0] = l + 1.0;
                frame[1] = r + 10.0;
            }
        }
    }

    /// **A stereo cord carries both channels, frame-aligned.** Before this slice a
    /// cord copied `frames` samples from the producer's flat buffer, so a stereo bus
    /// reached a consumer as the first *half* of its interleaved block — `L0,R0,L1,…`
    /// read as if they were consecutive mono samples (a 2× speed garbling), which is
    /// why the master bus could not exist.
    #[test]
    fn a_stereo_cord_carries_both_channels_frame_aligned() {
        let mut g = Graph::new();
        let src = g.add_node(
            NodeKind::Opaque(Box::new(StereoSource)),
            vec![Port::stereo_audio("audio", Direction::Out)],
        );
        let mark = g.add_node(
            NodeKind::Opaque(Box::new(StereoMark)),
            vec![
                Port::stereo_audio("audio", Direction::In),
                Port::stereo_audio("audio", Direction::Out),
            ],
        );
        g.connect(src, "audio", mark, "audio").unwrap();
        g.set_out(mark);
        assert_eq!(g.out_channels(), 2);

        let mut out = [0.0f32; 64];
        g.render(
            &mut out,
            RenderBlock {
                frame: 0,
                sample_rate: 48_000,
                tempo: &tempo(),
                mode: RenderMode::Timeline,
            },
        );
        for (i, frame) in out.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(
                (frame[0], frame[1]),
                (1.25, 9.75),
                "every frame is L,R in order (frame {i})"
            );
        }
    }

    /// **A chain past `MAX_PDC` saturates rather than panicking.** Three stereo nodes
    /// at 2731 frames of declared latency sum to 8193 > `MAX_PDC`: the cumulative
    /// latency is clamped at the cap, so every applied PDC delay still fits its
    /// preallocated `MAX_PDC * 2` ring (the gate reproduced a debug panic here before
    /// the clamp; the frame-based `* channels` multiplier is what halved the headroom).
    #[test]
    fn a_chain_beyond_max_pdc_saturates_instead_of_panicking() {
        let mut g = Graph::new();
        let mut prev = g.add_node(
            NodeKind::Sine(Sine::new(440.0)),
            vec![Port::stereo_audio("audio", Direction::Out)],
        );
        for _ in 0..3 {
            let delay = g.add_node(
                NodeKind::Opaque(Box::new(TestDelay { len: 2_731 })),
                vec![
                    Port::stereo_audio("in", Direction::In),
                    Port::stereo_audio("audio", Direction::Out),
                ],
            );
            g.connect(prev, "audio", delay, "in").unwrap();
            prev = delay;
        }
        g.set_out(prev);
        let mut out = vec![0.0f32; 128 * 2];
        g.render(
            &mut out,
            RenderBlock {
                frame: 0,
                sample_rate: 48_000,
                tempo: &tempo(),
                mode: RenderMode::Timeline,
            },
        );
        for (i, delay) in g.delays.iter().enumerate() {
            assert!(
                delay.delay() <= MAX_PDC * MAX_PDC_CHANNELS,
                "node {i}: applied delay {} exceeds its ring",
                delay.delay()
            );
        }
        assert!(g.flush_frames() > 0, "the chain's transit is accounted for");
    }

    /// A cord whose ends disagree about their channel count is refused, in the graph
    /// and before the log (`Engine::validate_patch`), never silently misread.
    #[test]
    fn a_channel_mismatched_cord_is_refused() {
        let mut g = Graph::new();
        let src = g.add_node(
            NodeKind::Opaque(Box::new(StereoSource)),
            vec![Port::stereo_audio("audio", Direction::Out)],
        );
        let mono = g.add_node(
            NodeKind::Opaque(Box::new(TestDelay { len: 0 })),
            vec![
                Port::audio("audio", Direction::In),
                Port::audio("audio", Direction::Out),
            ],
        );
        let err = g.connect(src, "audio", mono, "audio").unwrap_err();
        assert!(
            err.contains("channel mismatch") && err.contains("2ch") && err.contains("1ch"),
            "the refusal names both counts: {err}"
        );
    }

    /// Render one block through a graph holding one euclidean node, and hand back
    /// the drop counter the node shares. The node is a trigger-only source, so the
    /// graph has no bus owner and the block renders as silence — the walk is the
    /// whole subject.
    fn euclid_walk_block(
        map: &TempoMap,
        frame: u64,
        pattern: Vec<bool>,
        steps: u32,
    ) -> Arc<AtomicU64> {
        let drops = Arc::new(AtomicU64::new(0));
        let mut g = Graph::new();
        g.add_node(
            NodeKind::Opaque(Box::new(EuclideanGen::with_drop_counter(
                steps,
                4,
                pattern,
                drops.clone(),
            ))),
            vec![],
        );
        let mut out = [0.0f32; BLOCK];
        g.render(
            &mut out,
            RenderBlock {
                frame,
                sample_rate: 48_000,
                tempo: map,
                mode: RenderMode::Timeline,
            },
        );
        drops
    }

    /// The euclidean walk was the length of the block's **grid**, and the grid is
    /// a pure function of the tempo: at 1e300 bpm the block's last beat is
    /// 1.77e296, its cast to a step index saturates at `i64::MAX`, and the walk
    /// from step 0 was 9.2e18 steps — on the very first block, at frame 0. The
    /// body `continue`s on the pattern check long before it would emit, so
    /// `frame_at`'s saturation could not save it: the cost was the iteration and
    /// `render` never returned. The walk is now capped and the rest of the grid
    /// is *counted*, exactly.
    #[test]
    fn an_absurd_tempo_does_not_wedge_the_step_walk() {
        // A pattern with no pulse in it, so the walk's only possible stop is the
        // cap: the counts below are about the walk and about nothing else.
        let map = TempoMap::new(48_000, 1e300, 4);
        let drops = euclid_walk_block(&map, 0, vec![false; 8], 8);
        assert_eq!(
            drops.load(Ordering::Relaxed),
            i64::MAX as u64 - EUCLIDEAN_STEP_CAP,
            "the block walked exactly the cap and counted the other 9.2e18"
        );

        // A grid that fits the cap is walked whole and counts nothing, so the bound
        // changes no audible output at any tempo a person could mean: 120 bpm with
        // four pulses per beat is 4 steps per block.
        let map = TempoMap::new(48_000, 120.0, 4);
        let drops = euclid_walk_block(&map, 0, vec![false; 8], 8);
        assert_eq!(
            drops.load(Ordering::Relaxed),
            0,
            "a sane grid drops nothing"
        );
    }

    /// The same defect one order of magnitude below the saturation: a grid of
    /// 1e9 bpm holds 709 724 steps in one 512-frame block, and uncapped the
    /// block walked every one of them. The walk stops at the cap and the rest of
    /// the grid is counted **exactly** — `walked + counted == the block's whole
    /// grid` — with the trigger buffer out of the picture (a pattern with no
    /// pulse in it, so nothing could fill the buffer and stop the walk early).
    #[test]
    fn a_dense_grid_does_not_run_away_in_one_block() {
        let map = TempoMap::new(48_000, 1.0e9, 4);
        let drops = euclid_walk_block(&map, 0, vec![false; 8], 8);
        // 511 frames at 1e9 bpm is 177 430.556 beats, +1/4 beat, /1/4 beat per step
        // → 709 724 steps in the block's grid.
        assert_eq!(drops.load(Ordering::Relaxed), 709_724 - EUCLIDEAN_STEP_CAP);
    }
}
