//! The cpal device path (Spike B note, `devices`): enumeration/selection and
//! input/output stream builders whose callbacks only push/pop an SPSC ring —
//! never allocate, never block. Thin and isolated here so cpal API drift
//! (0.18.x is pre-1.0) cannot leak into the machinery the numeric tests cover.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::ring::Spsc;

pub fn list_devices() -> Vec<String> {
    cpal::default_host()
        .devices()
        .map(|devices| devices.map(|d| format!("{d}")).collect())
        .unwrap_or_default()
}

pub fn default_output_name() -> Option<String> {
    cpal::default_host().default_output_device().map(|d| format!("{d}"))
}

pub fn default_input_name() -> Option<String> {
    cpal::default_host().default_input_device().map(|d| format!("{d}"))
}

/// An open output stream plus the negotiated rate/layout and the starve counter.
///
/// `requested_rate` is what the caller asked for (the session rate) and
/// `rate_mismatch` is true when the device could not provide it — the caller
/// must surface that, never play at the wrong speed silently (the Tauri review's
/// F6: a 48 kHz session on a 44.1 kHz device would run ~8% flat).
pub struct OutputHandle {
    pub stream: cpal::Stream,
    /// The device's actual output sample rate.
    pub sample_rate: u32,
    /// The device's output channel count.
    pub channels: u16,
    /// The rate the caller requested (the session rate).
    pub requested_rate: u32,
    /// True when the device could not run at `requested_rate`.
    pub rate_mismatch: bool,
    /// Source frames the callback had to fill with silence because the ring was
    /// empty — an audible glitch, counted rather than hidden.
    pub underruns: Arc<AtomicU64>,
}

impl OutputHandle {
    /// Start (or resume) the stream. The cpal trait is used here so callers never
    /// need cpal in scope (this module isolates the API, per its own contract).
    pub fn play(&self) -> Result<(), String> {
        self.stream.play().map_err(|e| format!("play output: {e}"))
    }

    /// Pause the stream — the callback stops, so an idle host does not accrue
    /// underruns on an empty ring.
    pub fn pause(&self) -> Result<(), String> {
        self.stream.pause().map_err(|e| format!("pause output: {e}"))
    }
}

/// An open input stream plus the device's actual sample rate and a shared
/// source-ring overrun counter: the callback drops samples when the ring is
/// full, and every drop is counted here — a dropped take sample must never be
/// silent (kimi review finding 1).
pub struct InputHandle {
    pub stream: cpal::Stream,
    pub sample_rate: u32,
    pub overruns: Arc<AtomicU64>,
}

/// The default input device's capture layout: (sample_rate, channel count).
/// The profile sizes the capture and the adaptable mixer from this (P1.2: the
/// Notepad-12FX reports 4, the Scarlett 2i2 2).
pub fn default_input_config() -> Result<(u32, u16), String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("no default input device")?;
    let config = device.default_input_config().map_err(|e| format!("input config: {e}"))?;
    Ok((config.sample_rate(), config.channels()))
}

/// Open the default output device and **negotiate** its config.
///
/// The callback pops one **source frame** (`source_channels` interleaved
/// samples) per device frame and maps it across the device's channels (silence
/// when the ring is short, counted as an underrun). The rate is negotiated to
/// `requested_rate` (the session rate), so the session and device clocks agree;
/// if the device cannot run there the default is used and `rate_mismatch` is
/// reported. The stream runs until the handle is dropped.
pub fn open_output(
    ring: Arc<Spsc<f32>>,
    source_channels: u16,
    requested_rate: u32,
) -> Result<OutputHandle, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no default output device")?;
    let (config, rate_mismatch) = select_output_config(&device, requested_rate)?;
    let sample_rate = config.sample_rate();
    let channels = config.channels();
    let stream_config = config.config();
    let underruns = Arc::new(AtomicU64::new(0));
    let err: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let err_cb = {
        let err = err.clone();
        move |e: cpal::Error| *err.lock().unwrap() = Some(format!("output stream: {e}"))
    };
    let src = source_channels.clamp(1, MAX_SOURCE_CHANNELS as u16) as usize;
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_output::<f32>(&device, stream_config, ring, src, underruns.clone(), err_cb)?,
        cpal::SampleFormat::I16 => build_output::<i16>(&device, stream_config, ring, src, underruns.clone(), err_cb)?,
        cpal::SampleFormat::U16 => build_output::<u16>(&device, stream_config, ring, src, underruns.clone(), err_cb)?,
        other => return Err(format!("unsupported output sample format {other:?}")),
    };
    stream.play().map_err(|e| format!("play output: {e}"))?;
    if let Some(e) = err.lock().unwrap().take() {
        return Err(e);
    }
    Ok(OutputHandle { stream, sample_rate, channels, requested_rate, rate_mismatch, underruns })
}

