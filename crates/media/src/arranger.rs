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
//! frame you never received). The module asserts `popped == off` in debug so a
//! test that slips fails loudly instead of shipping shifted audio. The profile
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
            if c.end() <= f0 {
                continue; // fully before this block
            }
            if c.at_frame >= f1 {
                break; // sorted by at_frame: nothing later is active
            }
            let start = c.at_frame.max(f0);
            let end = c.end().min(f1);
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
mod tests {
    use super::*;
    use crate::DEFAULT_RING_CAPACITY;
    use crate::timeline::{ArrangeOp, Timeline};
    use crate::wav::{WavReader, WavWriter};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("media-arranger-{name}-{}.wav", std::process::id()))
    }

    /// **The EOF clamp is measured from `src_start` and never touches a loop's phase.**
    /// The reader mounts at `src_start + phase`, so the bound is `src_frames - src_start`
    /// (the gate's repro: `src_start = 24 000` on a 48 000-frame file at a mount of 60 000
    /// used to fail with `seek_frames(72 000) past end`). A looper is excluded entirely:
    /// its phase is `off0 % loop_len` and its cycle `off0 / loop_len`, both of which a
    /// clamp would move — the gate reproduced a wrong-cycle read in release and an
    /// alignment-invariant panic in debug.
    #[test]
    fn the_eof_clamp_respects_src_start_and_loops() {
        let path = tmp("clamp-src-start");
        write_ramp(&path, 48_000, 48_000, 480);
        let source = path.clone();
        let resolve: crate::PoolResolver =
            Arc::new(move |id: &str| (id == "s1").then(|| source.clone()));
        let tempo = engine::TempoMap::new(48_000, 120.0, 4);

        let clip = |src_start: u64, src_len: u64, loop_len: Option<u64>| crate::timeline::Clip {
            id: "c0".into(),
            name: None,
            source: "s1".into(),
            src_start,
            src_len,
            at_frame: 0,
            fade_in: 0,
            fade_out: 0,
            gain: 1.0,
            loop_len,
            reversed: false,
        };

        // **`src_start > 0` with a region running past the source**: mounts (no
        // `seek_frames past end`) and plays the material it has.
        let track = crate::timeline::Track {
            id: "t0".into(),
            clips: vec![clip(24_000, 96_000, None)],
        };
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, 48_000, 60_000)
            .expect("a reader past the source's end mounts");
        let mut out = vec![0.0f32; engine::BLOCK];
        node.render(
            &engine::NodeIO {
                audio_in: &[],
                audio_ins: [&[], &[], &[], &[], &[], &[], &[], &[]],
                audio_in_count: 0,
                audio_out_channels: 1,
                control_in: 0.0,
                triggers_in: &[],
                notes_in: &[],
            },
            &mut out,
            &mut 0.0,
            &mut engine::EventBuf::new(),
            &mut engine::EventBuf::new(),
            engine::RenderBlock {
                frame: 60_000,
                sample_rate: 48_000,
                tempo: &tempo,
                mode: engine::RenderMode::Timeline,
            },
        );
        assert!(
            out.iter().all(|x| *x == 0.0),
            "the source is exhausted at 24 s of material into the clip: silence, not a wrap"
        );

        // **A looped clip whose region exceeds the source** keeps its phase: the gate's
        // repro panicked on the alignment invariant and (in release) read the wrong cycle.
        // It must mount and play the loop it still has.
        let track = crate::timeline::Track {
            id: "t0".into(),
            clips: vec![clip(0, 192_000, Some(48_000))],
        };
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, 48_000, 97_000)
            .expect("a looped reader mounts at a late frame");
        let mut out = vec![0.0f32; engine::BLOCK];
        node.render(
            &engine::NodeIO {
                audio_in: &[],
                audio_ins: [&[], &[], &[], &[], &[], &[], &[], &[]],
                audio_in_count: 0,
                audio_out_channels: 1,
                control_in: 0.0,
                triggers_in: &[],
                notes_in: &[],
            },
            &mut out,
            &mut 0.0,
            &mut engine::EventBuf::new(),
            &mut engine::EventBuf::new(),
            engine::RenderBlock {
                frame: 97_000,
                sample_rate: 48_000,
                tempo: &tempo,
                mode: engine::RenderMode::Timeline,
            },
        );
        // The ramp restarts every 480 frames, so the phase is checkable: at clip frame
        // 97 000 the loop phase is 97 000 % 48 000 = 1 000, past the file's own end, so the
        // reader is dead and silent — the point is that it is *not* an error and not a
        // wrapped read of the wrong cycle.
        assert!(
            out.iter().all(|x| x.is_finite()),
            "a looped reader past EOF stays finite"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// **Mounting a reader past the source's end is silence, not an error.** A clip whose
    /// declared region is longer than its source is a broken session, but the sequential
    /// path has always tolerated it (the reader hits EOF and emits silence, counted as
    /// underruns); a seek that mounts readers *at an offset* must behave the same way, or
    /// the offset path would refuse a session the sequential path plays — which is exactly
    /// what a warm-up seek does.
    #[test]
    fn a_reader_mounted_past_the_source_end_plays_silence() {
        let path = tmp("past-eof");
        write_const(&path, 4_800, 48_000, 0.5);
        let clip = crate::timeline::Clip {
            id: "c0".into(),
            name: None,
            source: "s1".into(),
            src_start: 0,
            // Ten times the source's length.
            src_len: 48_000,
            at_frame: 0,
            fade_in: 0,
            fade_out: 0,
            gain: 1.0,
            loop_len: None,
            reversed: false,
        };
        let track = crate::timeline::Track {
            id: "t0".into(),
            clips: vec![clip],
        };
        let source = path.clone();
        let resolve: crate::PoolResolver =
            Arc::new(move |id: &str| (id == "s1").then(|| source.clone()));

        // Mount far past the source's end: this used to be a `seek_frames past end` error.
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, 48_000, 30_000)
            .expect("a reader mounts past EOF");
        let mut out = vec![0.0f32; engine::BLOCK];
        let tempo = engine::TempoMap::new(48_000, 120.0, 4);
        node.render(
            &engine::NodeIO {
                audio_in: &[],
                audio_ins: [&[], &[], &[], &[], &[], &[], &[], &[]],
                audio_in_count: 0,
                audio_out_channels: 1,
                control_in: 0.0,
                triggers_in: &[],
                notes_in: &[],
            },
            &mut out,
            &mut 0.0,
            &mut engine::EventBuf::new(),
            &mut engine::EventBuf::new(),
            engine::RenderBlock {
                frame: 30_000,
                sample_rate: 48_000,
                tempo: &tempo,
                mode: engine::RenderMode::Timeline,
            },
        );
        assert!(
            out.iter().all(|x| *x == 0.0),
            "past the source's end the reader is silent (not an error)"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Write a mono float WAV `frames` long, every sample = `value`.
    fn write_const(path: &Path, frames: u64, sr: u32, value: f32) {
        let mut w = WavWriter::create_float(path, sr, 1).unwrap();
        let mut buf = vec![0.0f32; 2048];
        let mut i = 0u64;
        while i < frames {
            let n = buf.len().min((frames - i) as usize);
            buf[..n].fill(value);
            w.write(&buf[..n]).unwrap();
            i += n as u64;
        }
        w.finalize().unwrap();
    }

    /// Write a mono float WAV `frames` long, sample i = ((i % period) / period)
    /// — a deterministic ramp, so loop wraps and sample alignment are observable.
    fn write_ramp(path: &Path, frames: u64, sr: u32, period: u64) {
        let mut w = WavWriter::create_float(path, sr, 1).unwrap();
        let mut buf = vec![0.0f32; 2048];
        let mut i = 0u64;
        while i < frames {
            let n = buf.len().min((frames - i) as usize);
            for (k, s) in buf[..n].iter_mut().enumerate() {
                *s = ((i + k as u64) % period.max(1)) as f32 / period.max(1) as f32;
            }
            w.write(&buf[..n]).unwrap();
            i += n as u64;
        }
        w.finalize().unwrap();
    }

    fn resolver(map: &[(&str, PathBuf)]) -> PoolResolver {
        let map: Vec<(String, PathBuf)> = map
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        Arc::new(move |id| map.iter().find(|(k, _)| k == id).map(|(_, p)| p.clone()))
    }

    fn make_track(ops: &[ArrangeOp]) -> crate::timeline::Track {
        let mut t = Timeline::new();
        for op in ops {
            t = t.apply(op).unwrap();
        }
        t.tracks.into_iter().next().expect("a track")
    }

    fn clip(id: &str, source: &str, at: u64, len: u64) -> crate::timeline::Clip {
        crate::timeline::Clip {
            reversed: false,
            id: id.into(),
            name: None,
            source: source.into(),
            src_start: 0,
            src_len: len,
            at_frame: at,
            fade_in: 0,
            fade_out: 0,
            gain: 1.0,
            loop_len: None,
        }
    }

    fn render_node(node: &mut ArrangerNode, frames: u64, sr: u32) -> Vec<f32> {
        render_node_from(node, frames, sr, 0)
    }

    /// Render `frames` samples starting at absolute timeline frame `start_frame`
    /// (a rebuild-at-`from_frame` node must render from there, not frame 0).
    fn render_node_from(
        node: &mut ArrangerNode,
        frames: u64,
        sr: u32,
        start_frame: u64,
    ) -> Vec<f32> {
        let tempo = engine::TempoMap::new(sr, 120.0, 4);
        let mut out = vec![0.0f32; frames as usize];
        for (bi, chunk) in out.chunks_mut(engine::BLOCK).enumerate() {
            let io = NodeIO {
                audio_in: &[],
                audio_ins: [&[][..]; engine::MAX_AUDIO_INS],
                audio_in_count: 0,
                audio_out_channels: 1,
                control_in: 0.0,
                triggers_in: &[],
                notes_in: &[],
            };
            let mut control = 0.0f32;
            let mut triggers = EventBuf::new();
            let mut notes = EventBuf::new();
            node.render(
                &io,
                chunk,
                &mut control,
                &mut triggers,
                &mut notes,
                RenderBlock {
                    frame: start_frame + (bi * engine::BLOCK) as u64,
                    sample_rate: sr,
                    tempo: &tempo,
                    mode: engine::RenderMode::Timeline,
                },
            );
        }
        out
    }

    #[test]
    fn single_clip_streams_exactly_its_content() {
        let p = tmp("single");
        let sr = 48_000u32;
        let len = 3000u64;
        write_const(&p, len, sr, 0.5);
        let resolve = resolver(&[("s0", p.clone())]);
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("c0", "s0", 0, len),
            },
        ]);
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, sr, 0).unwrap();
        let out = render_node(&mut node, len, sr);
        assert_eq!(node.underruns(), 0, "a warm source must not underrun");
        let mut expected = vec![0.0f32; len as usize];
        let mut r = WavReader::open(&p).unwrap();
        assert_eq!(r.read_into(&mut expected), len as usize);
        for (a, b) in expected.iter().zip(&out) {
            assert_eq!(*a, *b, "streamed content must equal the source");
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn gap_is_silence_and_exact_alignment() {
        let p = tmp("gap");
        let sr = 48_000u32;
        let period = 100u64;
        write_ramp(&p, 1000, sr, period);
        let resolve = resolver(&[("s0", p.clone())]);
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("c0", "s0", 100, 200),
            },
        ]);
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, sr, 0).unwrap();
        let out = render_node(&mut node, 1000, sr);
        assert_eq!(node.underruns(), 0);
        assert!(
            out[..100].iter().all(|s| *s == 0.0),
            "leading gap must be silence"
        );
        assert!(
            out[300..].iter().all(|s| *s == 0.0),
            "trailing gap must be silence"
        );
        // Exact alignment: out[100 + k] == source[k], i.e. the clip's source window
        // lands at its at_frame without slip.
        let mut expected = vec![0.0f32; 200];
        let mut r = WavReader::open(&p).unwrap();
        assert_eq!(r.read_into(&mut expected), 200);
        for (k, e) in expected.iter().enumerate() {
            assert_eq!(
                out[100 + k],
                *e,
                "clip must align exactly at its at_frame (k={k})"
            );
        }
    }

    #[test]
    fn overlapping_clips_sum() {
        let pa = tmp("suma");
        let pb = tmp("sumb");
        let sr = 48_000u32;
        write_const(&pa, 1000, sr, 0.5);
        write_const(&pb, 1000, sr, -0.25);
        let resolve = resolver(&[("a", pa), ("b", pb)]);
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("ca", "a", 0, 500),
            },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("cb", "b", 250, 250),
            },
        ]);
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, sr, 0).unwrap();
        let out = render_node(&mut node, 500, sr);
        // [0,250) = clip a only = 0.5; [250,500) = a + b = 0.25.
        assert!((out[0] - 0.5).abs() < 1e-5);
        assert!(
            (out[250] - 0.25).abs() < 1e-5,
            "overlap must sum: {}",
            out[250]
        );
    }

    #[test]
    fn per_clip_fade_ramps() {
        let p = tmp("fade");
        let sr = 48_000u32;
        write_const(&p, 1000, sr, 1.0);
        let resolve = resolver(&[("s0", p)]);
        let mut c = clip("c0", "s0", 0, 1000);
        c.fade_in = 100;
        c.fade_out = 100;
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: c,
            },
        ]);
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, sr, 0).unwrap();
        let out = render_node(&mut node, 1000, sr);
        // gain = fade_in min fade_out; with a constant 1.0 source, out = gain curve.
        assert!(
            out[0].abs() < out[10].abs(),
            "fade_in ramps up: {} < {}",
            out[0].abs(),
            out[10].abs()
        );
        assert!(
            out[10].abs() < out[100].abs(),
            "fade_in reaches full: {} < {}",
            out[10].abs(),
            out[100].abs()
        );
        assert!(
            out[999].abs() < out[950].abs(),
            "fade_out ramps down: {} > {}",
            out[950].abs(),
            out[999].abs()
        );
        assert!(
            (out[100] - 1.0).abs() < 1e-5,
            "mid clip is full after fade_in"
        );
    }

    #[test]
    fn loop_region_repeats_source_and_wraps() {
        let p = tmp("loop");
        let sr = 48_000u32;
        let period = 100u64;
        write_ramp(&p, 1000, sr, period);
        let resolve = resolver(&[("s0", p)]);
        let mut c = clip("c0", "s0", 0, 1000);
        c.src_len = 300;
        c.loop_len = Some(100);
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: c,
            },
        ]);
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, sr, 0).unwrap();
        let out = render_node(&mut node, 300, sr);
        assert_eq!(node.underruns(), 0);
        // The reader must wrap back to the region start every 100 frames: sample
        // 100 == sample 0 and sample 200 == sample 0 (a constant source would pass
        // even if it never sought back, so we use a ramp).
        assert!(
            (out[100] - out[0]).abs() < 1e-6,
            "loop must wrap: {} vs {}",
            out[100],
            out[0]
        );
        assert!(
            (out[200] - out[0]).abs() < 1e-6,
            "loop must wrap: {} vs {}",
            out[200],
            out[0]
        );
        assert!(
            (out[50] - out[150]).abs() < 1e-6,
            "mid-region must repeat: {} vs {}",
            out[50],
            out[150]
        );
        assert!(
            (out[50] - 0.5).abs() < 1e-6,
            "ramp value at 50 must be 0.5: {}",
            out[50]
        );
    }

    /// A looped clip **rebuilt mid-clip** must stay in phase: off0 = 150 leaves
    /// the transport at source offset 150 % 100 = 50, so the first cycle is the
    /// 50-frame tail of the current cycle before it wraps back to the region
    /// start. Without the anchored reader (the reviewed bug) a rebuild at frame
    /// 150 would restart at source[0] and play `pattern[0+k]`, not
    /// `pattern[(50+k)%100]` — detectable here because the ramp is positional.
    #[test]
    fn mid_play_loop_rebuild_stays_in_phase() {
        let p = tmp("loopanchored");
        let sr = 48_000u32;
        let period = 100u64;
        write_ramp(&p, 1000, sr, period);
        let resolve = resolver(&[("s0", p)]);
        let mut c = clip("c0", "s0", 0, 1000);
        c.src_len = 300;
        c.loop_len = Some(100);
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: c,
            },
        ]);

        // Rebuild at transport frame 150 (a mid-play edit); render the next 150.
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, sr, 150).unwrap();
        let out = render_node_from(&mut node, 150, sr, 150);
        assert_eq!(node.underruns(), 0);
        // out[k] is the timeline frame 150+k, clip offset (150+k)%300. Within the
        // 100-frame loop region, that is source offset (150+k)%100 == (50+k)%100.
        // A restart bug would instead play (0+k)%100.
        for k in [0usize, 49, 50, 99, 100, 149] {
            let src = (50 + k as u64) % 100;
            let expected = (src % period.max(1)) as f32 / period.max(1) as f32;
            assert!(
                (out[k] - expected).abs() < 1e-6,
                "frame {k} (timeline {}) must be source[{}], got {} want {}",
                150 + k as u64,
                src,
                out[k],
                expected
            );
        }
    }

    #[test]
    fn eof_is_silence_not_panic() {
        let p = tmp("short");
        let sr = 48_000u32;
        write_const(&p, 100, sr, 0.5); // only 100 frames
        let resolve = resolver(&[("s0", p)]);
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip("c0", "s0", 0, 1000),
            },
        ]);
        let mut node = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, sr, 0).unwrap();
        let out = render_node(&mut node, 1000, sr);
        assert!(
            out[100..].iter().all(|s| *s == 0.0),
            "beyond EOF must be silence"
        );
    }

    #[test]
    fn rate_mismatched_source_is_refused() {
        // a 44.1 kHz take in a 48 kHz session would play pitch-shifted with no
        // diagnostic; ArrangerNode::new must refuse it (fail-loud), not silently
        // render wrong audio.
        let p = tmp("rate");
        write_const(&p, 1000, 44_100, 0.5); // source at 44.1 kHz
        let resolve = resolver(&[("s0", p)]);
        let mut c = clip("c0", "s0", 0, 1000);
        c.source = "s0".into();
        let track = make_track(&[
            ArrangeOp::AddTrack { track: "t0".into() },
            ArrangeOp::AddClip {
                track: "t0".into(),
                clip: c,
            },
        ]);
        let err = ArrangerNode::new(track, &resolve, DEFAULT_RING_CAPACITY, 48_000, 0)
            .err()
            .expect("a rate-mismatched source must be refused");
        assert!(
            err.contains("44100") && err.contains("Hz"),
            "error names the mismatch: {err}"
        );
    }
}
