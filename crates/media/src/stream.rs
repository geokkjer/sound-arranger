//! The long-file player and splice-during-playback (Spike B note, `stream`).
//!
//! [`FilePlayer`] is a clip's reader: a thread reads the WAV from disk into an
//! SPSC ring (started and warmed on the *control side* — command-issue time).
//! [`PlaybackNode`] is the graph's opaque node: it pops the ring into its
//! `out("audio")` port, zero-fills and counts underruns, and applies
//! [`SpliceCmd`]s sample-accurately inside the block: an equal-power crossfade
//! from the current source to the incoming clip over `crossfade` samples at
//! the requested absolute frame (a `crossfade` too short to mix in is a cut).
//! The render path only pops rings and mixes; all allocation and thread spawning
//! happened when the command was issued, and a retired reader is *detached*,
//! never joined, on the render path.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use engine::{AudioNode, CAP_EVENTS, EventBuf, NodeIO, NoteEvent, RenderBlock, Trigger};

use crate::ring::Spsc;
use crate::wav::WavReader;

/// Default SPSC ring capacity (samples) for a player/recorder. 64 KiB of
/// read-ahead at 48 kHz ≈ 1.37 s — far beyond any disk stall a test provokes.
pub const DEFAULT_RING_CAPACITY: usize = 1 << 16;

/// A named region of a WAV file — the clip seed (an ACID clip is path +
/// region; the visual clip model is the substrate, musical-event note).
/// `PartialEq` so a `host v1` command that carries one can round-trip through the
/// text form in a test.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipRef {
    pub path: PathBuf,
    /// first frame of the region within the file's data chunk
    pub start: u64,
    /// region length in frames
    pub len: u64,
}

impl ClipRef {
    /// The whole file as a clip (start 0, len = file length).
    pub fn whole(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let reader = WavReader::open(&path)?;
        Ok(ClipRef {
            path,
            start: 0,
            len: reader.total_frames(),
        })
    }
}

/// A clip's reader thread + ring. Constructed (and the ring warmed) on the
/// control side; the audio path only ever pops the ring. On drop the reader
/// is *detached* (never joined) so a player retired by a splice cannot block
/// the render path — the reader owns clones of every `Arc` it touches, sees
/// `stop` within ~50 µs, and frees its refs on its own thread.
pub struct FilePlayer {
    handle: Option<JoinHandle<()>>,
    ring: Arc<Spsc<f32>>,
    stop: Arc<AtomicBool>,
    eof: Arc<AtomicBool>,
    produced: Arc<AtomicU64>,
    expected: u64,
}

impl FilePlayer {
    /// Start the reader thread for `clip`. Control side only: allocates,
    /// opens the file, spawns a thread, begins filling the ring immediately.
    pub fn start(clip: ClipRef, ring_capacity: usize) -> Result<Self, String> {
        Self::start_looped(clip, None, ring_capacity)
    }

    /// Start the reader for `clip`, optionally looping: when `loop_len` is
    /// `Some(r > 0)`, the reader reads `r` frames from `clip.start`, seeks back
    /// to `clip.start`, and repeats — until it has produced `clip.len` frames
    /// total (so a baked loop with `clip.len == r * times` wraps). `Some(0)` or
    /// `None` reads `clip.len` contiguous frames once. Control side only.
    pub fn start_looped(
        clip: ClipRef,
        loop_len: Option<u64>,
        ring_capacity: usize,
    ) -> Result<Self, String> {
        Self::start_looped_anchored(clip, loop_len, ring_capacity, 0)
    }

