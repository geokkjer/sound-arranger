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
    /// A **human name** for the clip (`None` = unnamed) — what a shell shows beside the
    /// id, so a 30-minute arrangement is recognisable ("bridge-take-2") rather than a
    /// wall of `c17`s. It carries no semantics: navigation, ops and the render path all
    /// key on `id`, and nothing binds a name to a uniqueness rule (two clips may share
    /// one — a name is a label, not an address).
    ///
    /// It must still be a token the `host v1` format can spell ([`valid_name`]): the
    /// format is whitespace-separated with no quoting, so a name with a space in it
    /// would save a session that could not be reopened. Names are therefore one word
    /// (use `-` or `_`), and the refusal says so.
    #[serde(default)]
    pub name: Option<String>,
    /// Play the region **backwards** (a clip property, not a rewritten pool copy:
    /// the source stays immutable and the reader reads the other way). A reversed
    /// clip cannot be looped or re-looped — see [`ArrangeOp::Reverse`].
    #[serde(default)]
    pub reversed: bool,
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
            // Reversed: the clip's first frame is the region's **top**, so the read
            // walks down to `src_start`. (Mirror of the forward mapping, which is why
            // split/trim/chop mirror their source arithmetic when `reversed`.)
            _ if self.reversed => self.src_start + (self.src_len - 1 - off),
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

/// Normalise a deserialized marker list: sorted by frame, at most one per frame (the
/// last one wins, which is what "set" means).
fn markers_from_json<'de, D>(deserializer: D) -> Result<Vec<Marker>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut markers = Vec::<Marker>::deserialize(deserializer)?;
    markers.sort_by_key(|m| m.at_frame);
    markers.dedup_by_key(|m| m.at_frame);
    Ok(markers)
}

/// A **marker**: a named point on the timeline (a section boundary, a take start, a
/// note to self). Markers are part of the arrangement *value* — they are logged like
/// any other edit, replay and save rebuild them, and the render path ignores them
/// entirely. One marker per frame; the name is the label a shell draws and jumps to.
///
/// The name must be a token the `host v1` format can spell (`valid_marker_name`): a
/// name the parser would split or strip is not a name, it is an unopenable session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub at_frame: Frame,
    pub name: String,
}

/// The arrangement value: the whole timeline of tracks, plus its markers (sorted by
/// frame, one per frame).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Timeline {
    pub tracks: Vec<Track>,
    /// Navigation labels, sorted by `at_frame` with at most one per frame. The
    /// deserializer **normalises** what it reads (sorted, deduplicated), so the
    /// invariant holds for a value that was never built by an op — a hand-written
    /// snapshot cannot break [`Timeline::marker_at`]'s binary search.
    #[serde(default, deserialize_with = "markers_from_json")]
    pub markers: Vec<Marker>,
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
    /// Rename a track in place. The *index* is what feeds `ch{ti}`, so a rename
    /// never moves the audio: the same track keeps its mixer channel.
    RenameTrack {
        track: Id,
        to: Id,
    },
    /// Move a track to `index` (0-based), shifting the others — the mixer channel
    /// each track feeds follows its position, which is what a reorder means.
    MoveTrack {
        track: Id,
        index: usize,
    },
    /// Play a clip backwards (or forwards again): a **toggle**, so the log stays one
    /// fact per press and `undo` means what it says. A *looped* clip is refused —
    /// the loop phase of a reversed read is not representable, the same reason
    /// razor-split and trim refuse one.
    Reverse {
        track: Id,
        clip: Id,
    },
    /// **Name a clip** (or clear its name with an empty one). A label, not an address:
    /// the id is what every other op keys on, so a rename can never move audio.
    RenameClip {
        track: Id,
        clip: Id,
        name: String,
    },
    /// **Set the marker at `at_frame`** to `name` — one op for "add" and "rename"
    /// (there is at most one marker per frame, so "set" is both, and it cannot fail on
    /// a duplicate). The name must be a token the log can spell.
    SetMarker {
        at_frame: Frame,
        name: String,
    },
    /// Remove the marker at `at_frame` (refused when there is none: a deletion that
    /// deleted nothing is not an edit).
    RemoveMarker {
        at_frame: Frame,
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
    ChopClip {
        track: Id,
        clip: Id,
        times: u32,
        prefix: Id,
    },
    /// Point a clip at **time-stretched material**: an offline render (the host's
    /// `stretch` command) writes a new pool source, and this op rewrites the clip's
    /// reference — `src_start` 0, `src_len` the frames actually written — so the
    /// arrangement gains no playback-rate property and the one-frame-domain rule
    /// survives: the arranger still reads a straight region of an immutable source.
    ///
    /// The ratio travels **with** the op (a rational, verbatim) because it is what was
    /// asked for: the source id and length record what happened, the ratio records the
    /// intent, and a replay reproduces the same value without re-rendering. Fades are
    /// capped to the new length, and a *looped* clip is refused (the loop phase of a
    /// stretched read is not representable — the same reason reverse refuses one).
    Stretch {
        track: Id,
        clip: Id,
        /// The new pool source (the rendered material).
        source: Id,
        /// The frames that source actually holds (the render's own count).
        src_len: Frame,
        num: u32,
        den: u32,
    },
}

