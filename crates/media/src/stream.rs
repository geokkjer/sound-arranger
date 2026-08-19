//! The long-file player and splice-during-playback (Spike B note, `stream`).
//!
//! [`FilePlayer`] is a clip's reader: a thread reads the WAV from disk into an
//! SPSC ring (started and warmed on the *control side* — command-issue time).
//! [`PlaybackNode`] is the graph's opaque node: it pops the ring into its
//! `out("audio")` port, zero-fills and counts underruns, and applies
//! [`SpliceCmd`]s sample-accurately inside the block: an equal-power crossfade
//! from the current source to the incoming clip over `crossfade` samples at
//! the requested absolute frame. The render path only pops rings and mixes;
//! all allocation and thread spawning happened when the command was issued,
//! and a retired reader is *detached*, never joined, on the render path.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use engine::{AudioNode, EventBuf, NodeIO, NoteEvent, RenderBlock, Trigger, CAP_EVENTS};

use crate::ring::Spsc;
use crate::wav::WavReader;

/// Default SPSC ring capacity (samples) for a player/recorder. 64 KiB of
/// read-ahead at 48 kHz ≈ 1.37 s — far beyond any disk stall a test provokes.
pub const DEFAULT_RING_CAPACITY: usize = 1 << 16;

/// A named region of a WAV file — the clip seed (an ACID clip is path +
/// region; the visual clip model is the substrate, musical-event note).
#[derive(Debug, Clone)]
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
        Ok(ClipRef { path, start: 0, len: reader.total_frames() })
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
        let ring = Arc::new(Spsc::new(ring_capacity));
        let stop = Arc::new(AtomicBool::new(false));
        let eof = Arc::new(AtomicBool::new(false));
        let produced = Arc::new(AtomicU64::new(0));

        let mut reader = WavReader::open(&clip.path)?;
        reader.seek_frames(clip.start)?;
        let want = clip.len;
        let (ring2, stop2, eof2, produced2) = (ring.clone(), stop.clone(), eof.clone(), produced.clone());

        let handle = std::thread::Builder::new()
            .name("media-player".into())
            .spawn(move || {
                let mut buf = [0.0f32; 1024];
                let mut remaining = want;
                while remaining > 0 {
                    if stop2.load(Ordering::Acquire) {
                        return;
                    }
                    let want_now = buf.len().min(remaining as usize);
                    let n = reader.read_into(&mut buf[..want_now]);
                    if n == 0 {
                        break; // EOF (or clipped region end)
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
                }
                eof2.store(true, Ordering::Release);
            })
            .map_err(|e| format!("player thread spawn: {e}"))?;

        Ok(FilePlayer { handle: Some(handle), ring, stop, eof, produced, expected: want })
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

    /// Frames the clip is supposed to deliver (clip.len).
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
    /// crossfade length in samples (equal-power)
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
            // equal-power crossfade: t runs 0 → 1 across the window, so the
            // last fade sample is pure incoming (no gain step at the end).
            let pos = fade.total - fade.remaining;
            let denom = (fade.total - 1).max(1) as f32;
            let t = pos as f32 / denom;
            let g_cur = (std::f32::consts::FRAC_PI_2 * t).cos();
            let g_in = (std::f32::consts::FRAC_PI_2 * t).sin();
            let a = match &mut fade.cur {
                Some(cur) => pop_sample(cur, &self.underruns),
                None => 0.0,
            };
            let b = pop_sample(&mut fade.incoming, &self.underruns);
            *sample = a * g_cur + b * g_in;
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
    use std::path::Path;
    use crate::wav::WavWriter;

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

    fn block<'a>(sr: u32, frame: u64, tempo: &'a engine::TempoMap) -> RenderBlock<'a> {
        RenderBlock { frame, sample_rate: sr, tempo }
    }

    #[test]
    fn player_streams_whole_clip_and_goes_silent() {
        let path = tmp("whole");
        let sr = 48_000u32;
        let frames = sr as u64; // 1 s
        write_tone(&path, 440.0, frames, sr);

        let player = FilePlayer::start(ClipRef::whole(&path).unwrap(), DEFAULT_RING_CAPACITY).unwrap();
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
                audio_ins: [&[][..]; engine::MAX_AUDIO_INS],
                audio_in_count: 0,
                control_in: 0.0,
                triggers_in: &[],
                notes_in: &[],
            };
            let mut control = 0.0f32;
            let mut triggers = EventBuf::new();
            let mut notes = EventBuf::new();
            node.render(&io, chunk, &mut control, &mut triggers, &mut notes, block(sr, (bi * engine::BLOCK) as u64, &tempo));
        }

        assert_eq!(node.underruns(), 0, "a warm, cached file must not underrun");
        assert_eq!(node.deferred(), 0, "nothing deferred");

        let mut expected = vec![0.0f32; frames as usize];
        let mut reader = WavReader::open(&path).unwrap();
        assert_eq!(reader.read_into(&mut expected), frames as usize);
        for (a, b) in expected.iter().zip(&out[..frames as usize]) {
            assert_eq!(*a, *b, "streamed content must equal the file sample-for-sample");
        }
        assert!(out[frames as usize..].iter().all(|s| *s == 0.0), "tail must be silence");

        let _ = std::fs::remove_file(&path);
    }
}
