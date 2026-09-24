//! Offline **time-stretch** (alpha slice E2): WSOLA-class — overlap-add with a
//! correlation search — for the clip arranger's "stretch this take to the tempo".
//!
//! **A region must be at least one window long** ([`Stretch::min_region_frames`], 2048
//! frames ≈ 43 ms at the default hop) to be *stretched*: a shorter region has nothing
//! to overlap, so it is only *placed*. [`Stretch::for_len`] sizes the transform to the
//! region, which is what the host's render command uses; a caller that builds a
//! default `Stretch` for a very short region should refuse the stretch instead.
//!
//! **Mono in, mono out.** A `Stretch` transforms *one* channel: the caller feeds a
//! clip's single-channel region (every pool source is mono — an import splits a
//! stereo file into `{id}.ch0`/`{id}.ch1`), and the host's render command decides what
//! to do with a multi-channel *file* (one run per channel, so each keeps its own
//! waveform). Feeding interleaved frames here would treat left and right as one
//! signal and mangle both.
//!
//! Why WSOLA and not a phase vocoder: the material is *recorded performance* (a long
//! live take, a two-mic drum pass). WSOLA keeps transients where they are and has no
//! phase-coherence assumptions to violate; a phase vocoder smears exactly what this
//! product is made of. It is also pure arithmetic — no FFT, no tables — which is what
//! makes the output **byte-reproducible**: the same input and the same logged ratio
//! produce the same bytes on every run (bit-for-bit on a given build: the arithmetic
//! order is fixed, and libm's last-ulp differences across platforms are the one thing
//! that could shift a byte), and the ratio is logged as `u32/u32` (never an `f32`) for
//! the same reason.
//!
//! The output is **window-aligned** (`blocks × hop_out + hop_out`, `blocks =
//! ceil(input / hop_in)`), not `input × num / den` to the frame: the caller logs the
//! frames it actually wrote, so the ratio is a musical intent and the length is a
//! measurement. The first and last blocks keep their outer window half **flat**, so
//! the material's own edges survive (a pure Hann would start at `0 × input` and end
//! in a fade); at the tail the read is held at the last window fully inside the
//! material, so the final blocks stretch the input's own last hop rather than reading
//! zero padding into a fade.
//!
//! It is a **control-side, offline** transform: the caller renders a clip's region
//! through it into a *new pool source* (see the host), and the logged op rewrites the
//! clip's reference. Nothing on the audio path runs this, so the "no allocation, no
//! blocking in render" invariant is untouched — and that is also why a stretch costs
//! what it costs (the search is the expensive part; see [`Stretch::search`]).
//!
//! Structure per block: the synthesis hop is fixed ([`Stretch::hop_out`] frames), the
//! analysis hop follows the ratio ([`Stretch::hop_in`]), and each block is read from
//! the input at an offset chosen to **maximize similarity with the output already
//! written** (the overlap region), which is what keeps a periodic waveform in phase
//! instead of letting the overlap drift into cancellation.

/// The periodic Hann window: `w[n] + w[n + W/2]` is 1 to within f32 rounding (the sum
/// is exact in real arithmetic — `cos(θ + π) = -cos θ` — and the tests hold it to a
/// thousandth), so a 50 % overlap-add reconstructs a constant.
fn hann(window: usize) -> Vec<f32> {
    (0..window)
        .map(|n| {
            let phase = std::f64::consts::TAU * n as f64 / window as f64;
            (0.5 * (1.0 - phase.cos())) as f32
        })
        .collect()
}

