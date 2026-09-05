//! The arrangement value (P1.3, `timeline`): clips placed at frames on tracks.
//!
//! This is a **graph value**, not nodes (the P1.3 shape decision): an immutable
//! `Timeline`/`Track`/`Clip` model plus the ACID ops as pure transforms
//! `apply(&self, op) -> Result<Self>`. The engine logs the ops (as `ArrangeOp`s
//! with `at_frame`); replay applies them to a fresh empty timeline and
//! reproduces the identical value — byte-identical audio for the same
//! (log, pool) pair. Ids are carried *in* the ops, so they are logged and
//! deterministic — never random/UUID/time.
//!
//! Semantics (P1.3 shape note + kimi design review):
//! - a `Clip` references a pool source (`source` = content-hash id) with a source
//!   window `[src_start, src_start + src_len)`, placed at `at_frame` on a track;
//! - the clip's **timeline span** is `[at_frame, at_frame + src_len)` (no
//!   stretch/pitch in Phase 1; mono). `loop_len` (when `Some`) makes the source
//!   read wrap at `loop_len` — a baked loop;
//! - clips **layer and overlap** (they sum at render), each with per-clip
//!   `fade_in`/`fade_out` (authoritative at boundaries); a gap is silence;
//! - a track's clips are kept **sorted by `at_frame`** (stable), so a render node
//!   may binary-search the active window and never depends on insertion order.
//!
//! Invariants enforced by every mutating op (fail-loud, never partial):
//! `src_len > 0`, `gain` finite, `loop_len != Some(0)`, `at_frame + src_len` not
//! overflowing, and `src_len <= i64::MAX` (so signed trim arithmetic never wraps).
//! A refused op returns `Err` and — per the engine contract — is never logged.

use serde::{Deserialize, Serialize};

/// Absolute frame on the timeline (samples).
pub type Frame = u64;

/// A clip/track/source id. The value model uses an owned [`String`]; interning
/// to `&'static str` (and canonical `f32`-bit encoding) is a *log* concern, kept
/// out of the value so it stays a plain, comparable model.
pub type Id = String;

/// Which edge of a clip a `Trim` adjusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    /// The clip's `at_frame`/`src_start` edge.
    Start,
    /// The clip's end edge (`src_len`).
    End,
}

/// A clip: a bounded region of a pool source, placed on a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Id,
    /// Content-hash id of the immutable float-WAV source (the media pool).
    pub source: Id,
    /// First source frame of the region (absolute in the source).
    pub src_start: Frame,
    /// Region length in frames (the clip's timeline length).
    pub src_len: Frame,
    /// Position of the clip's start on the track (timeline frames).
    pub at_frame: Frame,
    /// Per-clip fade-in (frames). **Authoritative** at boundaries.
    pub fade_in: Frame,
    /// Per-clip fade-out (frames). **Authoritative** at boundaries.
    pub fade_out: Frame,
    /// Per-clip gain (finite; serialised bit-exactly in the log).
    pub gain: f32,
    /// When `Some(r > 0)`, the source read wraps every `r` frames (a baked loop);
    /// `src_len` is then `r * times`. `None` = contiguous read.
    pub loop_len: Option<Frame>,
}

impl Clip {
    /// The clip's end frame on the timeline (exclusive).
    ///
    /// Callers may assume the span was validated when the clip was admitted
    /// (`at_frame.checked_add(src_len)`); a clip built by hand and fed straight
    /// to [`Timeline::apply`] is validated on the way in.
    pub fn end(&self) -> Frame {
        self.at_frame + self.src_len
    }

    /// The source offset for a timeline offset within the clip's span.
    ///
    /// Determines the source frame for `offset` samples after `at_frame`:
    /// wraps at `loop_len` when set, else offsets linearly. The render node
    /// calls this off the render path, per block.
    pub fn source_frame_at(&self, offset: Frame) -> Frame {
        debug_assert!(offset < self.src_len, "source_frame_at called out of span");
        let off = offset.min(self.src_len.saturating_sub(1));
        match self.loop_len {
            Some(loop_len) if loop_len > 0 => self.src_start + (off % loop_len),
            _ => self.src_start + off,
        }
    }
}

