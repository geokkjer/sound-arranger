//! P1.3.2b — the clip editor's ops are LOGGED commands carrying `at_frame`, and
//! replay reconstructs the identical `Timeline` value byte-for-byte (the
//! log-visibility carve-out: the arrangement node's state is the logged value).

use engine::Engine;
use media::{ArrangeOp, Clip, ClipEditor, Timeline};

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

/// A representative op sequence that builds and edits a two-track arrangement.
fn ops() -> Vec<ArrangeOp> {
    vec![
        ArrangeOp::AddTrack { track: "t0".into() },
        ArrangeOp::AddTrack { track: "t1".into() },
        ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 1000, 4000) },
        ArrangeOp::RazorSplit { track: "t0".into(), clip: "c0".into(), new_left: "cL".into(), new_right: "cR".into(), at_frame: 3000 },
        ArrangeOp::MoveClip { track: "t0".into(), clip: "cR".into(), at_frame: 9000 },
        ArrangeOp::AddClip { track: "t1".into(), clip: clip("c1", 0, 500) },
        ArrangeOp::LoopRegion { track: "t1".into(), clip: "c1".into(), times: 2 },
        ArrangeOp::SetClipFade { track: "t0".into(), clip: "cL".into(), fade_in: 64, fade_out: 128 },
        ArrangeOp::Duplicate { track: "t1".into(), clip: "c1".into(), new_id: "c1b".into() },
    ]
}

fn apply_and_flush(e: &mut Engine, editor: &mut ClipEditor, os: &[ArrangeOp]) {
    for op in os {
        editor.apply(e, op).unwrap(); // apply validates, logs, and flushes the op
    }
}

#[test]
fn logged_ops_reconstruct_the_timeline_on_replay() {
    // live session: use the engine to apply the ops (logged commands) and flush.
    let mut a = Engine::new(48_000, 120.0, 4);
    let mut ea = ClipEditor::new();
    ea.register(&mut a).unwrap();
    apply_and_flush(&mut a, &mut ea, &ops());

    let timeline_a: Timeline = ea.snapshot().unwrap();
    // sanity: the live value has both tracks and the split clips.
    assert_eq!(timeline_a.tracks.len(), 2);
    assert_eq!(timeline_a.tracks[0].clips.len(), 2);

    // replay: a fresh engine + editor, same log, applied the same way.
    let mut b = Engine::new(48_000, 120.0, 4);
    let eb = ClipEditor::new();
    eb.register(&mut b).unwrap();
    b.replay_from(&a.log).unwrap();
    b.flush_scheduled();

    let timeline_b: Timeline = eb.snapshot().unwrap();
    assert_eq!(timeline_a, timeline_b, "replay must reconstruct the identical Timeline value");
    assert!(!timeline_b.tracks.is_empty());
}

#[test]
fn refused_op_is_never_logged() {
    // a semantically invalid op (zero-length clip) is refused by the dry-run in
    // `ClipEditor::apply` — fail-loud, and it never reaches the log.
    let mut a = Engine::new(48_000, 120.0, 4);
    let mut ea = ClipEditor::new();
    ea.register(&mut a).unwrap();

    let mut bad = clip("c0", 0, 0);
    bad.src_len = 0;
    let res = ea.apply(&mut a, &ArrangeOp::AddClip { track: "t0".into(), clip: bad });
    assert!(res.is_err(), "a zero-length clip must be refused");
    assert!(a.log.events().iter().all(|ev| !matches!(ev, engine::Event::Arrangement { .. })));
    assert!(ea.snapshot().unwrap().tracks.is_empty());
}
