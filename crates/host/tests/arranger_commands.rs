//! P1.3.4 — the reference host speaks the clip-arrangement commands: `Pool` (point
//! the host at the pool) and `Arrange` (a logged ACID op) build the arrangement
//! value. The value is a pure reconstruction of the logged op stream, so a fresh
//! host replays it byte-identically. (The live audio wiring of the arranger
//! nodes into the mixer is a documented follow-up — the graph's forward-order
//! constraint requires the shared-state reconcile; see the P1.3.4 note.)

use std::path::{Path, PathBuf};

use host::{HostCommand, HostSession, run_script};
use media::{ArrangeOp, Clip, WavWriter};

const SR: u32 = 48_000;
/// The equal-power center-pan gain each mono mixer channel sees (`pan = 0`).
const CENTER: f32 = std::f32::consts::FRAC_1_SQRT_2;

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
            HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: Some(0),
        });
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("c0", 0, 8000),
            },
            at_frame: Some(0),
        });
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddTrack { track: "t1".into() },
            at_frame: Some(0),
        });
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddClip {
                track: "t1".into(),
                clip: clip("c1", 0, 1000),
            },
            at_frame: Some(0),
        });
        s.push(HostCommand::Bounce {
            frames: 4000,
            path: a.clone(),
        });
        // remove t0, then render the *next* 4000 frames — t0's clip spans
        // [0,8000), so it would still be playing here if its node were mounted.
        s.push(HostCommand::Arrange {
            op: ArrangeOp::RemoveTrack { track: "t0".into() },
            at_frame: Some(4000),
        });
        s.push(HostCommand::Bounce {
            frames: 4000,
            path: out_dir.join("b.wav"),
        });
        s
    };

    let _ = run_script(&script).unwrap();

    // The first render (frames 0..4000) must be non-silent (t0 is playing).
    let mut r = media::WavReader::open(&a).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().any(|s| s.abs() > 1e-4),
        "t0 must be audible in the first render"
    );

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
            HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: Some(0),
        });
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("c0", 0, 8000),
            },
            at_frame: Some(0),
        });
        s.push(HostCommand::Bounce {
            frames: 4000,
            path: out_dir.join(format!("{tag}-a.wav")),
        });
        s.push(HostCommand::Arrange {
            op: ArrangeOp::SetClipGain {
                track: "t0".into(),
                clip: "c0".into(),
                gain,
            },
            at_frame: Some(4000),
        });
        s.push(HostCommand::Bounce {
            frames: 4000,
            path: out_dir.join(format!("{tag}-b.wav")),
        });
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
        // The mono source is center-panned (a single channel on ch0), so the
        // stereo bounce's channel 0 carries source_value * √2/2.
        let expected = source_value(f, period) * CENTER;
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
            HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: Some(0),
        });
        s.push(HostCommand::Arrange {
            op: ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("c0", 0, 8000),
            },
            at_frame: Some(0),
        });
        s.push(HostCommand::Bounce {
            frames: 4000,
            path: out_dir.join(format!("{tag}-a.wav")),
        });
        s.push(HostCommand::Arrange {
            op: ArrangeOp::SetClipGain {
                track: "t0".into(),
                clip: "c0".into(),
                gain: 0.7,
            },
            at_frame: Some(4000),
        });
        s.push(HostCommand::Bounce {
            frames: 4000,
            path: out.to_path_buf(),
        });
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
        reversed: false,
        id: id.into(),
        name: None,
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
    let arrange = |op: ArrangeOp| HostCommand::Arrange {
        op,
        at_frame: Some(0),
    };
    vec![
        arrange(ArrangeOp::AddTrack { track: "t0".into() }),
        arrange(ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 4000),
        }),
        arrange(ArrangeOp::AddTrack { track: "t1".into() }),
        arrange(ArrangeOp::AddClip {
            track: "t1".into(),
            clip: clip("c1", 500, 1500),
        }),
        arrange(ArrangeOp::SetClipFade {
            track: "t1".into(),
            clip: "c1".into(),
            fade_in: 64,
            fade_out: 128,
        }),
    ]
}

