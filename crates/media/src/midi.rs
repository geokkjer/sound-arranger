//! The real MIDI output sink (the midi-clock-out note, slice B): [`MidiOut`]
//! implements the engine's `MidiSink` seam and owns a real port, opened
//! through `midir` — the same role `devices` plays for audio, isolated in one
//! module so backend drift cannot leak into the machinery the numeric tests
//! cover.
//!
//! The shape is the capture path's: `send` — which the render path calls, once
//! per block under the sink mutex — converts events to MIDI bytes and pushes
//! them into a **bounded lock-free ring** ([`crate::ring::Spsc`]); one writer
//! thread owns the port and drains the ring. `send` **never blocks, never
//! allocates, never touches the device** — a full ring drops the message and
//! counts it (`dropped_events`/`dropped_bytes`), exactly how capture counts an
//! overrun. The ring holds 1024 messages — about 42 s of pure MIDI clock at
//! 120 bpm, so only a stalled writer drops anything.
//!
//! **What slice B does *not* do, honestly.** Bytes are drained as fast as the
//! writer thread can, so **scheduling** is sample-accurate — the ticks are
//! generated at the right frames by the engine's `clock_out` plugin from the
//! tempo map — while **wire timing** is coarse: a tick hits the port whenever
//! the writer thread gets to it, and the jitter belongs to the OS scheduler
//! and the device. No jitter figures are claimed or measured here; that is the
//! design note's risk section saying what it means. A future slice could
//! timestamp against ALSA's event queue; until then the claim is only the
//! schedule.
//!
//! Conversions (all documented here because the wire has no fractions):
//! - `NoteOn` / `NoteOff`: channel 0 status `0x90`/`0x80`. The engine's pitch
//!   is **semitones from A4** (see `NoteEvent::pitch`), and MIDI note 69 is
//!   A4, so the wire note is the pitch shifted up 69, rounded, clamped into
//!   0..=127 (7-bit). Velocity (0..1) scales to 0..=127 the same way.
//! - `Control`: `0xB0` controller **1** (the modulation wheel) — the event
//!   currency carries a value but no controller index, so the default is
//!   chosen here rather than invented silently; a controller-numbered event is
//!   a later currency change. Value (0..1) scales to 0..=127 as above.
//! - `Clock` → `0xF8`, `Start` → `0xFA`, `Stop` → `0xFC`, `Continue` → `0xFB`
//!   — the MIDI realtime bytes.
//! - `Trigger` has no MIDI-clock representation on this wire and is dropped
//!   (not counted as an overflow: nothing was due).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use engine::{EventSink, ExternalEvent, MidiSink};

use crate::ring::Spsc;

/// The sink's declared id — the same name as the `"midi.out"` context key it
/// arrives under, so a diagnostic names the seam consistently.
pub const MIDI_OUT_ID: &str = "midi.out";

/// The ring's capacity in **messages** (a power of two, as `Spsc` requires).
/// At 24 PPQN a second of playback is `bpm × 24 / 60` messages; even 300 bpm
/// leaves this several minutes deep — it is a stall bound, not a working set.
const RING_MESSAGES: usize = 1 << 10;

/// How long the writer thread parks when the ring is empty. Sub-millisecond:
/// the next tick is due at most a few ms later at any musical tempo, and the
/// park only costs latency, never a drop.
const WRITER_PARK: Duration = Duration::from_micros(500);

/// One outbound MIDI message, wire-ready: status plus up to two data bytes.
/// A fixed-size `Copy` item keeps the ring allocation-free and never splits a
/// message across a ring boundary (a byte ring could).
#[derive(Clone, Copy)]
struct Wire {
    bytes: [u8; 3],
    len: u8,
}

impl Wire {
    fn one(status: u8) -> Self {
        Wire {
            bytes: [status, 0, 0],
            len: 1,
        }
    }