/// `base + delta` with sign handling; `None` when the result is negative or
/// overflows `u64`. Used for trim arithmetic without `as i64` cast hazards.
fn add_signed(base: u64, delta: i64) -> Option<u64> {
    let r = base as i128 + delta as i128;
    if r < 0 || r > u64::MAX as i128 {
        None
    } else {
        Some(r as u64)
    }
}

/// A track id must be a word the `host v1` text format can carry **back**: it is
/// written as an operand (`add_clip <track> …`), so it must be one whitespace-free
/// token and must not look like one of the parser's own tokens — a leading `@`
/// (`@frame`), a leading `snap=`, or a `#` (which starts a comment). A name the
/// parser would strip is not an id; it is a session that cannot be reopened.
pub fn valid_track_id(id: &str) -> bool {
    valid_name(id)
}

/// Whether `name` is a token the `host v1` text format can carry **back**: one
/// whitespace-free word that is not one of the parser's own tokens (a leading `@` for a
/// frame, a leading `snap=`, a `#` comment). The same rule serves track ids, clip names
/// and marker names — anything the log has to spell as an operand.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.split_whitespace().count() == 1
        && !name.starts_with('@')
        && !name.starts_with("snap=")
        && !name.contains('#')
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
    if let Some(name) = &c.name
        && !valid_name(name)
    {
        // A label is written as a token in the log (`add_clip … <name>`), so an
        // `AddClip` that bypasses `RenameClip`'s check is refused here too — and
        // `Some("")` is unreachable, which is what lets the codec spell "no name" as
        // the empty string without ambiguity.
        return Err(format!("clip '{}' has an unusable name '{name}'", c.id));
    }
    // `checked_add`, not `+`: a pair whose sum overflows (`u64::MAX + 1` wraps to
    // 0) would *pass* the length check and enter the value as a clip the renderer
    // reads at gain 0 for every sample — a silent clip that claims to be audible.
    // A sum that cannot be represented is refused like any other over-long fade.
    if c.fade_in
        .checked_add(c.fade_out)
        .is_none_or(|s| s > c.src_len)
    {
        return Err(format!("clip '{}' fades exceed the clip length", c.id));
    }
    Ok(())
}

