//! The multi-channel capture path (P1.2, the USB-hardware-mixer target): a
//! device — the Soundcraft Notepad-12FX's 4 USB capture channels, the Scarlett
//! 2i2's 2 — or a test's virtual source feeds **interleaved frames** into a
//! shared ring; a demux thread splits each frame into per-channel SPSC rings
//! (for monitoring via [`CaptureNode`]) and writes each channel to its own
//! **float-WAV pool source + `.peaks` sidecar** (the prior-art dispositions:
//! 32-bit float on disk, 256-sample min/max reductions).
//!
//! The pool write is authoritative; the monitoring rings are best-effort
//! (full → dropped + counted, the consumer must keep up — the tests pace it).
//! The demux thread is off the audio path (allocation allowed).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use engine::{AudioNode, CAP_EVENTS, EventBuf, NodeIO, NoteEvent, RenderBlock, Trigger};

use crate::peaks::{PeakBuilder, PeakFile};
use crate::ring::Spsc;
use crate::wav::WavWriter;

/// Frames the demux thread drains per batch from the source ring.
const DEMUX_BATCH: usize = 512;

/// The live multi-channel capture: pool writer + per-channel monitoring rings.
/// The widest capture layout a take accepts: a **sanity bound** (a typo guard), not a
/// design ceiling — nothing is sized by it (a take's rings and WAVs are allocated from
/// the channel count it was started with), and no interface reports anywhere near it.
pub const CAPTURE_CHANNELS_SANITY: usize = 64;

pub struct Capture {
    channels: usize,
    channel_rings: Vec<Arc<Spsc<f32>>>,
    stop: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
    err: Arc<Mutex<Option<String>>>,
    pool_dir: PathBuf,
    take_id: String,
    frames: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
}

