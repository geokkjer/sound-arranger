//! The MIDI clock-out plugin (the midi-clock-out note, 2026-09-29): the
//! recorder profile's *sender* — a follower is driven with MIDI clock (24 PPQN)
//! and transport (`Start` / `Stop` / `Continue`), sample-accurately against the
//! same tempo map the scheduler reads.
//!
//! The clock is the core's, read-only: the node computes each tick's frame
//! **statelessly from the block's own frame range** — no continuity is tracked
//! between blocks, and no transport is inferred from a frame jump. A seek
//! rebuilds the session, so nothing is sent across a rebuild; the next `play`
//! re-syncs (the conservative decision the note takes).
//!
//! Device ownership stays with the host: the node holds whatever sink the host
//! provided under the [`MIDI_OUT_KEY`] context key, and **both services are
//! optional** — a session mounts on a machine with no device, sends nothing,
//! and renders byte-identically (the takes-slice purity rule: the declaration
//! is state, the sending is a device-bound side effect, never replayed).

use std::sync::{Arc, Mutex};

use super::{Disposer, ExternalEvent, MidiSink, ParamDef, Plugin, PluginApi};
use crate::clock::TempoMap;
use crate::graph::{
    CAP_EVENTS, EventBuf, NodeIO, NodeId, NodeKind, NoteEvent, Port, RenderBlock, Trigger,
};

/// The declared port surface: none — the plugin produces no audio, triggers,
/// or notes, only outbound MIDI through the host's sink.
pub const CLOCK_OUT_PORTS: &[Port] = &[];

/// MIDI clock is 24 pulses per quarter note — the format's constant, not a
/// setting.
pub const TICKS_PER_BEAT: u64 = 24;

/// The context key the host provides the MIDI output sink under
/// ([`SharedMidiSink`]). Optional: absent means "no device", and the plugin
/// still mounts.
pub const MIDI_OUT_KEY: &str = "midi.out";

/// The context key the host provides the transport log under
/// ([`SharedTransportLog`]). Optional for the same reason.
pub const TRANSPORT_KEY: &str = "transport";

/// The shared-sink shape the host provides: a mutex around the sink so the
/// control side can swap or inspect it while the render path sends. The real
/// device sink will make `send` a fast queue push, and with a single sender
/// the lock is uncontended.
pub type SharedMidiSink = Arc<Mutex<Box<dyn MidiSink>>>;

/// The shared transport log the host feeds ([`TransportLog`]).
pub type SharedTransportLog = Arc<TransportLog>;

/// A MIDI transport command (the note's scope: `Start`, `Stop`, `Continue` —
/// Song Position Pointer is explicitly out of this slice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Start,
    Stop,
    Continue,
}

/// The transport commands the **host** feeds, keyed by the frame each takes
/// effect. This is the declared seam for slice B's wiring: `play` logs a
/// `Start` (or `Continue` after a pause), `stop` logs a `Stop` — the log is
/// fed from the host's command path, never from the render path, so the node
/// only ever drains it.
///
/// The name is deliberate: it is a *log of transport commands*, not a message
/// queue — entries carry the absolute frame the command takes effect, and
/// draining takes everything due by `block_end`, so a late feed flushes into
/// the next block rather than being lost.
#[derive(Default)]
pub struct TransportLog {
    queue: Mutex<Vec<(u64, Transport)>>,
}

impl TransportLog {
    pub fn new() -> Self {
        TransportLog {
            queue: Mutex::new(Vec::new()),
        }
    }

    /// Log a transport command taking effect at an absolute frame. The queue
    /// stays sorted by frame — a command can be logged after its frame has
    /// passed (the host's control path always trails the render clock), and
    /// `take_due`'s front-drain assumes order.
    pub fn push(&self, frame: u64, transport: Transport) {
        let mut queue = self
            .queue
            .lock()
            .expect("the transport log is not poisoned");
        let at = queue.partition_point(|&(f, _)| f <= frame);
        queue.insert(at, (frame, transport));
    }

