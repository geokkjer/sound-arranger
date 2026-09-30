use super::*;
use engine::plugins::SharedMidiSink;
use std::sync::Mutex;

/// The recordings one `FakeSink` appends to and its test reads.
type Sends = Arc<Mutex<Vec<(u64, Vec<ExternalEvent>)>>>;

/// A fake `MidiSink` recording `(frame, events)` per send — no device. It
/// arrives through the same `"midi.out"` context key the real device sink
/// uses: that is the seam, and why the test needs no hardware.
struct FakeSink {
    sends: Sends,
}

/// A shared fake sink (already inside the session's **slot** shape) plus
/// the handle the assertions read.
fn fake_sink() -> (SharedMidiSink, Sends) {
    let sends: Sends = Arc::new(Mutex::new(Vec::new()));
    let sink: SharedMidiSink = Arc::new(Mutex::new(Some(Box::new(FakeSink {
        sends: sends.clone(),
    }))));
    (sink, sends)
}

impl engine::EventSink for FakeSink {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn send(&mut self, events: &[ExternalEvent], frame: u64) {
        self.sends.lock().unwrap().push((frame, events.to_vec()));
    }
}

impl engine::MidiSink for FakeSink {}

/// The absolute frames of every `Clock` event, in send order.
fn clock_frames(sends: &Sends) -> Vec<u64> {
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
fn transport_events(sends: &Sends) -> Vec<(u64, &'static str)> {
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

fn mount_clock_out(session: &mut HostSession) {
    session
        .execute(&HostCommand::Mount {
            plugin: "clock_out",
            params: Vec::new(),
            at_frame: Some(0),
        })
        .expect("clock_out mounts");
}

/// A one-clip arrangement's op on track `t0` (the export test's material);
/// local copies of the `tests` module's helpers, which this module does not
/// share scope with.
fn add_clip(id: &str, at: u64) -> media::ArrangeOp {
    media::ArrangeOp::AddClip {
        track: "t0".into(),
        clip: media::Clip {
            reversed: false,
            id: id.into(),
            name: None,
            source: "s1".into(),
            src_start: 0,
            src_len: 4_800,
            at_frame: at,
            fade_in: 0,
            fade_out: 0,
            gain: 1.0,
            loop_len: None,
        },
    }
}

fn write_tone(dir: &std::path::Path, id: &str, frames: usize, rate: u32) {
    let path = dir.join(format!("{id}.wav"));
    let mut w = media::WavWriter::create(&path, rate, 1).expect("wav writer");
    let samples: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    w.write(&samples).expect("write tone");
    w.finalize().expect("finalize tone");
}

/// One second of playback at 120 bpm 48 kHz sends 48 ticks at frames
/// 0, 1000, … 47000 — the tempo map's own answer, to the sample — and a
/// play from frame 0 reaches the wire as a `Start`, a resume from a
/// non-zero position as a `Continue`, a stop as a `Stop`, each at the
/// frame the command took effect.
#[test]
fn clock_out_sends_ticks_at_the_tempo_maps_frames_and_maps_transport() {
    let (sink, sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    mount_clock_out(&mut session);
    session.execute(&HostCommand::TransportPlay).expect("play");
    session.render(48_000).expect("render one second");
    let expected: Vec<u64> = (0..48u64).map(|i| i * 1000).collect();
    assert_eq!(clock_frames(&sends), expected);
    assert_eq!(transport_events(&sends), vec![(0, "Start")]);

    // Stop at frame 48 000, then play again without rendering in between:
    // a resume from a non-zero position is a Continue. Both commands take
    // effect at the same frame, so both flush at offset 0 of the next
    // block, in log order.
    session.execute(&HostCommand::TransportStop).expect("stop");
    session
        .execute(&HostCommand::TransportPlay)
        .expect("resume");
    session.render(48_000).expect("render the second second");
    assert_eq!(
        transport_events(&sends),
        vec![(0, "Start"), (48_000, "Stop"), (48_000, "Continue"),]
    );
    let second_half = &clock_frames(&sends)[48..];
    let expected: Vec<u64> = (48..96u64).map(|i| i * 1000).collect();
    assert_eq!(second_half, &expected[..]);
}

/// A session with **no** sink mounts, renders, and replays byte-identically
/// — and the identical script *with* a sink renders the same audio: whether
/// gear is attached cannot change the mix (the purity rule), it only drives
/// the seam.
#[test]
fn a_session_without_a_sink_mounts_renders_and_replays_byte_identically() {
    let script = |session: &mut HostSession| {
        mount_clock_out(session);
        session.execute(&HostCommand::TransportPlay).expect("play");
    };
    let mut silent = HostSession::new_at_with(48_000, None, None);
    script(&mut silent);
    let first = silent.render(48_000).expect("render");

    // A replay (a seek rebuilds and re-renders from the state history)
    // reproduces the same bytes — silently, because there is no sink.
    silent
        .execute(&HostCommand::TransportSeek { frame: 0 })
        .expect("replay");
    let replayed = silent.render(48_000).expect("render after replay");
    assert_eq!(first, replayed, "the replay is byte-identical");

    let (sink, sends) = fake_sink();
    let mut driven = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    script(&mut driven);
    let driven_audio = driven.render(48_000).expect("render with a sink");
    assert_eq!(first, driven_audio, "the sink cannot change the audio");
    assert!(
        !clock_frames(&sends).is_empty(),
        "the driven session actually sent"
    );
}

/// The overflow counter reaches the host status through the plugin's
/// `"clock_out.overflows"` context service — the accessor the snapshot's
/// publish pass uses. 1e6 bpm is the cheap deterministic overflow (one tick
/// every ~0.12 frames puts thousands due inside one 512-frame block, far
/// past the plugin's 64-event cap); the point asserted is the counter, not
/// the tempo.
#[test]
fn the_overflow_counter_reaches_the_host_status() {
    let (sink, _sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    session
        .execute(&HostCommand::SetTempo {
            bpm: 1.0e6,
            beats_per_bar: 4,
            at_frame: None,
        })
        .expect("the tempo applies");
    mount_clock_out(&mut session);
    session.render(512).expect("render one block");
    let status = session.midi_status();
    assert_eq!(status.port.as_deref(), Some("fake"), "the port is reported");
    assert!(
        status.overflows > 0,
        "the block exceeded the cap: {status:?}"
    );

    // No clock_out mounted: zero, never an error — the counter's absence is
    // the legitimate "plugin not mounted" state.
    assert_eq!(HostSession::new().midi_status().overflows, 0);
}

/// **The test the detach/refill fix exists for**: a seek rebuilds the
/// session and renders it, and the reconstruction must drive no gear —
/// nothing is sent across a rebuild (the seek-and-rebuild decision) —
/// while the live session sends ticks again on its very next render.
#[test]
fn a_seek_sends_nothing_while_it_rebuilds_and_resumes_afterwards() {
    let (sink, sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    mount_clock_out(&mut session);
    session.execute(&HostCommand::TransportPlay).expect("play");
    session.render(48_000).expect("render one second");
    let frames_before = clock_frames(&sends);
    assert!(!frames_before.is_empty(), "the live session drove the gear");

    // Seek to 24 000 (< the warm-up bound, so this is the full rebuild:
    // replay the history and render the timeline to the target) — with the
    // slot detached, none of that reconstruction render may reach the sink.
    session
        .execute(&HostCommand::TransportSeek { frame: 24_000 })
        .expect("seek");
    assert_eq!(
        clock_frames(&sends),
        frames_before,
        "the rebuild render sent nothing"
    );
    assert_eq!(
        transport_events(&sends),
        vec![(0, "Start")],
        "no transport command crossed the rebuild either — the seek's re-anchor is \
         queued in the rebuilt tap and goes out with the first render after it"
    );

    // The device is back in the shared slot: the live render records
    // ticks again, all at or past the seek target, and the re-anchor
    // reaches the wire at the target frame — as the `Continue` a non-zero
    // target gets, pinned in the next test.
    session.render(48_000).expect("render after the seek");
    let all = clock_frames(&sends);
    let resumed = &all[frames_before.len()..];
    assert!(
        !resumed.is_empty() && resumed.iter().all(|&f| f >= 24_000),
        "ticks again after the seek, from the target onward: {resumed:?}"
    );
}

/// **A seek while playing re-anchors the follower.** `TransportPlay` is an
/// *action*, so it is not state, never enters the history, and a rebuild
/// never re-applies it — and the rebuilt session gets a brand-new tap. So
/// without the re-anchor the adopted session is still `playing`, its ticks
/// resume at the new position, and a follower is never told the position
/// moved at all.
///
/// The message is the one a `play` at that frame would send, and the
/// distinction is pinned here because it is the whole point of the two: at
/// 24 000 it is a **`Continue`**, because `Start` on this wire means "return
/// to song start" (the rule the `TransportPlay` arm already follows), and
/// sending it here would tell gear to jump to its own top while the session
/// sits half a second in. A seek to frame **0** does get the `Start` that
/// frame deserves — the same wire message a play from the top sends.
#[test]
fn a_seek_while_playing_re_anchors_the_follower_at_the_target() {
    let (sink, sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    mount_clock_out(&mut session);
    session.execute(&HostCommand::TransportPlay).expect("play");
    session.render(48_000).expect("render one second");
    assert_eq!(
        transport_events(&sends),
        vec![(0, "Start")],
        "the play reached the wire once"
    );

    // Seek while playing: the rebuild is silent (nothing crosses a rebuild),
    // and the re-anchor is due at the target — which is where the clock now
    // stands, so it flushes at offset 0 of the first block after the rebuild.
    session
        .execute(&HostCommand::TransportSeek { frame: 24_000 })
        .expect("seek");
    assert_eq!(session.position().frame, 24_000, "the playhead moved");
    assert!(session.is_playing(), "and the transport kept running");
    assert_eq!(
        transport_events(&sends),
        vec![(0, "Start")],
        "the rebuild itself sent nothing"
    );
    session
        .render(512)
        .expect("render one block after the seek");
    assert_eq!(
        transport_events(&sends),
        vec![(0, "Start"), (24_000, "Continue")],
        "a seek to a non-zero frame re-anchors with Continue: `Start` would mean \
         'return to song start', which 24 000 is not"
    );

    // A seek back to the top re-anchors too, and lands on the same wire
    // message a play from frame 0 sends — one `Start` at frame 0, not two.
    // This is the case that keeps the `frame == 0` half of the rule pinned:
    // the same feed that answered 24 000 with `Continue` answers 0 with
    // `Start`, because there the two messages finally agree.
    session
        .execute(&HostCommand::TransportSeek { frame: 0 })
        .expect("seek home");
    session
        .render(512)
        .expect("render one block after seeking home");
    assert_eq!(
        transport_events(&sends),
        vec![(0, "Start"), (24_000, "Continue"), (0, "Start")],
        "a seek to the top re-anchors with Start: frame 0 is the one frame where \
         'return to song start' and the session's position are the same statement"
    );
}

/// The re-anchor belongs to a **seek that moved a playing transport**. A
/// stopped transport has already been stopped on the wire by the `Stop` the
/// old session sent, and a rebuild is not what stopped it, so a seek then
/// re-anchors nothing — a `Start` there would be a message about a follower
/// that is not running.
#[test]
fn a_seek_while_stopped_re_anchors_nothing() {
    let (sink, sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    mount_clock_out(&mut session);
    session.execute(&HostCommand::TransportPlay).expect("play");
    session.render(48_000).expect("render one second");
    session.execute(&HostCommand::TransportStop).expect("stop");
    session.render(512).expect("render past the stop");
    let before = transport_events(&sends);
    assert_eq!(before, vec![(0, "Start"), (48_000, "Stop")]);

    // A seek while stopped moves the playhead and re-anchores nothing.
    session
        .execute(&HostCommand::TransportSeek { frame: 24_000 })
        .expect("seek while stopped");
    session.render(512).expect("render after the seek");
    assert_eq!(
        transport_events(&sends),
        before,
        "a stopped transport re-anchors nothing"
    );
}

/// …and it belongs to a **seek** alone. Undo and redo rebuild *at the current
/// frame*, over an arrangement edit, which cannot move a tick: the follower
/// is still in phase across the edit, so a re-anchor there would put a
/// message on the wire about nothing having moved.
#[test]
fn an_undo_while_playing_re_anchors_nothing() {
    let root = std::env::temp_dir().join(format!("host-undo-midi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let (sink, sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    session
        .execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
    mount_clock_out(&mut session);
    session
        .execute(&HostCommand::Pool { dir: pool })
        .expect("pool");
    session
        .execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
    session.execute(&HostCommand::TransportPlay).expect("play");
    session.render(48_000).expect("render one second");
    let before = transport_events(&sends);
    assert_eq!(before, vec![(0, "Start")]);

    // An arrangement edit, then its undo: both rebuild the session, and
    // neither moves the playhead (it is at 48 000 throughout).
    session
        .execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
    assert!(session.undo().expect("undo"), "an edit was undone");
    assert_eq!(session.position().frame, 48_000, "the playhead stayed");
    session.render(512).expect("render after the undo");
    assert_eq!(
        transport_events(&sends),
        before,
        "an undo at the current frame re-anchors nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The export clone renders offline: it must send nothing, while the sink
/// slot is detached — and the live session sends again afterwards.
#[test]
fn an_export_clone_sends_nothing() {
    let root = std::env::temp_dir().join(format!("host-export-midi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let (sink, sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    session
        .execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
    mount_clock_out(&mut session);
    session
        .execute(&HostCommand::Pool { dir: pool })
        .expect("pool");
    session
        .execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
    session
        .execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
    // Render once so the clock_out node is mounted and has driven the gear
    // (mounts apply on the next render, so nothing has been sent yet).
    session.render(48_000).expect("render one second");
    let frames_before = clock_frames(&sends);
    assert!(!frames_before.is_empty(), "the live session drove the gear");

    session
        .execute(&HostCommand::Export {
            path: root.join("mix.wav"),
            format: ExportFormat::F32,
        })
        .expect("export");
    assert_eq!(
        clock_frames(&sends),
        frames_before,
        "the export clone sent nothing"
    );

    // The slot is refilled after the export: gear is driven again.
    session.render(48_000).expect("render after the export");
    assert!(
        clock_frames(&sends).len() > frames_before.len(),
        "ticks again after the export"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **The bounce is the other command that exists to write a file**, so it
/// detaches the sink slot across its offline render for the reason the export
/// path gives: a render that produces a file drives no gear. Pinned on the
/// ticks of the bounce **and of its drain tail** (drain blocks walk the node's
/// schedule exactly as timeline blocks do), on the queued transport message an
/// offline render would otherwise flush, and on the refill — a bounce that
/// emptied the slot for good would silence a live session.
#[test]
fn a_bounce_sends_nothing_and_the_live_session_resumes_afterwards() {
    let root = std::env::temp_dir().join(format!("host-bounce-midi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let (sink, sends) = fake_sink();
    let mut session = HostSession::new_at_with(48_000, Some(sink), Some("fake".into()));
    session
        .execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
    mount_clock_out(&mut session);
    session
        .execute(&HostCommand::Pool { dir: pool })
        .expect("pool");
    session
        .execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
    session
        .execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
    // Render once so the clock_out node is mounted and has driven the gear
    // (mounts apply on the next render, so nothing has been sent yet).
    session.execute(&HostCommand::TransportPlay).expect("play");
    session.render(48_000).expect("render one second");
    let frames_before = clock_frames(&sends);
    let transport_before = transport_events(&sends);
    assert!(!frames_before.is_empty(), "the live session drove the gear");
    assert_eq!(transport_before, vec![(0, "Start")]);

    // A resume at 48 000 is a `Continue`, and nothing has rendered since it
    // was logged, so the message is still sitting in the tap waiting for the
    // next block. The bounce would be that next block — and an offline render
    // is no more entitled to put a transport command on the wire than to put
    // ticks there.
    session
        .execute(&HostCommand::TransportPlay)
        .expect("resume");
    assert_eq!(
        transport_events(&sends),
        transport_before,
        "the resume is queued, not yet on the wire"
    );

    let bounced = root.join("bounce.wav");
    session
        .execute(&HostCommand::Bounce {
            frames: 48_000,
            path: bounced.clone(),
        })
        .expect("bounce");
    assert!(
        bounced.exists(),
        "the bounce still wrote its file — detaching is not skipping the work"
    );
    assert_eq!(
        clock_frames(&sends),
        frames_before,
        "the bounce sent no ticks: not for its timeline, not for its drain tail"
    );
    assert_eq!(
        transport_events(&sends),
        transport_before,
        "the bounce flushed nothing out of the transport tap"
    );

    // The slot is refilled after the bounce: gear is driven again, and the
    // queued `Continue` goes out with the live render rather than having been
    // swallowed by the bounce.
    session.render(48_000).expect("render after the bounce");
    assert!(
        clock_frames(&sends).len() > frames_before.len(),
        "ticks again after the bounce"
    );
    let after = transport_events(&sends);
    assert_eq!(
        after.len(),
        2,
        "the queued resume reached the wire after the bounce, not during it: {after:?}"
    );
    assert_eq!(
        after[1].1, "Continue",
        "and it is the resume's own message, a Continue from a non-zero frame"
    );
    let _ = std::fs::remove_dir_all(&root);
}
