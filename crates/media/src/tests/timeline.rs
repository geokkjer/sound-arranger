use super::*;

fn clip(id: &str, at: Frame, len: Frame) -> Clip {
    Clip {
        reversed: false,
        id: id.into(),
        name: None,
        source: "pool-1".into(),
        src_start: 0,
        src_len: len,
        at_frame: at,
        fade_in: 0,
        fade_out: 0,
        gain: 1.0,
        loop_len: None,
    }
}

fn two_tracks() -> Timeline {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t1".into() })
        .unwrap();
    t
}

/// Every clip in the value satisfies the model's invariants. This is the property
/// that makes an accepted op safe to *render*: `ArrangerNode::new` validates every
/// clip on a track and refuses the whole track over one of them, so a single clip an
/// op left invalid makes the session unplayable (and every later edit fail with it).
fn assert_all_clips_valid(t: &Timeline) {
    for c in t.tracks.iter().flat_map(|tr| tr.clips.iter()) {
        let verdict = validate_clip(c);
        assert!(verdict.is_ok(), "clip '{}' is invalid: {verdict:?}", c.id);
    }
}

#[test]
fn add_and_list_tracks() {
    let t = two_tracks();
    assert_eq!(t.tracks.len(), 2);
    assert_eq!(t.tracks[0].id, "t0");
    assert!(
        t.apply(&ArrangeOp::AddTrack { track: "t0".into() })
            .is_err()
    );
}

/// **Reversed is a clip property with a mirrored reader.** The clip's first
/// frame is the region's *top*, so `source_frame_at` walks down — and the ops
/// that compute source offsets (split, trim, chop) must mirror their arithmetic
/// with it. This test pins every one of those, because getting one wrong is
/// silent audio corruption (the right length, the wrong samples).
#[test]
fn a_reversed_clip_reads_backwards_and_the_ops_mirror() {
    let mut t = two_tracks();
    let mut c = clip("c0", 0, 1_000);
    c.src_start = 200;
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c.clone(),
        })
        .unwrap();

    // Forward: offset 0 is the region's bottom.
    assert_eq!(c.source_frame_at(0), 200);
    assert_eq!(c.source_frame_at(999), 1_199);

    // The op is a **toggle**, so two presses restore the clip exactly…
    let mut rev = t
        .apply(&ArrangeOp::Reverse {
            track: "t0".into(),
            clip: "c0".into(),
        })
        .unwrap();
    assert!(rev.tracks[0].clips[0].reversed);
    let back = rev
        .apply(&ArrangeOp::Reverse {
            track: "t0".into(),
            clip: "c0".into(),
        })
        .unwrap();
    assert!(!back.tracks[0].clips[0].reversed);
    rev = back
        .apply(&ArrangeOp::Reverse {
            track: "t0".into(),
            clip: "c0".into(),
        })
        .unwrap();

    // Reversed: offset 0 is the region's top.
    let r = &rev.tracks[0].clips[0];
    assert_eq!(r.source_frame_at(0), 1_199);
    assert_eq!(r.source_frame_at(999), 200);

    // **Split**: in time, the left half is the *top* of the region.
    let split = rev
        .apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "l".into(),
            new_right: "rr".into(),
            at_frame: 400,
        })
        .unwrap();
    let left = split.tracks[0]
        .clips
        .iter()
        .find(|c| c.id == "l")
        .expect("left");
    let right = split.tracks[0]
        .clips
        .iter()
        .find(|c| c.id == "rr")
        .expect("right");
    assert_eq!(
        (left.src_start, left.src_len),
        (800, 400),
        "left is the top"
    );
    assert_eq!((right.src_start, right.src_len), (200, 600));
    assert_eq!(left.source_frame_at(0), 1_199, "and reads down from there");
    assert_eq!(right.source_frame_at(0), 799);

    // **Trim the start**: the clip's first frames go, the region's top shrinks —
    // `src_start` does not move (the mirror of the forward start trim).
    let start = rev
        .apply(&ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: Edge::Start,
            by_frames: 100,
        })
        .unwrap();
    let s = &start.tracks[0].clips[0];
    assert_eq!((s.at_frame, s.src_start, s.src_len), (100, 200, 900));
    assert_eq!(
        s.source_frame_at(0),
        1_099,
        "the new first frame is the old offset 100's sample"
    );

    // **Trim the end**: moving it earlier cuts the region's *bottom*, so
    // `src_start` rises (the sign flips against the forward case).
    let end = rev
        .apply(&ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: Edge::End,
            by_frames: -100,
        })
        .unwrap();
    let e = end.tracks[0].clips[0].clone();
    assert_eq!(
        (e.at_frame, e.src_start, e.src_len),
        (0, 300, 900),
        "the region's bottom rose"
    );
    assert_eq!(
        e.source_frame_at(e.src_len - 1),
        300,
        "down to the new bottom"
    );

    // …and extending the end reaches *below* `src_start`.
    let grew = rev
        .apply(&ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: Edge::End,
            by_frames: 100,
        })
        .unwrap();
    let g = grew.tracks[0].clips[0].clone();
    assert_eq!((g.at_frame, g.src_start, g.src_len), (0, 100, 1_100));
    assert_eq!(g.source_frame_at(g.src_len - 1), 100);

    // **Chop**: the pieces walk *down* from the top.
    let chopped = rev
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4,
            prefix: "pre".into(),
        })
        .unwrap();
    let pieces: Vec<(u64, u64)> = chopped.tracks[0]
        .clips
        .iter()
        .map(|c| (c.src_start, c.src_len))
        .collect();
    assert_eq!(
        pieces,
        vec![(950, 250), (700, 250), (450, 250), (200, 250)],
        "piece 0 in time is the top of the region"
    );
    assert!(chopped.tracks[0].clips.iter().all(|c| c.reversed));

    // A looped clip cannot be reversed, and a reversed clip cannot be looped:
    // the loop phase of a mirrored read is not representable (the same reason
    // split/trim/chop refuse a looped clip).
    let mut looped = two_tracks();
    let mut lc = clip("c0", 0, 1_000);
    lc.loop_len = Some(500);
    looped = looped
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: lc,
        })
        .unwrap();
    assert!(
        looped
            .apply(&ArrangeOp::Reverse {
                track: "t0".into(),
                clip: "c0".into(),
            })
            .is_err(),
        "reversing a looped clip is refused"
    );
    assert!(
        rev.apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 2,
        })
        .is_err(),
        "looping a reversed clip is refused"
    );
}