    /// Start the reader at a mid-clip offset. `off0` is frames already consumed
    /// of the clip (an arranger node rebuilt at the current transport frame),
    /// so the reader produces the *remaining* `clip.len - off0` frames starting
    /// at the source position `clip.start + (off0 % region_len)` — the phase the
    /// transport is at. A looped clip's first cycle is `region_len - phase`
    /// frames (the current cycle's tail) before it wraps back to `clip.start`,
    /// which keeps a mid-clip loop in phase instead of "walking" the region.
    /// Control side only.
    pub fn start_looped_anchored(
        clip: ClipRef,
        loop_len: Option<u64>,
        ring_capacity: usize,
        off0: u64,
    ) -> Result<Self, String> {
        let region_len = loop_len.filter(|r| *r > 0).unwrap_or(clip.len);
        let phase = if region_len == 0 {
            0
        } else {
            off0 % region_len
        };
        let start = clip
            .start
            .checked_add(phase)
            .ok_or("clip start overflows with loop phase")?;
        let want = clip.len.saturating_sub(off0);
        let looping = loop_len.is_some_and(|r| r > 0);

        let ring = Arc::new(Spsc::new(ring_capacity.max(1)));
        let stop = Arc::new(AtomicBool::new(false));
        let eof = Arc::new(AtomicBool::new(false));
        let produced = Arc::new(AtomicU64::new(0));

        let mut reader = WavReader::open(&clip.path)?;
        reader.seek_frames(start)?;
        let (ring2, stop2, eof2, produced2) =
            (ring.clone(), stop.clone(), eof.clone(), produced.clone());

        let handle = std::thread::Builder::new()
            .name("media-player".into())
            .spawn(move || {
                let mut buf = [0.0f32; 1024];
                let mut remaining = want;
                let mut produced_in_cycle = 0u64;
                // The first cycle is the tail of the current (mid-clip) cycle:
                // `region_len - phase` frames, then full `region_len` cycles.
                let mut cycle_len = if looping && region_len > 0 {
                    (region_len - phase).max(1)
                } else {
                    want
                };
                while remaining > 0 {
                    if stop2.load(Ordering::Acquire) {
                        return;
                    }
                    let budget = (cycle_len - produced_in_cycle).min(remaining);
                    let want_now = buf.len().min(budget as usize);
                    let n = reader.read_into(&mut buf[..want_now]);
                    if n == 0 {
                        // EOF before the region finished: the source is shorter than
                        // the clip claimed — stop (render treats it as silence). A
                        // loop that runs out is also done; no silent repeat ad infinitum.
                        break;
                    }
                    let mut i = 0;
                    while i < n {
                        if stop2.load(Ordering::Acquire) {
                            return;
                        }
                        if ring2.try_push(buf[i]) {
                            i += 1;
                            produced2.fetch_add(1, Ordering::Relaxed);
                        } else {
                            // Ring full: the consumer is behind; wait off the
                            // audio path. 50 µs is fine here.
                            std::thread::sleep(std::time::Duration::from_micros(50));
                        }
                    }
                    remaining -= n as u64;
                    produced_in_cycle += n as u64;
                    if looping && produced_in_cycle >= cycle_len {
                        produced_in_cycle = 0;
                        cycle_len = region_len.max(1);
                        if reader.seek_frames(clip.start).is_err() {
                            break; // seek failure: stop rather than loop forever
                        }
                    }
                }
                eof2.store(true, Ordering::Release);
                // Park at EOF, *holding our Arc clones*, until the player is
                // dropped (its `Drop` sets `stop`). The retires a finished clip
                // on the render thread (`self.cur = Some(finished.incoming)` drops
                // the old player) — if the reader had already exited here, the
                // 256 KiB ring + Arcs would free ON THE RENDER THREAD, violating
                // the no-alloc-on-render invariant. Parking defers that free to
                // this reader thread's own exit, off the audio path.
                while !stop2.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            })
            .map_err(|e| format!("player thread spawn: {e}"))?;

        Ok(FilePlayer {
            handle: Some(handle),
            ring,
            stop,
            eof,
            produced,
            expected: want,
        })
    }

    pub fn ring(&self) -> &Spsc<f32> {
        &self.ring
    }

    /// True once the region has been fully read into the ring.
    pub fn eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }

    /// Frames produced into the ring so far (monotonic; for control-side
    /// warm-up checks — the render path does not read this).
    pub fn produced(&self) -> u64 {
        self.produced.load(Ordering::Relaxed)
    }

    /// Frames the clip is supposed to deliver. For an anchored player this is
    /// the *remaining* count (`clip.len - off0`); a plain player delivers the
    /// whole region (`clip.len`).
    pub fn expected(&self) -> u64 {
        self.expected
    }
}

impl Drop for FilePlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Detach, never join: a player retired by a splice completes on the
        // render path, and join() would block it. The reader thread exits on
        // its own and frees its ring refs on its own thread.
        self.handle.take();
    }
}

