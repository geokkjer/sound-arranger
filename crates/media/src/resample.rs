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
mod tests {
    use super::*;

    /// A band-limited sine at `freq` Hz, `frames` long at `rate`.
    fn tone(freq: f64, rate: u32, frames: usize, amplitude: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                (std::f64::consts::TAU * freq * i as f64 / rate as f64).sin() as f32 * amplitude
            })
            .collect()
    }

    /// The energy of everything in `out` that is *not* a sinusoid at `freq`,
    /// in dB relative to the total. The fit absorbs amplitude and phase, so this
    /// measures droop-free distortion, not level.
    fn residual_db(out: &[f32], rate: u32, freq: f64) -> f64 {
        let n = out.len() as f64;
        let (mut ss, mut sc, mut cc, mut ys, mut yc, mut total) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, &y) in out.iter().enumerate() {
            let t = std::f64::consts::TAU * freq * i as f64 / rate as f64;
            let (s, c) = (t.sin(), t.cos());
            ss += s * s;
            sc += s * c;
            cc += c * c;
            ys += y as f64 * s;
            yc += y as f64 * c;
            total += (y as f64) * (y as f64);
        }
        // Solve the 2×2 normal equations for a·sin + b·cos.
        let det = ss * cc - sc * sc;
        let a = (ys * cc - yc * sc) / det;
        let b = (yc * ss - ys * sc) / det;
        let mut err = 0.0;
        for (i, &y) in out.iter().enumerate() {
            let t = std::f64::consts::TAU * freq * i as f64 / rate as f64;
            let fit = a * t.sin() + b * t.cos();
            err += (y as f64 - fit) * (y as f64 - fit);
        }
        assert!(total > 0.0, "no output energy");
        let _ = n;
        10.0 * (err / total).log10()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn equal_rates_are_a_bit_exact_passthrough() {
        let input = tone(440.0, 48_000, 1000, 0.7);
        let mut rs = Resampler::new(48_000, 48_000).expect("rates");
        assert!(rs.is_passthrough());
        let mut out = Vec::new();
        rs.process(&input[..400], &mut out);
        rs.process(&input[400..], &mut out);
        rs.flush(&mut out);
        assert_eq!(out, input, "equal rates must not touch the samples");
    }

    #[test]
    fn forty_four_one_to_forty_eight_is_exactly_the_right_length() {
        let mut rs = Resampler::new(44_100, 48_000).expect("rates");
        assert_eq!(rs.frames_out(44_100), 48_000);
        assert_eq!(rs.frames_out(1), 1); // round(1.088)
        let input = tone(440.0, 44_100, 44_100, 0.5);
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        assert!(
            out.len() < 48_000,
            "the tail needs the flush: {}",
            out.len()
        );
        rs.flush(&mut out);
        assert_eq!(out.len(), 48_000, "one second in is one second out");
        // Flush is idempotent and never produces extra frames.
        rs.flush(&mut out);
        assert_eq!(out.len(), 48_000);
    }

    #[test]
    fn empty_input_produces_nothing() {
        let out = resample_mono(&[], 44_100, 48_000).expect("rates");
        assert!(out.is_empty());
    }

    #[test]
    fn a_zero_rate_is_refused() {
        assert!(Resampler::new(0, 48_000).is_err());
        assert!(Resampler::new(48_000, 0).is_err());
    }

    #[test]
    fn pitch_survives_the_conversion() {
        // 5 s of 440 Hz: the zero-crossing count is the cheapest pitch probe and
        // is exactly the assertion the drift compensator uses.
        let seconds = 5u32;
        let input = tone(440.0, 44_100, 44_100 * seconds as usize, 0.5);
        let out = resample_mono(&input, 44_100, 48_000).expect("rates");
        let crossings = out
            .windows(2)
            .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
            .count();
        let expected = 440.0 * seconds as f32 * 2.0;
        assert!(
            (crossings as f32 - expected).abs() < expected * 0.01,
            "pitch moved: {crossings} crossings vs ~{expected}"
        );
    }

    #[test]
    fn a_constant_stays_constant() {
        // Unity DC gain per phase: this is what makes gain/DC survive. The very
        // first and last samples ring in/out of the centred kernel (silence
        // behind t = 0 and past the end), so the plateau is what must be exact.
        let input = vec![0.6f32; 44_100];
        let out = resample_mono(&input, 44_100, 48_000).expect("rates");
        for (i, v) in out.iter().enumerate().skip(TAPS).take(out.len() - 2 * TAPS) {
            assert!((v - 0.6).abs() < 1e-4, "sample {i} drifted: {v}");
        }
    }

    #[test]
    fn the_passband_is_flat_and_distortion_free() {
        // 1 kHz: far from both Nyquists — the converter must be transparent.
        let input = tone(1000.0, 44_100, 22_050, 0.5); // 0.5 s = 500 whole cycles
        let out = resample_mono(&input, 44_100, 48_000).expect("rates");
        let db = residual_db(&out, 48_000, 1000.0);
        assert!(db < -60.0, "1 kHz residual {db:.1} dB is not transparent");

        // 20 kHz: the top of the passband, where linear interpolation would be
        // ≈ -6 dB and any imaging would show up as residual. Measure the plateau
        // (the ramps at the file's edges are not part of the passband claim).
        let input = tone(20_000.0, 44_100, 22_050, 0.5); // 10000 whole cycles
        let out = resample_mono(&input, 44_100, 48_000).expect("rates");
        let middle = &out[TAPS..out.len() - TAPS];
        let db = residual_db(middle, 48_000, 20_000.0);
        assert!(db < -40.0, "20 kHz residual {db:.1} dB — images or droop");
        // …and it is not attenuated into nothing.
        let amplitude = rms(middle) * std::f64::consts::SQRT_2;
        assert!(
            (amplitude - 0.5).abs() < 0.05,
            "20 kHz lost level: amplitude {amplitude:.3}"
        );
    }

    #[test]
    fn downsampling_removes_content_above_the_new_nyquist() {
        // 30 kHz cannot survive 48 kHz (24 kHz Nyquist) — it must be filtered
        // out, not folded back as an alias. (48 → 44.1 kHz has no such room: its
        // stopband is 22.05–24 kHz and the input band ends at 24 kHz, so the
        // transition band covers all of it.)
        let input = tone(30_000.0, 96_000, 48_000, 0.5);
        let out = resample_mono(&input, 96_000, 48_000).expect("rates");
        assert_eq!(out.len(), 24_000);
        let middle = &out[TAPS..out.len() - TAPS];
        let ratio = rms(middle) / rms(&input);
        assert!(
            ratio < 0.05,
            "alias not filtered: output is {ratio:.4} of the input"
        );
    }

    #[test]
    fn a_downsample_keeps_what_fits() {
        // 1 kHz survives 48 → 44.1 kHz at full level.
        let input = tone(1000.0, 48_000, 24_000, 0.5);
        let out = resample_mono(&input, 48_000, 44_100).expect("rates");
        let middle = &out[TAPS..out.len() - TAPS];
        let amplitude = rms(middle) * std::f64::consts::SQRT_2;
        assert!(
            (amplitude - 0.5).abs() < 0.02,
            "level changed: {amplitude:.3}"
        );
        assert!(residual_db(middle, 44_100, 1000.0) < -60.0);
    }

    #[test]
    fn the_block_size_does_not_change_the_output() {
        let input = tone(3000.0, 44_100, 30_000, 0.8);

        let mut one = Vec::new();
        let mut rs = Resampler::new(44_100, 48_000).expect("rates");
        rs.process(&input, &mut one);
        rs.flush(&mut one);

        let mut many = Vec::new();
        let mut rs = Resampler::new(44_100, 48_000).expect("rates");
        for chunk in input.chunks(7) {
            rs.process(chunk, &mut many);
        }
        rs.flush(&mut many);

        assert_eq!(one.len(), many.len());
        assert_eq!(
            one, many,
            "chunking changed the conversion (not deterministic)"
        );
    }

    #[test]
    fn rates_other_than_the_common_pair_work() {
        // 48 kHz → 96 kHz (an integer ratio) and 22.05 → 48 kHz.
        let out = resample_mono(&tone(1000.0, 48_000, 4800, 0.5), 48_000, 96_000).expect("rates");
        assert_eq!(out.len(), 9600);
        assert!(residual_db(&out, 96_000, 1000.0) < -60.0);

        let out = resample_mono(&tone(1000.0, 22_050, 22_050, 0.5), 22_050, 48_000).expect("rates");
        assert_eq!(out.len(), 48_000);
        assert!(residual_db(&out, 48_000, 1000.0) < -60.0);
    }

    /// Not a correctness test — the measured rate, which is what decides that
    /// importing a long take is a pause and not a problem:
    /// `cargo test -p media --release -- --ignored --nocapture throughput`.
    #[test]
    #[ignore]
    fn throughput() {
        let seconds = 60usize;
        let input = tone(440.0, 44_100, 44_100 * seconds, 0.5);
        let start = std::time::Instant::now();
        let out = resample_mono(&input, 44_100, 48_000).expect("rates");
        let elapsed = start.elapsed().as_secs_f64();
        assert_eq!(out.len(), 48_000 * seconds);
        println!(
            "resample: {seconds} s of 44.1→48 kHz mono in {elapsed:.2} s ({:.0}× realtime)",
            seconds as f64 / elapsed
        );
    }
}