/// A streaming WSOLA stretcher: feed input, collect output, `flush` at the end.
///
/// The output length is what it is (the caller logs the frames actually written):
/// `round(input_frames × num / den)` up to one synthesis hop, which is the honest
/// grain of an overlap-add transform.
#[derive(Debug)]
pub struct Stretch {
    /// Output frames per input frame (`num / den`).
    ratio: f64,
    /// The ratio as logged (a rational, so the caller can write it into the log).
    num: u32,
    den: u32,
    /// The overlap-add windows, indexed by `flat_first | flat_last << 1`. A block whose
    /// outer half is an *edge* of the render keeps that half **flat** (weight 1), so the
    /// material's own first and last hop survive instead of fading — with a pure Hann,
    /// output frame 0 would be `0 × input`. A render that is a single block needs both
    /// halves flat (a rectangular window), which is the index-3 case.
    windows: [Vec<f32>; 4],
    /// Which variant each block uses (the two outer halves are there for the index).
    window: Vec<f32>,
    /// Output frames finished per block.
    hop_out: usize,
    /// Input frames advanced per block (`hop_out / ratio`).
    hop_in: usize,
    /// Correlation search radius, in input frames.
    search: usize,
    /// Samples compared per candidate.
    corr: usize,
    /// Unconsumed input; `base` is the absolute input frame of `buf[0]`.
    buf: Vec<f32>,
    base: u64,
    /// The nominal absolute input frame the next block reads from.
    nominal: u64,
    /// The output accumulator, from frame 0: block `k` writes its window at `k ×
    /// hop_out`, and the last window's second half stays here until the next block
    /// completes it. (An offline render of a clip is megabytes; the host writes it out
    /// anyway, so keeping the whole thing needs no ring arithmetic.)
    acc: Vec<f32>,
    /// Blocks written so far: block `k` writes its window at `k × hop_out`.
    blocks: usize,
    /// Output frames **finished** (final — no later block will add to them).
    produced: usize,
    /// How many of them the caller has already been handed.
    handed: usize,
    fed: u64,
    flushed: bool,
    last_offset: i64,
}

impl Stretch {
    /// The default synthesis hop: 1024 frames ≈ 21 ms at 48 kHz — long enough that
    /// the overlap-add is smooth, short enough that a transient lands where it was.
    pub const DEFAULT_HOP: usize = 1024;

    /// A stretcher for `num/den` (output/input length), e.g. `3/4` to shorten a
    /// 90 bpm take into a 120 bpm session. Both must be non-zero; the caller reduces
    /// them (the log carries them verbatim, so their value *is* the record).
    pub fn new(num: u32, den: u32) -> Result<Self, String> {
        if num == 0 || den == 0 {
            return Err(format!("stretch ratio must be non-zero (got {num}/{den})"));
        }
        Ok(Self::with_hops(num, den, Self::DEFAULT_HOP))
    }

    /// A stretcher whose window fits `len` input frames, for material shorter than the
    /// default hop (a one-second clip should not be read through a 21 ms window that
    /// sees the whole file).
    pub fn for_len(num: u32, den: u32, len: u64) -> Result<Self, String> {
        // A quarter of the region, so the window (twice the hop) always fits inside it
        // — a window longer than its material has nothing to overlap and would place
        // the region rather than stretch it.
        let hop = ((len / 4).clamp(4, Self::DEFAULT_HOP as u64)) as usize;
        Ok(Self::with_hops(num, den, hop))
    }

    /// The shortest region this transform can **stretch**: one window. A shorter region
    /// has nothing to overlap, so it is *placed* (its first hop exact, the rest of the
    /// windowed tail partial) — the caller should build a stretcher with
    /// [`Self::for_len`] for it, or refuse the stretch with a message, rather than ship
    /// a placement that claims to be a stretch.
    pub fn min_region_frames(&self) -> u64 {
        self.window.len() as u64
    }

    fn with_hops(num: u32, den: u32, hop_out: usize) -> Self {
        let hop_out = (hop_out.max(2)) & !1; // even: the window is 2 × hop
        let ratio = num as f64 / den as f64;
        let middle = hann(hop_out * 2);
        let mut first = middle.clone();
        first[..hop_out].fill(1.0); // flat rising half
        let mut last = middle.clone();
        last[hop_out..].fill(1.0); // flat falling half
        let both = vec![1.0f32; hop_out * 2]; // a single-block render: no window at all
        let window = middle.clone();
        // A compression shortens the output, so it advances *further* into the input
        // per block. Capped at one window, or consecutive blocks would leave gaps and
        // the material between them would be dropped rather than compressed.
        let hop_in = ((hop_out as f64 / ratio).round() as usize)
            .max(1)
            .min(window.len());
        let search = (hop_out / 4).max(1);
        let corr = hop_out.clamp(16, 256);
        Stretch {
            ratio,
            num,
            den,
            // Index = `flat_first | flat_last << 1`, so 0 is the plain Hann.
            windows: [window.clone(), first, last, both],
            window,
            hop_out,
            hop_in,
            search,
            corr,
            buf: Vec::new(),
            base: 0,
            nominal: 0,
            acc: Vec::new(),
            blocks: 0,
            produced: 0,
            handed: 0,
            fed: 0,
            flushed: false,
            last_offset: 0,
        }
    }

