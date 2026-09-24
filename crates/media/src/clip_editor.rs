//! The clip editor's engine seam (P1.3, `clip_editor`): the `ArrangeOp` ↔
//! `engine.arrange` codec and the logged-command reconstruction of the
//! [`Timeline`] value.
//!
//! An `ArrangeOp` (from [`crate::timeline`]) is a **value-level logged command**:
//! [`encode_op`] converts it to the engine's closed-core `op` + `fields`
//! (interned ids, canonical values), [`decode_op`] converts back. The engine logs
//! it as `Event::Arrangement` with `at_frame`; a handler the clip editor
//! registered decodes the fields and applies the op to the shared `Timeline`.
//! Replay re-applies the same ops and reconstructs the identical value — the
//! log-visibility carve-out: the arrangement node's *state* is the logged value.
//!
//! The interner is encode-side only (ids are `String` in the value, `&'static str`
//! in the log); decode maps `&'static str` back with `to_owned`. The design keeps
//! the std-only core closed (it never sees `ArrangeOp`) and is reviewed by kimi.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine::{Engine, OpHandler, Value};

use crate::timeline::{ArrangeOp, Clip, Edge, Timeline};

/// Maps interned ids to a stable `&'static str` for the log. Leaks each unique
/// string (spike scale — a serialized log would use a string table instead).
/// Deterministic: the same `str` always yields the same `&'static str`.
#[derive(Default)]
pub struct Interner {
    map: HashMap<String, &'static str>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> &'static str {
        if let Some(v) = self.map.get(s).copied() {
            return v;
        }
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        self.map.insert(s.to_string(), leaked);
        leaked
    }
}

fn v_u64(v: &Value) -> Option<u64> {
    // strict: only `U64` decodes as an unsigned (kimi should-fix S1) — a hand-edited
    // log carrying `I64`/`U32` where a `U64` is expected fails loud, not coerced.
    match v {
        Value::U64(n) => Some(*n),
        _ => None,
    }
}

fn v_f32(v: &Value) -> Option<f32> {
    match v {
        Value::F32(x) => Some(*x),
        _ => None,
    }
}

