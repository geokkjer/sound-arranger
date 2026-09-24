//! **Fixed-seed TPDF dither** for writing a 16-bit file from float material.
//!
//! Quantising a float mix to 16 bits without dither turns the truncation error into
//! correlated distortion (audible on fades and quiet passages as a gritty "fizz" that
//! follows the signal). Dither replaces that with a low-level noise floor: an
//! independent random offset *before* rounding, of a magnitude around one least
//! significant bit.
//!
//! - **TPDF** (triangular probability density): the sum of two independent uniform
//!   values in `[0, 1)`, i.e. `r1 - r2` in `(-1, 1)` LSB. Triangular dither is the
//!   standard choice: it removes the *first two* moments of the quantisation error
//!   (its mean and its correlation with the signal), so no signal-correlated distortion
//!   remains — at the cost of 4.8 dB more noise than rectangular dither.
//! - **Fixed seed, process-local**: the generator is a plain `SplitMix64` seeded with a
//!   documented constant, never thread-local or time-based entropy. Two exports of the
//!   same session therefore produce **byte-identical** files, which is the property the
//!   rest of the format work (and the log-is-the-document rule) rests on. A "random"
//!   dither would make an export irreproducible; a *seeded* one is reproducible *and*
//!   decorrelated from the signal, which is what matters audibly.
//!
//! The output samples land exactly on the 16-bit grid (`q / 32767`), so a writer that
//! multiplies by 32767 again reproduces `q` exactly — no double rounding.

/// The seed every export dither starts from: a fixed, documented constant (the first
/// 64 bits of the fractional part of √2), so a session's export is byte-reproducible
/// on every machine and every run.
pub const DITHER_SEED: u64 = 0x6a09_e667_f3bc_c909;

/// A 16-bit quantiser with triangular dither. One instance per file/channel run; the
/// same instance and input always produce the same output.
#[derive(Debug, Clone)]
pub struct TpdfDither {
    state: u64,
}

impl TpdfDither {
    /// A dither stream from `seed` ([`DITHER_SEED`] for a reproducible export).
    pub fn new(seed: u64) -> Self {
        TpdfDither { state: seed }
    }

    /// The next 64 bits of the stream (`SplitMix64`: one multiply-xor round, no
    /// tables, no allocation).
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A uniform value in `[0, 1)` — 24 bits, the width of the f32 mantissa.
    fn next_unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// The next **TPDF** offset in least significant bits: `r1 - r2`, in `(-1, 1)`.
    pub fn next_offset(&mut self) -> f32 {
        self.next_unit() - self.next_unit()
    }

    /// Quantise `samples` to the 16-bit grid **in place**, dithering each sample.
    ///
    /// The result is `q / 32767` for an integer `q` in `[-32767, 32767]`: exactly the
    /// value the 16-bit writer stores, so writing these samples introduces no further
    /// rounding. `+1.0` is the top of the grid (the writer maps `1.0` to `32767`).
    pub fn quantize_s16(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            let x = if s.is_finite() { *s } else { 0.0 };
            // One code is reserved at each rail *before* the dither is added. Otherwise
            // a full-scale sample (`x == 1.0`) scales to 32767 and `+d` rounds to 32768,
            // which the final clamp would flatten — signal-correlated error exactly at
            // full scale, where the gate measured it on 88 % of the samples. With the
            // margin, `[-32766, 32766] + (-1, 1)` never reaches a rail, so no sample is
            // ever clamped, and 32767 stays reachable through the dither itself.
            let scaled = (x * 32767.0).clamp(-32766.0, 32766.0);
            let d = self.next_offset();
            let q = (scaled + d).round().clamp(-32767.0, 32767.0);
            *s = q / 32767.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_bytes() {
        let mut a = TpdfDither::new(DITHER_SEED);
        let mut b = TpdfDither::new(DITHER_SEED);
        let mut x: Vec<f32> = (0..1_000).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        let mut y = x.clone();
        a.quantize_s16(&mut x);
        b.quantize_s16(&mut y);
        assert_eq!(x, y, "a fixed seed is a reproducible export");
        // …and a different seed dithers differently (the stream is real).
        let mut c = TpdfDither::new(DITHER_SEED ^ 1);
        let mut z: Vec<f32> = (0..1_000).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        c.quantize_s16(&mut z);
        assert_ne!(x, z);
    }

    #[test]
    fn the_output_is_on_the_16_bit_grid() {
        let mut d = TpdfDither::new(DITHER_SEED);
        let mut x: Vec<f32> = (0..500).map(|i| (i as f32 * 0.07).sin() * 0.9).collect();
        d.quantize_s16(&mut x);
        for s in &x {
            let q = s * 32767.0;
            assert!(
                (q - q.round()).abs() < 1e-2,
                "{s} is not on the 16-bit grid ({q})"
            );
            assert!((-32767.0..=32767.0).contains(&q));
        }
    }

    /// The dither really is **triangular**: zero mean and a variance of `1/6` LSB²
    /// (rectangular would be `1/12`, and no dither would be a spike at zero).
    #[test]
    fn the_dither_is_triangular() {
        let mut d = TpdfDither::new(DITHER_SEED);
        let n = 200_000;
        let mut sum = 0.0f64;
        let mut sumsq = 0.0f64;
        for _ in 0..n {
            let o = d.next_offset() as f64;
            sum += o;
            sumsq += o * o;
        }
        let mean = sum / n as f64;
        let var = sumsq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "zero mean, got {mean}");
        assert!(
            (var - 1.0 / 6.0).abs() < 0.01,
            "TPDF variance is 1/6, got {var}"
        );
        // The support is (-1, 1): no offset ever leaves it.
        assert!(((-1.0)..1.0).contains(&(d.next_offset() as f64)));
    }

