use super::*;

/// No `Arrange` command has run, so there is no editor: `arrangement()` is
/// `Ok(default)` — the legitimate "nothing built yet" case, never an error.
#[test]
fn arrangement_without_an_editor_is_ok_default() {
    let session = HostSession::new();
    assert_eq!(
        session
            .arrangement()
            .expect("no editor must be Ok, never Err"),
        media::Timeline::default(),
        "no editor → the default (empty) timeline"
    );
}

/// A fade pair whose **sum** overflows `u64` is refused, and refusing it does
/// not damage the session. Both fade operands reach the host as raw `u64`
/// (`arrange set_clip_fade t0 c0 18446744073709551615 1`), so the sum leaves
/// the range and wraps to 0 — which used to pass the length check: a debug
/// build panicked on the overflow *inside `ClipEditor::apply`'s lock*, and
/// the poisoned mutex made every later `arrangement()` an `Err`; a release
/// build admitted the op, logged it, and rendered the clip at gain 0 for
/// every sample.
///
/// Both halves are pinned here: the op is an `Err` naming the fade rule (not
/// a panic, not a silent success), and the editor is still readable and
/// writable afterwards — the clip keeps its fades and a legal `SetClipFade`
/// still applies, which is what "refused, never corrupted" means.
#[test]
fn an_overflowing_fade_pair_is_refused_and_the_editor_stays_usable() {
    let mut session = HostSession::new();
    session
        .ensure_editor()
        .expect("the editor registers its op handlers");
    let editor = session
        .editor
        .as_mut()
        .expect("ensure_editor built the editor");

    // A valid track + clip so the overflowing SetClipFade reaches the fade
    // check (an absent clip would refuse before it).
    let mut engine = Engine::new(48_000, 120.0, 4);
    editor.register(&mut engine).expect("register op handlers");
    editor
        .apply(
            &mut engine,
            &media::ArrangeOp::AddTrack { track: "t0".into() },
        )
        .expect("add track");
    editor
        .apply(
            &mut engine,
            &media::ArrangeOp::AddClip {
                track: "t0".into(),
                clip: media::Clip {
                    reversed: false,
                    id: "c0".into(),
                    name: None,
                    source: "s1".into(),
                    src_start: 0,
                    src_len: 4000,
                    at_frame: 0,
                    fade_in: 0,
                    fade_out: 0,
                    gain: 1.0,
                    loop_len: None,
                },
            },
        )
        .expect("add clip");

    // u64::MAX + 1 wraps to 0, so `0 > src_len` is false — the pair must be
    // refused by name, never admitted and never a panic under the lock.
    let err = editor
        .apply(
            &mut engine,
            &media::ArrangeOp::SetClipFade {
                track: "t0".into(),
                clip: "c0".into(),
                fade_in: u64::MAX,
                fade_out: 1,
            },
        )
        .expect_err("a wrapped fade sum must be refused, not accepted");
    assert!(
        err.contains("fades"),
        "the refusal names the fade rule, got: {err}"
    );
    // A legal pair over the same clip still applies, so the editor is writable.
    editor
        .apply(
            &mut engine,
            &media::ArrangeOp::SetClipFade {
                track: "t0".into(),
                clip: "c0".into(),
                fade_in: 64,
                fade_out: 128,
            },
        )
        .expect("a legal SetClipFade still applies after the refusal");

    // The refusal left the session readable: a poisoned timeline would make
    // this `Err`, and the legal pair above is the only fade in the value.
    let c = &session
        .arrangement()
        .expect("a refused op must not poison the timeline")
        .tracks[0]
        .clips[0];
    assert_eq!(
        (c.fade_in, c.fade_out),
        (64, 128),
        "the value carries the legal pair, never the refused one"
    );
}

/// Transport play/stop toggles the playing flag; the offline host records
/// the state (a live runtime pumps; the reference host renders via Bounce).
#[test]
fn transport_play_stop_toggles_playing() {
    let mut s = HostSession::new();
    assert!(!s.is_playing(), "a fresh session is stopped");
    assert_eq!(s.position().frame, 0);
    s.execute(&HostCommand::TransportPlay).expect("play");
    assert!(s.is_playing());
    s.execute(&HostCommand::TransportStop).expect("stop");
    assert!(!s.is_playing());
    assert!(!s.position().playing);
}

/// Forward seek renders to the target frame (the core clock only advances by
/// rendering) and the position reports the tempo-derived musical time.
#[test]
fn transport_seek_forward_renders_to_target() {
    let mut s = HostSession::new();
    s.execute(&HostCommand::TransportSeek { frame: 4_800 })
        .expect("seek");
    let p = s.position();
    assert_eq!(p.frame, 4_800, "seek forward renders to the target");
    assert!((p.seconds - 0.1).abs() < 1e-9, "4800 frames @48k = 0.1 s");
    assert!((p.beat - 0.2).abs() < 1e-6, "120 bpm, 0.1 s = 0.2 beats");
    assert!(
        (p.bpm - 120.0).abs() < 1e-9,
        "the tempo map reports 120 bpm"
    );
}

/// Backward seek rebuilds from the state-command history (the core clock
/// cannot run backwards) and the arrangement value survives the rebuild.
#[test]
fn transport_seek_backward_rebuilds_and_preserves_state() {
    let pool = std::env::temp_dir().join(format!("host-seek-pool-{}", std::process::id()));
    std::fs::create_dir_all(&pool).expect("pool dir");
    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mount mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: Some(0),
    })
    .expect("add track");

    s.execute(&HostCommand::TransportSeek { frame: 9_600 })
        .expect("seek forward");
    assert_eq!(s.position().frame, 9_600);
    s.execute(&HostCommand::TransportSeek { frame: 1_000 })
        .expect("seek backward");
    assert_eq!(
        s.position().frame,
        1_000,
        "backward seek rebuilds to the target"
    );
    assert_eq!(
        s.arrangement().expect("arrangement").tracks.len(),
        1,
        "the arrangement state survives the rebuild"
    );
    let _ = std::fs::remove_dir_all(&pool);
}

/// A single `arrange …` line parses to its op + optional frame; the `arrange`
/// keyword is optional (only the operands are required).
#[test]
fn parse_arrange_line_takes_the_op_and_an_optional_frame() {
    let (op, at) = parse_arrange_line("arrange move_clip t0 c0 9600 @48000").expect("parse");
    assert!(matches!(
        op,
        media::ArrangeOp::MoveClip {
            at_frame: 9_600,
            ..
        }
    ));
    assert_eq!(at, Some(48_000));

    let (op, at) = parse_arrange_line("trim t0 c0 end -24000").expect("parse without the keyword");
    assert!(matches!(
        op,
        media::ArrangeOp::Trim {
            by_frames: -24_000,
            ..
        }
    ));
    assert_eq!(at, None);

    assert!(parse_arrange_line("").is_err(), "an empty line is refused");
    assert!(
        parse_arrange_line("arrange nonsense t0").is_err(),
        "an unknown op is refused"
    );
}

// ---- undo/redo fixtures ----
/// A short mono float-WAV take, so an arrangement can actually wire a reader
/// (a render — which `seek_to`/undo do — opens every clip's source).
fn write_take(dir: &std::path::Path, id: &str, frames: usize) {
    let path = dir.join(format!("{id}.wav"));
    let mut w = media::WavWriter::create(&path, 48_000, 1).expect("wav writer");
    w.write(&vec![0.0f32; frames]).expect("write take");
    w.finalize().expect("finalize take");
}

// -- rig: declared sources are state, never hardware ----------------------

fn source_add(name: &str) -> HostCommand {
    HostCommand::SourceAdd {
        name: name.into(),
        kind: "alsa",
        matcher: "hw:USB".into(),
        channels: 2,
        clock: rig::ClockRole::Follower,
    }
}

#[test]
fn a_declared_rig_replays_with_no_hardware() {
    // Format → parse round-trips the exact line, and a rebuilt session
    // carries the declarations back — no device opened, bound or probed
    // anywhere on this path (the takes slice's purity rule).
    let line = format_command(
        &HostCommand::SourceAdd {
            name: "synth".into(),
            kind: "alsa",
            matcher: "hw:USB".into(),
            channels: 8,
            clock: rig::ClockRole::Master,
        },
        None,
    )
    .expect("a SourceAdd formats");
    assert_eq!(
        line,
        "source add synth kind=alsa match=hw:USB channels=8 clock=master"
    );
    let parsed =
        parse_script(&format!("host v{HOST_API_VERSION}\n{line}\n")).expect("the line parses back");
    assert_eq!(parsed.len(), 1);
    let s = HostSession::from_script(&parsed).expect("a rig rebuilds with no device");
    let sources = s.sources();
    assert_eq!(sources.len(), 1, "the declaration survives the rebuild");
    assert_eq!(sources[0].name, "synth");
    assert_eq!(sources[0].kind, "alsa");
    assert_eq!(sources[0].matcher, "hw:USB");
    assert_eq!(sources[0].channels, 8);
    assert_eq!(sources[0].clock, rig::ClockRole::Master);
}

#[test]
fn source_add_refuses_bad_names_kinds_clocks_and_widths() {
    let mut s = HostSession::new();
    s.execute(&source_add("synth")).expect("first declaration");

    // The name goes into the log and (later) a device binding, so it uses the
    // same character discipline as a take id.
    let bad_name = s.execute(&HostCommand::SourceAdd {
        name: "bad name!".into(),
        kind: "alsa",
        matcher: "hw:0".into(),
        channels: 2,
        clock: rig::ClockRole::Free,
    });
    assert!(bad_name.is_err(), "an unusable name is refused");

    // A duplicate address is a bug, not a merge.
    let dup = s.execute(&source_add("synth"));
    assert!(dup.is_err(), "a duplicate source name is refused");

    // channels references the capture sanity bound, never a literal.
    let zero = s.execute(&HostCommand::SourceAdd {
        name: "zero".into(),
        kind: "alsa",
        matcher: "hw:0".into(),
        channels: 0,
        clock: rig::ClockRole::Free,
    });
    assert!(zero.is_err(), "channels=0 is refused");
    let absurd = s.execute(&HostCommand::SourceAdd {
        name: "absurd".into(),
        kind: "alsa",
        matcher: "hw:0".into(),
        channels: media::capture::CAPTURE_CHANNELS_SANITY + 1,
        clock: rig::ClockRole::Free,
    });
    assert!(absurd.is_err(), "an absurd channel count is refused");

    // A refused declaration is never logged: nothing landed in the session.
    assert_eq!(s.sources().len(), 1, "only the first declaration landed");
}

/// The operands the log spells as **bare words** must be one token: the matcher of
/// a declared source, and the source a tempo is recorded for. A space in either (or
/// a `#`, which starts a comment) produces a line `parse_script` cannot read back, so
/// the session that holds it cannot be reopened — so the command is refused at the
/// door, like the name beside it.
#[test]
fn a_source_and_a_matcher_the_text_form_cannot_spell_are_refused() {
    let mut s = HostSession::new();
    let declare = |matcher: &str| HostCommand::SourceAdd {
        name: "s1".into(),
        kind: "alsa",
        matcher: matcher.into(),
        channels: 2,
        clock: rig::ClockRole::Follower,
    };

    // A matcher with a space is one token too many in `match=…`.
    let spaced = s.execute(&declare("hw:USB 1"));
    assert!(spaced.is_err(), "a matcher with a space is refused");
    // A `#` is a comment to the parser.
    let hashed = s.execute(&declare("hw:#0"));
    assert!(hashed.is_err(), "a matcher with a '#' is refused");
    assert!(s.execute(&declare("")).is_err(), "and so is an empty one");
    assert!(
        s.sources().is_empty(),
        "no refused declaration is logged: {}",
        spaced.expect_err("the error")
    );

    // `source_tempo <id> <bpm>` spells the id as a word, so it takes the same
    // discipline as the declaration's name.
    let spaced_source = s.execute(&HostCommand::SetSourceTempo {
        source: "jam ch0".into(),
        bpm: 90.0,
    });
    assert!(
        spaced_source.is_err(),
        "a source id with a space is refused"
    );
    assert!(s.source_tempos().is_empty(), "and no tempo is recorded");
    s.execute(&HostCommand::SetSourceTempo {
        source: "jam.ch0".into(),
        bpm: 90.0,
    })
    .expect("a spellable source id still records");
    assert_eq!(s.source_tempos().get("jam.ch0"), Some(&90.0));
}

#[test]
fn source_add_parses_loudly_or_not_at_all() {
    let head = format!("host v{HOST_API_VERSION}\n");
    // An unknown kind is a parse error naming the registry, not a silent
    // declaration that can never bind.
    let err = parse_script(&format!(
        "{head}source add s kind=firewire match=hw:0 channels=2 clock=free\n"
    ))
    .expect_err("unknown kind");
    assert!(
        err.contains("unknown source kind"),
        "and it says why: {err}"
    );

    // A malformed clock role is refused — no default, no guessing.
    let err = parse_script(&format!(
        "{head}source add s kind=alsa match=hw:0 channels=2 clock=slave\n"
    ))
    .expect_err("bad clock role");
    assert!(err.contains("clock role"), "and it says why: {err}");

    // The tokenizer is whitespace-based, so a matcher with a space is one
    // token too many — refused rather than silently truncated.
    let err = parse_script(&format!(
        "{head}source add s kind=alsa match=hw:0 extra channels=2 clock=free\n"
    ))
    .expect_err("extra token");
    assert!(
        err.contains("operand"),
        "the extra token is the refusal: {err}"
    );
}

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

fn move_clip(id: &str, at: u64) -> media::ArrangeOp {
    media::ArrangeOp::MoveClip {
        track: "t0".into(),
        clip: id.into(),
        at_frame: at,
    }
}