/// **A stretch points the clip at new material** and leaves the one-frame-domain
/// rule intact: the clip still reads a straight region (from `src_start` 0) of an
/// immutable source, and the ratio it was rendered at travels with the op as a
/// rational. Fades are capped to the new length, and the cases it cannot represent
/// (a looped clip, a zero ratio or length) are refused.
#[test]
fn a_stretch_rewrites_the_reference_and_caps_fades() {
    let mut t = two_tracks();
    let mut c = clip("c0", 0, 4_000);
    c.fade_in = 1_000;
    c.fade_out = 3_000; // exactly the clip's length: legal, and a stretch shrinks it
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c.clone(),
        })
        .unwrap();

    let stretched = t
        .apply(&ArrangeOp::Stretch {
            track: "t0".into(),
            clip: "c0".into(),
            source: "c0.stretch.1_2".into(),
            src_len: 2_000,
            num: 1,
            den: 2,
        })
        .unwrap();
    let n = &stretched.tracks[0].clips[0];
    assert_eq!(n.source, "c0.stretch.1_2", "the clip points at the render");
    assert_eq!((n.src_start, n.src_len), (0, 2_000));
    assert_eq!(
        (n.at_frame, n.gain),
        (c.at_frame, c.gain),
        "place and gain stay"
    );
    assert_eq!(
        (n.fade_in, n.fade_out),
        (1_000, 1_000),
        "fades capped to fit"
    );
    assert_eq!(stretched.tracks[0].clips[0].source_frame_at(0), 0);

    // The refusals: a zero ratio or length, and a looped clip (whose loop phase a
    // stretched read cannot represent).
    for (source, src_len, num, den) in [
        ("s", 2_000u64, 0u32, 2u32),
        ("s", 2_000, 1, 0),
        ("s", 0, 1, 2),
    ] {
        assert!(
            stretched
                .apply(&ArrangeOp::Stretch {
                    track: "t0".into(),
                    clip: "c0".into(),
                    source: source.into(),
                    src_len,
                    num,
                    den,
                })
                .is_err(),
            "stretch {num}/{den} len {src_len} must be refused"
        );
    }
    let mut looped = two_tracks();
    let mut lc = clip("c0", 0, 4_000);
    lc.loop_len = Some(1_000);
    looped = looped
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: lc,
        })
        .unwrap();
    assert!(
        looped
            .apply(&ArrangeOp::Stretch {
                track: "t0".into(),
                clip: "c0".into(),
                source: "s".into(),
                src_len: 2_000,
                num: 1,
                den: 2,
            })
            .is_err(),
        "a looped clip cannot be stretched"
    );
}

/// **Markers are named points, and the vocabulary says exactly that.** Adding,
/// renaming and removing are logged ops on the arrangement value: they replay, they
/// save, and `undo` means what it says. A marker never changes the audio — the
/// render path does not read them — and `end_frame` stays clip-based, so a marker
/// past the last clip does not make an export render silence.
#[test]
fn markers_are_set_renamed_sorted_and_removed() {
    let t = two_tracks();
    assert!(t.markers.is_empty());

    // Set at 4 800, then at 0 (out of order) and at 96 000: the list stays sorted.
    let mut t2 = t.clone();
    for (frame, name) in [(4_800u64, "verse"), (0, "intro"), (96_000, "outro")] {
        t2 = t2
            .apply(&ArrangeOp::SetMarker {
                at_frame: frame,
                name: name.into(),
            })
            .unwrap();
    }
    let names: Vec<(u64, String)> = t2
        .markers
        .iter()
        .map(|m| (m.at_frame, m.name.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            (0, "intro".to_string()),
            (4_800, "verse".to_string()),
            (96_000, "outro".to_string())
        ],
        "sorted by frame"
    );

    // Setting the same frame again **renames** it — at most one marker per frame is
    // what makes `set_marker` unable to fail on a duplicate.
    let t3 = t2
        .apply(&ArrangeOp::SetMarker {
            at_frame: 4_800,
            name: "chorus".into(),
        })
        .unwrap();
    assert_eq!(t3.markers.len(), 3);
    assert_eq!(t3.marker_at(4_800).map(|m| m.name.as_str()), Some("chorus"));

    // Navigation is strict on both sides, so repeated `next`/`prev` walk the list.
    assert_eq!(t3.marker_after(0).map(|m| m.name.as_str()), Some("chorus"));
    assert_eq!(
        t3.marker_after(4_800).map(|m| m.name.as_str()),
        Some("outro")
    );
    assert_eq!(t3.marker_after(96_000), None);
    assert_eq!(
        t3.marker_before(4_800).map(|m| m.name.as_str()),
        Some("intro")
    );
    assert_eq!(t3.marker_before(0), None);

    // Removing takes it out; removing something that is not there is refused (a
    // deletion that deletes nothing is not an edit).
    let t4 = t3
        .apply(&ArrangeOp::RemoveMarker { at_frame: 4_800 })
        .unwrap();
    assert_eq!(t4.markers.len(), 2);
    assert!(t4.marker_at(4_800).is_none());
    assert!(
        t4.apply(&ArrangeOp::RemoveMarker { at_frame: 4_800 })
            .is_err()
    );

    // A marker must be a name the log can spell.
    for bad in ["", "two words", "@5", "snap=3", "#nope"] {
        assert!(
            t3.apply(&ArrangeOp::SetMarker {
                at_frame: 1_000,
                name: bad.into(),
            })
            .is_err(),
            "'{bad}' must be refused"
        );
    }
    // And markers do not extend the arrangement.
    assert_eq!(
        t3.end_frame()
            .expect("an admitted value has a representable end"),
        t.end_frame()
            .expect("an admitted value has a representable end"),
        "a marker past the last clip is not rendered silence"
    );
}

