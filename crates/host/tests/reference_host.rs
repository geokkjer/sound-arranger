//! Reference-host acceptance tests (UI-as-plugin note):
//! - a full script (chain mount/patch, play + splice, mid-session set_param,
//!   bounce) runs twice → byte-identical bounces and identical logs
//!   (determinism across the Host API contract, with real command frames);
//! - the splice actually fires (deferred == 0) and the spliced bounce differs
//!   from an unspliced control;
//! - refused commands are rejected identically on both runs, nothing logged;
//! - unmount and set_tempo run; play-twice and splice-without-play refused;
//! - the text format (versioned) parses; out-of-range channels refused;
//! - the actual CLI binary (the headless smoke binary) records and bounces
//!   without any frontend.

use std::path::{Path, PathBuf};
use std::process::Command;

use host::{HostCommand, parse_script, run_script};
use media::{WavReader, WavWriter};

const SR: u32 = 48_000;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("host-ref-{name}-{}.wav", std::process::id()))
}

fn write_tone(path: &Path, freq: f32, frames: u64) {
    let mut w = WavWriter::create(path, SR, 1).unwrap();
    let mut buf = vec![0.0f32; 2048];
    let mut phase = 0.0f64;
    let mut left = frames;
    while left > 0 {
        let n = buf.len().min(left as usize);
        for s in &mut buf[..n] {
            *s = (std::f64::consts::TAU * phase).sin() as f32 * 0.5;
            phase += freq as f64 / SR as f64;
        }
        w.write(&buf[..n]).unwrap();
        left -= n as u64;
    }
    w.finalize().unwrap();
}

/// The canonical reference script, with real command frames: the chain
/// through the mixer, a clip playing into ch0, a gain change at 2 000, a
/// splice to the second clip at 4 000 (its crossfade lands inside the bounce,
/// which covers frames 4 000..10 000).
fn script(a: &Path, b: &Path, out: &Path) -> Vec<HostCommand> {
    vec![
        HostCommand::Mount {
            plugin: "euclidean",
            params: vec![("steps", 8.0), ("pulses", 1.0), ("rotation", 1.0), ("pulses_per_beat", 4.0)],
            at_frame: Some(0),
        },
        HostCommand::Mount { plugin: "scale", params: vec![("root", 0.0)], at_frame: Some(0) },
        HostCommand::Mount { plugin: "tone", params: vec![("gain", 0.25), ("blip_len", 1200.0)], at_frame: Some(0) },
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 4.0)], at_frame: Some(0) },
        HostCommand::Patch { from: ("euclidean", "triggers"), to: ("scale", "trigger"), at_frame: Some(0) },
        HostCommand::Patch { from: ("scale", "note"), to: ("tone", "note"), at_frame: Some(0) },
        HostCommand::Patch { from: ("tone", "audio"), to: ("mixer", "ch0"), at_frame: Some(0) },
        HostCommand::Play { clip: media::ClipRef::whole(a).unwrap(), channel: 0, at_frame: Some(0) },
        HostCommand::SetParam { plugin: "mixer", param: "ch0.gain", value: 0.5, at_frame: Some(2_000) },
        HostCommand::Splice { at_frame: 4_000, clip: media::ClipRef::whole(b).unwrap(), crossfade: 512 },
        HostCommand::Bounce { frames: 6_000, path: out.to_path_buf() },
    ]
}