/// Undo drops the most recent arrangement edit and rebuilds **to the current
/// position**; redo re-applies it; a new edit clears the redo branch.
#[test]
fn undo_and_redo_revert_and_reapply_an_edit() {
    let pool = std::env::temp_dir().join(format!("host-undo-pool-{}", std::process::id()));
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_take(&pool, "s1", 48_000);
    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mount mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("add track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("add clip");

    assert!(s.can_undo(), "edits are undoable");
    assert!(!s.can_redo(), "nothing is undone yet");

    // Play to 9600 so we can prove an undo does not rewind the playhead.
    s.execute(&HostCommand::TransportSeek { frame: 9_600 })
        .expect("seek");
    s.execute(&HostCommand::Arrange {
        op: move_clip("c0", 4_800),
        at_frame: None,
    })
    .expect("move");
    assert_eq!(
        s.arrangement().expect("tl").tracks[0].clips[0].at_frame,
        4_800
    );

    assert!(
        s.undo().expect("undo works"),
        "undo reports an edit was undone"
    );
    assert_eq!(
        s.arrangement().expect("tl").tracks[0].clips[0].at_frame,
        0,
        "the move is reverted"
    );
    assert_eq!(
        s.position().frame,
        9_600,
        "undo keeps the playhead where it was"
    );
    assert!(s.can_redo(), "the undone edit can be redone");

    assert!(s.redo().expect("redo works"));
    assert_eq!(
        s.arrangement().expect("tl").tracks[0].clips[0].at_frame,
        4_800,
        "redo re-applies the move"
    );
    assert!(!s.can_redo(), "the redo branch is consumed");

    // A new edit discards the redo branch.
    s.undo().expect("undo again");
    assert!(s.can_redo());
    s.execute(&HostCommand::Arrange {
        op: move_clip("c0", 20_000),
        at_frame: None,
    })
    .expect("a new edit");
    assert!(!s.can_redo(), "a new edit clears the redo branch");

    let _ = std::fs::remove_dir_all(&pool);
}

/// Nothing to undo is a clean no-op — and session setup is not an edit.
#[test]
fn undo_with_nothing_to_undo_is_a_noop() {
    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mount mixer");
    assert!(!s.can_undo(), "a mount is session setup, not an edit");
    assert!(!s.undo().expect("undo is a no-op"), "nothing was undone");
    assert!(!s.redo().expect("redo is a no-op"), "nothing was redone");
    assert_eq!(s.position().frame, 0);
}

/// **A refused replay changes nothing** — the contract `seek_to` states, owed by an
/// undo too. The rebuild re-applies the *remaining* history, so it can be refused
/// for a reason this edit has nothing to do with (the ordinary one being a pool
/// directory that moved or was deleted). The edit then has to go back where it came
/// from: out of the history and onto the redo branch it would leave the history
/// describing a session that never existed — the next `save` would write a script
/// the live arrangement no longer matches, `can_undo`/`can_redo` would report a
/// state that is not there, and the next `undo` would revert a *different* edit.
#[test]
fn a_refused_undo_leaves_the_history_alone() {
    let (mut s, pool) = session_with_clip("undo-refused");
    s.execute(&HostCommand::Arrange {
        op: move_clip("c0", 4_800),
        at_frame: None,
    })
    .expect("move");

    // The pool goes away under the session: the live arrangement value still holds
    // the clip, but a rebuild cannot re-open the pool.
    std::fs::remove_dir_all(&pool).expect("remove pool");
    let err = s.undo().expect_err("the rebuild refuses without the pool");
    assert!(err.contains("pool"), "and it says why: {err}");
    assert_eq!(
        clip_of(&s).at_frame,
        4_800,
        "the refused undo did not revert the move"
    );
    assert!(s.can_undo(), "the edit is still there to undo");
    assert!(
        !s.can_redo(),
        "nothing was undone, so there is nothing to redo"
    );

    // Put the pool back: the next undo undoes the *same* edit, not another one.
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_take(&pool, "s1", 48_000);
    assert!(s.undo().expect("undo works"), "the move is undone");
    let tl = s.arrangement().expect("tl");
    assert_eq!(
        tl.tracks[0].clips.len(),
        1,
        "the clip's own add was not what got undone"
    );
    assert_eq!(tl.tracks[0].clips[0].at_frame, 0, "the move is reverted");

    let _ = std::fs::remove_dir_all(&pool);
}

/// The mirror: a refused **redo** leaves the redo branch whole. Popping the entry
/// and inserting it before the replay would destroy it outright — the one edit the
/// branch held is gone and cannot be brought back.
#[test]
fn a_refused_redo_keeps_the_redo_branch() {
    let (mut s, pool) = session_with_clip("redo-refused");
    s.execute(&HostCommand::Arrange {
        op: move_clip("c0", 4_800),
        at_frame: None,
    })
    .expect("move");
    assert!(s.undo().expect("undo works"), "the move is undone");

    std::fs::remove_dir_all(&pool).expect("remove pool");
    let err = s.redo().expect_err("the rebuild refuses without the pool");
    assert!(err.contains("pool"), "and it says why: {err}");
    assert_eq!(
        clip_of(&s).at_frame,
        0,
        "the refused redo did not re-apply the move"
    );
    assert!(s.can_redo(), "the edit is still there to redo");

    std::fs::create_dir_all(&pool).expect("pool dir");
    write_take(&pool, "s1", 48_000);
    assert!(s.redo().expect("redo works"), "the move is re-applied");
    assert_eq!(clip_of(&s).at_frame, 4_800, "and it is the move");

    let _ = std::fs::remove_dir_all(&pool);
}

/// The undo / redo grammar lines parse.
#[test]
fn undo_and_redo_lines_parse() {
    let cmds = parse_script("host v1\nundo\nredo\n").expect("parse");
    assert!(matches!(cmds[0], HostCommand::Undo));
    assert!(matches!(cmds[1], HostCommand::Redo));
}

/// The transport text grammar parses play / seek / stop.
#[test]
fn transport_lines_parse() {
    let cmds = parse_script("host v1\ntransport play\ntransport seek 4800\ntransport stop\n")
        .expect("transport lines parse");
    assert!(matches!(&cmds[0], HostCommand::TransportPlay));
    assert!(matches!(&cmds[2], HostCommand::TransportStop));
    match &cmds[1] {
        HostCommand::TransportSeek { frame } => assert_eq!(*frame, 4_800),
        other => panic!("expected a seek, got {other:?}"),
    }
}

/// `export <path> [f32|s16]` — f32 when the format is omitted, `s16` when asked,
/// a bad format refused (and never logged: an action has no log form).
#[test]
fn export_lines_parse() {
    let cmds = parse_script("host v1\nexport /tmp/a.wav\nexport /tmp/b.wav s16\n")
        .expect("export lines parse");
    match (&cmds[0], &cmds[1]) {
        (
            HostCommand::Export { path, format },
            HostCommand::Export {
                path: p2,
                format: f2,
            },
        ) => {
            assert_eq!(path, &PathBuf::from("/tmp/a.wav"));
            assert_eq!(*format, ExportFormat::F32, "f32 is the default");
            assert_eq!(p2, &PathBuf::from("/tmp/b.wav"));
            assert_eq!(*f2, ExportFormat::S16);
        }
        other => panic!("expected two exports, got {other:?}"),
    }
    assert!(
        parse_script("host v1\nexport /tmp/a.wav flac\n").is_err(),
        "an unknown format is refused"
    );
    // Clearing a clip name: the 3-word `rename_clip` parses to an empty name, and
    // the formatter emits exactly that line (the gate's must-fix: an empty operand
    // cannot be spelled, so "clear" needed its own shape).
    let cleared = parse_script("host v1\narrange rename_clip t0 c0\n").expect("3 words parse");
    match &cleared[0] {
        HostCommand::Arrange { op, .. } => {
            assert!(
                matches!(op, media::ArrangeOp::RenameClip { name, .. } if name.is_empty()),
                "3 words clear the label: {op:?}"
            );
            assert_eq!(
                format_arrange(op),
                "rename_clip t0 c0",
                "and the formatter emits the line the parser accepts"
            );
        }
        other => panic!("expected an arrange command, got {other:?}"),
    }
    assert!(
        format_command(
            &HostCommand::Export {
                path: PathBuf::from("/tmp/a.wav"),
                format: ExportFormat::F32,
            },
            None
        )
        .is_none(),
        "an export is an action: it has no log form (its report is logged instead)"
    );
}

// ---- gestures: one entry, one undo, all-or-nothing ----

/// Build a session with a mixer, a pool holding one take, and one 4800-frame
/// clip at frame 0 on `t0`.
fn session_with_clip(name: &str) -> (HostSession, std::path::PathBuf) {
    let pool = std::env::temp_dir().join(format!("host-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_take(&pool, "s1", 48_000);
    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mount mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("add track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("add clip");
    (s, pool)
}

/// A session with a mixer, a pool, one track and one clip at frame 0.
fn session_with_pool(_name: &str, pool: &std::path::Path) -> HostSession {
    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mount mixer");
    s.execute(&HostCommand::Pool {
        dir: pool.to_path_buf(),
    })
    .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("add track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("add clip");
    s
}

fn clip_of(s: &HostSession) -> media::Clip {
    s.arrangement().expect("tl").tracks[0].clips[0].clone()
}

fn gesture(ops: Vec<media::ArrangeOp>) -> HostCommand {
    HostCommand::Group {
        commands: ops
            .into_iter()
            .map(|op| HostCommand::Arrange { op, at_frame: None })
            .collect(),
    }
}

// ---- a clip source is a pool id, not a path ----

/// A pool directory with one take in it, and a readable WAV **beside** it (the
/// file an escaping source would reach).
fn pool_with_a_neighbour_take(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);
    write_tone(&root, "secret", 48_000, 48_000);
    assert!(
        pool.join("../secret.wav").is_file(),
        "the escaping source has something to reach"
    );
    (pool, root)
}

/// **A clip `source` is a pool id, not a path.** The operand is an arbitrary
/// whitespace-free token, and a session file is a document a person can edit, so
/// `add_clip t0 c0 ../secret …` parses and used to land: the clip's source was
/// joined onto the pool directory, the file was streamed through the arranger, and
/// its audio came out in bounces and exports — a session file that reads an
/// arbitrary readable WAV. The value model refuses the source by name, so the
/// gesture is not applied and nothing is logged.
#[test]
fn a_clip_source_that_would_escape_the_pool_is_refused() {
    let (pool, root) = pool_with_a_neighbour_take("pool-escape");
    let head = format!(
        "host v1\nmount mixer channels=2 @0\npool {}\narrange add_track t0\n",
        pool.display()
    );

    let err = run_script(
        &parse_script(&format!(
            "{head}arrange add_clip t0 c0 ../secret 0 4800 0 0 0 1.0\n"
        ))
        .expect("the script parses"),
    )
    .expect_err("a source that is not a pool id is refused");
    assert!(err.contains("not a pool id"), "the refusal says why: {err}");

    // A pool id is unaffected, and the value holds nothing the log could not name.
    let s = run_script(
        &parse_script(&format!(
            "{head}arrange add_clip t0 c0 s1 0 4800 0 0 0 1.0\n"
        ))
        .expect("the script parses"),
    )
    .expect("a pool id still places a clip");
    assert_eq!(clip_of(&s).source, "s1");

    let _ = std::fs::remove_dir_all(&root);
}

/// **The resolver the production `set_pool` builds is the pool's guarded lookup.**
/// This is the seam every read of a clip's source goes through — the arranger
/// (`ArrangerNode::new`), `stretch` and the export — and it used to `join` the id
/// onto the pool directory itself, so `../secret` resolved to a path outside the
/// pool. It is the same closure the value check backs up, so it is pinned here
/// directly: an id that is not a plain pool id resolves to *nothing*.
#[test]
fn the_pool_resolver_only_resolves_a_plain_pool_id() {
    let (pool, root) = pool_with_a_neighbour_take("pool-guard");
    let s = session_with_pool("resolver-guard", &pool);
    let resolve = s.pool_resolver.clone().expect("a pool is set");
    assert_eq!(
        resolve("s1"),
        Some(pool.join("s1.wav")),
        "a plain id resolves"
    );
    for bad in ["../secret", "..", "sub/s1", "secret "] {
        assert!(
            resolve(bad).is_none(),
            "'{bad}' is not a plain pool id, so it resolves to nothing"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// **One gesture is one undo.** A group of two trims lands as a single history
/// entry: one `undo` restores the clip whole (not half-trimmed), and one `redo`
/// re-applies the whole gesture.
#[test]
fn a_group_is_one_undo_step() {
    let (mut s, pool) = session_with_clip("gesture-undo");
    let before = clip_of(&s);

    // Two edges move: the exact shape that used to take two undos.
    s.execute(&gesture(vec![
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::Start,
            by_frames: 1_200,
        },
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::End,
            by_frames: -600,
        },
    ]))
    .expect("the gesture applies");

    let after = clip_of(&s);
    assert_eq!(
        (after.at_frame, after.src_start, after.src_len),
        (1_200, 1_200, 3_000),
        "both trims landed"
    );

    assert!(s.undo().expect("undo works"), "the gesture is undoable");
    assert_eq!(clip_of(&s), before, "one undo restores the whole gesture");
    assert!(s.can_redo(), "and it can be redone");

    assert!(s.redo().expect("redo works"));
    assert_eq!(clip_of(&s), after, "redo re-applies the whole gesture");

    let _ = std::fs::remove_dir_all(&pool);
}

/// **All-or-nothing.** A member the model refuses means no member is applied and
/// nothing is logged — the session is exactly as it was.
#[test]
fn a_refused_group_changes_nothing() {
    let (mut s, pool) = session_with_clip("gesture-atomic");
    let before = clip_of(&s);
    let events = s.event_count();
    let commands = s.media_command_count();

    // The second trim would consume the whole clip — the model refuses it.
    let refused = s.execute(&gesture(vec![
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::Start,
            by_frames: 1_200,
        },
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::End,
            by_frames: -4_800,
        },
    ]));
    assert!(refused.is_err(), "the group is refused");
    let message = refused.expect_err("an error");
    assert!(
        message.contains("nothing applied"),
        "the refusal says so: {message}"
    );
    assert_eq!(clip_of(&s), before, "the first trim was rolled back");
    assert_eq!(s.event_count(), events, "and nothing was logged");
    assert_eq!(
        s.media_command_count(),
        commands,
        "no media command counted"
    );
    assert!(s.undo().is_ok(), "the session is still usable");

    let _ = std::fs::remove_dir_all(&pool);
}

/// A gesture is one moment over arrangement ops: a non-arrange member, or members
/// at different frames, is refused (a group must be skippable/appliable whole by
/// `replay_to`).
#[test]
fn a_group_is_one_frame_of_arrangement_ops() {
    let (mut s, pool) = session_with_clip("gesture-shape");

    let mixed_frames = s.execute(&HostCommand::Group {
        commands: vec![
            HostCommand::Arrange {
                op: move_clip("c0", 1_000),
                at_frame: None,
            },
            HostCommand::Arrange {
                op: move_clip("c0", 2_000),
                at_frame: Some(4_800),
            },
        ],
    });
    assert!(mixed_frames.is_err(), "one gesture happens at one frame");

    let not_arrangement = s.execute(&HostCommand::Group {
        commands: vec![HostCommand::SetParam {
            plugin: "mixer",
            param: "master.gain",
            value: 0.5,
            at_frame: None,
        }],
    });
    assert!(not_arrangement.is_err(), "a group is arrangement ops only");

    let _ = std::fs::remove_dir_all(&pool);
}

/// The text form round-trips gestures, and grouping changes the **host history**,
/// not the engine's log: the same ops grouped and ungrouped produce the same
/// events and the same audio.
#[test]
fn a_grouped_script_round_trips_and_logs_the_same_events() {
    let pool = std::env::temp_dir().join(format!("host-group-script-{}", std::process::id()));
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_take(&pool, "s1", 48_000);

    let head = format!(
        "host v1\nmount mixer channels=2 @0\npool {}\narrange add_track t0\narrange add_clip t0 c0 s1 0 4800 0 0 0 1.0\n",
        pool.display()
    );
    let grouped = format!(
        "{head}group begin\narrange trim t0 c0 start 1200\narrange trim t0 c0 end -600\ngroup end\n"
    );
    let loose = format!("{head}arrange trim t0 c0 start 1200\narrange trim t0 c0 end -600\n");

    // The markers parse into one Group command.
    let parsed = parse_script(&grouped).expect("the grouped script parses");
    match parsed.last().expect("a command") {
        HostCommand::Group { commands } => assert_eq!(commands.len(), 2, "two members"),
        other => panic!("expected a group, got {other:?}"),
    }

    let grouped_session = run_script(&parse_script(&grouped).expect("parse")).expect("run");
    let loose_session = run_script(&parse_script(&loose).expect("parse")).expect("run");
    assert_eq!(
        grouped_session.arrangement().expect("tl"),
        loose_session.arrangement().expect("tl"),
        "grouping does not change the value"
    );
    assert_eq!(
        grouped_session.event_count(),
        loose_session.event_count(),
        "grouping does not change the engine's log"
    );

    // …and it is one undo step, where the loose form takes two.
    let mut grouped_session = grouped_session;
    assert!(grouped_session.undo().expect("undo"));
    assert_eq!(
        clip_of(&grouped_session).src_len,
        4_800,
        "one undo restores the grouped gesture whole"
    );

    // Malformed group syntax is refused loudly.
    for bad in [
        "host v1\ngroup begin\narrange add_track t0\n",
        "host v1\ngroup end\n",
        "host v1\ngroup begin\ngroup begin\n",
        "host v1\ngroup begin\ngroup end\n",
    ] {
        assert!(parse_script(bad).is_err(), "refused: {bad:?}");
    }

    let _ = std::fs::remove_dir_all(&pool);
}

// ---- persistence: a session is a directory ----

/// A tone take (not silence) so a bounce can be asserted *audible*.
/// A 0.6 sine with a **spike** at `spike_at` (three frames to 1.4): the spike is
/// the alignment marker an offline render must not shift, and the sine is what the
/// compressor's steady-state gain reduction shows up in.
fn write_spiked_tone(dir: &std::path::Path, id: &str, frames: usize, spike_at: usize) {
    let path = dir.join(format!("{id}.wav"));
    let mut w = media::WavWriter::create_float(&path, 48_000, 1).expect("wav writer");
    let samples: Vec<f32> = (0..frames)
        .map(|i| {
            let v = (i as f32 * 0.05).sin() * 0.6;
            if i >= spike_at && i < spike_at + 3 {
                1.4
            } else {
                v
            }
        })
        .collect();
    w.write(&samples).expect("write spiked tone");
    w.finalize().expect("finalize spiked tone");
}

fn write_tone(dir: &std::path::Path, id: &str, frames: usize, rate: u32) {
    let path = dir.join(format!("{id}.wav"));
    let mut w = media::WavWriter::create(&path, rate, 1).expect("wav writer");
    let samples: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    w.write(&samples).expect("write tone");
    w.finalize().expect("finalize tone");
}

fn bounce(session: &mut HostSession, frames: usize, path: &std::path::Path) -> Vec<f32> {
    session
        .execute(&HostCommand::Bounce {
            frames,
            path: path.to_path_buf(),
        })
        .expect("bounce");
    let mut r = media::WavReader::open(path).expect("bounce file");
    let mut audio = vec![0.0f32; r.total_frames() as usize];
    let n = r.read_into(&mut audio);
    audio.truncate(n);
    audio
}

/// The text form round-trips **every** state command — the property a saved
/// session rests on. A new op or a changed operand must be taught to the
/// formatter, and this is where it fails.
#[test]
fn the_text_form_round_trips_every_state_command() {
    let clip = media::Clip {
        reversed: false,
        id: "c0".into(),
        // A name travels through the text form like any other clip field.
        name: Some("take-2".into()),
        source: "s1".into(),
        src_start: 100,
        src_len: 4_800,
        at_frame: 200,
        fade_in: 10,
        fade_out: 20,
        gain: 0.75,
        loop_len: Some(2_400),
    };
    let ops = vec![
        media::ArrangeOp::AddTrack { track: "t0".into() },
        media::ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip.clone(),
        },
        media::ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "L".into(),
            new_right: "R".into(),
            at_frame: 2_400,
        },
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::Start,
            by_frames: -120,
        },
        media::ArrangeOp::MoveClip {
            track: "t0".into(),
            clip: "c0".into(),
            at_frame: 9_600,
        },
        media::ArrangeOp::MoveClipToTrack {
            from: "t0".into(),
            clip: "c0".into(),
            to: "t1".into(),
            at_frame: 9_600,
        },
        media::ArrangeOp::Duplicate {
            track: "t0".into(),
            clip: "c0".into(),
            new_id: "c1".into(),
        },
        media::ArrangeOp::Delete {
            track: "t0".into(),
            clip: "c1".into(),
        },
        media::ArrangeOp::SetClipGain {
            track: "t0".into(),
            clip: "c0".into(),
            gain: 0.5,
        },
        media::ArrangeOp::SetClipFade {
            track: "t0".into(),
            clip: "c0".into(),
            fade_in: 64,
            fade_out: 128,
        },
        media::ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 3,
        },
        media::ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4,
            prefix: "pre".into(),
        },
        media::ArrangeOp::RemoveTrack { track: "t1".into() },
        media::ArrangeOp::RenameTrack {
            track: "t0".into(),
            to: "lead".into(),
        },
        media::ArrangeOp::MoveTrack {
            track: "t0".into(),
            index: 1,
        },
        media::ArrangeOp::Reverse {
            track: "t0".into(),
            clip: "c0".into(),
        },
        media::ArrangeOp::RenameClip {
            track: "t0".into(),
            clip: "c0".into(),
            name: "chorus".into(),
        },
        // Clearing a label is the 3-word form: the empty name cannot be spelled as
        // an operand, and the formatter must emit the line the parser accepts.
        media::ArrangeOp::RenameClip {
            track: "t0".into(),
            clip: "c0".into(),
            name: String::new(),
        },
        media::ArrangeOp::SetMarker {
            at_frame: 4_800,
            name: "verse".into(),
        },
        media::ArrangeOp::RemoveMarker { at_frame: 4_800 },
    ];

    let mut commands: Vec<HostCommand> = vec![
        HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        },
        HostCommand::Mount {
            plugin: "tone",
            params: vec![("gain", 0.25), ("blip_len", 1_800.0)],
            at_frame: None,
        },
        HostCommand::Patch {
            from: ("euclidean", "triggers"),
            to: ("scale", "trigger"),
            at_frame: Some(0),
        },
        HostCommand::SetParam {
            plugin: "mixer",
            param: "ch0.gain",
            value: 0.7,
            at_frame: None,
        },
        HostCommand::SetTempo {
            bpm: 137.5,
            beats_per_bar: 3,
            at_frame: Some(0),
        },
        HostCommand::Unmount {
            plugin: "tone",
            at_frame: Some(4_800),
        },
        HostCommand::Pool {
            dir: PathBuf::from("/tmp/pool"),
        },
        HostCommand::SessionRate { hz: 48_000 },
        HostCommand::Play {
            clip: media::ClipRef {
                path: PathBuf::from("/tmp/a.wav"),
                start: 0,
                len: 0,
            },
            channel: 1,
            at_frame: Some(0),
        },
        HostCommand::Splice {
            at_frame: 4_000,
            clip: media::ClipRef {
                path: PathBuf::from("/tmp/b.wav"),
                start: 0,
                len: 0,
            },
            crossfade: 512,
        },
    ];
    commands.extend(
        ops.into_iter()
            .map(|op| HostCommand::Arrange { op, at_frame: None }),
    );
    commands.push(HostCommand::Take {
        take_id: "jam".into(),
        frames: 96_000,
        dropped: 0,
        channels: 2,
        at_frame: 0,
    });
    commands.push(HostCommand::Group {
        commands: vec![
            HostCommand::Arrange {
                op: media::ArrangeOp::Trim {
                    track: "t0".into(),
                    clip: "c0".into(),
                    edge: media::Edge::Start,
                    by_frames: 100,
                },
                at_frame: None,
            },
            HostCommand::Arrange {
                op: media::ArrangeOp::Trim {
                    track: "t0".into(),
                    clip: "c0".into(),
                    edge: media::Edge::End,
                    by_frames: -100,
                },
                at_frame: None,
            },
        ],
    });

    let mut text = String::from("host v1\n");
    for cmd in &commands {
        let line = format_command(cmd, None).expect("a state command has a text form");
        text.push_str(&line);
        text.push('\n');
    }
    let back = parse_script(&text).expect("the formatted script parses");
    assert_eq!(
        back, commands,
        "the text form round-trips every state command"
    );

    // A pure action has no text form, so it can never leak into a log.
    for action in [
        HostCommand::TransportPlay,
        HostCommand::TransportStop,
        HostCommand::Undo,
        HostCommand::Redo,
        HostCommand::Record {
            take_id: "t".into(),
        },
        HostCommand::Bounce {
            frames: 1,
            path: PathBuf::from("/tmp/x.wav"),
        },
        HostCommand::Save {
            dir: PathBuf::from("/tmp/x"),
        },
        HostCommand::Load {
            dir: PathBuf::from("/tmp/x"),
        },
    ] {
        assert!(
            format_command(&action, None).is_none(),
            "{action:?} must not be serializable"
        );
    }
}