/// A control→render splice command. The incoming [`FilePlayer`] is constructed
/// (thread + warm ring) on the control side at issue time; the render path
/// only mixes two rings. The shared mailbox is the spike's miniature
/// control→render handoff (Spike B note, "The splice command seam").
pub struct SpliceCmd {
    /// absolute frame at which the crossfade begins
    pub at_frame: u64,
    /// the clip to fade into
    pub incoming: FilePlayer,
    /// crossfade length in samples (equal-power). A window too short to mix in
    /// — `0`, or the `1` it is read as — is a **hard cut**: the incoming clip's
    /// first sample *is* the frame at `at_frame`, at gain 1, so no part of it
    /// is faded in or lost.
    pub crossfade: u32,
}

/// The mailboxes a rig hands to its nodes (shared so the control side can push
/// commands while the graph owns the nodes).
pub type Mailbox = Arc<Mutex<VecDeque<SpliceCmd>>>;

pub fn mailbox() -> Mailbox {
    Arc::new(Mutex::new(VecDeque::new()))
}

/// A clip's playback state on the node: the reader plus how many frames the
/// node has popped. `popped` is the consumer-side source of truth for EOF —
/// ring empty and `popped >= expected` (or the reader reported EOF) is
/// legitimate silence, not an underrun. Reading the reader's `produced` would
/// race its final `fetch_add`, and a hand-built `ClipRef` longer than the file
/// would never reach `expected`; `eof()` covers that case.
struct Player {
    player: FilePlayer,
    popped: u64,
}

impl Player {
    fn new(player: FilePlayer) -> Self {
        Player { player, popped: 0 }
    }
}

/// Pop one sample, counting starvation as an underrun but treating a finished
/// clip as legitimate silence.
fn pop_sample(player: &mut Player, underruns: &AtomicU64) -> f32 {
    match player.player.ring().try_pop() {
        Some(v) => {
            player.popped += 1;
            v
        }
        None => {
            if player.player.eof() || player.popped >= player.player.expected() {
                0.0 // legitimate silence: clip done
            } else {
                underruns.fetch_add(1, Ordering::Relaxed);
                0.0
            }
        }
    }
}

struct Fade {
    /// None = fade from silence (a splice with no current source must not
    /// panic the audio thread).
    cur: Option<Player>,
    incoming: Player,
    remaining: u32,
    total: u32,
    /// samples into the current block before the fade starts (first block only)
    offset: u32,
}

/// The playback node: `out("audio")`. Pops the current clip's ring; applies
/// due splices sample-accurately inside the block with an equal-power
/// crossfade. Underruns and deferred (late) applications are counted on shared
/// counters so the control side (and tests) can observe them.
pub struct PlaybackNode {
    cur: Option<Player>,
    mailbox: Mailbox,
    pending: VecDeque<SpliceCmd>,
    fade: Option<Fade>,
    underruns: Arc<AtomicU64>,
    deferred: Arc<AtomicU64>,
}

