//! P1.3.4 — the reference host speaks the clip-arrangement commands: `Pool` (point
//! the host at the pool) and `Arrange` (a logged ACID op) build the arrangement
//! value. The value is a pure reconstruction of the logged op stream, so a fresh
//! host replays it byte-identically. (The live audio wiring of the arranger
//! nodes into the mixer is a documented follow-up — the graph's forward-order
//! constraint requires the shared-state reconcile; see the P1.3.4 note.)

use std::path::{Path, PathBuf};

use host::{HostCommand, run_script};
use media::{ArrangeOp, Clip, WavWriter};

const SR: u32 = 48_000;

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("host-arranger-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_source(dir: &Path, stem: &str, frames: u64) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), SR, 1).unwrap();
    let mut buf = vec![0.0f32; 4096];
    let mut i = 0u64;
    while i < frames {
        let n = buf.len().min((frames - i) as usize);
        buf[..n].fill(0.5);
        w.write(&buf[..n]).unwrap();
        i += n as u64;
    }
    w.finalize().unwrap();
}

fn clip(id: &str, at: u64, len: u64) -> Clip {
    Clip {
        id: id.into(),
        source: "s1".into(),
        src_start: 0,
        src_len: len,
        at_frame: at,
        fade_in: 0,
        fade_out: 0,
        gain: 1.0,
        loop_len: None,
    }
}

fn arrange_ops() -> Vec<HostCommand> {
    let arrange = |op: ArrangeOp| HostCommand::Arrange { op, at_frame: Some(0) };
    vec![
        arrange(ArrangeOp::AddTrack { track: "t0".into() }),
        arrange(ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 4000) }),
        arrange(ArrangeOp::AddTrack { track: "t1".into() }),
        arrange(ArrangeOp::AddClip { track: "t1".into(), clip: clip("c1", 500, 1500) }),
        arrange(ArrangeOp::SetClipFade { track: "t1".into(), clip: "c1".into(), fade_in: 64, fade_out: 128 }),
    ]
}

#[test]
fn arrange_commands_build_the_logged_value_and_replay_identically() {
    let pool = tmp_dir("pool");
    write_source(&pool, "s1", 8000);

    let script: Vec<HostCommand> = {
        let mut s = vec![
            HostCommand::Mount { plugin: "mixer", params: vec![("channels", 2.0)], at_frame: Some(0) },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.extend(arrange_ops());
        s
    };

    // live: the Arrange commands build the arrangement value.
    let sess = run_script(&script).unwrap();
    let value = sess.arrangement();
    assert_eq!(value.tracks.len(), 2, "two tracks built by the arrange commands");
    assert_eq!(value.tracks[0].clips.len(), 1, "t0 has one clip");
    assert_eq!(value.tracks[0].clips[0].src_len, 4000);
    assert_eq!(value.tracks[1].clips[0].fade_out, 128);
    assert!(sess.event_count() > 0, "the arrangement ops are logged");

    // replay: a fresh host, same script, identical value (byte-identical log reconstructs it).
    let sess2 = run_script(&script).unwrap();
    assert_eq!(value, sess2.arrangement(), "replay must reconstruct the identical value");

    let _ = std::fs::remove_dir_all(&pool);
}

#[test]
fn arrange_without_pool_is_refused() {
    let pool = tmp_dir("refused");
    write_source(&pool, "s1", 100);
    // No `Pool` command first → the arrange command fails loudly.
    let script = vec![
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 1.0)], at_frame: Some(0) },
        HostCommand::Arrange { op: ArrangeOp::AddTrack { track: "t0".into() }, at_frame: Some(0) },
    ];
    assert!(run_script(&script).is_err(), "arrange requires set_pool first");
    let _ = std::fs::remove_dir_all(&pool);
}

#[test]
fn arrangement_bounces_audio_and_replays_byte_identically() {
    let pool = tmp_dir("audio");
    write_source(&pool, "s1", 8000);
    let out_dir = tmp_dir("out");
    let a = out_dir.join("a.wav");
    let b = out_dir.join("b.wav");

    let script = |out: &Path| -> Vec<HostCommand> {
        let mut s = vec![
            HostCommand::Mount { plugin: "mixer", params: vec![("channels", 2.0)], at_frame: Some(0) },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.extend(arrange_ops());
        s.push(HostCommand::Bounce { frames: 6000, path: out.to_path_buf() });
        s
    };

    let sess = run_script(&script(&a)).unwrap();
    let frames_a = std::fs::read(&a).unwrap();
    // the arrangement actually plays (not a silent bounce)
    let mut r = media::WavReader::open(&a).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(audio[..n].iter().any(|s| s.abs() > 1e-4), "the arrangement must produce audio");
    assert_eq!(sess.arrangement().tracks.len(), 2);

    // replay: a fresh host, same script → byte-identical bounce.
    let _ = run_script(&script(&b)).unwrap();
    let frames_b = std::fs::read(&b).unwrap();
    assert_eq!(frames_a, frames_b, "replay must bounce byte-identically");

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}
