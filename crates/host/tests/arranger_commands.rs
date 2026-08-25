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

/// A **position-revealing** source: sample k = (k % period) / period, so a
/// reader that starts at the wrong source offset (a mid-play rebuild restarting
/// at source[0]) is detectable — a constant source hides it. `period` sets the
/// ramp length.
fn write_ramp(dir: &Path, stem: &str, frames: u64, period: u64) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), SR, 1).unwrap();
    let mut buf = vec![0.0f32; 4096];
    let mut i = 0u64;
    while i < frames {
        let n = buf.len().min((frames - i) as usize);
        for (k, s) in buf[..n].iter_mut().enumerate() {
            *s = ((i + k as u64) % period.max(1)) as f32 / period.max(1) as f32;
        }
        w.write(&buf[..n]).unwrap();
        i += n as u64;
    }
    w.finalize().unwrap();
}

/// The ramp source value at absolute frame `f` (a `write_ramp`-derived ramp).
fn source_value(f: u64, period: u64) -> f32 {
    (f % period.max(1)) as f32 / period.max(1) as f32
}

/// A long source so a removed track's clip is still playing when we re-render:
/// t0 spans [0, 8000), t1 spans [0, 1000). After a first render of 4000 frames
/// (t0+t1 then t0 alone), `RemoveTrack t0`, then a second render of the *next*
/// 4000 frames. With the ghost-playback bug, t0's `ArrangerNode` stays mounted
/// and its clip (spanning into [4000,8000)) keeps playing — the second render
/// would be non-silent. Fixed: the node is retired, so it is silent.
#[test]
fn removed_track_does_not_ghost_audio_on_rewire() {
    let pool = tmp_dir("ghost");
    write_ramp(&pool, "s1", 8000, 257);
    let out_dir = tmp_dir("ghostout");
    let a = out_dir.join("a.wav");

    let script: Vec<HostCommand> = {
        let mut s = vec![
            HostCommand::Mount { plugin: "mixer", params: vec![("channels", 2.0)], at_frame: Some(0) },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.push(HostCommand::Arrange { op: ArrangeOp::AddTrack { track: "t0".into() }, at_frame: Some(0) });
        s.push(HostCommand::Arrange { op: ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 8000) }, at_frame: Some(0) });
        s.push(HostCommand::Arrange { op: ArrangeOp::AddTrack { track: "t1".into() }, at_frame: Some(0) });
        s.push(HostCommand::Arrange { op: ArrangeOp::AddClip { track: "t1".into(), clip: clip("c1", 0, 1000) }, at_frame: Some(0) });
        s.push(HostCommand::Bounce { frames: 4000, path: a.clone() });
        // remove t0, then render the *next* 4000 frames — t0's clip spans
        // [0,8000), so it would still be playing here if its node were mounted.
        s.push(HostCommand::Arrange { op: ArrangeOp::RemoveTrack { track: "t0".into() }, at_frame: Some(4000) });
        s.push(HostCommand::Bounce { frames: 4000, path: out_dir.join("b.wav") });
        s
    };

    let _ = run_script(&script).unwrap();

    // The first render (frames 0..4000) must be non-silent (t0 is playing).
    let mut r = media::WavReader::open(&a).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(audio[..n].iter().any(|s| s.abs() > 1e-4), "t0 must be audible in the first render");

    // The second render (frames 4000..8000) must be silent: t0 is gone and t1's
    // clip ended at 1000. If t0's node ghost-plays, these frames are non-silent.
    let b = out_dir.join("b.wav");
    let mut r = media::WavReader::open(&b).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().all(|s| s.abs() <= 1e-4),
        "removed track must not ghost-play after a rewire (got non-silent frames)"
    );

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// A mid-play **edit** must not restart a clip from its source start. We render
/// 4000 frames of a ramp-valued clip, then change the already-wired track's clip
/// gain and render the next 4000 frames. The second render's sample at frame
/// `4000 + k` must equal `source[4000 + k] * gain` — i.e. the `k`-th source
/// frame *after* the transport position, NOT the clip's beginning. With the
/// restart bug the rebuilt reader starts at source[0], so it plays
/// `source[k]` (or panics in debug on a clip > ring capacity).
#[test]
fn edit_to_a_wired_track_continues_the_clip_not_restarts_it() {
    let pool = tmp_dir("editcontinue");
    let period = 257u64;
    write_ramp(&pool, "s1", 8000, period);
    let out_dir = tmp_dir("editcontinueout");

    // build, render [0,4000), edit gain to g, render [4000,8000).
    let script = |gain: f32, tag: &str| -> Vec<HostCommand> {
        let mut s = vec![
            HostCommand::Mount { plugin: "mixer", params: vec![("channels", 2.0)], at_frame: Some(0) },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.push(HostCommand::Arrange { op: ArrangeOp::AddTrack { track: "t0".into() }, at_frame: Some(0) });
        s.push(HostCommand::Arrange { op: ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 8000) }, at_frame: Some(0) });
        s.push(HostCommand::Bounce { frames: 4000, path: out_dir.join(format!("{tag}-a.wav")) });
        s.push(HostCommand::Arrange { op: ArrangeOp::SetClipGain { track: "t0".into(), clip: "c0".into(), gain }, at_frame: Some(4000) });
        s.push(HostCommand::Bounce { frames: 4000, path: out_dir.join(format!("{tag}-b.wav")) });
        s
    };

    let _ = run_script(&script(1.0, "r")).unwrap();

    // The second render's frame `4000 + k` (mid-block, away from the 0.0 ramp
    // zero-crossing) must be source[4000 + k] * 1.0 — i.e. the source region
    // *after* the first render, not the clip start.
    let mut r = media::WavReader::open(&out_dir.join("r-b.wav")).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(n > 500, "second render must fill the buffer (got {n})");
    for &f in &[4000u64 + 257, 4000 + 513, 4000 + 1024] {
        let expected = source_value(f, period) * 1.0;
        let got = audio[(f - 4000) as usize];
        assert!(
            (got - expected).abs() < 1e-4,
            "frame {f} must play source region {f} (got {got}, want {expected}) — a restart would play source[{}]",
            f % period
        );
    }

    // And the gain reaches audio: g=0.5 halves the region, so the same frame is
    // half as loud as the g=1.0 run (16-bit bounce clips >1.0, so we compare a
    // halving rather than a doubling).
    let _ = run_script(&script(0.5, "h")).unwrap();
    let mut r = media::WavReader::open(&out_dir.join("h-b.wav")).unwrap();
    let mut half = vec![0.0f32; r.total_frames() as usize];
    let _n = r.read_into(&mut half);
    let f = 4000 + 513;
    assert!(
        (half[(f - 4000) as usize] - 0.5 * audio[(f - 4000) as usize]).abs() < 1e-4,
        "gain 0.5 must halve the wired track's region ({} vs {})",
        half[(f - 4000) as usize],
        audio[(f - 4000) as usize]
    );

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// A bounce → edit → re-bounce session must be **byte-identical** across two
/// fresh runs (the reconcile is a deterministic function of the value), and the
/// re-render path — the one that the single-bounce replay tests never touch —
/// is the change under test.
#[test]
fn edit_then_rewire_replays_byte_identically() {
    let pool = tmp_dir("replayedit");
    write_ramp(&pool, "s1", 8000, 257);
    let out_dir = tmp_dir("replayeditout");

    let script = |out: &Path, tag: &str| -> Vec<HostCommand> {
        let mut s = vec![
            HostCommand::Mount { plugin: "mixer", params: vec![("channels", 2.0)], at_frame: Some(0) },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.push(HostCommand::Arrange { op: ArrangeOp::AddTrack { track: "t0".into() }, at_frame: Some(0) });
        s.push(HostCommand::Arrange { op: ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 8000) }, at_frame: Some(0) });
        s.push(HostCommand::Bounce { frames: 4000, path: out_dir.join(format!("{tag}-a.wav")) });
        s.push(HostCommand::Arrange { op: ArrangeOp::SetClipGain { track: "t0".into(), clip: "c0".into(), gain: 0.7 }, at_frame: Some(4000) });
        s.push(HostCommand::Bounce { frames: 4000, path: out.to_path_buf() });
        s
    };

    let a = out_dir.join("a-b.wav");
    let b = out_dir.join("b-b.wav");
    let _ = run_script(&script(&a, "a")).unwrap();
    let _ = run_script(&script(&b, "b")).unwrap();
    let fa = std::fs::read(&a).unwrap();
    let fb = std::fs::read(&b).unwrap();
    // The rewire path must be deterministic across fresh sessions.
    assert!(fa.len() > 44, "second bounce must be written");
    assert_eq!(fa, fb, "edit + rewire must replay byte-identically");

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
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
fn bounce_over_byte_budget_is_refused() {
    let pool = tmp_dir("bouncebudget");
    write_source(&pool, "s1", 100);
    let out = tmp_dir("o").join("o.wav");
    let script = vec![
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 1.0)], at_frame: Some(0) },
        HostCommand::Pool { dir: pool.clone() },
        HostCommand::Bounce { frames: 1 << 40, path: out }, // ~4 TB of f32
    ];
    assert!(run_script(&script).is_err(), "a bounce over the byte budget must be refused");
    let _ = std::fs::remove_dir_all(&pool);
}