/// The most source channels `fill_output` maps (the app's master is mono or
/// stereo); a larger source is clamped rather than indexed past the frame.
const MAX_SOURCE_CHANNELS: usize = 2;

/// Choose an output config: the best range that supports `requested_rate`
/// (preferring F32, then the most channels), else the device default with
/// `rate_mismatch = true` so the caller can surface it.
fn select_output_config(
    device: &cpal::Device,
    requested_rate: u32,
) -> Result<(cpal::SupportedStreamConfig, bool), String> {
    let ranges: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_output_configs()
        .map_err(|e| format!("output configs: {e}"))?
        .collect();
    let meta: Vec<(u16, u32, u32, bool)> = ranges
        .iter()
        .map(|r| {
            (
                r.channels(),
                r.min_sample_rate(),
                r.max_sample_rate(),
                r.sample_format() == cpal::SampleFormat::F32,
            )
        })
        .collect();
    if let Some(idx) = best_output_range(&meta, requested_rate) {
        let range = ranges.into_iter().nth(idx).expect("index from the same list");
        return Ok((range.with_sample_rate(requested_rate), false));
    }
    let default = device
        .default_output_config()
        .map_err(|e| format!("output config: {e}"))?;
    Ok((default, true))
}

/// The best supported output range for `requested_rate`: those whose
/// `[min, max]` contains it, preferring F32 over integer formats and then the
/// most channels. Pure, so it is unit-tested without a device.
fn best_output_range(ranges: &[(u16, u32, u32, bool)], requested: u32) -> Option<usize> {
    let mut best: Option<(usize, (u8, u16))> = None;
    for (i, &(channels, min, max, is_f32)) in ranges.iter().enumerate() {
        if min <= requested && requested <= max {
            let key = (u8::from(is_f32), channels);
            if best.is_none_or(|(_, b)| key > b) {
                best = Some((i, key));
            }
        }
    }
    best.map(|(i, _)| i)
}

fn build_output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    ring: Arc<Spsc<f32>>,
    source_channels: usize,
    underruns: Arc<AtomicU64>,
    err_cb: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                fill_output(data, &ring, source_channels, channels, &underruns);
            },
            err_cb,
            None,
        )
        .map_err(|e| format!("build output stream: {e}"))
}

/// Fill an interleaved output buffer from a `source_channels`-interleaved source
/// ring, **one source frame per device frame** — the per-frame pop is what stops
/// a stereo callback halving the rate and splitting L/R (a real-hardware bug;
/// unit-tested so CI can see it).
///
/// A short ring plays silence for that frame and counts an **underrun**; a
/// partial source frame is never consumed, so L/R stay aligned across a starve.
/// The frame is then mapped across the device's channels: passthrough when the
/// counts match, duplicated from a mono source, and averaged for a mono device
/// (never silently dropping a channel).
fn fill_output<T>(
    data: &mut [T],
    ring: &Spsc<f32>,
    source_channels: usize,
    device_channels: usize,
    underruns: &AtomicU64,
) where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let device_channels = device_channels.max(1);
    let source_channels = source_channels.clamp(1, MAX_SOURCE_CHANNELS);
    let mut frame = [0.0f32; MAX_SOURCE_CHANNELS];
    for out_frame in data.chunks_mut(device_channels) {
        if ring.len() >= source_channels {
            for s in frame.iter_mut().take(source_channels) {
                *s = ring.try_pop().unwrap_or(0.0);
            }
        } else {
            underruns.fetch_add(1, Ordering::Relaxed);
            frame[..source_channels].fill(0.0);
        }
        for (k, d) in out_frame.iter_mut().enumerate() {
            let v = if device_channels == 1 && source_channels > 1 {
                frame[..source_channels].iter().sum::<f32>() / source_channels as f32
            } else if source_channels == 1 {
                frame[0]
            } else if k < source_channels {
                frame[k]
            } else {
                0.0
            };
            *d = T::from_sample(v);
        }
    }
}

