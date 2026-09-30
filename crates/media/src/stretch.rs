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
#[path = "tests/stretch.rs"]
mod tests;