/// The same script on fresh sessions bounces byte-identically, logs
/// identically, applies the splice sample-accurately (deferred == 0), and the
/// splice's effect is audible in the bounced window.
#[test]
fn full_script_bounces_byte_identically() {
    let a = tmp("clip-a");
    let b = tmp("clip-b");
    write_tone(&a, 440.0, SR as u64);
    write_tone(&b, 880.0, SR as u64);
    let out1 = tmp("out-1");
    let out2 = tmp("out-2");

    let run = |out: &Path| -> (Vec<u8>, Vec<engine::Event>, u64, u64) {
        let s = run_script(&script(&a, &b, out)).expect("script runs");
        let bytes = std::fs::read(out).unwrap();
        (bytes, s.log().events().to_vec(), s.underruns(), s.deferred())
    };

    let (bytes1, log1, underruns1, deferred1) = run(&out1);
    let (bytes2, log2, _underruns2, _deferred2) = run(&out2);
    assert_eq!(bytes1, bytes2, "the same script must bounce byte-identically");
    assert_eq!(log1, log2, "the logs must be identical, not just equal length");
    assert_eq!((underruns1, deferred1), (0, 0), "no underruns; the splice applies sample-accurately");
    assert!(bytes1.len() > 44, "the bounce holds audio");

    // The log records the command's real frames (not all-zero).
    let set_param_frames: Vec<u64> = log1
        .iter()
        .filter_map(|ev| match ev {
            engine::Event::SetParam { at_frame, .. } => Some(*at_frame),
            _ => None,
        })
        .collect();
    assert_eq!(set_param_frames, vec![2_000], "the set_param is logged at its frame");

    // The bounce is audible content.
    let mut r = WavReader::open(&out1).unwrap();
    let mut back = vec![0.0f32; r.total_frames() as usize];
    r.read_into(&mut back);
    assert!(back.iter().any(|s| *s != 0.0), "the bounce is not silence");

    // The spliced bounce differs from an unspliced control (B vs A content).
    let control_out = tmp("control");
    let mut control_script = script(&a, &b, &control_out);
    control_script.retain(|c| !matches!(c, HostCommand::Splice { .. }));
    run_script(&control_script).unwrap();
    let control = std::fs::read(&control_out).unwrap();
    assert_ne!(bytes1, control, "the splice must change the bounced audio");

    for p in [&a, &b, &out1, &out2, &control_out] {
        let _ = std::fs::remove_file(p);
    }
}

/// Unmount and set_tempo run through the contract; play-twice and
/// splice-without-play are refused fail-loud.
#[test]
fn lifecycle_and_refusals() {
    let a = tmp("life-a");
    write_tone(&a, 440.0, SR as u64);
    let out = tmp("life-out");

    // set_tempo + unmount execute
    let s = run_script(&[
        HostCommand::Mount { plugin: "euclidean", params: vec![], at_frame: Some(0) },
        HostCommand::SetTempo { bpm: 240.0, beats_per_bar: 4, at_frame: Some(0) },
        HostCommand::Unmount { plugin: "euclidean", at_frame: Some(0) },
        HostCommand::Bounce { frames: 512, path: out.clone() },
    ])
    .expect("set_tempo and unmount run");
    assert!(s.event_count() >= 3, "mount + set_tempo + unmount are logged");

    // play without the mixer → refused
    let err = run_script(&[HostCommand::Play {
        clip: media::ClipRef::whole(&a).unwrap(),
        channel: 0,
        at_frame: Some(0),
    }])
    .unwrap_err();
    assert!(err.contains("mixer"), "got: {err}");

    // splice without a playing clip → refused
    let err = run_script(&[
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 4.0)], at_frame: Some(0) },
        HostCommand::Splice {
            at_frame: 100,
            clip: media::ClipRef::whole(&a).unwrap(),
            crossfade: 512,
        },
    ])
    .unwrap_err();
    assert!(err.contains("splice requires"), "got: {err}");

    // playing twice → refused
    let err = run_script(&[
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 4.0)], at_frame: Some(0) },
        HostCommand::Play { clip: media::ClipRef::whole(&a).unwrap(), channel: 0, at_frame: Some(0) },
        HostCommand::Play { clip: media::ClipRef::whole(&a).unwrap(), channel: 1, at_frame: Some(0) },
    ])
    .unwrap_err();
    assert!(err.contains("one clip"), "got: {err}");

    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&out);
}

