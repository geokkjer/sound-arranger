//! The recording writer (Spike B note, `record`).
//!
//! [`Recorder`] is the composition-seams seam: capture audio to a media pool,
//! with providers behind it (WAV via [`WavRecorder`] today; the Notepad-12FX
//! routing via `nusb` later). [`RecordNode`] is the graph's opaque sink
//! (`in("audio")`): it pushes its input into the recorder's ring — the audio
//! path only pushes an SPSC ring. The writer thread drains into a
//! [`crate::wav::WavWriter`] whose header is crash-recoverable from the first
//! byte ([`crate::wav::WavWriter::recover`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use engine::{AudioNode, CAP_EVENTS, EventBuf, NodeIO, NoteEvent, RenderBlock, Trigger};

use crate::ring::Spsc;
use crate::stream::DEFAULT_RING_CAPACITY;
use crate::wav::WavWriter;

/// The recorder seam (composition-seams note): capture audio to a media pool.
/// Providers implement it (WAV; Notepad-12FX routing later).
///
/// Contract: `push` is on the audio path — it must never allocate or block.
/// `stop()` is a control-side call and must not race the render thread: call
/// it only when the transport is quiescent (render stopped). A frame pushed
/// just after `stop()` begins may be counted by `frames_pushed` but not
/// written — the writer drains the ring once, sees the stop flag, and exits
/// (kimi review finding 4; the spike's tests stop after rendering; Phase 1
/// gates `RecordNode::render` on transport state and logs a `RecordStop`
/// frame so the drain is deterministic).
pub trait Recorder: Send + Sync {
    fn id(&self) -> &'static str;
    /// Audio path: push one mono frame.
    fn push(&self, frame: f32);
    /// Control side: stop the take and finalize the file. Idempotent —
    /// but *single-caller*: concurrent `stop()` calls are not serialized
    /// (a second caller may return before the first's finalize completes).
    fn stop(&self) -> Result<(), String>;
    fn path(&self) -> &Path;
}

/// A ring-fed WAV recorder: the writer thread drains the ring into a WAV file;
/// `stop()` stops the feed, drains, and finalizes the header. Frames pushed
/// while the ring is full are dropped and counted (overruns) — the soak test
/// asserts zero of those.
pub struct WavRecorder {
    ring: Arc<Spsc<f32>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    stop_flag: Arc<AtomicBool>,
    err: Arc<Mutex<Option<String>>>,
    path: PathBuf,
    sample_rate: u32,
    frames: AtomicU64,
    overruns: AtomicU64,
    finalized: AtomicBool,
}

impl WavRecorder {
    /// Start a take: open the file (placeholder header — crash-recoverable
    /// from the first byte) and spawn the writer thread. Control side.
    pub fn start(path: impl Into<PathBuf>, sample_rate: u32) -> Result<Self, String> {
        let path = path.into();
        let mut writer = WavWriter::create(&path, sample_rate, 1).map_err(|e| e.to_string())?;
        let ring = Arc::new(Spsc::new(DEFAULT_RING_CAPACITY));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (ring2, stop2, err2) = (ring.clone(), stop_flag.clone(), err.clone());

        let handle = std::thread::Builder::new()
            .name("media-recorder".into())
            .spawn(move || {
                let mut buf = [0.0f32; 1024];
                loop {
                    let mut n = 0usize;
                    while n < buf.len() {
                        match ring2.try_pop() {
                            Some(v) => {
                                buf[n] = v;
                                n += 1;
                            }
                            None => break,
                        }
                    }
                    if n > 0
                        && let Err(e) = writer.write(&buf[..n])
                    {
                        *err2.lock().unwrap() = Some(e);
                        return;
                    }
                    // Flush every chunk: a crashed take (no finalize) must not
                    // lose the tail sitting in the writer's buffer.
                    if n > 0
                        && let Err(e) = writer.flush()
                    {
                        *err2.lock().unwrap() = Some(e);
                        return;
                    }
                    if stop2.load(Ordering::Acquire) && n == 0 {
                        break;
                    }
                    if n == 0 {
                        std::thread::sleep(std::time::Duration::from_micros(50));
                    }
                }
                if let Err(e) = writer.finalize() {
                    *err2.lock().unwrap() = Some(e);
                }
            })
            .map_err(|e| format!("recorder thread spawn: {e}"))?;

        Ok(WavRecorder {
            ring,
            handle: Mutex::new(Some(handle)),
            stop_flag,
            err,
            path,
            sample_rate,
            frames: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            finalized: AtomicBool::new(false),
        })
    }

    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    pub fn frames_pushed(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl Recorder for WavRecorder {
    fn id(&self) -> &'static str {
        "wav"
    }

    fn push(&self, frame: f32) {
        if self.stop_flag.load(Ordering::Acquire) {
            return;
        }
        if self.ring.try_push(frame) {
            self.frames.fetch_add(1, Ordering::Relaxed);
        } else {
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn stop(&self) -> Result<(), String> {
        if self.finalized.swap(true, Ordering::AcqRel) {
            return Ok(()); // idempotent
        }
        self.stop_flag.store(true, Ordering::Release);
        // Join the writer: it drains the ring and finalizes the header.
        let handle = self.handle.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        self.err.lock().unwrap().clone().map_or(Ok(()), Err)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WavRecorder {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// The graph's recording sink: `in("audio")` → the recorder's ring. Owns an
/// `Arc` to the recorder so the control side can stop the take independently.
pub struct RecordNode {
    recorder: Arc<dyn Recorder>,
}

impl RecordNode {
    pub fn new(recorder: Arc<dyn Recorder>) -> Self {
        RecordNode { recorder }
    }

    pub fn recorder(&self) -> Arc<dyn Recorder> {
        self.recorder.clone()
    }
}

impl AudioNode for RecordNode {
    fn latency(&self) -> u32 {
        0
    }

    fn render(
        &mut self,
        io: &NodeIO,
        _out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _block: RenderBlock,
    ) {
        for &s in io.audio_in {
            self.recorder.push(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_roundtrips_through_the_writer_thread() {
        let path = std::env::temp_dir().join(format!("media-record-rt-{}.wav", std::process::id()));
        let rec: Arc<dyn Recorder> = Arc::new(WavRecorder::start(&path, 48_000).unwrap());
        // 50k samples: below the 64k ring capacity, so no overrun is possible
        // regardless of writer-thread timing; the roundtrip proves the thread.
        let n = 50_000u64;
        let samples: Vec<f32> = (0..n).map(|i| (i % 1000) as f32 / 1000.0 - 0.5).collect();
        for s in &samples {
            rec.push(*s);
        }
        rec.stop().unwrap();
        drop(rec);

        assert_eq!(
            crate::wav::WavReader::open(&path).unwrap().total_frames(),
            n
        );
        let mut r = crate::wav::WavReader::open(&path).unwrap();
        let mut back = vec![0.0f32; n as usize];
        assert_eq!(r.read_into(&mut back), n as usize);
        for (a, b) in samples.iter().zip(&back) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recorder_ignores_pushes_after_stop() {
        let path =
            std::env::temp_dir().join(format!("media-record-stop-{}.wav", std::process::id()));
        let rec: Arc<dyn Recorder> = Arc::new(WavRecorder::start(&path, 48_000).unwrap());
        for _ in 0..100 {
            rec.push(0.25);
        }
        rec.stop().unwrap();
        rec.push(0.5); // must be ignored
        drop(rec);
        assert_eq!(
            crate::wav::WavReader::open(&path).unwrap().total_frames(),
            100
        );
        let _ = std::fs::remove_file(&path);
    }
}
