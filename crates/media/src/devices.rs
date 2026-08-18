//! The cpal device path (Spike B note, `devices`): enumeration/selection and
//! input/output stream builders whose callbacks only push/pop an SPSC ring —
//! never allocate, never block. Thin and isolated here so cpal API drift
//! (0.18.x is pre-1.0) cannot leak into the machinery the numeric tests cover.

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

/// An open input stream plus the device's actual sample rate.
pub struct InputHandle {
    pub stream: cpal::Stream,
    pub sample_rate: u32,
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
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for d in data.iter_mut() {
                    *d = T::from_sample(ring.try_pop().unwrap_or(0.0));
                }
            },
            err_cb,
            None,
        )
        .map_err(|e| format!("build output stream: {e}"))
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
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_input::<f32>(&device, stream_config, ring, err_cb)?,
        cpal::SampleFormat::I16 => build_input::<i16>(&device, stream_config, ring, err_cb)?,
        cpal::SampleFormat::U16 => build_input::<u16>(&device, stream_config, ring, err_cb)?,
        other => return Err(format!("unsupported input sample format {other:?}")),
    };
    stream.play().map_err(|e| format!("play input: {e}"))?;
    if let Some(e) = err.lock().unwrap().take() {
        return Err(e);
    }
    Ok(InputHandle { stream, sample_rate })
}

fn build_input<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    ring: Arc<Spsc<f32>>,
    err_cb: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::Sample<Float = f32>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                for d in data {
                    let _ = ring.try_push(d.to_float_sample());
                }
            },
            err_cb,
            None,
        )
        .map_err(|e| format!("build input stream: {e}"))
}
