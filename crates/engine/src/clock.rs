//! Core piece 1 — the clock: sample-accurate master time, an editable tempo/meter
//! map, and a sample-accurate scheduling queue (minimal-core note §1).
//!
//! Musical position (beats) is always *derived* through the [`TempoMap`] from
//! absolute sample frames, so tempo edits never corrupt stored positions (the
//! log's time-basis rule).

use std::ops::Range;

/// A tempo/meter map: ascending `(start_frame, bpm, beats_per_bar)` segments.
#[derive(Debug, Clone)]
pub struct TempoMap {
    sample_rate: u32,
    segments: Vec<TempoSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoSegment {
    pub start_frame: u64,
    pub bpm: f64,
    pub beats_per_bar: u32,
}

impl TempoMap {
    pub fn new(sample_rate: u32, bpm: f64, beats_per_bar: u32) -> Self {
        TempoMap {
            sample_rate,
            segments: vec![TempoSegment {
                start_frame: 0,
                bpm,
                beats_per_bar,
            }],
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn segments(&self) -> &[TempoSegment] {
        &self.segments
    }

    /// Append a tempo change at an absolute frame. Segments must be appended in
    /// ascending `start_frame` order.
    pub fn push(&mut self, start_frame: u64, bpm: f64, beats_per_bar: u32) {
        debug_assert!(
            self.segments
                .last()
                .is_none_or(|s| start_frame >= s.start_frame),
            "tempo segments must be appended in ascending order"
        );
        self.segments.push(TempoSegment {
            start_frame,
            bpm,
            beats_per_bar,
        });
    }

    fn segment_at(&self, frame: u64) -> &TempoSegment {
        let idx = self.segments.partition_point(|s| s.start_frame <= frame) - 1;
        &self.segments[idx]
    }

    pub fn tempo_at(&self, frame: u64) -> f64 {
        self.segment_at(frame).bpm
    }

    pub fn meter_at(&self, frame: u64) -> u32 {
        self.segment_at(frame).beats_per_bar
    }

    /// Fractional beat position at an absolute frame (0.0 at frame 0).
    pub fn beat_at(&self, frame: u64) -> f64 {
        let mut beat = 0.0f64;
        for (i, seg) in self.segments.iter().enumerate() {
            if frame <= seg.start_frame {
                break;
            }
            let end = self
                .segments
                .get(i + 1)
                .map(|s| s.start_frame)
                .unwrap_or(u64::MAX)
                .min(frame);
            let frames = end - seg.start_frame;
            beat += frames as f64 * seg.bpm / 60.0 / self.sample_rate as f64;
        }
        beat
    }

    /// Absolute frame at a fractional beat position (inverse of [`beat_at`]).
    ///
    /// A beat the map cannot reach — only a tempo so slow that even the final
    /// open-ended segment spans less than one tick — saturates at `u64::MAX`
    /// rather than falling back to a *real* frame: the honest answer is "past
    /// the end of time", and a caller that walks frames (the clock-out node
    /// looking for its next tick) terminates on it instead of spinning on a
    /// constant.
    ///
    /// **The frame domain is total, and monotone in the beat.** Every `f64` beat
    /// — negative, `NaN`, infinite, absurd — answers with some `u64` and nothing
    /// on this path panics: the `f64` arithmetic cannot overflow, Rust's
    /// `f64 → u64` casts saturate rather than wrap, and the one integer sum is
    /// `saturating_add`. Two routes reach `u64::MAX` and they agree — no segment
    /// spans the beat, or the final open-ended segment's own frames run off the
    /// top — so the answer is never *below* the segment it was placed in, and a
    /// beat the map can place is never placed after the segment that follows it.
    /// That second half is what the clock-out node's doubling-then-binary search
    /// over tick frames rests on.
    pub fn frame_at(&self, beat: f64) -> u64 {
        let mut remaining = beat;
        for (i, seg) in self.segments.iter().enumerate() {
            let seg_frames = self
                .segments
                .get(i + 1)
                .map(|s| s.start_frame - seg.start_frame)
                .unwrap_or(u64::MAX);
            let seg_beats = seg_frames as f64 * seg.bpm / 60.0 / self.sample_rate as f64;
            if remaining < seg_beats {
                let frames = (remaining * 60.0 / seg.bpm * self.sample_rate as f64).round() as u64;
                // Saturating, because a segment can start within one beat's worth
                // of frames of the top of the range — `Engine::seek` takes any
                // `u64`, and `push_tempo` appends a segment at the frame it left
                // the clock on. Unchecked, a beat inside that segment is a debug
                // panic on the render thread and, in release, a frame near zero: a
                // tick moved back to the start of the session. Only the final
                // open-ended segment can reach the sum at all (a closed segment's
                // `frames` never exceeds its own length), so saturating cannot
                // cost the monotonicity the doc above claims.
                return seg.start_frame.saturating_add(frames);
            }
            remaining -= seg_beats;
        }
        // The loop falls through only when no segment spans the beat: at a
        // tempo slow enough that `seg_beats` is below one tick, every beat past
        // the first is unreachable. Saturate — a walk over frames stays finite.
        u64::MAX
    }
}

/// The sample-accurate master clock. No plugin owns time; every plugin agrees on
/// *when* by reading this.
#[derive(Debug, Clone)]
pub struct Clock {
    pub sample_rate: u32,
    frame: u64,
    pub tempo_map: TempoMap,
}

impl Clock {
    pub fn new(sample_rate: u32, bpm: f64, beats_per_bar: u32) -> Self {
        Clock {
            sample_rate,
            frame: 0,
            tempo_map: TempoMap::new(sample_rate, bpm, beats_per_bar),
        }
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Saturating, for the same reason [`TempoMap::frame_at`]'s sum is:
    /// [`Self::seek_to`] (and `Engine::seek`) place the clock at *any* `u64`,
    /// and the render loop advances by the block length every block — so a block
    /// that crosses the top of the range is a debug panic on the render thread,
    /// and in release a wrap to frame 0 that restarts the timeline from the
    /// beginning in the middle of a render.
    pub fn advance(&mut self, frames: u64) {
        self.frame = self.frame.saturating_add(frames);
    }

    /// **Place the clock at `frame` without rendering the frames in between.**
    ///
    /// This is only sound when the graph's state at `frame` is already correct: a
    /// clip reader derives its read position from the block's frame
    /// (`ArrangerNode::render` is a pure function of `block.frame`), so readers need
    /// nothing; a *stateful* effect (a compressor's envelope, a delay line) does, and
    /// the caller owes it a warm-up render ([`crate::Engine::seek`] leaves that to the
    /// host, which renders a short run-in and proves the equality).
    pub fn seek_to(&mut self, frame: u64) {
        self.frame = frame;
    }

    pub fn seconds(&self) -> f64 {
        self.frame as f64 / self.sample_rate as f64
    }

    pub fn beat(&self) -> f64 {
        self.tempo_map.beat_at(self.frame)
    }

    /// Push a tempo change at the *current* frame (caller logs it first).
    pub fn push_tempo(&mut self, bpm: f64, beats_per_bar: u32) {
        self.tempo_map.push(self.frame, bpm, beats_per_bar);
    }
}

/// A sample-accurate scheduling queue: one-shot events delivered at an exact
/// absolute frame. The render thread only drains; scheduling happens on the
/// control side.
#[derive(Debug, Clone, Default)]
pub struct Scheduler<T> {
    /// (frame, payload), kept sorted by frame.
    entries: Vec<(u64, T)>,
}

impl<T> Scheduler<T> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn schedule(&mut self, frame: u64, payload: T) {
        let at = self.entries.partition_point(|(f, _)| *f <= frame);
        self.entries.insert(at, (frame, payload));
    }

    /// Frame of the next entry — the render loop interleaves blocks with events
    /// by peeking this.
    pub fn peek_frame(&self) -> Option<u64> {
        self.entries.first().map(|(f, _)| *f)
    }

    /// Pop the next entry, preserving schedule order. O(n) on the control side
    /// is fine (few events); the render loop pops one at a time.
    pub fn pop(&mut self) -> Option<T> {
        if self.entries.is_empty() {
            return None;
        }
        Some(self.entries.remove(0).1)
    }

    /// Lazy drain of entries at or before `frame`, in schedule order — no
    /// allocation (the render loop may use this via peeking instead).
    pub fn drain_until(&mut self, frame: u64) -> impl Iterator<Item = T> + '_ {
        let at = self.entries.partition_point(|(f, _)| *f <= frame);
        self.entries.drain(..at).map(|(_, p)| p)
    }
}

/// Trigger frames of a pattern inside `block` — the pull-based scheduling query
/// used by generators (pure; no allocation).
pub trait PatternQuery {
    fn for_each_trigger(&self, block: Range<u64>, tempo: &TempoMap, emit: &mut dyn FnMut(u64));
}

#[cfg(test)]
#[path = "tests/clock.rs"]
mod tests;