#[test]
fn arrange_commands_build_the_logged_value_and_replay_identically() {
    let pool = tmp_dir("pool");
    write_source(&pool, "s1", 8000);

    let script: Vec<HostCommand> = {
        let mut s = vec![
            HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.extend(arrange_ops());
        s
    };

    // live: the Arrange commands build the arrangement value.
    let sess = run_script(&script).unwrap();
    let value = sess
        .arrangement()
        .expect("the live arrangement must snapshot");
    assert_eq!(
        value.tracks.len(),
        2,
        "two tracks built by the arrange commands"
    );
    assert_eq!(value.tracks[0].clips.len(), 1, "t0 has one clip");
    assert_eq!(value.tracks[0].clips[0].src_len, 4000);
    assert_eq!(value.tracks[1].clips[0].fade_out, 128);
    assert!(sess.event_count() > 0, "the arrangement ops are logged");

    // replay: a fresh host, same script, identical value (byte-identical log reconstructs it).
    let sess2 = run_script(&script).unwrap();
    assert_eq!(
        value,
        sess2
            .arrangement()
            .expect("the replayed arrangement must snapshot"),
        "replay must reconstruct the identical value"
    );

    let _ = std::fs::remove_dir_all(&pool);
}

#[test]
fn arrange_without_pool_is_refused() {
    let pool = tmp_dir("refused");
    write_source(&pool, "s1", 100);
    // No `Pool` command first → the arrange command fails loudly.
    let script = vec![
        HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 1.0)],
            at_frame: Some(0),
        },
        HostCommand::Arrange {
            op: ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: Some(0),
        },
    ];
    assert!(
        run_script(&script).is_err(),
        "arrange requires set_pool first"
    );
    let _ = std::fs::remove_dir_all(&pool);
}

#[test]
fn bounce_over_byte_budget_is_refused() {
    let pool = tmp_dir("bouncebudget");
    write_source(&pool, "s1", 100);
    let out = tmp_dir("o").join("o.wav");
    let script = vec![
        HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 1.0)],
            at_frame: Some(0),
        },
        HostCommand::Pool { dir: pool.clone() },
        HostCommand::Bounce {
            frames: 1 << 40,
            path: out,
        }, // ~4 TB of f32
    ];
    assert!(
        run_script(&script).is_err(),
        "a bounce over the byte budget must be refused"
    );
    let _ = std::fs::remove_dir_all(&pool);
}

#[test]
fn fractional_or_zero_mixer_channels_is_refused() {
    let pool = tmp_dir("channels");
    // a fraction (4.5) must not be silently truncated to 4
    let frac = vec![
        HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 4.5)],
            at_frame: Some(0),
        },
        HostCommand::Pool { dir: pool.clone() },
    ];
    assert!(
        run_script(&frac).is_err(),
        "fractional mixer channels must be refused"
    );
    // zero / negative refused
    let zero = vec![
        HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 0.0)],
            at_frame: Some(0),
        },
        HostCommand::Pool { dir: pool.clone() },
    ];
    assert!(
        run_script(&zero).is_err(),
        "zero mixer channels must be refused"
    );
    // A width past the old ceiling is a valid mount now: channels are lanes, not a knob.
    let wide = vec![
        HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 9.0)],
            at_frame: Some(0),
        },
        HostCommand::Pool { dir: pool.clone() },
    ];
    assert!(
        run_script(&wide).is_ok(),
        "nine channels must mount: the mixer's width is no longer a compile-time limit"
    );
    // The only bound left is a sanity guard against gross typos, and it is loud.
    let absurd = vec![
        HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 1000.0)],
            at_frame: Some(0),
        },
        HostCommand::Pool { dir: pool.clone() },
    ];
    assert!(
        run_script(&absurd).is_err(),
        "an absurd channel count must be refused"
    );
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

    let script = |out: &std::path::Path| {
        format!(
            "host v1\n\
         mount mixer channels=2 @0\n\
         pool {}\n\
         arrange add_track t0 @0\n\
         arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0\n\
         arrange add_track t1 @0\n\
         arrange add_clip t1 c1 s1 0 1500 500 64 128 1.0 @0\n\
         bounce 6000 {}\n",
            pool.display(),
            out.display()
        )
    };

    let script_a = host::parse_script(&script(&a)).unwrap();
    let sess = host::run_script(&script_a).unwrap();
    let frames_a = std::fs::read(&a).unwrap();
    // the arrangement plays (non-silent)
    let mut r = media::WavReader::open(&a).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().any(|s| s.abs() > 1e-4),
        "must produce audio"
    );
    assert_eq!(
        sess.arrangement()
            .expect("the text-format arrangement must snapshot")
            .tracks
            .len(),
        2
    );

    // replay byte-identically
    let script_b = host::parse_script(&script(&b)).unwrap();
    let _ = host::run_script(&script_b).unwrap();
    let frames_b = std::fs::read(&b).unwrap();
    assert_eq!(
        frames_a, frames_b,
        "text-format arrangement must replay byte-identically"
    );

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