    /// Take every entry whose frame is `< block_end` — due in this block *or
    /// earlier*, so a command logged after its frame has passed still flushes
    /// into the very next block instead of sitting in the queue forever. The
    /// node reuses a preallocated buffer, so draining allocates nothing.
    pub fn take_due(&self, block_end: u64, out: &mut Vec<(u64, Transport)>) {
        let mut queue = self
            .queue
            .lock()
            .expect("the transport log is not poisoned");
        let at = queue.partition_point(|&(f, _)| f < block_end);
        out.extend(queue.drain(..at));
        drop(queue);
    }
}

/// The hard bound on events emitted per block. A [`BLOCK`](crate::graph::BLOCK)-
/// frame block cannot carry more than a handful of ticks at any sane tempo, but
/// an absurd one could: when a block would exceed the bound, the node emits
/// what fits and counts the rest. Dropping clock ticks is bad — but allocating
/// on the render path is worse, so the bound must be **visible**: the overflow
/// counter is a loud failure, not silent growth.
const CLOCK_OUT_CAP: usize = 64;

/// The clock-out node: computes the MIDI clock ticks and transport events due
/// in each block and sends them through the host's sink. It produces no audio,
/// declares no ports, and tracks no state between blocks.
pub struct ClockOutNode {
    /// The host's sink, if any. `None` is a first-class state — a session
    /// without the device — not an error.
    sink: Option<SharedMidiSink>,
    transport: Option<SharedTransportLog>,
    /// Per-block event scratch, allocated once at construction and `clear`ed
    /// each block: the render path never grows it (see [`CLOCK_OUT_CAP`]).
    scratch: Vec<ExternalEvent>,
    /// Transport drained from the log this block — also preallocated, for the
    /// same reason.
    transport_scratch: Vec<(u64, Transport)>,
    /// Events that did not fit the scratch: a loud bound, read by tests and,
    /// later, the host's health surface.
    overflows: u64,
}

impl ClockOutNode {
    /// Construct with the services the plugin found in the context at apply
    /// time — either may be `None`.
    pub fn new(sink: Option<SharedMidiSink>, transport: Option<SharedTransportLog>) -> Self {
        ClockOutNode {
            sink,
            transport,
            scratch: Vec::with_capacity(CLOCK_OUT_CAP),
            transport_scratch: Vec::with_capacity(CLOCK_OUT_CAP),
            overflows: 0,
        }
    }

    /// How many events were dropped because a block exceeded
    /// [`CLOCK_OUT_CAP`] — a mount this sane never sees a nonzero count.
    pub fn overflows(&self) -> u64 {
        self.overflows
    }

    /// Push into the scratch, or count the overflow — never grow.
    fn push(&mut self, event: ExternalEvent) {
        if self.scratch.len() < self.scratch.capacity() {
            self.scratch.push(event);
        } else {
            self.overflows += 1;
        }
    }

    /// The absolute frame of tick `n` (tick *n* sits at beat `n / 24`).
    fn tick_frame(map: &TempoMap, n: u64) -> u64 {
        map.frame_at(n as f64 / TICKS_PER_BEAT as f64)
    }

    /// First tick index whose frame is `>= frame`, corrected through
    /// `frame_at` so f64 noise in `beat_at` cannot shift a tick off its exact
    /// frame: the candidate from the beat domain is walked onto the true
    /// answer, which the frame domain defines.
    fn first_tick_at_or_after(map: &TempoMap, frame: u64) -> u64 {
        let mut n = (map.beat_at(frame) * TICKS_PER_BEAT as f64).floor() as u64;
        while Self::tick_frame(map, n) < frame {
            n += 1;
        }
        while n > 0 && Self::tick_frame(map, n - 1) >= frame {
            n -= 1;
        }
        n
    }
}

impl crate::graph::AudioNode for ClockOutNode {
    fn latency(&self) -> u32 {
        0
    }

