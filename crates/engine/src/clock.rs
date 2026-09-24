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
                return seg.start_frame + frames;
            }
            remaining -= seg_beats;
        }
        // Unreachable: the final open-ended segment covers any finite beat.
        self.segments
            .last()
            .expect("tempo map never empty")
            .start_frame
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

    pub fn advance(&mut self, frames: u64) {
        self.frame += frames;
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