/// **A fade pair whose sum overflows `u64` is refused, from the wire in.** The
/// text format takes both fade operands as raw `u64` with no bound and no
/// `frame_operand` to snap, so `arrange set_clip_fade t0 c0 18446744073709551615
/// 1` parses and reaches the value model. `u64::MAX + 1` wraps to 0, which used
/// to pass the length check — a release build logged the op and rendered the clip
/// at gain 0 for every sample (silent while the log claimed otherwise), and a
/// debug build panicked inside the editor's timeline lock, poisoning every later
/// `arrangement()`. The script must be refused loudly, and the session must
/// still be readable afterwards.
#[test]
fn a_fade_pair_whose_sum_overflows_u64_is_refused_by_a_script() {
    let pool = tmp_dir("fadeoverflow");
    write_source(&pool, "s1", 8000);
    let overflowing = format!(
        "host v1\n\
         mount mixer channels=2 @0\n\
         pool {}\n\
         arrange add_track t0 @0\n\
         arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0\n\
         arrange set_clip_fade t0 c0 18446744073709551615 1 @0\n",
        pool.display()
    );
    // The parse itself succeeds: the wire schema carries the operands, and the
    // refusal is the value model's (a parse error would blame the wrong layer).
    let cmds = host::parse_script(&overflowing).expect("the operands are plain u64");
    assert_eq!(cmds.len(), 5);
    let err = host::run_script(&cmds).expect_err("a wrapped fade sum must be refused");
    assert!(
        err.contains("fades"),
        "the refusal names the fade rule, got: {err}"
    );

    // `add_clip` carries the same pair (words 7/8) and is refused the same way.
    let add_overflowing = format!(
        "host v1\n\
         mount mixer channels=2 @0\n\
         pool {}\n\
         arrange add_track t0 @0\n\
         arrange add_clip t0 c0 s1 0 4000 0 18446744073709551615 1 1.0 @0\n",
        pool.display()
    );
    let err = host::run_script(&host::parse_script(&add_overflowing).expect("parses"))
        .expect_err("an add_clip whose fades overflow must be refused");
    assert!(
        err.contains("fades"),
        "the refusal names the fade rule, got: {err}"
    );

    // A legal pair over the same clip is still accepted, so the guard is the
    // fade rule and not a blanket refusal of large operands.
    let legal = format!(
        "host v1\n\
         mount mixer channels=2 @0\n\
         pool {}\n\
         arrange add_track t0 @0\n\
         arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0\n\
         arrange set_clip_fade t0 c0 4000 0 @0\n",
        pool.display()
    );
    let sess = host::run_script(&host::parse_script(&legal).expect("parses"))
        .expect("a fade pair within the clip length applies");
    assert_eq!(
        sess.arrangement()
            .expect("a legal fade must snapshot")
            .tracks[0]
            .clips[0]
            .fade_in,
        4000,
        "the legal fade pair is in the value"
    );

    let _ = std::fs::remove_dir_all(&pool);
}

