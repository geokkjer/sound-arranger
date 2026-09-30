use super::*;

/// A `MidiOut` with **no device and no writer thread** — the ring is only
/// fed, so the drop path is observable deterministically.
fn deviceless(capacity: usize) -> MidiOut {
    MidiOut {
        ring: Arc::new(Spsc::new(capacity)),
        dropped_events: Arc::new(AtomicU64::new(0)),
        dropped_bytes: Arc::new(AtomicU64::new(0)),
        stop: Arc::new(AtomicBool::new(false)),
        writer: None,
    }
}

/// The drained wire bytes, for assertions.
fn drained(out: &MidiOut) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(msg) = out.ring.try_pop() {
        bytes.extend_from_slice(msg.as_slice());
    }
    bytes
}

/// The realtime bytes map one-to-one: clock, start, stop, continue.
#[test]
fn clock_and_transport_map_to_their_status_bytes() {
    let mut out = deviceless(8);
    out.send(
        &[
            ExternalEvent::Clock { offset: 0 },
            ExternalEvent::Start { offset: 0 },
            ExternalEvent::Stop { offset: 0 },
            ExternalEvent::Continue { offset: 0 },
        ],
        0,
    );
    assert_eq!(drained(&out), vec![0xF8, 0xFA, 0xFC, 0xFB]);
    assert_eq!(out.dropped_events(), 0);
}

/// Notes and control use the documented conversions: pitch is semitones
/// from A4 → MIDI note (+69, rounded, clamped 7-bit); velocity and CC
/// values scale 0..1 → 0..=127; `Control` rides controller 1 (the
/// documented default, the currency carries no controller index).
#[test]
fn notes_and_control_use_the_documented_conversions() {
    let mut out = deviceless(8);
    out.send(
        &[
            // A4 (pitch 0) fortissimo, then C3 (pitch -21) silent.
            ExternalEvent::NoteOn {
                offset: 0,
                pitch: 0.0,
                velocity: 1.0,
            },
            ExternalEvent::NoteOff {
                offset: 0,
                pitch: -21.0,
            },
            // A clamping note (two octaves above the wire's top) and a
            // mid-scale CC.
            ExternalEvent::NoteOn {
                offset: 0,
                pitch: 100.0,
                velocity: 0.5,
            },
            ExternalEvent::Control { value: 0.5 },
            // No wire form: dropped without counting (nothing was due).
            ExternalEvent::Trigger { offset: 0 },
        ],
        0,
    );
    assert_eq!(
        drained(&out),
        vec![
            0x90, 69, 127, // A4 on, velocity 127
            0x80, 48, 0, // C3 off
            0x90, 127, 64, // clamped to the wire's top note, velocity 64
            0xB0, 1, 64, // CC#1, half scale
        ]
    );
    assert_eq!(out.dropped_events(), 0, "a Trigger is not an overflow");
}

/// A full ring drops and counts — never blocks, never grows: `send` on a
/// deviceless sink with a tiny ring fills it and counts the rest, per
/// event and per wire byte.
#[test]
fn a_full_ring_drops_and_counts() {
    let mut out = deviceless(4);
    let events: Vec<ExternalEvent> = (0..10)
        .map(|_| ExternalEvent::Clock { offset: 0 })
        .collect();
    out.send(&events, 0);
    assert_eq!(out.dropped_events(), 6, "4 fit, the rest are counted");
    assert_eq!(out.dropped_bytes(), 6, "each dropped tick is one byte");
    assert_eq!(out.ring.len(), 4, "the ring stays at capacity");
}

/// Port matching: case-insensitive substring, all matches reported so the
/// caller can fail loudly naming them. Pure — no backend involved.
#[test]
fn port_matching_is_a_case_insensitive_substring() {
    let available = vec![
        "Midi Through:Port-0".to_string(),
        "UM-1:UM-1 MIDI 1".to_string(),
    ];
    assert_eq!(match_ports(&available, "um-1"), vec!["UM-1:UM-1 MIDI 1"]);
    assert_eq!(match_ports(&available, "midi"), available, "both match");
    assert!(match_ports(&available, "q49").is_empty(), "none match");
}

/// Open a **real** port and send a few ticks — the hardware test, ignored
/// like the capture/audio ones: it needs a device and it touches it.
#[test]
#[ignore = "needs a real MIDI output; set DSH_MIDI_OUT to a port-name substring and run: cargo test -p media -- --ignored midi_out_real_port_sends_a_few_ticks -- --nocapture"]
fn midi_out_real_port_sends_a_few_ticks() {
    let Ok(port) = std::env::var("DSH_MIDI_OUT") else {
        eprintln!("DSH_MIDI_OUT is not set — nothing to open");
        eprintln!("available: {:?}", ports().unwrap_or_default());
        return;
    };
    let mut out = MidiOut::open(&port).expect("the requested port opens");
    // A musically legible burst: start, one beat of clock, stop.
    out.send(&[ExternalEvent::Start { offset: 0 }], 0);
    for _ in 0..24 {
        out.send(&[ExternalEvent::Clock { offset: 0 }], 0);
    }
    out.send(&[ExternalEvent::Stop { offset: 0 }], 0);
    // Give the writer thread time to drain, then report — the human sees
    // the counters, the gear sees the burst.
    std::thread::sleep(Duration::from_millis(200));
    eprintln!(
        "sent 26 messages to '{port}' — dropped: {} events / {} bytes",
        out.dropped_events(),
        out.dropped_bytes()
    );
    assert_eq!(out.dropped_events(), 0, "the writer kept up");
}
