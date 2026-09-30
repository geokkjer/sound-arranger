//! The MIDI clock-out plugin (the midi-clock-out note, 2026-09-29): the
//! recorder profile's *sender* — a follower is driven with MIDI clock (24 PPQN)
//! and transport (`Start` / `Stop` / `Continue`), sample-accurately against the
//! same tempo map the scheduler reads.
//!
//! The clock is the core's, read-only: the node computes each tick's frame
//! **statelessly from the block's own frame range** — no continuity is tracked
//! between blocks, and no transport is inferred from a frame jump. A seek
//! rebuilds the session, so nothing is sent across a rebuild; the **host** owns
//! the re-sync at the far side of one — the same message a `play` at that frame
//! sends in the rebuilt session's own log (`Start` at frame 0, `Continue`
//! elsewhere) when the transport was still playing (see `replay_to_kind` in
//! `crates/host`), and otherwise nothing until the next `play`.
//!
//! Device ownership stays with the host: the node holds the sink **slot** the
//! host provided under the [`MIDI_OUT_KEY`] context key (or its own empty one
//! when the key is absent), and **both services are optional** — a session
//! mounts on a machine with no device, sends nothing, and renders
//! byte-identically (the takes-slice purity rule: the declaration is state,
//! the sending is a device-bound side effect, never replayed). The host can
//! empty the slot across a rebuild and across an offline render, and refill it
//! after, so a seek sends nothing while it reconstructs, a `bounce` sends
//! nothing while it renders, and gear is driven again on the next live render.

use std::sync::atomic::{AtomicU64, Ordering};
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

/// The context key the transport log the host feeds arrives under
/// ([`SharedTransportLog`]). Optional for the same reason.
pub const TRANSPORT_KEY: &str = "transport";

/// The context key the plugin **publishes** its overflow counter under — the
/// host (and through it, a shell's snapshot) reads it back with the same
/// context-service pattern the mixer's `mixer.meters` uses. Provided on apply,
/// withdrawn by the disposer.
pub const CLOCK_OUT_OVERFLOWS_KEY: &str = "clock_out.overflows";

/// The shared-sink **slot** the host provides: a mutex around an *optional*
/// device, so the host can empty it (across any rebuild — the seek-and-rebuild
/// decision: nothing is sent while a session is reconstructed) and refill it
/// afterwards, with the re-fill visible to a node that already holds the slot.
/// The real device sink will make `send` a fast queue push, and with a single
/// sender the lock is uncontended.
pub type SharedMidiSink = Arc<Mutex<Option<Box<dyn MidiSink>>>>;

/// The shared transport log the host feeds ([`TransportLog`]).
pub type SharedTransportLog = Arc<TransportLog>;

/// A MIDI transport command (the note's scope: `Start`, `Stop`, `Continue` —
/// Song Position Pointer is explicitly out of this slice).
///
/// The distinction between `Start` and `Continue` is **the point of the two**,
/// not a stylistic choice, and it is the host's job to honour it when it feeds
/// the log: `Start` means **"return to song start"**, so it is truthful only at
/// frame 0 — anywhere else it tells a follower to jump to its own top while the
/// session sits elsewhere. `Continue` means "carry on running from where you
/// are" and makes no claim about an absolute position, so it is the honest
/// message at every other frame. Neither states a position in the middle of a
/// piece; that is what SPP is for, and it is not in this scope.
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
    /// into the very next block instead of sitting in the queue forever.
    ///
    /// The drain is bounded by the room `out` has, so the node's preallocated
    /// buffer is never grown and this stays allocation-free on the render path
    /// however many commands are pending. What does not fit stays **queued** —
    /// this log's contract is that a command flushes late, not that it is
    /// dropped — and the next block's drain carries it, at that block's first
    /// frame. A buffer that was never given a capacity has no bound to honour,
    /// so it takes the whole due prefix (the pre-bound behaviour, for a caller
    /// that is not the render path).
    pub fn take_due(&self, block_end: u64, out: &mut Vec<(u64, Transport)>) {
        let mut queue = self
            .queue
            .lock()
            .expect("the transport log is not poisoned");
        let due = queue.partition_point(|&(f, _)| f < block_end);
        let at = if out.capacity() == 0 {
            due
        } else {
            due.min(out.capacity() - out.len())
        };
        out.extend(queue.drain(..at));
        drop(queue);
    }
}