    fn render(
        &mut self,
        io: &NodeIO,
        _out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    ) {
        self.scratch.clear();
        self.transport_scratch.clear();
        let block_end = block.frame + io.frames as u64;

        // Transport first: the log is the host's record of when each command
        // takes effect; a command logged after its frame has passed still
        // flushes here (take-due, not take-exact). A late entry's offset
        // saturates at 0 — the earliest position in the block we can still
        // mean.
        if let Some(log) = &self.transport {
            log.take_due(block_end, &mut self.transport_scratch);
            // Index loop: borrowing the scratch immutably while `push` needs
            // `&mut self` fights the borrow checker, and `std::mem::take`
            // would drop the preallocated capacity — an allocation per block
            // on the render path, exactly what the scratch exists to avoid.
            for i in 0..self.transport_scratch.len() {
                let (frame, transport) = self.transport_scratch[i];
                let offset = frame.saturating_sub(block.frame) as u32;
                let event = match transport {
                    Transport::Start => ExternalEvent::Start { offset },
                    Transport::Stop => ExternalEvent::Stop { offset },
                    Transport::Continue => ExternalEvent::Continue { offset },
                };
                self.push(event);
            }
        }

        // Ticks, computed statelessly from the block's own frame range: the
        // tick indices whose beat lands inside `[block.frame, block_end)`.
        // Exact across a tempo change inside the block, because every tick
        // frame comes from the tempo map's `frame_at`, not from a rate carried
        // between blocks.
        let mut n = Self::first_tick_at_or_after(block.tempo, block.frame);
        loop {
            let frame = Self::tick_frame(block.tempo, n);
            if frame >= block_end {
                break;
            }
            self.push(ExternalEvent::Clock {
                offset: (frame - block.frame) as u32,
            });
            n += 1;
        }

        // One send per block; the offsets carry the exact sub-block positions.
        // The lock is held only for the call, and with a single sender it is
        // uncontended — the real device sink will make this a fast queue push.
        if !self.scratch.is_empty()
            && let Some(sink) = &self.sink
        {
            let mut sink = sink.lock().expect("the midi.out sink is not poisoned");
            sink.send(&self.scratch, block.frame);
        }
    }
}

/// The clock-out plugin: mounting it registers the node. Both services are
/// optional — a plugin that *required* a device could not mount on a machine
/// without one, which is exactly the replay case the purity rule protects.
///
/// The factory takes no strings and no closures — `fn(&[(&'static str, f32)])`
/// — which is precisely why the sink arrives through the context rather than
/// through a mount parameter: a port name is not a float, and a sink is not a
/// parameter at all.
#[derive(Default)]
pub struct ClockOutPlugin;

impl Plugin for ClockOutPlugin {
    fn id(&self) -> &'static str {
        "clock_out"
    }

    fn inject(&self) -> &'static [&'static str] {
        // Deliberately empty: `midi.out` and `transport` are optional services,
        // and `inject` declares *requirements*.
        &[]
    }

    fn ports(&self) -> &'static [Port] {
        CLOCK_OUT_PORTS
    }

    fn params(&self) -> &'static [ParamDef] {
        &[]
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let sink = api.ctx.get::<SharedMidiSink>(MIDI_OUT_KEY).cloned();
        let transport = api.ctx.get::<SharedTransportLog>(TRANSPORT_KEY).cloned();
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(ClockOutNode::new(sink, transport))),
            Vec::new(),
        );
        Ok((
            node,
            Box::new(move |dis: &mut super::DisposerCtx| {
                dis.graph.remove_node(node);
            }),
        ))
    }
}