/// **A clip name is a label, not an address**: it can be set and cleared, it is
/// carried by a chop, and every op keeps keying on the id.
#[test]
fn a_clip_can_be_named_and_unnamed() {
    let mut t = two_tracks();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 4_000),
        })
        .unwrap();

    t = t
        .apply(&ArrangeOp::RenameClip {
            track: "t0".into(),
            clip: "c0".into(),
            name: "bridge-take-2".into(),
        })
        .unwrap();
    assert_eq!(
        t.clip("c0").map(|(_, c)| c.name.as_deref()),
        Some(Some("bridge-take-2")),
        "the name is the label a shell shows"
    );

    // A name the log could not spell is refused (the format is space-separated):
    // a session that saves a name it cannot reopen is worse than one that says no.
    for bad in ["two words", "@take", "snap=2", "#take", ""] {
        let r = t.apply(&ArrangeOp::RenameClip {
            track: "t0".into(),
            clip: "c0".into(),
            name: bad.into(),
        });
        // An empty name *clears* the label; the rest are refused.
        if bad.is_empty() {
            assert!(r.is_ok(), "an empty name clears the label");
        } else {
            assert!(r.is_err(), "'{bad}' must be refused");
        }
    }

    // An empty name clears it.
    let cleared = t
        .apply(&ArrangeOp::RenameClip {
            track: "t0".into(),
            clip: "c0".into(),
            name: String::new(),
        })
        .unwrap();
    assert_eq!(cleared.tracks[0].clips[0].name, None);

    // A chop carries the label onto its pieces (a named take stays recognisable).
    let chopped = t
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 2,
            prefix: "c0".into(),
        })
        .unwrap();
    assert!(
        chopped.tracks[0].clips.iter().all(|c| c.name.is_some()),
        "both pieces keep the name"
    );

    // Naming a clip that is not there says which one.
    let e = t
        .apply(&ArrangeOp::RenameClip {
            track: "t0".into(),
            clip: "nope".into(),
            name: "x".into(),
        })
        .unwrap_err();
    assert!(e.contains("nope"), "{e}");
}

/// Renaming keeps a track's **position** (so the mixer channel it feeds does
/// not move) and refuses a name the text format could not carry back; moving a
/// track carries its clips and shifts the others.
#[test]
fn tracks_rename_and_move_with_their_audio() {
    let mut t = two_tracks();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 480),
        })
        .unwrap();
    // `AddTrack` validates the same way, so a track can never be *created* with
    // a name it could not be renamed to (the library-level hole the gate found).
    for bad in ["", "two words", "@48000", "snap=480", "a#b"] {
        assert!(
            t.apply(&ArrangeOp::AddTrack { track: bad.into() }).is_err(),
            "add_track {bad:?} must be refused"
        );
    }

    t = t
        .apply(&ArrangeOp::RenameTrack {
            track: "t0".into(),
            to: "lead".into(),
        })
        .unwrap();
    assert_eq!(
        t.tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        vec!["lead", "t1"],
        "a rename keeps the position"
    );
    assert_eq!(t.tracks[0].clips.len(), 1, "and its clips");

    // Refusals: the old name is gone, a taken name is a collision, and an id
    // the `host v1` format cannot carry back is refused rather than written —
    // whitespace, emptiness, a comment marker, and the parser's own tokens.
    for (track, to) in [
        ("t0", "gone"),
        ("lead", "t1"),
        ("lead", "two words"),
        ("lead", ""),
        ("lead", "@48000"),
        ("lead", "snap=480"),
        ("lead", "lead#1"),
        ("nope", "fine"),
    ] {
        assert!(
            t.apply(&ArrangeOp::RenameTrack {
                track: track.into(),
                to: to.into(),
            })
            .is_err(),
            "rename {track} → {to:?} must be refused"
        );
    }

    // Move: index 1 swaps the pair, carrying the clip.
    let mut moved = t
        .apply(&ArrangeOp::MoveTrack {
            track: "lead".into(),
            index: 1,
        })
        .unwrap();
    assert_eq!(
        moved
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        vec!["t1", "lead"]
    );
    assert_eq!(moved.tracks[1].clips.len(), 1, "the clip moved with it");
    // Moving to its own index is a no-op, not an error; out of range and an
    // unknown track are refused.
    moved = moved
        .apply(&ArrangeOp::MoveTrack {
            track: "lead".into(),
            index: 1,
        })
        .unwrap();
    assert!(
        moved
            .apply(&ArrangeOp::MoveTrack {
                track: "lead".into(),
                index: 2,
            })
            .is_err()
    );
    assert!(
        moved
            .apply(&ArrangeOp::MoveTrack {
                track: "nope".into(),
                index: 0,
            })
            .is_err()
    );
}

#[test]
fn add_clip_places_and_keeps_sorted() {
    let mut t = two_tracks();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c2", 48000, 24000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c1", 0, 24000),
        })
        .unwrap();
    let ids: Vec<_> = t.tracks[0].clips.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["c1", "c2"]);
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "nope".into(),
            clip: clip("c3", 0, 1)
        })
        .is_err()
    );
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c1", 0, 1)
        })
        .is_err()
    );
}

#[test]
fn add_clip_validates_the_clip() {
    let t = two_tracks();
    // src_len 0
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 0)
        })
        .is_err()
    );
    // NaN gain
    let mut c = clip("c0", 0, 100);
    c.gain = f32::NAN;
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c
        })
        .is_err()
    );
    // loop_len Some(0)
    let mut c = clip("c0", 0, 100);
    c.loop_len = Some(0);
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c
        })
        .is_err()
    );
    // span overflow
    let c = clip("c0", u64::MAX - 10, 100);
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c
        })
        .is_err()
    );
    // fades exceed clip
    let mut c = clip("c0", 0, 100);
    c.fade_in = 60;
    c.fade_out = 60;
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c
        })
        .is_err()
    );
}