/// **The two-keypress sequence that used to make a track unplayable, end to end.**
/// `f` at the clip's end sets a *full-length* fade-in (the shell caps it at
/// `src_len - fade_out`, and the pair is legal: `4000 + 0 <= 4000`); `x` then
/// razor-splits at an earlier frame. The split zeroed the seam fades but kept the
/// inherited ones on halves a fraction of the original length, so it held a clip the
/// model refuses — and `ArrangerNode::new` refuses the **whole track** over one clip,
/// so the *bounce* failed and nothing on the track played. The split caps the fades
/// to the halves they land on, so the sequence renders.
#[test]
fn a_razor_split_of_a_full_length_fade_still_bounces_audio() {
    let pool = tmp_dir("spliffade");
    write_ramp(&pool, "s1", 8000, 257);
    let out_dir = tmp_dir("spliffadeout");
    let out = out_dir.join("a.wav");

    let script_text = format!(
        "host v1\nmount mixer channels=2 @0\npool {}\narrange add_track t0 @0\narrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0\narrange set_clip_fade t0 c0 4000 0 @0\narrange razor_split t0 c0 cL cR 1200 @0\nbounce 4000 {}\n",
        pool.display(),
        out.display()
    );
    let cmds = host::parse_script(&script_text).expect("script parses");
    // The bounce is the assertion that matters: `bounce` renders through
    // `wire_arranger`, which builds an `ArrangerNode` per track and returns the
    // refusal as an `Err`. On the unfixed value this line failed with
    // "arranger: clip 'cL' fades exceed the clip length".
    let sess = run_script(&cmds).expect("a split of a fading clip must still bounce");

    let timeline = sess.arrangement().expect("the arrangement must snapshot");
    let ids: Vec<_> = timeline.tracks[0]
        .clips
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids, vec!["cL", "cR"], "the split produced two halves");
    for c in &timeline.tracks[0].clips {
        media::timeline::validate_clip(c)
            .unwrap_or_else(|e| panic!("clip '{}' is invalid: {e}", c.id));
    }
    assert_eq!(
        timeline.tracks[0].clips[0].fade_in, 1200,
        "the fade-in is capped to the left half, not the whole clip"
    );

    let mut r = media::WavReader::open(&out).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().any(|s| s.abs() > 1e-4),
        "the split halves render audio"
    );
    // The capped fade is *audible*, not merely present: the left half ramps from
    // silence at its first frame to full level over the 1200 frames it now spans.
    assert!(
        audio[0].abs() < 1e-3,
        "the first frame of the split half is still inside its fade-in, got {}",
        audio[0]
    );
    assert!(
        audio[1000].abs() > 0.05,
        "and the fade opens within the half, got {}",
        audio[1000]
    );

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
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
            HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            },
            HostCommand::Pool { dir: pool.clone() },
        ];
        s.extend(arrange_ops());
        s.push(HostCommand::Bounce {
            frames: 6000,
            path: out.to_path_buf(),
        });
        s
    };

    let sess = run_script(&script(&a)).unwrap();
    let frames_a = std::fs::read(&a).unwrap();
    // the arrangement actually plays (not a silent bounce)
    let mut r = media::WavReader::open(&a).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().any(|s| s.abs() > 1e-4),
        "the arrangement must produce audio"
    );
    assert_eq!(
        sess.arrangement()
            .expect("the arrangement must snapshot")
            .tracks
            .len(),
        2
    );

    // replay: a fresh host, same script → byte-identical bounce.
    let _ = run_script(&script(&b)).unwrap();
    let frames_b = std::fs::read(&b).unwrap();
    assert_eq!(frames_a, frames_b, "replay must bounce byte-identically");

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// The incremental session API (`HostSession::new` + `execute`) applies edits
/// one command at a time onto a *persistent* session — the live path a UI uses.
/// After building an arrangement and bouncing, the session reports the exact
/// value a fresh `run_script` of the same log would reconstruct.
#[test]
fn incremental_session_applies_edits_and_reports_arrangement() {
    let pool = tmp_dir("incr");
    write_ramp(&pool, "s1", 8000, 257);
    let out_dir = tmp_dir("incrout");
    let out = out_dir.join("i.wav");

    let mut s = HostSession::new();
    s.set_pool(&pool).unwrap();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .unwrap();
    s.execute(&HostCommand::Pool { dir: pool.clone() }).unwrap();
    s.execute(&HostCommand::Arrange {
        op: ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: Some(0),
    })
    .unwrap();
    s.execute(&HostCommand::Arrange {
        op: ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 4000),
        },
        at_frame: Some(0),
    })
    .unwrap();
    s.execute(&HostCommand::Bounce {
        frames: 4000,
        path: out.clone(),
    })
    .unwrap();

    let timeline = s.arrangement().expect("a persisted session has a value");
    assert_eq!(
        timeline.tracks.len(),
        1,
        "one track from the incremental AddTrack"
    );
    assert_eq!(
        timeline.tracks[0].clips.len(),
        1,
        "one clip from the incremental AddClip"
    );
    assert_eq!(timeline.tracks[0].clips[0].id, "c0");
    assert_eq!(
        s.underruns(),
        0,
        "no underruns on a clean incremental bounce"
    );

    let mut r = media::WavReader::open(&out).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().any(|s| s.abs() > 1e-4),
        "the incremental session renders audio"
    );

    // A refused op (bad pool dir) fails the session without breaking it.
    let bad = s.execute(&HostCommand::Pool {
        dir: PathBuf::from("/no/such/pool"),
    });
    assert!(bad.is_err(), "a bad Pool is refused, never a panic");
    assert_eq!(
        s.arrangement().unwrap().tracks.len(),
        1,
        "a refused op changes nothing"
    );

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// The `chop` text-format op splits a clip into N contiguous pieces, and the
/// resulting value is a pure reconstruction (the arrangement reports 4 pieces
/// tiling the original span, and the bounce renders audio).
#[test]
fn chop_text_format_splits_a_clip_and_renders() {
    let pool = tmp_dir("chop");
    write_ramp(&pool, "s1", 8000, 257);
    let out_dir = tmp_dir("chopout");
    let out = out_dir.join("c.wav");

    let script_text = format!(
        "host v1\nmount mixer channels=2 @0\npool {}\narrange add_track t0 @0\narrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0\narrange chop t0 c0 4 pre @0\nbounce 4000 {}\n",
        pool.display(),
        out.display()
    );
    let cmds = host::parse_script(&script_text).expect("script parses");
    let sess = host::run_script(&cmds).unwrap();

    let timeline = sess.arrangement().expect("the arrangement must snapshot");
    assert_eq!(timeline.tracks[0].clips.len(), 4, "chop 4 -> four pieces");
    assert_eq!(timeline.tracks[0].clips[0].id, "pre.0");
    assert_eq!(timeline.tracks[0].clips[3].id, "pre.3");
    assert_eq!(timeline.tracks[0].clips.last().unwrap().end(), 4000);

    let mut r = media::WavReader::open(&out).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().any(|s| s.abs() > 1e-4),
        "chopped pieces render audio"
    );

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Write a constant source at an explicit rate (a foreign-rate import fixture).
fn write_source_at_rate(dir: &Path, stem: &str, sample_rate: u32, frames: u64) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), sample_rate, 1).unwrap();
    w.write(&vec![0.5f32; frames as usize]).unwrap();
    w.finalize().unwrap();
}

