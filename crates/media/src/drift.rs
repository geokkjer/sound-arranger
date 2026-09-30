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
#[path = "tests/drift.rs"]
mod tests;