/// A track: a named lane holding layered clips (sorted by `at_frame`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: Id,
    pub clips: Vec<Clip>,
}

/// The arrangement value: the whole timeline of tracks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Timeline {
    pub tracks: Vec<Track>,
}

/// An ACID operation on the timeline (the logged command list). Every entity-creating
/// op carries the id it creates, so all ids are logged and deterministic.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrangeOp {
    AddTrack {
        track: Id,
    },
    RemoveTrack {
        track: Id,
    },
    AddClip {
        track: Id,
        clip: Clip,
    },
    RazorSplit {
        track: Id,
        clip: Id,
        new_left: Id,
        new_right: Id,
        at_frame: Frame,
    },
    Trim {
        track: Id,
        clip: Id,
        edge: Edge,
        by_frames: i64,
    },
    MoveClip {
        track: Id,
        clip: Id,
        at_frame: Frame,
    },
    MoveClipToTrack {
        from: Id,
        clip: Id,
        to: Id,
        at_frame: Frame,
    },
    Duplicate {
        track: Id,
        clip: Id,
        new_id: Id,
    },
    Delete {
        track: Id,
        clip: Id,
    },
    SetClipGain {
        track: Id,
        clip: Id,
        gain: f32,
    },
    SetClipFade {
        track: Id,
        clip: Id,
        fade_in: Frame,
        fade_out: Frame,
    },
    LoopRegion {
        track: Id,
        clip: Id,
        times: u32,
    },
}

/// `base + delta` with sign handling; `None` when the result is negative or
/// overflows `u64`. Used for trim arithmetic without `as i64` cast hazards.
fn add_signed(base: u64, delta: i64) -> Option<u64> {
    let r = base as i128 + delta as i128;
    if r < 0 || r > u64::MAX as i128 { None } else { Some(r as u64) }
}

