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

/// An open output stream plus the device's actual sample rate and channel
/// count (the caller generates content for that clock).
pub struct OutputHandle {
    pub stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
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

/// Open the default output device; the callback pops mono frames from `ring`
/// (silence when empty) and duplicates them across channels. The stream runs
/// until the handle is dropped.
pub fn open_output(ring: Arc<Spsc<f32>>) -> Result<OutputHandle, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no default output device")?;
    let config = device.default_output_config().map_err(|e| format!("output config: {e}"))?;
    let sample_rate = config.sample_rate();
    let channels = config.channels();
    let stream_config: cpal::StreamConfig = config.into();
    let err: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let err_cb = {
        let err = err.clone();
        move |e: cpal::Error| *err.lock().unwrap() = Some(format!("output stream: {e}"))
    };
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_output::<f32>(&device, stream_config, ring, err_cb)?,
        cpal::SampleFormat::I16 => build_output::<i16>(&device, stream_config, ring, err_cb)?,
        cpal::SampleFormat::U16 => build_output::<u16>(&device, stream_config, ring, err_cb)?,
        other => return Err(format!("unsupported output sample format {other:?}")),
    };
    stream.play().map_err(|e| format!("play output: {e}"))?;
    if let Some(e) = err.lock().unwrap().take() {
        return Err(e);
    }
    Ok(OutputHandle { stream, sample_rate, channels })
}

fn build_output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    ring: Arc<Spsc<f32>>,
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
                fill_output(data, &ring, channels);
            },
            err_cb,
            None,
        )
        .map_err(|e| format!("build output stream: {e}"))
}

/// Fill an interleaved output buffer from a mono source ring, **per frame**:
/// pop one sample, duplicate it across the frame's channels (mono→stereo/...).
/// Without the per-frame pop a stereo callback plays every *other* sample at
/// half rate (a real-hardware bug; unit-tested here so CI can see it).
fn fill_output<T>(data: &mut [T], ring: &Spsc<f32>, channels: usize)
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = channels.max(1);
    for frame in data.chunks_mut(channels) {
        let s = T::from_sample(ring.try_pop().unwrap_or(0.0));
        for d in frame.iter_mut() {
            *d = s;
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

    #[test]
    fn output_duplicates_each_frame_across_channels() {
        // A mono source ring; a stereo output buffer must get each MONO sample
        // once per FRAME, duplicated across L and R — not one sample per output
        // sample (that would halve the rate and split L/R incorrectly).
        let ring = Spsc::new(16);
        for v in [0.1f32, 0.2, 0.3, 0.4] {
            assert!(ring.try_push(v), "ring must accept the source");
        }
        let mut out = [0.0f32; 8]; // 4 stereo frames
        fill_output(&mut out, &ring, 2);
        assert_eq!(out, [0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn output_passes_mono_through() {
        let ring = Spsc::new(16);
        for v in [0.5f32, -0.5] {
            assert!(ring.try_push(v));
        }
        let mut out = [0.0f32; 2];
        fill_output(&mut out, &ring, 1);
        assert_eq!(out, [0.5, -0.5]);
    }

    #[test]
    fn output_silence_when_empty() {
        let ring = Spsc::new(16);
        let mut out = [1.0f32; 4];
        fill_output(&mut out, &ring, 2);
        assert_eq!(out, [0.0; 4], "empty ring must play silence");
    }

    #[test]
    fn output_zero_channels_degrades_to_mono_not_crash() {
        // documents why `fill_output` guards channels.max(1): chunks_mut(0) panics.
        // A hypothetical zero-channel device degrades to mono (fills the buffer),
        // never crashes.
        let ring = Spsc::new(16);
        for _ in 0..4 {
            assert!(ring.try_push(0.5));
        }
        let mut out = [1.0f32; 4];
        fill_output(&mut out, &ring, 0);
        assert_eq!(out, [0.5; 4]);
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