/// The hard bound on events emitted per block. A [`BLOCK`](crate::graph::BLOCK)-
/// frame block cannot carry more than a handful of ticks at any sane tempo, but
/// an absurd one could: when a block would exceed the bound, the node emits
/// what fits and counts the rest. Dropping clock ticks is bad — but allocating
/// on the render path is worse, so the bound must be **visible**: the overflow
/// counter is a loud failure, not silent growth. The bound is on the **walk**,
/// not only on the scratch: a tempo whose ticks all round onto one frame would
/// otherwise keep the loop running after the scratch stopped growing.
const CLOCK_OUT_CAP: usize = 64;

/// The hard bound on the noise-correction walk in
/// [`ClockOutNode::first_tick_at_or_after`], **in both directions**. The
/// correction exists to undo f64 rounding, so it is one or two ticks at any tempo
/// where a tick is worth a frame; a generous bound never changes the answer. It
/// exists because a tempo whose ticks are *closer together than frames* cannot
/// converge — there the beat-domain candidate and the frame domain disagree by
/// an unbounded number of ticks — and a walk that cannot converge must still
/// return. The downward walk needs the bound for the same reason: a map whose
/// `frame_at` disagrees with `beat_at` by more than rounding makes even the
/// downward correction unbounded.
const TICK_WALK_CAP: u64 = 1 << 16;

/// The clock-out node: computes the MIDI clock ticks and transport events due
/// in each block and sends them through the host's sink. It produces no audio,
/// declares no ports, and tracks no state between blocks.
pub struct ClockOutNode {
    /// The host's sink slot, always present: when the host provided a slot
    /// under [`MIDI_OUT_KEY`] the node shares it, and when it did not the
    /// node holds its **own empty slot** — either way the render path sends
    /// only when the slot currently contains a device, and the host can
    /// empty and refill a shared slot without re-mounting the plugin.
    sink: SharedMidiSink,
    transport: Option<SharedTransportLog>,
    /// Per-block event scratch, allocated once at construction and `clear`ed
    /// each block: the render path never grows it (see [`CLOCK_OUT_CAP`]).
    scratch: Vec<ExternalEvent>,
    /// Transport drained from the log this block — also preallocated, and the
    /// render path never grows it: [`TransportLog::take_due`] takes what fits
    /// and leaves the rest queued for the next block.
    transport_scratch: Vec<(u64, Transport)>,
    /// Events that did not fit the scratch: a loud bound, kept behind a shared
    /// atomic so the plugin can **publish** it under [`CLOCK_OUT_OVERFLOWS_KEY`]
    /// while the render path only ever increments — a counter is not worth a
    /// lock in the block.
    overflows: Arc<AtomicU64>,
}

impl ClockOutNode {
    /// Construct with the services the plugin found in the context at apply
    /// time — either may be `None`.
    pub fn new(sink: Option<SharedMidiSink>, transport: Option<SharedTransportLog>) -> Self {
        Self::with_overflow_counter(sink, transport, Arc::new(AtomicU64::new(0)))
    }

    /// Construct with an **existing** overflow counter — the apply path's form,
    /// which publishes the same `Arc` into the context so a reader outside the
    /// graph sees the live count.
    pub fn with_overflow_counter(
        sink: Option<SharedMidiSink>,
        transport: Option<SharedTransportLog>,
        overflows: Arc<AtomicU64>,
    ) -> Self {
        // No host-provided slot under `midi.out` is a first-class state — the
        // node mounts its own, permanently empty, so a session without the
        // device sends nothing and renders byte-identically.
        ClockOutNode {
            sink: sink.unwrap_or_else(|| Arc::new(Mutex::new(None))),
            transport,
            scratch: Vec::with_capacity(CLOCK_OUT_CAP),
            transport_scratch: Vec::with_capacity(CLOCK_OUT_CAP),
            overflows,
        }
    }

