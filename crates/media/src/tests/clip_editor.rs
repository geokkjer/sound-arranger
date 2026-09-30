use super::*;

fn clip(id: &str, at: u64, len: u64) -> Clip {
    Clip {
        reversed: false,
        id: id.into(),
        name: None,
        source: "pool-1".into(),
        src_start: 0,
        src_len: len,
        at_frame: at,
        fade_in: 4,
        fade_out: 8,
        gain: 0.5,
        loop_len: None,
    }
}

/// Snapshot an op via encode (with a fresh interner) and assert decoding
/// reconstructs the exact same op — the logged-command round-trip.
fn roundtrip(op: &ArrangeOp) {
    let mut i = Interner::new();
    let (name, fields) = encode_op(&mut i, op);
    // every op name must be in the registration list (W1): a variant that
    // encodes but isn't registered would be refused by `engine.arrange`.
    assert!(
        ALL_OPS.contains(&name),
        "op name '{name}' must be registered"
    );
    let back = decode_op(name, &fields).unwrap();
    assert_eq!(back, *op, "round-trip must be exact for {name}");
}

#[test]
fn every_op_round_trips_exactly() {
    roundtrip(&ArrangeOp::AddTrack { track: "t0".into() });
    roundtrip(&ArrangeOp::RemoveTrack { track: "t1".into() });
    roundtrip(&ArrangeOp::AddClip {
        track: "t0".into(),
        clip: clip("c0", 100, 400),
    });
    // A **named** clip round-trips (`Some(name)`), while an unnamed one stays `None`:
    // the codec's `""` means "no name", the same convention `loop_len`'s `0` uses.
    roundtrip(&ArrangeOp::AddClip {
        track: "t0".into(),
        clip: Clip {
            name: Some("take-2".into()),
            ..clip("c0", 100, 400)
        },
    });
    // The direction is part of the clip value, so the op carries it: a reversed
    // clip must survive encode→decode exactly (the gate found it decoding forward).
    roundtrip(&ArrangeOp::AddClip {
        track: "t0".into(),
        clip: Clip {
            reversed: true,
            ..clip("c0", 100, 400)
        },
    });
    roundtrip(&ArrangeOp::RazorSplit {
        track: "t0".into(),
        clip: "c0".into(),
        new_left: "l".into(),
        new_right: "r".into(),
        at_frame: 250,
    });
    roundtrip(&ArrangeOp::Trim {
        track: "t0".into(),
        clip: "c0".into(),
        edge: Edge::Start,
        by_frames: 25,
    });
    roundtrip(&ArrangeOp::MoveClip {
        track: "t0".into(),
        clip: "c0".into(),
        at_frame: 900,
    });
    roundtrip(&ArrangeOp::MoveClipToTrack {
        from: "t0".into(),
        clip: "c0".into(),
        to: "t1".into(),
        at_frame: 50,
    });
    roundtrip(&ArrangeOp::Duplicate {
        track: "t0".into(),
        clip: "c0".into(),
        new_id: "c1".into(),
    });
    roundtrip(&ArrangeOp::Delete {
        track: "t0".into(),
        clip: "c0".into(),
    });
    roundtrip(&ArrangeOp::SetClipGain {
        track: "t0".into(),
        clip: "c0".into(),
        gain: 0.75,
    });
    roundtrip(&ArrangeOp::SetClipFade {
        track: "t0".into(),
        clip: "c0".into(),
        fade_in: 16,
        fade_out: 0,
    });
    roundtrip(&ArrangeOp::LoopRegion {
        track: "t0".into(),
        clip: "c0".into(),
        times: 3,
    });
    roundtrip(&ArrangeOp::ChopClip {
        track: "t0".into(),
        clip: "c0".into(),
        times: 4,
        prefix: "slice".into(),
    });
    roundtrip(&ArrangeOp::Reverse {
        track: "t0".into(),
        clip: "c0".into(),
    });
    roundtrip(&ArrangeOp::RenameClip {
        track: "t0".into(),
        clip: "c0".into(),
        name: "bridge".into(),
    });
    roundtrip(&ArrangeOp::SetMarker {
        at_frame: 4_800,
        name: "verse".into(),
    });
    roundtrip(&ArrangeOp::RemoveMarker { at_frame: 4_800 });
    roundtrip(&ArrangeOp::Stretch {
        track: "t0".into(),
        clip: "c0".into(),
        source: "c0.stretch.3_2".into(),
        src_len: 6_000,
        num: 3,
        den: 2,
    });
}

#[test]
fn loop_len_encodes_zero_as_none() {
    let mut i = Interner::new();
    // loop_len None encodes as ("loop", U64(0)); Some(100) as U64(100).
    let mut c = clip("c0", 0, 1000);
    let (_, fields) = encode_op(
        &mut i,
        &ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c.clone(),
        },
    );
    assert_eq!(u64_field(&fields, "loop").unwrap(), 0);
    c.loop_len = Some(100);
    let (_, fields) = encode_op(
        &mut i,
        &ArrangeOp::AddClip {
            track: "t0".into(),
            clip: c,
        },
    );
    assert_eq!(u64_field(&fields, "loop").unwrap(), 100);
}

#[test]
fn decode_rejects_wrong_field_types() {
    // a Str where the op expects a U64 must fail loud, not silently coerce.
    let fields = vec![
        ("track", Value::Str("t0")),
        ("clip", Value::Str("c0")),
        ("at_frame", Value::Str("not-a-number")),
    ];
    assert!(decode_op("MoveClip", &fields).is_err());
    assert!(decode_op("NopeOp", &[]).is_err());
}