/// A refused command (a type-mismatched patch) is rejected identically on
/// both runs, and nothing is logged (fail-loud before the log).
#[test]
fn refused_commands_are_rejected_identically() {
    let bad_script = |out: &Path| -> Vec<HostCommand> {
        vec![
            HostCommand::Mount { plugin: "euclidean", params: vec![], at_frame: Some(0) },
            HostCommand::Mount { plugin: "tone", params: vec![], at_frame: Some(0) },
            HostCommand::Patch { from: ("euclidean", "triggers"), to: ("tone", "note"), at_frame: Some(0) }, // kind mismatch
            HostCommand::Bounce { frames: 512, path: out.to_path_buf() },
        ]
    };

    let run = |out: &Path| -> Result<(Vec<u8>, usize), String> {
        match run_script(&bad_script(out)) {
            Ok(s) => Ok((std::fs::read(out).unwrap(), s.event_count())),
            Err(e) => Err(e),
        }
    };

    let out1 = tmp("ref-out1");
    let out2 = tmp("ref-out2");
    let err1 = run(&out1).expect_err("the mismatch is refused");
    let err2 = run(&out2).expect_err("refused identically");
    assert!(err1.contains("signal kind mismatch"), "got: {err1}");
    assert_eq!(err1, err2, "the refusal is identical on both runs");
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);
}

/// The text format (versioned) parses into the contract's command list, and
/// out-of-range channels are refused at parse time.
#[test]
fn text_format_parses() {
    let text = r#"
        # the canonical script, in text form
        host v1
        mount euclidean steps=8 pulses=1 rotation=1 pulses_per_beat=4 @0
        mount scale root=0 @0
        mount tone gain=0.25 blip_len=1200 @0
        mount mixer channels=4 @0
        patch euclidean.triggers scale.trigger @0
        patch scale.note tone.note @0
        patch tone.audio mixer.ch0 @0
        play /tmp/clip-a.wav ch0 @0
        set_param mixer ch0.gain 0.5 @2000
        splice 4000 /tmp/clip-b.wav 512
        bounce 6000 /tmp/out.wav
    "#;
    let commands = parse_script(text).unwrap();
    assert_eq!(commands.len(), 11);
    match &commands[8] {
        HostCommand::SetParam { at_frame: Some(2_000), .. } => {}
        other => panic!("expected the gain change at 2000, got {other:?}"),
    }
    match &commands[9] {
        HostCommand::Splice { at_frame, crossfade, .. } => {
            assert_eq!(*at_frame, 4_000);
            assert_eq!(*crossfade, 512);
        }
        other => panic!("expected splice, got {other:?}"),
    }

    // version mismatch refused
    assert!(parse_script("host v99\nbounce 100 /tmp/x.wav").is_err());
    assert!(parse_script("mount tone\n").is_err(), "missing version line");
    // unknown names refused per slot
    assert!(parse_script("host v1\nmount reverb\n").is_err());
    assert!(parse_script("host v1\npatch tone.audio mixer.ch9\n").is_err());
    // out-of-range channel refused by channel_of
    assert!(parse_script("host v1\nplay /tmp/a.wav ch9\n").is_err());
}

/// The events/values half of the contract: after a script with the mixer
/// mounted, the host renders the meters and the provider registry.
#[test]
fn events_and_values_are_observable() {
    let a = tmp("ev-a");
    let b = tmp("ev-b");
    let out = tmp("ev-out");
    write_tone(&a, 440.0, SR as u64);
    write_tone(&b, 880.0, SR as u64);
    let s = run_script(&script(&a, &b, &out)).expect("script runs");
    let providers = s.providers(engine::SignalKind::Audio);
    assert!(providers.contains(&"tone"), "the tone is an audio provider: {providers:?}");
    let meters = s.meters().expect("the mixer's meters are provided");
    assert!(meters.master_peak().is_finite(), "the master meter is readable");
    assert!(s.event_count() > 0, "the log reflects the applied commands");
    assert!(s.media_command_count() >= 3, "play + splice + bounce are reported separately");
    for p in [&a, &b, &out] {
        let _ = std::fs::remove_file(p);
    }
}