#[test]
fn fractional_or_zero_mixer_channels_is_refused() {
    let pool = tmp_dir("channels");
    // a fraction (4.5) must not be silently truncated to 4
    let frac = vec![
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 4.5)], at_frame: Some(0) },
        HostCommand::Pool { dir: pool.clone() },
    ];
    assert!(run_script(&frac).is_err(), "fractional mixer channels must be refused");
    // zero / negative refused
    let zero = vec![
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 0.0)], at_frame: Some(0) },
        HostCommand::Pool { dir: pool.clone() },
    ];
    assert!(run_script(&zero).is_err(), "zero mixer channels must be refused");
    // over the max (8) refused
    let big = vec![
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 9.0)], at_frame: Some(0) },
        HostCommand::Pool { dir: pool.clone() },
    ];
    assert!(run_script(&big).is_err(), "channels over the mixer max must be refused");
    let _ = std::fs::remove_dir_all(&pool);
}

#[test]
fn text_format_pool_and_arrange_run_byte_identically() {
    // the text form (the CLI smoke binary's wire schema) must drive the clip
    // editor end-to-end: pool + arrange + bounce, byte-identical on replay.
    let pool = tmp_dir("textpool");
    write_source(&pool, "s1", 8000);
    let out_dir = tmp_dir("textout");
    let a = out_dir.join("a.wav");
    let b = out_dir.join("b.wav");

    let script = |out: &std::path::Path| format!(
        "host v1\n\
         mount mixer channels=2 @0\n\
         pool {}\n\
         arrange add_track t0 @0\n\
         arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0\n\
         arrange add_track t1 @0\n\
         arrange add_clip t1 c1 s1 0 1500 500 64 128 1.0 @0\n\
         bounce 6000 {}\n",
        pool.display(), out.display()
    );

    let script_a = host::parse_script(&script(&a)).unwrap();
    let sess = host::run_script(&script_a).unwrap();
    let frames_a = std::fs::read(&a).unwrap();
    // the arrangement plays (non-silent)
    let mut r = media::WavReader::open(&a).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(audio[..n].iter().any(|s| s.abs() > 1e-4), "must produce audio");
    assert_eq!(sess.arrangement().tracks.len(), 2);

    // replay byte-identically
    let script_b = host::parse_script(&script(&b)).unwrap();
    let _ = host::run_script(&script_b).unwrap();
    let frames_b = std::fs::read(&b).unwrap();
    assert_eq!(frames_a, frames_b, "text-format arrangement must replay byte-identically");

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn text_format_arrange_ops_parse() {
    // each arrange op's text grammar parses to the expected command (no silent
    // leniency — the wire schema must be strict about operands).
    for (line, _) in [
        ("arrange add_track t0 @0", 0),
        ("arrange remove_track t1 @0", 0),
        ("arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0", 0),
        ("arrange razor_split t0 c0 cL cR 3000 @0", 0),
        ("arrange trim t0 c0 start 500 @0", 0),
        ("arrange trim t0 c0 end -200 @0", 0),
        ("arrange move_clip t0 c0 9000 @0", 0),
        ("arrange move_clip_to_track t0 c0 t1 50 @0", 0),
        ("arrange duplicate t0 c0 c1 @0", 0),
        ("arrange delete t0 c0 @0", 0),
        ("arrange set_clip_gain t0 c0 0.75 @0", 0),
        ("arrange set_clip_fade t0 c0 64 128 @0", 0),
        ("arrange loop_region t0 c0 3 @0", 0),
    ] {
        let cmds = host::parse_script(&format!("host v1\n{line}\n")).unwrap();
        assert_eq!(cmds.len(), 1, "{line}");
    }
    // a malformed arrange line refuses, never panics
    assert!(host::parse_script("host v1\narrange add_clip t0 c0\n").is_err());
    assert!(host::parse_script("host v1\narrange nope_op t0\n").is_err());
    // strict arity: trailing junk / too many operands refuse
    assert!(host::parse_script("host v1\narrange add_track t1 junk\n").is_err());
    // non-finite gain refuses (NaN reaches the audio path otherwise)
    assert!(host::parse_script("host v1\narrange add_clip t0 c0 s1 0 4000 0 0 0 nan\n").is_err());
    // loop_region times must fit u32 (no silent truncation)
    assert!(host::parse_script("host v1\narrange loop_region t0 c0 4294967296\n").is_err());
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