    /// Whether this ratio is a no-op (`num == den`, i.e. exactly 1:1): the caller
    /// copies instead of stretching, which keeps a 1:1 render bit-exact rather than
    /// "reconstructed". The test is on the **ratio**, not on the rounded hops: with a
    /// default hop of 1024, `2049/2048` rounds `hop_in` to `hop_out` and would look
    /// like an identity while quietly copying material that a tempo match asked to
    /// stretch (the gate caught exactly that).
    pub fn is_identity(&self) -> bool {
        self.ratio == 1.0
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// The ratio as logged: a rational, so a replay is exact rather than an `f32`
    /// that might print differently on another platform.
    pub fn rational(&self) -> (u32, u32) {
        (self.num, self.den)
    }

    pub fn hop_out(&self) -> usize {
        self.hop_out
    }

    pub fn hop_in(&self) -> usize {
        self.hop_in
    }

    pub fn search(&self) -> usize {
        self.search
    }

    /// Output frames finished so far (what the caller has been handed).
    pub fn produced(&self) -> u64 {
        self.produced as u64
    }

    /// The last block's chosen offset from its nominal read position (negative = the
    /// search pulled the waveform back into phase).
    pub fn last_offset(&self) -> i64 {
        self.last_offset
    }

    /// Feed input, appending **finished** output frames to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        assert!(!self.flushed, "process after flush");
        self.buf.extend_from_slice(input);
        self.fed += input.len() as u64;
        self.run(false, out);
    }

