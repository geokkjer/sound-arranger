//! The arrangement render node (P1.3, `arranger`): one opaque `ArrangerNode` per
//! track, interpreting a [`Timeline`] value on the render path.
//!
//! The node reads a track's `Clip`s (sorted by `at_frame`, from
//! [`crate::timeline`]) and, for each block, produces the track's audio:
//! - **active-clip selection** by overlapping `[frame, frame+BLOCK)` with each
//!   clip's span `[at_frame, end)` (a sorted-by-`at_frame` scan, no allocation);
//! - **per-read-position streaming** — one reader per active clip *instance*
//!   (a source read at two `src_start` offsets is two readers; kimi must-fix 3),
//!   warmed on the control side and detached (never joined) on retire;
//! - **overlap summing** — layered clips sum; a gap is silence;
//! - **per-clip fades** — `fade_in`/`fade_out` are *authoritative* (the
//!   auto-splice/equal-power boundary crossfade is not applied here, so there is
//!   no double-fade; kimi must-fix 2);
//!
//! The render path allocates nothing and only pops rings. **Determinism scope:**
//! byte-identical output holds for a **reader that never underruns**. An underrun
//! is counted and is a *hard error the bounce must surface* (assert `underruns ==
//! 0`), because an underrun cannot be recovered without shifting every later
//! sample of that clip (an SPSC ring has no random access — you cannot skip a
//! frame you never received). The module asserts `popped + off0 == off` in debug
//! (skipped at a source's end-of-file, where `off` correctly runs ahead into
//! silence) so a test that slips fails loudly instead of shipping shifted audio.
//! The profile
//! must also render **contiguously from frame 0** (a bounce from an offset would
//! begin every reader at its source frame 0 while `off` claims otherwise).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::{AudioNode, CAP_EVENTS, EventBuf, NodeIO, NoteEvent, RenderBlock, Trigger};

use crate::stream::{ClipRef, FilePlayer};
use crate::timeline::{Clip, Id, Track};

/// Resolves a pool source id to a WAV path (the media pool's id → path index;
/// supplied by the pool in P1.3.3).
pub type PoolResolver = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// A clip's stream on the node: the reader, frames popped *relative to the
/// reader's start* (`popped`), and the clip offset the reader was built at
/// (`off0`). `off0` is how far into the clip the transport was when the node was
/// (re)built — a rebuilt-at-current-frame reader must begin there, not at the
/// clip's source start, or it plays the wrong region (and `popped + off0` is
/// what the render path compares against the absolute clip offset `off`).
struct ClipReader {
    player: FilePlayer,
    popped: u64,
    off0: u64,
}

/// One track's arrangement interpreter — an opaque audio node (`out("audio")`).
pub struct ArrangerNode {
    clips: Vec<Clip>,
    readers: HashMap<Id, ClipReader>,
    underruns: Arc<AtomicU64>,
}

