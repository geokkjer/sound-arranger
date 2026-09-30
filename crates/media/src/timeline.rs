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
//! overflowing, `src_start + src_len` not overflowing (the source window is an
//! address, and every op that rewrites a clip's geometry adds to it),
//! `src_len <= i64::MAX` (so signed trim arithmetic never wraps), and a `source` that
//! is a **plain pool id** (`pool::valid_id` — a source is a file stem, not a path, and
//! the log carries it as an arbitrary token). An op that
//! *changes* a clip's geometry (razor-split, chop, trim, stretch, loop) leaves the
//! clip satisfying all of them — a shrink caps the fades it can no longer fit, the
//! way `Stretch` always has — so no op the log accepts can hold a clip
//! [`validate_clip`] refuses, which is what would make the whole track unplayable.
//! A refused op returns `Err` and — per the engine contract — is never logged.
//!
//! **An op checks the arithmetic it does, it does not trust an admission it cannot
//! see.** `Timeline` is `Deserialize` and a hand-built `Track` is a legal `&self`
//! to [`Timeline::apply`], so a clip can reach an arm without having passed
//! `AddClip`: the sums a geometry op computes are `checked` (an unchecked one is a
//! debug panic inside the editor's timeline lock and a silent wrap in release),
//! `Clip::end` is checked for the same reason, and an arm validates the clip it was
//! handed where its own arithmetic depends on one.

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
    /// Content-hash id of the immutable float-WAV source (the media pool) — a
    /// **pool id** (a plain file stem), not a path: [`validate_clip`] refuses one
    /// that could address a file outside the pool.
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
    /// The clip's end frame on the timeline (exclusive), or `None` when the span
    /// `at_frame + src_len` cannot be represented.
    ///
    /// **Checked, not wrapped.** `AddClip` and every op that rewrites a clip's
    /// geometry validate the span, so this is `Some` for every clip the model
    /// admits — but a `Clip` is `Deserialize` and a hand-built `Track` is a legal
    /// `&self` to [`Timeline::apply`], so an unrepresentable span can reach a
    /// `pub` method. An unchecked `+` here is a debug panic inside
    /// [`Timeline::end_frame`] (which every `export` calls) and a **wrapped** end
    /// in release: an export that silently drops the clip. [`Timeline::end_frame`]
    /// refuses such a value by name; the render path saturates, because
    /// `ArrangerNode::new` validated every clip it holds.
    pub fn end(&self) -> Option<Frame> {
        self.at_frame.checked_add(self.src_len)
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

/// The most slices one [`ArrangeOp::ChopClip`] will make.
///
/// A chop's grain is a slice per bar or finer, so 4096 is far past any hand-made
/// arrangement — and it is a **bound**, not a cap: a larger `times` is refused by name
/// rather than sized. `times` is a `u32` off the wire and the only other limit is
/// `times <= src_len`, which reaches `i64::MAX`; without this the op is one logged
/// line that builds billions of `Clip`s (three heap `String`s each) inside a
/// `Timeline` `apply` already cloned, and sorts them.
const MAX_CHOP_SLICES: Frame = 4096;

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
    /// **Bake a loop**: the clip plays its region `times` times, and is as long on the
    /// timeline as the region is `times`.
    ///
    /// `times` is the **number of passes, not a factor**, and the region is fixed by
    /// the first bake (`loop_len`, or the clip's own length when this is the first
    /// loop) — so a re-bake re-reads the same region and sets the total to `times`
    /// passes of it. `times: 3` applied twice is three passes, not nine; `times: 1` is
    /// the single pass the clip was before its first bake, so **a re-bake with a
    /// smaller count shortens the clip** — and caps the fades it can no longer cover,
    /// the same rule every other shrink follows. `times: 0` is refused: it is not
    /// "un-loop", which no op in this model represents (delete and re-add is).
    LoopRegion {
        track: Id,
        clip: Id,
        times: u32,
    },
    ChopClip {
        track: Id,
        clip: Id,
        /// The slice count. Bounded: `1 <= times <= min(src_len, 4096)` — a count past
        /// that is refused by name, not sized (`MAX_CHOP_SLICES` in this module).
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
    // The **source window** `[src_start, src_start + src_len)` is bounded for the same
    // reason as the timeline span: razor-split and chop add to `src_start`, and a sum
    // that leaves the range is a debug panic inside the editor's lock and a *wrapped*
    // source offset in release — the clip reads from somewhere the user never asked
    // for, with no diagnostic.
    if c.src_start.checked_add(c.src_len).is_none() {
        return Err(format!("clip '{}' source window overflows", c.id));
    }
    // A `source` is a **pool id**, and it is user-craftable: an arbitrary token in
    // the `host v1` log (`add_clip <track> <clip> <source> …`) and a `Clip` is
    // `Deserialize`. One carrying a path separator or whitespace is not an id, and
    // every op that sets a source goes through here — so a value that could address a
    // file outside the pool never enters the value, let alone the log. The lookup
    // refuses it too (`Pool::path_for`): this is the guard at the *writing* end, so a
    // saved session cannot carry a source that would only be refused when read.
    if !crate::pool::valid_id(&c.source) {
        return Err(format!(
            "clip '{}' source '{}' is not a pool id (a source is a file stem, not a path)",
            c.id, c.source
        ));
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
    ///
    /// `Err` — **naming the clip** — when some clip's span is not representable.
    /// The alternative is not a shorter answer but a wrong one: a wrapped sum is a
    /// panic in debug and an end that fell *below* the clip's start in release, so
    /// an `export` would write a mix missing its last clip. A `Timeline` is
    /// `Deserialize`, so a snapshot can hold such a clip even though no op admits
    /// one.
    pub fn end_frame(&self) -> Result<Frame, String> {
        let mut end = 0;
        for c in self.tracks.iter().flat_map(|t| t.clips.iter()) {
            let c_end = c
                .end()
                .ok_or_else(|| format!("clip '{}' span overflows the timeline", c.id))?;
            end = end.max(c_end);
        }
        Ok(end)
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
                // `end()` is checked: a span that cannot be represented has no
                // representable split frame either, and the `at_frame - c.at_frame` /
                // `c.src_len - split_in` below would be nonsense on one. Naming the
                // refusal beats a wrap, and a clip reaches an op without passing
                // `AddClip` (a `Timeline` is `Deserialize`).
                let end = c
                    .end()
                    .ok_or("razor-split: the clip's span is out of range")?;
                if *at_frame <= c.at_frame || *at_frame >= end {
                    return Err(format!(
                        "split frame {at_frame} must be strictly inside [{}, {})",
                        c.at_frame, end
                    ));
                }
                let split_in = at_frame - c.at_frame; // frames into the clip (guaranteed > 0)
                let mut left = c.clone();
                left.id = new_left.clone();
                left.src_len = split_in;
                left.fade_out = 0; // the split seam is hard (a crossfade is a later SetClipFade)
                // A fade that was legal for the **whole** clip can outlast the half it
                // lands on, and this op already rewrites the fades (the seam above), so
                // cap them to the halves they must fit — the same rule `Stretch` follows
                // when it repoints a clip at shorter material. Leaving them would admit a
                // clip `validate_clip` refuses, and `ArrangerNode::new` refuses the whole
                // *track* over one clip: a legal split that makes the session unplayable.
                left.fade_in = left.fade_in.min(left.src_len);
                let mut right = c.clone();
                right.id = new_right.clone();
                right.at_frame = *at_frame;
                right.src_len = c.src_len - split_in;
                right.fade_in = 0;
                right.fade_out = right.fade_out.min(right.src_len);
                if c.reversed {
                    // The clip's *first* frames are the region's **top**, so the left
                    // half takes the top and the right half the bottom — the mirror of
                    // the forward split (`clip_tests` pins both).
                    left.src_start = c
                        .src_start
                        .checked_add(right.src_len)
                        .ok_or("razor-split would move the left half past the source end")?;
                    right.src_start = c.src_start;
                } else {
                    left.src_start = c.src_start;
                    right.src_start = c
                        .src_start
                        .checked_add(split_in)
                        .ok_or("razor-split would move the right half past the source end")?;
                }
                validate_clip(&left).map_err(|e| format!("razor-split left half: {e}"))?;
                validate_clip(&right).map_err(|e| format!("razor-split right half: {e}"))?;
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
                // `times` is a **count of passes, not a factor**, and the region is fixed
                // by the first bake — `loop_len`, or the clip's own length on the first
                // loop. So a re-bake re-reads *the same* region and sets the total to
                // `times` passes of it: `times: 3` twice is three passes, not nine, and
                // `times: 1` is the single pass the clip was before its first bake.
                let region = c.loop_len.unwrap_or(c.src_len);
                let src_len = region
                    .checked_mul(*times as Frame)
                    .ok_or("loop region length overflows")?;
                c.loop_len = Some(region);
                c.src_len = src_len;
                // A re-bake with a **smaller** count shortens the clip, so a fade that
                // was legal for the longer clip can outlast the length it now covers:
                // cap it, exactly as `RazorSplit`, `ChopClip` and `Stretch` do when they
                // shrink `src_len`. Leaving it would admit a clip `validate_clip`
                // refuses — and `ArrangerNode::new` refuses the whole *track* over one,
                // so a logged op that made the session unplayable. (On a first bake the
                // clip only grows, and both caps are no-ops.)
                c.fade_in = c.fade_in.min(c.src_len);
                c.fade_out = c.fade_out.min(c.src_len.saturating_sub(c.fade_in));
                // `checked_mul` keeps the *length* representable; growing it can still
                // push the clip's timeline span or its source window out of range (a
                // clip placed near the end of the frame range, looped many times). Same
                // rule as every other geometry op: an op the log accepts must not leave
                // a clip the renderer refuses, because that wedges the whole track.
                validate_clip(c).map_err(|e| format!("loop region: {e}"))?;
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
                // A clip reaches an op without passing `AddClip` — a `Timeline` is
                // `Deserialize`, and a hand-built `Track` is a legal `&self` — so this
                // arm checks the clip it is about to re-tile instead of trusting an
                // admission it cannot see. It also *names* what is wrong: a source
                // window that cannot be represented, where the `checked_add`s below
                // would have reported it as a piece that would not fit.
                validate_clip(&c).map_err(|e| format!("chop: {e}"))?;
                let times_f = *times as Frame;
                if times_f > c.src_len {
                    return Err(format!(
                        "chop {} times exceeds src_len {}",
                        times_f, c.src_len
                    ));
                }
                // `times > src_len` is the only bound above, and `src_len` reaches
                // `i64::MAX` — so `times` (a `u32` straight off the wire) can ask for
                // billions of pieces. The loop below is `times` iterations, each
                // allocating a `Clip` with three heap `String`s, inside a `Timeline`
                // `apply` already cloned, followed by a `times`-element sort: a logged
                // op that hangs long before it aborts on the allocation. The op's own
                // grain is a slice per bar or finer, so a few thousand is a typo and
                // the refusal names the bound.
                if times_f > MAX_CHOP_SLICES {
                    return Err(format!(
                        "chop {} times exceeds the {MAX_CHOP_SLICES}-slice bound",
                        times_f
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
                    c.src_start
                        .checked_add(c.src_len)
                        .ok_or("chop would move a piece past the source end")?
                } else {
                    c.src_start
                };
                let mut at = c.at_frame;
                // The arrangement's clip ids, gathered **once**: ids are unique across
                // every track (`clip_id_exists` is the rule this preserves), so one set
                // answers both questions the piece loop asks — is this derived id
                // already taken, and has this loop already produced it — in O(1) each.
                // Asking `clip_id_exists` per piece was `times` × clips string
                // comparisons, which is what made an unbounded `times` quadratic.
                let mut ids: std::collections::HashSet<Id> = self
                    .tracks
                    .iter()
                    .flat_map(|t| t.clips.iter())
                    .map(|c| c.id.clone())
                    .collect();
                for i in 0..times_f {
                    let pid = format!("{prefix}.{i}");
                    if !ids.insert(pid.clone()) {
                        return Err(format!("chop derived id '{pid}' already exists or repeats"));
                    }
                    let slen = base + if i < rem { 1 } else { 0 };
                    // Preserve the clip's outer fades on the first/last piece (as
                    // RazorSplit does) so a chop doesn't silently remove audible
                    // crossfades; interior seams are hard (a SetClipFade follows).
                    // A piece is shorter than the clip the fade was legal for, so it is
                    // capped to the piece — an uncapped fade would make a piece the
                    // model refuses, and the renderer refuses the whole track over it.
                    if c.reversed {
                        src_at = src_at
                            .checked_sub(slen)
                            .ok_or("chop would move a piece before the source start")?;
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
                        fade_in: if i == 0 { c.fade_in.min(slen) } else { 0 },
                        fade_out: if i + 1 == times_f {
                            c.fade_out.min(slen)
                        } else {
                            0
                        },
                        gain: c.gain,
                        loop_len: None,
                        reversed: c.reversed,
                    });
                    // Walk the pieces up the source window and down the timeline, both
                    // `checked`: an unchecked `+` here is the same debug panic inside
                    // the editor's lock (and the same silent wrap in release) that the
                    // reversed arm's seed already refuses — one `reversed` boolean away
                    // from the fixture the first pass of this fix added.
                    if !c.reversed {
                        src_at = src_at
                            .checked_add(slen)
                            .ok_or("chop would move a piece past the source end")?;
                    }
                    at = at
                        .checked_add(slen)
                        .ok_or("chop would move a piece past the end of the timeline")?;
                }
                // The pieces tile the original clip's span and source window, so for a
                // clip the model accepted above (checked on the way in) this cannot
                // fail — it states that, so a future change to the piece construction
                // cannot quietly reintroduce a logged clip the renderer refuses.
                for p in &pieces {
                    validate_clip(p).map_err(|e| format!("chop piece '{}': {e}", p.id))?;
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
#[path = "tests/timeline.rs"]
mod tests;
