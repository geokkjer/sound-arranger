//! The clock-out plugin's engine-level tests (the midi-clock-out note's
//! acceptance criteria 3 and 4): the mount is a plugin like any other, the
//! audio is **byte-identical with and without a sink**, and a session mounts
//! and renders with no device at all.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use engine::plugins::clock_out::{MIDI_OUT_KEY, SharedMidiSink, clock_out_factory};
use engine::*;

const SR: u32 = 48_000;

/// The fake sink of the module tests, again — the recording lives behind an
/// inner `Arc` because the context stores a `Box<dyn MidiSink>`.
#[derive(Default)]
struct FakeSink;

impl MidiSink for FakeSink {}

impl EventSink for FakeSink {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn send(&mut self, _events: &[ExternalEvent], _frame: u64) {
        // Recorded count only; the tick math is asserted in the module tests.
        SENDS.fetch_add(1, Ordering::Relaxed);
    }
}

static SENDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn engine_with_clock_out(sink: bool) -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory(
        "tone",
        plugins::tone_factory,
        plugins::tone::TONE_PORTS,
        plugins::tone::TONE_PARAMS,
    );
    e.register_factory("clock_out", clock_out_factory, &[], &[]);
    if sink {
        let shared: SharedMidiSink = Arc::new(Mutex::new(Some(Box::new(FakeSink))));
        e.ctx.provide(MIDI_OUT_KEY, shared);
    }
    // The canonical one-voice chain: tone owns the out bus, clock_out hangs
    // beside it with no ports at all.
    e.mount("tone", &[("gain", 0.2), ("blip_len", 800.0)])
        .unwrap();
    e.mount("clock_out", &[]).unwrap();
    e
}

/// Purity: the same session rendered with a sink in the context and without
/// one produces **byte-identical audio**. Whether the gear is attached cannot
/// change the sound — this is the property the note promises.
#[test]
fn audio_is_byte_identical_with_and_without_a_sink() {
    let bits = |e: &mut Engine| {
        let mut out = Vec::new();
        for _ in 0..16 {
            out.extend(e.render(BLOCK).into_iter().map(f32::to_bits));
        }
        out
    };
    let with = bits(&mut engine_with_clock_out(true));
    let sent = SENDS.load(Ordering::Relaxed);
    let without = bits(&mut engine_with_clock_out(false));
    assert_eq!(with, without, "the sink must not influence the audio");
    assert!(sent > 0, "the sink actually sent");
}

/// No sink, no transport log: mounting succeeds and renders — a session
/// without the device is a first-class state, not an error. (Mounts apply on
/// the test's own first render, so the node exists only after one.)
#[test]
fn mounts_and_renders_with_neither_service() {
    let mut e = engine_with_clock_out(false);
    let rendered = e.render(BLOCK);
    assert!(e.node_of("clock_out").is_some());
    assert_eq!(rendered.len(), BLOCK, "the tone's mono bus renders");
}