/// Save → load reproduces the session: the same value, the same folded
/// parameters, the same tempo, and a **byte-identical bounce**.
#[test]
fn save_and_load_round_trips_a_session() {
    let root = std::env::temp_dir().join(format!("host-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("work");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::SetTempo {
        bpm: 96.0,
        beats_per_bar: 4,
        at_frame: Some(0),
    })
    .expect("tempo");
    s.execute(&HostCommand::SetParam {
        plugin: "mixer",
        param: "ch0.gain",
        value: 0.6,
        at_frame: None,
    })
    .expect("gain");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("clip");
    // A gesture, so the save has to carry group structure too.
    s.execute(&gesture(vec![
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::Start,
            by_frames: 1_200,
        },
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::End,
            by_frames: -600,
        },
    ]))
    .expect("gesture");

    let dir = root.join("mysong.d");
    s.save(&dir).expect("save");
    assert!(dir.join("session.txt").is_file(), "the script is written");
    assert!(
        dir.join("pool/s1.wav").is_file(),
        "the pool travels with it"
    );
    assert_eq!(s.session_dir(), Some(dir.as_path()));

    let mut loaded = HostSession::new();
    loaded.load_session(&dir).expect("load");
    assert_eq!(
        loaded.arrangement().expect("tl"),
        s.arrangement().expect("tl"),
        "the arrangement round-trips"
    );
    // A `set_tempo` is *scheduled* at its frame, so it takes effect when the clock
    // renders — the bounce below is what applies it.
    assert_eq!(loaded.position().bpm, 120.0, "nothing rendered yet");
    let gain = loaded
        .params()
        .iter()
        .find(|(p, k, _)| *p == "mixer" && *k == "ch0.gain")
        .map(|(_, _, v)| *v);
    assert_eq!(
        gain,
        Some(0.6),
        "the parameters round-trip (folded from the log)"
    );

    // …and the audio is identical, which is the property that matters.
    let a = bounce(&mut s, 4_800, &root.join("a.wav"));
    let b = bounce(&mut loaded, 4_800, &root.join("b.wav"));
    assert!(a.iter().any(|x| x.abs() > 1e-3), "the take is audible");
    assert_eq!(a, b, "save → load bounces the same audio");
    assert_eq!(s.position().bpm, 96.0, "the tempo applied on render");
    assert_eq!(
        loaded.position().bpm,
        96.0,
        "and the loaded session's tempo matches"
    );

    // A reloaded session is a *session*, not just a value: it can be saved again,
    // and the new script still holds the baseline (the load built the history).
    let again = root.join("again.d");
    loaded.save(&again).expect("save again");
    let text = std::fs::read_to_string(again.join("session.txt")).expect("second script");
    assert!(
        text.contains("arrange add_clip t0 c0 s1"),
        "the baseline is in the reloaded history:\n{text}"
    );
    assert!(text.contains("set_tempo 96"), "{text}");

    // A gesture is still one undo step after a reload.
    let before = clip_of(&loaded);
    loaded
        .execute(&gesture(vec![media::ArrangeOp::MoveClip {
            track: "t0".into(),
            clip: "c0".into(),
            at_frame: 24_000,
        }]))
        .expect("move");
    assert!(loaded.undo().expect("undo"));
    assert_eq!(clip_of(&loaded), before);

    let _ = std::fs::remove_dir_all(&root);
}

