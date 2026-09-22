//! Media-command logging: `pool`/`play`/`splice` (and the `bounce` record) are
//! carried by the one session log as `Event::Arrangement` ops, so media
//! determinism is not a parallel command seam. Plus the two Stage-0 replay
//! correctness fixes.

use std::path::{Path, PathBuf};

use engine::Event;
use host::{HostCommand, HostSession};

const SR: u32 = 48_000;

fn unique_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sa-media-log-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

fn write_tone(path: &Path, freq: f32, frames: u64) {
    let mut w = media::WavWriter::create(path, SR, 1).expect("wav writer");
    let buf: Vec<f32> = (0..frames)
        .map(|i| (std::f32::consts::TAU * freq * i as f32 / SR as f32).sin() * 0.5)
        .collect();
    w.write(&buf).expect("write");
    w.finalize().expect("finalize");
}

fn arranges(session: &HostSession) -> Vec<&'static str> {
    session
        .log()
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::Arrangement { op, .. } => Some(*op),
            _ => None,
        })
        .collect()
}

#[test]
fn media_commands_are_logged_in_the_session_log() {
    let dir = unique_dir("logged");
    let clip = dir.join("clip.wav");
    write_tone(&clip, 440.0, 2_000);
    let out = dir.join("out.wav");

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: None,
    })
    .expect("mount mixer");
    s.execute(&HostCommand::Pool { dir: dir.clone() })
        .expect("pool");
    s.execute(&HostCommand::Play {
        clip: media::ClipRef::whole(&clip).expect("clip"),
        channel: 0,
        at_frame: None,
    })
    .expect("play");
    s.execute(&HostCommand::Bounce {
        frames: 512,
        path: out.clone(),
    })
    .expect("bounce");

    let ops = arranges(&s);
    assert!(ops.contains(&"MediaPool"), "pool must be logged: {ops:?}");
    assert!(ops.contains(&"MediaPlay"), "play must be logged: {ops:?}");
    assert!(
        ops.contains(&"MediaBounce"),
        "bounce records must be logged: {ops:?}"
    );
    // The log and the host's media counter agree (one record per command).
    assert_eq!(ops.len(), s.media_command_count());
}

#[test]
fn a_refused_media_command_is_never_logged() {
    let dir = unique_dir("refused");
    let clip = dir.join("clip.wav");
    write_tone(&clip, 440.0, 1_000);

    let mut s = HostSession::new();
    // No mixer mounted: `play` is refused.
    let err = s
        .execute(&HostCommand::Play {
            clip: media::ClipRef::whole(&clip).expect("clip"),
            channel: 0,
            at_frame: None,
        })
        .expect_err("play without a mixer must be refused");
    assert!(err.contains("mixer"), "got: {err}");
    assert!(
        !arranges(&s).contains(&"MediaPlay"),
        "a refused media command is never logged"
    );
}

#[test]
fn replay_rejects_a_media_op_with_no_registered_handler() {
    // A log carrying a media op replayed into an engine without the media
    // handlers must fail loud, not schedule an op that is silently dropped at
    // apply (which would produce a session with no media state and no error).
    let mut log = engine::SessionLog::new();
    log.push(Event::Arrangement {
        op: "MediaPlay",
        fields: vec![
            ("path", engine::Value::Str("x.wav")),
            ("start", engine::Value::U64(0)),
            ("len", engine::Value::U64(1)),
            ("channel", engine::Value::U32(0)),
        ],
        at_frame: 0,
    });
    let mut engine = engine::Engine::new(SR, 120.0, 4);
    let err = engine
        .replay_from(&log)
        .expect_err("a handler-less op must be refused");
    assert!(err.contains("no handler"), "got: {err}");
}

#[test]
fn backward_seek_lands_before_a_later_state_change() {
    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: None,
    })
    .expect("mount mixer");
    // A state command that takes effect at frame 5000 (it renders the clock there).
    s.execute(&HostCommand::SetParam {
        plugin: "mixer",
        param: "ch0.gain",
        value: 0.5,
        at_frame: Some(5_000),
    })
    .expect("set_param");
    assert_eq!(s.position().frame, 5_000);

    // Seeking back before it must land at the target, not be a no-op forced
    // forward by the later command's frame.
    s.execute(&HostCommand::TransportSeek { frame: 1_000 })
        .expect("seek");
    assert_eq!(
        s.position().frame,
        1_000,
        "a backward seek must land at its target"
    );
}