/// The actual CLI binary — the headless smoke binary — records and bounces
/// without any frontend (composition-seams acceptance).
#[test]
fn smoke_binary_records_and_bounces() {
    let a = tmp("smoke-a");
    let b = tmp("smoke-b");
    let out = tmp("smoke-out");
    let script_file = std::env::temp_dir().join(format!("host-ref-script-{}.txt", std::process::id()));
    write_tone(&a, 440.0, SR as u64);
    write_tone(&b, 660.0, SR as u64);
    let text = format!(
        "host v1\nmount mixer channels=4 @0\nplay {} ch0 @0\nsplice 30000 {} 512\nbounce 40000 {}\n",
        a.display(),
        b.display(),
        out.display()
    );
    std::fs::write(&script_file, &text).unwrap();

    let bin = env!("CARGO_BIN_EXE_host");
    let status = Command::new(bin)
        .arg(&script_file)
        .output()
        .expect("run the host binary");
    assert!(status.status.success(), "host exit: {:?}\n{}", status.status, String::from_utf8_lossy(&status.stderr));
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("bounced"), "the binary reports the bounce: {stdout}");
    assert!(out.exists(), "the bounce file exists");
    let mut r = WavReader::open(&out).unwrap();
    let mut back = vec![0.0f32; r.total_frames() as usize];
    r.read_into(&mut back);
    assert!(back.iter().any(|s| *s != 0.0), "the smoke binary's bounce is audible");

    // stdin path + determinism through the binary itself
    let out2 = tmp("smoke-out2");
    let text2 = text.replace(&out.display().to_string(), &out2.display().to_string());
    let status2 = Command::new(bin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(text2.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run the host binary via stdin");
    assert!(status2.status.success(), "stdin host: {:?}", status2.status);
    assert_eq!(
        std::fs::read(&out).unwrap(),
        std::fs::read(&out2).unwrap(),
        "the same script through the binary is deterministic"
    );

    // a bad script exits 2
    let bad_file = std::env::temp_dir().join(format!("host-ref-bad-{}.txt", std::process::id()));
    std::fs::write(&bad_file, "host v1\nmount reverb\n").unwrap();
    let bad = Command::new(bin).arg(&bad_file).output().expect("run");
    assert_eq!(bad.status.code(), Some(2), "bad script exits 2: {:?}", bad.status);

    for p in [&a, &b, &out, &out2, &script_file, &bad_file] {
        let _ = std::fs::remove_file(p);
    }
}

/// GLM-5.3 review #3: `play` after any render must not break the session. The
/// player node is placed BEFORE the mixer in topological order (insert_before),
/// so a mixer materialized by an earlier bounce no longer makes the player's
/// cord backward.
#[test]
fn play_after_a_render_does_not_break_the_session() {
    let clip = tmp("clip");
    write_tone(&clip, 440.0, 4096);
    let out1 = tmp("b1");
    let out2 = tmp("b2");

    let script = vec![
        HostCommand::Mount { plugin: "mixer", params: vec![("channels", 2.0)], at_frame: Some(0) },
        HostCommand::Bounce { frames: 512, path: out1.clone() }, // materialize the mixer
        HostCommand::Play { clip: media::ClipRef::whole(&clip).unwrap(), channel: 0, at_frame: Some(512) },
        HostCommand::Bounce { frames: 512, path: out2.clone() }, // failed before the fix
    ];
    let sess = run_script(&script).expect("play after a render must not break the session");
    assert_eq!(sess.underruns(), 0);
    let mut r = WavReader::open(&out2).unwrap();
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    assert!(audio[..n].iter().any(|s| s.abs() > 1e-4), "the post-play render must actually produce audio");

    let _ = std::fs::remove_file(&clip);
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);
}
