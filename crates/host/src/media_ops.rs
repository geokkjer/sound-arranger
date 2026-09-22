//! Logged media commands.
//!
//! The host's media commands (`pool` / `play` / `splice`, and a `bounce` record)
//! are **profile-level ops carried by the one session log** — the same
//! closed-core `Event::Arrangement` carrier the clip editor uses. A handler the
//! host registers mutates a shared [`MediaSession`] *value*; the host reconciles
//! the graph from that value on the next render (the clip-editor pattern). So a
//! media session is reconstructable from the log and media determinism stops
//! being a parallel command seam.
//!
//! Handlers do **no I/O and no graph work**: `Engine::replay_from` runs them on a
//! fresh engine, where they rebuild the value; the graph and the reader threads
//! are host-side (`HostSession::wire_media`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use engine::{Engine, OpHandler, Value};
use media::Interner;

/// Op names — a closed vocabulary, prefixed so a plugin op cannot collide.
pub const OP_POOL: &str = "MediaPool";
pub const OP_PLAY: &str = "MediaPlay";
pub const OP_SPLICE: &str = "MediaSplice";
pub const OP_BOUNCE: &str = "MediaBounce";

/// Every media op name, for registration iteration.
pub const ALL_OPS: &[&str] = &[OP_POOL, OP_PLAY, OP_SPLICE, OP_BOUNCE];

/// A resolved `play` request: a clip region into a mixer channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerIntent {
    pub path: String,
    pub start: u64,
    pub len: u64,
    pub channel: usize,
}

/// A pending splice: the incoming clip plus a crossfade, at an absolute frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpliceIntent {
    pub at_frame: u64,
    pub path: String,
    pub start: u64,
    pub len: u64,
    pub crossfade: u32,
}

/// A completed bounce, recorded in the log: frames rendered and the drain
/// policy's outcome. Recorded for replay/observability; never re-executed (a
/// bounce writes a file, it is not session state). The output **path is not
/// recorded** — it is an action target, not session state, and recording it would
/// make two identical sessions bouncing to different files log differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BounceRecord {
    pub frames: u64,
    pub drained_frames: u64,
    pub capped: bool,
}

/// The media intent value: what the session asked the media layer to do. The host
/// reconciles the graph from this; a replayed log reconstructs it exactly.
#[derive(Debug, Default)]
pub struct MediaSession {
    pub pool_dir: Option<PathBuf>,
    pub player: Option<PlayerIntent>,
    pub splices: Vec<SpliceIntent>,
    pub bounces: Vec<BounceRecord>,
}

// ------------------------------------------------------------------- encode

/// Encode a `pool` request (the media pool directory).
pub fn encode_pool(i: &mut Interner, dir: &str) -> (&'static str, Vec<(&'static str, Value)>) {
    (OP_POOL, vec![("dir", Value::Str(i.intern(dir)))])
}

/// Encode a `play` request (clip region + mixer channel).
pub fn encode_play(
    i: &mut Interner,
    p: &PlayerIntent,
) -> (&'static str, Vec<(&'static str, Value)>) {
    (
        OP_PLAY,
        vec![
            ("path", Value::Str(i.intern(&p.path))),
            ("start", Value::U64(p.start)),
            ("len", Value::U64(p.len)),
            ("channel", Value::U32(p.channel as u32)),
        ],
    )
}

/// Encode a `splice` request (incoming clip + crossfade + absolute frame).
pub fn encode_splice(
    i: &mut Interner,
    s: &SpliceIntent,
) -> (&'static str, Vec<(&'static str, Value)>) {
    (
        OP_SPLICE,
        vec![
            ("at_frame", Value::U64(s.at_frame)),
            ("path", Value::Str(i.intern(&s.path))),
            ("start", Value::U64(s.start)),
            ("len", Value::U64(s.len)),
            ("crossfade", Value::U32(s.crossfade)),
        ],
    )
}

/// Encode a completed `bounce` (a record; the log carries the drain outcome).
pub fn encode_bounce(b: &BounceRecord) -> (&'static str, Vec<(&'static str, Value)>) {
    (
        OP_BOUNCE,
        vec![
            ("frames", Value::U64(b.frames)),
            ("drained", Value::U64(b.drained_frames)),
            ("capped", Value::U32(u32::from(b.capped))),
        ],
    )
}

// ------------------------------------------------------------------- decode

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
    match field(fields, name)? {
        Value::U64(n) => Ok(*n),
        other => Err(format!("field '{name}' must be a U64, got {other:?}")),
    }
}

fn u32_field(fields: &[(&'static str, Value)], name: &str) -> Result<u32, String> {
    match field(fields, name)? {
        Value::U32(n) => Ok(*n),
        other => Err(format!("field '{name}' must be a U32, got {other:?}")),
    }
}

/// Register the media-op handlers on an engine. Each decodes its fields and
/// mutates the shared [`MediaSession`] (control side — no I/O, no graph work, so
/// `Engine::replay_from` can run them on a fresh engine).
pub fn register_handlers(
    engine: &mut Engine,
    media: Arc<Mutex<MediaSession>>,
) -> Result<(), String> {
    for &op in ALL_OPS {
        let m = media.clone();
        let handler: OpHandler = Box::new(move |fields| {
            let mut s = m.lock().map_err(|_| "media session poisoned".to_string())?;
            match op {
                OP_POOL => {
                    s.pool_dir = Some(PathBuf::from(str_field(fields, "dir")?));
                }
                OP_PLAY => {
                    s.player = Some(PlayerIntent {
                        path: str_field(fields, "path")?,
                        start: u64_field(fields, "start")?,
                        len: u64_field(fields, "len")?,
                        channel: u32_field(fields, "channel")? as usize,
                    });
                }
                OP_SPLICE => {
                    s.splices.push(SpliceIntent {
                        at_frame: u64_field(fields, "at_frame")?,
                        path: str_field(fields, "path")?,
                        start: u64_field(fields, "start")?,
                        len: u64_field(fields, "len")?,
                        crossfade: u32_field(fields, "crossfade")?,
                    });
                }
                OP_BOUNCE => {
                    s.bounces.push(BounceRecord {
                        frames: u64_field(fields, "frames")?,
                        drained_frames: u64_field(fields, "drained")?,
                        capped: u32_field(fields, "capped")? != 0,
                    });
                }
                other => return Err(format!("unknown media op '{other}'")),
            }
            Ok(())
        });
        engine.register_op_handler(op, handler)?;
    }
    Ok(())
}