impl PlaybackNode {
    pub fn new(initial: Option<FilePlayer>, mailbox: Mailbox) -> Self {
        PlaybackNode {
            cur: initial.map(Player::new),
            mailbox,
            // Bounded command buffer: pushes beyond the preallocated capacity
            // would allocate on the render path (a documented Phase-0 edge —
            // the profile schedules splices on clean boundaries, so bursts
            // beyond 8 are not expected).
            pending: VecDeque::with_capacity(8),
            fade: None,
            underruns: Arc::new(AtomicU64::new(0)),
            deferred: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn mailbox(&self) -> &Mailbox {
        &self.mailbox
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Splices whose frame had already passed when applied (a missed mailbox
    /// `try_lock`, or a command arriving mid-fade) — observable, not silent.
    pub fn deferred(&self) -> u64 {
        self.deferred.load(Ordering::Relaxed)
    }

    /// Clones of the shared counters — the rig keeps one to assert
    /// glitch-freedom after the node lives inside the engine graph.
    pub fn underrun_counter(&self) -> Arc<AtomicU64> {
        self.underruns.clone()
    }

    pub fn deferred_counter(&self) -> Arc<AtomicU64> {
        self.deferred.clone()
    }

    /// Drain the mailbox into `pending` (try_lock: the render path never
    /// blocks on it). Called once per block.
    fn drain_mailbox(&mut self) {
        if let Ok(mut mbox) = self.mailbox.try_lock() {
            while let Some(cmd) = mbox.pop_front() {
                self.pending.push_back(cmd);
            }
        }
    }
}

impl AudioNode for PlaybackNode {
    fn latency(&self) -> u32 {
        0 // ring read-ahead is a stream buffer, not processing latency (note, findings)
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
        self.drain_mailbox();
        let f0 = block.frame;
        let f1 = f0 + out.len() as u64;

        // Apply at most one command per block: a splice arriving while a
        // crossfade is running waits for it to complete, and two commands due
        // in one block apply one per block, in order (the second is not
        // silently dropped — it stays pending). A command whose frame already
        // passed is counted as deferred and applies at block start.
        if self.fade.is_none()
            && let Some(cmd) = self.pending.pop_front_if(|cmd| cmd.at_frame < f1)
        {
            if cmd.at_frame < f0 {
                self.deferred.fetch_add(1, Ordering::Relaxed);
            }
            let offset = cmd.at_frame.saturating_sub(f0).min(out.len() as u64) as u32;
            // A crossfade too short to mix in (0, or the 1 it is clamped to below)
            // is a **cut**, and the render loop reads a one-sample window as one.
            // `max(1)` is here so `remaining` cannot underflow below.
            let crossfade = cmd.crossfade.max(1);
            self.fade = Some(Fade {
                cur: self.cur.take(), // None → fade from silence
                incoming: Player::new(cmd.incoming),
                remaining: crossfade,
                total: crossfade,
                offset,
            });
        }

        for (i, sample) in out.iter_mut().enumerate() {
            if self.fade.is_none() {
                match &mut self.cur {
                    Some(cur) => *sample = pop_sample(cur, &self.underruns),
                    None => *sample = 0.0,
                }
                continue;
            }
            let fade = self.fade.as_mut().expect("checked");
            if (i as u32) < fade.offset {
                // pre-fade window: pure current source
                match &mut fade.cur {
                    Some(cur) => *sample = pop_sample(cur, &self.underruns),
                    None => *sample = 0.0,
                }
                continue;
            }
            // A window of one sample is a **cut**, not a crossfade: equal-power has
            // no room to mix there, and `t` would be 0, so the incoming clip's
            // first sample — the whole of a one-frame clip — would be multiplied
            // by zero and lost. So the window's single sample is the incoming's
            // first at gain 1, and the outgoing's last is the frame before it.
            let b = pop_sample(&mut fade.incoming, &self.underruns);
            if fade.total <= 1 {
                *sample = b;
            } else {
                // equal-power crossfade: t runs 0 → 1 across the window, so the
                // last fade sample is pure incoming (no gain step at the end).
                let pos = fade.total - fade.remaining;
                let t = pos as f32 / (fade.total - 1) as f32;
                let g_cur = (std::f32::consts::FRAC_PI_2 * t).cos();
                let g_in = (std::f32::consts::FRAC_PI_2 * t).sin();
                let a = match &mut fade.cur {
                    Some(cur) => pop_sample(cur, &self.underruns),
                    None => 0.0,
                };
                *sample = a * g_cur + b * g_in;
            }
            fade.remaining -= 1;
            if fade.remaining == 0 {
                let finished = self.fade.take().expect("fade in progress");
                self.cur = Some(finished.incoming); // finished.cur detached → its reader stops
            }
        }

        // The offset only applies to the block that began the fade.
        if let Some(fade) = &mut self.fade {
            fade.offset = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::WavWriter;
    use std::path::Path;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("media-stream-{name}-{}.wav", std::process::id()))
    }

    fn write_tone(path: &Path, freq: f32, frames: u64, sr: u32) {
        let mut w = WavWriter::create(path, sr, 1).unwrap();
        let mut buf = vec![0.0f32; 2048];
        let mut phase = 0.0f64;
        let mut left = frames;
        while left > 0 {
            let n = buf.len().min(left as usize);
            for s in &mut buf[..n] {
                *s = (std::f64::consts::TAU * phase).sin() as f32 * 0.5;
                phase += freq as f64 / sr as f64;
            }
            w.write(&buf[..n]).unwrap();
            left -= n as u64;
        }
        w.finalize().unwrap();
    }

    /// A **position-revealing** float WAV: sample k = (k % period) / period. A
    /// reader that produces the wrong source region (a mid-clip rebuild starting
    /// at source[0]) is detectable because each offset has a distinct value.
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

    /// A position-revealing ramp as `write_ramp` makes it, but phase-shifted by
    /// `shift`, so its **first sample is not `0.0`**. A clip that starts on zero
    /// is indistinguishable from silence, which is exactly the mistake these
    /// cut tests must not be able to make.
    fn write_shifted_ramp(path: &Path, frames: u64, sr: u32, period: u64, shift: u64) {
        let mut w = WavWriter::create_float(path, sr, 1).unwrap();
        let mut buf = vec![0.0f32; 2048];
        let mut i = 0u64;
        while i < frames {
            let n = buf.len().min((frames - i) as usize);
            for (k, s) in buf[..n].iter_mut().enumerate() {
                *s = ((i + k as u64 + shift) % period.max(1)) as f32 / period.max(1) as f32;
            }
            w.write(&buf[..n]).unwrap();
            i += n as u64;
        }
        w.finalize().unwrap();
    }

    /// The value the ring contains for `k`-th production of an anchored reader.
    fn ramp_at(frames: u64, period: u64) -> f32 {
        (frames % period.max(1)) as f32 / period.max(1) as f32
    }

    /// Drain `count` samples from a FilePlayer's ring (test helper; the reader
    /// thread fills it as we pop).
    fn drain(player: &FilePlayer, count: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; count];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        for s in out.iter_mut() {
            loop {
                if let Some(v) = player.ring().try_pop() {
                    *s = v;
                    break;
                }
                if std::time::Instant::now() > deadline {
                    panic!("reader did not deliver in time");
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        out
    }

    fn block<'a>(sr: u32, frame: u64, tempo: &'a engine::TempoMap) -> RenderBlock<'a> {
        RenderBlock {
            frame,
            sample_rate: sr,
            tempo,
            mode: engine::RenderMode::Timeline,
        }
    }

    /// Read `frames` samples from a WAV (the files under test are short enough to
    /// be one whole read).
    fn read_file(path: &Path, frames: usize) -> Vec<f32> {
        let mut reader = WavReader::open(path).unwrap();
        let mut out = vec![0.0f32; frames];
        assert_eq!(reader.read_into(&mut out), frames, "read the whole file");
        out
    }

    /// Wait until the reader has produced its whole clip into the ring, so a
    /// render cannot underrun for want of data (deterministic, not a fixed sleep).
    fn warm(player: &FilePlayer) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while player.produced() < player.expected() && !player.eof() {
            assert!(
                std::time::Instant::now() < deadline,
                "reader did not fill its ring in time"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// A `PlaybackNode` playing all of `a`, with a splice of all of `b` waiting
    /// in its mailbox for frame `f`. Both rings are warm before it returns.
    fn splice_node(a: &Path, b: &Path, f: u64, crossfade: u32) -> PlaybackNode {
        let cur = FilePlayer::start(ClipRef::whole(a).unwrap(), DEFAULT_RING_CAPACITY).unwrap();
        warm(&cur);
        let incoming =
            FilePlayer::start(ClipRef::whole(b).unwrap(), DEFAULT_RING_CAPACITY).unwrap();
        warm(&incoming);
        let node = PlaybackNode::new(Some(cur), mailbox());
        node.mailbox().lock().unwrap().push_back(SpliceCmd {
            at_frame: f,
            incoming,
            crossfade,
        });
        node
    }

    /// Render `frames` through a `PlaybackNode` in `engine::BLOCK` chunks — the
    /// node is mounted opaque, so the graph would hand it exactly this one-block
    /// `NodeIO`.
    fn render_node(node: &mut PlaybackNode, frames: usize, sr: u32) -> Vec<f32> {
        let tempo = engine::TempoMap::new(sr, 120.0, 4);
        let mut out = vec![0.0f32; frames];
        let mut control = 0.0f32;
        let mut triggers = EventBuf::new();
        let mut notes = EventBuf::new();
        for (bi, chunk) in out.chunks_mut(engine::BLOCK).enumerate() {
            let io = NodeIO {
                audio_in: &[],
                audio_ins: engine::AudioInputs::none(),
                audio_in_count: 0,
                audio_out_channels: 1,
                frames: chunk.len(),
                control_in: 0.0,
                triggers_in: &[],
                notes_in: &[],
            };
            node.render(
                &io,
                chunk,
                &mut control,
                &mut triggers,
                &mut notes,
                block(sr, (bi * engine::BLOCK) as u64, &tempo),
            );
        }
        out
    }

    #[test]
    fn player_streams_whole_clip_and_goes_silent() {
        let path = tmp("whole");
        let sr = 48_000u32;
        let frames = sr as u64; // 1 s
        write_tone(&path, 440.0, frames, sr);

        let player =
            FilePlayer::start(ClipRef::whole(&path).unwrap(), DEFAULT_RING_CAPACITY).unwrap();
        let mut node = PlaybackNode::new(Some(player), mailbox());
        std::thread::sleep(std::time::Duration::from_millis(50)); // warm the ring

        // Render the clip length + one extra block: content must match the
        // file, the tail must be silence, and nothing may underrun.
        let tempo = engine::TempoMap::new(sr, 120.0, 4);
        let total = frames as usize + engine::BLOCK;
        let mut out = vec![0.0f32; total];
        for (bi, chunk) in out.chunks_mut(engine::BLOCK).enumerate() {
            let io = NodeIO {
                audio_in: &[],
                audio_ins: engine::AudioInputs::none(),
                audio_in_count: 0,
                audio_out_channels: 1,
                frames: engine::BLOCK,
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
                block(sr, (bi * engine::BLOCK) as u64, &tempo),
            );
        }

        assert_eq!(node.underruns(), 0, "a warm, cached file must not underrun");
        assert_eq!(node.deferred(), 0, "nothing deferred");

        let mut expected = vec![0.0f32; frames as usize];
        let mut reader = WavReader::open(&path).unwrap();
        assert_eq!(reader.read_into(&mut expected), frames as usize);
        for (a, b) in expected.iter().zip(&out[..frames as usize]) {
            assert_eq!(
                *a, *b,
                "streamed content must equal the file sample-for-sample"
            );
        }
        assert!(
            out[frames as usize..].iter().all(|s| *s == 0.0),
            "tail must be silence"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// An anchored (rebuilt-mid-clip) reader must produce the *remaining* region
    /// of the clip starting at `start + off0`, not the clip's beginning — the
    /// critical bug the review found (a rebuild at transport frame F had started
    /// every reader at source[0], playing the wrong region and tripping the debug
    /// assert on clips longer than the ring).
    #[test]
    fn anchored_reader_continues_from_off0_not_the_start() {
        let path = tmp("anchored");
        let sr = 48_000u32;
        let len = 8000u64;
        let period = 257u64;
        write_ramp(&path, len, sr, period);

        let clip = ClipRef {
            path: path.clone(),
            start: 0,
            len,
        };
        let off0 = 4000u64;
        let player =
            FilePlayer::start_looped_anchored(clip, None, DEFAULT_RING_CAPACITY, off0).unwrap();
        // The reader must deliver `len - off0` frames, each equal to the source
        // value at `off0 + k` (NOT at `k`).
        let got = drain(&player, (len - off0) as usize);
        for (k, &v) in got.iter().enumerate() {
            let want = ramp_at(off0 + k as u64, period);
            assert_eq!(
                v, want,
                "frame {k} must be source[{off0}+{k}], got {v} want {want}"
            );
        }
        // The reader's own `expected` is the remaining count, so it reports done
        // exactly when the tail is consumed — not sooner (which would wrongly
        // treat a long clip still playing as silence).
        assert_eq!(player.expected(), len - off0);
        // After the tail, the ring runs dry (silence, not an underrun): eof is set.
        assert!(
            player.eof(),
            "the remaining region must be all the reader produces"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A looped anchored reader must stay **in phase**: off0 frames into the
    /// clip lands mid-cycle, the first cycle is the current cycle's tail, and the
    /// wrap-back is to the clip's true source region start (not the offset start,
    /// which would "walk" the region each cycle).
    #[test]
    fn anchored_loop_wraps_to_the_true_region_start() {
        let path = tmp("anchoredloop");
        let sr = 48_000u32;
        let period = 64u64;
        let region = 1000u64;
        let times = 8u64;
        let len = region * times; // 8000, loop reads `region` then wraps
        write_ramp(&path, len.max(region + region), sr, period); // enough for a loop

        let clip = ClipRef {
            path: path.clone(),
            start: 0,
            len,
        };
        // off0 = 2500 is 2 full region-cycles (2000) + 500 into the third, so it
        // starts at source[500] and the first cycle is 500 frames (region - phase).
        let off0 = 2500u64;
        let player =
            FilePlayer::start_looped_anchored(clip, Some(region), DEFAULT_RING_CAPACITY, off0)
                .unwrap();
        let got = drain(&player, (len - off0) as usize);
        for (k, &v) in got.iter().enumerate() {
            let src = (off0 + k as u64) % region; // wraps within the region
            let want = ramp_at(src, period);
            assert_eq!(
                v, want,
                "frame {k} must be source[{off0}+{k}] wrapped to [{src}], got {v} want {want}"
            );
        }
        assert_eq!(player.expected(), len - off0);
        assert!(
            player.eof(),
            "the looped remainder must be all the reader produces"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A crossfade too short to mix in — `0`, or the `1` it is read as — is a
    /// **hard cut**, sample-accurate at the requested frame: pure A before it,
    /// the incoming clip's **first** sample *at* it, then the incoming clip
    /// running on unshifted. Equal-power has no room to mix in a one-sample
    /// window (`t` is 0 there), so the old form multiplied the incoming's first
    /// sample by zero, emitted the cut one sample late and shifted the whole
    /// incoming clip by one.
    #[test]
    fn a_crossfade_too_short_to_mix_is_a_sample_accurate_cut() {
        let a = tmp("cut-a");
        let b = tmp("cut-b");
        let sr = 48_000u32;
        let f = 1000usize; // mid-block: the cut must land on this exact frame
        let total = 2048usize;
        // Two position-revealing clips that share no value at the seam: A starts
        // its own ramp, B's is phase-shifted, so A[f] and B[0] differ and neither
        // is zero.
        write_shifted_ramp(&a, total as u64, sr, 257, 0);
        write_shifted_ramp(&b, total as u64, sr, 251, 100);
        let fa = read_file(&a, total);
        let fb = read_file(&b, total);
        assert_ne!(
            fa[f], fb[0],
            "the test is only honest if A's frame at the cut differs from B's first"
        );

        for &crossfade in &[0u32, 1] {
            let mut node = splice_node(&a, &b, f as u64, crossfade);
            let out = render_node(&mut node, total, sr);
            assert_eq!(
                node.underruns(),
                0,
                "warm rings must not underrun (crossfade {crossfade})"
            );
            assert_eq!(
                node.deferred(),
                0,
                "the cut applies at its exact frame (crossfade {crossfade})"
            );
            assert_eq!(
                out[..f],
                fa[..f],
                "before the cut: pure A (crossfade {crossfade})"
            );
            assert_eq!(
                out[f], fb[0],
                "the cut frame is the incoming's FIRST sample, not A's (crossfade {crossfade})"
            );
            assert_eq!(
                out[f + 1..],
                fb[1..total - f],
                "after the cut: B from its second sample on, unshifted (crossfade {crossfade})"
            );
        }

        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    /// The sharpest form of the same defect: an incoming clip of **one frame**
    /// has nothing but its first sample, so a fade that starts at `t = 0`
    /// silenced it for its entire life — the sample was consumed by the window
    /// and multiplied by zero, the ring then ran dry with `eof` set, and every
    /// later `pop_sample` returned legitimate silence. A cut plays it.
    #[test]
    fn a_one_frame_incoming_clip_is_audible_through_a_cut() {
        let a = tmp("cut1-a");
        let b = tmp("cut1-b");
        let sr = 48_000u32;
        let f = 700usize; // mid-block again
        let total = 1024usize;
        write_shifted_ramp(&a, total as u64, sr, 257, 0);
        let mut w = WavWriter::create_float(&b, sr, 1).unwrap();
        w.write(&[0.75]).unwrap(); // one frame, a value nothing else in the test has
        w.finalize().unwrap();

        let mut node = splice_node(&a, &b, f as u64, 0);
        let out = render_node(&mut node, total, sr);

        assert_eq!(node.underruns(), 0, "warm rings must not underrun");
        assert_eq!(node.deferred(), 0, "the cut applies at its exact frame");
        assert_eq!(
            out[f], 0.75,
            "a one-frame clip must be audible, not swallowed by the cut"
        );
        assert!(
            out[f + 1..].iter().all(|s| *s == 0.0),
            "then silence: the one-frame clip is over, and that is not an underrun"
        );

        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }
}
