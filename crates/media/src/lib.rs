//! The media engine (Phase 0, Spike B) — *core-privileged, not a plugin*
//! (minimal-core note §5): disk streaming for long-form audio, the recording
//! writer with crash recovery, input↔output device-clock drift reconciliation,
//! and the cpal device path. A sibling of the std-only core in `crates/engine`;
//! media nodes mount into the engine graph as opaque-tier
//! [`engine::AudioNode`]s (the patch bay's own opaque tier — no core change).
//!
//! Modules:
//! - [`ring`] — lock-free SPSC ring, the audio-path carrier;
//! - [`wav`] — minimal RIFF reader/writer with header finalization + recovery;
//! - [`stream`] — the long-file player and splice-during-playback;
//! - [`record`] — the `Recorder` seam and the WAV recorder provider;
//! - [`drift`] — device-clock drift reconciliation;
//! - [`resample`] — band-limited sample-rate conversion (the pool boundary);
//! - [`devices`] — the cpal device path (enumerate, open input + output).
//!
//! Invariants (Spike B note):
//! - the render path never allocates or blocks: nodes only pop/push SPSC rings;
//!   all thread spawning, file opening, and ring warming happen on the control
//!   side (command-issue time);
//! - media determinism: the same command sequence on fresh engines produces
//!   byte-identical bounce WAVs (tested). Media mounts bypass the core log in
//!   the spike (the flat-f32 `Event::Mount` params cannot carry a file handle —
//!   a core-shape finding); the core's own invariant is unchanged;
//! - a take is crash-recoverable from the first byte: the WAV header is
//!   written with placeholder sizes before any audio, and [`wav::WavWriter::recover`]
//!   patches it after a crash.
//!
//! Phase 1 adds the profile-level helper [`bounce`]: render the engine's
//! master out (owned by the mixer plugin) to a 16-bit WAV — 16-bit at
//! bounce/export; the media *pool* keeps 32-bit float sources.

pub mod arranger;
pub mod capture;
pub mod clip_editor;
pub mod devices;
pub mod drift;
pub mod peaks;
pub mod pool;
pub mod record;
pub mod resample;
pub mod ring;
pub mod stream;
pub mod timeline;
pub mod wav;

use std::path::Path;

pub use arranger::{ArrangerNode, PoolResolver};
pub use capture::{Capture, CaptureNode};
pub use clip_editor::{decode_op, encode_op, register_handlers, ClipEditor, Interner};
pub use drift::DriftCompensator;
pub use peaks::{PeakBuilder, PeakFile, PEAK_BASE_BIN, PEAK_LEVELS};
pub use pool::{Conform, ConformReport, Pool, PoolIndex, PoolSource, Recovery};
pub use record::{Recorder, RecordNode, WavRecorder};
pub use resample::Resampler;
pub use ring::Spsc;
pub use stream::{mailbox, ClipRef, FilePlayer, Mailbox, PlaybackNode, SpliceCmd, DEFAULT_RING_CAPACITY};
pub use timeline::{ArrangeOp, Clip, Edge, Frame, Id, Timeline, Track};
pub use wav::{WavReader, WavWriter};

/// Bounce: render `frames` of the engine's master out and write it to a
/// 16-bit WAV (16-bit at bounce/export; the media pool keeps float sources).
/// The channel count follows the master (the mixer's stereo out → a 2-channel
/// WAV); a mono master still writes mono.
pub fn bounce(e: &mut engine::Engine, frames: usize, path: &Path) -> Result<(), String> {
    // `render` flushes scheduled mounts first, which may change the master's
    // channel count (e.g. a scheduled mixer mount); read the width afterwards.
    let out = e.render(frames);
    let channels = e.graph.out_channels().max(1);
    let mut w = wav::WavWriter::create(path, e.clock.sample_rate, channels as u16)?;
    w.write(&out)?;
    w.finalize()?;
    Ok(())
}