impl Capture {
    /// Start a take: spawn the demux thread writing `channels` float WAVs
    /// (`pool_dir/take_id.ch{k}.wav`) plus `.peaks` sidecars. `source` is the
    /// interleaved ring the device callback (or test fixture) feeds.
    ///
    /// `sample_rate` is the **session** clock (the declared WAV rate; the pool
    /// source must match the session). `input_rate` is the **device** clock —
    /// the input device delivers `input_rate` frames/sec, which drifts slightly
    /// from the session; the demux runs a [`DriftCompensator`] per channel to
    /// keep the recorded take in *session* frames (a 20-min jam can drift
    /// thousands of samples otherwise). Pass `input_rate == sample_rate` for a
    /// no-drift passthrough.
    pub fn start(
        pool_dir: &Path,
        take_id: &str,
        channels: usize,
        sample_rate: u32,
        input_rate: u32,
        source: Arc<Spsc<f32>>,
    ) -> Result<Self, String> {
        if !(1..=CAPTURE_CHANNELS_SANITY).contains(&channels) {
            return Err(format!(
                "capture channels must be 1..={CAPTURE_CHANNELS_SANITY} (a sanity bound, not a \
                 design limit), got {channels}"
            ));
        }
        if !take_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "take id '{take_id}' must be [A-Za-z0-9_-]+ (it goes into filenames)"
            ));
        }
        std::fs::create_dir_all(pool_dir).map_err(|e| format!("pool dir: {e}"))?;
        let channel_rings: Vec<Arc<Spsc<f32>>> = (0..channels)
            .map(|_| Arc::new(Spsc::new(1 << 16)))
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        let err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let frames = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let (rings2, stop2, err2) = (channel_rings.clone(), stop.clone(), err.clone());
        let (frames2, dropped2) = (frames.clone(), dropped.clone());
        let pool_dir = pool_dir.to_path_buf();
        let take_id = take_id.to_string();
        let (source2, pool_dir2, take_id2) = (source.clone(), pool_dir.clone(), take_id.clone());

        // Open the pool writers synchronously — fail-loud at start(), not
        // deferred to stop() (kimi review finding 11: an unwritable pool must
        // not silently record nothing).
        let mut writers: Vec<Option<ChannelWriter>> = Vec::with_capacity(channels);
        for k in 0..channels {
            writers.push(Some(ChannelWriter::open(
                &pool_dir,
                &take_id,
                k,
                sample_rate,
            )?));
        }

        // One drift compensator per channel (it is mono): convert the device
        // clock's frames to session frames so the recorded take stays in session
        // frames across a long jam. Passthrough when input_rate == sample_rate.
        // Each channel's compensator is fed identical-length input with an
        // identical ratio and identical f64 operation order, so `pos`/`pending`
        // evolve bit-identically — channels cannot diverge (alignment is
        // load-bearing for the pool source).
        let mut comps: Vec<crate::drift::DriftCompensator> = (0..channels)
            .map(|_| crate::drift::DriftCompensator::new(input_rate, sample_rate))
            .collect();

        let handle = std::thread::Builder::new()
            .name("media-capture".into())
            .spawn(move || {
                let mut buf = Vec::with_capacity(channels * DEMUX_BATCH);
                loop {
                    while buf.len() < channels * DEMUX_BATCH {
                        match source2.try_pop() {
                            Some(s) => buf.push(s),
                            None => break,
                        }
                    }
                    // Whole frames only: a partial frame at the tail is kept
                    // for the next batch — discarding it would shift every
                    // channel by a sample (a real bug found by the e2e test).
                    let whole = buf.len() - (buf.len() % channels);
                    let n_frames = whole / channels;
                    if n_frames == 0 {
                        if stop2.load(Ordering::Acquire) {
                            if !buf.is_empty() {
                                *err2.lock().unwrap() = Some(format!(
                                    "capture stopped with a partial frame ({}/{} samples) dropped",
                                    buf.len() % channels,
                                    channels
                                ));
                            }
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_micros(50));
                        continue;
                    }
                    // session buffer sized for BOTH drift directions: ratio > 1
                    // (device faster) produces n_frames/ratio outputs from the fed
                    // input; ratio < 1 (device slower) needs up to n_frames/ratio
                    // outputs to drain the input. A n_frames-sized buffer would cap
                    // the ratio < 1 case (drift + an unbounded pending leak).
                    let ratio = input_rate as f64 / sample_rate as f64;
                    let session_cap = (n_frames as f64 / ratio).ceil() as usize + 2;
                    let mut last_ok = 0usize;
                    for k in 0..channels {
                        let mut chan = Vec::with_capacity(n_frames);
                        for f in 0..n_frames {
                            chan.push(buf[f * channels + k]);
                        }
                        comps[k].push_input(&chan);
                        let mut session = vec![0.0f32; session_cap];
                        let n_ok = comps[k].pull_output(&mut session);
                        last_ok = n_ok;
                        session.truncate(n_ok);
                        if let Some(pc) = &mut writers[k]
                            && let Err(e) = pc.push(&session)
                        {
                            *err2.lock().unwrap() = Some(e);
                            writers[k] = None; // stop hammering a failed writer
                        }
                        // Monitoring rings are best-effort: the pool write is
                        // authoritative; a full ring is a dropped monitor
                        // sample (counted), never a lost take.
                        for &s in &session {
                            if !rings2[k].try_push(s) {
                                dropped2.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    frames2.fetch_add(last_ok as u64, Ordering::Relaxed);
                    buf.drain(..whole);
                }
                for (k, w) in writers.into_iter().enumerate() {
                    if let Some(pc) = w
                        && let Err(e) = pc.finish(&pool_dir2, &take_id2, k, sample_rate)
                    {
                        *err2.lock().unwrap() = Some(e);
                    }
                }
            })
            .map_err(|e| format!("capture thread spawn: {e}"))?;

        Ok(Capture {
            channels,
            channel_rings,
            stop,
            handle: Mutex::new(Some(handle)),
            err,
            pool_dir,
            take_id,
            frames,
            dropped,
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The monitoring ring for channel `k` (fed by the demux thread).
    pub fn channel_ring(&self, k: usize) -> Arc<Spsc<f32>> {
        self.channel_rings[k].clone()
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Pool path of channel `k`'s source WAV.
    pub fn path_for(&self, k: usize) -> PathBuf {
        self.pool_dir.join(format!("{}.ch{k}.wav", self.take_id))
    }

    /// Stop the take: drain the source, finalize the float WAVs, write the
    /// peaks sidecars, join the demux thread.
    pub fn stop(&self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        let handle = self.handle.lock().unwrap().take();
        if let Some(handle) = handle
            && handle.join().is_err()
        {
            return Err("capture demux thread panicked".into());
        }
        self.err.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// One channel's pool writer: float WAV + incremental peaks.
struct ChannelWriter {
    writer: WavWriter,
    peaks: PeakBuilder,
}

impl ChannelWriter {
    fn open(pool_dir: &Path, take_id: &str, k: usize, sample_rate: u32) -> Result<Self, String> {
        let path = pool_dir.join(format!("{take_id}.ch{k}.wav"));
        Ok(ChannelWriter {
            writer: WavWriter::create_float(&path, sample_rate, 1)?,
            peaks: PeakBuilder::new(),
        })
    }

    fn push(&mut self, samples: &[f32]) -> Result<(), String> {
        self.writer.write(samples)?;
        self.writer.flush()?; // crash-safe takes, like the recorder thread
        self.peaks.push(samples);
        Ok(())
    }

    fn finish(
        mut self,
        pool_dir: &Path,
        take_id: &str,
        k: usize,
        sample_rate: u32,
    ) -> Result<(), String> {
        self.writer.finalize()?;
        let peaks_path = pool_dir.join(format!("{take_id}.ch{k}.peaks"));
        PeakFile::write(&peaks_path, &mut self.peaks, sample_rate)?;
        Ok(())
    }
}

/// A graph node exposing one capture channel's monitoring stream
/// (`out("audio")`). Pops its channel ring; empty → zero-fill + count (the
/// consumer must keep up — tests pace it, real monitoring is real-time).
pub struct CaptureNode {
    ring: Arc<Spsc<f32>>,
    underruns: Arc<AtomicU64>,
}

impl CaptureNode {
    pub fn new(ring: Arc<Spsc<f32>>) -> Self {
        CaptureNode {
            ring,
            underruns: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn underrun_counter(&self) -> Arc<AtomicU64> {
        self.underruns.clone()
    }
}

impl AudioNode for CaptureNode {
    fn latency(&self) -> u32 {
        0
    }

    fn render(
        &mut self,
        _io: &NodeIO,
        out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    ) {
        if block.mode == engine::RenderMode::Drain {
            out.fill(0.0); // a source mutes past the timeline; only tails drain
            return;
        }
        for sample in out.iter_mut() {
            match self.ring.try_pop() {
                Some(v) => *sample = v,
                None => {
                    *sample = 0.0;
                    self.underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A virtual 4-channel device: interleaved frames, one sine per channel.
    fn feed_interleaved(source: &Spsc<f32>, channels: usize, frames: usize, sr: u32) {
        let freqs = [440.0f32, 660.0, 880.0, 220.0];
        let mut phase = [0.0f64; 4];
        let mut i = 0usize;
        while i < frames * channels {
            for (k, s) in phase.iter_mut().enumerate() {
                let v = (std::f64::consts::TAU * *s).sin() as f32 * 0.5;
                while !source.try_push(v) {
                    std::thread::sleep(std::time::Duration::from_micros(10));
                }
                *s += freqs[k] as f64 / sr as f64;
                i += 1;
                if i >= frames * channels {
                    break;
                }
            }
        }
    }

    #[test]
    fn capture_writes_per_channel_pool_sources() {
        let dir = std::env::temp_dir().join(format!("p12-pool-{}", std::process::id()));
        let channels = 4;
        let sr = 48_000u32;
        let frames = 5120usize;
        let source = Arc::new(Spsc::new(1 << 18));
        let cap = Capture::start(&dir, "take1", channels, sr, sr, source.clone()).unwrap();
        feed_interleaved(&source, channels, frames, sr);
        // wait for the demux to drain
        std::thread::sleep(std::time::Duration::from_millis(100));
        cap.stop().unwrap();

        for k in 0..channels {
            let wav_path = cap.path_for(k);
            let peaks_path = dir.join(format!("take1.ch{k}.peaks"));
            assert!(wav_path.exists(), "channel {k} source WAV");
            assert!(peaks_path.exists(), "channel {k} peaks sidecar");

            let mut r = crate::wav::WavReader::open(&wav_path).unwrap();
            assert_eq!(r.sample_rate(), sr);
            assert_eq!(r.total_frames(), frames as u64);
            let mut back = vec![0.0f32; frames];
            assert_eq!(r.read_into(&mut back), frames);

            // float round-trip: the pool source is bit-identical to the input
            let freqs = [440.0f32, 660.0, 880.0, 220.0];
            let mut phase = 0.0f64;
            for (i, s) in back.iter().enumerate() {
                let expected = (std::f64::consts::TAU * phase).sin() as f32 * 0.5;
                phase += freqs[k] as f64 / sr as f64;
                assert_eq!(*s, expected, "ch{k} sample {i} must round-trip exactly");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A device whose clock drifts from the session must be recorded into SESSION
    /// frames: the take's length is device_frames / ratio, and the pitch is
    /// preserved (a rate-mismatched take would play shifted).
    fn assert_drift_corrected(input_sr: u32, session_sr: u32) {
        let dir = std::env::temp_dir().join(format!("p12-drift-{}-{input_sr}", std::process::id()));
        let channels = 1u32;
        let dev_frames = session_sr as u64; // 1 s of device frames
        let source = Arc::new(Spsc::new(1 << 18));
        let cap = Capture::start(
            &dir,
            "drift1",
            channels as usize,
            session_sr,
            input_sr,
            source.clone(),
        )
        .unwrap();

        // feed a 440 Hz tone at the DEVICE clock
        let mut phase = 0.0f64;
        let mut pushed = 0u64;
        while pushed < dev_frames {
            let v = (std::f64::consts::TAU * phase).sin() as f32 * 0.5;
            while !source.try_push(v) {
                std::thread::sleep(std::time::Duration::from_micros(10));
            }
            phase += 440.0 / input_sr as f64;
            pushed += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        cap.stop().unwrap();

        let wav_path = cap.path_for(0);
        let mut r = crate::wav::WavReader::open(&wav_path).unwrap();
        // device_frames / ratio = session_frames (the drift absorbs the fractional
        // remainder; ± a couple for the batch-pull tail).
        let expected = (dev_frames as f64 * session_sr as f64 / input_sr as f64).round() as i64;
        let got = r.total_frames() as i64;
        assert!(
            (got - expected).abs() <= 4,
            "drifted take must land in session frames, got {got} (expected ~{expected})"
        );
        // pitch preserved: zero-crossings over the recorded span ≈ 440 * seconds * 2
        let mut back = vec![0.0f32; r.total_frames() as usize];
        let n = r.read_into(&mut back);
        let crossings = back[..n]
            .windows(2)
            .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
            .count();
        let expect_cross = 440.0 * (got as f64 / session_sr as f64) * 2.0;
        assert!(
            (crossings as f64 - expect_cross).abs() < expect_cross * 0.02,
            "pitch drifted: {crossings} vs ~{expect_cross}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn faster_drift_records_into_session_frames() {
        assert_drift_corrected(48_001, 48_000); // device clock faster than session
    }

    #[test]
    fn slower_drift_records_into_session_frames() {
        assert_drift_corrected(47_999, 48_000); // device clock slower (the ratio < 1 case)
    }
}