/// **The reported bug.** A 44.1 kHz source in a 48 kHz session used to stop the
/// transport at play ("clip 'c0' source is 44100 Hz but the session is 48000 Hz").
/// Adopting the pool converts it — once, in place, preserving the original — so
/// the arrangement is playable and the session reports what it moved.
#[test]
fn adopting_a_pool_converts_a_foreign_rate() {
    let pool = tmp_dir("foreignrate");
    write_source_at_rate(&pool, "s1", 44_100, 44_100); // one second, foreign rate
    let out_dir = tmp_dir("foreignrateout");
    let out = out_dir.join("out.wav");

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .unwrap();
    s.execute(&HostCommand::Pool { dir: pool.clone() }).unwrap();

    // The pool pass converted it, and the session says so (a shell shows this).
    let conformed = s.pool_conformed();
    assert_eq!(conformed.len(), 1, "one source was converted");
    assert_eq!(conformed[0].id, "s1");
    assert_eq!(
        (conformed[0].from_rate, conformed[0].to_rate),
        (44_100, 48_000)
    );
    assert_eq!(
        (conformed[0].frames_in, conformed[0].frames_out),
        (44_100, 48_000)
    );
    assert!(conformed[0].converted);
    assert!(
        pool.join("s1.wav.pre44100").is_file(),
        "the pre-conversion original is preserved"
    );

    // The pool now lists one source at the session rate (the original is not indexed).
    let sources = s.pool_sources().expect("a pool");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].sample_rate, SR);
    assert_eq!(sources[0].frames, SR as u64);

    // Adopting it again is a no-op — the conversion does not run twice.
    s.execute(&HostCommand::Pool { dir: pool.clone() }).unwrap();
    assert!(s.pool_conformed().is_empty(), "conform is idempotent");

    // …and the clip that spans it renders audio instead of refusing the transport.
    s.execute(&HostCommand::Arrange {
        op: ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: Some(0),
    })
    .unwrap();
    let foreign = Clip {
        src_len: SR as u64,
        ..clip("c0", 0, SR as u64)
    };
    s.execute(&HostCommand::Arrange {
        op: ArrangeOp::AddClip {
            track: "t0".into(),
            clip: foreign,
        },
        at_frame: Some(0),
    })
    .unwrap();
    s.execute(&HostCommand::Bounce {
        frames: 4800,
        path: out.clone(),
    })
    .expect("the converted source plays");

    let mut r = media::WavReader::open(&out).unwrap();
    assert_eq!(r.sample_rate(), SR);
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(
        audio[..n].iter().any(|s| s.abs() > 1e-3),
        "the resampled source must be audible through the mixer"
    );

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Write a stereo float source: channel 0 is a `left_hz` tone, channel 1 a
/// `right_hz` tone, so a dropped channel or a swapped one is visible in the
/// samples (a constant source would hide both).
fn write_stereo_tones(dir: &Path, stem: &str, frames: u64, left_hz: f64, right_hz: f64) {
    let path = dir.join(format!("{stem}.wav"));
    let mut w = WavWriter::create_float(&path, SR, 2).unwrap();
    let mut interleaved = Vec::with_capacity(frames as usize * 2);
    for i in 0..frames {
        let t = i as f64 / SR as f64;
        interleaved.push((std::f64::consts::TAU * left_hz * t).sin() as f32 * 0.5);
        interleaved.push((std::f64::consts::TAU * right_hz * t).sin() as f32 * 0.5);
    }
    w.write(&interleaved).unwrap();
    w.finalize().unwrap();
}