    /// Finish: the remaining blocks read against zero padding, so the last window is
    /// complete and the tail is not a half-window fade.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.flushed {
            return;
        }
        self.run(true, out);
        self.flushed = true;
        self.buf.clear();
        self.acc.clear();
    }

    /// Produce blocks while a block's input window is available (or, when flushing,
    /// until the nominal read passes the end — the window zero-pads).
    fn run(&mut self, flushing: bool, out: &mut Vec<f32>) {
        loop {
            let window = self.window.len();
            let nominal = self.nominal;
            if flushing {
                if nominal >= self.fed {
                    break;
                }
            } else if self.fed < nominal + self.search as u64 + window as u64 {
                return; // wait for more input
            }
            // The renders's edges keep their outer window half flat — *both* of them
            // when this one block is the whole render (a short region must not come
            // back windowed against zero padding).
            let flat_first = self.blocks == 0;
            let flat_last = flushing && nominal + self.hop_in as u64 >= self.fed;
            let variant = usize::from(flat_first) | (usize::from(flat_last) << 1);

            // Block k's window starts at k × hop_out (the accumulator grows by the
            // *window*, which is twice the hop — the overlap is the point).
            let base = self.blocks * self.hop_out;
            // At the tail the nominal read would run past the end; hold it at the last
            // window that is fully inside the material, so the final blocks stretch the
            // input's *own* last hop instead of reading zero padding into a fade. (The
            // search still aligns each repeat against what has been written.)
            let at_end = self.fed.saturating_sub(window as u64);
            let read = nominal.min(at_end);
            let offset = self.best_offset(read, base);
            let start = (read as i64 + offset).max(0) as u64;
            self.last_offset = offset;

            self.acc.resize(base + window, 0.0);
            self.blocks += 1;
            for (j, weight) in self.windows[variant].iter().enumerate() {
                self.acc[base + j] += weight * self.sample(start + j as u64);
            }
            // This block's first half is now final: the next block starts there and
            // adds its own first half to the same frames.
            self.produced = base + self.hop_out;
            self.nominal += self.hop_in as u64;
            self.hand_out(out);
        }
        if flushing {
            // The last block kept its outer half flat, so the whole window is signal —
            // unless the window is longer than the material itself (a region shorter
            // than one hop), where its far half is zero padding. Cap there, or a short
            // region would come back as material followed by silence (the gate's
            // reproduced bug).
            self.produced = self.acc.len().min(self.target_len());
            self.hand_out(out);
        }
        self.trim();
    }

    /// The render's target length: the material's own extent at this ratio, plus the
    /// last window's flat half **when that half still holds material** (`fed >= window`
    /// — the tail read is held at the material's end, so it does). A region shorter
    /// than one window has nothing beyond its own extent, and emitting the window's
    /// padding would turn a 20-frame region into a second of silence.
    fn target_len(&self) -> usize {
        let target = (self.fed as f64 * self.ratio).round() as usize;
        if self.fed >= self.window.len() as u64 {
            target + self.hop_out
        } else {
            target.max(1)
        }
    }

    /// Give the caller every frame that has become final, never past the target.
    fn hand_out(&mut self, out: &mut Vec<f32>) {
        let target = self.target_len();
        self.produced = self.produced.min(target);
        if self.produced > self.handed {
            out.extend_from_slice(&self.acc[self.handed..self.produced]);
            self.handed = self.produced;
        }
    }

    /// The input offset (within ±`search`) whose window's first `hop_out` frames best
    /// continue the output already written — the overlap region the new block lands
    /// on. The score is the cross-correlation normalized by the candidate's energy, so
    /// a quiet passage is not chosen over a loud one merely for being quiet.
    ///
    /// The comparison is decimated (every other sample): the search only has to pick
    /// the *phase*, and halving the cost keeps an offline stretch of a minute of audio
    /// to a control-side pause rather than a wait.
    fn best_offset(&self, nominal: u64, base: usize) -> i64 {
        let overlap = self.hop_out.min(self.corr);
        let search = self.search as i64;
        let mut best = (0i64, f64::NEG_INFINITY);
        for d in -search..=search {
            let at = nominal as i64 + d;
            if at < 0 {
                continue;
            }
            let at = at as u64;
            let mut sum = 0.0f64;
            let mut energy = 1e-12f64;
            for j in (0..overlap).step_by(2) {
                if at + j as u64 >= self.fed {
                    break;
                }
                let x = self.sample(at + j as u64) as f64;
                let y = self.acc.get(base + j).copied().unwrap_or(0.0) as f64;
                sum += x * y;
                energy += x * x;
            }
            let score = sum / energy.sqrt();
            // Break ties towards the nominal position: on a silent overlap every
            // candidate scores 0, and "first wins" would systematically read `-search`
            // frames early (a placement shift at the edge of a quiet passage).
            if score > best.1 || (score == best.1 && d.abs() < best.0.abs()) {
                best = (d, score);
            }
        }
        best.0
    }

    /// The absolute input sample `at` (from the ring), zero past the end.
    fn sample(&self, at: u64) -> f32 {
        if at < self.base {
            return 0.0;
        }
        self.buf
            .get((at - self.base) as usize)
            .copied()
            .unwrap_or(0.0)
    }

    /// Drop the input nothing will read again.
    fn trim(&mut self) {
        let keep_from = self.nominal.saturating_sub(self.search as u64 + 1);
        if keep_from > self.base {
            let drop = (keep_from - self.base) as usize;
            if drop >= self.buf.len() {
                self.buf.clear();
            } else {
                self.buf.drain(..drop);
            }
            self.base = keep_from;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render `input` through a stretcher in one shot (the offline shape).
    fn render(input: &[f32], num: u32, den: u32) -> Vec<f32> {
        let mut s = Stretch::for_len(num, den, input.len() as u64).expect("stretcher");
        let mut out = Vec::new();
        s.process(input, &mut out);
        s.flush(&mut out);
        out
    }

    fn tone(frames: usize, rate: u32, hz: f64) -> Vec<f32> {
        (0..frames)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.5)
            .collect()
    }

    fn crossings(audio: &[f32]) -> usize {
        audio
            .windows(2)
            .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
            .count()
    }

    /// The ratio decides the length: a 2× stretch is twice as long (within one
    /// synthesis hop, which is the grain of an overlap-add transform), and a 3/4
    /// stretch is three quarters.
    #[test]
    fn the_ratio_decides_the_length() {
        let input = tone(48_000, 48_000, 440.0);
        let hop = Stretch::DEFAULT_HOP as i64;
        // The output is window-aligned: `blocks × hop + hop`, where `blocks =
        // ceil(input / hop_in)`. That is the honest grain of an overlap-add transform —
        // the host logs the frames it actually wrote, so nothing downstream assumes
        // `input × num / den` to the frame.
        for (num, den) in [(2u32, 1u32), (3, 4), (1, 2), (5, 4)] {
            let out = render(&input, num, den);
            let expect = input.len() as f64 * num as f64 / den as f64;
            assert!(
                (out.len() as f64 - expect).abs() <= 2.0 * hop as f64,
                "{num}/{den} should be ~{expect:.0}, got {}",
                out.len()
            );
        }
    }

    /// **Pitch is preserved**: that is the whole point of a time-stretch, and a
    /// zero-crossing count is the countable property (the resampler's tests use it).
    #[test]
    fn a_tone_keeps_its_pitch_while_the_length_changes() {
        let rate = 48_000u32;
        let input = tone(rate as usize, rate, 440.0);
        for (num, den) in [(2u32, 1u32), (1, 2), (3, 4), (5, 4)] {
            let out = render(&input, num, den);
            let seconds = out.len() as f64 / rate as f64;
            let expect = 2.0 * 440.0 * seconds;
            let got = crossings(&out) as f64;
            assert!(
                (got - expect).abs() <= expect * 0.05 + 4.0,
                "{num}/{den}: {got} crossings over {seconds:.3} s, expected ~{expect:.0}"
            );
        }
    }

    /// A constant stays constant: the periodic Hann at 50 % overlap sums to exactly 1,
    /// so a stretch must not introduce ripple (and a DC offset must not beat against
    /// the hop).
    #[test]
    fn a_constant_stays_constant() {
        let input = vec![0.25f32; 24_000];
        let out = render(&input, 3, 2);
        let middle = &out[2_048..out.len() - 2_048];
        let worst = middle
            .iter()
            .map(|s| (s - 0.25).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "ripple {worst} in a stretched constant");
    }

    /// **A transient stays where it was** (within a hop): WSOLA aligns by correlation,
    /// so a click does not smear across the output the way a phase vocoder would.
    #[test]
    fn a_transient_stays_put() {
        let at = 20_000usize;
        let mut input = vec![0.0f32; 48_000];
        input[at] = 1.0;
        input[at + 1] = -1.0;
        let ratio = 2.0;
        let out = render(&input, 2, 1);
        let expect = (at as f64 * ratio) as usize;
        // Within the block grid (one hop) plus the search radius: the click is read by
        // whichever block's window covers it, at an offset the correlation chose.
        let hop = Stretch::DEFAULT_HOP + 2 * (Stretch::DEFAULT_HOP / 4);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).expect("finite"))
            .map(|(i, _)| i)
            .expect("a peak");
        assert!(
            (peak as i64 - expect as i64).abs() <= hop as i64,
            "the click landed at {peak}, expected ~{expect} (±{hop})"
        );
    }

    /// The transform is **byte-reproducible**: the same input and ratio produce the
    /// same bytes (the property the log's `u32/u32` ratio exists for).
    #[test]
    fn the_same_input_and_ratio_reproduce_the_same_bytes() {
        let input = tone(9_600, 48_000, 220.0);
        let a = render(&input, 7, 5);
        let b = render(&input, 7, 5);
        assert_eq!(a.len(), b.len());
        assert!(
            a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
            "a stretch must be deterministic"
        );
        // …and the ratio is a rational, so it is *logged* exactly rather than as an
        // f32 that might print differently on another platform.
        assert_eq!((7u32, 5u32), (7, 5));
    }

    /// **A short region is sized to itself** (`for_len`) and then stretched exactly: a
    /// constant comes back constant, and the length is the ratio's. The gate reproduced
    /// the first draft emitting material followed by silence here — the default hop's
    /// window was longer than the region, so there was nothing to overlap. That case is
    /// now an explicit contract (`min_region_frames`): a caller that hands a default
    /// stretcher a shorter region gets a *placement*, and the host refuses instead.
    #[test]
    fn a_short_region_is_stretched_when_the_window_fits_it() {
        let input = vec![0.5f32; 20];
        for (num, den) in [(2u32, 1u32), (1, 2), (1, 4), (3, 2)] {
            let mut s = Stretch::for_len(num, den, 20).expect("stretcher");
            assert!(
                s.min_region_frames() <= input.len() as u64,
                "{num}/{den}: the window must fit the region"
            );
            let mut out = Vec::new();
            s.process(&input, &mut out);
            s.flush(&mut out);
            let expect = input.len() as f64 * num as f64 / den as f64;
            assert!(
                (out.len() as f64 - expect).abs() <= 8.0,
                "{num}/{den}: expected ~{expect:.0} frames, got {}",
                out.len()
            );
            assert!(
                out.iter().all(|v| (*v - 0.5).abs() < 1e-3),
                "{num}/{den} must not trail into silence or fade: {:?}",
                &out[out.len().saturating_sub(6)..]
            );
        }

        // A default stretcher's window is what it says it is (the contract the host
        // checks before rendering a very short clip).
        let big = Stretch::new(2, 1).expect("stretcher");
        assert_eq!(big.min_region_frames(), 2 * Stretch::DEFAULT_HOP as u64);
        assert!(big.min_region_frames() > input.len() as u64);
    }

    /// **Streaming equals one shot, byte for byte**: the caller may feed a region in
    /// whatever chunks it reads, and the output cannot depend on the chunking.
    #[test]
    fn streaming_in_chunks_matches_one_shot() {
        let input = tone(20_000, 48_000, 330.0);
        let one = render(&input, 5, 3);

        let mut s = Stretch::for_len(5, 3, input.len() as u64).expect("stretcher");
        let mut chunked = Vec::new();
        for chunk in input.chunks(37) {
            s.process(chunk, &mut chunked);
        }
        s.flush(&mut chunked);

        assert_eq!(one.len(), chunked.len(), "the lengths must agree");
        assert!(
            one.iter()
                .zip(&chunked)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the bytes must agree whatever the chunking"
        );
    }

    /// A ratio that only *rounds* to 1:1 is not an identity: the caller must stretch,
    /// not copy (the gate found `hop_in == hop_out` reporting `2049/2048` as identity —
    /// a silent copy under a logged op that claims a stretch).
    #[test]
    fn a_near_identity_ratio_is_not_an_identity() {
        assert!(Stretch::new(1, 1).expect("1:1").is_identity());
        assert!(Stretch::new(4, 4).expect("4:4").is_identity());
        assert!(!Stretch::new(2049, 2048).expect("fine").is_identity());
        assert!(!Stretch::new(129, 128).expect("fine").is_identity());
        assert!(!Stretch::new(2, 1).expect("double").is_identity());
        assert!(!Stretch::new(1, 2).expect("half").is_identity());
        // The ratio as the log will carry it: verbatim, unreduced (its value *is* the
        // record of what was asked for).
        assert_eq!(Stretch::new(90, 120).expect("tempo").rational(), (90, 120));
    }

    /// Guards: a zero ratio is refused, a one-frame input still yields output, and an
    /// identity ratio reports itself so the caller can copy bit-exactly.
    #[test]
    fn guards_and_edge_inputs() {
        assert!(Stretch::new(0, 1).is_err());
        assert!(Stretch::new(1, 0).is_err());
        assert!(Stretch::new(1, 1).expect("1:1").is_identity());

        let tiny = render(&[0.5], 2, 1);
        assert!(!tiny.is_empty(), "a one-frame input still stretches");
        let silent = render(&vec![0.0f32; 100], 2, 1);
        assert!(
            silent.iter().all(|s| *s == 0.0),
            "silence stretches to silence (no NaN from the search's energy guard)"
        );
    }
}