#[test]
fn razor_split_inside_produces_two_sorted_halves() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 1000, 4000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "cL".into(),
            new_right: "cR".into(),
            at_frame: 3000,
        })
        .unwrap();
    let clips = &t.tracks[0].clips;
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].id, "cL");
    assert_eq!(clips[0].at_frame, 1000);
    assert_eq!(clips[0].src_start, 0);
    assert_eq!(clips[0].src_len, 2000);
    assert_eq!(clips[1].id, "cR");
    assert_eq!(clips[1].at_frame, 3000);
    assert_eq!(clips[1].src_start, 2000);
    assert_eq!(clips[1].src_len, 2000);
    // boundary / invalid splits
    assert!(
        t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "cL".into(),
            new_left: "x".into(),
            new_right: "y".into(),
            at_frame: 1000,
        })
        .is_err()
    );
    // distinct split ids required
    assert!(
        t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "cL".into(),
            new_left: "z".into(),
            new_right: "z".into(),
            at_frame: 1500,
        })
        .is_err()
    );
}

#[test]
fn razor_split_keeps_sorted_with_overlapping_neighbor() {
    // a neighbor starts inside the split clip's span; the split must re-sort.
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("big", 0, 10000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("mid", 5000, 100),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "big".into(),
            new_left: "A".into(),
            new_right: "B".into(),
            at_frame: 8000,
        })
        .unwrap();
    let ids: Vec<_> = t.tracks[0].clips.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["A", "mid", "B"]);
    let frames: Vec<_> = t.tracks[0].clips.iter().map(|c| c.at_frame).collect();
    assert!(
        frames.windows(2).all(|w| w[0] <= w[1]),
        "clips must stay sorted: {frames:?}"
    );
}

#[test]
fn razor_split_refuses_a_looped_clip() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 1000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 2,
        })
        .unwrap();
    assert!(
        t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "L".into(),
            new_right: "R".into(),
            at_frame: 500,
        })
        .is_err()
    );
    assert!(
        t.apply(&ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: Edge::Start,
            by_frames: 100
        })
        .is_err()
    );
}

#[test]
fn trim_moves_at_frame_with_src_start_and_resorts() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 1000, 4000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c1", 500, 100),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: Edge::Start,
            by_frames: 500,
        })
        .unwrap();
    let c = t.tracks[0].clips.iter().find(|c| c.id == "c0").unwrap();
    assert_eq!(c.at_frame, 1500);
    assert_eq!(c.src_start, 500);
    assert_eq!(c.src_len, 3500);
    // trim start to before frame 0 is refused
    assert!(
        t.apply(&ArrangeOp::Trim {
            track: "t0".into(),
            clip: "c0".into(),
            edge: Edge::Start,
            by_frames: -5000
        })
        .is_err()
    );
    // order kept sorted after the move
    let frames: Vec<_> = t.tracks[0].clips.iter().map(|c| c.at_frame).collect();
    assert!(
        frames.windows(2).all(|w| w[0] <= w[1]),
        "clips must stay sorted: {frames:?}"
    );
}

#[test]
fn move_and_cross_track_move() {
    let mut t = two_tracks();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 100, 100),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c1", 200, 100),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::MoveClip {
            track: "t0".into(),
            clip: "c0".into(),
            at_frame: 900,
        })
        .unwrap();
    assert_eq!(
        t.tracks[0]
            .clips
            .iter()
            .find(|c| c.id == "c0")
            .unwrap()
            .at_frame,
        900
    );
    // move within the same track re-sorts
    let frames: Vec<_> = t.tracks[0].clips.iter().map(|c| c.at_frame).collect();
    assert!(
        frames.windows(2).all(|w| w[0] <= w[1]),
        "clips must stay sorted: {frames:?}"
    );
    t = t
        .apply(&ArrangeOp::MoveClipToTrack {
            from: "t0".into(),
            clip: "c0".into(),
            to: "t1".into(),
            at_frame: 50,
        })
        .unwrap();
    assert!(t.tracks[0].clips.iter().all(|c| c.id != "c0"));
    assert_eq!(
        t.tracks[1]
            .clips
            .iter()
            .find(|c| c.id == "c0")
            .unwrap()
            .at_frame,
        50
    );
    // cross-track move with same source+to actually moves (from != to)
    assert_eq!(t.tracks[1].clips.len(), 1);
}

#[test]
fn duplicate_and_delete() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 1000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::Duplicate {
            track: "t0".into(),
            clip: "c0".into(),
            new_id: "c1".into(),
        })
        .unwrap();
    assert_eq!(t.tracks[0].clips.len(), 2);
    assert!(
        t.apply(&ArrangeOp::Duplicate {
            track: "t0".into(),
            clip: "c0".into(),
            new_id: "c1".into()
        })
        .is_err()
    );
    // duplicate with same id is refused
    assert!(
        t.apply(&ArrangeOp::Duplicate {
            track: "t0".into(),
            clip: "c0".into(),
            new_id: "c0".into()
        })
        .is_err()
    );
    t = t
        .apply(&ArrangeOp::Delete {
            track: "t0".into(),
            clip: "c1".into(),
        })
        .unwrap();
    assert_eq!(t.tracks[0].clips.len(), 1);
}

#[test]
fn loop_region_bakes_repeats_with_wrap_len() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 1000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 3,
        })
        .unwrap();
    let c = &t.tracks[0].clips[0];
    assert_eq!(c.loop_len, Some(1000));
    assert_eq!(c.src_len, 3000);
    assert_eq!(c.source_frame_at(0), 0);
    assert_eq!(c.source_frame_at(999), 999);
    assert_eq!(c.source_frame_at(1000), 0);
    assert_eq!(c.source_frame_at(2500), 500);

    // Growing `src_len` can push the clip's timeline span out of range, and that is
    // refused rather than admitted: an op the log accepts must not leave a clip the
    // renderer refuses, or the whole track stops building readers.
    let mut t = two_tracks();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", u64::MAX - 100, 100),
        })
        .unwrap();
    assert!(
        t.apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 2,
        })
        .is_err(),
        "a loop that would push the clip's span out of range is refused"
    );
}