    fn two(status: u8, a: u8, b: u8) -> Self {
        Wire {
            bytes: [status, a, b],
            len: 3,
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

/// The engine pitch (semitones from A4) as a MIDI note number: MIDI note 69 is
/// A4, so the pitch shifts up 69, rounds, and clamps into 0..=127 — a note
/// outside the wire's 7-bit range clamps rather than wraps (a wrapped note is
/// a wrong note a octave away, which is worse than a bounded one).
fn note_number(pitch: f32) -> u8 {
    (pitch + 69.0).round().clamp(0.0, 127.0) as u8
}

/// A 0..1 fraction as a 7-bit MIDI value (velocity, CC value): clamp first,
/// scale, round. MIDI has no fraction of a bit, and an out-of-range input
/// clamps instead of wrapping into a neighboring value.
fn to_7bit(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// One event as wire bytes, or `None` when it has none (`Trigger`).
fn to_wire(event: &ExternalEvent) -> Option<Wire> {
    match *event {
        ExternalEvent::NoteOn {
            pitch, velocity, ..
        } => Some(Wire::two(0x90, note_number(pitch), to_7bit(velocity))),
        ExternalEvent::NoteOff { pitch, .. } => Some(Wire::two(0x80, note_number(pitch), 0)),
        ExternalEvent::Control { value, .. } => Some(Wire::two(0xB0, 1, to_7bit(value))),
        ExternalEvent::Clock { .. } => Some(Wire::one(0xF8)),
        ExternalEvent::Start { .. } => Some(Wire::one(0xFA)),
        ExternalEvent::Stop { .. } => Some(Wire::one(0xFC)),
        ExternalEvent::Continue { .. } => Some(Wire::one(0xFB)),
        ExternalEvent::Trigger { .. } => None,
    }
}

/// The MIDI output ports available, by name — for the human who has to type a
/// `--midi-out` substring. The names are the backend's (ALSA's client:port
/// spellings on Linux).
pub fn ports() -> Result<Vec<String>, String> {
    let out =
        midir::MidiOutput::new("sound-arranger").map_err(|e| format!("midi output init: {e}"))?;
    port_names(&out)
}

fn port_names(out: &midir::MidiOutput) -> Result<Vec<String>, String> {
    out.ports()
        .iter()
        .map(|p| out.port_name(p).map_err(|e| format!("midi port name: {e}")))
        .collect()
}

/// Which of `available` matches `port` (case-insensitive *contains*, so a
/// substring of an ALSA `client:port` name is enough). Pure, and unit-tested
/// without a backend: none → empty, several → all of them, and the caller
/// fails loudly either way rather than guessing.
fn match_ports(available: &[String], port: &str) -> Vec<String> {
    let needle = port.to_lowercase();
    available
        .iter()
        .filter(|name| name.to_lowercase().contains(&needle))
        .cloned()
        .collect()
}

/// A real MIDI output: the `MidiSink` the host provides under `"midi.out"`
/// when the process asked for one. Dropping it stops the writer thread and
/// closes the port.
pub struct MidiOut {
    ring: Arc<Spsc<Wire>>,
    dropped_events: Arc<AtomicU64>,
    dropped_bytes: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
}

impl MidiOut {
    /// Open a MIDI output whose port name **contains** `port`
    /// (case-insensitive). Zero or several matches fail loudly and list what
    /// *is* there — guessing a port silently sends the clock nowhere.
    pub fn open(port: &str) -> Result<Self, String> {
        let out = midir::MidiOutput::new("sound-arranger")
            .map_err(|e| format!("midi output init: {e}"))?;
        let ports = out.ports();
        let named: Vec<(String, &midir::MidiOutputPort)> = ports
            .iter()
            .map(|p| {
                Ok((
                    out.port_name(p)
                        .map_err(|e| format!("midi port name: {e}"))?,
                    p,
                ))
            })
            .collect::<Result<_, String>>()?;
        let names: Vec<String> = named.iter().map(|(name, _)| name.clone()).collect();
        let mut matches = match_ports(&names, port);
        let name = match matches.len() {
            0 => {
                return Err(format!(
                    "no MIDI output matching '{port}' (available: {})",
                    if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(", ")
                    }
                ));
            }
            1 => matches.pop().expect("len 1"),
            n => {
                return Err(format!(
                    "{n} MIDI outputs match '{port}' — be more specific: {}",
                    matches.join(", ")
                ));
            }
        };
        let target = named
            .iter()
            .find(|(candidate, _)| candidate == &name)
            .expect("the matched name came from this list")
            .1;
        let conn = out
            .connect(target, "clock-out")
            .map_err(|e| format!("connect MIDI output '{name}': {e}"))?;
        Ok(Self::spawn(conn, RING_MESSAGES))
    }

    /// Start the writer thread that owns the connection and drains the ring.
    fn spawn(conn: midir::MidiOutputConnection, capacity: usize) -> Self {
        let ring = Arc::new(Spsc::<Wire>::new(capacity));
        let stop = Arc::new(AtomicBool::new(false));
        let dropped_events = Arc::new(AtomicU64::new(0));
        let dropped_bytes = Arc::new(AtomicU64::new(0));
        let (ring2, stop2) = (ring.clone(), stop.clone());
        let writer = std::thread::Builder::new()
            .name("media-midi-out".into())
            .spawn(move || {
                let mut conn = conn;
                while !stop2.load(Ordering::Relaxed) {
                    let mut sent = false;
                    while let Some(msg) = ring2.try_pop() {
                        // A failed send is printed, never propagated onto the
                        // render path — the render side only ever pushed.
                        if let Err(e) = conn.send(msg.as_slice()) {
                            eprintln!("media-midi-out: send: {e}");
                        }
                        sent = true;
                    }
                    if !sent {
                        std::thread::sleep(WRITER_PARK);
                    }
                }
            })
            .expect("the midi-out writer thread spawns");
        MidiOut {
            ring,
            dropped_events,
            dropped_bytes,
            stop,
            writer: Some(writer),
        }
    }

    /// How many events were dropped because the ring was full — the loud
    /// bound, read by the host (a nonzero count means the writer stalled).
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    /// How many **wire bytes** those dropped events carried.
    pub fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes.load(Ordering::Relaxed)
    }
}

impl Drop for MidiOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

impl EventSink for MidiOut {
    fn id(&self) -> &'static str {
        MIDI_OUT_ID
    }

    /// The render path's only entry point: convert, push, count. No lock, no
    /// allocation, no device — a full ring drops and counts.
    fn send(&mut self, events: &[ExternalEvent], _frame: u64) {
        for event in events {
            let Some(msg) = to_wire(event) else {
                continue;
            };
            if self.ring.try_push(msg) {
                continue;
            }
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
            self.dropped_bytes
                .fetch_add(msg.len as u64, Ordering::Relaxed);
        }
    }
}

impl MidiSink for MidiOut {}

#[cfg(test)]
mod tests {
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
}
