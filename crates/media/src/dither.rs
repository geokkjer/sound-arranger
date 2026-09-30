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
#[path = "tests/dither.rs"]
mod tests;
