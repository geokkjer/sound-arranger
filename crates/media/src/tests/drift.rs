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
fn run(c: &mut DriftCompensator, dev: &mut Device, blocks: usize) -> (usize, usize, usize, usize) {
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
