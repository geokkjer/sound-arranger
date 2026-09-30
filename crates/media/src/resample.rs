//! Sample-rate conversion for material **entering the session** (P1.3, `resample`).
//!
//! The session has one rate (48 kHz by default) and the media pool holds its
//! sources at that rate, so an arrangement clip is a straight read and the
//! timeline has a single frame domain. Capture reaches that invariant in real
//! time, off the audio path, with the [`crate::drift::DriftCompensator`]; an
//! **imported** file reaches it here — once, at the pool boundary, with a
//! band-limited (windowed-sinc) resampler.
//!
//! Why not the drift compensator's linear interpolation: 44.1 kHz → 48 kHz is
//! upsampling, so nothing aliases, but linear interpolation's `sinc²` droop is
//! ≈ -6 dB at 20 kHz and ≈ -1.5 dB at 10 kHz — audibly dull on cymbals and air,
//! and this is a music tool. A 64-tap Kaiser-windowed sinc is ≈ -80 dB
//! stopband with a flat passband to ~20 kHz, for a cost paid once per import.
//!
//! The converter is a **fractional-delay kernel table**: a sinc kernel cut off at
//! `min(in, out)/2` (so it both removes images when upsampling and anti-aliases
//! when downsampling), sampled at [`PHASES`] fractional offsets and normalized so
//! every phase sums to exactly 1 (unity DC gain at every phase). Each output
//! sample is a [`TAPS`]-tap dot product at its fractional input position — no
//! delay, no allocation per sample, and the same result whatever the block size,
//! which is what makes a converted take byte-reproducible. A file starts at
//! t = 0 with silence behind it, so the first and last few samples ring in and
//! out of the kernel (≲ 1.5 ms at 48 kHz — inaudible, and it never touches the
//! interior of a take).
//!
//! Equal rates are a **bit-exact passthrough** (the same rule the capture path
//! follows), so a conformed pool never re-quantizes a source that already fits.

use std::f64::consts::PI;

/// Taps per output sample (the kernel length). 128 taps at β = 8.6 gives a
/// ~-80 dB stopband with a transition band of ~0.039 cycles/sample — flat well
/// past 20 kHz and fully attenuated ~2 kHz above the cutoff, which is what a
/// 96→48 kHz import needs. The cost is paid once, at import, not per playback.
const TAPS: usize = 128;
/// Fractional phases in the kernel table. The error of linearly interpolating
/// between adjacent rows is ~-100 dB, far below the kernel's own ripple.
const PHASES: usize = 512;
/// Kaiser window shape: ≈ -80 dB sidelobes.
const BETA: f64 = 8.6;

/// The zero-order modified Bessel function of the first kind, by its power
/// series (converges in a handful of terms for the β used here).
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut sum = 1.0f64;
    let mut term = 1.0f64;
    for k in 1..64u32 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < 1e-17 * sum {
            break;
        }
    }
    sum
}

/// The precomputed polyphase kernel: `PHASES + 1` rows (the extra row is the
/// `f = 1` endpoint for interpolation) × [`TAPS`] taps.
struct Kernel {
    table: Vec<f32>,
}

impl Kernel {
    /// Build the kernel for `in_rate → out_rate`. `fc` is the cutoff in cycles
    /// per *input* sample: the lower of the two Nyquist limits, so the same
    /// filter is an anti-imaging filter when upsampling and an anti-aliasing
    /// filter when downsampling.
    fn new(in_rate: u32, out_rate: u32) -> Self {
        let fc = 0.5 * (out_rate as f64 / in_rate as f64).min(1.0);
        let h = TAPS as f64 / 2.0;
        let i0_beta = bessel_i0(BETA);
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];

        for phase in 0..=PHASES {
            let f = phase as f64 / PHASES as f64;
            let row = &mut table[phase * TAPS..(phase + 1) * TAPS];
            let mut sum = 0.0f64;
            for (tap, slot) in row.iter_mut().enumerate() {
                // The kernel argument for input sample `i + k` is `f - k`; tap 0
                // reads `k = -(h - 1)`, so the argument is `f - tap + h - 1`.
                let x = f - (tap as f64 - (h - 1.0));
                let window = if x.abs() >= h {
                    0.0
                } else {
                    let r = x / h;
                    bessel_i0(BETA * (1.0 - r * r).sqrt()) / i0_beta
                };
                // 2·fc·sinc(2·fc·x) = sin(2π·fc·x) / (π·x)
                let sinc = if x == 0.0 {
                    2.0 * fc
                } else {
                    (2.0 * fc * x * PI).sin() / (PI * x)
                };
                let value = sinc * window;
                *slot = value as f32;
                sum += value;
            }
            // Unity DC gain at every phase: the fractional position changes the
            // tap weights, not the gain.
            if sum != 0.0 {
                for slot in row.iter_mut() {
                    *slot = (*slot as f64 / sum) as f32;
                }
            }
        }
        Kernel { table }
    }
}

/// A streaming mono sample-rate converter.
///
/// ```no_run
/// # use media::resample::Resampler;
/// let mut rs = Resampler::new(44_100, 48_000).unwrap();
/// let mut out = Vec::new();
/// rs.process(&[0.0f32; 1024], &mut out);
/// rs.flush(&mut out);
/// assert_eq!(out.len() as u64, rs.frames_out(1024));
/// ```
pub struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Output frames per input frame.
    ratio: f64,
    /// `None` when `in_rate == out_rate` (a bit-exact passthrough).
    kernel: Option<Kernel>,
    /// Pending input; `hist[0]` is absolute input frame `base`.
    hist: Vec<f32>,
    base: u64,
    /// Absolute input position of the next output sample.
    pos: f64,
    /// Input frames fed so far.
    fed: u64,
    /// Output frames produced so far.
    produced: u64,
    flushed: bool,
}

