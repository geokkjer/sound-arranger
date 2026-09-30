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
fn euclid_walk_block(map: &TempoMap, frame: u64, pattern: Vec<bool>, steps: u32) -> Arc<AtomicU64> {
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

/// The length invariant is **not** enforced by the constructor alone: a
/// public `steps` field could be reassigned after the assert, and the very
/// next block would index a 64-step modulo into an 8-long pattern — a panic
/// on the render thread, from safe code. The fields are private now, so
/// the constructor is the only way in, and the node's own read is back by
/// the pattern's own length. The accessors are the whole read surface.
#[test]
fn a_node_is_built_through_its_constructor_and_read_through_accessors() {
    let node = EuclideanGen::new(8, 4, vec![false; 8]);
    assert_eq!(node.steps(), 8);
    assert_eq!(node.pulses_per_beat(), 4);
    assert_eq!(node.pattern().len(), 8);
    // `steps = 0` is the degenerate form the `max(1)` exists for: one step,
    // a one-long pattern, and a walk that indexes within bounds.
    let degenerate = EuclideanGen::new(0, 4, vec![false; 1]);
    assert_eq!(degenerate.steps(), 0);
    assert_eq!(degenerate.pattern().len(), 1);
}

/// A pattern that does not match `steps` is refused at construction, with a
/// message that says why the two are one thing.
#[test]
#[should_panic(expected = "indexed by `step % steps`")]
fn a_pattern_shorter_than_its_steps_is_refused() {
    EuclideanGen::new(8, 4, vec![false; 4]);
}

/// The pattern is one `bool` per step and the node is built on the render
/// thread's call stack, so a nonsense length is refused **before** the
/// allocation — loudly, and naming the limit, so a 30-byte script line
/// cannot ask for a gigabyte.
#[test]
#[should_panic(expected = "over the 4096-step limit")]
fn an_absurd_step_count_is_refused_before_it_is_allocated() {
    EuclideanGen::new(EUCLIDEAN_MAX_STEPS + 1, 4, Vec::new());
}

/// Render one block of `notes` into `out` through a bare [`ToneGen`] — the
/// view a note consumer gets, with no graph around it.
fn tone_block(node: &mut ToneGen, notes: &[NoteEvent], out: &mut [f32], frame: u64) {
    let map = tempo();
    let io = NodeIO {
        audio_in: &[],
        audio_ins: AudioInputs::none(),
        audio_in_count: 0,
        audio_out_channels: 1,
        frames: out.len(),
        control_in: 0.0,
        triggers_in: &[],
        notes_in: notes,
    };
    let mut control = 0.0f32;
    let mut triggers = EventBuf::new();
    let mut out_notes = EventBuf::new();
    node.render(
        &io,
        out,
        &mut control,
        &mut triggers,
        &mut out_notes,
        RenderBlock {
            frame,
            sample_rate: 48_000,
            tempo: &map,
            mode: RenderMode::Timeline,
        },
    );
}

/// A note at `offset == frames` is a **legal `EventBuf` payload** and nothing
/// forbids one: `EventBuf` bounds capacity, never offsets. The tone's
/// pending-onset queue is a ring of [`MAX_PENDING`], the per-sample loop
/// reaches only `0..out.len()`, and the head used to return to 0 **only when
/// the queue emptied by itself** — so such a note was queued and never
/// drained, the head stayed parked, and the next block's write at
/// `head + count` indexed past the end of the array: an out-of-bounds panic
/// on the render thread.
#[test]
fn an_onset_past_the_block_does_not_outlive_its_block() {
    const FRAMES: usize = 64;
    let inside = NoteEvent {
        offset: 0,
        pitch: 0.0,
        velocity: 1.0,
        duration: 4,
    };
    let mut node = ToneGen::new(0.25, 4);

    // Block A: one note the block can reach, then a pool's worth of notes
    // offset past its last sample.
    let mut notes = vec![inside];
    for _ in 0..MAX_PENDING {
        notes.push(NoteEvent {
            offset: FRAMES as u32,
            ..inside
        });
    }
    let mut a = [0.0f32; FRAMES];
    tone_block(&mut node, &notes, &mut a, 0);
    assert!(
        a[1] > 0.0,
        "the in-block note still sounds: the first sample is sin(0), the second is not"
    );
    assert_eq!(
        (node.pending_head, node.pending_count),
        (0, 0),
        "nothing queued outlives the block it was queued for"
    );

    // Block B: one ordinary note. Unfixed, the write at `pending[8]` panicked
    // here — and with the queue merely *bounded* instead of cleared, this
    // block would be silent.
    let mut b = [0.0f32; FRAMES];
    tone_block(&mut node, &[inside], &mut b, FRAMES as u64);
    assert!(
        b[1] > 0.0,
        "the next block's note sounds: the queue is empty, not full of undrained entries"
    );
}