fn warm(player: &FilePlayer, want: u64, cap: usize) -> Result<(), String> {
    let target = (cap as u64).min(want);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while player.produced() < target && !player.eof() {
        if std::time::Instant::now() > deadline {
            return Err("arranger reader did not warm up in 10 s".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}

fn pop_sample(reader: &mut ClipReader, underruns: &AtomicU64) -> f32 {
    match reader.player.ring().try_pop() {
        Some(v) => {
            reader.popped += 1;
            v
        }
        None => {
            // The reader produces `src_len - off0` frames (its `expected`); the
            // clip is done when the reader's own frame count reaches that.
            if reader.player.eof() || reader.popped >= reader.player.expected() {
                0.0 // legitimate: clip done
            } else {
                underruns.fetch_add(1, Ordering::Relaxed);
                0.0
            }
        }
    }
}

/// The per-clip fade gain: linear ramp up over `fade_in` and down over
/// `fade_out` (authoritative per-clip fades; `off` is frames into the clip).
fn fade_gain(c: &Clip, off: u64) -> f32 {
    let gin = if c.fade_in > 0 {
        (off as f32 / c.fade_in as f32).min(1.0)
    } else {
        1.0
    };
    let gout = if c.fade_out > 0 {
        let remaining = c.src_len.saturating_sub(off);
        (remaining as f32 / c.fade_out as f32).min(1.0)
    } else {
        1.0
    };
    gin.min(gout)
}

impl ArrangerNode {
    /// Build the node for a track, creating and warming one reader per clip.
    /// Control side only (threads, file opens, ring warm-up happen here).
    /// `session_rate` is the engine's sample rate; a clip whose source differs is
    /// **refused** (a rate-mismatched take would play pitch-shifted with no
    /// diagnostic — a real-hardware-invisible bug). `from_frame` is the transport
    /// frame the node is being built at: a reader is positioned `off0 =
    /// `from_frame - at_frame` frames into the clip (clamped to its length), so a
    /// node rebuilt mid-transport continues the clip instead of restarting it.
    pub fn new(
        track: Track,
        resolve: &PoolResolver,
        ring_capacity: usize,
        session_rate: u32,
        from_frame: u64,
    ) -> Result<Self, String> {
        let mut readers = HashMap::new();
        for c in &track.clips {
            // a hand-built Track bypasses Timeline validation — validate here so
            // an invalid clip (zero length, NaN gain, bad loop, frame overflow)
            // never reaches the readers.
            crate::timeline::validate_clip(c).map_err(|e| format!("arranger: {e}"))?;
            let path = resolve(&c.source)
                .ok_or_else(|| format!("arranger: pool has no source '{}'", c.source))?;
            let source_reader = crate::wav::WavReader::open(&path)
                .map_err(|e| format!("arranger: source {}: {e}", c.source))?;
            let src_rate = source_reader.sample_rate();
            // The source's own length: a clip whose declared region runs past its source
            // (a session that says "play 24 s of an 8 s take") is tolerated, not an error —
            // it plays what the source has and silence after. That is what a **warm-up
            // seek** needs: it mounts readers *at an offset* (a full replay warms them
            // from 0 instead), and mounting past EOF must behave like the full path, not
            // refuse, or a seek that used to work would fail after the optimisation.
            let src_frames = source_reader.total_frames();
            if src_rate != session_rate {
                return Err(format!(
                    "clip '{}' source is {src_rate} Hz but the session is {session_rate} Hz (a pool source is converted to the session rate when the pool is adopted — import it through `Pool::import`/`Pool::conform`, or re-point the pool)",
                    c.id
                ));
            }
            // How far into the clip the transport already is. If the clip hasn't
            // started (`from_frame <= at_frame`) this is 0 (read from its start); if it
            // has already ended, clamp to the length (the reader emits silence).
            //
            // The source's own end bounds it too, but **only for a contiguous read**, and
            // it must be measured from `src_start` — the reader mounts at
            // `src_start + phase`, so the material left is `src_frames - src_start`. A
            // *looped* clip is left alone: its phase is `off0 % loop_len` and its cycle
            // count is `off0 / loop_len`, both of which the bound would move (the gate
            // reproduced a debug panic on the alignment invariant and, in release, a warm
            // seek playing the wrong cycle) — a looping reader past EOF is already dead by
            // construction, so it needs no help.
            let phase = from_frame.saturating_sub(c.at_frame);
            let off0 = match c.loop_len {
                Some(r) if r > 0 => phase.min(c.src_len),
                _ => phase
                    .min(c.src_len)
                    .min(src_frames.saturating_sub(c.src_start)),
            };
            let clip_ref = ClipRef {
                path,
                start: c.src_start,
                len: c.src_len,
            };
            let player =
                FilePlayer::start_looped_anchored(clip_ref, c.loop_len, ring_capacity, off0)?;
            warm(&player, c.src_len.saturating_sub(off0), ring_capacity)?;
            readers.insert(
                c.id.clone(),
                ClipReader {
                    player,
                    popped: 0,
                    off0,
                },
            );
        }
        Ok(ArrangerNode {
            clips: track.clips,
            readers,
            underruns: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// A handle to the underrun counter so a host can keep reading it after the
    /// node is moved into the graph (the render path writes, the control side
    /// reads — no allocation, no blocking).
    pub fn underruns_arc(&self) -> Arc<AtomicU64> {
        self.underruns.clone()
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// Reset the underrun counter (a test/control convenience, not the render path).
    pub fn reset_underruns(&mut self) {
        self.underruns.store(0, Ordering::Relaxed);
    }
}

impl AudioNode for ArrangerNode {
    fn latency(&self) -> u32 {
        0 // disk read-ahead is a stream buffer, not processing latency
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
        let f0 = block.frame;
        let f1 = f0 + out.len() as u64;
        debug_assert!(out.len() <= engine::BLOCK, "render chunk exceeds BLOCK");
        let mut acc = [0.0f32; engine::BLOCK];
        for c in self.clips.iter() {
            // `ArrangerNode::new` validated every clip on the track (`validate_clip`
            // bounds the span), so this is the clip's end. The saturating fallback keeps
            // the block loop *total* for a hand-built node — a wrapped end here would
            // clip the block short and a panic here would stop the render — and it only
            // differs from the true end for a span that cannot be represented, which
            // `new` refuses.
            let c_end = c.end().unwrap_or(u64::MAX);
            if c_end <= f0 {
                continue; // fully before this block
            }
            if c.at_frame >= f1 {
                break; // sorted by at_frame: nothing later is active
            }
            let start = c.at_frame.max(f0);
            let end = c_end.min(f1);
            let j0 = (start - f0) as usize;
            let j1 = (end - f0) as usize;
            let Some(reader) = self.readers.get_mut(&c.id) else {
                // A clip with no reader (a value change landed without reconcile) —
                // count it as an underrun and play silence, deterministic.
                self.underruns.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            for (k, slot) in acc[j0..j1].iter_mut().enumerate() {
                let fr = f0 + (j0 + k) as u64;
                let off = fr - c.at_frame;
                // Alignment: for contiguous-from-`off0` rendering, the reader's
                // own `popped` plus its offset `off0` must equal the clip's
                // absolute offset `off`. A slip (underrun) or an offset render
                // violates it — fail loud in debug rather than silently shifting
                // the rest of the clip. At a legitimate end (source EOF or fully
                // popped), off runs ahead into silence and popped is capped; that
                // is not a slip.
                let at_end = reader.player.eof() || reader.popped >= reader.player.expected();
                if !at_end {
                    debug_assert_eq!(
                        reader.popped + reader.off0,
                        off,
                        "clip '{}' reader slipped (off {off}, popped {} + off0 {})",
                        c.id,
                        reader.popped,
                        reader.off0
                    );
                }
                let s = pop_sample(reader, &self.underruns);
                *slot += s * c.gain * fade_gain(c, off);
            }
        }
        let len = out.len().min(engine::BLOCK);
        out[..len].copy_from_slice(&acc[..len]);
    }
}

#[cfg(test)]
#[path = "tests/arranger.rs"]
mod tests;