impl Resampler {
    /// A converter `in_rate → out_rate`. Both rates must be non-zero.
    pub fn new(in_rate: u32, out_rate: u32) -> Result<Self, String> {
        if in_rate == 0 || out_rate == 0 {
            return Err(format!(
                "sample rate must be non-zero (got {in_rate} → {out_rate})"
            ));
        }
        Ok(Resampler {
            step: in_rate as f64 / out_rate as f64,
            ratio: out_rate as f64 / in_rate as f64,
            kernel: (in_rate != out_rate).then(|| Kernel::new(in_rate, out_rate)),
            hist: Vec::new(),
            base: 0,
            pos: 0.0,
            fed: 0,
            produced: 0,
            flushed: false,
        })
    }

    /// Whether this is a bit-exact passthrough (equal rates).
    pub fn is_passthrough(&self) -> bool {
        self.kernel.is_none()
    }

    /// The exact output length for `frames_in` input frames — the same rounding
    /// `process`/`flush` produce, so a caller can size a buffer in advance.
    pub fn frames_out(&self, frames_in: u64) -> u64 {
        (frames_in as f64 * self.ratio).round() as u64
    }

    /// Feed `input` (mono frames) and append every output frame that is fully
    /// determined by the input so far. The tail (whose window runs past the last
    /// input sample) is produced by [`Resampler::flush`], so feeding the same
    /// material in different block sizes cannot change the output.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        assert!(!self.flushed, "process after flush");
        if self.is_passthrough() {
            out.extend_from_slice(input);
            self.fed += input.len() as u64;
            self.produced += input.len() as u64;
            return;
        }
        if !input.is_empty() {
            self.hist.extend_from_slice(input);
            self.fed += input.len() as u64;
        }
        let target = self.frames_out(self.fed);
        while self.produced < target && self.ready() {
            let value = self.sample(false);
            out.push(value);
            self.produced += 1;
            self.pos += self.step;
        }
        self.trim();
    }

    /// Produce the remaining frames, treating input past the end as silence.
    /// Call once, after the last [`Resampler::process`].
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.flushed {
            return;
        }
        if !self.is_passthrough() {
            let target = self.frames_out(self.fed);
            while self.produced < target {
                let value = self.sample(true);
                out.push(value);
                self.produced += 1;
                self.pos += self.step;
            }
        }
        self.flushed = true;
    }

    /// Whether the full kernel window for the next output sample is available.
    fn ready(&self) -> bool {
        let last = self.pos.floor() + (TAPS as f64 / 2.0);
        last < self.fed as f64
    }

    /// One output sample at `pos`: a `TAPS`-tap dot product, linearly
    /// interpolated between the two nearest kernel phases. With `pad`, input
    /// past the end reads as silence (the flush tail).
    fn sample(&self, pad: bool) -> f32 {
        let kernel = self
            .kernel
            .as_ref()
            .expect("sample() is not called in passthrough");
        let i = self.pos.floor();
        let first = i - (TAPS as f64 / 2.0 - 1.0);
        let frac = (self.pos - i) as f32;
        let phase = frac * PHASES as f32;
        // A fractional part that rounds up to 1.0 in `f32` would index one row
        // past the table; clamp and let the blend reach the `f = 1` row instead.
        let p0 = (phase.floor() as usize).min(PHASES - 1);
        let blend = (phase - p0 as f32).clamp(0.0, 1.0);
        let row0 = &kernel.table[p0 * TAPS..(p0 + 1) * TAPS];
        let row1 = &kernel.table[(p0 + 1) * TAPS..(p0 + 2) * TAPS];

        let mut acc = 0.0f64;
        for tap in 0..TAPS {
            let weight = row0[tap] + (row1[tap] - row0[tap]) * blend;
            let index = first as i64 + tap as i64;
            let value = if index < self.base as i64 {
                0.0
            } else if index >= self.fed as i64 {
                if pad {
                    0.0
                } else {
                    // `ready()` guarantees this cannot happen; keep the sample
                    // well-defined rather than reading stale or out-of-range data.
                    break;
                }
            } else {
                self.hist[(index - self.base as i64) as usize]
            };
            acc += weight as f64 * value as f64;
        }
        acc as f32
    }

    /// Drop input the kernel can no longer reach (bounded memory over long runs).
    fn trim(&mut self) {
        let keep_from = (self.pos.floor() - (TAPS as f64 / 2.0 - 1.0)).max(0.0) as u64;
        if keep_from > self.base {
            let drop = ((keep_from - self.base) as usize).min(self.hist.len());
            self.hist.drain(..drop);
            self.base += drop as u64;
        }
    }
}

/// One-shot conversion of a whole buffer (tests, small material). For files use
/// a streaming [`Resampler`] so a long take never lands in memory twice.
pub fn resample_mono(input: &[f32], in_rate: u32, out_rate: u32) -> Result<Vec<f32>, String> {
    let mut rs = Resampler::new(in_rate, out_rate)?;
    let mut out = Vec::with_capacity(rs.frames_out(input.len() as u64) as usize);
    rs.process(input, &mut out);
    rs.flush(&mut out);
    Ok(out)
}

#[cfg(test)]
#[path = "tests/resample.rs"]
mod tests;