#[test]
fn gain_and_fade_validate() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 100),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::SetClipGain {
            track: "t0".into(),
            clip: "c0".into(),
            gain: 0.5,
        })
        .unwrap();
    assert_eq!(t.tracks[0].clips[0].gain, 0.5);
    assert!(
        t.apply(&ArrangeOp::SetClipGain {
            track: "t0".into(),
            clip: "c0".into(),
            gain: f32::NAN
        })
        .is_err()
    );
    t = t
        .apply(&ArrangeOp::SetClipFade {
            track: "t0".into(),
            clip: "c0".into(),
            fade_in: 10,
            fade_out: 10,
        })
        .unwrap();
    assert_eq!(t.tracks[0].clips[0].fade_in, 10);
    // fades exceeding the clip length are refused
    assert!(
        t.apply(&ArrangeOp::SetClipFade {
            track: "t0".into(),
            clip: "c0".into(),
            fade_in: 80,
            fade_out: 80
        })
        .is_err()
    );
}

/// **A fade pair whose sum overflows `u64` is refused, not wrapped.** Both fade
/// operands are raw `u64` on the `host v1` text path (`arrange set_clip_fade t0
/// c0 18446744073709551615 1`), so the sum can leave the range: it wraps to 0,
/// `0 > src_len` is false, and the pair passes the length check. In a debug
/// build the `+` itself panics *under the editor's timeline lock*; in a release
/// build the clip is accepted and rendered at gain 0 for every sample — silent
/// while the log, panel and gain all claim it is audible.
#[test]
fn a_fade_pair_whose_sum_overflows_is_refused_not_wrapped() {
    let t = two_tracks();
    let fading = |fade_in, fade_out| ArrangeOp::SetClipFade {
        track: "t0".into(),
        clip: "c0".into(),
        fade_in,
        fade_out,
    };
    let t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 100),
        })
        .unwrap();

    // `AddClip` runs the same rule through `validate_clip`; a wrapped sum must
    // not admit a clip the value model would refuse.
    let mut c = clip("c0", 0, 100);
    c.fade_in = u64::MAX;
    c.fade_out = 1;
    assert!(
        t.apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c
        })
        .is_err(),
        "an overflowing fade pair must not enter the value through AddClip"
    );

    // `SetClipFade`: the overflowing pair is refused...
    assert!(
        t.apply(&fading(u64::MAX, 1)).is_err(),
        "fade_in u64::MAX + fade_out 1 wraps to 0 — refuse, never admit"
    );
    // ...and so is the pair that only *reaches* the top of the range, which the
    // wrapped comparison would also have waved through.
    assert!(
        t.apply(&fading(u64::MAX, 0)).is_err(),
        "a fade pair above the clip length is refused however it was spelled"
    );
    // The boundary that is legal stays legal: `fade_in + fade_out == src_len`
    // is exactly what the model allows.
    assert!(t.apply(&fading(60, 40)).is_ok());
}

/// **A razor-split of a clip with a long fade leaves two clips the model accepts.**
/// The fade rule is a sum against `src_len`, and razor-split is an op that *shrinks*
/// `src_len` — it zeroed the two seam fades but kept the inherited ones on the halves
/// that did not have the seam. A full-length fade-in is one keypress away in the shell
/// (`f` with the playhead at the clip's end caps at `src_len - fade_out`), so
/// `f`-then-`x` produced a half with `fade_in = src_len_of_the_whole` on a clip a
/// fraction of that long. `ArrangerNode::new` validates every clip on a track and
/// refuses the *whole* track over one, so the split — legal, accepted and logged —
/// made the session unplayable, and the half could not even be trimmed back.
#[test]
fn a_razor_split_of_a_long_fade_leaves_two_valid_halves() {
    let mut t = two_tracks();
    // A full-length fade-in is legal (48000 + 0 == src_len) and one keypress away.
    let mut c = clip("c0", 0, 48_000);
    c.fade_in = 47_999;
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .unwrap();

    let split = t
        .apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "L".into(),
            new_right: "R".into(),
            at_frame: 12_000,
        })
        .expect("a split of a fading clip is not refused");
    assert_all_clips_valid(&split);

    let l = split.clip("L").expect("left half").1;
    let r = split.clip("R").expect("right half").1;
    assert_eq!(l.src_len, 12_000, "the left half is the short one");
    // Capped to the half, not kept whole: 47_999 does not fit 12_000 frames.
    assert_eq!(l.fade_in, 12_000, "the fade is capped to the half");
    assert_eq!(l.fade_out, 0, "the split seam stays hard");
    assert_eq!((r.fade_in, r.fade_out), (0, 0), "the seam is hard");
    assert_eq!(r.src_len, 36_000);

    // The mirror: a fade-*out* longer than the right half is capped there.
    let mut t = two_tracks();
    let mut c = clip("c0", 0, 48_000);
    c.fade_out = 47_000;
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .unwrap();
    let split = t
        .apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "L".into(),
            new_right: "R".into(),
            at_frame: 12_000,
        })
        .expect("a split of a fading-out clip is not refused");
    assert_all_clips_valid(&split);
    assert_eq!(
        split.clip("R").expect("right half").1.fade_out,
        36_000,
        "the fade-out is capped to the right half"
    );
}

