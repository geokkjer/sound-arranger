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
    let incoming = FilePlayer::start(ClipRef::whole(b).unwrap(), DEFAULT_RING_CAPACITY).unwrap();
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
        FilePlayer::start_looped_anchored(clip, Some(region), DEFAULT_RING_CAPACITY, off0).unwrap();
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