/// Open the default input device; the callback converts each captured sample
/// to f32 and pushes it into `ring`. The stream runs until the handle drops.
pub fn open_input(ring: Arc<Spsc<f32>>) -> Result<InputHandle, String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("no default input device")?;
    let config = device.default_input_config().map_err(|e| format!("input config: {e}"))?;
    let sample_rate = config.sample_rate();
    let stream_config: cpal::StreamConfig = config.into();
    let err: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let err_cb = {
        let err = err.clone();
        move |e: cpal::Error| *err.lock().unwrap() = Some(format!("input stream: {e}"))
    };
    let overruns = Arc::new(AtomicU64::new(0));
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_input::<f32>(&device, stream_config, ring, overruns.clone(), err_cb)?,
        cpal::SampleFormat::I16 => build_input::<i16>(&device, stream_config, ring, overruns.clone(), err_cb)?,
        cpal::SampleFormat::U16 => build_input::<u16>(&device, stream_config, ring, overruns.clone(), err_cb)?,
        other => return Err(format!("unsupported input sample format {other:?}")),
    };
    stream.play().map_err(|e| format!("play input: {e}"))?;
    if let Some(e) = err.lock().unwrap().take() {
        return Err(e);
    }
    Ok(InputHandle { stream, sample_rate, overruns })
}

fn build_input<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    ring: Arc<Spsc<f32>>,
    overruns: Arc<AtomicU64>,
    err_cb: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::Sample<Float = f32>,
{
    let channels = config.channels as usize;
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                fill_input(data, &ring, channels, &overruns);
            },
            err_cb,
            None,
        )
        .map_err(|e| format!("build input stream: {e}"))
}