/// **A chop of a clip with a long fade leaves pieces the model accepts.** Same hole
/// as the split, per piece: the first piece inherited `fade_in` and the last
/// `fade_out` from the whole clip, and each piece is only `src_len / times` frames.
#[test]
fn a_chop_of_a_long_fade_leaves_pieces_the_model_accepts() {
    let mut t = two_tracks();
    let mut c = clip("c0", 0, 100);
    c.fade_in = 50;
    c.fade_out = 50; // exactly the clip's length: legal, and each piece is 10 frames
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .unwrap();

    let chopped = t
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 10,
            prefix: "s".into(),
        })
        .expect("a chop of a fading clip is not refused");
    assert_all_clips_valid(&chopped);
    let pieces = &chopped.tracks[0].clips;
    assert_eq!(pieces.len(), 10);
    assert_eq!(
        (pieces[0].fade_in, pieces[0].fade_out),
        (10, 0),
        "the leading fade is capped to the first piece"
    );
    assert_eq!(
        (pieces[9].fade_in, pieces[9].fade_out),
        (0, 10),
        "the trailing fade is capped to the last piece"
    );
    assert!(
        pieces[1..pieces.len() - 1]
            .iter()
            .all(|p| (p.fade_in, p.fade_out) == (0, 0)),
        "the interior seams stay hard"
    );
}

/// **A split and a chop are refused rather than wrapping when they would push the
/// source window past the frame range.** `src_start` is an unbounded `u64` on the
/// `add_clip` path, and a reversed split/chop *adds* to it: `u64::MAX - 3999 + 4000`
/// wraps in debug (a panic inside the editor's lock) and silently to a wrong offset
/// in release — the piece reads from somewhere the user never asked for. The clip is
/// refused at `AddClip` now that the window is bounded, and the arithmetic in both
/// arms is `checked_add` so a hand-built clip (a deserialized snapshot) is refused
/// there too rather than wrapped.
#[test]
fn a_source_window_past_the_frame_range_is_refused_not_wrapped() {
    let t = two_tracks();
    let mut c = clip("c0", 0, 4_000);
    c.src_start = u64::MAX - 4_000; // the window ends on the last frame: legal
    let t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .expect("the last representable window is legal");
    let mut c = clip("c1", 0, 4_000);
    c.src_start = u64::MAX - 3_999; // one frame further: the end leaves the range
    let t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .expect_err("a source window that cannot be represented is refused");
    assert!(
        t.contains("source window"),
        "the refusal names the source window, got: {t}"
    );

    // The arms refuse too, when a clip reaches them without passing `AddClip` (a
    // value deserialized from a snapshot is a legal `&self` to `apply`).
    let mut hand = two_tracks();
    let mut c = clip("c0", 0, 4_000);
    c.src_start = u64::MAX - 10; // the window cannot be represented
    c.reversed = true; // the reversed arms add to `src_start`
    hand.tracks[0].clips.push(c);
    assert!(
        hand.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "L".into(),
            new_right: "R".into(),
            at_frame: 100,
        })
        .is_err(),
        "a reversed split that would wrap the source offset is refused"
    );
    assert!(
        hand.apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4,
            prefix: "p".into(),
        })
        .is_err(),
        "a reversed chop that would wrap the source offset is refused"
    );
}

/// **A clip's `source` is a pool id, not a path.** It is an arbitrary
/// whitespace-free token in the `host v1` log (`add_clip <track> <clip> <source>
/// …`) and a `Clip` is `Deserialize`, so a crafted value reaches the model — and
/// unfixed, the resolver joined it straight onto the pool directory, so
/// `../../elsewhere` read a WAV from outside the pool into a bounce or an export.
/// `AddClip` refuses it, and so does `Stretch` (the other op that sets a source),
/// so the bad value is never *logged*; `Pool::path_for` refuses it again on the
/// reading side.
#[test]
fn a_clip_source_that_is_not_a_pool_id_is_refused() {
    let t = two_tracks();
    for bad in [
        "../../elsewhere",
        "..",
        ".",
        "sub/s1",
        "back\\slash",
        "s1 ",
        " ",
    ] {
        let mut c = clip("c0", 0, 4_000);
        c.source = bad.into();
        let err = t
            .apply(&ArrangeOp::AddClip {
                track: "t0".into(),
                clip: c,
            })
            .expect_err("a source that is not a plain pool id is refused");
        assert!(
            err.contains("not a pool id"),
            "the refusal says what is wrong: {err}"
        );
    }
    assert!(
        t.tracks[0].clips.is_empty(),
        "a refused clip is not in the value (so it is not in the log)"
    );

    // A plain id is still a legal source.
    let t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 4_000),
        })
        .expect("a pool id places a clip");
    assert_eq!(t.tracks[0].clips[0].source, "pool-1");
    // …and `Stretch`, which repoints a clip at a rendered source, holds the same
    // line: a rendered id is derived from the old source, so a bad one only arrives
    // from a hand-built op.
    assert!(
        t.apply(&ArrangeOp::Stretch {
            track: "t0".into(),
            clip: "c0".into(),
            source: "../elsewhere".into(),
            src_len: 4_000,
            num: 2,
            den: 1,
        })
        .is_err(),
        "a stretch cannot point a clip outside the pool either"
    );
}

/// **The *forward* chop is checked the same way, and the arm validates the clip it
/// was handed.** The fix above made the *reversed* chop's seed a `checked_add` and
/// left the forward walk summing `src_at += slen` raw — the same panic and the same
/// silent wrap, one boolean (`reversed`) away from the fixture it added. The route is
/// a clip that reaches an op without passing `AddClip`: a hand-built `Track` is a
/// legal `&self`, and a `Timeline` is `Deserialize`. The arm now checks the clip
/// *and* the walk, and the refusal names the window rather than a piece that would
/// not fit.
#[test]
fn a_forward_chop_that_would_wrap_the_source_window_is_refused() {
    let mut hand = two_tracks();
    let mut c = clip("c0", 0, 4_000);
    c.src_start = u64::MAX - 10; // the window cannot be represented
    hand.tracks[0].clips.push(c);
    let err = hand
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4,
            prefix: "p".into(),
        })
        .expect_err("a forward chop of an unrepresentable window is refused");
    assert!(
        err.contains("source window"),
        "the refusal names the source window, got: {err}"
    );
    assert_eq!(
        hand.tracks[0].clips.len(),
        1,
        "a refused op leaves the value alone"
    );

    // A clip that is *admitted* can never make that sum overflow — the pieces tile
    // `[src_start, src_start + src_len)` — and the pin that says so is the last
    // piece's own `validate_clip`.
    let t = two_tracks();
    let mut c = clip("c0", 0, 4_000);
    c.src_start = u64::MAX - 4_000; // the window ends on the last frame: legal
    let t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .expect("the last representable window is legal");
    let chopped = t
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4,
            prefix: "p".into(),
        })
        .expect("a forward chop of a representable window applies");
    assert_all_clips_valid(&chopped);
    assert_eq!(
        chopped.tracks[0]
            .clips
            .iter()
            .map(|c| c.src_start + c.src_len)
            .max(),
        Some(u64::MAX),
        "the last piece's window ends where the original's did"
    );
}