    /// How many events were dropped because a block exceeded
    /// [`CLOCK_OUT_CAP`] — a mount this sane never sees a nonzero count.
    pub fn overflows(&self) -> u64 {
        self.overflows.load(Ordering::Relaxed)
    }

    /// Whether the slot currently holds a device — the one question that decides
    /// whether this block may consume the transport log (see `render`). The lock
    /// is held only for the check, and the host empties and refills the slot
    /// between its own commands rather than from inside a render, so the answer
    /// does not change under the block that read it.
    fn device_attached(&self) -> bool {
        self.sink
            .lock()
            .expect("the midi.out sink is not poisoned")
            .is_some()
    }

    /// Push into the scratch, or count the overflow — never grow.
    fn push(&mut self, event: ExternalEvent) {
        if self.scratch.len() < self.scratch.capacity() {
            self.scratch.push(event);
        } else {
            self.overflows.fetch_add(1, Ordering::Relaxed);
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
    ///
    /// The walk is bounded by [`TICK_WALK_CAP`] in **both** directions — a
    /// correction is short, and a walk that cannot converge (a tempo whose ticks
    /// are closer together than frames, or a map whose `frame_at` disagrees with
    /// its `beat_at` by more than rounding) must still return, because this runs
    /// on the render path.
    ///
    /// Every step is also **checked**: `n0` saturates at `u64::MAX` when a tick
    /// is worth far less than a frame, and `tick_frame(u64::MAX)` then reads back
    /// as some early frame, so the loop condition stayed true at the top of the
    /// range. An unchecked `n += 1` there panicked in a debug build; in release
    /// the add wrapped to 0 and the walk restarted from tick 0 — the hang the
    /// cap exists to prevent, arriving through the overflow instead of past it.
    fn first_tick_at_or_after(map: &TempoMap, frame: u64) -> Option<u64> {
        // `frame_at` is monotone in the tick index, so the *last* index answers
        // for all of them. If even it lands short of `frame`, no tick index
        // exists at or after `frame` — the block carries no ticks at all, and
        // the honest answer is `None` rather than a saturated index the emit
        // loop would send at offset 0.
        if Self::tick_frame(map, u64::MAX) < frame {
            return None;
        }
        let n0 = (map.beat_at(frame) * TICKS_PER_BEAT as f64).floor() as u64;
        let mut n = n0;
        let mut walked = 0u64;
        while walked < TICK_WALK_CAP && Self::tick_frame(map, n) < frame {
            match n.checked_add(1) {
                Some(next) => n = next,
                // Unreachable while `frame_at` is monotone and the check above
                // holds; present so a non-monotone map cannot panic the render
                // thread here.
                None => break,
            }
            walked += 1;
        }
        walked = 0;
        while walked < TICK_WALK_CAP && n > 0 && Self::tick_frame(map, n - 1) >= frame {
            n -= 1;
            walked += 1;
        }
        Some(n)
    }

    /// How many ticks from `from` on fall before `block_end` — the count the
    /// capped walk stopped short of. `frame_at` is monotone in the tick index,
    /// so a binary search answers it in a fixed number of lookups: the block
    /// pays for the count instead of the unbounded walk the count replaces, and
    /// the overflow counter keeps meaning *exactly* how many ticks the walk
    /// skipped. `tick_frame(map, from) < block_end` must hold.
    ///
    /// "Exactly" includes the saturated case, and the earlier version of this
    /// comment (that the count could only ever be *over*stated) was wrong: when
    /// every index is due — a tempo so fast that even `u64::MAX` rounds onto a
    /// frame inside the block — the doubling bracket had no "not due" end, so the
    /// binary search settled one short of the top and the count was understated
    /// by one. That case is now answered before the bracket is built.
    fn due_ticks_from(map: &TempoMap, from: u64, block_end: u64) -> u64 {
        // Every index the map can name is due, so the count is every index from
        // `from` on — there is no first *not*-due tick to bracket against.
        // `frame_at` is monotone, so `u64::MAX` answers for all of them. The
        // count saturates only if it cannot be represented at all (`from == 0`),
        // which is the one case where "exact" is not a number.
        if Self::tick_frame(map, u64::MAX) < block_end {
            return u64::MAX.saturating_sub(from).saturating_add(1);
        }
        // Bracket by doubling from `from` (already known due) up to a tick at or
        // past `block_end`. The doubling cannot run away: the last step pins `hi`
        // at `u64::MAX`, which the check above has just established is *not*
        // due, so the bracket is real and the search converges on the first
        // index past the block.
        let mut lo = from;
        let mut hi = from.saturating_add(1);
        while Self::tick_frame(map, hi) < block_end {
            lo = hi;
            if hi >= u64::MAX / 2 {
                hi = u64::MAX;
                break;
            }
            hi *= 2;
        }
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if Self::tick_frame(map, mid) < block_end {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo.saturating_sub(from).saturating_add(1)
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
        //
        // **Only a block that can speak drains it.** The log's whole contract is
        // that a command flushes *late, never lost*, so a block with an empty
        // slot (the host detaches the device across a rebuild, and across an
        // offline render) must leave the queue alone: a take that found no device
        // to send through would drop the command outright, and no later message
        // restates it — the same loss the bounded `take_due` refuses. The ticks
        // below are not in that class, because they are computed from
        // `block.frame` rather than from a queue: a silent block costs nothing
        // and the next block recomputes its own range.
        if let Some(log) = &self.transport
            && self.device_attached()
        {
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
        //
        // `None` is a block with **no** due tick (a tempo fast enough that every
        // index rounds onto a frame before this one): there is nothing to send
        // and nothing to drop, so the counter stays where it is rather than
        // counting ticks that never existed.
        if let Some(mut n) = Self::first_tick_at_or_after(block.tempo, block.frame) {
            let n0 = n;
            loop {
                let frame = Self::tick_frame(block.tempo, n);
                if frame >= block_end {
                    break;
                }
                if n - n0 >= CLOCK_OUT_CAP as u64 {
                    // The cap the module declares: the rest of the block's ticks are
                    // *counted*, not walked. Exact, so `overflows` still means "how
                    // many ticks this block dropped", and bounded, so no tempo can
                    // hold the render thread in this loop.
                    self.overflows.fetch_add(
                        Self::due_ticks_from(block.tempo, n, block_end),
                        Ordering::Relaxed,
                    );
                    break;
                }
                // Saturating: a tick the bounded correction could not walk onto the
                // block's frame lands at the block's start rather than underflowing.
                self.push(ExternalEvent::Clock {
                    offset: frame.saturating_sub(block.frame) as u32,
                });
                // Checked, like the correction walk: `u64::MAX` is the last tick
                // index there is, and every due one has been emitted, so stopping
                // here drops nothing. A wrapped `n` would restart the block at
                // tick 0.
                match n.checked_add(1) {
                    Some(next) => n = next,
                    None => break,
                }
            }
        }

        // One send per block; the offsets carry the exact sub-block positions.
        // The lock is held only for the call, and with a single sender it is
        // uncontended — the real device sink will make this a fast queue push.
        // An **empty** slot sends nothing: the host detaches the device across a
        // rebuild and across an offline render, and the tick schedule is computed
        // from the block's own frame, so a silent block loses nothing — a queued
        // transport command is the exception, and it stays queued (above).
        if !self.scratch.is_empty()
            && let Some(device) = self
                .sink
                .lock()
                .expect("the midi.out sink is not poisoned")
                .as_mut()
        {
            device.send(&self.scratch, block.frame);
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
        // The overflow counter is published, not kept private: the host's
        // snapshot reads it back under the key (the mixer's `mixer.meters`
        // pattern), so a dropped tick is visible rather than silent.
        let overflows = Arc::new(AtomicU64::new(0));
        let node = ClockOutNode::with_overflow_counter(sink, transport, overflows.clone());
        api.ctx.provide(CLOCK_OUT_OVERFLOWS_KEY, overflows);
        let node_id = api
            .graph
            .add_node(NodeKind::Opaque(Box::new(node)), Vec::new());
        Ok((
            node_id,
            Box::new(move |dis: &mut super::DisposerCtx| {
                dis.ctx.remove(CLOCK_OUT_OVERFLOWS_KEY);
                dis.graph.remove_node(node_id);
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

    /// A node with a fake sink in its slot, plus the recording handle the
    /// assertions read.
    fn node_with_sink() -> (ClockOutNode, Sends) {
        let (sink, recorded) = FakeSink::new();
        let slot: SharedMidiSink = Arc::new(Mutex::new(Some(Box::new(sink))));
        (ClockOutNode::new(Some(slot), None), recorded)
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
        let slot: SharedMidiSink = Arc::new(Mutex::new(Some(Box::new(sink))));
        let mut node = ClockOutNode::new(Some(slot), Some(log.clone()));
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

    /// A block can owe **more transport commands than the scratch holds**: the
    /// host feeds the log from its control path with no bound, and nothing
    /// renders between the commands (a script of seventy `transport play`
    /// lines, or a stopped session's worth of commands to a live host, which
    /// renders only while playing). `take_due` drained the *whole* due prefix
    /// into `transport_scratch`, and `Vec::extend` over a `Drain` reallocates,
    /// so the overflow was an allocation inside `render` under a doc that
    /// promised otherwise. The drain is now bounded by the room the buffer has:
    /// the scratch keeps its capacity, and the commands that did not fit stay
    /// **queued** and flush into the next block — this log drops nothing, which
    /// is the whole of its contract.
    #[test]
    fn a_transport_flood_is_bounded_and_nothing_is_lost() {
        let map = TempoMap::new(48_000, 120.0, 4);
        let log = Arc::new(TransportLog::new());
        let (sink, recorded) = FakeSink::new();
        let slot: SharedMidiSink = Arc::new(Mutex::new(Some(Box::new(sink))));
        let mut node = ClockOutNode::new(Some(slot), Some(log.clone()));
        // Two caps and one, all logged at frame 300, so all of them are due in
        // the first block ([0, 512)) and three blocks are needed to carry them.
        let due = CLOCK_OUT_CAP * 2 + 1;
        for _ in 0..due {
            log.push(300, Transport::Start);
        }
        render_range(&mut node, &map, 0, 3 * 512, 512);
        let sent = FakeSink::transport(&recorded);
        assert_eq!(sent.len(), due, "every queued command reached the wire");
        assert_eq!(sent[0], (300, "Start"), "the first at its logged frame");
        assert_eq!(
            sent[CLOCK_OUT_CAP],
            (512, "Start"),
            "the cap's next command flushed at the next block's first frame"
        );
        assert_eq!(
            sent[due - 1],
            (1024, "Start"),
            "and the last one a block after that — late, not lost"
        );
        assert_eq!(
            node.transport_scratch.capacity(),
            CLOCK_OUT_CAP,
            "the drain must never grow the scratch: that allocation was on the render path"
        );
        // The bound stays loud about what it did drop. Transport is sent before
        // ticks, so a block whose scratch is all transport drops its own tick —
        // counted, never grown, exactly as the tick path already was.
        let ticks_due = (0..)
            .map(|n| ClockOutNode::tick_frame(&map, n))
            .take_while(|&f| f < 3 * 512)
            .count() as u64;
        assert_eq!(
            FakeSink::clock_frames(&recorded).len() as u64 + node.overflows(),
            ticks_due
        );
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

    /// The cap bounds the **walk**, not only the scratch, so no tempo can hold
    /// the render thread in the tick loop. A tempo too slow for any tick to
    /// land in the block used to pin every tick at frame 0 (and one too fast
    /// rounds them all onto frame 0); either way the loop never reached the
    /// block end, `render` never returned, and the actor thread wedged with the
    /// shell frozen. Both must return, and the fast one must be loud.
    #[test]
    fn an_absurd_tempo_does_not_hold_the_render_loop() {
        // Too slow: tick 0 is at frame 0, and no later tick is in reach.
        let map = TempoMap::new(48_000, 1e-15, 4);
        let (mut node, recorded) = node_with_sink();
        render_range(&mut node, &map, 0, 512, 512);
        assert_eq!(FakeSink::clock_frames(&recorded), vec![0]);
        assert_eq!(node.overflows(), 0);

        // Too fast: every tick rounds onto frame 0, so the cap stops the walk
        // and the block's remaining ticks are counted.
        let map = TempoMap::new(48_000, 1e300, 4);
        let (mut node, recorded) = node_with_sink();
        render_range(&mut node, &map, 0, 512, 512);
        assert_eq!(
            FakeSink::clock_frames(&recorded).len(),
            CLOCK_OUT_CAP,
            "the block emitted exactly the cap"
        );
        assert!(node.overflows() > 0, "and the rest was counted, not walked");
    }

    /// The cap bounds the **correction walk**, both ways, and the walk's
    /// increment cannot overflow. At 1e300 bpm every tick index in `u64` rounds
    /// onto frame 0, so from the *second* block on there is no tick at or after
    /// the block's first frame: `beat_at` saturates `n0` at `u64::MAX`,
    /// `tick_frame(u64::MAX)` reads back as `0`, and an unchecked `n += 1` past
    /// the top panicked in any debug build (`set_tempo(1e300, 4)` reaches it —
    /// the floor only bounds the slow side). In release the add wrapped to 0 and
    /// the walk restarted from tick 0. The node must report "no such tick" and
    /// send nothing instead: the second block carries no ticks at all, so there
    /// is nothing to drop and the overflow counter must not move.
    #[test]
    fn a_fast_tempo_one_block_in_does_not_panic_or_walk() {
        let map = TempoMap::new(48_000, 1e300, 4);
        // The honest answer, asserted directly: no index reaches frame 512.
        assert_eq!(ClockOutNode::first_tick_at_or_after(&map, 512), None);
        assert_eq!(
            ClockOutNode::first_tick_at_or_after(&map, 0),
            Some(0),
            "and the first block still starts at tick 0"
        );
        // Two blocks, so the second one has `block.frame >= 1`. Both return.
        let (mut node, recorded) = node_with_sink();
        render_range(&mut node, &map, 0, 2 * 512, 512);
        assert_eq!(
            FakeSink::clock_frames(&recorded).len(),
            CLOCK_OUT_CAP,
            "only the first block had a tick to send, and it sent exactly the cap"
        );
        assert_eq!(
            recorded.lock().unwrap().len(),
            1,
            "the second block sent nothing at all — not even an empty send"
        );
        // The counter moved for the first block only; the second block dropped
        // no ticks, because it had none. And it moved by *exactly* the ticks the
        // walk did not take: every index from 64 on is due here, because
        // `tick_frame(u64::MAX)` is inside the block too, so the count is the
        // whole remaining range. This is the saturated case the old bracket
        // answered one short.
        assert_eq!(node.overflows(), u64::MAX - 63, "exact, not short by one");
    }

    /// The **downward** correction walk is bounded too, and it does have a
    /// trigger: a map whose `frame_at` disagrees with `beat_at` by more than
    /// rounding. A fast segment followed by a sub-floor one is enough — here
    /// `beat_at(1000)` says tick `n0` is at the boundary while `frame_at`
    /// resolves the whole neighbourhood of `n0` onto frame 1000, because one
    /// tick there is worth ~1e-15 frames and f64 cannot resolve 24 ticks per
    /// beat at that magnitude. Uncapped, the walk steps ~1e14 times before the
    /// frame rounds down (it did not return in 600 s of a debug build).
    #[test]
    fn the_downward_walk_is_bounded() {
        let mut map = TempoMap::new(48_000, 1e20, 4);
        map.push(1000, 1e-16, 4);
        let n0 = (map.beat_at(1000) * TICKS_PER_BEAT as f64).floor() as u64;
        assert!(
            ClockOutNode::tick_frame(&map, n0 - 1) >= 1000,
            "the map really does disagree: tick n0 - 1 reads at or after the frame"
        );
        // Short by exactly the cap — the walk stopped at its bound rather than
        // running to the true first tick.
        assert_eq!(
            ClockOutNode::first_tick_at_or_after(&map, 1000),
            Some(n0 - TICK_WALK_CAP)
        );
        // And the render path returns: one bounded walk, then the capped emit.
        let (mut node, recorded) = node_with_sink();
        render_range(&mut node, &map, 512, 1024, 512);
        assert_eq!(
            FakeSink::clock_frames(&recorded),
            vec![512; CLOCK_OUT_CAP],
            "the block emitted exactly the cap, every tick pinned at the frame the \
             bounded walk landed on"
        );
        assert!(node.overflows() > 0, "and the rest was counted, not walked");
    }

    /// An **empty** slot sends nothing: the host's detach across a rebuild
    /// leaves the node holding an empty slot, and the render path treats that
    /// as "no device" — silently, never an error.
    #[test]
    fn an_empty_slot_sends_nothing() {
        let map = TempoMap::new(48_000, 120.0, 4);
        let (mut node, recorded) = node_with_sink();
        // The host's detach: the device leaves the slot, the node keeps it.
        *node.sink.lock().expect("not poisoned") = None;
        render_range(&mut node, &map, 0, 48_000, crate::graph::BLOCK as u64);
        assert!(
            recorded.lock().unwrap().is_empty(),
            "the detached node sent nothing"
        );
        assert_eq!(node.overflows(), 0);
    }

    /// A **silent block must not consume the transport log**: the log's contract
    /// is that a command flushes *late, never lost*, so a block with an empty
    /// slot leaves the queue alone and the next block that has a device carries
    /// the command — late, at that block's first frame. The ticks are in no such
    /// danger (they are computed from the block's own frame range), so silence
    /// costs the log nothing and the gear no message it would otherwise have
    /// restated.
    #[test]
    fn a_detached_block_keeps_a_queued_transport_command() {
        let map = TempoMap::new(48_000, 120.0, 4);
        let log = Arc::new(TransportLog::new());
        let (sink, recorded) = FakeSink::new();
        let slot: SharedMidiSink = Arc::new(Mutex::new(Some(Box::new(sink))));
        let mut node = ClockOutNode::new(Some(slot), Some(log.clone()));
        log.push(1_000, Transport::Start);

        // The host's detach — the device leaves the slot, the node keeps it —
        // across blocks that owe the command.
        let device = node.sink.lock().expect("not poisoned").take();
        render_range(&mut node, &map, 0, 4_800, crate::graph::BLOCK as u64);
        assert!(
            recorded.lock().unwrap().is_empty(),
            "the detached blocks sent nothing"
        );

        // The device goes back — the host's refill, the same box — and the command
        // that came due while it was away is still queued: late, not lost.
        *node.sink.lock().expect("not poisoned") = device;
        render_range(&mut node, &map, 4_800, 9_600, crate::graph::BLOCK as u64);
        assert_eq!(
            FakeSink::transport(&recorded),
            vec![(4_800, "Start")],
            "the first block with a device flushes the command the silent ones kept"
        );
    }
}