/// Validates a clip's invariant fields; `Err` names the first violation.
pub fn validate_clip(c: &Clip) -> Result<(), String> {
    if c.src_len == 0 {
        return Err(format!("clip '{}' src_len must be > 0", c.id));
    }
    if c.src_len > i64::MAX as Frame {
        return Err(format!("clip '{}' src_len too large", c.id));
    }
    if !c.gain.is_finite() {
        return Err(format!("clip '{}' gain must be finite", c.id));
    }
    if c.loop_len == Some(0) {
        return Err(format!("clip '{}' loop_len must be > 0 or None", c.id));
    }
    if c.at_frame.checked_add(c.src_len).is_none() {
        return Err(format!("clip '{}' span overflows the timeline", c.id));
    }
    if c.fade_in + c.fade_out > c.src_len {
        return Err(format!("clip '{}' fades exceed the clip length", c.id));
    }
    Ok(())
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    fn track_index(&self, id: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// (track index, clip index) for `clip` on `track`; the outer `None` when the
    /// track is absent, the inner `None` when the clip is not on it.
    fn locate(&self, track: &str, clip: &str) -> Option<(usize, usize)> {
        let ti = self.track_index(track)?;
        let ci = self.tracks[ti].clips.iter().position(|c| c.id == clip)?;
        Some((ti, ci))
    }

    /// Look up a clip by id across all tracks (for uniqueness checks).
    fn clip_id_exists(&self, id: &str) -> bool {
        self.tracks.iter().any(|t| t.clips.iter().any(|c| c.id == id))
    }

    /// Apply an op purely: returns the updated timeline, or `Err` (never partially
    /// applies). A returned `Err` is **never logged** by the engine (fail-loud).
    pub fn apply(&self, op: &ArrangeOp) -> Result<Self, String> {
        let mut next = self.clone();
        next.apply_mut(op)?;
        Ok(next)
    }

    /// Re-sort a track's clips by `at_frame` (stable — equal frames keep their
    /// relative order, which is deterministic for a given op log).
    fn sort_track(&mut self, ti: usize) {
        self.tracks[ti].clips.sort_by_key(|c| c.at_frame);
    }

    /// In-place apply (the engine's dispatcher uses this). Validate-first, so a
    /// returned `Err` never leaves a half-mutated value.
    fn apply_mut(&mut self, op: &ArrangeOp) -> Result<(), String> {
        match op {
            ArrangeOp::AddTrack { track } => {
                if self.track_index(track).is_some() {
                    return Err(format!("track '{track}' already exists"));
                }
                self.tracks.push(Track { id: track.clone(), clips: Vec::new() });
                Ok(())
            }
            ArrangeOp::RemoveTrack { track } => {
                let ti = self.track_index(track).ok_or_else(|| format!("no track '{track}'"))?;
                self.tracks.remove(ti);
                Ok(())
            }
            ArrangeOp::AddClip { track, clip } => {
                self.track_index(track).ok_or_else(|| format!("no track '{track}'"))?;
                validate_clip(clip).map_err(|e| format!("add clip: {e}"))?;
                if self.clip_id_exists(&clip.id) {
                    return Err(format!("clip id '{}' already exists", clip.id));
                }
                let ti = self.track_index(track).expect("checked");
                let clips = &mut self.tracks[ti].clips;
                let pos = clips.partition_point(|c| c.at_frame <= clip.at_frame);
                clips.insert(pos, clip.clone());
                Ok(())
            }
            ArrangeOp::RazorSplit { track, clip, new_left, new_right, at_frame } => {
                if new_left == new_right {
                    return Err("razor-split ids must be distinct".into());
                }
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                if self.clip_id_exists(new_left) || self.clip_id_exists(new_right) {
                    return Err("razor-split ids must be new".into());
                }
                let c = self.tracks[ti].clips[ci].clone();
                if c.loop_len.is_some() {
                    // A looped clip cannot be cleanly split: the loop phase at the
                    // cut is not representable, and "drop the wrap" would silently
                    // change the right half's audio. Fail-loud (never logged).
                    return Err("cannot razor-split a looped clip (loop phase is not representable)".into());
                }
                if *at_frame <= c.at_frame || *at_frame >= c.end() {
                    return Err(format!("split frame {at_frame} must be strictly inside [{}, {})", c.at_frame, c.end()));
                }
                let split_in = at_frame - c.at_frame; // frames into the clip (guaranteed > 0)
                let mut left = c.clone();
                left.id = new_left.clone();
                left.src_len = split_in;
                left.fade_out = 0; // the split seam is hard (a crossfade is a later SetClipFade)
                let mut right = c.clone();
                right.id = new_right.clone();
                right.at_frame = *at_frame;
                right.src_start = c.src_start + split_in;
                right.src_len = c.src_len - split_in;
                right.fade_in = 0;
                // drop the original clip, then sorted-insert both halves
                self.tracks[ti].clips.remove(ci);
                self.tracks[ti].clips.push(left);
                self.tracks[ti].clips.push(right);
                self.sort_track(ti);
                Ok(())
            }
            ArrangeOp::Trim { track, clip, edge, by_frames } => {
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let c = self.tracks[ti].clips[ci].clone();
                match edge {
                    Edge::Start => {
                        if c.loop_len.is_some() {
                            return Err("cannot trim the start of a looped clip (loop phase is not representable)".into());
                        }
                        // move at_frame + src_start together; src_len shrinks/grows the same.
                        let at = add_signed(c.at_frame, *by_frames).ok_or("trim start would move before frame 0")?;
                        let src_start = add_signed(c.src_start, *by_frames).ok_or("trim start would move before the source start")?;
                        let len = add_signed(c.src_len, -(*by_frames)).ok_or("trim start length out of range")?;
                        if len == 0 {
                            return Err("trim start would consume the whole clip".into());
                        }
                        let mut n = c;
                        n.at_frame = at;
                        n.src_start = src_start;
                        n.src_len = len;
                        validate_clip(&n).map_err(|e| format!("trim start: {e}"))?;
                        self.tracks[ti].clips[ci] = n;
                        self.sort_track(ti);
                        Ok(())
                    }
                    Edge::End => {
                        let len = add_signed(c.src_len, *by_frames).ok_or("trim end length out of range")?;
                        if len == 0 {
                            return Err("trim end would remove the whole clip".into());
                        }
                        let mut n = c;
                        n.src_len = len;
                        validate_clip(&n).map_err(|e| format!("trim end: {e}"))?;
                        self.tracks[ti].clips[ci] = n;
                        Ok(())
                    }
                }
            }
            ArrangeOp::MoveClip { track, clip, at_frame } => {
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let mut n = self.tracks[ti].clips[ci].clone();
                n.at_frame = *at_frame;
                validate_clip(&n).map_err(|e| format!("move clip: {e}"))?;
                self.tracks[ti].clips[ci] = n;
                self.sort_track(ti);
                Ok(())
            }
            ArrangeOp::MoveClipToTrack { from, clip, to, at_frame } => {
                let (ti, ci) = self.locate(from, clip).ok_or_else(|| format!("clip '{clip}' not on track '{from}'"))?;
                let to_ti = self.track_index(to).ok_or_else(|| format!("no track '{to}'"))?;
                let mut n = self.tracks[ti].clips[ci].clone();
                n.at_frame = *at_frame;
                validate_clip(&n).map_err(|e| format!("move clip: {e}"))?;
                self.tracks[ti].clips.remove(ci);
                self.tracks[to_ti].clips.push(n);
                self.sort_track(to_ti);
                if to_ti != ti {
                    self.sort_track(ti);
                }
                Ok(())
            }
            ArrangeOp::Duplicate { track, clip, new_id } => {
                if new_id == clip {
                    return Err("duplicate id must differ from the source clip".into());
                }
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                if self.clip_id_exists(new_id) {
                    return Err(format!("duplicate id '{new_id}' already exists"));
                }
                let mut copy = self.tracks[ti].clips[ci].clone();
                copy.id = new_id.clone();
                self.tracks[ti].clips.push(copy);
                self.sort_track(ti); // same at_frame as the source → stays adjacent, deterministic
                Ok(())
            }
            ArrangeOp::Delete { track, clip } => {
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                self.tracks[ti].clips.remove(ci);
                Ok(())
            }
            ArrangeOp::SetClipGain { track, clip, gain } => {
                if !gain.is_finite() {
                    return Err("clip gain must be finite".into());
                }
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                self.tracks[ti].clips[ci].gain = *gain;
                Ok(())
            }
            ArrangeOp::SetClipFade { track, clip, fade_in, fade_out } => {
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                if fade_in + fade_out > self.tracks[ti].clips[ci].src_len {
                    return Err("fades exceed the clip length".into());
                }
                self.tracks[ti].clips[ci].fade_in = *fade_in;
                self.tracks[ti].clips[ci].fade_out = *fade_out;
                Ok(())
            }
            ArrangeOp::LoopRegion { track, clip, times } => {
                if *times == 0 {
                    return Err("loop times must be >= 1".into());
                }
                let (ti, ci) = self.locate(track, clip).ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let c = &mut self.tracks[ti].clips[ci];
                let region = c.loop_len.unwrap_or(c.src_len);
                let src_len = region.checked_mul(*times as Frame).ok_or("loop region length overflows")?;
                c.loop_len = Some(region);
                c.src_len = src_len;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: &str, at: Frame, len: Frame) -> Clip {
        Clip {
            id: id.into(),
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
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddTrack { track: "t1".into() }).unwrap();
        t
    }

    #[test]
    fn add_and_list_tracks() {
        let t = two_tracks();
        assert_eq!(t.tracks.len(), 2);
        assert_eq!(t.tracks[0].id, "t0");
        assert!(t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).is_err());
    }

    #[test]
    fn add_clip_places_and_keeps_sorted() {
        let mut t = two_tracks();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c2", 48000, 24000) }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c1", 0, 24000) }).unwrap();
        let ids: Vec<_> = t.tracks[0].clips.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["c1", "c2"]);
        assert!(t.apply(&ArrangeOp::AddClip { track: "nope".into(), clip: clip("c3", 0, 1) }).is_err());
        assert!(t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c1", 0, 1) }).is_err());
    }

    #[test]
    fn add_clip_validates_the_clip() {
        let t = two_tracks();
        // src_len 0
        assert!(t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 0) }).is_err());
        // NaN gain
        let mut c = clip("c0", 0, 100);
        c.gain = f32::NAN;
        assert!(t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: c }).is_err());
        // loop_len Some(0)
        let mut c = clip("c0", 0, 100);
        c.loop_len = Some(0);
        assert!(t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: c }).is_err());
        // span overflow
        let c = clip("c0", u64::MAX - 10, 100);
        assert!(t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: c }).is_err());
        // fades exceed clip
        let mut c = clip("c0", 0, 100);
        c.fade_in = 60;
        c.fade_out = 60;
        assert!(t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: c }).is_err());
    }

    #[test]
    fn razor_split_inside_produces_two_sorted_halves() {
        let mut t = Timeline::new();
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 1000, 4000) }).unwrap();
        t = t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(),
            clip: "c0".into(),
            new_left: "cL".into(),
            new_right: "cR".into(),
            at_frame: 3000,
        }).unwrap();
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
        assert!(t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(), clip: "cL".into(), new_left: "x".into(), new_right: "y".into(), at_frame: 1000,
        }).is_err());
        // distinct split ids required
        assert!(t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(), clip: "cL".into(), new_left: "z".into(), new_right: "z".into(), at_frame: 1500,
        }).is_err());
    }

    #[test]
    fn razor_split_keeps_sorted_with_overlapping_neighbor() {
        // a neighbor starts inside the split clip's span; the split must re-sort.
        let mut t = Timeline::new();
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("big", 0, 10000) }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("mid", 5000, 100) }).unwrap();
        t = t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(), clip: "big".into(), new_left: "A".into(), new_right: "B".into(), at_frame: 8000,
        }).unwrap();
        let ids: Vec<_> = t.tracks[0].clips.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "mid", "B"]);
        let frames: Vec<_> = t.tracks[0].clips.iter().map(|c| c.at_frame).collect();
        assert!(frames.windows(2).all(|w| w[0] <= w[1]), "clips must stay sorted: {frames:?}");
    }

    #[test]
    fn razor_split_refuses_a_looped_clip() {
        let mut t = Timeline::new();
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 1000) }).unwrap();
        t = t.apply(&ArrangeOp::LoopRegion { track: "t0".into(), clip: "c0".into(), times: 2 }).unwrap();
        assert!(t.apply(&ArrangeOp::RazorSplit {
            track: "t0".into(), clip: "c0".into(), new_left: "L".into(), new_right: "R".into(), at_frame: 500,
        }).is_err());
        assert!(t.apply(&ArrangeOp::Trim { track: "t0".into(), clip: "c0".into(), edge: Edge::Start, by_frames: 100 }).is_err());
    }

    #[test]
    fn trim_moves_at_frame_with_src_start_and_resorts() {
        let mut t = Timeline::new();
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 1000, 4000) }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c1", 500, 100) }).unwrap();
        t = t.apply(&ArrangeOp::Trim { track: "t0".into(), clip: "c0".into(), edge: Edge::Start, by_frames: 500 }).unwrap();
        let c = t.tracks[0].clips.iter().find(|c| c.id == "c0").unwrap();
        assert_eq!(c.at_frame, 1500);
        assert_eq!(c.src_start, 500);
        assert_eq!(c.src_len, 3500);
        // trim start to before frame 0 is refused
        assert!(t.apply(&ArrangeOp::Trim { track: "t0".into(), clip: "c0".into(), edge: Edge::Start, by_frames: -5000 }).is_err());
        // order kept sorted after the move
        let frames: Vec<_> = t.tracks[0].clips.iter().map(|c| c.at_frame).collect();
        assert!(frames.windows(2).all(|w| w[0] <= w[1]), "clips must stay sorted: {frames:?}");
    }

    #[test]
    fn move_and_cross_track_move() {
        let mut t = two_tracks();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 100, 100) }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c1", 200, 100) }).unwrap();
        t = t.apply(&ArrangeOp::MoveClip { track: "t0".into(), clip: "c0".into(), at_frame: 900 }).unwrap();
        assert_eq!(t.tracks[0].clips.iter().find(|c| c.id == "c0").unwrap().at_frame, 900);
        // move within the same track re-sorts
        let frames: Vec<_> = t.tracks[0].clips.iter().map(|c| c.at_frame).collect();
        assert!(frames.windows(2).all(|w| w[0] <= w[1]), "clips must stay sorted: {frames:?}");
        t = t.apply(&ArrangeOp::MoveClipToTrack { from: "t0".into(), clip: "c0".into(), to: "t1".into(), at_frame: 50 }).unwrap();
        assert!(t.tracks[0].clips.iter().all(|c| c.id != "c0"));
        assert_eq!(t.tracks[1].clips.iter().find(|c| c.id == "c0").unwrap().at_frame, 50);
        // cross-track move with same source+to actually moves (from != to)
        assert_eq!(t.tracks[1].clips.len(), 1);
    }

    #[test]
    fn duplicate_and_delete() {
        let mut t = Timeline::new();
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 1000) }).unwrap();
        t = t.apply(&ArrangeOp::Duplicate { track: "t0".into(), clip: "c0".into(), new_id: "c1".into() }).unwrap();
        assert_eq!(t.tracks[0].clips.len(), 2);
        assert!(t.apply(&ArrangeOp::Duplicate { track: "t0".into(), clip: "c0".into(), new_id: "c1".into() }).is_err());
        // duplicate with same id is refused
        assert!(t.apply(&ArrangeOp::Duplicate { track: "t0".into(), clip: "c0".into(), new_id: "c0".into() }).is_err());
        t = t.apply(&ArrangeOp::Delete { track: "t0".into(), clip: "c1".into() }).unwrap();
        assert_eq!(t.tracks[0].clips.len(), 1);
    }

    #[test]
    fn loop_region_bakes_repeats_with_wrap_len() {
        let mut t = Timeline::new();
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 1000) }).unwrap();
        t = t.apply(&ArrangeOp::LoopRegion { track: "t0".into(), clip: "c0".into(), times: 3 }).unwrap();
        let c = &t.tracks[0].clips[0];
        assert_eq!(c.loop_len, Some(1000));
        assert_eq!(c.src_len, 3000);
        assert_eq!(c.source_frame_at(0), 0);
        assert_eq!(c.source_frame_at(999), 999);
        assert_eq!(c.source_frame_at(1000), 0);
        assert_eq!(c.source_frame_at(2500), 500);
    }

    #[test]
    fn gain_and_fade_validate() {
        let mut t = Timeline::new();
        t = t.apply(&ArrangeOp::AddTrack { track: "t0".into() }).unwrap();
        t = t.apply(&ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 100) }).unwrap();
        t = t.apply(&ArrangeOp::SetClipGain { track: "t0".into(), clip: "c0".into(), gain: 0.5 }).unwrap();
        assert_eq!(t.tracks[0].clips[0].gain, 0.5);
        assert!(t.apply(&ArrangeOp::SetClipGain { track: "t0".into(), clip: "c0".into(), gain: f32::NAN }).is_err());
        t = t.apply(&ArrangeOp::SetClipFade { track: "t0".into(), clip: "c0".into(), fade_in: 10, fade_out: 10 }).unwrap();
        assert_eq!(t.tracks[0].clips[0].fade_in, 10);
        // fades exceeding the clip length are refused
        assert!(t.apply(&ArrangeOp::SetClipFade { track: "t0".into(), clip: "c0".into(), fade_in: 80, fade_out: 80 }).is_err());
    }

    #[test]
    fn pure_apply_is_deterministic_and_replays() {
        let ops = [
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddTrack { track: "t1".into() },
            ArrangeOp::AddClip { track: "t0".into(), clip: clip("c0", 0, 1000) },
            ArrangeOp::LoopRegion { track: "t0".into(), clip: "c0".into(), times: 2 },
            ArrangeOp::MoveClipToTrack { from: "t0".into(), clip: "c0".into(), to: "t1".into(), at_frame: 500 },
            ArrangeOp::SetClipFade { track: "t1".into(), clip: "c0".into(), fade_in: 8, fade_out: 0 },
        ];
        let mut a = Timeline::new();
        let mut b = Timeline::new();
        for op in &ops {
            a = a.apply(op).unwrap();
            b = b.apply(op).unwrap();
        }
        assert_eq!(a, b);
    }
}