/// Encode an `ArrangeOp` into the engine's op name + fields (ids interned).
pub fn encode_op(i: &mut Interner, op: &ArrangeOp) -> (&'static str, Vec<(&'static str, Value)>) {
    match op {
        ArrangeOp::AddTrack { track } => ("AddTrack", vec![("track", Value::Str(i.intern(track)))]),
        ArrangeOp::RemoveTrack { track } => {
            ("RemoveTrack", vec![("track", Value::Str(i.intern(track)))])
        }
        ArrangeOp::RenameTrack { track, to } => (
            "RenameTrack",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("to", Value::Str(i.intern(to))),
            ],
        ),
        ArrangeOp::MoveTrack { track, index } => (
            "MoveTrack",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("index", Value::U64(*index as u64)),
            ],
        ),
        ArrangeOp::Stretch {
            track,
            clip,
            source,
            src_len,
            num,
            den,
        } => (
            "Stretch",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("source", Value::Str(i.intern(source))),
                ("src_len", Value::U64(*src_len)),
                ("num", Value::U32(*num)),
                ("den", Value::U32(*den)),
            ],
        ),
        ArrangeOp::Reverse { track, clip } => (
            "Reverse",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
            ],
        ),
        ArrangeOp::AddClip { track, clip } => {
            // loop_len must be > 0 (validate_clip enforces it); Some(0) would silently
            // encode as None. Guard here so a direct encode of a hand-built clip is
            // caught in debug (kimi should-fix S2).
            debug_assert!(
                clip.loop_len != Some(0),
                "clip '{}' loop_len must be > 0",
                clip.id
            );
            (
                "AddClip",
                vec![
                    ("track", Value::Str(i.intern(track))),
                    ("id", Value::Str(i.intern(&clip.id))),
                    ("source", Value::Str(i.intern(&clip.source))),
                    ("src_start", Value::U64(clip.src_start)),
                    ("src_len", Value::U64(clip.src_len)),
                    ("at_frame", Value::U64(clip.at_frame)),
                    ("fade_in", Value::U64(clip.fade_in)),
                    ("fade_out", Value::U64(clip.fade_out)),
                    ("gain", Value::F32(clip.gain)),
                    // loop_len must be > 0; 0 encodes "no loop".
                    ("loop", Value::U64(clip.loop_len.unwrap_or(0))),
                    // The direction is a clip property, so the op carries it: without
                    // it, an `AddClip` with `reversed: true` would encode fine and
                    // decode forward — a silent value change (the gate caught it).
                    ("reversed", Value::U64(u64::from(clip.reversed))),
                    // The clip's human name is a value field like the others: without it
                    // an `AddClip` of a named clip would replay unnamed.
                    (
                        "name",
                        Value::Str(i.intern(clip.name.as_deref().unwrap_or(""))),
                    ),
                ],
            )
        }
        ArrangeOp::RenameClip { track, clip, name } => (
            "RenameClip",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("name", Value::Str(i.intern(name))),
            ],
        ),
        ArrangeOp::SetMarker { at_frame, name } => (
            "SetMarker",
            vec![
                ("at_frame", Value::U64(*at_frame)),
                ("name", Value::Str(i.intern(name))),
            ],
        ),
        ArrangeOp::RemoveMarker { at_frame } => {
            ("RemoveMarker", vec![("at_frame", Value::U64(*at_frame))])
        }
        ArrangeOp::RazorSplit {
            track,
            clip,
            new_left,
            new_right,
            at_frame,
        } => (
            "RazorSplit",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("new_left", Value::Str(i.intern(new_left))),
                ("new_right", Value::Str(i.intern(new_right))),
                ("at_frame", Value::U64(*at_frame)),
            ],
        ),
        ArrangeOp::Trim {
            track,
            clip,
            edge,
            by_frames,
        } => (
            "Trim",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                (
                    "edge",
                    Value::Str(match edge {
                        Edge::Start => "start",
                        Edge::End => "end",
                    }),
                ),
                ("by", Value::I64(*by_frames)),
            ],
        ),
        ArrangeOp::MoveClip {
            track,
            clip,
            at_frame,
        } => (
            "MoveClip",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("at_frame", Value::U64(*at_frame)),
            ],
        ),
        ArrangeOp::MoveClipToTrack {
            from,
            clip,
            to,
            at_frame,
        } => (
            "MoveClipToTrack",
            vec![
                ("from", Value::Str(i.intern(from))),
                ("clip", Value::Str(i.intern(clip))),
                ("to", Value::Str(i.intern(to))),
                ("at_frame", Value::U64(*at_frame)),
            ],
        ),
        ArrangeOp::Duplicate {
            track,
            clip,
            new_id,
        } => (
            "Duplicate",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("new_id", Value::Str(i.intern(new_id))),
            ],
        ),
        ArrangeOp::Delete { track, clip } => (
            "Delete",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
            ],
        ),
        ArrangeOp::SetClipGain { track, clip, gain } => (
            "SetClipGain",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("gain", Value::F32(*gain)),
            ],
        ),
        ArrangeOp::SetClipFade {
            track,
            clip,
            fade_in,
            fade_out,
        } => (
            "SetClipFade",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("fade_in", Value::U64(*fade_in)),
                ("fade_out", Value::U64(*fade_out)),
            ],
        ),
        ArrangeOp::LoopRegion { track, clip, times } => (
            "LoopRegion",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("times", Value::U32(*times)),
            ],
        ),
        ArrangeOp::ChopClip {
            track,
            clip,
            times,
            prefix,
        } => (
            "ChopClip",
            vec![
                ("track", Value::Str(i.intern(track))),
                ("clip", Value::Str(i.intern(clip))),
                ("times", Value::U32(*times)),
                ("prefix", Value::Str(i.intern(prefix))),
            ],
        ),
    }
}