/// Fill a mono source ring from an interleaved input buffer, **per frame**:
/// push one sample per frame (channel 0) — the mirror of `fill_output`. Without
/// it a stereo input pushes L,R,L,R into the mono ring (every sample of an
/// interleaved buffer), recording at 2× rate with channels alternated — the
/// input-side twin of the output bug.
fn fill_input<T>(data: &[T], ring: &Spsc<f32>, channels: usize, overruns: &AtomicU64)
where
    T: cpal::SizedSample + cpal::Sample<Float = f32>,
{
    let channels = channels.max(1);
    for frame in data.chunks(channels) {
        let Some(&first) = frame.first() else { continue };
        if !ring.try_push(first.to_float_sample()) {
            overruns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono source is duplicated across the device's channels, one source
    /// sample per FRAME (not per output sample — that would halve the rate).
    #[test]
    fn output_duplicates_mono_source_across_channels() {
        let ring = Spsc::new(16);
        for v in [0.1f32, 0.2, 0.3, 0.4] {
            assert!(ring.try_push(v), "ring must accept the source");
        }
        let mut out = [0.0f32; 8]; // 4 stereo frames
        let u = AtomicU64::new(0);
        fill_output(&mut out, &ring, 1, 2, &u);
        assert_eq!(out, [0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4]);
        assert_eq!(u.load(Ordering::Relaxed), 0, "no starve");
    }

    /// A stereo source passes through a stereo device frame-for-frame.
    #[test]
    fn output_passes_stereo_source_through() {
        let ring = Spsc::new(16);
        for v in [0.1f32, 0.9, 0.2, 0.8] {
            assert!(ring.try_push(v));
        }
        let mut out = [0.0f32; 4]; // 2 stereo frames
        let u = AtomicU64::new(0);
        fill_output(&mut out, &ring, 2, 2, &u);
        assert_eq!(out, [0.1, 0.9, 0.2, 0.8], "L/R stay paired");
        assert_eq!(u.load(Ordering::Relaxed), 0);
    }

    /// A mono device averages a stereo source instead of dropping a channel.
    #[test]
    fn output_downmixes_stereo_to_a_mono_device() {
        let ring = Spsc::new(16);
        for v in [0.4f32, 0.2] {
            assert!(ring.try_push(v));
        }
        let mut out = [0.0f32; 1];
        let u = AtomicU64::new(0);
        fill_output(&mut out, &ring, 2, 1, &u);
        assert!((out[0] - 0.3).abs() < 1e-6, "mono device averages L/R, got {}", out[0]);
    }

    #[test]
    fn output_silence_when_empty_counts_underruns() {
        let ring = Spsc::new(16);
        let mut out = [1.0f32; 4]; // 2 stereo frames
        let u = AtomicU64::new(0);
        fill_output(&mut out, &ring, 2, 2, &u);
        assert_eq!(out, [0.0; 4], "empty ring must play silence");
        assert_eq!(u.load(Ordering::Relaxed), 2, "each starved frame is counted");
    }

    /// A partial source frame is **not** consumed: both frames starve and the
    /// lone sample stays put, so L/R never swap across the gap.
    #[test]
    fn output_partial_frame_starves_without_misaligning() {
        let ring = Spsc::new(16);
        assert!(ring.try_push(0.1)); // half of an expected stereo pair
        let mut out = [9.0f32; 4];
        let u = AtomicU64::new(0);
        fill_output(&mut out, &ring, 2, 2, &u);
        assert_eq!(out, [0.0; 4], "a partial frame plays silence");
        assert_eq!(u.load(Ordering::Relaxed), 2);
        assert_eq!(ring.len(), 1, "the lone sample is left for a complete frame");
    }

    #[test]
    fn output_zero_channels_degrades_to_mono_not_crash() {
        // documents why `fill_output` guards device_channels.max(1): chunks_mut(0)
        // panics. A zero-channel device degrades to mono, never crashes.
        let ring = Spsc::new(16);
        for _ in 0..4 {
            assert!(ring.try_push(0.5));
        }
        let mut out = [1.0f32; 4];
        let u = AtomicU64::new(0);
        fill_output(&mut out, &ring, 1, 0, &u);
        assert_eq!(out, [0.5; 4]);
    }

    /// Rate negotiation prefers F32 and the most channels at the requested rate,
    /// and reports "no range" when the rate is unsupported (so the caller falls
    /// back and flags the mismatch rather than lying about the clock).
    #[test]
    fn output_range_selection_prefers_f32_stereo_at_the_rate() {
        let ranges = [
            (2u16, 44_100u32, 48_000u32, false), // i16 stereo covering 48k
            (1, 44_100, 192_000, true),          // f32 mono
            (2, 44_100, 192_000, true),          // f32 stereo
        ];
        assert_eq!(best_output_range(&ranges, 48_000), Some(2), "f32 stereo wins");
        assert_eq!(best_output_range(&ranges, 96_000), Some(2));
        assert_eq!(best_output_range(&ranges, 22_050), None, "below every min -> fall back");
    }

    #[test]
    fn input_pushes_one_sample_per_frame() {
        // stereo input buffer interleaved [L0,R0,L1,R1,...]; the mono ring must get
        // channel 0 per FRAME (not every sample — which would double the rate).
        let ring = Spsc::new(16);
        let data = [0.1f32, 0.9, 0.2, 0.8, 0.3, 0.7];
        let overruns = AtomicU64::new(0);
        fill_input(&data, &ring, 2, &overruns);
        assert_eq!(ring.try_pop(), Some(0.1));
        assert_eq!(ring.try_pop(), Some(0.2));
        assert_eq!(ring.try_pop(), Some(0.3));
        assert_eq!(ring.try_pop(), None, "exactly one sample per frame");
        assert_eq!(overruns.load(Ordering::Relaxed), 0);
    }
}