impl Timeline {
    /// The **end of the arrangement**: the last frame any clip occupies (exclusive),
    /// or 0 when there are no clips. This is the length an export renders — measured
    /// from the value, not handed in, so a mix cannot be exported short by a stale
    /// frame count.
    pub fn end_frame(&self) -> Frame {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.end())
            .max()
            .unwrap_or(0)
    }

    /// The marker at `frame`, if any.
    pub fn marker_at(&self, frame: Frame) -> Option<&Marker> {
        self.markers
            .binary_search_by_key(&frame, |m| m.at_frame)
            .ok()
            .map(|i| &self.markers[i])
    }

    /// The first marker **strictly after** `frame` — the "next section" a shell jumps
    /// to. Strictly, so pressing next at a marker moves on rather than standing still.
    pub fn marker_after(&self, frame: Frame) -> Option<&Marker> {
        self.markers.iter().find(|m| m.at_frame > frame)
    }

    /// The last marker **strictly before** `frame` — the "previous section".
    pub fn marker_before(&self, frame: Frame) -> Option<&Marker> {
        self.markers.iter().rev().find(|m| m.at_frame < frame)
    }

    /// The clip with `id` anywhere in the arrangement (the shell's "the clip under the
    /// playhead", resolved by the value rather than by a panel's copy).
    pub fn clip(&self, id: &str) -> Option<(&Track, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))
    }

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
        self.tracks
            .iter()
            .any(|t| t.clips.iter().any(|c| c.id == id))
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
                if !valid_track_id(track) {
                    return Err(format!("'{track}' is not usable as a track id"));
                }
                if self.track_index(track).is_some() {
                    return Err(format!("track '{track}' already exists"));
                }
                self.tracks.push(Track {
                    id: track.clone(),
                    clips: Vec::new(),
                });
                Ok(())
            }
            ArrangeOp::RemoveTrack { track } => {
                let ti = self
                    .track_index(track)
                    .ok_or_else(|| format!("no track '{track}'"))?;
                self.tracks.remove(ti);
                Ok(())
            }
            ArrangeOp::RenameTrack { track, to } => {
                let ti = self
                    .track_index(track)
                    .ok_or_else(|| format!("no track '{track}'"))?;
                // A track id is written into the `host v1` format, so it must stay
                // one whitespace-free word — otherwise the rename produces a
                // session that cannot be parsed back.
                if !valid_track_id(to) {
                    return Err(format!("'{to}' is not usable as a track id"));
                }
                if self.track_index(to).is_some() {
                    return Err(format!("track '{to}' already exists"));
                }
                self.tracks[ti].id = to.clone();
                Ok(())
            }
            ArrangeOp::Reverse { track, clip } => {
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let c = &self.tracks[ti].clips[ci];
                if c.loop_len.is_some() {
                    return Err(
                        "cannot reverse a looped clip (loop phase is not representable)".into(),
                    );
                }
                self.tracks[ti].clips[ci].reversed = !c.reversed;
                Ok(())
            }
            ArrangeOp::RenameClip { track, clip, name } => {
                if !name.is_empty() && !valid_name(name) {
                    return Err(format!("'{name}' is not usable as a clip name"));
                }
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                // An empty name *clears* the label (a rename that only ever added names
                // would make "the clip is unnamed again" unexpressible).
                self.tracks[ti].clips[ci].name = (!name.is_empty()).then(|| name.clone());
                Ok(())
            }
            ArrangeOp::SetMarker { at_frame, name } => {
                if !valid_name(name) {
                    return Err(format!("'{name}' is not usable as a marker name"));
                }
                match self.markers.binary_search_by_key(at_frame, |m| m.at_frame) {
                    Ok(i) => self.markers[i].name = name.clone(),
                    Err(i) => self.markers.insert(
                        i,
                        Marker {
                            at_frame: *at_frame,
                            name: name.clone(),
                        },
                    ),
                }
                Ok(())
            }
            ArrangeOp::RemoveMarker { at_frame } => {
                match self.markers.binary_search_by_key(at_frame, |m| m.at_frame) {
                    Ok(i) => {
                        self.markers.remove(i);
                        Ok(())
                    }
                    Err(_) => Err(format!("no marker at frame {at_frame}")),
                }
            }
            ArrangeOp::MoveTrack { track, index } => {
                let ti = self
                    .track_index(track)
                    .ok_or_else(|| format!("no track '{track}'"))?;
                if *index >= self.tracks.len() {
                    return Err(format!(
                        "track index {index} is out of range (0..{})",
                        self.tracks.len()
                    ));
                }
                if *index != ti {
                    let moved = self.tracks.remove(ti);
                    self.tracks.insert(*index, moved);
                }
                Ok(())
            }
            ArrangeOp::AddClip { track, clip } => {
                self.track_index(track)
                    .ok_or_else(|| format!("no track '{track}'"))?;
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
            ArrangeOp::RazorSplit {
                track,
                clip,
                new_left,
                new_right,
                at_frame,
            } => {
                if new_left == new_right {
                    return Err("razor-split ids must be distinct".into());
                }
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                if self.clip_id_exists(new_left) || self.clip_id_exists(new_right) {
                    return Err("razor-split ids must be new".into());
                }
                let c = self.tracks[ti].clips[ci].clone();
                if c.loop_len.is_some() {
                    // A looped clip cannot be cleanly split: the loop phase at the
                    // cut is not representable, and "drop the wrap" would silently
                    // change the right half's audio. Fail-loud (never logged).
                    return Err(
                        "cannot razor-split a looped clip (loop phase is not representable)".into(),
                    );
                }
                if *at_frame <= c.at_frame || *at_frame >= c.end() {
                    return Err(format!(
                        "split frame {at_frame} must be strictly inside [{}, {})",
                        c.at_frame,
                        c.end()
                    ));
                }
                let split_in = at_frame - c.at_frame; // frames into the clip (guaranteed > 0)
                let mut left = c.clone();
                left.id = new_left.clone();
                left.src_len = split_in;
                left.fade_out = 0; // the split seam is hard (a crossfade is a later SetClipFade)
                let mut right = c.clone();
                right.id = new_right.clone();
                right.at_frame = *at_frame;
                right.src_len = c.src_len - split_in;
                right.fade_in = 0;
                if c.reversed {
                    // The clip's *first* frames are the region's **top**, so the left
                    // half takes the top and the right half the bottom — the mirror of
                    // the forward split (`clip_tests` pins both).
                    left.src_start = c.src_start + right.src_len;
                    right.src_start = c.src_start;
                } else {
                    left.src_start = c.src_start;
                    right.src_start = c.src_start + split_in;
                }
                // drop the original clip, then sorted-insert both halves
                self.tracks[ti].clips.remove(ci);
                self.tracks[ti].clips.push(left);
                self.tracks[ti].clips.push(right);
                self.sort_track(ti);
                Ok(())
            }
            ArrangeOp::Trim {
                track,
                clip,
                edge,
                by_frames,
            } => {
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let c = self.tracks[ti].clips[ci].clone();
                match edge {
                    Edge::Start => {
                        if c.loop_len.is_some() {
                            return Err("cannot trim the start of a looped clip (loop phase is not representable)".into());
                        }
                        // Forward: move at_frame + src_start together, src_len shrinks.
                        // Reversed: the clip's first frames are the region's *top*, so
                        // trimming the start only shrinks `src_len` (the mirror of the
                        // forward end trim).
                        let at = add_signed(c.at_frame, *by_frames)
                            .ok_or("trim start would move before frame 0")?;
                        let len = add_signed(c.src_len, -(*by_frames))
                            .ok_or("trim start length out of range")?;
                        if len == 0 {
                            return Err("trim start would consume the whole clip".into());
                        }
                        let src_start = if c.reversed {
                            c.src_start
                        } else {
                            add_signed(c.src_start, *by_frames)
                                .ok_or("trim start would move before the source start")?
                        };
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
                        let len = add_signed(c.src_len, *by_frames)
                            .ok_or("trim end length out of range")?;
                        if len == 0 {
                            return Err("trim end would remove the whole clip".into());
                        }
                        let reversed = c.reversed;
                        let mut n = c;
                        n.src_len = len;
                        if reversed {
                            // Reversed: the clip's last frames are the region's
                            // *bottom*, so moving the end **earlier** (a negative
                            // `by`) raises `src_start` — the sign flips against the
                            // forward case.
                            n.src_start = add_signed(n.src_start, -(*by_frames))
                                .ok_or("trim end would move before the source start")?;
                        }
                        validate_clip(&n).map_err(|e| format!("trim end: {e}"))?;
                        self.tracks[ti].clips[ci] = n;
                        Ok(())
                    }
                }
            }
            ArrangeOp::MoveClip {
                track,
                clip,
                at_frame,
            } => {
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let mut n = self.tracks[ti].clips[ci].clone();
                n.at_frame = *at_frame;
                validate_clip(&n).map_err(|e| format!("move clip: {e}"))?;
                self.tracks[ti].clips[ci] = n;
                self.sort_track(ti);
                Ok(())
            }
            ArrangeOp::MoveClipToTrack {
                from,
                clip,
                to,
                at_frame,
            } => {
                let (ti, ci) = self
                    .locate(from, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{from}'"))?;
                let to_ti = self
                    .track_index(to)
                    .ok_or_else(|| format!("no track '{to}'"))?;
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
            ArrangeOp::Duplicate {
                track,
                clip,
                new_id,
            } => {
                if new_id == clip {
                    return Err("duplicate id must differ from the source clip".into());
                }
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
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
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                self.tracks[ti].clips.remove(ci);
                Ok(())
            }
            ArrangeOp::SetClipGain { track, clip, gain } => {
                if !gain.is_finite() {
                    return Err("clip gain must be finite".into());
                }
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                self.tracks[ti].clips[ci].gain = *gain;
                Ok(())
            }
            ArrangeOp::SetClipFade {
                track,
                clip,
                fade_in,
                fade_out,
            } => {
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                // `checked_add` for the same reason as `validate_clip`: a wrapped sum
                // would pass the length check and mute the clip instead of refusing it.
                if fade_in
                    .checked_add(*fade_out)
                    .is_none_or(|s| s > self.tracks[ti].clips[ci].src_len)
                {
                    return Err("fades exceed the clip length".into());
                }
                self.tracks[ti].clips[ci].fade_in = *fade_in;
                self.tracks[ti].clips[ci].fade_out = *fade_out;
                Ok(())
            }
            ArrangeOp::LoopRegion { track, clip, times } => {
                if let Some((ti, ci)) = self.locate(track, clip)
                    && self.tracks[ti].clips[ci].reversed
                {
                    return Err(
                        "cannot loop a reversed clip (loop phase is not representable)".into(),
                    );
                }
                if *times == 0 {
                    return Err("loop times must be >= 1".into());
                }
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let c = &mut self.tracks[ti].clips[ci];
                let region = c.loop_len.unwrap_or(c.src_len);
                let src_len = region
                    .checked_mul(*times as Frame)
                    .ok_or("loop region length overflows")?;
                c.loop_len = Some(region);
                c.src_len = src_len;
                Ok(())
            }
            ArrangeOp::ChopClip {
                track,
                clip,
                times,
                prefix,
            } => {
                if *times == 0 {
                    return Err("chop times must be >= 1".into());
                }
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let c = self.tracks[ti].clips[ci].clone();
                if c.loop_len.is_some() {
                    return Err(
                        "cannot chop a looped clip (loop phase is not representable)".into(),
                    );
                }
                let times_f = *times as Frame;
                if times_f > c.src_len {
                    return Err(format!(
                        "chop {} times exceeds src_len {}",
                        times_f, c.src_len
                    ));
                }
                // Split the source region into `times` contiguous equal (within 1
                // frame) pieces. Piece ids are a pure function of `prefix` + index,
                // so replay reproduces them deterministically with no randomness.
                let base = c.src_len / times_f;
                let rem = c.src_len % times_f;
                let mut pieces = Vec::new();
                // Forward, the pieces walk up from `src_start`; reversed, they walk
                // **down** from the region's top (the first piece in time is the top).
                let mut src_at = if c.reversed {
                    c.src_start + c.src_len
                } else {
                    c.src_start
                };
                let mut at = c.at_frame;
                let mut seen = std::collections::HashSet::new();
                for i in 0..times_f {
                    let pid = format!("{prefix}.{i}");
                    if self.clip_id_exists(&pid) || !seen.insert(pid.clone()) {
                        return Err(format!("chop derived id '{pid}' already exists or repeats"));
                    }
                    let slen = base + if i < rem { 1 } else { 0 };
                    // Preserve the clip's outer fades on the first/last piece (as
                    // RazorSplit does) so a chop doesn't silently remove audible
                    // crossfades; interior seams are hard (a SetClipFade follows).
                    if c.reversed {
                        src_at -= slen;
                    }
                    pieces.push(Clip {
                        id: pid,
                        // A chop names its pieces from the source's name, so a labelled
                        // take stays recognisable instead of becoming `c17`/`c18`.
                        name: c.name.clone(),
                        source: c.source.clone(),
                        src_start: src_at,
                        src_len: slen,
                        at_frame: at,
                        fade_in: if i == 0 { c.fade_in } else { 0 },
                        fade_out: if i + 1 == times_f { c.fade_out } else { 0 },
                        gain: c.gain,
                        loop_len: None,
                        reversed: c.reversed,
                    });
                    if !c.reversed {
                        src_at += slen;
                    }
                    at += slen;
                }
                self.tracks[ti].clips.remove(ci);
                self.tracks[ti].clips.extend(pieces);
                self.sort_track(ti);
                Ok(())
            }
            ArrangeOp::Stretch {
                track,
                clip,
                source,
                src_len,
                num,
                den,
            } => {
                if *num == 0 || *den == 0 {
                    return Err(format!("stretch ratio must be non-zero (got {num}/{den})"));
                }
                if *src_len == 0 {
                    return Err("a stretch must point the clip at a non-empty source".into());
                }
                let (ti, ci) = self
                    .locate(track, clip)
                    .ok_or_else(|| format!("clip '{clip}' not on track '{track}'"))?;
                let c = self.tracks[ti].clips[ci].clone();
                if c.loop_len.is_some() {
                    return Err(
                        "cannot stretch a looped clip (loop phase is not representable)".into(),
                    );
                }
                let mut n = c;
                n.source = source.clone();
                n.src_start = 0;
                n.src_len = *src_len;
                n.loop_len = None;
                // The clip's fades were legal for the old length; the new one may be
                // shorter, so cap them (the same rule the clipboard's micro-fades and
                // trim-to-content follow).
                n.fade_in = n.fade_in.min(n.src_len);
                n.fade_out = n.fade_out.min(n.src_len.saturating_sub(n.fade_in));
                validate_clip(&n).map_err(|e| format!("stretch: {e}"))?;
                self.tracks[ti].clips[ci] = n;
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
            t3.end_frame(),
            t.end_frame(),
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
            track.clips.last().unwrap().end(),
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
}