fn field<'a>(fields: &'a [(&'static str, Value)], name: &str) -> Result<&'a Value, String> {
    fields
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
        .ok_or_else(|| format!("missing field '{name}'"))
}

fn str_field(fields: &[(&'static str, Value)], name: &str) -> Result<String, String> {
    match field(fields, name)? {
        Value::Str(s) => Ok((*s).to_owned()),
        other => Err(format!("field '{name}' must be a Str, got {other:?}")),
    }
}

fn u64_field(fields: &[(&'static str, Value)], name: &str) -> Result<u64, String> {
    v_u64(field(fields, name)?).ok_or_else(|| format!("field '{name}' must be an integer"))
}

fn u32_field(fields: &[(&'static str, Value)], name: &str) -> Result<u32, String> {
    match field(fields, name)? {
        Value::U32(n) => Ok(*n),
        Value::U64(n) => {
            u32::try_from(*n).map_err(|_| format!("field '{name}' does not fit a u32"))
        }
        other => Err(format!("field '{name}' must be a U32, got {other:?}")),
    }
}

fn f32_field(fields: &[(&'static str, Value)], name: &str) -> Result<f32, String> {
    v_f32(field(fields, name)?).ok_or_else(|| format!("field '{name}' must be an F32"))
}

/// Decode engine op fields back into an `ArrangeOp`.
///
/// Strict on the fields it reads (missing / wrong-typed fields fail loud); an
/// **unknown extra field is ignored** (forward-compatibility — a newer op version
/// with an added field decodes on this decoder, dropping the extra data). A
/// duplicate field returns the first match (encode never emits duplicates).
pub fn decode_op(op: &str, fields: &[(&'static str, Value)]) -> Result<ArrangeOp, String> {
    match op {
        "AddTrack" => Ok(ArrangeOp::AddTrack {
            track: str_field(fields, "track")?,
        }),
        "RemoveTrack" => Ok(ArrangeOp::RemoveTrack {
            track: str_field(fields, "track")?,
        }),
        "RenameTrack" => Ok(ArrangeOp::RenameTrack {
            track: str_field(fields, "track")?,
            to: str_field(fields, "to")?,
        }),
        "MoveTrack" => Ok(ArrangeOp::MoveTrack {
            track: str_field(fields, "track")?,
            index: u64_field(fields, "index")? as usize,
        }),
        "Reverse" => Ok(ArrangeOp::Reverse {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
        }),
        "Stretch" => Ok(ArrangeOp::Stretch {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            source: str_field(fields, "source")?,
            src_len: u64_field(fields, "src_len")?,
            num: u32_field(fields, "num")?,
            den: u32_field(fields, "den")?,
        }),
        "AddClip" => {
            let src_len = u64_field(fields, "src_len")?;
            let loop_len = u64_field(fields, "loop")?;
            let clip = Clip {
                reversed: u64_field(fields, "reversed")? != 0,
                id: str_field(fields, "id")?,
                source: str_field(fields, "source")?,
                src_start: u64_field(fields, "src_start")?,
                src_len,
                at_frame: u64_field(fields, "at_frame")?,
                fade_in: u64_field(fields, "fade_in")?,
                fade_out: u64_field(fields, "fade_out")?,
                gain: f32_field(fields, "gain")?,
                loop_len: (loop_len != 0).then_some(loop_len),
                name: {
                    let name = str_field(fields, "name")?;
                    (!name.is_empty()).then_some(name)
                },
            };
            Ok(ArrangeOp::AddClip {
                track: str_field(fields, "track")?,
                clip,
            })
        }
        "RenameClip" => Ok(ArrangeOp::RenameClip {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            name: str_field(fields, "name")?,
        }),
        "SetMarker" => Ok(ArrangeOp::SetMarker {
            at_frame: u64_field(fields, "at_frame")?,
            name: str_field(fields, "name")?,
        }),
        "RemoveMarker" => Ok(ArrangeOp::RemoveMarker {
            at_frame: u64_field(fields, "at_frame")?,
        }),
        "RazorSplit" => Ok(ArrangeOp::RazorSplit {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            new_left: str_field(fields, "new_left")?,
            new_right: str_field(fields, "new_right")?,
            at_frame: u64_field(fields, "at_frame")?,
        }),
        "Trim" => Ok(ArrangeOp::Trim {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            edge: match str_field(fields, "edge")?.as_str() {
                "start" => Edge::Start,
                "end" => Edge::End,
                other => return Err(format!("unknown trim edge '{other}'")),
            },
            by_frames: match field(fields, "by")? {
                Value::I64(v) => *v,
                other => return Err(format!("field 'by' must be I64, got {other:?}")),
            },
        }),
        "MoveClip" => Ok(ArrangeOp::MoveClip {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            at_frame: u64_field(fields, "at_frame")?,
        }),
        "MoveClipToTrack" => Ok(ArrangeOp::MoveClipToTrack {
            from: str_field(fields, "from")?,
            clip: str_field(fields, "clip")?,
            to: str_field(fields, "to")?,
            at_frame: u64_field(fields, "at_frame")?,
        }),
        "Duplicate" => Ok(ArrangeOp::Duplicate {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            new_id: str_field(fields, "new_id")?,
        }),
        "Delete" => Ok(ArrangeOp::Delete {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
        }),
        "SetClipGain" => Ok(ArrangeOp::SetClipGain {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            gain: f32_field(fields, "gain")?,
        }),
        "SetClipFade" => Ok(ArrangeOp::SetClipFade {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            fade_in: u64_field(fields, "fade_in")?,
            fade_out: u64_field(fields, "fade_out")?,
        }),
        "LoopRegion" => Ok(ArrangeOp::LoopRegion {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            times: match field(fields, "times")? {
                Value::U32(t) => *t,
                other => return Err(format!("field 'times' must be U32, got {other:?}")),
            },
        }),
        "ChopClip" => Ok(ArrangeOp::ChopClip {
            track: str_field(fields, "track")?,
            clip: str_field(fields, "clip")?,
            times: match field(fields, "times")? {
                Value::U32(t) => *t,
                other => return Err(format!("field 'times' must be U32, got {other:?}")),
            },
            prefix: str_field(fields, "prefix")?,
        }),
        other => Err(format!("unknown arrangement op '{other}'")),
    }
}

/// The handlers a clip editor registers on an engine: each decodes the op fields
/// and applies the op to the shared `Timeline` (control side — a reconcile would
/// also rebuild readers here, but graph-node mounting needs `&mut Engine`, which
/// the host owns in P1.3.4).
pub fn register_handlers(
    engine: &mut Engine,
    timeline: Arc<Mutex<Timeline>>,
) -> Result<(), String> {
    for name in ALL_OPS {
        let tl = timeline.clone();
        let handler: OpHandler = Box::new(move |fields| {
            let op = decode_op(name, fields)?;
            let mut tl = tl.lock().map_err(|_| "timeline poisoned".to_string())?;
            *tl = tl.apply(&op)?;
            Ok(())
        });
        engine.register_op_handler(name, handler)?;
    }
    Ok(())
}

/// All op names, in a fixed order, for registration iteration.
pub const ALL_OPS: &[&str] = &[
    "AddTrack",
    "RemoveTrack",
    "RenameTrack",
    "MoveTrack",
    "Reverse",
    "Stretch",
    "RenameClip",
    "SetMarker",
    "RemoveMarker",
    "AddClip",
    "RazorSplit",
    "Trim",
    "MoveClip",
    "MoveClipToTrack",
    "Duplicate",
    "Delete",
    "SetClipGain",
    "SetClipFade",
    "LoopRegion",
    "ChopClip",
];

/// The clip-editor engine seam: owns the interner and the shared `Timeline`, and
/// issues logged ops. `apply` encodes the op and hands it to the engine (the
/// handler applies it to the timeline on the control-side flush).
pub struct ClipEditor {
    intern: Interner,
    timeline: Arc<Mutex<Timeline>>,
}

impl ClipEditor {
    pub fn new() -> Self {
        ClipEditor {
            intern: Interner::new(),
            timeline: Arc::new(Mutex::new(Timeline::new())),
        }
    }

    /// A snapshot of the current value — a **read-only** window (no write handle
    /// around the log; kimi must-fix M1). The value is only ever mutated by the
    /// op handlers, so it stays a pure reconstruction of the logged op stream.
    /// Poison maps to `Err` (matching `register_handlers`) — never a panic.
    pub fn snapshot(&self) -> Result<Timeline, String> {
        self.timeline
            .lock()
            .map(|g| g.clone())
            .map_err(|_| "timeline poisoned".to_string())
    }

    /// Register the op handlers on an engine (once, after the engine is built).
    pub fn register(&self, engine: &mut Engine) -> Result<(), String> {
        register_handlers(engine, self.timeline.clone())
    }

    /// Issue a logged command for `op` at the current frame. **Applies the op to
    /// the live value eagerly** (validating fail-loud first — a refused op is
    /// never logged), and logs it via `arrange_logged` (for replay) without
    /// scheduling it — arrangement ops are value-level, so they never touch the
    /// scheduler and cannot flush the mixer early. `replay_from` re-schedules the
    /// logged ops to reconstruct the identical value.
    pub fn apply(&mut self, engine: &mut Engine, op: &ArrangeOp) -> Result<(), String> {
        {
            let mut tl = self
                .timeline
                .lock()
                .map_err(|_| "timeline poisoned".to_string())?;
            *tl = tl.apply(op)?; // validate + apply live; a refused op returns before logging
        }
        let (name, fields) = encode_op(&mut self.intern, op);
        engine.arrange_logged(name, fields)
    }
}

impl Default for ClipEditor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