    /// Quantising a dithered signal is **unbiased**: the error's mean over a long
    /// run of a quiet constant is ~0 (a truncating quantiser would show a fixed
    /// offset — the correlated distortion dither exists to remove).
    #[test]
    fn the_quantisation_error_has_no_offset() {
        let mut d = TpdfDither::new(DITHER_SEED);
        let value = 0.000_04f32; // far below one LSB (1/32767 ≈ 0.000_0305)
        let mut x = vec![value; 100_000];
        d.quantize_s16(&mut x);
        let mean: f64 = x.iter().map(|s| (*s - value) as f64).sum::<f64>() / x.len() as f64;
        assert!(
            mean.abs() < 1e-6,
            "the error averages out, got {mean} (a whole LSB is {})",
            1.0f64 / 32767.0
        );
        // …and the quantised values actually vary (it is dithering, not dropping).
        let distinct = x.iter().filter(|s| **s != x[0]).count();
        assert!(distinct > 0, "the dither moves samples across the grid");
    }

    /// **Full scale is not a hard clip.** A constant 1.0 must dither between the top two
    /// codes (32766 and 32767), never sit clamped on 32767: a clamp is signal-correlated
    /// error, which is the one thing dither exists to remove.
    #[test]
    fn full_scale_dithers_without_clamping() {
        let mut d = TpdfDither::new(DITHER_SEED);
        let mut x = vec![1.0f32; 40_000];
        d.quantize_s16(&mut x);
        assert!(
            x.iter().all(|s| *s <= 1.0),
            "nothing is written past full scale"
        );
        let top = x
            .iter()
            .filter(|s| (**s * 32767.0).round() == 32767.0)
            .count();
        let next = x
            .iter()
            .filter(|s| (**s * 32767.0).round() == 32766.0)
            .count();
        assert!(
            top > 0 && next > 0,
            "the top two codes both appear ({top} / {next}) — full scale dithers, it does not clamp"
        );
        // `round(32766 + d)` for `d` in (-1, 1) can also reach 32765; nothing else, and
        // never a clamp (the peak check already refused anything above 1.0).
        assert!(
            x.iter().all(|s| {
                let q = (*s * 32767.0).round();
                (32765.0..=32767.0).contains(&q)
            }),
            "no sample is pushed past the rails"
        );
    }

    /// Non-finite input is silenced rather than written as a NaN sample.
    #[test]
    fn non_finite_input_is_silenced() {
        let mut d = TpdfDither::new(DITHER_SEED);
        let mut x = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        d.quantize_s16(&mut x);
        assert!(x.iter().all(|s| s.is_finite()));
        // 0.5 is not on the 16-bit grid (16384/32767 = 0.50001526); it must land on
        // the nearest grid point, within one LSB.
        assert!(
            (x[3] - 0.5).abs() <= 1.0 / 32767.0,
            "a finite value is quantised, not dropped: {}",
            x[3]
        );
    }
}
