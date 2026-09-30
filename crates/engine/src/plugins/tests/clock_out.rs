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