/// **`times` is a count of passes, not a factor — and a re-bake that shortens a clip
/// caps its fades instead of being refused.** `LoopRegion` reads the region from
/// `loop_len`, so a re-bake re-reads *the same* region: `times: 3` twice is three
/// passes (a factor reading would make it nine, and every script that re-baked a
/// clip would grow it), and `times: 1` is the single pass the clip was before its
/// first bake. A shrink is a shrink, so the fade rule is the one every other shrink
/// follows (razor-split, chop, stretch): cap the fades to the length they now
/// cover. Without the cap the *newly added* `validate_clip` refused the op — a
/// shorter loop of a fading clip could not be made at all.
#[test]
fn a_re_baked_loop_sets_the_pass_count_and_caps_its_fades() {
    let mut t = two_tracks();
    // A 1000-frame region, baked three times, and then given a fade-in over the
    // *baked* 3000 frames: legal there (the sum is against `src_len`), and 3x longer
    // than the region it will have to fit when the loop is baked back down to one
    // pass.
    let c = clip("c0", 0, 1_000);
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 3,
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::SetClipFade {
            track: "t0".into(),
            clip: "c0".into(),
            fade_in: 3_000,
            fade_out: 0,
        })
        .expect("a fade over the baked length is legal");
    let c = &t.tracks[0].clips[0];
    assert_eq!(
        (c.loop_len, c.src_len),
        (Some(1_000), 3_000),
        "three passes"
    );

    // The same count again is the same value — `times` counts, so it does not
    // compound into nine passes.
    t = t
        .apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 3,
        })
        .unwrap();
    let c = &t.tracks[0].clips[0];
    assert_eq!(
        (c.loop_len, c.src_len),
        (Some(1_000), 3_000),
        "`times` is a count of passes, not a factor"
    );

    // Fewer passes shortens the clip to the region — and the fade that covered the
    // baked length is capped to what is left, not left to be refused.
    let shorter = t
        .apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 1,
        })
        .expect("a shorter loop is the clip's own length, not a refusal");
    assert_all_clips_valid(&shorter);
    let c = &shorter.tracks[0].clips[0];
    assert_eq!(
        (c.loop_len, c.src_len),
        (Some(1_000), 1_000),
        "one pass is the region the first bake fixed"
    );
    assert_eq!(c.fade_in, 1_000, "the fade is capped to the shorter clip");
    assert_eq!(
        c.source_frame_at(0),
        0,
        "and the region is unchanged, so the material is the region's"
    );

    // …and the region stays fixed: baking three passes again is three passes of the
    // *original* region, not three of what is left.
    let rebaked = shorter
        .apply(&ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 3,
        })
        .unwrap();
    let c = &rebaked.tracks[0].clips[0];
    assert_eq!(
        (c.loop_len, c.src_len),
        (Some(1_000), 3_000),
        "the first bake fixed the region for good"
    );

    // `times: 0` is not "un-loop": it is refused, and un-looping is a delete and a
    // re-add (no op in this model drops a loop).
    assert!(
        shorter
            .apply(&ArrangeOp::LoopRegion {
                track: "t0".into(),
                clip: "c0".into(),
                times: 0,
            })
            .is_err(),
        "`times: 0` is refused rather than read as one pass"
    );
}

/// **A span that cannot be represented has no end — it is checked, not wrapped.**
/// `Clip::end` is `pub` and its doc used to *assert* the span was validated on
/// admission, but a `Timeline` is `Deserialize` and a hand-built `Track` is a legal
/// `&self`: the unchecked `+` was a debug panic inside `end_frame()` — which every
/// `export` calls — and a *wrapped* end in release, i.e. a mix silently missing its
/// last clip. `end` is `Option`, `end_frame` is `Result` and names the clip, so the
/// export is refused instead.
#[test]
fn a_span_that_cannot_be_represented_has_no_end() {
    let mut hand = two_tracks();
    let mut c = clip("c0", 0, 1_000);
    c.at_frame = u64::MAX - 10; // `at_frame + src_len` leaves the range
    hand.tracks[0].clips.push(c);
    let c = &hand.tracks[0].clips[0];
    assert_eq!(c.end(), None, "an unrepresentable span has no end frame");
    let err = hand
        .end_frame()
        .expect_err("the arrangement's end is refused, not wrapped");
    assert!(
        err.contains("clip 'c0'") && err.contains("span overflows"),
        "the refusal names the clip and the span, got: {err}"
    );

    // An admitted clip is unaffected: the last representable span still ends on the
    // last frame, and an empty arrangement is still 0.
    let t = two_tracks();
    let mut c = clip("c0", 0, 1_000);
    c.at_frame = u64::MAX - 1_000;
    let t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        })
        .expect("the last representable span is legal");
    assert_eq!(t.end_frame().unwrap(), u64::MAX);
    assert_eq!(two_tracks().end_frame().unwrap(), 0);
}