/// Zero crossings in `audio` (a monotone property of a pure tone's frequency).
fn crossings(audio: &[f32]) -> usize {
    audio
        .windows(2)
        .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
        .count()
}

/// Read one channel of a (possibly stereo) WAV in full.
fn read_channel(path: &Path, channel: u16) -> Vec<f32> {
    let mut r = media::WavReader::open(path)
        .unwrap()
        .with_channel(channel)
        .unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    audio.truncate(n);
    audio
}

/// **Stereo material survives the trip.** A stereo file adopted into the pool is
/// split into one mono source per channel (the same `{id}.ch{k}` naming capture
/// writes), and arranging them on two tracks with the mixer's pan puts the left
/// tone on the left and the right tone on the right — so neither channel is
/// dropped, and neither is swapped.
#[test]
fn a_stereo_pool_source_plays_both_channels_on_the_right_sides() {
    let pool = tmp_dir("stereosplit");
    let out_dir = tmp_dir("stereosplitout");
    let out = out_dir.join("out.wav");
    write_stereo_tones(&pool, "jam", SR as u64, 440.0, 880.0);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .unwrap();
    // Adopting the pool conforms it: the stereo file is split in place.
    s.execute(&HostCommand::Pool { dir: pool.clone() }).unwrap();

    // One entry per pool source the split produced, the file's own `jam` first.
    let conformed = s.pool_conformed();
    assert_eq!(
        conformed.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec!["jam", "jam.ch1"]
    );
    assert_eq!(conformed[0].channels, 2);
    assert_eq!(conformed[0].extracted, vec!["jam.ch1"]);
    assert!(!conformed[0].converted, "the rate was already right");

    // The pool is mono now: channel 0 stays `jam` (a clip that referenced it
    // still plays what it always played), channel 1 is `jam.ch1`.
    let sources = s.pool_sources().expect("a pool");
    let mut ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["jam", "jam.ch1"]);
    assert!(sources.iter().all(|s| s.channels == 1));
    assert!(sources.iter().all(|s| s.sample_rate == SR));

    // One track per channel, panned hard to its own side.
    for (i, (track, channel)) in [("t0", "jam"), ("t1", "jam.ch1")].into_iter().enumerate() {
        s.execute(&HostCommand::Arrange {
            op: ArrangeOp::AddTrack {
                track: track.into(),
            },
            at_frame: Some(0),
        })
        .unwrap();
        s.execute(&HostCommand::Arrange {
            op: ArrangeOp::AddClip {
                track: track.into(),
                clip: Clip {
                    source: channel.into(),
                    ..clip(&format!("c{i}"), 0, SR as u64)
                },
            },
            at_frame: Some(0),
        })
        .unwrap();
    }
    s.execute(&HostCommand::SetParam {
        plugin: "mixer",
        param: "ch0.pan",
        value: -1.0,
        at_frame: Some(0),
    })
    .unwrap();
    s.execute(&HostCommand::SetParam {
        plugin: "mixer",
        param: "ch1.pan",
        value: 1.0,
        at_frame: Some(0),
    })
    .unwrap();
    s.execute(&HostCommand::Bounce {
        frames: 4800,
        path: out.clone(),
    })
    .expect("both channels play");

    // The bounce is the stereo master: hard-left is channel 0 only.
    let r = media::WavReader::open(&out).unwrap();
    assert_eq!(r.channels(), 2, "the mixer renders a stereo master");
    drop(r);
    let left = read_channel(&out, 0);
    let right = read_channel(&out, 1);
    let left_rms = (left.iter().map(|s| s * s).sum::<f32>() / left.len() as f32).sqrt();
    let right_rms = (right.iter().map(|s| s * s).sum::<f32>() / right.len() as f32).sqrt();
    assert!(
        left_rms > 0.2,
        "the left tone is audible: rms {left_rms:.3}"
    );
    assert!(
        right_rms > 0.2,
        "the right tone is audible: rms {right_rms:.3}"
    );

    // …and the right tone is *the right tone*: 440 Hz crosses zero ~44 times per
    // 100 ms at 48 kHz, 880 Hz twice that, so a swap cannot pass.
    let (lc, rc) = (crossings(&left), crossings(&right));
    let expect = |hz: f64| (2.0 * hz * left.len() as f64 / SR as f64).round() as i64;
    assert!(
        (lc as i64 - expect(440.0)).abs() <= 2,
        "left channel should be 440 Hz: {lc} crossings, expected ~{}",
        expect(440.0)
    );
    assert!(
        (rc as i64 - expect(880.0)).abs() <= 2,
        "right channel should be 880 Hz: {rc} crossings, expected ~{}",
        expect(880.0)
    );

    let _ = std::fs::remove_dir_all(&pool);
    let _ = std::fs::remove_dir_all(&out_dir);
}
