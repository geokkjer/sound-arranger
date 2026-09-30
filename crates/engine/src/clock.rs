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
mod tests {
    use super::*;

    #[test]
    fn tempo_roundtrip() {
        let map = TempoMap::new(48_000, 120.0, 4);
        for beat in [0.0, 0.5, 1.0, 7.25, 42.0] {
            let frame = map.frame_at(beat);
            let back = map.beat_at(frame);
            assert!(
                (back - beat).abs() < 1e-3,
                "beat {beat} → frame {frame} → {back}"
            );
        }
    }

    #[test]
    fn tempo_change_respects_frames() {
        let mut map = TempoMap::new(48_000, 120.0, 4);
        // one bar at 120bpm = 2s = 96_000 samples; then slow to 60bpm.
        map.push(96_000, 60.0, 4);
        assert_eq!(map.tempo_at(95_999), 120.0);
        assert_eq!(map.tempo_at(96_000), 60.0);
        // beat 4 (end of bar 1) must land exactly at the change frame.
        assert_eq!(map.frame_at(4.0), 96_000);
        // a beat after the change: 1 more beat at 60bpm = 48_000 samples.
        assert_eq!(map.frame_at(5.0), 144_000);
    }

    /// A beat no segment can reach saturates instead of answering with a real
    /// frame. At a tempo slow enough that the open-ended segment spans less than
    /// one tick, the old fallback returned the map's last start frame — `0` —
    /// so `frame_at` was constant, and a caller walking tick frames (the
    /// clock-out node) spun forever looking for a tick that never advanced.
    #[test]
    fn frame_at_saturates_on_a_tempo_too_slow_to_reach_the_beat() {
        let map = TempoMap::new(48_000, 1e-15, 4);
        assert_eq!(map.frame_at(0.0), 0);
        // One tick (1/24 beat) already needs more frames than u64 can hold.
        assert_eq!(map.frame_at(1.0 / 24.0), u64::MAX);
        // The walk a tick scheduler performs therefore terminates, and says so
        // rather than sitting on a constant.
        let mut n = 0u64;
        while map.frame_at(n as f64 / 24.0) < 48_000 {
            n += 1;
            assert!(
                n < 8,
                "frame_at did not advance: tick {n} is still in range"
            );
        }
        assert_eq!(n, 1);
    }

    /// A seek near the top of the frame range, then a tempo change, is the
    /// render-path overflow this saturates for. The new segment starts four
    /// frames below `u64::MAX` and one beat at 60 bpm is 48 000 of them, so the
    /// answer for a beat inside it summed `start_frame + frames` past the top:
    /// a debug panic on the render thread, and in release `47_995` — a tick
    /// moved back to the start of the session, which is the silent half of the
    /// same bug.
    #[test]
    fn frame_at_saturates_the_sum_when_a_seek_puts_a_tempo_near_the_top() {
        let mut clock = Clock::new(48_000, 120.0, 4);
        clock.seek_to(u64::MAX - 4);
        clock.push_tempo(60.0, 4);

        // The beat at the seek frame, and one beat past it — inside the new
        // segment, and 48 000 frames above the top of the range.
        let at_seek = clock.beat();
        let past = at_seek + 1.0;
        assert_eq!(
            clock.tempo_map.frame_at(past),
            u64::MAX,
            "a beat past the top of the frame range must saturate, not wrap"
        );
        // Monotone, and never below the segment the beat was placed in: the
        // clock-out node's search over tick frames is only sound because of
        // both.
        let mut previous = clock.tempo_map.frame_at(at_seek);
        assert_eq!(previous, u64::MAX - 4, "the seek frame itself is placeable");
        for beat in [at_seek + 0.5, at_seek + 1.0, at_seek + 2.0, at_seek + 1e6] {
            let frame = clock.tempo_map.frame_at(beat);
            assert!(
                frame >= previous,
                "frame_at went backwards: beat {beat} answered {frame} after {previous}"
            );
            assert!(
                frame >= u64::MAX - 4,
                "beat {beat} answered {frame}, inside the segment it cannot be"
            );
            previous = frame;
        }
        // The round trip that a wrapped frame breaks: a beat *after* the frame a
        // seek placed must never answer before it.
        assert!(clock.tempo_map.frame_at(past) >= clock.frame());
    }

    /// The claim `frame_at`'s doc now makes, tested rather than asserted: the
    /// frame domain is total. Every `f64` beat answers, a beat that is not a
    /// number at all is answered rather than feared, and a map that can place no
    /// beat whatsoever still answers.
    #[test]
    fn frame_at_is_total_over_the_beat_domain() {
        let map = TempoMap::new(48_000, 120.0, 4);
        // The beats this map can place.
        assert_eq!(map.frame_at(0.0), 0);
        assert_eq!(map.frame_at(1.0), 24_000);
        // The beats it cannot: past the end of the open-ended segment, and the
        // two values that are not a beat at all. All three saturate.
        for beat in [f64::NAN, f64::INFINITY, 1e300] {
            assert_eq!(map.frame_at(beat), u64::MAX, "beat {beat}");
        }
        // A negative beat reaches frame 0 through the saturating cast rather than
        // wrapping to the top of the range.
        for beat in [f64::NEG_INFINITY, -1.0, -1e300] {
            assert_eq!(map.frame_at(beat), 0, "beat {beat}");
        }
        // A map that can place no beat at all — a zero, a negative and a `NaN`
        // tempo, each of which makes every `seg_beats` zero, negative or
        // unordered — still answers, and says "past the end of time" every time.
        for bpm in [0.0, -120.0, f64::NAN] {
            let mut flat = TempoMap::new(48_000, bpm, 4);
            flat.push(96_000, bpm, 4);
            for beat in [0.0, 0.5, 1.0, 4.0, 1e9, f64::NAN, f64::INFINITY] {
                assert_eq!(flat.frame_at(beat), u64::MAX, "bpm {bpm}, beat {beat}");
            }
        }
    }

    /// The clock's own counter, for the same reason: a seek near the top of the
    /// range followed by one block's worth of `advance` is the same overflow one
    /// layer down, and the render loop calls it on every block. Unchecked it is a
    /// debug panic on the render thread and, in release, a wrap to frame 0 that
    /// restarts the timeline in the middle of a render.
    #[test]
    fn advance_saturates_at_the_top_of_the_frame_range() {
        let mut clock = Clock::new(48_000, 120.0, 4);
        clock.seek_to(u64::MAX - 4);
        clock.advance(8);
        assert_eq!(clock.frame(), u64::MAX, "a block past the top saturates");
        // Monotone, and a further block does not move it back.
        clock.advance(512);
        assert_eq!(clock.frame(), u64::MAX);
        // And the tempo map still reads the clock it is asked about.
        assert_eq!(clock.tempo_map.tempo_at(u64::MAX - 1), 120.0);
    }

    #[test]
    fn scheduler_delivers_exactly_at_frame() {
        let mut s = Scheduler::new();
        s.schedule(100, "a");
        s.schedule(50, "b");
        s.schedule(75, "c");
        assert_eq!(s.peek_frame(), Some(50));
        assert!(s.drain_until(49).collect::<Vec<_>>().is_empty());
        assert_eq!(s.drain_until(50).collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(s.drain_until(74).collect::<Vec<_>>(), Vec::<&str>::new());
        assert_eq!(s.drain_until(100).collect::<Vec<_>>(), vec!["c", "a"]);
        assert!(s.is_empty());
        assert_eq!(s.peek_frame(), None);
    }
}