#[test]
fn pure_apply_is_deterministic_and_replays() {
    let ops = [
        ArrangeOp::AddTrack { track: "t0".into() },
        ArrangeOp::AddTrack { track: "t1".into() },
        ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 1000),
        },
        ArrangeOp::LoopRegion {
            track: "t0".into(),
            clip: "c0".into(),
            times: 2,
        },
        ArrangeOp::MoveClipToTrack {
            from: "t0".into(),
            clip: "c0".into(),
            to: "t1".into(),
            at_frame: 500,
        },
        ArrangeOp::SetClipFade {
            track: "t1".into(),
            clip: "c0".into(),
            fade_in: 8,
            fade_out: 0,
        },
    ];
    let mut a = Timeline::new();
    let mut b = Timeline::new();
    for op in &ops {
        a = a.apply(op).unwrap();
        b = b.apply(op).unwrap();
    }
    assert_eq!(a, b);
}

#[test]
fn chop_splits_a_clip_into_contiguous_pieces() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 4000),
        })
        .unwrap();

    t = t
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4,
            prefix: "slice".into(),
        })
        .unwrap();
    let track = &t.tracks[0];
    assert_eq!(track.clips.len(), 4, "chop 4 produces four pieces");
    assert_eq!(track.clips[0].id, "slice.0");
    assert_eq!(track.clips[3].id, "slice.3");
    // contiguous equal (within 1 frame) coverage of the original [0, 4000) span.
    for (i, c) in track.clips.iter().enumerate() {
        assert_eq!(c.at_frame, (i as Frame) * 1000, "piece {i} start frame");
        assert_eq!(c.src_len, 1000, "piece {i} length");
        assert_eq!(c.src_start, (i as Frame) * 1000, "piece {i} source start");
        assert_eq!(c.gain, 1.0, "chop preserves the clip gain");
    }
    assert_eq!(
        track
            .clips
            .last()
            .unwrap()
            .end()
            .expect("a tiled piece has an end"),
        4000,
        "pieces tile the original span"
    );
}

#[test]
fn chop_refuses_bad_inputs_and_a_looped_clip() {
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 4000),
        })
        .unwrap();

    // times = 0
    assert!(
        t.apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 0,
            prefix: "p".into()
        })
        .is_err()
    );
    // more slices than frames
    assert!(
        t.apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4001,
            prefix: "p".into()
        })
        .is_err()
    );

    // a looped clip is not representable
    let mut t = Timeline::new();
    t = t
        .apply(&ArrangeOp::AddTrack { track: "t0".into() })
        .unwrap();
    let mut lc = clip("c0", 0, 4000);
    lc.loop_len = Some(1000);
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: lc,
        })
        .unwrap();
    assert!(
        t.apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 2,
            prefix: "p".into()
        })
        .is_err()
    );

    // an unknown track / clip is refused
    assert!(
        t.apply(&ArrangeOp::ChopClip {
            track: "nope".into(),
            clip: "c0".into(),
            times: 2,
            prefix: "p".into()
        })
        .is_err()
    );
}

/// **A chop is refused a slice count past its bound rather than sized for it.**
/// `times` is a `u32` straight off the wire (`arrange chop t0 c0 4294967295 pre @0`)
/// and the only other limit was `times <= src_len`, which reaches `i64::MAX`. So one
/// logged op could walk billions of pieces — each iteration a full scan of every
/// track's clip ids *and* a `Clip` with three heap `String`s, inside a `Timeline`
/// `apply` already cloned — and sort the result: a hang first, an allocation abort
/// second. The bound is a **refusal naming the limit**, and it is inclusive: the
/// count at the bound still applies, so nothing a hand makes is turned away.
#[test]
fn a_chop_past_the_slice_bound_is_refused_not_sized() {
    let mut t = two_tracks();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, MAX_CHOP_SLICES + 1),
        })
        .unwrap();

    // One past the bound, on a clip long enough to satisfy `times <= src_len`. This
    // is the assertion the fix turns: on the unbounded value the op *applies* and
    // builds 4097 pieces, so it also fails fast there rather than hanging.
    let err = t
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: (MAX_CHOP_SLICES + 1) as u32,
            prefix: "pre".into(),
        })
        .expect_err("a chop past the slice bound is refused");
    assert!(
        err.contains(&MAX_CHOP_SLICES.to_string()),
        "the refusal names the bound, got: {err}"
    );

    // The reported trigger: the wire's largest `u32`, on a clip long enough that
    // `times <= src_len` does not catch it. Refused before a piece is built.
    let mut wide = two_tracks();
    wide = wide
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, u32::MAX as Frame),
        })
        .unwrap();
    assert!(
        wide.apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: u32::MAX,
            prefix: "pre".into(),
        })
        .is_err(),
        "chop times = u32::MAX is refused"
    );

    // At the bound it still applies, and every piece is a clip the model accepts.
    let at_bound = t
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: MAX_CHOP_SLICES as u32,
            prefix: "pre".into(),
        })
        .expect("a chop at the bound applies");
    assert_eq!(
        at_bound.tracks[0].clips.len(),
        MAX_CHOP_SLICES as usize,
        "every slice at the bound is produced"
    );
    assert_all_clips_valid(&at_bound);
}

/// **The derived-id check spans every track, not the chopped one.** Clip ids are
/// unique across the whole arrangement (`Timeline::clip` resolves an id without
/// naming a track, and `Duplicate` checks the whole value), so a chop whose piece id
/// is already taken on *another* track is refused. The duplicate check is now a hash
/// set gathered before the walk — this is what keeps it from becoming the chopped
/// track's own ids, which would let two tracks hold the same clip id.
#[test]
fn a_chop_is_refused_a_derived_id_taken_on_another_track() {
    let mut t = two_tracks();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t0".into(),
            clip: clip("c0", 0, 4000),
        })
        .unwrap();
    t = t
        .apply(&ArrangeOp::AddClip {
            track: "t1".into(),
            clip: clip("pre.1", 0, 4000),
        })
        .unwrap();

    let err = t
        .apply(&ArrangeOp::ChopClip {
            track: "t0".into(),
            clip: "c0".into(),
            times: 4,
            prefix: "pre".into(),
        })
        .expect_err("a piece id already on another track is refused");
    assert_eq!(
        err, "chop derived id 'pre.1' already exists or repeats",
        "the refusal names the colliding piece"
    );
}