/// The session directory is **movable**: the script's pool path is relative, so
/// renaming the directory keeps the session playable (the plan's path trap).
/// **The stretch round trip**: a clip's material is rendered into a new pool
/// source at a rational ratio, the logged op points the clip at it, the pitch is
/// preserved while the length grows, one undo restores the old reference, and the
/// deterministic id means stretching twice reuses the same material instead of
/// piling up copies.
#[test]
fn stretching_a_clip_materialises_a_pool_source() {
    let root = std::env::temp_dir().join(format!("host-stretch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("clip");

    // A source tempo is state: the log carries it, the outcome exposes it.
    s.execute(&HostCommand::SetSourceTempo {
        source: "s1".into(),
        bpm: 90.0,
    })
    .expect("source tempo");
    assert_eq!(s.source_tempos().get("s1"), Some(&90.0));

    // Refusals first: a 1:1 ratio copies rather than stretches, a zero is not a
    // ratio, and an unbounded one is refused *before* the allocator sees it (the
    // gate executed `stretch t0 c0 4294967295 1` and the process died with an
    // 824 TB allocation failure — a logged line must not be able to do that).
    assert!(s.stretch("t0", "c0", 1, 1).is_err(), "1:1 is refused");
    assert!(
        s.stretch("t0", "c0", 0, 1).is_err(),
        "a zero ratio is refused"
    );
    assert!(
        s.stretch("t0", "c0", u32::MAX, 1).is_err(),
        "a ratio beyond the host's limit is refused, not attempted"
    );
    assert!(
        s.stretch("t0", "c0", 1, u32::MAX).is_err(),
        "and so is an absurd denominator"
    );

    let before = s.arrangement().expect("arrangement").tracks[0].clips[0].clone();
    s.stretch("t0", "c0", 3, 2).expect("stretch");
    let after = s.arrangement().expect("arrangement").tracks[0].clips[0].clone();
    assert_eq!(
        after.source, "s1.stretch.0_4800.3_2",
        "the clip points at the render"
    );
    assert_eq!(after.src_start, 0);
    assert_eq!(
        after.at_frame, before.at_frame,
        "its place in time is untouched"
    );
    assert!(
        after.src_len > before.src_len,
        "the material grew: {} → {}",
        before.src_len,
        after.src_len
    );

    // The material is in the pool, complete, with its peaks.
    let index = media::Pool::open(&pool)
        .expect("pool")
        .list()
        .expect("list");
    let rendered = index
        .sources
        .iter()
        .find(|source| source.id == "s1.stretch.0_4800.3_2")
        .expect("the render is a pool source");
    assert_eq!(
        rendered.frames, after.src_len,
        "the log's length is the file's"
    );
    assert!(!rendered.peaks_missing && rendered.finalized);
    assert_eq!(rendered.sample_rate, 48_000);

    // **Pitch is preserved**: the zero-crossing *rate* is the original's, while the
    // duration grew (a resample would have raised both together).
    let crossings = |path: &std::path::Path| -> (usize, u64) {
        let mut r = media::WavReader::open(path)
            .expect("wav")
            .with_channel(0)
            .expect("mono");
        let frames = r.total_frames();
        let mut buf = vec![0.0f32; frames as usize];
        let n = r.read_into(&mut buf);
        buf.truncate(n);
        let c = buf
            .windows(2)
            .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
            .count();
        (c, frames)
    };
    let (c0, f0) = crossings(&pool.join("s1.wav"));
    let (c1, f1) = crossings(&pool.join("s1.stretch.0_4800.3_2.wav"));
    let rate = |c: usize, f: u64| c as f64 / f as f64;
    assert!(
        (rate(c1, f1) - rate(c0, f0)).abs() < 0.05 * rate(c0, f0),
        "the pitch must be preserved: {} → {} crossings/frame",
        rate(c0, f0),
        rate(c1, f1)
    );
    // The *clip's region* is what was stretched (the fixture's clip reads 4 800
    // frames of the 48 000-frame tone), and the output is window-aligned.
    let expect = (before.src_len as f64 * 1.5) as u64;
    assert!(
        (f1 as f64 - expect as f64).abs() < 2.5 * 1024.0,
        "the length must follow the ratio: {} frames → {f1}, expected ~{expect}",
        before.src_len
    );
    let _ = f0;

    // One undo restores the old reference (the material is working material and
    // stays in the pool).
    s.execute(&HostCommand::Undo).expect("undo");
    let undone = s.arrangement().expect("arrangement").tracks[0].clips[0].clone();
    assert_eq!(undone.source, "s1");
    assert_eq!(undone.src_len, before.src_len);

    // The same material at the same ratio reuses the same source id (the id is
    // deterministic), so stretching, undoing and stretching again does not pile up
    // copies. (Stretching the *already stretched* clip renders new material — that
    // is a different input, and its own deterministic id says so.)
    s.stretch("t0", "c0", 3, 2).expect("stretch again");
    assert_eq!(
        s.arrangement().expect("arrangement").tracks[0].clips[0].source,
        "s1.stretch.0_4800.3_2"
    );
    let copies = media::Pool::open(&pool)
        .expect("pool")
        .list()
        .expect("list")
        .sources
        .iter()
        .filter(|source| source.id == "s1.stretch.0_4800.3_2")
        .count();
    assert_eq!(copies, 1, "one render, reused");

    // `source_tempo` is **state**: it survives a save/load like the rest of the log
    // (the saved script carries the line, and replaying it rebuilds the map).
    let dir = root.join("session");
    s.execute(&HostCommand::Save { dir: dir.clone() })
        .expect("save");
    let mut reloaded = HostSession::new();
    reloaded.execute(&HostCommand::Load { dir }).expect("load");
    assert_eq!(
        reloaded.source_tempos().get("s1"),
        Some(&90.0),
        "the recorded tempo is part of the document"
    );
    assert_eq!(
        reloaded.arrangement().expect("arrangement").tracks[0].clips[0].source,
        "s1.stretch.0_4800.3_2",
        "and the stretch replays as a reference, not a re-render"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A stretch id keys on the region, not just the source.** Two clips of one pool
/// source at different offsets are *different material*: an id of
/// `{source}.stretch.{num}_{den}` makes the second render silently overwrite the
/// first clip's file, and the first clip goes quiet (reproduced before the fix: the
/// master dropped from a 0.354 peak to silence). The fixture is half tone, half
/// silence so the two renders are distinguishable by content.
#[test]
fn a_stretch_id_keys_on_the_region_not_the_source() {
    let root = std::env::temp_dir().join(format!("host-stretch-id-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");

    let path = pool.join("s2.wav");
    let mut w = media::WavWriter::create(&path, 48_000, 1).expect("wav writer");
    let samples: Vec<f32> = (0..9_600)
        .map(|i| {
            if i < 4_800 {
                (i as f32 * 0.05).sin() * 0.5
            } else {
                0.0
            }
        })
        .collect();
    w.write(&samples).expect("write fixture");
    w.finalize().expect("finalize fixture");

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    for (id, at, src_start) in [("loud", 0u64, 0u64), ("quiet", 9_600, 4_800)] {
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddClip {
                track: "t0".into(),
                clip: media::Clip {
                    reversed: false,
                    id: id.into(),
                    name: None,
                    source: "s2".into(),
                    src_start,
                    src_len: 4_800,
                    at_frame: at,
                    fade_in: 0,
                    fade_out: 0,
                    gain: 1.0,
                    loop_len: None,
                },
            },
            at_frame: None,
        })
        .expect("clip");
    }

    s.stretch("t0", "loud", 3, 2)
        .expect("stretch the loud clip");
    s.stretch("t0", "quiet", 3, 2)
        .expect("stretch the quiet clip");

    let timeline = s.arrangement().expect("arrangement");
    let source_of = |id: &str| {
        timeline.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.source.clone())
            .expect("clip")
    };
    assert_eq!(source_of("loud"), "s2.stretch.0_4800.3_2");
    assert_eq!(source_of("quiet"), "s2.stretch.4800_4800.3_2");

    let peak = |path: &std::path::Path| -> f32 {
        let mut r = media::WavReader::open(path).expect("wav");
        let mut buf = vec![0.0f32; r.total_frames() as usize];
        let n = r.read_into(&mut buf);
        buf.truncate(n);
        buf.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    };
    assert!(
        peak(&pool.join("s2.stretch.0_4800.3_2.wav")) > 0.1,
        "the loud clip's render must still be its own material"
    );
    assert_eq!(
        peak(&pool.join("s2.stretch.4800_4800.3_2.wav")),
        0.0,
        "the quiet clip's render is silence — and, crucially, a *different* file"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **The mastering chain is a graph node on the bus.** Mounted after the mixer and
/// patched from it with a stereo cord, it claims the output, so the live pump and
/// the bounce both flow through it. A loud take proves the compressor pulls the
/// material down, the limiter holds the ceiling on a spike, the bounce is
/// **latency-aligned** (a spike at the clip's frame 0 is at frame 0 of the file, not
/// after the lookahead), and it is byte-identical across runs. Unmounting restores
/// the mixer as the bus owner.
#[test]
fn the_master_chain_sits_on_the_bus_and_bounces_aligned() {
    let root = std::env::temp_dir().join(format!("host-master-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    // A 0.6 sine with a **spike** at frame 5 000: the clip starts at 5 000, so the
    // spike lands on the clip's frame 0 — the alignment marker — and the steady sine
    // is what the compressor's gain reduction shows up in.
    write_spiked_tone(&pool, "s1", 48_000, 5_000);

    let build = |with_master: bool| -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        if with_master {
            s.execute(&HostCommand::Mount {
                plugin: "master",
                params: vec![],
                at_frame: Some(0),
            })
            .expect("master");
            s.execute(&HostCommand::Patch {
                from: ("mixer", "audio"),
                to: ("master", "audio"),
                at_frame: Some(0),
            })
            .expect("patch the bus");
            for (param, value) in [
                ("threshold", -24.0),
                ("ratio", 8.0),
                ("attack_ms", 1.0),
                ("ceiling", -6.0),
            ] {
                s.execute(&HostCommand::SetParam {
                    plugin: "master",
                    param,
                    value,
                    at_frame: Some(0),
                })
                .expect("master param");
            }
        }
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        let mut clip = match add_clip("c0", 0) {
            media::ArrangeOp::AddClip { clip, .. } => clip,
            other => panic!("add_clip built {other:?}"),
        };
        clip.src_start = 5_000;
        // Long enough that a bounce *after* the two 16 000-frame renders still has
        // material (the unmount check below renders from the advanced position).
        clip.src_len = 40_000;
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddClip {
                track: "t0".into(),
                clip,
            },
            at_frame: None,
        })
        .expect("clip");
        s
    };

    let mut dry_session = build(false);
    let dry = bounce(&mut dry_session, 16_000, &root.join("dry.wav"));

    let mut s = build(true);
    let path = root.join("mix.wav");
    // The mount and its params are *scheduled*: the first render applies them.
    let audio = bounce(&mut s, 16_000, &path);
    assert!(
        s.master_meters().is_some(),
        "the mastering stage publishes its meters while mounted"
    );

    // The brickwall: the spike is caught (the dry file carries it well above the
    // ceiling; the master file cannot).
    let ceiling = 10f32.powf(-6.0 / 20.0);
    let peak = audio.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let dry_peak = dry.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(
        dry_peak > ceiling * 1.5,
        "the fixture must exceed the ceiling dry: {dry_peak} vs {ceiling}"
    );
    assert!(
        peak <= ceiling + 1e-4,
        "the brickwall holds on the bounce: {peak} vs {ceiling}"
    );
    assert!(
        peak > ceiling * 0.9,
        "and the spike is not ducked to nothing: {peak}"
    );

    // **Aligned**: the spike is at frame 0 of the file, exactly where the dry
    // bounce has it — not 240+ frames late, and not preceded by the lookahead's
    // silence.
    assert!(
        audio[0].abs() > 0.3 && audio[1].abs() > 0.3,
        "the spike is at frame 0, both sides: {:?}",
        &audio[..4]
    );

    // The compressor works on the steady material: the second half of the master
    // bounce is clearly quieter than the dry one (the spike's first block aside).
    let rms = |v: &[f32], from: usize, to: usize| -> f32 {
        let s: f32 = v[from..to].iter().map(|x| x * x).sum();
        (s / (to - from) as f32).sqrt()
    };
    let (m, d) = (rms(&audio, 8_000, 15_000), rms(&dry, 8_000, 15_000));
    assert!(
        m < d * 0.6,
        "the compressor pulls the mix down: master rms {m} vs dry {d}"
    );

    assert!(
        (audio.len() as i64 - 16_000).abs() < 2_000,
        "the file is the piece (plus tails), not the piece plus latency: {} frames",
        audio.len()
    );
    assert_eq!(
        s.engine.graph.out_channels(),
        2,
        "the master owns the stereo bus"
    );

    // Deterministic: rewind and render the same window — byte-identical (the chain
    // is pure DSP, and the alignment is computed the same way twice).
    s.execute(&HostCommand::TransportSeek { frame: 0 })
        .expect("rewind");
    let again = bounce(&mut s, 16_000, &root.join("mix2.wav"));
    assert_eq!(audio, again, "the mastering chain is deterministic");

    // Unmounting restores the previous bus owner — dropping the mastering stage
    // must not leave the graph silent. (The unmount applies at the next render.)
    s.execute(&HostCommand::Unmount {
        plugin: "master",
        at_frame: None,
    })
    .expect("unmount master");
    let dry_again = bounce(&mut s, 4_800, &root.join("dry2.wav"));
    assert!(s.master_meters().is_none(), "the meters go with the plugin");
    assert_eq!(
        s.engine.graph.out_channels(),
        2,
        "the mixer still owns the bus after the master is unmounted"
    );
    assert!(
        dry_again.iter().any(|x| x.abs() > 0.01),
        "and the bus still carries audio (the mixer was restored as out)"
    );
    // The same material again, but through the mixer alone: the compressed level
    // was a fraction of the dry one, so a restored dry path proves the chain is out
    // of the way (not merely silent).
    let dry_again_peak = dry_again.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let compressed_peak = rms(&audio, 8_000, 15_000) * 4.0; // a generous multiple
    assert!(
        dry_again_peak > compressed_peak,
        "the dry path is back and uncompressed: {dry_again_peak} vs a compressed ~{compressed_peak}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **`export` is the deliverable sibling of `bounce`.** It renders the whole
/// arrangement from frame 0 (not from wherever the playhead is), measures the
/// length itself, writes f32 by default or s16 with fixed-seed TPDF dither, and
/// **refuses rather than writing a clipped file**. Verified: the length is the
/// arrangement's own, the file starts at the piece (aligned), two exports are
/// byte-identical (dither included), the report is the mix's real peak/RMS, and a
/// hot mix leaves no file behind.
#[test]
fn export_writes_the_whole_arrangement_and_refuses_to_clip() {
    let root = std::env::temp_dir().join(format!("host-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    // Two clips: the arrangement ends at 9 600, and the playhead is parked
    // somewhere else entirely (an export must not care).
    for (id, at) in [("c0", 0u64), ("c1", 4_800)] {
        s.execute(&HostCommand::Arrange {
            op: add_clip(id, at),
            at_frame: None,
        })
        .expect("clip");
    }
    s.execute(&HostCommand::TransportSeek { frame: 2_000 })
        .expect("playhead elsewhere");

    let f32_path = root.join("mix.wav");
    s.execute(&HostCommand::Export {
        path: f32_path.clone(),
        format: ExportFormat::F32,
    })
    .expect("export f32");
    let record = s.last_export().cloned().expect("a report");
    assert_eq!(record.frames, 9_600, "the arrangement's own length");
    assert_eq!(record.format, ExportFormat::F32.code());
    assert!(
        record.peak > 0.1 && record.peak <= 1.0,
        "peak {} is the mix's",
        record.peak
    );
    assert!(record.rms > 0.0 && record.rms < record.peak);

    // The file is the piece: the arrangement length (+ the drain's tail), stereo,
    // and f32 (the pool's float reader reads it back exactly).
    let mut r = media::WavReader::open(&f32_path).expect("wav");
    assert_eq!(r.channels(), 2, "the master bus is stereo");
    let frames = r.total_frames();
    assert!(
        (9_600..9_600 + 4_096).contains(&frames),
        "exported {frames} frames for a 9 600-frame arrangement"
    );
    // `read_into` returns **frames** and fills one channel (channel 0).
    let mut buf = vec![0.0f32; frames as usize];
    let n = r.read_into(&mut buf);
    buf.truncate(n);
    assert!(
        buf[..2_000].iter().any(|x| x.abs() > 0.01),
        "the file starts at the piece, not after the (absent) latency"
    );

    // Deterministic, twice over — and the second export is a *different* run of
    // the same session from a different playhead position.
    let again = root.join("mix2.wav");
    s.execute(&HostCommand::TransportSeek { frame: 7_000 })
        .expect("move the playhead again");
    s.execute(&HostCommand::Export {
        path: again.clone(),
        format: ExportFormat::F32,
    })
    .expect("export again");
    assert_eq!(
        std::fs::read(&f32_path).expect("bytes"),
        std::fs::read(&again).expect("bytes"),
        "an export depends on the session, not on the playhead"
    );

    // s16 with the fixed-seed dither: also byte-identical across runs, and a
    // 16-bit file (half the sample width).
    let s16_a = root.join("mix-s16-a.wav");
    let s16_b = root.join("mix-s16-b.wav");
    for path in [&s16_a, &s16_b] {
        s.execute(&HostCommand::Export {
            path: path.clone(),
            format: ExportFormat::S16,
        })
        .expect("export s16");
    }
    assert_eq!(
        std::fs::read(&s16_a).expect("bytes"),
        std::fs::read(&s16_b).expect("bytes"),
        "the dither is seeded, so an export is reproducible"
    );
    let mut r16 = media::WavReader::open(&s16_a).expect("wav");
    assert_eq!(r16.channels(), 2, "the s16 export is stereo too");
    assert_eq!(r16.total_frames(), frames, "same length as the f32 export");
    let mut buf16 = vec![0.0f32; frames as usize];
    let n16 = r16.read_into(&mut buf16);
    assert_eq!(n16, buf.len(), "and the same frames come back");
    // The dithered file tracks the float one: a diff of a few LSBs, not a
    // different performance.
    let worst = buf16
        .iter()
        .zip(&buf)
        .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(
        worst < 8.0 / 32767.0,
        "s16 is the same mix within a few LSBs: {worst}"
    );

    // **Never a clipped file**: make the mix exceed full scale (two tracks, each
    // clipped at +6 dB, summing into the same span) and check that the export
    // refuses *and* leaves nothing behind.
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::SetClipGain {
            track: "t0".into(),
            clip: "c0".into(),
            gain: 2.0,
        },
        at_frame: None,
    })
    .expect("gain");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t1".into() },
        at_frame: None,
    })
    .expect("second track");
    let mut hot_clip = match add_clip("c2", 0) {
        media::ArrangeOp::AddClip { clip, .. } => clip,
        other => panic!("add_clip built {other:?}"),
    };
    hot_clip.gain = 2.0;
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddClip {
            track: "t1".into(),
            clip: hot_clip,
        },
        at_frame: None,
    })
    .expect("hot clip");
    let hot = root.join("hot.wav");
    let err = s
        .execute(&HostCommand::Export {
            path: hot.clone(),
            format: ExportFormat::S16,
        })
        .expect_err("a clipping export is refused");
    assert!(
        err.contains("would clip") && err.contains("the file was not written"),
        "the refusal explains itself: {err}"
    );
    assert!(
        !hot.exists(),
        "and no clipped file was written ({})",
        hot.display()
    );

    // An empty arrangement has nothing to export (and says so).
    let empty = HostSession::new();
    let mut empty = empty;
    let e = empty
        .export(&root.join("empty.wav"), ExportFormat::F32)
        .expect_err("nothing to export");
    assert!(e.contains("no clips"), "{e}");

    let _ = std::fs::remove_dir_all(&root);
}

/// **An export is side-effect free and takes the session's current state.** The
/// gate measured two must-fixes here: the playhead jumping to the arrangement's end
/// (and a take in progress being finalized) because the export rendered *through the
/// live session*, and state stamped `@frame > 0` being absent from the export — so a
/// limiter mounted mid-session mastered the audible mix but not the deliverable.
/// Both are fixed by rendering a rebuilt clone that applies every state command at
/// the present instant.
#[test]
fn an_export_leaves_the_session_alone_and_includes_later_state() {
    let root = std::env::temp_dir().join(format!("host-export-state-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    // Two tracks, each clipped at +4 dB, summing over full scale: without a limiter
    // the mix cannot be exported, with one it can. (`add_clip`'s 0.5 tone × 1.6 × the
    // equal-power pan's 0.707 ≈ 0.57 per track ≈ 1.13 together.)
    let hot_tracks = |s: &mut HostSession| {
        for (track, clip) in [("t0", "c0"), ("t1", "c1")] {
            s.execute(&HostCommand::Arrange {
                op: media::ArrangeOp::AddTrack {
                    track: track.into(),
                },
                at_frame: None,
            })
            .expect("track");
            let mut c = match add_clip(clip, 0) {
                media::ArrangeOp::AddClip { clip, .. } => clip,
                other => panic!("add_clip built {other:?}"),
            };
            c.gain = 1.6;
            s.execute(&HostCommand::Arrange {
                op: media::ArrangeOp::AddClip {
                    track: track.into(),
                    clip: c,
                },
                at_frame: None,
            })
            .expect("clip");
        }
    };

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    hot_tracks(&mut s);

    // A limiter mounted at frame 5 000 — *after* the export's start.
    for (cmd, what) in [
        (
            HostCommand::Mount {
                plugin: "master",
                params: vec![],
                at_frame: Some(5_000),
            },
            "mid-session master",
        ),
        (
            HostCommand::Patch {
                from: ("mixer", "audio"),
                to: ("master", "audio"),
                at_frame: Some(5_000),
            },
            "mid-session patch",
        ),
        (
            HostCommand::SetParam {
                plugin: "master",
                param: "ceiling",
                value: -6.0,
                at_frame: Some(5_000),
            },
            "mid-session ceiling",
        ),
    ] {
        s.execute(&cmd).expect(what);
    }

    // Park the playhead somewhere specific and export: it must not move.
    s.execute(&HostCommand::TransportSeek { frame: 7_000 })
        .expect("seek");
    let before = s.position().frame;
    let path = root.join("mix.wav");
    s.execute(&HostCommand::Export {
        path: path.clone(),
        format: ExportFormat::F32,
    })
    .expect("export");
    assert_eq!(
        s.position().frame,
        before,
        "an export is a file write, not a seek"
    );
    assert_eq!(before, 7_000);

    // The mid-session limiter is in the export: the whole file respects the ceiling.
    let ceiling = 10f32.powf(-6.0 / 20.0);
    let mut r = media::WavReader::open(&path).expect("wav");
    let frames = r.total_frames();
    let mut buf = vec![0.0f32; frames as usize];
    let n = r.read_into(&mut buf);
    buf.truncate(n);
    let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(
        peak > 0.3,
        "the fixture is over full scale dry, so the limiter had work to do: {peak}"
    );
    assert!(
        peak <= ceiling + 1e-4,
        "the limiter mounted at 5 000 masters the whole export: {peak} vs {ceiling}"
    );

    // **A refusal leaves an existing file alone.** The same mix without a limiter
    // must refuse, and the earlier export's bytes stay exactly as they were.
    let bytes_before = std::fs::read(&path).expect("bytes");
    let mut plain = HostSession::new_at(48_000);
    plain
        .execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
    plain
        .execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    hot_tracks(&mut plain);
    let err = plain
        .export(&path, ExportFormat::F32)
        .expect_err("a limiterless +4 dB pair exceeds full scale");
    assert!(err.contains("would clip"), "{err}");
    assert_eq!(
        std::fs::read(&path).expect("bytes"),
        bytes_before,
        "a refused export never touches an existing file"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A session that unmounts and re-mounts a plugin must still rebuild.**
///
/// The engine releases a plugin's name when its unmount *applies*, so
/// `mount mixer` / `unmount mixer @96 000` / `mount mixer @192 000` is a session
/// the live path accepts and records. Every re-apply path — `export` (which
/// re-issues the history with its placements stripped) and a seek (the warm
/// prefix and the full replay, which `undo`/`redo` share) — used to walk that
/// history without rendering between the commands, so the apply queue never
/// drained and the second `mount` was refused as a second instance:
/// `Err("plugin 'mixer' is already mounted")` out of `export` and out of every
/// seek, on a session the platform itself had recorded. The rebuild now answers
/// the one-instance rule from the **history's** lifecycle, the way
/// `Engine::replay_from` answers it from a log's.
#[test]
fn a_session_that_re_mounts_a_plugin_still_exports_and_seeks() {
    let root = std::env::temp_dir().join(format!("host-remount-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 240_000, 48_000);

    // Four clips, one every two seconds: the plugin goes away at 2 s and comes
    // back at 4 s, so the re-mount is a real lifecycle rather than a no-op pair.
    let arrangement = |s: &mut HostSession| {
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        for (id, at) in [
            ("c0", 0u64),
            ("c1", 96_000),
            ("c2", 144_000),
            ("c3", 192_000),
        ] {
            let mut op = add_clip(id, at);
            if let media::ArrangeOp::AddClip { clip, .. } = &mut op {
                clip.src_len = 48_000;
            }
            s.execute(&HostCommand::Arrange { op, at_frame: None })
                .expect("clip");
        }
    };
    // The **bus**, re-mounted: the shape a `unmount mixer` + `mount mixer`
    // gesture writes, and the one all three rebuilds refused.
    let build_mixer = || -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        arrangement(&mut s);
        s.execute(&HostCommand::Unmount {
            plugin: "mixer",
            at_frame: Some(96_000),
        })
        .expect("unmount the mixer mid-session");
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(192_000),
        })
        .expect("mount it again — the live path released the name");
        s
    };
    // A plugin that is **not** the bus, so the clips keep playing across its
    // re-mount: what the rebuilt session renders is then an observation of the
    // rebuilt graph rather than of the bus being taken down.
    let build_clock_out = || -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        arrangement(&mut s);
        s.execute(&HostCommand::Mount {
            plugin: "clock_out",
            params: vec![],
            at_frame: Some(0),
        })
        .expect("first mount");
        s.execute(&HostCommand::Unmount {
            plugin: "clock_out",
            at_frame: Some(96_000),
        })
        .expect("unmount it mid-session");
        s.execute(&HostCommand::Mount {
            plugin: "clock_out",
            params: vec![],
            at_frame: Some(192_000),
        })
        .expect("mount it again — the live path released the name");
        s
    };

    // The history is what a rebuild re-issues: the triple is really there.
    let text = build_mixer().script_text(&root).expect("the session text");
    let mounts = text
        .lines()
        .filter(|l| l.starts_with("mount mixer"))
        .count();
    assert_eq!(
        mounts, 2,
        "both mounts are in the session's own text form:\n{text}"
    );
    assert!(
        text.contains("unmount mixer @96000"),
        "and the unmount keeps its placement:\n{text}"
    );

    // **`export`**: the rebuild strips every placement, so nothing renders
    // between the commands — the exact shape that used to be refused.
    let mut s = build_mixer();
    s.export(&root.join("mix.wav"), ExportFormat::F32)
        .expect("a re-mounted mixer still exports");
    let record = s.last_export().cloned().expect("a report");
    assert!(
        record.peak > 0.01,
        "the re-mounted bus carries the audio: {record:?}"
    );

    // **The warm-up seek** (`rebuild_prefix`): the run-in is not what releases
    // the name either — the state walk has to.
    let mut warm = build_mixer();
    warm.execute(&HostCommand::TransportSeek { frame: 240_000 })
        .expect("a warm seek over a re-mounted mixer");
    let (frame, warmed) = warm.last_seek().expect("a seek ran");
    assert_eq!(frame, 240_000);
    assert!(warmed, "this is the prefix path this test is for");

    // **The full replay** seek (the `upto = Some(frame)` rebuild, and the path
    // `undo`/`redo` take), into the re-mounted era.
    let mut full = build_clock_out();
    full.replay_full(200_000)
        .expect("a full-replay seek over a re-mounted plugin");
    assert_eq!(full.position().frame, 200_000);
    assert_eq!(
        full.arrangement().expect("arrangement"),
        warm.arrangement().expect("arrangement"),
        "both rebuilds carry the arrangement across"
    );
    let audio = bounce(&mut full, 8_000, &root.join("after.wav"));
    assert!(
        audio.iter().any(|x| x.abs() > 0.01),
        "the rebuilt session renders audio, so the re-mount did not break the graph"
    );

    // **An unplaced triple** — the shape an interactive `execute` writes, since a
    // command with no placement renders nothing, so the queue never drains
    // between them — takes the same three paths down if the rebuild only fixed
    // the placed shape. Written out command by command, the way a UI sends them.
    let mut plain = HostSession::new();
    plain
        .execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
    plain
        .execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    plain
        .execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
    plain
        .execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
    plain
        .execute(&HostCommand::Mount {
            plugin: "clock_out",
            params: vec![],
            at_frame: None,
        })
        .expect("mount");
    plain
        .execute(&HostCommand::Unmount {
            plugin: "clock_out",
            at_frame: None,
        })
        .expect("unmount");
    plain
        .execute(&HostCommand::Bounce {
            frames: 4_800,
            path: root.join("unplaced-before.wav"),
        })
        .expect("a render applies the unplaced unmount");
    plain
        .execute(&HostCommand::Mount {
            plugin: "clock_out",
            params: vec![],
            at_frame: None,
        })
        .expect("re-mount after a render — the name was released");
    plain
        .export(&root.join("unplaced.wav"), ExportFormat::F32)
        .expect("an unplaced remount still exports");
    plain
        .execute(&HostCommand::TransportSeek { frame: 240_000 })
        .expect("and still seeks");

    let _ = std::fs::remove_dir_all(&root);
}

/// **The loader is a document walk too, or a saved re-mount cannot be reopened.**
///
/// The commit above fixed every *re-apply* path — `rebuild`, `rebuild_prefix` — by
/// entering a document walk, so the one-instance rule is the history's question
/// while the state is re-issued. The **load** path was left out: `from_script`
/// issues each command with `execute`, unwalked. The saved text of the unplaced
/// triple is `mount clock_out` / `unmount clock_out` / `mount clock_out` with no
/// placement on any of them, and the bounce that released the name between the
/// first two is an *action*, so it is not in the text — a load re-issues three
/// unplaced commands with nothing rendering between them, and the second mount was
/// refused: `Err("plugin 'clock_out' is already mounted")`. The session could be
/// written but never opened, which is the same user-facing defect the walk exists
/// to kill, still reachable through `load_session`.
///
/// The round-trip check inside `save` (the parsed text must equal the history) does
/// not catch it: the text is faithful, it is the *application* of the text that
/// failed. So this test goes all the way round — build, save, reopen — rather than
/// comparing commands.
#[test]
fn a_saved_session_that_re_mounts_a_plugin_reopens() {
    let root = std::env::temp_dir().join(format!("host-remount-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 240_000, 48_000);

    // The unplaced triple, built the way a UI sends it: a command with no
    // placement renders nothing, so the name is released only by the bounce
    // between the unmount and the re-mount. The live path accepts it, which is
    // what puts it in the history a save writes.
    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("clip");
    s.execute(&HostCommand::Mount {
        plugin: "clock_out",
        params: vec![],
        at_frame: None,
    })
    .expect("mount");
    s.execute(&HostCommand::Unmount {
        plugin: "clock_out",
        at_frame: None,
    })
    .expect("unmount");
    bounce(&mut s, 4_800, &root.join("mid.wav")); // a render applies the unmount
    s.execute(&HostCommand::Mount {
        plugin: "clock_out",
        params: vec![],
        at_frame: None,
    })
    .expect("re-mount — the name was released");

    let dir = root.join("re-mounted.d");
    s.save(&dir).expect("a session that re-mounts is savable");
    let text = std::fs::read_to_string(dir.join("session.txt")).expect("the script");
    let mounts = text
        .lines()
        .filter(|l| l.starts_with("mount clock_out"))
        .count();
    assert_eq!(
        mounts, 2,
        "both mounts are in the session's own text form:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("unmount clock_out")),
        "…with the unmount between them:\n{text}"
    );

    // **The load.** This is the round trip the commit's own test did not make: the
    // session is rebuilt from the *file*, not compared as commands.
    let mut loaded = HostSession::new();
    loaded
        .load_session(&dir)
        .expect("a saved session that re-mounts a plugin must reopen");
    assert_eq!(
        loaded.arrangement().expect("tl"),
        s.arrangement().expect("tl"),
        "the arrangement round-trips"
    );

    // And the reopened session is a session, not a value: it exports, renders,
    // seeks, and saves again (so a second load has to work as well).
    loaded
        .export(&root.join("reopened.wav"), ExportFormat::F32)
        .expect("the reopened session exports");
    let audio = bounce(&mut loaded, 4_800, &root.join("reopened-clip.wav"));
    assert!(
        audio.iter().any(|x| x.abs() > 0.01),
        "the reopened session renders its clip"
    );
    loaded
        .execute(&HostCommand::TransportSeek { frame: 240_000 })
        .expect("and still seeks");
    let again = root.join("re-mounted-again.d");
    loaded.save(&again).expect("save again");
    let mut twice = HostSession::new();
    twice
        .load_session(&again)
        .expect("a re-saved re-mounting session reopens too");

    let _ = std::fs::remove_dir_all(&root);
}

/// **The journal is a document walk too, or a re-mounting edit is dropped on load.**
///
/// The journal is appended by the *live* path, where `mount p` / `unmount p` /
/// `mount p` is accepted because the name was released when the unmount applied.
/// `apply_journal` replays those lines back to back on a session that has rendered
/// nothing, so unwalked the re-mount was refused — and a refused journal entry is
/// **dropped and reported**, which means the load succeeded while silently losing
/// the user's last edit. The walk makes the journal's own lifecycle the answer,
/// exactly as it does for the script.
///
/// The test drives the real thing: a session directory whose `journal.txt` holds an
/// unplaced re-mount triple, torn the way a crash mid-gesture tears one.
#[test]
fn a_journal_that_re_mounts_a_plugin_is_applied_not_dropped() {
    let root = std::env::temp_dir().join(format!("host-journal-remount-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 240_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("clip");
    let dir = root.join("journalled.d");
    s.save(&dir).expect("save the baseline");

    // The autosave since that save: a re-mount of `clock_out`, written the way the
    // live path writes it (unplaced, with a render between the unmount and the
    // re-mount — the render is an action, so it is not in the journal). The journal
    // holds entries only; `apply_journal` supplies the `host v1` header.
    std::fs::write(
        dir.join("journal.txt"),
        b"mount clock_out\nunmount clock_out\nmount clock_out\n",
    )
    .expect("write the journal");

    let mut loaded = HostSession::new();
    loaded.load_session(&dir).expect("load");
    let recovery = loaded.last_recovery().expect("a recovery report");
    assert_eq!(
        recovery.applied, 3,
        "every journal command applied: {recovery:?}"
    );
    assert_eq!(
        recovery.refused, 0,
        "the re-mount is not a refusal any more: {recovery:?}"
    );
    assert!(
        recovery.refused_reason.is_none(),
        "…with no reason to report: {recovery:?}"
    );

    // The applied triple really is in the loaded session's history, so a further
    // save carries it (this is what was being lost).
    let again = root.join("journalled-again.d");
    loaded.save(&again).expect("save the recovered session");
    let text = std::fs::read_to_string(again.join("session.txt")).expect("the script");
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("mount clock_out"))
            .count(),
        2,
        "both mounts survived the load:\n{text}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A warm seek over a tempo placed in the past does not walk the clock back.**
///
/// `rebuild_prefix` places the clock at each `SetTempo`'s *stated* frame, because a
/// tempo segment has to sit at its own frame in the map even when the run-in starts
/// later. It did that unconditionally, so a tempo command whose placement is already
/// behind the clock moved the clock **backwards** — and every `at_now` command after
/// it was then stamped *before* the commands before it. On a history that also
/// re-mounts a plugin the result is `Mount@0 … Unmount@120000 … Mount@48000`: a
/// frame-inverted document, which the walk refuses (`plugin 'clock_out' goes back to
/// frame 48000 after frame 120000`) on a session the live path wrote and accepted.
///
/// The rule is the live path's: `process` renders up to a placement only when it is
/// in the future, and `set_tempo` stamps at the current frame otherwise — so the
/// clock moves forward and only forward. `max` says that in one expression, and this
/// is the test that keeps it said.
#[test]
fn a_warm_seek_over_a_past_tempo_placement_still_works() {
    let root = std::env::temp_dir().join(format!("host-past-tempo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 480_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    // A clip long enough to still be playing at the seek target, so "the rebuilt
    // session renders" is an observation about the rebuilt graph.
    let mut clip = add_clip("c0", 0);
    if let media::ArrangeOp::AddClip { clip, .. } = &mut clip {
        clip.src_len = 480_000;
    }
    s.execute(&HostCommand::Arrange {
        op: clip,
        at_frame: None,
    })
    .expect("clip");
    s.execute(&HostCommand::Mount {
        plugin: "clock_out",
        params: vec![],
        at_frame: Some(0),
    })
    .expect("mount");
    s.execute(&HostCommand::SetTempo {
        bpm: 90.0,
        beats_per_bar: 4,
        at_frame: Some(120_000),
    })
    .expect("tempo at 2.5 s");
    s.execute(&HostCommand::Unmount {
        plugin: "clock_out",
        at_frame: Some(120_000),
    })
    .expect("unmount");
    // **The past placement.** Issued while the clock is already past 48 000, so the
    // live path stamps it at the current frame — and the history carries the stale
    // one, which is what `rebuild_prefix` used to seek to.
    s.execute(&HostCommand::SetTempo {
        bpm: 140.0,
        beats_per_bar: 4,
        at_frame: Some(48_000),
    })
    .expect("a tempo whose placement is already behind the clock");
    s.execute(&HostCommand::Mount {
        plugin: "clock_out",
        params: vec![],
        at_frame: Some(300_000),
    })
    .expect("re-mount");
    let live = s.engine.log.clone();
    let live_bpm = s.position().bpm;

    // A warm seek (the `rebuild_prefix` path) over that history.
    s.execute(&HostCommand::TransportSeek { frame: 360_000 })
        .expect("a warm seek over a past tempo placement");
    let (frame, warmed) = s.last_seek().expect("a seek ran");
    assert_eq!(frame, 360_000);
    assert!(warmed, "this is the prefix path this test is for");
    assert_eq!(
        s.position().bpm,
        live_bpm,
        "the tempo map is the session's, not the stale placement's"
    );
    let audio = bounce(&mut s, 8_000, &root.join("after.wav"));
    assert!(
        audio.iter().any(|x| x.abs() > 0.01),
        "the rebuilt session still renders"
    );

    // The invariant the walk now enforces, checked against the log the live path
    // actually wrote: the lifecycle frames for one name are non-decreasing. The
    // seek above succeeded precisely because `rebuild_prefix` preserved it — the
    // stale 48 000 placement is not in the *log* (the live path stamped it at the
    // current frame), it was only in the command the rebuild read.
    let frames: Vec<u64> = live
        .events()
        .iter()
        .filter_map(|e| match e {
            engine::Event::Mount {
                plugin: "clock_out",
                at_frame,
                ..
            } => Some(*at_frame),
            engine::Event::ScheduleUnmount {
                plugin: "clock_out",
                at_frame,
            } => Some(*at_frame),
            _ => None,
        })
        .collect();
    assert_eq!(
        frames,
        vec![0, 120_000, 300_000],
        "the live path's lifecycle frames for clock_out are in order"
    );
    assert!(
        frames.windows(2).all(|w| w[0] <= w[1]),
        "…which is what the walk's frame rule requires: {frames:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A warm-up seek is a seek: provably equal to a replay.** The plan's item 15 asks
/// for a jump into a long piece to be interactive *and* "provably equal to a replay".
/// A jump past [`SEEK_WARMUP_FRAMES`] places the clock one second before the target
/// and renders only that run-in (readers need nothing — a clip's read is a pure
/// function of the block frame — and the bus effects settle inside a second). This
/// test renders the same window from a warmed seek and from a full replay and compares
/// the **bytes**, for a session whose stateful bus chain is live across the seek.
#[test]
fn a_warm_seek_equals_a_replay() {
    let root = std::env::temp_dir().join(format!("host-warmseek-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 480_000, 48_000);

    // A piece far longer than the run-in, with a master chain and clips spread across
    // it, so the limiter's state at the target is real work (not silence).
    let build = || -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Mount {
            plugin: "master",
            params: vec![],
            at_frame: Some(0),
        })
        .expect("master");
        s.execute(&HostCommand::Patch {
            from: ("mixer", "audio"),
            to: ("master", "audio"),
            at_frame: Some(0),
        })
        .expect("patch");
        s.execute(&HostCommand::SetParam {
            plugin: "master",
            param: "threshold",
            value: -18.0,
            at_frame: Some(0),
        })
        .expect("threshold");
        s.execute(&HostCommand::SetParam {
            plugin: "master",
            param: "ceiling",
            value: -6.0,
            at_frame: Some(0),
        })
        .expect("ceiling");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        // Clips every second for two minutes, so the run-in ends inside material.
        for k in 0..120u64 {
            let mut op = add_clip(&format!("c{k}"), k * 48_000);
            if let media::ArrangeOp::AddClip { clip, .. } = &mut op {
                clip.src_start = 0;
                clip.src_len = 48_000;
            }
            s.execute(&HostCommand::Arrange { op, at_frame: None })
                .expect("clip");
        }
        s
    };

    let target = 3_600_000u64; // 75 s in — past the run-in, inside the piece
    let mut plain = build();
    // The **reference**: the same session rendered from 0 by the full replay path
    // (the warm-up path is what `transport seek` takes when it can prove soundness).
    plain.replay_full(target).expect("full-replay seek");
    let (_, warmed) = plain.last_seek().expect("a seek ran");
    assert!(!warmed, "the reference is the un-warmed path");

    let mut warm = build();
    warm.execute(&HostCommand::TransportSeek { frame: target })
        .expect("warm seek");
    let (frame, warmed) = warm.last_seek().expect("a seek ran");
    assert_eq!(frame, target);
    assert!(warmed, "the warm-up path is what this test is for");
    assert_eq!(
        warm.arrangement().expect("arrangement"),
        plain.arrangement().expect("arrangement"),
        "the value agrees"
    );
    assert_eq!(
        warm.position().frame,
        plain.position().frame,
        "and the clock lands in the same place"
    );

    // Byte-identical audio from the target: the limiter's state came out of the run-in
    // exactly as a replay would have produced it.
    let plain_audio = bounce(&mut plain, 24_000, &root.join("plain.wav"));
    let warm_audio = bounce(&mut warm, 24_000, &root.join("warm.wav"));
    assert_eq!(
        plain_audio, warm_audio,
        "a warm-up seek renders the same bytes as a replay"
    );

    // State placed **inside the run-in** cannot be warmed: a ceiling change at
    // 3 576 000 (the run-in starts at target - 48 000 = 3 552 000) has to be applied at
    // its own frame, which only a full replay can place, so the seek falls back.
    let mut late = build();
    late.execute(&HostCommand::SetParam {
        plugin: "master",
        param: "ceiling",
        value: -12.0,
        at_frame: Some(3_576_000),
    })
    .expect("late state");
    late.execute(&HostCommand::TransportSeek { frame: target })
        .expect("seek with late state");
    let (_, warmed) = late.last_seek().expect("a seek ran");
    assert!(
        !warmed,
        "state stamped after the run-in's start forces a full replay"
    );
    assert_eq!(
        late.arrangement().expect("arrangement"),
        plain.arrangement().expect("arrangement")
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **Tempo is frame-placed value state, and the run-in keeps it placed.** A tempo
/// change at 30 s must sit at 30 s even when the run-in starts at 74 s: the audio is
/// frame-based either way, but every beat reading (the ruler, the position readout,
/// `source_tempo` matching) would disagree with a full replay if the warm prefix
/// simply moved it to 0.
#[test]
fn a_warm_seek_keeps_mid_piece_tempo_placement() {
    let root = std::env::temp_dir().join(format!("host-warm-tempo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 480_000, 48_000);

    let build = || -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::SetTempo {
            bpm: 120.0,
            beats_per_bar: 4,
            at_frame: Some(0),
        })
        .expect("tempo");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        // A tempo change at 30 s, well before the run-in of a seek to 75 s.
        s.execute(&HostCommand::SetTempo {
            bpm: 90.0,
            beats_per_bar: 4,
            at_frame: Some(1_440_000),
        })
        .expect("mid-piece tempo");
        for k in 0..120u64 {
            let mut op = add_clip(&format!("c{k}"), k * 48_000);
            if let media::ArrangeOp::AddClip { clip, .. } = &mut op {
                clip.src_len = 48_000;
            }
            s.execute(&HostCommand::Arrange { op, at_frame: None })
                .expect("clip");
        }
        s
    };

    let target = 3_600_000u64;
    let mut plain = build();
    plain.replay_full(target).expect("full replay");
    let mut warm = build();
    warm.execute(&HostCommand::TransportSeek { frame: target })
        .expect("warm seek");
    assert!(warm.last_seek().expect("a seek ran").1, "warmed");

    // The tempo *map* agrees at the target: the same bpm and the same beat count.
    assert_eq!(
        warm.position().bpm,
        plain.position().bpm,
        "the tempo in force at the target is the mid-piece one, both ways"
    );
    assert_eq!(
        warm.engine.clock.tempo_map.segments().len(),
        plain.engine.clock.tempo_map.segments().len(),
        "and the map has the same segments"
    );
    assert_eq!(
        warm.engine.clock.tempo_map.tempo_at(target),
        plain.engine.clock.tempo_map.tempo_at(target),
        "with the same bpm in force at the target"
    );
    assert_eq!(
        warm.position().beat,
        plain.position().beat,
        "so the beat position agrees to the bit"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **`last_seek` reports seeks, not edits.** Undo and redo rebuild the session through
/// the same path a seek does, so without the flag a shell would announce "seek to
/// frame X — warmed" after an undo (the gate's should-fix). The report keeps naming the
/// last real jump, and an edit never overwrites it.
#[test]
fn last_seek_ignores_undo_and_redo() {
    let (mut s, pool) = session_with_clip("last-seek");
    s.execute(&HostCommand::TransportSeek { frame: 4_800 })
        .expect("seek");
    assert_eq!(s.last_seek().map(|(f, _)| f), Some(4_800));

    // An edit, then an undo and a redo: none of them is a seek.
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::SetClipGain {
            track: "t0".into(),
            clip: "c0".into(),
            gain: 0.5,
        },
        at_frame: None,
    })
    .expect("edit");
    assert_eq!(
        s.last_seek().map(|(f, _)| f),
        Some(4_800),
        "an edit does not overwrite the last seek's report"
    );
    s.execute(&HostCommand::Undo).expect("undo");
    assert_eq!(s.last_seek().map(|(f, _)| f), Some(4_800), "nor an undo");
    s.execute(&HostCommand::Redo).expect("redo");
    assert_eq!(s.last_seek().map(|(f, _)| f), Some(4_800), "nor a redo");

    // …and a real seek still reports itself.
    s.execute(&HostCommand::TransportSeek { frame: 1_200 })
        .expect("seek again");
    assert_eq!(s.last_seek().map(|(f, _)| f), Some(1_200));

    let _ = std::fs::remove_dir_all(&pool);
}

/// **The warm-up path is equal even when a clip runs past its source.** A session
/// whose clip declares more material than the take holds is broken, but the sequential
/// path has always played it (silence after EOF); the offset path must not refuse it,
/// or an optimisation would turn a working seek into an error. (It did, before
/// `ArrangerNode::new` clamped its mount offset to the source's length — found while
/// measuring this slice.)
#[test]
fn a_warm_seek_equals_a_replay_past_a_clips_source_end() {
    let root = std::env::temp_dir().join(format!("host-warm-eof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    // A 1-second take, declared as a 10-second clip.
    write_tone(&pool, "s1", 48_000, 48_000);

    let build = || -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        let mut op = add_clip("c0", 0);
        if let media::ArrangeOp::AddClip { clip, .. } = &mut op {
            clip.src_len = 480_000; // ten times the source
        }
        s.execute(&HostCommand::Arrange { op, at_frame: None })
            .expect("clip");
        s
    };

    // A target well past the source's end (and past the run-in).
    let target = 240_000u64;
    let mut plain = build();
    plain.replay_full(target).expect("full replay past EOF");
    let plain_audio = bounce(&mut plain, 4_800, &root.join("plain.wav"));

    let mut warm = build();
    warm.execute(&HostCommand::TransportSeek { frame: target })
        .expect("warm seek past EOF");
    let (_, warmed) = warm.last_seek().expect("a seek ran");
    assert!(warmed, "the warmed path is what is under test");
    let warm_audio = bounce(&mut warm, 4_800, &root.join("warm.wav"));
    assert_eq!(
        plain_audio, warm_audio,
        "both paths play the same silence after the source's end"
    );
    assert!(
        plain_audio.iter().all(|x| *x == 0.0),
        "and it is silence (the take is long over)"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **Markers do not touch the audio.** The acceptance checklist tells a second person
/// to export before and after adding markers and compare the files; this is that
/// claim as a test — the render path does not read markers, and `end_frame` is
/// clip-based, so the bytes are identical.
#[test]
fn markers_do_not_change_the_exported_bytes() {
    let root = std::env::temp_dir().join(format!("host-marker-bytes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let build = || -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
        s
    };

    let plain = root.join("plain.wav");
    let marked = root.join("marked.wav");
    let mut s = build();
    s.export(&plain, ExportFormat::F32).expect("export");
    // Markers, including one *past* the last clip (which must not extend the render).
    for (frame, name) in [(0u64, "intro"), (4_800, "verse"), (480_000, "outro")] {
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::SetMarker {
                at_frame: frame,
                name: name.into(),
            },
            at_frame: None,
        })
        .expect("marker");
    }
    s.export(&marked, ExportFormat::F32).expect("export again");
    assert_eq!(
        std::fs::read(&plain).expect("bytes"),
        std::fs::read(&marked).expect("bytes"),
        "markers are navigation: the mix is byte-identical"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **Save is idempotent.** `session_rate` is a header line *and* a logged command
/// (loading a session records it), so writing both made every open-and-save cycle
/// grow the script by a line — two, three, four… (found while writing the onboarding
/// docs, which tell a reader to reopen a saved session). One round trip is now a fixed
/// point.
#[test]
fn saving_a_loaded_session_is_idempotent() {
    let root = std::env::temp_dir().join(format!("host-save-idem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("clip");

    let one = root.join("one");
    s.save(&one).expect("first save");
    let first = std::fs::read_to_string(one.join("session.txt")).expect("script");
    assert_eq!(
        first.matches("session_rate").count(),
        1,
        "the header carries the rate once:\n{first}"
    );

    // Open it twice more, saving each time: the script must not grow.
    for (from, to) in [("one", "two"), ("two", "three")] {
        let mut loaded = HostSession::new();
        loaded
            .execute(&HostCommand::Load {
                dir: root.join(from),
            })
            .expect("load");
        loaded.save(&root.join(to)).expect("save");
    }
    let third = std::fs::read_to_string(root.join("three/session.txt")).expect("script");
    assert_eq!(first, third, "two more round trips changed nothing");

    let _ = std::fs::remove_dir_all(&root);
}

/// **A region the clip only *declares* is not allocated.** `src_len` is the clip's
/// window, not a fact about the file (`validate_clip` bounds it at `i64::MAX`, not at
/// the source's length), so a clip claiming ten billion frames must read what the
/// source has and stretch *that* — the id keys on the material actually rendered, so
/// two clips whose declared windows both run off the end of the same file share one
/// render. (The gate found the neighbouring hazard: an unbounded *ratio* reached the
/// allocator and killed the process; this is the same class of bug on the input side.)
#[test]
fn a_declared_region_longer_than_the_source_is_read_to_the_end() {
    let (mut s, pool) = session_with_clip("stretch-region");
    let mut long = match add_clip("c9", 9_600) {
        media::ArrangeOp::AddClip { clip, .. } => clip,
        other => panic!("add_clip built {other:?}"),
    };
    long.src_len = 10_000_000_000;
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddClip {
            track: "t0".into(),
            clip: long,
        },
        at_frame: None,
    })
    .expect("a clip may declare more than the file holds");

    s.stretch("t0", "c9", 3, 2)
        .expect("read to the end, then stretch");
    let c9 = s.arrangement().expect("arrangement").tracks[0]
        .clips
        .iter()
        .find(|c| c.id == "c9")
        .cloned()
        .expect("c9");
    assert_eq!(
        c9.source, "s1.stretch.0_48000.3_2",
        "the id is the material that was rendered (48 000 frames), not the declared length"
    );
    assert!(
        media::Pool::open(&pool)
            .expect("pool")
            .path_for(&c9.source)
            .is_some(),
        "and the render is really in the pool"
    );

    let _ = std::fs::remove_dir_all(&pool);
}

/// **A rename and a reorder survive the round trip.** Found by the slice's gate:
/// the note claimed this end-to-end but only the format/parse round-trip was
/// committed — which never *applies* the ops. This saves a session with
/// `rename_track` + `move_track`, loads it back, and checks the order (the mixer
/// channel each track feeds is its index, so the order is the thing that matters).
#[test]
fn a_renamed_and_reordered_session_reloads_in_order() {
    let root = std::env::temp_dir().join(format!("host-tracks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 4.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    for track in ["t0", "t1"] {
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack {
                track: track.into(),
            },
            at_frame: None,
        })
        .expect("track");
    }
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("clip");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::RenameTrack {
            track: "t0".into(),
            to: "lead".into(),
        },
        at_frame: None,
    })
    .expect("rename");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::MoveTrack {
            track: "lead".into(),
            index: 1,
        },
        at_frame: None,
    })
    .expect("reorder");

    let order = |s: &HostSession| -> Vec<String> {
        s.arrangement()
            .expect("arrangement")
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect()
    };
    assert_eq!(order(&s), vec!["t1", "lead"]);

    let dir = root.join("tracks.d");
    s.save(&dir).expect("save");

    // The saved text names both ops (they are state, so they are in the log)…
    let script = std::fs::read_to_string(dir.join("session.txt")).expect("script");
    assert!(
        script.contains("rename_track t0 lead"),
        "the rename is in the session:\n{script}"
    );
    assert!(
        script.contains("move_track lead 1"),
        "the reorder is in the session:\n{script}"
    );

    // …and loading it back reproduces the same order and the same clip.
    let mut reloaded = HostSession::new();
    reloaded.load_session(&dir).expect("load");
    assert_eq!(
        order(&reloaded),
        vec!["t1", "lead"],
        "the order round-trips"
    );
    let arrangement = reloaded.arrangement().expect("arrangement");
    assert_eq!(
        arrangement.tracks[1].clips.len(),
        1,
        "the clip is in `lead`"
    );
    assert_eq!(arrangement.tracks[1].clips[0].id, "c0");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_saved_session_can_be_moved() {
    let root = std::env::temp_dir().join(format!("host-move-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("elsewhere");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let (mut s, _) = {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
        (s, ())
    };

    let dir = root.join("take1.d");
    s.save(&dir).expect("save");
    let script = std::fs::read_to_string(dir.join("session.txt")).expect("script");
    assert!(
        script.contains("pool pool"),
        "the pool path is relative to the session:\n{script}"
    );
    let reference = bounce(&mut s, 4_800, &root.join("ref.wav"));

    // Move the whole session elsewhere and load it there.
    let moved = root.join("moved.d");
    std::fs::rename(&dir, &moved).expect("rename the session dir");
    let _ = std::fs::remove_dir_all(&pool); // the original pool is gone

    let mut loaded = HostSession::new();
    loaded.load_session(&moved).expect("load from the new path");
    let audio = bounce(&mut loaded, 4_800, &root.join("moved.wav"));
    assert!(
        audio.iter().any(|x| x.abs() > 1e-3),
        "still audible after a move"
    );
    assert_eq!(audio, reference, "and identical");

    let _ = std::fs::remove_dir_all(&root);
}

/// The journal is the autosave: every committed gesture is appended (with its
/// group markers), and a torn trailing line — a crash mid-write — is dropped on
/// load and reported, never a parse error that costs the session.
#[test]
fn the_journal_autosaves_and_a_torn_line_is_dropped() {
    let root = std::env::temp_dir().join(format!("host-journal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    s.execute(&HostCommand::Arrange {
        op: add_clip("c0", 0),
        at_frame: None,
    })
    .expect("clip");

    let dir = root.join("song.d");
    s.save(&dir).expect("save");
    assert_eq!(
        std::fs::read_to_string(dir.join("journal.txt")).expect("journal"),
        "",
        "a save resets the journal to its baseline"
    );

    // An edit after the save is autosaved, gestures included.
    s.execute(&gesture(vec![
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::Start,
            by_frames: 1_200,
        },
        media::ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: media::Edge::End,
            by_frames: -600,
        },
    ]))
    .expect("gesture");
    let journal = std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
    assert!(
        journal.contains("group begin"),
        "the gesture is bracketed:\n{journal}"
    );
    assert!(
        journal.contains("arrange trim t0 c0 start 1200"),
        "{journal}"
    );
    assert!(journal.contains("group end"), "{journal}");
    assert!(s.journal_error().is_none(), "no journal error");

    // Simulate a crash mid-append: a half-written line with no newline.
    let mut torn = std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
    torn.push_str("arrange trim t0 c0 start 99");
    std::fs::write(dir.join("journal.txt"), torn).expect("torn journal");

    let mut loaded = HostSession::new();
    loaded.load_session(&dir).expect("load with a torn journal");
    let recovery = loaded.last_recovery().cloned().expect("a recovery report");
    assert_eq!(recovery.torn_lines, 1, "the torn line is reported");
    assert_eq!(
        recovery.applied, 1,
        "the intact journal held one gesture (one command)"
    );
    assert_eq!(
        clip_of(&loaded).src_len,
        3_000,
        "the journal's gesture survived"
    );

    // The harder crash: cut *inside* a gesture — one member on disk, no `group
    // end`. The incomplete gesture is dropped (never half-applied) and the
    // session still opens.
    std::fs::write(
        dir.join("journal.txt"),
        "group begin\narrange trim t0 c0 start 1200\n",
    )
    .expect("mid-gesture journal");
    let mut loaded = HostSession::new();
    loaded
        .load_session(&dir)
        .expect("a gesture cut mid-write must not make the session unopenable");
    let recovery = loaded.last_recovery().cloned().expect("a recovery report");
    assert_eq!(recovery.applied, 0, "the incomplete gesture was dropped");
    assert_eq!(recovery.torn_lines, 2, "both of its lines are reported");
    assert_eq!(
        clip_of(&loaded).src_len,
        4_800,
        "and the clip is untouched — not half-trimmed"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The recovery record's one-line form — the wording the shells print beside
/// a `load` line. Counts first (they are the facts), then the first refusal's
/// own words, because a refusal worth showing is worth quoting. An empty
/// journal has no story, and a caller stays quiet rather than print zeros.
#[test]
fn a_recovery_report_describes_itself() {
    let clean = JournalRecovery::default();
    assert!(!clean.has_story(), "an empty journal has no story");

    let torn = JournalRecovery {
        applied: 2,
        torn_lines: 1,
        refused: 0,
        refused_reason: None,
    };
    assert!(torn.has_story());
    assert_eq!(torn.describe(), "journal: 2 applied, 1 torn, 0 refused");

    let refused = JournalRecovery {
        applied: 1,
        torn_lines: 1,
        refused: 2,
        refused_reason: Some("arrange add_clip t1 c0 s1 0 48000 … — no track t1".into()),
    };
    assert_eq!(
        refused.describe(),
        "journal: 1 applied, 1 torn, 2 refused — first refusal: \
         arrange add_clip t1 c0 s1 0 48000 … — no track t1"
    );
}

/// A session's rate is context: `session_rate` round-trips, the loaded session
/// runs at that rate, and a live session refuses to *change* rate.
#[test]
fn a_session_at_another_rate_round_trips() {
    let root = std::env::temp_dir().join(format!("host-rate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 44_100, 44_100);

    let script = format!(
        "host v1\nsession_rate 44100\nmount mixer channels=2 @0\npool {}\narrange add_track t0\narrange add_clip t0 c0 s1 0 44100 0 0 0 1.0\n",
        pool.display()
    );
    let mut s = run_script(&parse_script(&script).expect("parse")).expect("run");
    assert_eq!(s.sample_rate(), 44_100);

    let dir = root.join("cd.d");
    s.save(&dir).expect("save");
    let mut loaded = HostSession::new();
    loaded.load_session(&dir).expect("load");
    assert_eq!(loaded.sample_rate(), 44_100, "the rate round-trips");

    let audio = bounce(&mut loaded, 4_800, &root.join("cd.wav"));
    assert!(audio.iter().any(|x| x.abs() > 1e-3), "and it plays");
    let r = media::WavReader::open(&root.join("cd.wav")).expect("bounce");
    assert_eq!(r.sample_rate(), 44_100, "the bounce is at the session rate");

    // A live session cannot change rate — the clock and every frame derive from it.
    assert!(s.execute(&HostCommand::SessionRate { hz: 48_000 }).is_err());
    assert!(s.execute(&HostCommand::SessionRate { hz: 44_100 }).is_ok());

    let _ = std::fs::remove_dir_all(&root);
}

/// **The gate's finding #1**: a rebuild (undo, redo, seek) used to drop the
/// session directory, so autosave silently stopped after the first undo. The
/// journal must keep growing through both.
#[test]
fn autosave_survives_an_undo_and_a_seek() {
    let root = std::env::temp_dir().join(format!("host-autosave-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("autosave", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save");
    assert_eq!(s.session_dir(), Some(dir.as_path()));

    let journal = || std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::MoveClip {
            track: "t0".into(),
            clip: "c0".into(),
            at_frame: 4_800,
        },
        at_frame: None,
    })
    .expect("move");
    assert!(journal().contains("move_clip t0 c0 4800"), "{}", journal());
    let after_move = journal().len();

    // Undo and seek both rebuild the session — the directory must survive.
    assert!(s.undo().expect("undo"));
    assert_eq!(
        s.session_dir(),
        Some(dir.as_path()),
        "an undo must not lose the session directory"
    );
    assert!(s.journal_error().is_none());
    s.execute(&HostCommand::TransportSeek { frame: 2_400 })
        .expect("seek");
    assert_eq!(
        s.session_dir(),
        Some(dir.as_path()),
        "a seek must not lose the session directory"
    );

    // …so the next edit is autosaved, and the journal has grown.
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::SetClipGain {
            track: "t0".into(),
            clip: "c0".into(),
            gain: 0.5,
        },
        at_frame: None,
    })
    .expect("gain");
    let text = journal();
    assert!(text.contains("set_clip_gain t0 c0 0.5"), "{text}");
    assert!(
        text.len() > after_move,
        "the journal kept growing after the rebuild"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **The gate's finding #2**: `save` re-pointed the live session at the pool copy
/// but left the history naming the original, so a later replay re-adopted it (and
/// failed if the original was gone). An undo after a save must still play.
#[test]
fn an_undo_after_a_save_uses_the_session_pool() {
    let root = std::env::temp_dir().join(format!("host-rebase-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("elsewhere");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("rebase", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save");
    let reference = bounce(&mut s, 2_400, &root.join("a.wav"));

    // The original pool disappears — exactly a "save as" that left the source
    // behind — and then the session *rebuilds*: the seek and the undo both replay
    // the history, which must now name the session's own pool copy.
    std::fs::remove_dir_all(&pool).expect("remove the original pool");
    s.execute(&HostCommand::TransportSeek { frame: 0 })
        .expect("a seek after a save replays against the pool copy");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::MoveClip {
            track: "t0".into(),
            clip: "c0".into(),
            at_frame: 4_800,
        },
        at_frame: None,
    })
    .expect("move");

    // The undo rebuilds from history — which must name the session's own pool.
    assert!(s.undo().expect("undo"));
    let audio = bounce(&mut s, 2_400, &root.join("b.wav"));
    assert!(
        audio.iter().any(|x| x.abs() > 1e-3),
        "the replay found the pool copy and played"
    );
    assert_eq!(audio, reference, "and the audio is unchanged");

    let _ = std::fs::remove_dir_all(&root);
}

/// **The gate's findings #3/#4**: a stale journal (a crash between the script
/// rename and the journal reset) replays on the new baseline. Its commands are
/// already in the script, so they are refused — dropped and reported, never a
/// session that will not open.
#[test]
fn a_stale_journal_is_dropped_not_fatal() {
    let root = std::env::temp_dir().join(format!("host-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("stale", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save");

    // The stale journal re-adds the clip the script already has, then adds a
    // legitimate edit that must survive.
    std::fs::write(
        dir.join("journal.txt"),
        "arrange add_clip t0 c0 s1 0 4800 0 0 0 1.0\narrange set_clip_gain t0 c0 0.25\n",
    )
    .expect("stale journal");

    let mut loaded = HostSession::new();
    loaded
        .load_session(&dir)
        .expect("a stale journal must not stop the load");
    let recovery = loaded.last_recovery().cloned().expect("a report");
    assert_eq!(recovery.refused, 1, "the duplicate add_clip was refused");
    assert!(
        recovery.refused_reason.is_some(),
        "and the refusal is reported"
    );
    assert_eq!(recovery.applied, 1, "the legitimate edit applied");
    assert_eq!(clip_of(&loaded).gain, 0.25, "the edit is in the session");

    let _ = std::fs::remove_dir_all(&root);
}

/// A save that cannot be reproduced is **refused**, not written. The word-based
/// `host v1` form cannot express an id with whitespace, and it has no form at all
/// for a region play — writing either would produce a file that opens as a
/// *different* session. (A pool *path* with whitespace is fine: the save copies the
/// pool into the session, so the log names the copy.)
#[test]
fn a_save_that_cannot_round_trip_is_refused() {
    let root = std::env::temp_dir().join(format!("host-lossy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("my pool"); // the space is only in the *source* path
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("lossy", &pool);
    let dir = root.join("song.d");
    // A clip id with a space is what survives into the log and cannot be written.
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddClip {
            track: "t0".into(),
            clip: media::Clip {
                reversed: false,
                id: "c 0".into(),
                name: None,
                source: "s1".into(),
                src_start: 0,
                src_len: 2_400,
                at_frame: 0,
                fade_in: 0,
                fade_out: 0,
                gain: 1.0,
                loop_len: None,
            },
        },
        at_frame: None,
    })
    .expect("the timeline itself accepts the id");
    let refused = s.save(&dir);
    assert!(refused.is_err(), "an id with whitespace cannot be saved");
    assert!(
        !dir.join("session.txt").exists(),
        "nothing was written: {}",
        refused.expect_err("the error")
    );

    // A whole-file play saves; a region play does not. (A clean path here: a
    // whitespace path in a `play` line is refused by the same self-check, which is
    // what the case above just proved.)
    let clean = root.join("clean");
    std::fs::create_dir_all(&clean).expect("clean dir");
    write_tone(&clean, "s1", 48_000, 48_000);
    let mut s2 = HostSession::new();
    s2.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s2.execute(&HostCommand::Pool { dir: clean.clone() })
        .expect("pool");
    s2.execute(&HostCommand::Play {
        clip: media::ClipRef {
            path: clean.join("s1.wav"),
            start: 0,
            len: 0,
        },
        channel: 0,
        at_frame: None,
    })
    .expect("whole-file play");
    let dir2 = root.join("ok.d");
    s2.save(&dir2).expect("a whole-file play round-trips");
    assert!(dir2.join("session.txt").is_file());

    // The region case needs its own session: the reference host plays one clip at
    // a time, and the point is a *history* that holds a region play.
    let region = root.join("region.d");
    let mut s3 = HostSession::new();
    s3.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s3.execute(&HostCommand::Pool { dir: clean.clone() })
        .expect("pool");
    s3.execute(&HostCommand::Play {
        clip: media::ClipRef {
            path: clean.join("s1.wav"),
            start: 100,
            len: 50,
        },
        channel: 0,
        at_frame: None,
    })
    .expect("region play");
    let refused = s3.save(&region);
    assert!(
        refused.is_err(),
        "a region play has no host v1 form: {refused:?}"
    );
    assert!(
        !region.join("session.txt").exists(),
        "nothing was written: {}",
        refused.expect_err("the error")
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **The save's self-check has to have a twin on the autosave.** `save` refuses a
/// history the text form cannot spell, but the journal was written unchecked — and
/// a line `parse_script` rejects makes the **whole session** unopenable, not just
/// that edit. So a `play` whose path the word-based form cannot carry is applied but
/// *not* journalled, the failure is reported, and the journal keeps the edits it can
/// spell.
#[test]
fn a_journal_line_the_text_form_cannot_spell_is_not_written() {
    let root = std::env::temp_dir().join(format!("host-journal-lossy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("my pool"); // the space is in the *play* path
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("journal-lossy", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save the baseline");
    assert_eq!(
        std::fs::read_to_string(dir.join("journal.txt")).expect("journal"),
        "",
        "a save resets the journal to its baseline"
    );

    // The host plays a path with a space in it (any path `FilePlayer` opens)…
    s.execute(&HostCommand::Play {
        clip: media::ClipRef {
            path: pool.join("s1.wav"),
            start: 0,
            len: 0,
        },
        channel: 0,
        at_frame: None,
    })
    .expect("the live path plays it");
    // …and the autosave says so, naming the line it would not write.
    let error = s
        .journal_error()
        .expect("the autosave reports what it could not write")
        .to_string();
    assert!(
        error.contains("host v1") && error.contains("play"),
        "and it names the format and the line: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("journal.txt")).expect("journal"),
        "",
        "and the unspellable edit is not written"
    );

    // An edit the text form *can* spell still is, so the journal is not merely empty.
    s.execute(&gesture(vec![media::ArrangeOp::Trim {
        track: "t0".into(),
        clip: "c0".into(),
        edge: media::Edge::Start,
        by_frames: 1_200,
    }]))
    .expect("a spellable edit");

    let journal = std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
    assert!(
        !journal.contains("play"),
        "the unspellable play is not in the journal:\n{journal}"
    );
    assert!(
        journal.contains("arrange trim t0 c0 start 1200"),
        "but the spellable edit is: {journal}"
    );
    // …and the report of the refused one **stands**: a later successful write does
    // not put that play back in any durable record, and a save still refuses the
    // history, so clearing the message here would leave the loss unreported.
    let still = s
        .journal_error()
        .expect("a refusal is not erased by the next successful write")
        .to_string();
    assert!(
        still.contains("play") && still.contains("host v1"),
        "and it is the same report: {still}"
    );

    // The point of the guard: the session still opens, and it opens with the edit
    // that *was* spellable.
    let mut loaded = HostSession::new();
    loaded
        .load_session(&dir)
        .expect("an unspellable edit must not make the session unopenable");
    let recovery = loaded.last_recovery().cloned().expect("a report");
    assert_eq!(recovery.applied, 1, "the spellable edit replayed");
    assert_eq!(recovery.refused, 0, "nothing in the journal was refused");
    assert_eq!(
        clip_of(&loaded).src_len,
        3_600,
        "and the trim is in the reopened session"
    );
    assert!(s.save(&dir).is_err(), "a save still refuses the history");

    let _ = std::fs::remove_dir_all(&root);
}

/// **A value that cannot compare equal to itself must not wedge the writer that has
/// to spell it.** `HostCommand` carries `f32`/`f64` and IEEE-754 leaves `NaN != NaN`,
/// so the read-back — the autosave's, and `save`'s, which is the same check over a
/// whole script — answered "no" to a `NaN` operand *permanently*: the entry was
/// refused, the message blamed the spelling rather than the value, and every later
/// save of the session failed for good. The `host v1` form spells a `NaN` as `NaN`
/// and reads it back as `NaN`, so the entry **is** in the form.
///
/// **This is the autosave's half of the rule, and only that.** The journal's
/// predicate is *reads back as itself*, so the entry is written and the loss is
/// reported per entry on replay. `save` — the durable baseline — holds the stronger
/// rule, *and applies* (see `a_session_the_applier_refuses_is_not_saved`): a `NaN`
/// param is refused there by name, which is the honest verdict, because a session
/// file carrying it is a file `load_session` cannot open.
///
/// The `NaN` is committed through `commit_state` because the live path cannot produce
/// one: `Engine::set_param` refuses a non-finite value before it is logged (the test
/// below). What is under test is the *guard*, not the door — so the guard must be a
/// property of the form and not of the value's ability to compare equal to itself.
#[test]
fn a_value_that_cannot_compare_equal_to_itself_does_not_wedge_the_autosave() {
    let root = std::env::temp_dir().join(format!("host-journal-nan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("journal-nan", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save the baseline");
    let journal = || std::fs::read_to_string(dir.join("journal.txt")).expect("journal");

    // A `NaN` param committed to the document — the one way it can be in a history.
    s.commit_state(vec![HostCommand::SetParam {
        plugin: "mixer",
        param: "ch0.gain",
        value: f32::NAN,
        at_frame: None,
    }]);
    assert!(
        journal().contains("set_param mixer ch0.gain NaN"),
        "the form spells a NaN and reads it back, so the entry is journalled:\n{}",
        journal()
    );
    assert!(
        s.journal_error().is_none(),
        "and there is nothing to report: {:?}",
        s.journal_error()
    );

    // The next edit is journalled too: one operand that cannot compare equal to
    // itself stops nothing.
    s.execute(&gesture(vec![media::ArrangeOp::Trim {
        track: "t0".into(),
        clip: "c0".into(),
        edge: media::Edge::Start,
        by_frames: 1_200,
    }]))
    .expect("a normal edit");
    let text = journal();
    assert!(
        text.contains("set_param mixer ch0.gain NaN")
            && text.contains("arrange trim t0 c0 start 1200"),
        "both entries are in the journal:\n{text}"
    );

    // The reopened session costs that one refused param and nothing else: the entry
    // parses, and the *engine* refuses the value (the door below), which the replay
    // reports per entry rather than treating as fatal.
    let mut loaded = HostSession::new();
    loaded.load_session(&dir).expect("load");
    let recovery = loaded.last_recovery().cloned().expect("a report");
    assert_eq!(recovery.applied, 1, "the trim replayed (one gesture)");
    assert_eq!(recovery.refused, 1, "the NaN param is refused on replay");
    let reason = recovery.refused_reason.expect("and it says why");
    assert!(
        reason.contains("finite") && reason.contains("NaN"),
        "naming the value, not the spelling: {reason}"
    );
    assert_eq!(
        clip_of(&loaded).src_len,
        3_600,
        "and the edit beside it is in the reopened session"
    );

    // The permanent half of the wedge is gone too, and gone *honestly*: one such
    // operand no longer makes every later save refuse for a reason the user cannot
    // act on — the save now refuses it **by name**, because the file it would write
    // would not open. (`a_session_the_applier_refuses_is_not_saved` is where that
    // rule is pinned, along with what the refusal must leave behind.)
    assert!(
        s.save(&dir).is_err(),
        "a baseline the applier would refuse is not written"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **`SetParam` does not take a non-finite value, and the host keeps no second copy
/// of the rule.** The engine's `set_param` refuses it before the value is logged, so
/// the refusal is the engine's message and the host's commit path is never reached:
/// no history entry, no journal line, no reason to report, and a session that still
/// saves. This is what makes the test above a statement about the *form* — the door
/// is what stops a `NaN`, and this is where it is pinned on the host side.
#[test]
fn a_non_finite_param_is_refused_at_the_door_and_leaves_nothing_behind() {
    let root = std::env::temp_dir().join(format!("host-journal-door-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("journal-door", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save the baseline");

    for (value, what) in [(f32::NAN, "a NaN"), (f32::INFINITY, "an infinity")] {
        let refused = s
            .execute(&HostCommand::SetParam {
                plugin: "mixer",
                param: "ch0.gain",
                value,
                at_frame: None,
            })
            .expect_err("a non-finite param is refused");
        assert!(
            refused.contains("ch0.gain") && refused.contains("finite"),
            "{what} is named by the engine's rule: {refused}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("journal.txt")).expect("journal"),
        "",
        "and a refused command is not an edit, so nothing was journalled"
    );
    assert!(
        s.journal_error().is_none(),
        "nor is it a journal fault to report: {:?}",
        s.journal_error()
    );
    s.save(&dir).expect("and the session still saves");

    let _ = std::fs::remove_dir_all(&root);
}

/// **`save` must not be able to write a session directory that will not open.** The
/// self-check `save` inherited compared the re-parsed script with the history, so it
/// answered the *form*'s question — "is this the same session?" — and not the
/// reader's. An entry can be spelled faithfully and still be **refused by the
/// applier**: `set_param … NaN` is the one spelling a `NaN` has, and the one verdict
/// is `parameter '…' must be finite`. So the file parsed, the comparison passed, and
/// the platform wrote a `session.txt` that `load_session` refuses — a brick needing
/// a hand-edit of its own text, with the journal already truncated.
///
/// The rule is now *whatever is written can be read back **and applied***, checked by
/// building the session from the script through the walk `load_session` runs. So the
/// save is refused, by name, **before anything is written** — and the directory the
/// user already had (with the journal's unsaved tail in it) is exactly as it was.
#[test]
fn a_session_the_applier_refuses_is_not_saved() {
    let root = std::env::temp_dir().join(format!("host-save-apply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("save-apply", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save the baseline");

    // A spellable edit, so the journal holds a tail the refused save must not eat.
    s.execute(&gesture(vec![media::ArrangeOp::Trim {
        track: "t0".into(),
        clip: "c0".into(),
        edge: media::Edge::Start,
        by_frames: 1_200,
    }]))
    .expect("a spellable edit");
    // …and a value the applier refuses, committed to the document because the live
    // path cannot produce one (`Engine::set_param` is the door — the test above).
    s.commit_state(vec![HostCommand::SetParam {
        plugin: "mixer",
        param: "ch0.gain",
        value: f32::NAN,
        at_frame: None,
    }]);
    let script_before = std::fs::read(dir.join(SESSION_FILE)).expect("session");
    let journal_before = std::fs::read(dir.join(JOURNAL_FILE)).expect("journal");
    assert!(
        String::from_utf8_lossy(&journal_before).contains("arrange trim t0 c0 start 1200"),
        "the tail is the autosave: {}",
        String::from_utf8_lossy(&journal_before)
    );

    let refused = s
        .save(&dir)
        .expect_err("a script the applier refuses is not written");
    // **The edit and the reason, both by name** — an index and "the session text
    // does not apply" would leave the user with no way to act on either.
    assert!(
        refused.contains("set_param mixer ch0.gain NaN"),
        "the refusal names the edit it would not write: {refused}"
    );
    assert!(
        refused.contains("finite") && refused.contains("NaN"),
        "and the reason the reader would have refused it: {refused}"
    );

    // **Nothing was written, and nothing was emptied.** The order is the point: the
    // journal is the only durable record of the edits since the last save, so a save
    // that fails after truncating it destroys the tail to buy nothing.
    assert_eq!(
        std::fs::read(dir.join(SESSION_FILE)).expect("session"),
        script_before,
        "the previous session.txt is untouched"
    );
    assert_eq!(
        std::fs::read(dir.join(JOURNAL_FILE)).expect("journal"),
        journal_before,
        "and the journal kept the tail it was holding"
    );
    assert!(
        !dir.join("session.txt.tmp").exists(),
        "not even a temp file: a refused save writes nothing"
    );

    // The point of the order: the directory the user already had still opens, with
    // the edit in it. Without this the refused save would have bricked it.
    let mut loaded = HostSession::new();
    loaded
        .load_session(&dir)
        .expect("a refused save must not leave the directory unopenable");
    assert_eq!(
        clip_of(&loaded).src_len,
        3_600,
        "and it opens with the unsaved edit, not without it"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A missing file is a runtime condition, not an invalid document.** The `save`
/// self-check walked the script it was about to write by *building* it, and building
/// is **running**: the `Play` arm opened the named file through `resolve_clip`, spawned
/// a `FilePlayer` and blocked in `warm_player` on a ten-second deadline. So a
/// `play` whose file had moved — a rename, an ejected drive, a pool directory deleted
/// out from under the session — failed the build, and `save` refused a history that is
/// a perfectly good document, over a fact about *this machine's copy of the material*.
/// That is the same class of wrong as a save that fails because the disk is full: the
/// document is fine and the session must still save.
///
/// The rule is now that the check asks the **document's** questions (does this mixer
/// exist, is this channel inside it, is this op legal in this arrangement) and not the
/// **material's** (can this file be opened, decoded and warmed). So the save succeeds
/// and the directory is written; the missing file is reported where it always was, by
/// the **reader** — and this test pins that half too, so the tolerance above cannot
/// quietly become silence: reopening still names the file, in the shape `load_session`
/// has always used for it.
#[test]
fn a_session_whose_play_file_moved_still_saves() {
    let root = std::env::temp_dir().join(format!("host-save-moved-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("save-moved", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save the baseline");

    // The recorder's shape: a `play` naming a file **by path** (the pool directory is
    // *not* re-pointed at the copy — `rebase_pool` rewrites `pool` commands only, so
    // the play line keeps the absolute path it was made with). The file is there now.
    let clip = pool.join("s1.wav");
    s.execute(&HostCommand::Play {
        clip: media::ClipRef {
            path: clip.clone(),
            start: 0,
            len: 0,
        },
        channel: 0,
        at_frame: None,
    })
    .expect("play it while it is still there");

    // …and now it is not. The session has not changed: the same commands, the same
    // document, one fewer file on this machine.
    std::fs::remove_file(&clip).expect("the file moves");

    s.save(&dir)
        .expect("a file that moved is a runtime condition, not an invalid document");
    let script = std::fs::read_to_string(dir.join(SESSION_FILE)).expect("session");
    assert!(
        script.contains(&format!("play {}", clip.display())),
        "the play line is in the baseline: it is part of the session, and a file that has \
         moved is not a reason to refuse one"
    );
    // The journal was reset to the new baseline, so the save is whole rather than a
    // write that left the previous one in place.
    assert_eq!(
        std::fs::read_to_string(dir.join(JOURNAL_FILE)).expect("journal"),
        "",
        "and the journal restarts from the baseline it just wrote"
    );

    // **The reader still reports the missing file**, in the shape `load_session` has
    // always used for it: the baseline is a document walk, and a `play` it cannot
    // resolve is an `Err` naming the path. Untouched by this change, and pinned here
    // because "the save tolerates it" must never become "nobody mentions it".
    let mut loaded = HostSession::new();
    let err = loaded
        .load_session(&dir)
        .expect_err("the reader reports the file that is not there");
    assert!(
        err.contains("s1.wav"),
        "naming the file, which is the actionable fact: {err}"
    );

    // **And the journal path is the tolerant one it has always been**: a `play` that
    // reaches the load through the *autosave* is dropped and reported, not fatal, so a
    // session edited after its material moved still opens. A second session, because
    // the first one's baseline already holds the play and this half is about the tail.
    write_tone(&pool, "s1", 48_000, 48_000); // the file comes back
    let mut s2 = session_with_pool("save-moved-journal", &pool);
    s2.save(&dir).expect("save a baseline with no play in it");
    s2.execute(&HostCommand::Play {
        clip: media::ClipRef {
            path: clip.clone(),
            start: 0,
            len: 0,
        },
        channel: 0,
        at_frame: None,
    })
    .expect("play it");
    // …and goes away again, *after* the play reached the autosave.
    let _ = std::fs::remove_file(&clip);
    let journal = std::fs::read_to_string(dir.join(JOURNAL_FILE)).expect("journal");
    assert!(
        journal.contains("play "),
        "the play is in the autosave tail, which is the path being tested: {journal}"
    );

    let mut loaded = HostSession::new();
    loaded
        .load_session(&dir)
        .expect("a journal play whose file moved is dropped, not fatal");
    let recovery = loaded.last_recovery().cloned().expect("a report");
    assert_eq!(
        recovery.refused, 1,
        "the one refused command is the play, reported"
    );
    assert!(
        recovery
            .refused_reason
            .as_deref()
            .is_some_and(|r| r.contains("s1.wav")),
        "naming the file, which is the actionable fact: {:?}",
        recovery.refused_reason
    );
    assert_eq!(
        clip_of(&loaded).src_len,
        4_800,
        "and the rest of the session is intact"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **The dry run must not open the material, and the way to show that is a file it
/// could open and must not.** The moved-file test above is not enough on its own: a
/// missing file is the one case the old code refused *fast*, so it says nothing about
/// the ten-second `warm_player` deadline or the decoder thread, which are paid for a
/// file that *is* there. This names a file that exists and is deliberately **not a
/// WAV**, so the old code read it (`not a RIFF file`) before it ever reached the warm
/// — and a check that opens no material cannot be refused by what is at the path.
/// The clock assertion is a bound, not a measurement: it is there so a future change
/// that reintroduces a blocking read fails *here*, loudly, rather than costing a user
/// ten seconds per save.
#[test]
fn the_save_check_never_opens_the_material_it_validates() {
    let root = std::env::temp_dir().join(format!("host-save-nomedia-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("save-nomedia", &pool);
    let dir = root.join("song.d");

    // A `play` of a path that is not audio at all, committed to the document the way
    // the live path commits a real one (the pool's source is fine; this names a file
    // beside it).
    let not_audio = root.join("notes.txt");
    std::fs::write(&not_audio, b"this is not a wav file\n").expect("write");
    s.commit_state(vec![HostCommand::Play {
        clip: media::ClipRef {
            path: not_audio.clone(),
            start: 0,
            len: 0,
        },
        channel: 0,
        at_frame: None,
    }]);

    let started = std::time::Instant::now();
    s.save(&dir)
        .expect("the check asks the document's questions, not the file's");
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the save must not wait on a decoder it never starts: {elapsed:?}"
    );
    assert!(
        std::fs::read_to_string(dir.join(SESSION_FILE))
            .expect("session")
            .contains(&format!("play {}", not_audio.display())),
        "and the play line is written: a file's contents are not a property of the document"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A successful save must not manufacture a fault whose text is false.** `save`
/// re-derives the outstanding refusal after it writes, because the pool re-point above
/// can have *changed* the answer — the journal spells a `pool` by the absolute path
/// the session used, a save spells the copy inside the session directory. But the
/// re-derivation asked only the **journal's** spelling, and those two differ for
/// exactly one line: `Pool`. So saving into a directory whose own path holds a space
/// (`"/…/My Songs/song.d"`) re-pointed the history at `"/…/My Songs/song.d/pool"`,
/// the journal could not spell that, and the save **reported a refusal over a save
/// that had plainly succeeded** — a message the user cannot act on, naming a
/// text-form failure for a path the text form spells perfectly well as `pool pool`.
///
/// The report is a claim about **both** durable records, so it is now made only where
/// both agree: an entry the save can write is not "in the live session and nowhere
/// else", because it is in the baseline this save just wrote. A save into a directory
/// with a space in it therefore leaves no fault at all, and — the half that matters
/// most, because a false alarm is worse than none — the history stays saveable and
/// the report stays silent across a later edit.
#[test]
fn a_save_into_a_directory_with_a_space_reports_nothing() {
    let root = std::env::temp_dir().join(format!("host-save-space-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("save-space", &pool);
    // The space is in the *session* path, which is what the pool re-point then copies
    // into: the history's `pool` line becomes `<dir>/pool`, unspellable in the
    // journal's spelling and spellable in the save's.
    let dir = root.join("My Songs").join("song.d");
    s.save(&dir).expect("a path with a space is a legal path");
    assert!(
        s.journal_error().is_none(),
        "a save that succeeded reports nothing: {:?}",
        s.journal_error()
    );
    assert!(
        std::fs::read_to_string(dir.join(SESSION_FILE))
            .expect("session")
            .contains("pool pool\n"),
        "and the save spells the pool the way a movable session must: relative"
    );

    // **And it stays silent.** The re-derivation runs again on every history change,
    // so a false claim here would come back on the next `undo` too. A spellable edit
    // is autosaved and reported as nothing.
    s.execute(&gesture(vec![media::ArrangeOp::Trim {
        track: "t0".into(),
        clip: "c0".into(),
        edge: media::Edge::Start,
        by_frames: 1_200,
    }]))
    .expect("a spellable edit");
    assert!(
        s.journal_error().is_none(),
        "and a later edit does not resurrect the false alarm: {:?}",
        s.journal_error()
    );
    s.save(&dir).expect("and the session saves again");
    assert!(
        s.journal_error().is_none(),
        "still nothing to report: {:?}",
        s.journal_error()
    );

    // The directory is a normal, movable session: it opens from where it is.
    let mut loaded = HostSession::new();
    loaded.load_session(&dir).expect("open");
    assert_eq!(
        loaded.last_recovery().map(|r| r.refused),
        Some(0),
        "and nothing in it needed dropping"
    );
    assert_eq!(
        clip_of(&loaded).src_len,
        3_600,
        "with the edits that were saved"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A report cannot outlive the edit it names.** The refusal is a fact about an
/// *edit*, and `undo`/`redo` rebuild the session from the history — so an edit that
/// has been undone is in no durable record because it is **not in the session at
/// all**. Carried across the rebuild, the report outlived its subject and the TUI's
/// status line read "autosave failed" after every subsequent edit, over an edit that
/// was no longer there; the one thing that would have cleared it is the next `save`,
/// which now *refuses* a history holding such an entry, so it never would have.
///
/// The report is therefore **re-derived from the history** rather than latched, and
/// re-derived is not the same as cleared: the *redo* at the end puts the edit back
/// into the session, so its report must come back with it.
#[test]
fn an_undo_that_removes_a_refused_edit_clears_the_report() {
    let root = std::env::temp_dir().join(format!("host-journal-alarm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("journal-alarm", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save the baseline");

    // A clip id with a space: the timeline takes it, the word-based form cannot spell
    // it, so the autosave drops the entry and says so. An `Arrange` op, so it is
    // undoable — the false alarm needs a *removable* subject.
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddClip {
            track: "t0".into(),
            clip: media::Clip {
                reversed: false,
                id: "c 0".into(),
                name: None,
                source: "s1".into(),
                src_start: 0,
                src_len: 2_400,
                at_frame: 0,
                fade_in: 0,
                fade_out: 0,
                gain: 1.0,
                loop_len: None,
            },
        },
        at_frame: None,
    })
    .expect("the timeline itself accepts the id");
    let report = s
        .journal_error()
        .expect("the autosave reports what it could not write")
        .to_string();
    assert!(
        report.contains("c 0") && report.contains("not autosaved"),
        "and it names the edit that is in no durable record: {report}"
    );

    // A **successful** write must not erase it — half the rule, and the half the
    // previous change restored. (A `SetParam`, so it is not the `Arrange` entry the
    // undo below is about.)
    s.execute(&HostCommand::SetParam {
        plugin: "mixer",
        param: "ch0.gain",
        value: 0.5,
        at_frame: None,
    })
    .expect("a spellable edit");
    assert!(
        std::fs::read_to_string(dir.join(JOURNAL_FILE))
            .expect("journal")
            .contains("set_param mixer ch0.gain 0.5"),
        "the autosave carried this one"
    );
    assert!(
        s.journal_error().is_some(),
        "and the refusal stands beside it: {:?}",
        s.journal_error()
    );

    // Undo it: the edit leaves the history, so there is nothing left to report.
    assert!(s.undo().expect("undo the refused edit"));
    assert!(
        s.journal_error().is_none(),
        "the report cannot outlive the edit it names: {:?}",
        s.journal_error()
    );

    // **Re-derived, not latched**: the redo puts the edit back in the session, so
    // its report must come back with it. (Before the next edit, which clears the
    // redo stack — a new state change makes an undone branch unreachable.)
    assert!(s.redo().expect("redo the refused edit"));
    let back = s
        .journal_error()
        .expect("an edit that is in the session again is lost again")
        .to_string();
    assert!(
        back.contains("c 0"),
        "and it is the same edit's report: {back}"
    );
    assert!(s.undo().expect("undo it again"));
    assert!(
        s.journal_error().is_none(),
        "and it dies with its subject again: {:?}",
        s.journal_error()
    );

    // …and the next edit is autosaved and silent, which is what the user sees: no
    // status line at all, rather than a standing "autosave failed".
    s.execute(&gesture(vec![media::ArrangeOp::Trim {
        track: "t0".into(),
        clip: "c0".into(),
        edge: media::Edge::Start,
        by_frames: 1_200,
    }]))
    .expect("a spellable edit after the undo");
    let journal = std::fs::read_to_string(dir.join(JOURNAL_FILE)).expect("journal");
    assert!(
        journal.contains("arrange trim t0 c0 start 1200"),
        "the autosave is working again: {journal}"
    );
    assert!(
        s.journal_error().is_none(),
        "and there is nothing to say about it: {:?}",
        s.journal_error()
    );

    // The session directory the whole alarm was about is healthy: it opens, and it
    // opens with the edits that are in the session.
    let mut loaded = HostSession::new();
    loaded.load_session(&dir).expect("load");
    assert_eq!(
        loaded.last_recovery().map(|r| r.refused),
        Some(0),
        "nothing in the journal needed dropping"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A journal line **nobody can parse** is dropped and reported, never fatal. The
/// write side refuses to produce one (the case above), so this is what a
/// hand-edited journal, or one from an older host, looks like — and the rule is the
/// one `apply_journal` already states for a *refused* entry: an autosave line must
/// not cost the user the session it sits in. The good line beside it still lands.
#[test]
fn an_unparseable_journal_line_is_dropped_not_fatal() {
    let root = std::env::temp_dir().join(format!("host-journal-garbage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");
    write_tone(&pool, "s1", 48_000, 48_000);

    let mut s = session_with_pool("journal-garbage", &pool);
    let dir = root.join("song.d");
    s.save(&dir).expect("save the baseline");

    // A play line with a space in its path is four words against a three-word form.
    // The gesture after it is well formed and must survive.
    std::fs::write(
        dir.join("journal.txt"),
        "play /tmp/a b.wav ch0\ngroup begin\narrange set_clip_gain t0 c0 0.25\ngroup end\n",
    )
    .expect("a hand-written journal");

    let mut loaded = HostSession::new();
    loaded
        .load_session(&dir)
        .expect("an unparseable journal line must not stop the load");
    let recovery = loaded.last_recovery().cloned().expect("a report");
    assert_eq!(recovery.applied, 1, "the gesture beside it applied");
    assert_eq!(recovery.refused, 1, "the unparseable entry is reported");
    let reason = recovery
        .refused_reason
        .clone()
        .expect("and the refusal says why");
    assert!(
        reason.contains("play") || reason.contains("operand"),
        "the reason names the line: {reason}"
    );
    assert_eq!(clip_of(&loaded).gain, 0.25, "the gesture is in the session");

    let _ = std::fs::remove_dir_all(&root);
}

// ---- recording: the input device becomes pool material ----

/// Feed an interleaved tone into a ring, the way a device callback would.
fn feed_tone(ring: &media::Spsc<f32>, frames: usize, channels: usize, rate: u32) {
    let mut i = 0usize;
    while i < frames {
        let phase = std::f64::consts::TAU * 440.0 * (i as f64) / rate as f64;
        let sample = (phase.sin() * 0.5) as f32;
        for _ in 0..channels {
            // Bounded: the ring is big, but a full ring is not a test failure.
            let _ = ring.try_push(sample);
        }
        i += 1;
    }
}

/// **The loop's first half**: a take recorded through the host lands in the pool
/// as `{take_id}.ch{k}` sources with peaks, is arrangeable, and is audible — the
/// whole record → arrange → render path, with the device replaced by the ring
/// seam.
#[test]
fn a_take_records_into_the_pool_and_plays() {
    let root = std::env::temp_dir().join(format!("host-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");

    let mut s = HostSession::new();
    s.execute(&HostCommand::Mount {
        plugin: "mixer",
        params: vec![("channels", 2.0)],
        at_frame: Some(0),
    })
    .expect("mixer");
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");

    let rate = s.sample_rate();
    let ring = std::sync::Arc::new(media::Spsc::new(1 << 16));
    s.start_recording("jam", std::sync::Arc::clone(&ring), rate, 2)
        .expect("record starts");
    let status = s.recording().expect("a status while recording");
    assert_eq!(status.take_id, "jam");
    assert_eq!(status.channels, 2);

    // Push a second of interleaved tone, then stop once the demux has drained
    // what the ring accepted. A fixed sleep here is a **race** — a loaded
    // parallel run failed it (found as a flake by slice D1's gate) — and the
    // ring drops what it cannot hold, so "wait for N frames" cannot work: poll
    // the live count instead and wait for it to stop moving (three equal,
    // non-zero readings), with a bound so a stuck demux still fails loudly in
    // the assertion below rather than hanging.
    feed_tone(&ring, rate as usize, 2, rate);
    let mut last = 0u64;
    let mut steady = 0;
    for _ in 0..400 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let now = s.recording().map(|status| status.frames).unwrap_or(0);
        if now > 0 && now == last {
            steady += 1;
            if steady >= 3 {
                break;
            }
        } else {
            steady = 0;
        }
        last = now;
    }
    let take = s.stop_recording().expect("record stops");
    assert_eq!(take.take_id, "jam");
    assert_eq!(take.channels, 2);
    assert_eq!(
        take.sources,
        vec!["jam.ch0".to_string(), "jam.ch1".to_string()]
    );
    assert!(
        take.frames > rate as u64 / 2,
        "a second of input produced {} frames",
        take.frames
    );
    assert!(s.recording().is_none(), "the take is finished");
    assert_eq!(s.last_take(), Some(&take));

    // The takes are pool sources, with peaks — and they are at the session rate.
    let index = media::Pool::open(&pool)
        .expect("pool")
        .list()
        .expect("list");
    for (k, source) in index.sources.iter().enumerate() {
        assert_eq!(source.id, format!("jam.ch{k}"));
        assert_eq!(source.sample_rate, rate, "the take is at the session rate");
        assert!(!source.peaks_missing, "the peaks are written");
    }

    // …and they play: place ch0 on a track and render it.
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddTrack { track: "t0".into() },
        at_frame: None,
    })
    .expect("track");
    s.execute(&HostCommand::Arrange {
        op: media::ArrangeOp::AddClip {
            track: "t0".into(),
            clip: media::Clip {
                reversed: false,
                id: "c0".into(),
                name: None,
                source: "jam.ch0".into(),
                src_start: 0,
                src_len: take.frames.min(rate as u64),
                at_frame: 0,
                fade_in: 0,
                fade_out: 0,
                gain: 1.0,
                loop_len: None,
            },
        },
        at_frame: None,
    })
    .expect("clip from the take");
    let audio = bounce(&mut s, 4_800, &root.join("t.wav"));
    assert!(
        audio.iter().any(|x| x.abs() > 1e-3),
        "the recorded take is audible"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A finished take is **state**: the capture is the side effect, and what the session
/// keeps is the declaration — so the document names its own material and never carries
/// the `record` line that would open a device on replay.
#[test]
fn a_finished_take_is_committed_to_the_session_state() {
    let root = std::env::temp_dir().join(format!("host-take-commit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("work");
    std::fs::create_dir_all(&pool).expect("root");

    let mut s = HostSession::new();
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    let ring = std::sync::Arc::new(media::Spsc::<f32>::new(64));
    s.start_recording("jam", ring, 48_000, 2)
        .expect("the take starts");
    s.execute(&HostCommand::RecordStop).expect("the take stops");

    let text = s.script_text(&root).expect("the session writes");
    assert!(
        text.contains("take jam "),
        "the document names the take that landed: {text}"
    );
    assert!(
        !text.contains("record jam"),
        "the capture is an action and must never reach the document: {text}"
    );
    let back = parse_script(&text).expect("the session parses");
    assert!(
        back.iter().any(|c| matches!(
            c,
            HostCommand::Take { take_id, channels, .. } if take_id == "jam" && *channels == 2
        )),
        "the declaration round-trips: {text}"
    );
}

/// The pool-id cache the snapshot publishes: `set_pool` lists the directory, a
/// finished take joins it, and a refresh that fails **keeps** what it had.
///
/// The last is the point. A shell auto-names takes against this list, so a transient
/// listing failure that emptied it would restart the names at `take-1` and eat a
/// refusal for an id the pool already holds. The host's free-id check is the safety
/// net against overwrite, not against forgetting.
#[test]
fn the_pool_id_cache_lists_the_pool_and_survives_a_failed_refresh() {
    let root = std::env::temp_dir().join(format!("host-pool-ids-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");

    // A source already in the directory is listed when the pool is bound.
    let held = pool.join("held.ch0.wav");
    let mut writer = media::wav::WavWriter::create_float(&held, 48_000, 1).expect("create");
    writer.write(&[0.0f32; 64]).expect("samples");
    writer.finalize().expect("finalize");

    let mut s = HostSession::new();
    assert!(
        s.pool_ids().is_empty(),
        "nothing is listed before a pool is bound"
    );
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    assert!(
        s.pool_ids().iter().any(|id| id == "held.ch0"),
        "set_pool lists what the directory holds: {:?}",
        s.pool_ids()
    );

    // A **rebind** clears: the previous pool's names must not follow the session to
    // a different directory. (The merge gate caught this half unpinned — the rest of
    // the test binds one pool, so carrying the old ids across would have passed.)
    let other = root.join("other");
    std::fs::create_dir_all(&other).expect("other pool");
    s.execute(&HostCommand::Pool { dir: other })
        .expect("rebind");
    assert!(
        !s.pool_ids().iter().any(|id| id == "held.ch0"),
        "a rebind drops the previous pool's names: {:?}",
        s.pool_ids()
    );
    // Binding the original pool again lists it again, and keeps the rest of the
    // test on the pool that holds the take.
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("rebind back");
    assert!(
        s.pool_ids().iter().any(|id| id == "held.ch0"),
        "binding the original pool lists it again: {:?}",
        s.pool_ids()
    );

    // A finished take's sources join the cache without a re-bind.
    let ring = std::sync::Arc::new(media::Spsc::<f32>::new(64));
    s.start_recording("jam", ring, 48_000, 2)
        .expect("the take starts");
    s.execute(&HostCommand::RecordStop).expect("the take stops");
    for source in ["jam.ch0", "jam.ch1"] {
        assert!(
            s.pool_ids().iter().any(|id| id == source),
            "the finished take's {source} joins the cache: {:?}",
            s.pool_ids()
        );
    }

    // A refresh that cannot read the directory leaves the cache as it was: the
    // session still holds these ids, and forgetting them is the defect.
    let ring = std::sync::Arc::new(media::Spsc::<f32>::new(64));
    s.start_recording("jam-2", ring, 48_000, 1)
        .expect("the take starts");
    let before = s.pool_ids().to_vec();
    std::fs::remove_dir_all(&pool).expect("the pool directory goes away under the session");
    assert!(
        s.stop_recording().is_err(),
        "finalizing had nowhere to write"
    );
    assert_eq!(
        s.pool_ids(),
        before.as_slice(),
        "a failed refresh keeps the ids it had"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A `.wav` that cannot be read still **holds its name**: the cache lists it, or a
/// shell naming a take against the list proposes an id the host then refuses.
///
/// `Pool::list` reports an unreadable source in `errors` and skips it from `sources`,
/// so mapping only `sources` lost the name — the merge gate's finding, the same
/// refusal-eating symptom one level below a missing directory.
#[test]
fn an_unreadable_pool_file_still_holds_its_name_in_the_cache() {
    let root = std::env::temp_dir().join(format!("host-pool-broken-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");

    // Zero bytes: the file is there, but it cannot be parsed as a WAV.
    std::fs::write(pool.join("broken.ch0.wav"), []).expect("a zero-byte wav");

    let mut s = HostSession::new();
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    assert!(
        s.pool_ids().iter().any(|id| id == "broken.ch0"),
        "an unreadable file's name is still held: {:?}",
        s.pool_ids()
    );

    // Knowing it is the point: the host still refuses a capture over that name, so
    // the shell has to be able to skip it rather than propose it.
    let ring = std::sync::Arc::new(media::Spsc::<f32>::new(64));
    assert!(
        s.start_recording("broken", ring, 48_000, 1).is_err(),
        "the occupied name is refused, not overwritten"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A take that finalizes **with a complaint** is still declared.
///
/// The capture is the side effect and the declaration is the state: a take whose WAV
/// and peaks landed, with a complaint about the tail, has to be in the document or a
/// reload silently loses it. The complaint is surfaced either way.
#[test]
fn a_take_that_finalizes_with_a_complaint_is_still_declared() {
    let root = std::env::temp_dir().join(format!("host-take-complaint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pool = root.join("pool");
    std::fs::create_dir_all(&pool).expect("pool dir");

    let mut s = HostSession::new();
    s.execute(&HostCommand::Pool { dir: pool.clone() })
        .expect("pool");
    let ring = std::sync::Arc::new(media::Spsc::<f32>::new(64));
    s.start_recording("jam", ring, 48_000, 1)
        .expect("the take starts");

    // The directory goes away under the take, so finalizing has nowhere to write: the
    // stop complains, and the files already written are what is left.
    std::fs::remove_dir_all(&pool).expect("the pool goes away under the take");
    assert!(
        s.execute(&HostCommand::RecordStop).is_err(),
        "the stop reports the complaint"
    );

    // The declaration is state even so — the take did not vanish with the complaint,
    // and a document that omitted it would lose the take on reload.
    let text = s.script_text(&root).expect("the session writes");
    assert!(
        text.contains("take jam "),
        "a complained take is still declared: {text}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The point of the declaration: a loaded session binds its take **without opening a
/// device** — what is in the WAV is not this layer's business.
#[test]
fn a_take_declaration_replays_without_a_device() {
    let script = parse_script("host v1\nsession_rate 48000\ntake jam 96000 7 2 120\n")
        .expect("a take line parses");
    let s = run_script(&script).expect("the session replays");
    let take = s.last_take().expect("the take is bound");
    assert_eq!(take.take_id, "jam");
    assert_eq!(take.frames, 96_000);
    assert_eq!(take.dropped, 7);
    assert_eq!(take.channels, 2);
    assert_eq!(take.sources, vec!["jam.ch0", "jam.ch1"]);
    assert_eq!(
        take.sample_rate, 48_000,
        "the session's rate, not a declared one"
    );
    assert_eq!(take.at_frame, 120, "the take's origin is kept");
}

/// A declaration is **wire input** that names its own pool sources: the take's channel
/// count sizes `{take_id}.ch{k}`, so a malformed one would size that vector from the
/// script line — thirty bytes aborting the process. The bound is the capture sanity
/// bound, the same rule every other channel count on this path obeys.
#[test]
fn a_take_declaration_refuses_a_channel_count_it_could_not_have_recorded() {
    let sanity = media::capture::CAPTURE_CHANNELS_SANITY;

    // The wire form: a well-formed line whose channel count no capture could produce.
    let script = parse_script(&format!("host v1\ntake jam 96000 0 {} 0\n", sanity + 1))
        .expect("the line's shape is well formed");
    let refused = run_script(&script).expect_err("an unboundable width is refused");
    assert!(refused.contains("channels"), "and it says why: {refused}");

    // The same bound in memory, where a replayed declaration comes from and where
    // `usize::MAX` is a capacity overflow rather than a merely enormous allocation.
    let mut s = HostSession::new();
    assert!(
        s.execute(&HostCommand::Take {
            take_id: "jam".into(),
            frames: 0,
            dropped: 0,
            channels: usize::MAX,
            at_frame: 0,
        })
        .is_err(),
        "a width no capture could have recorded is refused"
    );
    assert!(s.last_take().is_none(), "and nothing landed in the session");

    // The bound is inclusive, and it is the *sanity* bound rather than a new ceiling:
    // a take recorded at the sanity width still replays.
    let widest =
        parse_script(&format!("host v1\ntake jam 96000 0 {sanity} 0\n")).expect("it parses");
    let s = run_script(&widest).expect("the widest legal take binds");
    assert_eq!(s.last_take().expect("the take is bound").channels, sanity);
}

/// Recording is one take at a time, needs somewhere to put it, and stopping
/// nothing is an error — never a silent half-take or a panic.
#[test]
fn recording_refuses_what_it_cannot_do() {
    let root = std::env::temp_dir().join(format!("host-record-refuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");

    // No pool: the take has nowhere to land.
    let mut s = HostSession::new();
    let ring = std::sync::Arc::new(media::Spsc::<f32>::new(64));
    let refused = s.start_recording("jam", std::sync::Arc::clone(&ring), 48_000, 1);
    assert!(refused.is_err(), "{refused:?}");
    assert!(
        refused.expect_err("the reason").contains("set_pool"),
        "the refusal names the fix"
    );

    // With a pool: one at a time, and a stop with nothing recording is refused.
    s.execute(&HostCommand::Pool { dir: root.clone() })
        .expect("pool");
    s.start_recording("jam", std::sync::Arc::clone(&ring), 48_000, 1)
        .expect("first take starts");
    assert!(
        s.start_recording("jam2", std::sync::Arc::clone(&ring), 48_000, 1)
            .is_err(),
        "one take at a time"
    );
    let take = s.stop_recording().expect("the take stops");
    assert_eq!(take.take_id, "jam");
    assert!(s.stop_recording().is_err(), "stopping nothing is an error");

    // A bad take id is refused by the media layer, with its rule stated.
    let refused = s.start_recording("bad id!", std::sync::Arc::clone(&ring), 48_000, 1);
    assert!(refused.is_err(), "a take id goes into filenames");

    // Re-recording over an existing take is refused: a clip that already references
    // `{take_id}.ch0` must not silently change content.
    let again = s.start_recording("jam", std::sync::Arc::clone(&ring), 48_000, 1);
    assert!(again.is_err(), "an existing take id is refused");
    assert!(
        again
            .expect_err("the reason")
            .contains("already has a take"),
        "and it says which file is in the way"
    );

    // A rebuild (`transport seek`) stops a take in progress — finalized, and the
    // report survives the rebuild so the shell can still place it.
    s.start_recording("later", std::sync::Arc::clone(&ring), 48_000, 1)
        .expect("a second take under a new id");
    s.execute(&HostCommand::TransportSeek { frame: 0 })
        .expect("the seek rebuilds the session");
    assert!(s.recording().is_none(), "the take was stopped, not dropped");
    let interrupted = s
        .last_take()
        .cloned()
        .expect("the interrupted take is reported");
    assert_eq!(interrupted.take_id, "later");
    assert!(
        root.join("later.ch0.wav").is_file(),
        "and its WAV was finalized in the pool"
    );

    // The text form: `record <take_id>` starts, `record stop` stops.
    let parsed = parse_script("host v1\nrecord jam1\nrecord stop\n").expect("parse");
    assert!(matches!(&parsed[0], HostCommand::Record { take_id } if take_id == "jam1"));
    assert!(matches!(&parsed[1], HostCommand::RecordStop));

    let _ = std::fs::remove_dir_all(&root);
}

/// **A grid is UI state, a snapped frame is the log's.** `snap=<frames>` is a
/// parse-time modifier: it quantizes the frame operand, so a *script* snaps
/// exactly like the shell without the log ever learning what a grid is. The
/// formatter therefore writes the snapped frame and no modifier.
#[test]
fn the_snap_modifier_quantizes_the_frame_operand() {
    // The plan's example: 48 213 frames on a 480-frame grid is 48 000.
    let (op, at) = parse_arrange_line("arrange move_clip t0 c0 48213 snap=480 @0").unwrap();
    assert_eq!(at, Some(0));
    let media::ArrangeOp::MoveClip { at_frame, .. } = op else {
        panic!("expected move_clip, got {op:?}");
    };
    assert_eq!(at_frame, 48_000);
    assert_eq!(
        format_command(
            &HostCommand::Arrange {
                op: media::ArrangeOp::MoveClip {
                    track: "t0".into(),
                    clip: "c0".into(),
                    at_frame,
                },
                at_frame: Some(0),
            },
            None,
        )
        .as_deref(),
        Some("arrange move_clip t0 c0 48000 @0"),
        "the log records the snapped frame, never the grid"
    );

    // Every op with a frame operand takes it, and the modifier may precede
    // the `@frame` token or follow it.
    let script = "host v1\nmount mixer channels=2 @0\narrange add_clip t0 c0 s1 0 4000 48213 0 0 1.0 snap=24000\narrange razor_split t0 c0 cL cR 48213 snap=480 @0\narrange move_clip_to_track t0 c0 t1 48213 snap=480\ntransport seek 48213 snap=480\n";
    let commands = parse_script(script).expect("the modifier parses anywhere it applies");
    let frames: Vec<u64> = commands
        .iter()
        .filter_map(|c| match c {
            HostCommand::Arrange { op, .. } => match op {
                media::ArrangeOp::AddClip { clip, .. } => Some(clip.at_frame),
                media::ArrangeOp::RazorSplit { at_frame, .. } => Some(*at_frame),
                media::ArrangeOp::MoveClipToTrack { at_frame, .. } => Some(*at_frame),
                _ => None,
            },
            HostCommand::TransportSeek { frame } => Some(*frame),
            _ => None,
        })
        .collect();
    assert_eq!(frames, vec![48_000, 48_000, 48_000, 48_000]);

    // A modifier that cannot apply is an error, never a silent no-op.
    for bad in [
        "host v1\narrange delete t0 c0 snap=480\n",
        "host v1\nmount mixer channels=2 snap=480 @0\n",
        "host v1\narrange move_clip t0 c0 48213 snap=0\n",
        "host v1\narrange move_clip t0 c0 48213 snap=abc\n",
        "host v1\ntransport stop snap=480\n",
    ] {
        assert!(parse_script(bad).is_err(), "refused: {bad:?}");
    }
}

/// **A stray operand is a typo, not something to ignore.** Found the hard way: a
/// test typed `record bad id!` and the parser silently started a take called `bad`
/// — and opened the real input device. Every fixed-shape command is strict now.
#[test]
fn a_stray_operand_is_a_parse_error() {
    for line in [
        "pool /data/takes extra",
        "record jam extra",
        "save /tmp/x extra",
        "load /tmp/x extra",
        "session_rate 48000 extra",
        "unmount tone extra",
        "bounce 1000 /tmp/x.wav extra",
        "patch euclidean.triggers scale.trigger extra",
        "set_param mixer ch0.gain 0.5 extra",
        "set_tempo 120 4 extra",
        "play /tmp/a.wav ch0 extra",
        "splice 100 /tmp/a.wav 64 extra",
        "undo extra",
        "redo extra",
        "transport play extra",
        "transport stop extra",
        "transport seek 4800 extra",
    ] {
        let script = format!("host v1\n{line}\n");
        assert!(
            parse_script(&script).is_err(),
            "a stray word must be refused: {line}"
        );
    }

    // …and the strict forms themselves still parse.
    let script = "host v1\npool /data/takes\nrecord jam\nrecord stop\nsession_rate 48000\ntransport play\ntransport seek 4800\ntransport stop\nset_param mixer ch0.gain 0.5\nset_tempo 120 4\npatch euclidean.triggers scale.trigger\nunmount tone\nbounce 1000 /tmp/x.wav\nundo\nredo\n";
    assert!(
        parse_script(script).is_ok(),
        "the strict forms must still parse: {:?}",
        parse_script(script).err()
    );
}
