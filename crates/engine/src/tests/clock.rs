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