/// Factory form — the plugin has no mount parameters; the form exists so the
/// registry treats it like every other plugin.
pub fn clock_out_factory(_params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    Ok(Box::new(ClockOutPlugin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{AudioInputs, AudioNode, RenderMode};
    use crate::plugins::EventSink;

    /// A fake sink implementing `MidiSink`/`EventSink`: records `(frame,
    /// events)` per send. The node receives a `Box<dyn MidiSink>`, so the
    /// recordings live behind a shared `Arc<Mutex<Vec>>` the test also holds.
    /// The recording one `FakeSink` appends to and its test reads.
    type Sends = Arc<Mutex<Vec<(u64, Vec<ExternalEvent>)>>>;

    #[derive(Default)]
    struct FakeSink {
        sends: Sends,
    }

    impl FakeSink {
        fn new() -> (Self, Sends) {
            let sends: Sends = Arc::new(Mutex::new(Vec::new()));
            (
                FakeSink {
                    sends: sends.clone(),
                },
                sends,
            )
        }

        /// The absolute frames of every `Clock` event, in send order.
        fn clock_frames(sends: &Mutex<Vec<(u64, Vec<ExternalEvent>)>>) -> Vec<u64> {
            let sends = sends.lock().unwrap();
            let mut out = Vec::new();
            for &(block_frame, ref events) in sends.iter() {
                for ev in events {
                    if let ExternalEvent::Clock { offset } = ev {
                        out.push(block_frame + *offset as u64);
                    }
                }
            }
            out
        }

        /// Every transport event with its absolute frame, in send order.
        fn transport(sends: &Mutex<Vec<(u64, Vec<ExternalEvent>)>>) -> Vec<(u64, &'static str)> {
            let sends = sends.lock().unwrap();
            let mut out = Vec::new();
            for &(block_frame, ref events) in sends.iter() {
                for ev in events {
                    let (kind, offset) = match ev {
                        ExternalEvent::Start { offset } => ("Start", *offset),
                        ExternalEvent::Stop { offset } => ("Stop", *offset),
                        ExternalEvent::Continue { offset } => ("Continue", *offset),
                        _ => continue,
                    };
                    out.push((block_frame + offset as u64, kind));
                }
            }
            out
        }
    }

    impl EventSink for FakeSink {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn send(&mut self, events: &[ExternalEvent], frame: u64) {
            self.sends.lock().unwrap().push((frame, events.to_vec()));
        }
    }

    impl MidiSink for FakeSink {}

    /// A node with a fake sink, plus the recording handle the assertions read.
    fn node_with_sink() -> (ClockOutNode, Sends) {
        let (sink, recorded) = FakeSink::new();
        let shared: SharedMidiSink = Arc::new(Mutex::new(Box::new(sink)));
        (ClockOutNode::new(Some(shared), None), recorded)
    }

    /// Render `[from, to)` in `block`-frame chunks through the node directly.
    fn render_range(node: &mut ClockOutNode, map: &TempoMap, from: u64, to: u64, block: u64) {
        let mut f = from;
        while f < to {
            let frames = block.min(to - f) as usize;
            let io = NodeIO {
                audio_in: &[],
                audio_ins: AudioInputs::none(),
                audio_in_count: 0,
                audio_out_channels: 0,
                frames,
                control_in: 0.0,
                triggers_in: &[],
                notes_in: &[],
            };
            let mut empty: [f32; 0] = [];
            let mut control = 0.0f32;
            let mut triggers = EventBuf::<Trigger, CAP_EVENTS>::new();
            let mut notes = EventBuf::<NoteEvent, CAP_EVENTS>::new();
            node.render(
                &io,
                &mut empty,
                &mut control,
                &mut triggers,
                &mut notes,
                RenderBlock {
                    frame: f,
                    sample_rate: map.sample_rate(),
                    tempo: map,
                    mode: RenderMode::Timeline,
                },
            );
            f += frames as u64;
        }
    }

    /// Acceptance 1: 24 PPQN at 120 bpm 48 kHz is one tick every 1000 frames —
    /// one second of rendering produces exactly 48 ticks at frames 0, 1000,
    /// … 47000, asserted to the sample, with block boundaries falling between
    /// ticks.
    #[test]
    fn tick_math_48_ticks_per_second_at_exact_frames() {
        let map = TempoMap::new(48_000, 120.0, 4);
        let (mut node, recorded) = node_with_sink();
        render_range(&mut node, &map, 0, 48_000, crate::graph::BLOCK as u64);
        let expected: Vec<u64> = (0..48u64).map(|i| i * 1000).collect();
        assert_eq!(FakeSink::clock_frames(&recorded), expected);
        assert_eq!(node.overflows(), 0);
    }

    /// A tempo change **inside a block**: the tick frames must match
    /// `frame_at(beat)` exactly rather than drifting — the schedule comes from
    /// the tempo map, never from a rate carried between blocks.
    #[test]
    fn tempo_change_inside_a_block_stays_exact() {
        let mut map = TempoMap::new(48_000, 120.0, 4);
        // Slow to 60 bpm a quarter second in — squarely inside a 512-frame block.
        map.push(12_000, 60.0, 4);
        let (mut node, recorded) = node_with_sink();
        render_range(&mut node, &map, 0, 48_000, crate::graph::BLOCK as u64);
        // The expected frames are the tempo map's own answer: every tick n
        // whose frame_at(n/24) lands in the rendered range.
        let expected: Vec<u64> = (0..)
            .map(|n| ClockOutNode::tick_frame(&map, n))
            .take_while(|&f| f < 48_000)
            .collect();
        assert_eq!(FakeSink::clock_frames(&recorded), expected);
        assert_eq!(node.overflows(), 0);
    }

    /// Transport follows the logged commands, at the frame each takes effect,
    /// with the offset carrying the exact sub-block position — and a command
    /// logged after its frame has passed flushes late rather than being lost.
    #[test]
    fn transport_is_emitted_at_logged_frames() {
        let map = TempoMap::new(48_000, 120.0, 4);
        let log = Arc::new(TransportLog::new());
        let (sink, recorded) = FakeSink::new();
        let shared: SharedMidiSink = Arc::new(Mutex::new(Box::new(sink)));
        let mut node = ClockOutNode::new(Some(shared), Some(log.clone()));
        log.push(1_000, Transport::Start);
        log.push(3_000, Transport::Stop);
        log.push(5_500, Transport::Continue);
        // Logged after its frame has passed: flushes late, at offset 0.
        log.push(300, Transport::Stop);
        render_range(&mut node, &map, 0, 6_000, 512);
        assert_eq!(
            FakeSink::transport(&recorded),
            vec![
                (300, "Stop"),
                (1_000, "Start"),
                (3_000, "Stop"),
                (5_500, "Continue"),
            ]
        );
        assert_eq!(
            FakeSink::clock_frames(&recorded),
            vec![0, 1000, 2000, 3000, 4000, 5000]
        );
        assert_eq!(node.overflows(), 0);
    }

    /// The bound is loud: a tempo absurd enough to exceed [`CLOCK_OUT_CAP`] in
    /// one block emits exactly the cap and counts the rest — the scratch never
    /// grows.
    #[test]
    fn overflow_is_counted_never_grown() {
        // 1e9 bpm: ticks come far closer than frames, so one block far
        // exceeds the cap. The due count is the tick loop's own semantics —
        // every n whose `frame_at(n/24)` rounds below the block end —
        // computed here rather than from a beat-rate formula, which would
        // ignore `frame_at`'s rounding (at this tempo many ticks share a
        // rounded frame; at any sane tempo none do).
        let map = TempoMap::new(48_000, 1.0e9, 4);
        let (mut node, recorded) = node_with_sink();
        render_range(&mut node, &map, 0, 512, 512);
        let due = (0..)
            .map(|n| ClockOutNode::tick_frame(&map, n))
            .take_while(|&f| f < 512)
            .count() as u64;
        let emitted = FakeSink::clock_frames(&recorded).len() as u64;
        assert_eq!(emitted + node.overflows(), due);
        assert_eq!(emitted, CLOCK_OUT_CAP as u64);
        assert!(node.overflows() > 0);
    }
}
