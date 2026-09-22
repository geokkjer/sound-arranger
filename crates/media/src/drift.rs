//! Input↔output device-clock drift reconciliation (Spike B note, `drift`).
//!
//! The input device delivers `in_rate` frames per second; the session runs at
//! `out_rate`. Over 20–30 minutes the difference is thousands of samples — the
//! recorded timeline must be in *session* frames and the buffer must not grow
//! without bound. [`DriftCompensator`] is a pure fractional-accumulator
//! linear-interpolation resampler: [`DriftCompensator::push_input`] accepts a
//! drifting input block, [`DriftCompensator::pull_output`] produces exactly
//! `out.len()` session frames. The ratio is passed in; real measurement from
//! cpal device frame counts is deferred.
//!
//! Wired into the capture demux (one per channel, P1.3.3+): the `Vec` buffering
//! is fine there — the demux is a background thread off the audio path. An
//! input-device-callback placement (the comment in an earlier revision) would
//! need a fixed ring for no-alloc.

pub struct DriftCompensator {
    /// input frames per output frame (in_rate / out_rate)
    ratio: f64,
    /// fractional position within the pending input
    pos: f64,
    pending: Vec<f32>,
}

impl DriftCompensator {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        DriftCompensator {
            ratio: in_rate as f64 / out_rate as f64,
            pos: 0.0,
            pending: Vec::new(),
        }
    }

    /// Feed a block of input frames (device clock). Buffered until consumed.
    /// Note: `extend_from_slice` may allocate — fine in the spike (driven by
    /// tests), but the compensator's natural home is the input-device
    /// callback, so Phase 1 must move its buffering into a fixed ring (kimi
    /// review nit).
    pub fn push_input(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
    }

    /// Pull exactly `out.len()` output frames (session clock). Returns the
    /// number written (== `out.len()` unless input ran out entirely — a
    /// starvation the caller counts). Linear interpolation with a *hold* at
    /// the tail (the last pending sample is used as its own neighbor, so every
    /// delivered sample is consumed); exact take-boundary timing is Phase 1.
    pub fn pull_output(&mut self, out: &mut [f32]) -> usize {
        let mut written = 0usize;
        while written < out.len() {
            let i = self.pos.floor() as usize;
            if i >= self.pending.len() {
                break;
            }
            let frac = (self.pos - i as f64) as f32;
            let a = self.pending[i];
            let b = if i + 1 < self.pending.len() {
                self.pending[i + 1]
            } else {
                a
            };
            out[written] = a + (b - a) * frac;
            written += 1;
            self.pos += self.ratio;
        }
        // Drop consumed input, keep the fractional remainder. Clamp to the
        // pending length: a fractional `pos` carried into a *short* batch (e.g.
        // the tail of a take) can floor above `pending.len()`, and a bare
        // `drain(..floor(pos))` would panic.
        let consumed = (self.pos.floor() as usize).min(self.pending.len());
        if consumed > 0 {
            self.pending.drain(..consumed);
            self.pos -= consumed as f64;
        }
        written
    }

    /// Buffered input not yet consumed (must stay bounded over long runs).
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 440 Hz tone generator at `rate` frames/second, as a device would see it.
    struct Tone {
        rate: u32,
        phase: f64,
    }

    impl Tone {
        fn next_block(&mut self, n: usize) -> Vec<f32> {
            (0..n)
                .map(|_| {
                    let s = (std::f64::consts::TAU * self.phase).sin() as f32 * 0.5;
                    self.phase += 440.0 / self.rate as f64;
                    s
                })
                .collect()
        }
    }

    /// A device whose clock is `in_rate` Hz delivering to a session at
    /// `out_rate` Hz: per output block it produces a *fractional* number of
    /// frames on average (sometimes 512, sometimes 513).
    struct Device {
        in_rate: u32,
        out_rate: u32,
        acc: f64,
        fed: u64,
        tone: Tone,
    }

    impl Device {
        fn new(in_rate: u32, out_rate: u32) -> Self {
            Device {
                in_rate,
                out_rate,
                acc: 0.0,
                fed: 0,
                tone: Tone {
                    rate: in_rate,
                    phase: 0.0,
                },
            }
        }

        /// Feed the compensator the frames the device delivered for one output
        /// block.
        fn feed_block(&mut self, c: &mut DriftCompensator) {
            self.acc += self.in_rate as f64 / self.out_rate as f64 * 512.0;
            let want = self.acc.floor() as u64 - self.fed;
            if want > 0 {
                c.push_input(&self.tone.next_block(want as usize));
                self.fed += want;
            }
        }
    }

    /// Run `blocks` output blocks with the device delivering exactly
    /// in_rate/out_rate frames per output frame. Returns (total pulled,
    /// pending left, min pull, max pull).
    fn run(
        c: &mut DriftCompensator,
        dev: &mut Device,
        blocks: usize,
    ) -> (usize, usize, usize, usize) {
        let mut out = vec![0.0f32; 512];
        let (mut total, mut min_pull, mut max_pull) = (0usize, usize::MAX, 0usize);
        for _ in 0..blocks {
            dev.feed_block(c);
            let n = c.pull_output(&mut out);
            min_pull = min_pull.min(n);
            max_pull = max_pull.max(n);
            total += n;
        }
        (total, c.pending_len(), min_pull, max_pull)
    }

    #[test]
    fn output_is_exactly_session_length() {
        let mut c = DriftCompensator::new(48_001, 48_000);
        let mut dev = Device::new(48_001, 48_000);
        let (total, pending, min_pull, max_pull) = run(&mut c, &mut dev, 1000);
        // 512_000 session frames need 512_010.67 input frames; the device
        // delivered the integer floors (~512_010) — the take ends short by at
        // most the fractional remainder, never mid-block while input flows.
        assert!(total >= 512_000 - 2, "total {total}");
        assert_eq!(max_pull, 512, "full blocks while input flows");
        assert!(min_pull >= 511, "at most a fractional tail, min {min_pull}");
        assert!(pending < 4, "pending must stay bounded, got {pending}");
        // Steady state resumes with full blocks: the tail was a take boundary,
        // not starvation.
        dev.feed_block(&mut c);
        let mut out = vec![0.0f32; 512];
        assert_eq!(c.pull_output(&mut out), 512);
    }

    /// Drift must not shift the pitch: a 440 Hz input recorded into session
    /// frames stays ~440 Hz (zero-crossing count over a long window).
    #[test]
    fn drift_preserves_pitch() {
        let mut c = DriftCompensator::new(48_001, 48_000);
        let mut dev = Device::new(48_001, 48_000);
        let seconds = 10u32;
        let mut out = vec![0.0f32; 512];
        let mut recorded = Vec::with_capacity(48_000 * seconds as usize);
        for _ in 0..(48_000 / 512 * seconds) {
            dev.feed_block(&mut c);
            let n = c.pull_output(&mut out);
            recorded.extend_from_slice(&out[..n]);
        }
        let crossings = recorded
            .windows(2)
            .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
            .count();
        let expected = 440.0 * seconds as f32 * 2.0; // 2 crossings per cycle
        assert!(
            (crossings as f32 - expected).abs() < expected * 0.01,
            "pitch drifted: {crossings} crossings vs ~{expected}"
        );
    }

    /// The reverse ratio (output clock faster than input) must also stay
    /// bounded and exact.
    #[test]
    fn reverse_ratio_stays_bounded() {
        let mut c = DriftCompensator::new(47_999, 48_000);
        let mut dev = Device::new(47_999, 48_000);
        let (total, pending, min_pull, max_pull) = run(&mut c, &mut dev, 1000);
        assert!(total >= 512_000 - 2, "total {total}");
        assert_eq!(max_pull, 512);
        assert!(min_pull >= 511, "min {min_pull}");
        assert!(pending < 4, "pending must stay bounded, got {pending}");
    }
}
